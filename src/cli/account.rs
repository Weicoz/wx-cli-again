use anyhow::Result;
use rusqlite::OptionalExtension;
use serde_json::{json, Value};

#[derive(Debug)]
pub(crate) struct ReaderError {
    pub code: &'static str,
    pub message: String,
}
impl std::fmt::Display for ReaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for ReaderError {}

pub(crate) fn reader_error(code: &'static str, message: impl Into<String>) -> anyhow::Error {
    ReaderError {
        code,
        message: message.into(),
    }
    .into()
}

pub fn error_json(error: &anyhow::Error) -> Value {
    let code = error
        .downcast_ref::<ReaderError>()
        .map_or("INTERNAL", |e| e.code);
    json!({"error": {"code": code, "message": error.to_string()}})
}

/// 每次查询直接验证源库与进程占用；不得从 daemon 的旧联系人缓存推断身份。
pub(crate) fn reader_connection() -> Result<(crate::config::Config, rusqlite::Connection)> {
    let config_path = crate::config::find_config_file()?;
    if !config_path.exists() {
        return Err(reader_error(
            "LOGIN_REQUIRED",
            "账号未初始化，请先登录并运行 wx init",
        ));
    }
    let cfg = crate::config::load_config()?;
    crate::process::find_for_db(Some(&cfg.db_dir), crate::config::selected_pid()?)?;
    let conn = open_contact(&cfg)?;
    Ok((cfg, conn))
}

fn open_contact(cfg: &crate::config::Config) -> Result<rusqlite::Connection> {
    open_database(cfg, "contact/contact.db")
}

pub(crate) fn open_database(
    cfg: &crate::config::Config,
    rel: &str,
) -> Result<rusqlite::Connection> {
    use anyhow::Context;
    use std::io::Read;
    let path = cfg.db_dir.join(rel);
    if !path.is_file() {
        return Err(reader_error(
            "LOGIN_REQUIRED",
            format!("目标账号缺少数据库 {rel}"),
        ));
    }
    let mut header = [0u8; 16];
    std::fs::File::open(&path)?.read_exact(&mut header)?;
    let conn = if &header == b"SQLite format 3\0" {
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?
    } else {
        let keys = std::fs::read_to_string(&cfg.keys_file)
            .map_err(|_| reader_error("KEY_FAILED", "账号密钥文件缺失或不可读，请重新取钥"))?;
        let keys: Value = serde_json::from_str(&keys)
            .map_err(|_| reader_error("KEY_FAILED", "账号密钥文件格式错误"))?;
        let keys = crate::daemon::extract_keys(&keys);
        let key = keys
            .get(rel)
            .ok_or_else(|| reader_error("KEY_FAILED", format!("缺少 {rel} 密钥，请重新取钥")))?;
        crate::crypto::sqlcipher::open_encrypted_readonly(&path, key)
            .map_err(|_| reader_error("KEY_FAILED", format!("{rel} 密钥不匹配，请重新取钥")))?
    };
    conn.execute_batch("PRAGMA query_only=ON")
        .context("设置只读查询失败")?;
    Ok(conn)
}

fn schema_error(error: rusqlite::Error) -> anyhow::Error {
    match &error {
        rusqlite::Error::SqliteFailure(_, Some(message))
        | rusqlite::Error::SqlInputError { msg: message, .. }
            if message.starts_with("no such table:") || message.starts_with("no such column:") =>
        {
            reader_error(
                "VERSION_UNSUPPORTED",
                "当前微信联系人或群成员表结构不受支持",
            )
        }
        _ => error.into(),
    }
}

pub(crate) fn is_known_group(cfg: &crate::config::Config, chat: &str) -> Result<bool> {
    if !chat.ends_with("@chatroom") {
        return Ok(false);
    }
    open_contact(cfg)?
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM contact WHERE username=?1)",
            [chat],
            |row| row.get(0),
        )
        .map_err(schema_error)
}

pub(crate) fn identity(conn: &rusqlite::Connection, db_dir: &std::path::Path) -> Result<Value> {
    let mut stmt = conn
        .prepare("SELECT username, nick_name FROM contact")
        .map_err(schema_error)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?.unwrap_or_default(),
            ))
        })
        .map_err(schema_error)?;
    let mut matches = Vec::new();
    for row in rows {
        let (wxid, nickname) = row.map_err(schema_error)?;
        if !wxid.is_empty() && crate::config::account_matches_dir(&wxid, db_dir) {
            matches.push(json!({"wxid": wxid, "displayName": nickname}));
        }
    }
    if matches.len() != 1 {
        return Err(reader_error(
            "VERSION_UNSUPPORTED",
            "无法从账号目录与本人联系人唯一确认 wxid",
        ));
    }
    let identity = matches.remove(0);
    if let Some(account) = crate::config::selected_account() {
        if identity["wxid"].as_str() != Some(account.as_str()) {
            return Err(reader_error(
                "INTERNAL",
                "实际账号与 --wxid 不一致，拒绝读取",
            ));
        }
    }
    Ok(identity)
}

fn groups(conn: &rusqlite::Connection) -> Result<Value> {
    let mut stmt = conn.prepare("SELECT username, nick_name, remark FROM contact WHERE username LIKE '%@chatroom' ORDER BY username").map_err(schema_error)?;
    let rooms = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            ))
        })
        .map_err(schema_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(schema_error)?;
    let mut room_queries = Vec::new();
    for column in ["username", "chat_room_name", "name"] {
        match conn.prepare(&format!("SELECT id FROM chat_room WHERE {column}=?")) {
            Ok(stmt) => room_queries.push(stmt),
            Err(rusqlite::Error::SqliteFailure(_, Some(message)))
            | Err(rusqlite::Error::SqlInputError { msg: message, .. })
                if message.starts_with("no such column:") => {}
            Err(error) => return Err(schema_error(error)),
        }
    }
    if room_queries.is_empty() {
        return Err(reader_error(
            "VERSION_UNSUPPORTED",
            "当前微信联系人或群成员表结构不受支持",
        ));
    }
    let mut member_query = conn.prepare("SELECT DISTINCT c.username, c.nick_name, c.remark FROM chatroom_member cm LEFT JOIN contact c ON c.id=cm.member_id WHERE cm.room_id=? ORDER BY c.username").map_err(schema_error)?;
    let mut result = Vec::new();
    for (wxid, nickname, remark) in rooms {
        let mut room_id = None;
        for query in &mut room_queries {
            room_id = query
                .query_row([&wxid], |row| row.get::<_, i64>(0))
                .optional()
                .map_err(schema_error)?;
            if room_id.is_some() {
                break;
            }
        }
        let members = if let Some(room_id) = room_id {
            member_query
                .query_map([room_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                        row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    ))
                })
                .map_err(schema_error)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(schema_error)?
        } else {
            Vec::new()
        };
        let group_names =
            crate::daemon::query::load_group_nickname_map_from_conn(conn, &wxid, None);
        let members: Vec<_> = members
            .into_iter()
            .map(|(id, nick, remark)| {
                let display = group_names
                    .get(&id)
                    .filter(|name| !name.is_empty())
                    .cloned()
                    .unwrap_or_else(|| display_name(&id, &nick, &remark));
                json!({"wxid": id, "displayName": display})
            })
            .collect();
        result.push(json!({"wxid":wxid,"displayName":display_name(&wxid, &nickname, &remark),"memberCount":members.len(),"members":members,"membersComplete":room_id.is_some()}));
    }
    Ok(Value::Array(result))
}

fn display_name(id: &str, nickname: &str, remark: &str) -> String {
    if !remark.is_empty() {
        remark
    } else if !nickname.is_empty() {
        nickname
    } else {
        id
    }
    .to_owned()
}

pub(crate) fn verify_configured_identity(cfg: &crate::config::Config) -> Result<()> {
    identity(&open_contact(cfg)?, &cfg.db_dir)?;
    Ok(())
}

pub fn cmd_whoami(json: bool) -> Result<()> {
    let (cfg, conn) = reader_connection()?;
    super::output::print_value(
        &identity(&conn, &cfg.db_dir)?,
        &super::output::resolve(json),
    )
}

pub fn cmd_groups(json: bool) -> Result<()> {
    let (cfg, conn) = reader_connection()?;
    // 完整性门禁与 whoami 相同，不读取任何消息表。
    identity(&conn, &cfg.db_dir)?;
    super::output::print_value(&groups(&conn)?, &super::output::resolve(json))
}
