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

fn decoded_text(row: &rusqlite::Row<'_>, column: usize, compression: usize) -> Option<String> {
    let bytes = match row.get_ref(column).ok()? {
        rusqlite::types::ValueRef::Text(bytes) | rusqlite::types::ValueRef::Blob(bytes) => bytes,
        _ => return None,
    };
    let bytes = if row.get::<_, Option<i64>>(compression).ok()?.unwrap_or(0) == 4 {
        zstd::decode_all(bytes).ok()?
    } else {
        bytes.to_vec()
    };
    String::from_utf8(bytes).ok()
}

fn mentions(source: Option<&str>) -> Vec<String> {
    let Some(doc) = source.and_then(|xml| roxmltree::Document::parse(xml).ok()) else {
        return Vec::new();
    };
    let root = doc.root_element();
    if !root.has_tag_name("msgsource") {
        return Vec::new();
    }
    root.children()
        .find(|node| node.has_tag_name("atuserlist"))
        .and_then(|node| node.text())
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .collect()
}

fn quote(text: &str) -> Option<Value> {
    let doc = roxmltree::Document::parse(text).ok()?;
    let root = doc.root_element();
    let appmsg = if root.has_tag_name("appmsg") {
        root
    } else if root.has_tag_name("msg") {
        root.children().find(|node| node.has_tag_name("appmsg"))?
    } else {
        return None;
    };
    if appmsg
        .children()
        .find(|node| node.has_tag_name("type"))?
        .text()?
        .trim()
        != "57"
    {
        return None;
    }
    let refer = appmsg
        .children()
        .find(|node| node.has_tag_name("refermsg"))?;
    let field = |tag| {
        refer
            .children()
            .find(|node| node.has_tag_name(tag))
            .and_then(|node| node.text())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    let message_id = field("svrid").filter(|id| {
        id.bytes().all(|b| b.is_ascii_digit()) && id.parse::<u64>().is_ok_and(|id| id > 0)
    });
    Some(json!({"message_id":message_id,"sender_wxid":field("chatusr")}))
}

fn message_sender(mapped: Option<String>, content: &str, chat: &str) -> Option<String> {
    mapped
        .filter(|id| !id.is_empty() && id != chat)
        .or_else(|| content.split_once(":\n").map(|(id, _)| id.to_owned()))
}

fn quote_sender(
    shards: &BTreeMap<String, Shard>,
    table: &str,
    chat: &str,
    server_id: &str,
) -> Result<Option<String>> {
    let sql = format!("SELECT n.user_name,m.message_content,m.WCDB_CT_message_content FROM [{table}] m LEFT JOIN Name2Id n ON n.rowid=m.real_sender_id WHERE m.server_id=?1 LIMIT 1");
    for shard in shards.values().filter(|shard| shard.position.has_table) {
        let sender = shard
            .conn
            .query_row(&sql, [server_id], |row| {
                let content = decoded_text(row, 1, 2);
                Ok(message_sender(
                    row.get::<_, Option<String>>(0).ok().flatten(),
                    content.as_deref().unwrap_or(""),
                    chat,
                ))
            })
            .optional()
            .map_err(schema_error)?
            .flatten();
        if sender.is_some() {
            return Ok(sender);
        }
    }
    Ok(None)
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
        let (has_source, has_compression): (bool, bool) = shard.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name='source'), EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name='WCDB_CT_source')",
            [&cursor.table], |row| Ok((row.get(0)?, row.get(1)?)))?;
        let source = if has_source { "m.source" } else { "NULL" };
        let compression = if has_compression {
            "m.WCDB_CT_source"
        } else {
            "0"
        };
        let mut stmt = shard.conn.prepare(&format!("SELECT m.local_id, CAST(m.server_id AS TEXT), m.local_type, m.create_time, m.message_content, m.WCDB_CT_message_content, n.user_name, {source}, {compression} FROM [{}] m LEFT JOIN Name2Id n ON n.rowid=m.real_sender_id WHERE m.local_id>?1 ORDER BY m.local_id LIMIT ?2",cursor.table)).map_err(schema_error)?;
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
            let sender = message_sender(row.get(6)?, &content, chat);
            let text = sender
                .as_ref()
                .and_then(|id| content.strip_prefix(&format!("{id}:\n")))
                .unwrap_or(&content);
            let source = decoded_text(row, 7, 8);
            let quote = if crate::daemon::query::type_code(local_type) == 49 {
                quote(text)
            } else {
                None
            };
            messages.push(json!({"server_id":server_id,"svrid":server_id,"local_id":local_id,"shard":rel,"chat_wxid":chat,"sender_wxid":sender,"is_self":sender.as_ref().map(|s|s==account),"timestamp":timestamp,"type":crate::daemon::query::type_code(local_type),"text":text,"mentions":mentions(source.as_deref()),"quote":quote,"raw":{"message_content":content,"source":source}}));
            shard.position.local_id = local_id;
            shard.position.server_id = Some(server_id);
        }
    }
    for message in &mut messages {
        if message["quote"].is_object() && message["quote"]["sender_wxid"].is_null() {
            let sender = match message["quote"]["message_id"].as_str() {
                Some(id) => quote_sender(&shards, &cursor.table, chat, id)?,
                None => None,
            };
            message["quote"]["sender_wxid"] = json!(sender);
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
    fn mentions_and_raw_preserve_wcdb_source_without_using_display_names() {
        let fixture = Fixture::new();
        let conn = fixture.shard(0);
        let table = format!("Msg_{:x}", md5::compute(CHAT));
        conn.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN source; ALTER TABLE {table} ADD COLUMN WCDB_CT_source INTEGER"
        )).unwrap();
        let sources = [
            ("<msgsource><atuserlist>wxid_service</atuserlist></msgsource>", false, json!(["wxid_service"])),
            ("<msgsource><atuserlist><![CDATA[wxid_service, wxid_friend,,]]></atuserlist></msgsource>", true, json!(["wxid_service", "wxid_friend"])),
            ("<msgsource><atuserlist><![CDATA[notify@all]]></atuserlist></msgsource>", true, json!(["notify@all"])),
            ("<msgsource/>", false, json!([])),
            ("<msgsource><atuserlist>wxid_service</msgsource>", false, json!([])),
        ];
        for (index, (source, compressed, _)) in sources.iter().enumerate() {
            Fixture::insert(
                &conn,
                101 + index as i64,
                100,
                2,
                "wxid_friend:\n@客服 原始正文",
            );
            let bytes = if *compressed {
                zstd::encode_all(source.as_bytes(), 1).unwrap()
            } else {
                source.as_bytes().to_vec()
            };
            conn.execute(
                &format!("UPDATE {table} SET source=?1, WCDB_CT_source=?2 WHERE server_id=?3"),
                rusqlite::params![bytes, if *compressed { 4 } else { 0 }, 101 + index as i64],
            )
            .unwrap();
        }
        Fixture::insert(&conn, 106, 100, 2, "无 source");
        let page = fixture.read(Some(""), 10).unwrap();
        for (message, (source, _, mentions)) in
            page["messages"].as_array().unwrap().iter().zip(sources)
        {
            assert_eq!(message["mentions"], mentions);
            assert_eq!(
                message["raw"],
                json!({"message_content":"wxid_friend:\n@客服 原始正文", "source":source})
            );
            assert_eq!(message["quote"], Value::Null);
            assert_eq!(message["text"], "@客服 原始正文");
        }
        assert_eq!(page["messages"][5]["mentions"], json!([]));
        assert_eq!(
            page["messages"][5]["raw"],
            json!({"message_content":"无 source", "source":null})
        );
        assert_eq!(fixture.read(Some(""), 10).unwrap(), page);
    }

    #[test]
    fn quote_json_uses_chatusr_and_preserves_ids_without_parsing_display_names() {
        let fixture = Fixture::new();
        let conn = fixture.shard(0);
        let cases = [
            (244813135921_i64, "<msg><appmsg><type>57</type><refermsg><svrid>9007199254740993</svrid><chatusr><![CDATA[wxid_service]]></chatusr><fromusr>wxid_wrong</fromusr></refermsg></appmsg></msg>", json!({"message_id":"9007199254740993","sender_wxid":"wxid_service"})),
            (49, "<appmsg><type>57</type><refermsg><svrid>102</svrid><chatusr>wxid_friend</chatusr><displayname>客服</displayname></refermsg></appmsg>", json!({"message_id":"102","sender_wxid":"wxid_friend"})),
            (49, "<msg><appmsg><type>57</type><refermsg><chatusr>wxid_service</chatusr></refermsg></appmsg></msg>", json!({"message_id":null,"sender_wxid":"wxid_service"})),
            (49, "<msg><appmsg><type>57</type><refermsg><svrid>invalid</svrid><chatusr>wxid_service</chatusr></refermsg></appmsg></msg>", json!({"message_id":null,"sender_wxid":"wxid_service"})),
            (49, "<msg><appmsg><type>5</type><refermsg><svrid>101</svrid><chatusr>wxid_service</chatusr></refermsg></appmsg></msg>", Value::Null),
            (49, "<msg><appmsg><type>57</type></appmsg></msg>", Value::Null),
            (49, "<msg><appmsg><type>57</type><refermsg>", Value::Null),
            (1, "<msg><appmsg><type>57</type><refermsg><svrid>101</svrid><chatusr>wxid_service</chatusr></refermsg></appmsg></msg>", Value::Null),
        ];
        for (index, (local_type, xml, _)) in cases.iter().enumerate() {
            let content = format!("wxid_friend:\n{xml}");
            Fixture::insert(&conn, 201 + index as i64, 100, 2, &content);
            let bytes = if index == 0 {
                zstd::encode_all(content.as_bytes(), 1).unwrap()
            } else {
                content.into_bytes()
            };
            conn.execute(&format!("UPDATE Msg_{:x} SET local_type=?1,message_content=?2,WCDB_CT_message_content=?3 WHERE server_id=?4", md5::compute(CHAT)), rusqlite::params![local_type,bytes,if index == 0 {4} else {0},201 + index as i64]).unwrap();
        }
        let page = fixture.read(Some(""), 20).unwrap();
        assert_eq!(page["messages"].as_array().unwrap().len(), cases.len());
        for (message, (_, xml, quote)) in page["messages"].as_array().unwrap().iter().zip(cases) {
            assert_eq!(message["quote"], quote, "{xml}");
            assert_eq!(
                message["raw"]["message_content"],
                format!("wxid_friend:\n{xml}")
            );
            assert_eq!(message["mentions"], json!([]));
        }
    }

    #[test]
    fn quote_sender_lookup_spans_shards_and_cursor_boundaries_without_guessing() {
        let fixture = Fixture::new();
        let conn = fixture.shard(0);
        let later = fixture.shard(1);
        Fixture::insert(&conn, 101, 100, 1, "我方旧消息");
        Fixture::insert(&conn, 102, 100, 99, "placeholder");
        conn.execute(&format!("UPDATE Msg_{:x} SET message_content=?1,WCDB_CT_message_content=4 WHERE server_id=102", md5::compute(CHAT)), [zstd::encode_all("wxid_prefix:\n旧消息".as_bytes(), 1).unwrap()]).unwrap();
        Fixture::insert(&later, 501, 100, 2, "另一分片原消息");
        let other_table = format!("Msg_{:x}", md5::compute("456@chatroom"));
        later.execute_batch(&format!("CREATE TABLE {other_table} AS SELECT * FROM Msg_{:x}; UPDATE {other_table} SET server_id=601", md5::compute(CHAT))).unwrap();
        for (server, referred, chatusr) in [
            (201, 101, ""),
            (202, 501, "<chatusr><![CDATA[ ]]></chatusr>"),
            (203, 102, ""),
            (204, 999999, ""),
            (205, 601, ""),
            (206, 101, "<chatusr>wxid_service</chatusr>"),
        ] {
            let xml = format!("<msg><appmsg><type>57</type><refermsg><svrid>{referred}</svrid>{chatusr}<fromusr>wxid_wrong</fromusr><displayname>客服</displayname></refermsg></appmsg></msg>");
            Fixture::insert(&conn, server, 100, 2, &xml);
        }
        conn.execute_batch(&format!(
            "UPDATE Msg_{:x} SET local_type=49 WHERE server_id>=201",
            md5::compute(CHAT)
        ))
        .unwrap();
        let old = fixture.read(Some(""), 2).unwrap();
        let first = fixture.read(old["next_cursor"].as_str(), 2).unwrap();
        assert_eq!(
            first["messages"][0]["quote"],
            json!({"message_id":"101","sender_wxid":ACCOUNT})
        );
        assert_eq!(
            first["messages"][1]["quote"],
            json!({"message_id":"501","sender_wxid":"wxid_friend"})
        );
        assert_eq!(fixture.read(old["next_cursor"].as_str(), 2).unwrap(), first);
        let second = fixture.read(first["next_cursor"].as_str(), 2).unwrap();
        assert_eq!(
            second["messages"][0]["quote"],
            json!({"message_id":"102","sender_wxid":"wxid_prefix"})
        );
        assert_eq!(
            second["messages"][1]["quote"],
            json!({"message_id":"999999","sender_wxid":null})
        );
        let third = fixture.read(second["next_cursor"].as_str(), 2).unwrap();
        assert_eq!(
            third["messages"][0]["quote"],
            json!({"message_id":"601","sender_wxid":null})
        );
        assert_eq!(
            third["messages"][1]["quote"],
            json!({"message_id":"101","sender_wxid":"wxid_service"})
        );
        let last = fixture.read(third["next_cursor"].as_str(), 2).unwrap();
        assert_eq!(last["messages"].as_array().unwrap().len(), 1);
        assert_eq!(last["messages"][0]["server_id"], "501");
        assert_eq!(last["has_more"], false);
    }

    #[test]
    fn damaged_source_only_empties_its_fields_and_keeps_the_batch_and_quote() {
        use rusqlite::types::Value as SqlValue;
        let fixture = Fixture::new();
        let conn = fixture.shard(0);
        let table = format!("Msg_{:x}", md5::compute(CHAT));
        conn.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN source; ALTER TABLE {table} ADD COLUMN WCDB_CT_source"
        ))
        .unwrap();
        let cases = [
            (SqlValue::Blob(vec![0xff]), SqlValue::Integer(0)),
            (SqlValue::Blob(b"not zstd".to_vec()), SqlValue::Integer(4)),
            (
                SqlValue::Blob(zstd::encode_all(&[0xff][..], 1).unwrap()),
                SqlValue::Integer(4),
            ),
            (SqlValue::Integer(42), SqlValue::Integer(0)),
            (SqlValue::Null, SqlValue::Integer(4)),
            (
                SqlValue::Text("<msgsource/>".into()),
                SqlValue::Text("invalid".into()),
            ),
        ];
        let xml = "<msg><appmsg><type>57</type><refermsg><svrid>101</svrid><chatusr>wxid_service</chatusr></refermsg></appmsg></msg>";
        for (index, (source, compression)) in cases.iter().enumerate() {
            Fixture::insert(&conn, 201 + index as i64, 100, 2, xml);
            conn.execute(&format!("UPDATE {table} SET local_type=49,source=?1,WCDB_CT_source=?2 WHERE server_id=?3"), rusqlite::params![source,compression,201 + index as i64]).unwrap();
        }
        let page = fixture.read(None, 20).unwrap();
        assert_eq!(page["messages"].as_array().unwrap().len(), cases.len());
        for message in page["messages"].as_array().unwrap() {
            assert_eq!(message["mentions"], json!([]));
            assert_eq!(message["raw"], json!({"message_content":xml,"source":null}));
            assert_eq!(
                message["quote"],
                json!({"message_id":"101","sender_wxid":"wxid_service"})
            );
        }
        assert_eq!(fixture.read(None, 20).unwrap()["messages"], json!([]));
    }

    #[test]
    fn source_without_compression_column_still_exposes_mentions_and_raw() {
        let fixture = Fixture::new();
        let conn = fixture.shard(0);
        Fixture::insert(&conn, 101, 100, 2, "text");
        let source = "<msgsource><atuserlist>wxid_service</atuserlist></msgsource>";
        conn.execute_batch(&format!(
            "ALTER TABLE Msg_{:x} ADD COLUMN source TEXT",
            md5::compute(CHAT)
        ))
        .unwrap();
        conn.execute(
            &format!("UPDATE Msg_{:x} SET source=?1", md5::compute(CHAT)),
            [source],
        )
        .unwrap();
        let page = fixture.read(Some(""), 10).unwrap();
        assert_eq!(page["messages"][0]["mentions"], json!(["wxid_service"]));
        assert_eq!(
            page["messages"][0]["raw"],
            json!({"message_content":"text","source":source})
        );
    }

    #[test]
    fn quote_lookup_database_errors_do_not_advance_the_checkpoint() {
        let fixture = Fixture::new();
        let conn = fixture.shard(0);
        Fixture::insert(&conn, 101, 100, 1, "old");
        fixture.read(None, 1).unwrap();
        conn.execute_batch("ALTER TABLE Name2Id RENAME TO senders;
            CREATE VIEW Name2Id AS SELECT rowid, CASE WHEN rowid=1 THEN abs(-9223372036854775808) ELSE user_name END AS user_name FROM senders;").unwrap();
        Fixture::insert(
            &conn,
            201,
            100,
            2,
            "<msg><appmsg><type>57</type><refermsg><svrid>101</svrid></refermsg></appmsg></msg>",
        );
        conn.execute_batch(&format!(
            "UPDATE Msg_{:x} SET local_type=49 WHERE server_id=201",
            md5::compute(CHAT)
        ))
        .unwrap();
        let error = fixture.read(None, 1).unwrap_err();
        assert_eq!(
            crate::cli::account::error_json(&error)["error"]["code"],
            "INTERNAL"
        );
        assert!(error.to_string().contains("integer overflow"), "{error}");
        conn.execute_batch("DROP VIEW Name2Id; ALTER TABLE senders RENAME TO Name2Id")
            .unwrap();
        let page = fixture.read(None, 1).unwrap();
        assert_eq!(page["messages"][0]["server_id"], "201");
        assert_eq!(
            page["messages"][0]["quote"],
            json!({"message_id":"101","sender_wxid":ACCOUNT})
        );
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
                {"server_id":"9007199254740993","svrid":"9007199254740993","local_id":1,"chat_wxid":CHAT,"sender_wxid":"wxid_friend","is_self":false,"timestamp":100,"type":1,"text":"甲","shard":"message/message_0.db","mentions":[],"quote":null,"raw":{"message_content":"wxid_friend:\n甲","source":null}},
                {"server_id":"9007199254740994","svrid":"9007199254740994","local_id":2,"chat_wxid":CHAT,"sender_wxid":ACCOUNT,"is_self":true,"timestamp":100,"type":1,"text":"乙","shard":"message/message_0.db","mentions":[],"quote":null,"raw":{"message_content":"乙","source":null}}
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
