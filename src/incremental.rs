use anyhow::Result;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path};

use crate::cli::account::{open_database, reader_error};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Position {
    generation: String,
    has_table: bool,
    first: Option<(i64, String)>,
    local_id: i64,
    server_id: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    version: u32,
    sequence: u64,
    account: String,
    chat: String,
    table: String,
    shards: BTreeMap<String, Position>,
}

impl Cursor {
    fn decode(token: &str, account: &str, chat: &str) -> Result<Self> {
        let table = format!("Msg_{:x}", md5::compute(chat));
        if token.is_empty() {
            return Ok(Self {
                version: 1,
                sequence: 0,
                account: account.into(),
                chat: chat.into(),
                table,
                shards: BTreeMap::new(),
            });
        }
        let invalid = || reader_error("CURSOR_INVALID", "游标格式、版本或字段非法");
        if token.len() > 1_048_576 {
            return Err(invalid());
        }
        let bytes = URL_SAFE_NO_PAD.decode(token).map_err(|_| invalid())?;
        let cursor: Self = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        if cursor.version != 1
            || cursor.table != format!("Msg_{:x}", md5::compute(&cursor.chat))
            || cursor
                .shards
                .values()
                .any(|p| p.local_id < 0 || (p.local_id > 0) != p.server_id.is_some())
        {
            return Err(invalid());
        }
        if cursor.account != account {
            return Err(reader_error("CURSOR_ACCOUNT_MISMATCH", "游标属于其他账号"));
        }
        if cursor.chat != chat {
            return Err(reader_error("CURSOR_CHAT_MISMATCH", "游标属于其他会话"));
        }
        Ok(cursor)
    }

    fn encode(&self) -> Result<String> {
        Ok(URL_SAFE_NO_PAD.encode(serde_json::to_vec(self)?))
    }
}

struct Shard {
    conn: Connection,
    position: Position,
}

fn schema_error(error: rusqlite::Error) -> anyhow::Error {
    match &error {
        rusqlite::Error::SqliteFailure(_, Some(message))
        | rusqlite::Error::SqlInputError { msg: message, .. }
            if message.starts_with("no such table:") || message.starts_with("no such column:") =>
        {
            reader_error("VERSION_UNSUPPORTED", "当前微信消息表结构不受支持")
        }
        _ => error.into(),
    }
}

fn generation(path: &Path) -> Result<String> {
    use std::io::Read;
    let created = std::fs::metadata(path)?
        .created()?
        .duration_since(std::time::UNIX_EPOCH)?;
    let mut header = [0u8; 16];
    std::fs::File::open(path)?.read_exact(&mut header)?;
    // 明文库头固定；加密库前 16 字节为盐。不得用随写入变化的 mtime 判代际。
    Ok(format!("{}:{header:x?}", created.as_nanos()))
}

fn open_shards(cfg: &crate::config::Config, cursor: &Cursor) -> Result<BTreeMap<String, Shard>> {
    let mut shards = BTreeMap::new();
    let entries = match std::fs::read_dir(cfg.db_dir.join("message")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(if cursor.shards.is_empty() {
                reader_error("CHAT_NOT_FOUND", "账号没有消息分片")
            } else {
                reader_error("DB_ROTATED", "游标中的消息目录已消失")
            });
        }
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(number) = name
            .strip_prefix("message_")
            .and_then(|s| s.strip_suffix(".db"))
        else {
            continue;
        };
        if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let rel = format!("message/{name}");
        let generation = generation(&entry.path())?;
        if cursor
            .shards
            .get(&rel)
            .is_some_and(|old| old.generation != generation)
        {
            return Err(reader_error("DB_ROTATED", "消息库代际改变"));
        }
        let conn = open_database(cfg, &rel)?;
        conn.execute_batch("BEGIN DEFERRED")?;
        let has_table = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            [&cursor.table],
            |row| row.get::<_, bool>(0),
        )?;
        let first = if has_table {
            conn.query_row(&format!("SELECT local_id, COALESCE(CAST(server_id AS TEXT),'0') FROM [{}] ORDER BY local_id LIMIT 1", cursor.table), [], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))).optional().map_err(schema_error)?
        } else {
            None
        };
        if first.as_ref().is_some_and(|(_, server)| server == "0") {
            return Err(reader_error(
                "MESSAGE_NOT_READY",
                "server_id 尚未分配，请稍后重试",
            ));
        }
        let mut position = Position {
            generation,
            has_table,
            first,
            local_id: 0,
            server_id: None,
        };
        if let Some(old) = cursor.shards.get(&rel) {
            validate_position(&conn, &cursor.table, &position, old)?;
            position.local_id = old.local_id;
            position.server_id.clone_from(&old.server_id);
        }
        shards.insert(rel, Shard { conn, position });
    }
    if cursor.shards.keys().any(|key| !shards.contains_key(key)) {
        return Err(reader_error("DB_ROTATED", "游标中的分片已消失"));
    }
    if !shards.values().any(|s| s.position.has_table) {
        return Err(reader_error("CHAT_NOT_FOUND", "各消息分片均无该会话表"));
    }
    Ok(shards)
}

fn validate_position(
    conn: &Connection,
    table: &str,
    current: &Position,
    old: &Position,
) -> Result<()> {
    if current.generation != old.generation
        || old.has_table && !current.has_table
        || old.first.is_some() && current.first != old.first
    {
        return Err(reader_error("DB_ROTATED", "消息库代际或会话锚点改变"));
    }
    if old.local_id > 0 {
        let server: Option<String> = conn
            .query_row(
                &format!("SELECT CAST(server_id AS TEXT) FROM [{table}] WHERE local_id=?1"),
                [old.local_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(schema_error)?;
        let Some(server) = server else {
            return Err(reader_error(
                "CURSOR_OUT_OF_RANGE",
                "游标 ID 越界或边界记录消失",
            ));
        };
        if Some(server) != old.server_id {
            return Err(reader_error("DB_ROTATED", "游标边界的 server_id 改变"));
        }
    }
    Ok(())
}

pub(crate) fn poll(
    cfg: &crate::config::Config,
    account: &str,
    chat: &str,
    token: Option<&str>,
    limit: usize,
    state_path: &Path,
) -> Result<Value> {
    if !(1..=10000).contains(&limit) {
        return Err(reader_error("INTERNAL", "limit 必须在 1–10000 之间"));
    }
    if let Some(parent) = state_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut state = open_state(state_path)?;
    let transaction = state.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let saved: Option<String> = if token.is_none() {
        transaction
            .query_row(
                "SELECT cursor FROM checkpoints WHERE chat=?1",
                [chat],
                |r| r.get(0),
            )
            .optional()?
    } else {
        None
    };
    let input = token.or(saved.as_deref()).unwrap_or("");
    let mut cursor = Cursor::decode(input, account, chat)?;
    let mut shards = open_shards(cfg, &cursor)?;
    let replay: Option<String> = transaction
        .query_row(
            "SELECT response FROM batches WHERE chat=?1 AND input=?2 AND page_limit=?3",
            rusqlite::params![chat, input, limit as i64],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(replay) = replay {
        let response: Value = serde_json::from_str(&replay)?;
        let next = response["next_cursor"]
            .as_str()
            .ok_or_else(|| reader_error("INTERNAL", "持久化批次损坏"))?;
        let replay_cursor = Cursor::decode(next, account, chat)?;
        for (rel, old) in &replay_cursor.shards {
            let shard = shards
                .get(rel)
                .ok_or_else(|| reader_error("DB_ROTATED", "重放批次的分片已消失"))?;
            validate_position(&shard.conn, &cursor.table, &shard.position, old)?;
        }
        verify_generations(cfg, &shards)?;
        if token.is_none() {
            save_checkpoint(&transaction, chat, next)?;
        }
        transaction.commit()?;
        return Ok(response);
    }
    let mut messages = Vec::new();
    let mut has_more = false;
    for (rel, shard) in &mut shards {
        if !shard.position.has_table {
            continue;
        }
        let mut stmt = shard.conn.prepare(&format!("SELECT m.local_id, CAST(m.server_id AS TEXT), m.local_type, m.create_time, m.message_content, m.WCDB_CT_message_content, n.user_name FROM [{}] m LEFT JOIN Name2Id n ON n.rowid=m.real_sender_id WHERE m.local_id>?1 ORDER BY m.local_id LIMIT ?2",cursor.table)).map_err(schema_error)?;
        let mut rows = stmt.query(rusqlite::params![
            shard.position.local_id,
            (limit - messages.len() + 1) as i64
        ])?;
        while let Some(row) = rows.next()? {
            if messages.len() == limit {
                has_more = true;
                break;
            }
            let local_id: i64 = row.get(0)?;
            let server_id: Option<String> = row.get(1)?;
            let server_id = server_id.filter(|s| s != "0").ok_or_else(|| {
                reader_error("MESSAGE_NOT_READY", "server_id 尚未分配，请稍后重试")
            })?;
            let local_type: i64 = row.get(2)?;
            let timestamp: i64 = row.get(3)?;
            let bytes = match row.get_ref(4)? {
                rusqlite::types::ValueRef::Text(b) | rusqlite::types::ValueRef::Blob(b) => b,
                _ => {
                    return Err(reader_error(
                        "VERSION_UNSUPPORTED",
                        "消息正文不是 TEXT/BLOB",
                    ))
                }
            };
            let content = if row.get::<_, i64>(5)? == 4 {
                zstd::decode_all(bytes)
                    .map_err(|_| reader_error("VERSION_UNSUPPORTED", "消息正文 zstd 解压失败"))?
            } else {
                bytes.to_vec()
            };
            let content = String::from_utf8(content)
                .map_err(|_| reader_error("VERSION_UNSUPPORTED", "消息正文不是 UTF-8"))?;
            let sender = row
                .get::<_, Option<String>>(6)?
                .filter(|s| !s.is_empty() && s != chat)
                .or_else(|| content.split_once(":\n").map(|(id, _)| id.to_owned()));
            let text = sender
                .as_ref()
                .and_then(|id| content.strip_prefix(&format!("{id}:\n")))
                .unwrap_or(&content);
            messages.push(json!({"server_id":server_id,"svrid":server_id,"local_id":local_id,"shard":rel,"chat_wxid":chat,"sender_wxid":sender,"is_self":sender.as_ref().map(|s|s==account),"timestamp":timestamp,"type":crate::daemon::query::type_code(local_type),"text":text}));
            shard.position.local_id = local_id;
            shard.position.server_id = Some(server_id);
        }
    }
    verify_generations(cfg, &shards)?;
    cursor.sequence = cursor
        .sequence
        .checked_add(1)
        .ok_or_else(|| reader_error("CURSOR_INVALID", "游标序列已溢出"))?;
    cursor.shards = shards
        .into_iter()
        .map(|(rel, shard)| (rel, shard.position))
        .collect();
    let next = cursor.encode()?;
    let response = json!({"account_wxid":account,"chat_wxid":chat,"messages":messages,"next_cursor":next,"has_more":has_more});
    transaction.execute(
        "INSERT INTO batches(chat,input,page_limit,response) VALUES(?1,?2,?3,?4)",
        rusqlite::params![chat, input, limit as i64, serde_json::to_string(&response)?],
    )?;
    if token.is_none() {
        save_checkpoint(&transaction, chat, &next)?;
    }
    transaction.commit()?;
    Ok(response)
}

fn verify_generations(cfg: &crate::config::Config, shards: &BTreeMap<String, Shard>) -> Result<()> {
    for (rel, shard) in shards {
        let current = generation(&cfg.db_dir.join(rel))
            .map_err(|_| reader_error("DB_ROTATED", "读取期间无法核对源库代际"))?;
        if current != shard.position.generation {
            return Err(reader_error("DB_ROTATED", "读取期间源库被替换"));
        }
    }
    Ok(())
}

fn open_state(path: &Path) -> Result<Connection> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let fresh = match options.open(path) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(error) => return Err(error.into()),
    };
    if !fresh && std::fs::metadata(path)?.len() == 0 {
        return Err(reader_error("INTERNAL", "既有游标状态库被截空，拒绝重置"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    let state = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    state.busy_timeout(std::time::Duration::from_secs(5))?;
    if fresh {
        state.execute_batch("BEGIN IMMEDIATE;
            CREATE TABLE checkpoints(chat TEXT PRIMARY KEY, cursor TEXT NOT NULL);
            CREATE TABLE batches(chat TEXT NOT NULL, input TEXT NOT NULL, page_limit INTEGER NOT NULL, response TEXT NOT NULL, PRIMARY KEY(chat,input,page_limit));
            PRAGMA user_version=1; COMMIT;")?;
    }
    let version: i64 = state.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version != 1 {
        return Err(reader_error("INTERNAL", "游标状态库未初始化或版本不支持"));
    }
    state.prepare("SELECT chat,cursor FROM checkpoints LIMIT 0")?;
    state.prepare("SELECT chat,input,page_limit,response FROM batches LIMIT 0")?;
    Ok(state)
}

fn save_checkpoint(
    transaction: &rusqlite::Transaction<'_>,
    chat: &str,
    cursor: &str,
) -> Result<()> {
    transaction.execute("INSERT INTO checkpoints(chat,cursor) VALUES(?1,?2) ON CONFLICT(chat) DO UPDATE SET cursor=excluded.cursor",[chat,cursor])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    const CHAT: &str = "123@chatroom";
    const ACCOUNT: &str = "wxid_fixture";
    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        root: PathBuf,
        cfg: crate::config::Config,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "wx-incremental-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let cfg = crate::config::Config {
                db_dir: root.join("db_storage"),
                keys_file: root.join("all_keys.json"),
                decrypted_dir: root.join("decrypted"),
                wechat_process: String::new(),
            };
            std::fs::create_dir_all(cfg.db_dir.join("message")).unwrap();
            Self { root, cfg }
        }

        fn shard(&self, number: usize) -> Connection {
            let conn =
                Connection::open(self.cfg.db_dir.join(format!("message/message_{number}.db")))
                    .unwrap();
            conn.execute_batch(&format!(
                "CREATE TABLE Name2Id(user_name TEXT); INSERT INTO Name2Id(rowid,user_name) VALUES(1,'wxid_fixture'),(2,'wxid_friend');
                 CREATE TABLE Msg_{:x}(local_id INTEGER PRIMARY KEY AUTOINCREMENT, server_id INTEGER, local_type INTEGER, create_time INTEGER, real_sender_id INTEGER, message_content, WCDB_CT_message_content INTEGER);", md5::compute(CHAT)
            )).unwrap();
            conn
        }

        fn insert(conn: &Connection, server: i64, timestamp: i64, sender: i64, text: &str) {
            conn.execute(&format!("INSERT INTO Msg_{:x}(server_id,local_type,create_time,real_sender_id,message_content,WCDB_CT_message_content) VALUES(?1,1,?2,?3,?4,0)",md5::compute(CHAT)),rusqlite::params![server,timestamp,sender,text]).unwrap();
        }

        fn read(&self, cursor: Option<&str>, limit: usize) -> Result<Value> {
            poll(
                &self.cfg,
                ACCOUNT,
                CHAT,
                cursor,
                limit,
                &self.root.join("state.sqlite3"),
            )
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn same_second_and_late_messages_advance_by_local_id() {
        let fixture = Fixture::new();
        let conn = fixture.shard(0);
        Fixture::insert(&conn, 9007199254740993, 100, 2, "wxid_friend:\n甲");
        Fixture::insert(&conn, 9007199254740994, 100, 1, "乙");
        Fixture::insert(&conn, 9007199254740995, 90, 2, "晚到");
        let first = fixture.read(Some(""), 2).unwrap();
        assert_eq!(
            first["messages"],
            json!([
                {"server_id":"9007199254740993","svrid":"9007199254740993","local_id":1,"chat_wxid":CHAT,"sender_wxid":"wxid_friend","is_self":false,"timestamp":100,"type":1,"text":"甲","shard":"message/message_0.db"},
                {"server_id":"9007199254740994","svrid":"9007199254740994","local_id":2,"chat_wxid":CHAT,"sender_wxid":ACCOUNT,"is_self":true,"timestamp":100,"type":1,"text":"乙","shard":"message/message_0.db"}
            ])
        );
        assert_eq!(first["has_more"], true);
        let next = fixture.read(first["next_cursor"].as_str(), 2).unwrap();
        assert_eq!(next["messages"][0]["local_id"], 3);
        assert_eq!(next["messages"][0]["timestamp"], 90);
        assert_eq!(next["has_more"], false);
    }

    #[test]
    fn replay_survives_new_inserts_and_process_state_reopen() {
        let fixture = Fixture::new();
        let conn = fixture.shard(0);
        Fixture::insert(&conn, 101, 100, 1, "first");
        let first = fixture.read(Some(""), 2).unwrap();
        Fixture::insert(&conn, 102, 100, 2, "second");
        assert_eq!(fixture.read(Some(""), 2).unwrap(), first);
        let second = fixture.read(first["next_cursor"].as_str(), 2).unwrap();
        assert_eq!(second["messages"].as_array().unwrap().len(), 1);
        assert_eq!(second["messages"][0]["server_id"], "102");
        let empty = fixture.read(second["next_cursor"].as_str(), 2).unwrap();
        assert_eq!(empty["messages"], json!([]));
        Fixture::insert(&conn, 103, 99, 2, "third");
        assert_eq!(
            fixture.read(second["next_cursor"].as_str(), 2).unwrap(),
            empty
        );
        assert_eq!(
            fixture.read(empty["next_cursor"].as_str(), 2).unwrap()["messages"][0]["server_id"],
            "103"
        );
    }

    #[test]
    fn new_shards_continue_and_disappearing_empty_tables_are_rotation() {
        let fixture = Fixture::new();
        let empty = fixture.shard(0);
        let first_shard = fixture.shard(1);
        Fixture::insert(&first_shard, 101, 100, 2, "first");
        let first = fixture.read(Some(""), 10).unwrap();
        let new_shard = fixture.shard(2);
        Fixture::insert(&new_shard, 201, 100, 2, "new shard");
        Fixture::insert(&first_shard, 102, 90, 2, "late");
        let next = fixture.read(first["next_cursor"].as_str(), 10).unwrap();
        let messages = next["messages"].as_array().unwrap();
        assert_eq!(
            messages
                .iter()
                .map(|m| m["server_id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["102", "201"]
        );
        assert_eq!(messages[1]["local_id"], 1);
        empty
            .execute_batch(&format!("DROP TABLE Msg_{:x}", md5::compute(CHAT)))
            .unwrap();
        let error = fixture.read(next["next_cursor"].as_str(), 10).unwrap_err();
        assert_eq!(
            crate::cli::account::error_json(&error)["error"]["code"],
            "DB_ROTATED"
        );
    }

    #[test]
    fn unassigned_server_id_blocks_without_advancing_checkpoint() {
        let fixture = Fixture::new();
        let conn = fixture.shard(0);
        Fixture::insert(&conn, 0, 100, 1, "pending");
        conn.execute_batch(&format!(
            "UPDATE Msg_{:x} SET server_id=NULL",
            md5::compute(CHAT)
        ))
        .unwrap();
        let error = fixture.read(None, 10).unwrap_err();
        assert_eq!(
            crate::cli::account::error_json(&error)["error"]["code"],
            "MESSAGE_NOT_READY"
        );
        conn.execute_batch(&format!(
            "UPDATE Msg_{:x} SET server_id=101",
            md5::compute(CHAT)
        ))
        .unwrap();
        assert_eq!(
            fixture.read(None, 10).unwrap()["messages"][0]["server_id"],
            "101"
        );
    }

    #[test]
    fn missing_message_directory_distinguishes_first_read_from_rotation() {
        let fixture = Fixture::new();
        let conn = fixture.shard(0);
        Fixture::insert(&conn, 101, 100, 1, "first");
        let first = fixture.read(Some(""), 10).unwrap();
        drop(conn);
        std::fs::remove_dir_all(fixture.cfg.db_dir.join("message")).unwrap();
        let error = fixture.read(first["next_cursor"].as_str(), 10).unwrap_err();
        assert_eq!(
            crate::cli::account::error_json(&error)["error"]["code"],
            "DB_ROTATED"
        );
        let error = fixture.read(Some(""), 3).unwrap_err();
        assert_eq!(
            crate::cli::account::error_json(&error)["error"]["code"],
            "CHAT_NOT_FOUND"
        );
    }
    fn assert_code(result: Result<Value>, code: &str) {
        let error = result.unwrap_err();
        assert_eq!(
            crate::cli::account::error_json(&error)["error"]["code"],
            code,
            "{error}"
        );
    }

    #[test]
    fn cursor_errors_are_distinct_from_missing_chat_and_schema() {
        let fixture = Fixture::new();
        let conn = fixture.shard(0);
        Fixture::insert(&conn, 101, 100, 1, "first");
        let first = fixture.read(Some(""), 10).unwrap();
        let cursor = first["next_cursor"].as_str();
        assert_code(fixture.read(Some("not.a.cursor"), 10), "CURSOR_INVALID");
        assert_code(
            poll(
                &fixture.cfg,
                "wxid_other",
                CHAT,
                cursor,
                10,
                &fixture.root.join("other.db"),
            ),
            "CURSOR_ACCOUNT_MISMATCH",
        );
        assert_code(
            poll(
                &fixture.cfg,
                ACCOUNT,
                "456@chatroom",
                cursor,
                10,
                &fixture.root.join("state.sqlite3"),
            ),
            "CURSOR_CHAT_MISMATCH",
        );
        assert_code(
            poll(
                &fixture.cfg,
                ACCOUNT,
                "456@chatroom",
                Some(""),
                10,
                &fixture.root.join("state.sqlite3"),
            ),
            "CHAT_NOT_FOUND",
        );
        conn.execute_batch(&format!(
            "ALTER TABLE Msg_{:x} DROP COLUMN WCDB_CT_message_content",
            md5::compute(CHAT)
        ))
        .unwrap();
        assert_code(fixture.read(cursor, 10), "VERSION_UNSUPPORTED");
    }

    #[test]
    fn truncation_rebuild_and_file_removal_never_reset_cursor() {
        let fixture = Fixture::new();
        let conn = fixture.shard(0);
        Fixture::insert(&conn, 101, 100, 1, "first");
        Fixture::insert(&conn, 102, 100, 1, "second");
        let first = fixture.read(None, 10).unwrap();
        let cursor = first["next_cursor"].as_str();
        conn.execute_batch(&format!(
            "DELETE FROM Msg_{:x} WHERE local_id=2",
            md5::compute(CHAT)
        ))
        .unwrap();
        assert_code(fixture.read(cursor, 10), "CURSOR_OUT_OF_RANGE");
        conn.execute_batch(&format!(
            "UPDATE Msg_{:x} SET server_id=999",
            md5::compute(CHAT)
        ))
        .unwrap();
        assert_code(fixture.read(cursor, 10), "DB_ROTATED");
        drop(conn);
        std::fs::remove_file(fixture.cfg.db_dir.join("message/message_0.db")).unwrap();
        assert_code(fixture.read(cursor, 10), "DB_ROTATED");
        let path = fixture.cfg.db_dir.join("message/message_0.db");
        std::fs::write(&path, [0xabu8; 4096]).unwrap();
        assert_code(fixture.read(cursor, 10), "DB_ROTATED");
        std::fs::remove_file(&path).unwrap();
        let replacement = fixture.shard(0);
        Fixture::insert(&replacement, 201, 100, 1, "replacement");
        assert_code(fixture.read(cursor, 10), "DB_ROTATED");
        assert_code(fixture.read(Some(""), 10), "DB_ROTATED");
        assert_code(fixture.read(None, 10), "DB_ROTATED");
    }

    #[test]
    fn unreadable_new_shard_returns_no_partial_batch_or_checkpoint() {
        let fixture = Fixture::new();
        let conn = fixture.shard(0);
        Fixture::insert(&conn, 101, 100, 1, "first");
        let path = fixture.cfg.db_dir.join("message/message_1.db");
        std::fs::write(&path, [0xabu8; 4096]).unwrap();
        assert_code(fixture.read(None, 10), "KEY_FAILED");
        std::fs::remove_file(path).unwrap();
        let second = fixture.shard(1);
        Fixture::insert(&second, 201, 100, 1, "second");
        let page = fixture.read(None, 10).unwrap();
        assert_eq!(
            page["messages"]
                .as_array()
                .unwrap()
                .iter()
                .map(|m| m["server_id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["101", "201"]
        );
        assert_eq!(page["has_more"], false);
    }

    #[test]
    fn windows_key_separators_encrypted_wal_and_zstd_use_synthetic_data() {
        let fixture = Fixture::new();
        let conn = fixture.shard(0);
        Fixture::insert(&conn, 101, 100, 2, "placeholder");
        let compressed = zstd::encode_all("wxid_friend:\n压缩正文".as_bytes(), 1).unwrap();
        conn.execute(
            &format!(
                "UPDATE Msg_{:x} SET message_content=?1,WCDB_CT_message_content=4",
                md5::compute(CHAT)
            ),
            [compressed],
        )
        .unwrap();
        let path = fixture.cfg.db_dir.join("message/message_1.db");
        let key = "12".repeat(32);
        conn.execute(
            "ATTACH DATABASE ?1 AS encrypted KEY ?2",
            rusqlite::params![path.to_str().unwrap(), format!("x'{key}'")],
        )
        .unwrap();
        conn.query_row("SELECT sqlcipher_export('encrypted')", [], |_| Ok(()))
            .unwrap();
        conn.execute_batch("DETACH DATABASE encrypted").unwrap();
        drop(conn);
        std::fs::remove_file(fixture.cfg.db_dir.join("message/message_0.db")).unwrap();
        std::fs::write(
            &fixture.cfg.keys_file,
            json!({r"message\message_1.db":{"enc_key":key}}).to_string(),
        )
        .unwrap();
        let writer = Connection::open(&path).unwrap();
        writer
            .execute_batch(&format!(
                "PRAGMA key=\"x'{key}'\"; PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;"
            ))
            .unwrap();
        Fixture::insert(&writer, 102, 90, 1, "WAL message");
        let page = fixture.read(None, 10).unwrap();
        assert_eq!(page["messages"][0]["text"], "压缩正文");
        assert_eq!(page["messages"][1]["text"], "WAL message");
        assert_eq!(page["messages"][1]["server_id"], "102");
    }

    #[test]
    fn bad_compression_and_corrupt_state_fail_without_restarting() {
        let fixture = Fixture::new();
        let conn = fixture.shard(0);
        Fixture::insert(&conn, 101, 100, 1, "first");
        conn.execute_batch(&format!(
            "UPDATE Msg_{:x} SET WCDB_CT_message_content=4",
            md5::compute(CHAT)
        ))
        .unwrap();
        assert_code(fixture.read(None, 10), "VERSION_UNSUPPORTED");
        conn.execute_batch(&format!(
            "UPDATE Msg_{:x} SET WCDB_CT_message_content=0,real_sender_id=99",
            md5::compute(CHAT)
        ))
        .unwrap();
        let page = fixture.read(None, 10).unwrap();
        assert_eq!(page["messages"][0]["sender_wxid"], Value::Null);
        assert_eq!(page["messages"][0]["is_self"], Value::Null);
        std::fs::write(fixture.root.join("state.sqlite3"), b"corrupt state").unwrap();
        assert_code(fixture.read(None, 10), "INTERNAL");
    }
    #[test]
    fn truncated_or_incomplete_state_never_becomes_a_first_read() {
        for damage in [
            "truncate",
            "drop checkpoint",
            "drop batches",
            "wrong version",
        ] {
            let fixture = Fixture::new();
            let conn = fixture.shard(0);
            Fixture::insert(&conn, 101, 100, 1, "first");
            fixture.read(None, 10).unwrap();
            let state_path = fixture.root.join("state.sqlite3");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    std::fs::metadata(&state_path).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
            if damage == "truncate" {
                std::fs::write(&state_path, []).unwrap();
            } else {
                let state = Connection::open(&state_path).unwrap();
                state
                    .execute_batch(match damage {
                        "drop checkpoint" => "DROP TABLE checkpoints",
                        "drop batches" => "DROP TABLE batches",
                        _ => "PRAGMA user_version=999",
                    })
                    .unwrap();
            }
            assert_code(fixture.read(None, 10), "INTERNAL");
        }
    }
}
