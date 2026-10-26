mod codec;
mod key_cache;
mod secure_fs;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    MessageNotReady,
    LocalResourceMissing,
    ImageKeyUnavailable,
    ResourceIncomplete,
    DecodeFailed,
    FormatUnsupported,
    TranscodeFailed,
    AccountMismatch,
    Timeout,
}
impl Failure {
    fn code(self) -> &'static str {
        match self {
            Self::MessageNotReady => "MESSAGE_NOT_READY",
            Self::LocalResourceMissing => "LOCAL_RESOURCE_MISSING",
            Self::ImageKeyUnavailable => "IMAGE_KEY_UNAVAILABLE",
            Self::ResourceIncomplete => "RESOURCE_INCOMPLETE",
            Self::DecodeFailed => "DECODE_FAILED",
            Self::FormatUnsupported => "FORMAT_UNSUPPORTED",
            Self::TranscodeFailed => "TRANSCODE_FAILED",
            Self::AccountMismatch => "ACCOUNT_MISMATCH",
            Self::Timeout => "TIMEOUT",
        }
    }
    fn retryable(self) -> bool {
        matches!(
            self,
            Self::MessageNotReady
                | Self::LocalResourceMissing
                | Self::ResourceIncomplete
                | Self::Timeout
        )
    }
}

use crate::{attachment::decoder, cli::account, config};
use anyhow::{anyhow, Result};
use rusqlite::params;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Response {
    contract_version: u32,
    account_wxid: String,
    chat_id: String,
    message_id: String,
    status: &'static str,
    resource_kind: Option<&'static str>,
    output_path: Option<String>,
    mime_type: Option<&'static str>,
    width: Option<u32>,
    height: Option<u32>,
    byte_length: Option<u64>,
    sha256: Option<String>,
    converted_from: Option<&'static str>,
    error_code: Option<&'static str>,
    retryable: bool,
}
impl Response {
    fn new(account: &str, chat: &str, message: &str) -> Self {
        Self {
            contract_version: 1,
            account_wxid: account.into(),
            chat_id: chat.into(),
            message_id: message.into(),
            status: "unavailable",
            resource_kind: None,
            output_path: None,
            mime_type: None,
            width: None,
            height: None,
            byte_length: None,
            sha256: None,
            converted_from: None,
            error_code: None,
            retryable: false,
        }
    }
    fn failed(mut self, f: Failure) -> Self {
        self.error_code = Some(f.code());
        self.retryable = f.retryable();
        self
    }
}

/// 新路径直接只读查询，不启动 daemon；故图片 key 失败不会改变文本读取状态。
pub(crate) fn run(
    account_id: Option<&str>,
    pid: Option<u32>,
    chat: &str,
    message: &str,
    output: &Path,
) -> i32 {
    match resolve(account_id, pid, chat, message, output) {
        Ok(response) => {
            let exit = if response.status == "ready" { 0 } else { 2 };
            println!(
                "{}",
                serde_json::to_string(&response).expect("serializable response")
            );
            exit
        }
        Err(e) => {
            let code = e
                .downcast_ref::<account::ReaderError>()
                .map_or("PROCESS_FAILED", |e| e.code);
            // 数据库/进程故障保留既有停读错误码，异常正文绝不直接进入输出。
            println!(
                "{}",
                json!({"error":{"code":code,"message":"图片读取未完成，请检查账号、参数与数据库状态"}})
            );
            1
        }
    }
}
fn resolve(
    account_id: Option<&str>,
    pid: Option<u32>,
    chat: &str,
    message: &str,
    output: &Path,
) -> Result<Response> {
    let account_id = account_id.ok_or_else(|| anyhow!("需要 --wxid"))?;
    config::select_account(Some(account_id), pid)?;
    anyhow::ensure!(
        chat.ends_with("@chatroom")
            && chat.len() <= 256
            && chat
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_@-".contains(&b)),
        "需要精确群 wxid"
    );
    anyhow::ensure!(
        !message.is_empty()
            && message.len() <= 40
            && message.bytes().all(|b| b.is_ascii_digit())
            && message != "0",
        "需要字符串服务端消息 ID"
    );
    anyhow::ensure!(
        output.is_absolute()
            && !output
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir)),
        "输出目录必须为绝对受限目录"
    );
    let response = Response::new(account_id, chat, message);
    let config_path = config::find_config_file()?;
    if config_path.exists() {
        let raw: Value = serde_json::from_slice(&std::fs::read(config_path)?)?;
        if raw
            .get("db_dir")
            .and_then(Value::as_str)
            .is_some_and(|p| !config::account_matches_dir(account_id, Path::new(p)))
        {
            return Ok(response.failed(Failure::AccountMismatch));
        }
    }
    let (cfg, contact) = account::reader_connection()?;
    let self_exists: bool = contact.query_row(
        "SELECT EXISTS(SELECT 1 FROM contact WHERE username=?1)",
        [account_id],
        |r| r.get(0),
    )?;
    if !self_exists {
        return Ok(response.failed(Failure::AccountMismatch));
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    let target = locate_message(&cfg, chat, message)?;
    let Some((local, time)) = target else {
        return Ok(response.failed(Failure::MessageNotReady));
    };
    let Some(md5) = resource_md5(&cfg, chat, local, time)? else {
        return Ok(response.failed(Failure::LocalResourceMissing));
    };
    let candidates = candidates(&cfg.db_dir, chat, &md5)?;
    if candidates.is_empty() {
        return Ok(response.failed(Failure::LocalResourceMissing));
    }
    let mut provider = key_cache::Provider::new(account_id, &cfg.db_dir)?;
    let mut best: Option<(Candidate, codec::StandardImage)> = None;
    let mut failure = Failure::LocalResourceMissing;
    for c in candidates {
        if Instant::now() >= deadline {
            failure = Failure::Timeout;
            break;
        }
        let decoded = read_candidate(&c.path)
            .and_then(|dat| {
                if dat.starts_with(&decoder::V2_MAGIC) {
                    provider.decode(&dat)
                } else if dat.starts_with(&decoder::V1_MAGIC) {
                    if dat.len() < 31 {
                        return Err(Failure::ResourceIncomplete);
                    }
                    decoder::dispatch(&dat, decoder::V2KeyMaterial::default())
                        .map(|d| d.data)
                        .map_err(|_| Failure::DecodeFailed)
                } else if decoder::detect_image_format(&dat) != "bin" {
                    Ok(dat)
                } else {
                    decoder::dispatch(&dat, decoder::V2KeyMaterial::default())
                        .map(|d| d.data)
                        .map_err(|_| Failure::FormatUnsupported)
                }
            })
            .and_then(|plain| codec::standard(&plain, deadline));
        match decoded {
            Ok(img) => {
                let area = u64::from(img.width) * u64::from(img.height);
                if best.as_ref().is_none_or(|(old, b)| {
                    area > u64::from(b.width) * u64::from(b.height)
                        || (area == u64::from(b.width) * u64::from(b.height) && c.rank < old.rank)
                }) {
                    best = Some((c, img));
                }
            }
            Err(Failure::AccountMismatch) => return Ok(response.failed(Failure::AccountMismatch)),
            Err(f) => {
                if failure_priority(f) > failure_priority(failure) {
                    failure = f
                }
            }
        }
    }
    let Some((candidate, img)) = best else {
        return Ok(response.failed(failure));
    };
    // 在改 ACL 或创建目录前先拒绝源库、配置目录及其祖先目录。
    let base = cfg
        .db_dir
        .parent()
        .ok_or_else(|| anyhow!("数据根缺失"))?
        .canonicalize()?;
    let profile = config::cli_dir().canonicalize()?;
    let projected = projected_output(output)?;
    anyhow::ensure!(
        !projected.starts_with(&base)
            && !base.starts_with(&projected)
            && !projected.starts_with(&profile)
            && !profile.starts_with(&projected),
        "输出目录不得覆盖账号数据或配置目录"
    );
    let dir = secure_fs::private_dir(output)?;
    anyhow::ensure!(dir == projected, "输出目录在检查后发生变化");
    let mut file = tempfile::Builder::new()
        .prefix("image-")
        .suffix(".png")
        .tempfile_in(&dir)?;
    file.write_all(&img.bytes)?;
    file.as_file().sync_all()?;
    let (_, path) = file.keep()?;
    Ok(Response {
        status: "ready",
        resource_kind: Some(candidate.kind),
        output_path: Some(path.to_string_lossy().into_owned()),
        mime_type: Some("image/png"),
        width: Some(img.width),
        height: Some(img.height),
        byte_length: Some(img.bytes.len() as u64),
        sha256: Some(format!("{:x}", Sha256::digest(&img.bytes))),
        converted_from: img.converted_from,
        ..response
    })
}
fn projected_output(path: &Path) -> Result<PathBuf> {
    let mut ancestor = path;
    let mut missing = Vec::new();
    while !ancestor.exists() {
        missing.push(
            ancestor
                .file_name()
                .ok_or_else(|| anyhow!("输出目录无效"))?
                .to_os_string(),
        );
        ancestor = ancestor
            .parent()
            .ok_or_else(|| anyhow!("输出目录无父目录"))?;
    }
    let mut canonical = ancestor.canonicalize()?;
    for component in missing.into_iter().rev() {
        canonical.push(component);
    }
    Ok(canonical)
}
fn failure_priority(f: Failure) -> u8 {
    match f {
        Failure::LocalResourceMissing => 0,
        Failure::FormatUnsupported => 1,
        Failure::DecodeFailed => 2,
        Failure::TranscodeFailed => 3,
        Failure::ResourceIncomplete => 4,
        Failure::ImageKeyUnavailable => 5,
        Failure::Timeout => 6,
        _ => 7,
    }
}
fn locate_message(cfg: &config::Config, chat: &str, message: &str) -> Result<Option<(i64, i64)>> {
    let dir = cfg.db_dir.join("message");
    let mut found = None;
    let mut shards = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let table = format!("Msg_{:x}", md5::compute(chat));
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(number) = name
            .strip_prefix("message_")
            .and_then(|n| n.strip_suffix(".db"))
        else {
            continue;
        };
        if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let conn = account::open_database(cfg, &format!("message/{name}"))?;
        conn.execute_batch("BEGIN DEFERRED")?;
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            [&table],
            |r| r.get(0),
        )?;
        if !exists {
            continue;
        }
        let mut stmt=conn.prepare(&format!("SELECT local_id,create_time FROM [{table}] WHERE CAST(server_id AS TEXT)=?1 AND (local_type & 4294967295)=3"))?;
        let rows = stmt.query_map([message], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let row = row?;
            if found.is_some() {
                return Ok(None);
            }
            found = Some(row);
        }
        drop(stmt);
        shards.push(conn);
    }
    if let Some((local, time)) = found {
        for conn in shards {
            let collision:bool=conn.query_row(
                &format!("SELECT EXISTS(SELECT 1 FROM [{table}] WHERE local_id=?1 AND create_time=?2 AND (local_type & 4294967295)=3 AND COALESCE(CAST(server_id AS TEXT),'')!=?3)"),
                params![local,time,message],|r|r.get(0)
            )?;
            if collision {
                return Ok(None);
            }
        }
    }
    Ok(found)
}
fn resource_md5(cfg: &config::Config, chat: &str, local: i64, time: i64) -> Result<Option<String>> {
    if !cfg.db_dir.join("message/message_resource.db").is_file() {
        return Ok(None);
    }
    let conn = account::open_database(cfg, "message/message_resource.db")?;
    let mut stmt=conn.prepare("SELECT r.packed_info FROM MessageResourceInfo r JOIN ChatName2Id c ON c.rowid=r.chat_id WHERE c.user_name=?1 AND r.message_local_id=?2 AND r.message_create_time=?3 AND (r.message_local_type & 4294967295)=3")?;
    let mut rows = stmt.query(params![chat, local, time])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let blob: Vec<u8> = row.get(0)?;
    if rows.next()?.is_some() {
        return Ok(None);
    }
    // 新接缝仅采纳确定的 protobuf 字段，不沿用旧 extract 的任意 hex fallback。
    let hashes: Vec<String> = blob
        .windows(36)
        .filter_map(|w| {
            if w[..4] != [0x12, 0x22, 0x0a, 0x20] {
                return None;
            }
            let s = std::str::from_utf8(&w[4..]).ok()?;
            s.bytes()
                .all(|b| b.is_ascii_hexdigit())
                .then(|| s.to_ascii_lowercase())
        })
        .collect();
    Ok(if hashes.len() == 1 {
        hashes.into_iter().next()
    } else {
        None
    })
}
struct Candidate {
    path: PathBuf,
    kind: &'static str,
    rank: u8,
}
fn candidates(db: &Path, chat: &str, md5: &str) -> Result<Vec<Candidate>> {
    let root = db
        .parent()
        .ok_or_else(|| anyhow!("数据目录无父目录"))?
        .join(format!("msg/attach/{:x}", md5::compute(chat)));
    let mut out = Vec::new();
    if !root.is_dir() {
        return Ok(out);
    }
    if secure_fs::is_link(&root)? {
        return Ok(out);
    }
    let base = db.parent().unwrap().canonicalize()?;
    let mut months: Vec<_> = std::fs::read_dir(&root)?.collect::<std::io::Result<Vec<_>>>()?;
    months.sort_by_key(|e| e.file_name());
    for month in months.into_iter().take(256) {
        if !month.file_type()?.is_dir() {
            continue;
        }
        let img = month.path().join("Img");
        if !img.is_dir() || secure_fs::is_link(&img)? {
            continue;
        }
        for (suffix, kind, rank) in [
            ("", "full_path", 0),
            ("_h", "hd", 1),
            ("_t", "thumbnail", 3),
        ] {
            let path = img.join(format!("{md5}{suffix}.dat"));
            if path.is_file()
                && !secure_fs::is_link(&path)?
                && path.canonicalize()?.starts_with(&base)
            {
                out.push(Candidate { path, kind, rank });
            }
        }
    }
    // Bubble 的当前资源库未给出可靠消息映射，明确拒绝名称/大小相近的猜测。
    Ok(out)
}
fn read_candidate(path: &Path) -> std::result::Result<Vec<u8>, Failure> {
    let mut file = std::fs::File::open(path).map_err(|_| Failure::LocalResourceMissing)?;
    let before = file.metadata().map_err(|_| Failure::LocalResourceMissing)?;
    if before.len() == 0 {
        return Err(Failure::ResourceIncomplete);
    }
    if before.len() > 64 * 1024 * 1024 {
        return Err(Failure::FormatUnsupported);
    }
    let mut data = Vec::new();
    Read::by_ref(&mut file)
        .take(64 * 1024 * 1024 + 1)
        .read_to_end(&mut data)
        .map_err(|_| Failure::ResourceIncomplete)?;
    let after = file.metadata().map_err(|_| Failure::ResourceIncomplete)?;
    if data.len() as u64 != before.len()
        || after.len() != before.len()
        || after.modified().ok() != before.modified().ok()
    {
        return Err(Failure::ResourceIncomplete);
    }
    Ok(data)
}
