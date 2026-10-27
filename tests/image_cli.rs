#![cfg(unix)]
use image::{DynamicImage, ImageFormat};
use rusqlite::Connection;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs, io::Cursor, os::unix::fs::PermissionsExt, path::PathBuf, process::Command};

const ACCOUNT: &str = "wxid_image_synthetic";
const CHAT: &str = "10001@chatroom";
const MESSAGE: &str = "9007199254740993002";
const MD5: &str = "0123456789abcdef0123456789abcdef";
struct Fixture {
    root: PathBuf,
}
impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("wx-image-{name}-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let f = Self { root };
        fs::create_dir(f.root.join("bin")).unwrap();
        for (name, script) in [
            (
                "pgrep",
                "#!/bin/sh\ncase \"$2\" in WeChat|wechat) echo 4242;; *) exit 1;; esac\n",
            ),
            ("lsof", "#!/bin/sh\necho 4242\n"),
        ] {
            let p = f.root.join("bin").join(name);
            fs::write(&p, script).unwrap();
            fs::set_permissions(p, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::create_dir_all(f.profile()).unwrap();
        fs::create_dir_all(f.db().join("contact")).unwrap();
        fs::create_dir_all(f.db().join("message")).unwrap();
        fs::write(
            f.profile().join("config.json"),
            json!({"db_dir":f.db()}).to_string(),
        )
        .unwrap();
        let c = Connection::open(f.db().join("contact/contact.db")).unwrap();
        c.execute_batch("CREATE TABLE contact(username TEXT,nick_name TEXT);")
            .unwrap();
        c.execute("INSERT INTO contact VALUES(?1,'')", [ACCOUNT])
            .unwrap();
        c.execute("INSERT INTO contact VALUES(?1,'')", [CHAT])
            .unwrap();
        let m = Connection::open(f.db().join("message/message_0.db")).unwrap();
        m.execute_batch(&format!("CREATE TABLE [Msg_{:x}](local_id INTEGER,server_id INTEGER,local_type INTEGER,create_time INTEGER);INSERT INTO [Msg_{:x}] VALUES(7,{MESSAGE},3,1700000000);",md5::compute(CHAT),md5::compute(CHAT))).unwrap();
        let r = Connection::open(f.db().join("message/message_resource.db")).unwrap();
        r.execute_batch("CREATE TABLE ChatName2Id(user_name TEXT); CREATE TABLE MessageResourceInfo(chat_id INTEGER,message_local_id INTEGER,message_local_type INTEGER,message_create_time INTEGER,packed_info BLOB);").unwrap();
        r.execute("INSERT INTO ChatName2Id VALUES(?1)", [CHAT])
            .unwrap();
        let mut packed = vec![0x12, 0x22, 0x0a, 0x20];
        packed.extend_from_slice(MD5.as_bytes());
        r.execute(
            "INSERT INTO MessageResourceInfo VALUES(1,7,3,1700000000,?1)",
            [packed],
        )
        .unwrap();
        fs::create_dir_all(f.img()).unwrap();
        f
    }
    fn profile(&self) -> PathBuf {
        self.root
            .join("accounts")
            .join(format!("{:x}", Sha256::digest(ACCOUNT)))
    }
    fn db(&self) -> PathBuf {
        self.root.join("data").join(ACCOUNT).join("db_storage")
    }
    fn img(&self) -> PathBuf {
        self.db()
            .parent()
            .unwrap()
            .join(format!("msg/attach/{:x}/2023-11/Img", md5::compute(CHAT)))
    }
    fn png(&self, suffix: &str, w: u32, h: u32) {
        let mut b = Cursor::new(Vec::new());
        DynamicImage::new_rgb8(w, h)
            .write_to(&mut b, ImageFormat::Png)
            .unwrap();
        fs::write(
            self.img().join(format!("{MD5}{suffix}.dat")),
            b.into_inner(),
        )
        .unwrap();
    }
    fn invoke(&self, chat: &str, message: &str) -> (i32, Value) {
        self.invoke_with(chat, message, &[])
    }
    fn invoke_with(&self, chat: &str, message: &str, extra: &[&str]) -> (i32, Value) {
        let executable =
            std::env::var_os("WX_TEST_BIN").unwrap_or_else(|| env!("CARGO_BIN_EXE_wx").into());
        let out = Command::new(executable)
            .env("WX_CLI_HOME", &self.root)
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.root.join("bin").display()),
            )
            .env_remove("WX_DAEMON_MODE")
            .env_remove("WX_ACCOUNT")
            .env_remove("WX_PID")
            .args([
                "--wxid",
                ACCOUNT,
                "--pid",
                "4242",
                "image",
                "resolve",
                chat,
                "--message-id",
                message,
                "--output-dir",
            ])
            .arg(self.root.join("output"))
            .arg("--json")
            .args(extra)
            .output()
            .unwrap();
        assert!(out.stderr.is_empty(), "diagnostics must be sanitized");
        let v = serde_json::from_slice(&out.stdout).expect("exactly one JSON response");
        (out.status.code().unwrap(), v)
    }
    fn golden(&self, name: &str, mut v: Value) {
        if v["status"] == "ready" {
            let p = PathBuf::from(v["outputPath"].as_str().unwrap());
            assert!(p.starts_with(self.root.join("output").canonicalize().unwrap()));
            let b = fs::read(p).unwrap();
            let decoded = image::load_from_memory(&b).unwrap();
            assert_eq!(decoded.width(), v["width"].as_u64().unwrap() as u32);
            assert_eq!(decoded.height(), v["height"].as_u64().unwrap() as u32);
            assert_eq!(b.len() as u64, v["byteLength"].as_u64().unwrap());
            assert_eq!(format!("{:x}", Sha256::digest(&b)), v["sha256"]);
            v["outputPath"] = json!("<restricted-output-dir>/image.png");
        }
        let b = format!("{}\n", serde_json::to_string_pretty(&v).unwrap());
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(format!("tests/fixtures/contract/{name}.json"));
        if std::env::var("UPDATE_CONTRACT_FIXTURES").as_deref() == Ok("1") {
            fs::write(&p, &b).unwrap();
        }
        assert_eq!(fs::read_to_string(p).unwrap(), b);
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}
#[test]
fn image_regular_and_pixel_area_selection() {
    let f = Fixture::new("regular");
    f.png("", 8, 8);
    f.png("_h", 16, 8);
    f.png("_t", 4, 4);
    let (exit, v) = f.invoke(CHAT, MESSAGE);
    assert_eq!(exit, 0);
    assert_eq!(v["resourceKind"], "hd");
    assert_eq!(v["messageId"], MESSAGE);
    f.golden("image-regular", v);
}
#[test]
fn image_corrupt_large_falls_back_to_thumbnail() {
    let f = Fixture::new("fallback");
    f.png("_t", 4, 4);
    fs::write(
        f.img().join(format!("{MD5}.dat")),
        b"\x89PNG\r\n\x1a\ncorrupt",
    )
    .unwrap();
    let (exit, v) = f.invoke(CHAT, MESSAGE);
    assert_eq!(exit, 0);
    assert_eq!(v["resourceKind"], "thumbnail");
    f.golden("image-thumbnail-fallback", v);
}
#[test]
fn image_missing_and_long_id_not_rounded() {
    let f = Fixture::new("missing");
    let (exit, v) = f.invoke(CHAT, MESSAGE);
    assert_eq!(exit, 2);
    assert_eq!(v["errorCode"], "LOCAL_RESOURCE_MISSING");
    f.golden("image-resource-missing", v);
    let (exit, v) = f.invoke(CHAT, "9007199254740993003");
    assert_eq!(exit, 2);
    assert_eq!(v["errorCode"], "MESSAGE_NOT_READY");
}
#[test]
fn image_resource_join_never_uses_reused_local_id() {
    let f = Fixture::new("exact");
    f.png("", 8, 8);
    Connection::open(f.db().join("message/message_resource.db"))
        .unwrap()
        .execute(
            "UPDATE MessageResourceInfo SET message_create_time=1700000001",
            [],
        )
        .unwrap();
    let (exit, v) = f.invoke(CHAT, MESSAGE);
    assert_eq!(exit, 2);
    assert_eq!(v["errorCode"], "LOCAL_RESOURCE_MISSING");
}
#[test]
fn image_wrong_account_is_a_hard_identity_failure() {
    let f = Fixture::new("account");
    fs::write(
        f.profile().join("config.json"),
        json!({"db_dir":f.root.join("data/wxid_other/db_storage")}).to_string(),
    )
    .unwrap();
    let (exit, v) = f.invoke(CHAT, MESSAGE);
    assert_eq!(exit, 2);
    assert_eq!(v["errorCode"], "ACCOUNT_MISMATCH");
    assert_eq!(v["retryable"], false);
    f.golden("image-account-mismatch", v);
}
#[test]
fn image_unassociated_bubble_is_ignored() {
    let f = Fixture::new("bubble");
    let b = f.db().parent().unwrap().join("cache/2023-11/Bubble");
    fs::create_dir_all(&b).unwrap();
    fs::write(b.join(format!("{MD5}_b.dat")), b"not-associated").unwrap();
    let (exit, v) = f.invoke(CHAT, MESSAGE);
    assert_eq!(exit, 2);
    assert_eq!(v["errorCode"], "LOCAL_RESOURCE_MISSING");
}

#[test]
fn image_key_failure_has_no_secret_or_plain_output() {
    let f = Fixture::new("key");
    let mut dat = b"\x07\x08V2\x08\x07".to_vec();
    dat.extend_from_slice(&16u32.to_le_bytes());
    dat.extend_from_slice(&0u32.to_le_bytes());
    dat.push(0);
    dat.extend_from_slice(&[0; 32]);
    fs::write(f.img().join(format!("{MD5}.dat")), dat).unwrap();
    let (exit, v) = f.invoke(CHAT, MESSAGE);
    assert_eq!(exit, 2);
    assert_eq!(v["errorCode"], "IMAGE_KEY_UNAVAILABLE");
    assert_eq!(v["retryable"], false);
    assert!(!f.root.join("output").exists());
    f.golden("image-key-unavailable", v);
}
#[test]
fn image_truncated_container_is_retryable_not_a_key_error() {
    let f = Fixture::new("truncated");
    fs::write(f.img().join(format!("{MD5}.dat")), b"\x07\x08V2\x08\x07").unwrap();
    let (exit, v) = f.invoke(CHAT, MESSAGE);
    assert_eq!(exit, 2);
    assert_eq!(v["errorCode"], "RESOURCE_INCOMPLETE");
    assert_eq!(v["retryable"], true);
    assert_eq!(v["stage"], "aes_head");
}
#[test]
fn image_equal_area_prefers_full_path_and_never_fuzzy_matches_chat() {
    let f = Fixture::new("tie");
    f.png("_t", 8, 8);
    f.png("_h", 16, 4);
    f.png("", 4, 16);
    let (exit, v) = f.invoke(CHAT, MESSAGE);
    assert_eq!(exit, 0);
    assert_eq!(v["resourceKind"], "full_path");
    let (exit, v) = f.invoke("10002@chatroom", MESSAGE);
    assert_eq!(exit, 2);
    assert_eq!(v["errorCode"], "MESSAGE_NOT_READY");
}
#[test]
fn image_symlink_resource_cannot_escape_account_tree() {
    let f = Fixture::new("link");
    f.png("_t", 4, 4);
    let outside = f.root.join("outside.dat");
    fs::rename(f.img().join(format!("{MD5}_t.dat")), &outside).unwrap();
    std::os::unix::fs::symlink(outside, f.img().join(format!("{MD5}.dat"))).unwrap();
    let (exit, v) = f.invoke(CHAT, MESSAGE);
    assert_eq!(exit, 2);
    assert_eq!(v["errorCode"], "LOCAL_RESOURCE_MISSING");
}
#[test]
#[ignore = "需要在仓库外临时生成的合成 HEVC 和随包 codec，详见 docs/image-resolve.md"]
fn image_wxgf_real_transcode() {
    let f = Fixture::new("wxgf");
    let stream = fs::read(
        std::env::var_os("WX_TEST_HEVC").expect("WX_TEST_HEVC points to synthetic 16x16 HEVC"),
    )
    .unwrap();
    let mut data = b"wxgf\x0f\0\0\0\0\0\0\0\0\0\0".to_vec();
    data.extend_from_slice(&(stream.len() as u32).to_be_bytes());
    data.extend_from_slice(&stream);
    fs::write(f.img().join(format!("{MD5}.dat")), &data).unwrap();
    let (exit, v) = f.invoke(CHAT, MESSAGE);
    assert_eq!(exit, 0);
    assert_eq!(v["convertedFrom"], "wxgf");
    assert_eq!(v["width"], 16);
    assert_eq!(v["height"], 16);
    f.golden("image-wxgf", v);
    data.pop();
    fs::write(f.img().join(format!("{MD5}.dat")), &data).unwrap();
    let (exit, v) = f.invoke(CHAT, MESSAGE);
    assert_eq!(exit, 2);
    assert_eq!(v["errorCode"], "RESOURCE_INCOMPLETE");
}

#[test]
fn image_output_directory_rejects_shared_or_linked_paths() {
    let f = Fixture::new("output-safety");
    f.png("", 4, 4);
    fs::create_dir(f.root.join("output")).unwrap();
    fs::set_permissions(f.root.join("output"), fs::Permissions::from_mode(0o755)).unwrap();
    let (exit, v) = f.invoke(CHAT, MESSAGE);
    assert_eq!(exit, 1);
    assert!(v["error"].is_object());
    assert_eq!(fs::read_dir(f.root.join("output")).unwrap().count(), 0);
    fs::remove_dir(f.root.join("output")).unwrap();
    std::os::unix::fs::symlink(f.img(), f.root.join("output")).unwrap();
    let (exit, v) = f.invoke(CHAT, MESSAGE);
    assert_eq!(exit, 1);
    assert!(v["error"].is_object());
}

#[test]
fn image_resource_join_rejects_cross_shard_local_id_collision() {
    let f = Fixture::new("collision");
    f.png("", 4, 4);
    let conn = Connection::open(f.db().join("message/message_1.db")).unwrap();
    let table = format!("Msg_{:x}", md5::compute(CHAT));
    conn.execute_batch(&format!("CREATE TABLE [{table}](local_id INTEGER,server_id TEXT,local_type INTEGER,create_time INTEGER); INSERT INTO [{table}] VALUES(7,'different-server-id',3,1700000000);")).unwrap();
    let (exit, v) = f.invoke(CHAT, MESSAGE);
    assert_eq!(exit, 2);
    assert_eq!(v["errorCode"], "MESSAGE_NOT_READY");
}

#[test]
fn image_wait_key_flag_preserves_ready_contract() {
    let f = Fixture::new("wait-ready");
    f.png("", 2, 2);
    let (exit, v) = f.invoke_with(CHAT, MESSAGE, &["--wait-key-seconds", "1"]);
    assert_eq!(exit, 0);
    assert_eq!(v["status"], "ready");
    assert!(v.get("stage").is_none());
}
#[test]
fn image_decode_failure_adds_sanitized_stage() {
    let f = Fixture::new("stage");
    fs::write(
        f.img().join(format!("{MD5}.dat")),
        b"\x89PNG\r\n\x1a\nsynthetic",
    )
    .unwrap();
    let (exit, v) = f.invoke(CHAT, MESSAGE);
    assert_eq!(exit, 2);
    assert_eq!(v["errorCode"], "DECODE_FAILED");
    assert_eq!(v["stage"], "image_decode");
    assert!(!f.root.join("output").exists());
    // 旧 V1 decoder 的未细分异常也必须只有阶段码，不输出异常正文。
    let mut dat = b"\x07\x08V1\x08\x07".to_vec();
    dat.extend_from_slice(&16u32.to_le_bytes());
    dat.extend_from_slice(&0u32.to_le_bytes());
    dat.push(0);
    dat.extend_from_slice(&[0; 32]);
    fs::write(f.img().join(format!("{MD5}.dat")), dat).unwrap();
    let (exit, v) = f.invoke(CHAT, MESSAGE);
    assert_eq!(exit, 2);
    assert_eq!(v["errorCode"], "DECODE_FAILED");
    assert_eq!(v["stage"], "other");
}
#[test]
fn image_harvest_timeout_json_never_includes_identity_paths_or_candidates() {
    let f = Fixture::new("harvest-timeout");
    let mut dat = b"\x07\x08V2\x08\x07".to_vec();
    dat.extend_from_slice(&16u32.to_le_bytes());
    dat.extend_from_slice(&0u32.to_le_bytes());
    dat.push(0);
    dat.extend_from_slice(&[0; 32]);
    fs::write(f.img().join(format!("{MD5}.dat")), dat).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_wx"))
        .env("WX_CLI_HOME", &f.root)
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", f.root.join("bin").display()),
        )
        .env_remove("WX_DAEMON_MODE")
        .env_remove("WX_ACCOUNT")
        .env_remove("WX_PID")
        .args([
            "image",
            "key-harvest",
            "--wxid",
            ACCOUNT,
            "--pid",
            "4242",
            "--chat",
            CHAT,
            "--message-id",
            MESSAGE,
            "--interval-ms",
            "1",
            "--timeout-seconds",
            "0",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stderr.is_empty());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["status"], "unavailable");
    assert_eq!(v["errorCode"], "IMAGE_KEY_HARVEST_TIMEOUT");
    assert_eq!(v["retryable"], true);
    let names: std::collections::HashSet<_> =
        v.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        names,
        std::collections::HashSet::from([
            "status",
            "attempts",
            "elapsedMs",
            "candidateCounts",
            "cacheWritten",
            "errorCode",
            "retryable"
        ])
    );
    assert!(v["candidateCounts"]
        .as_object()
        .unwrap()
        .values()
        .all(Value::is_u64));
    assert_eq!(v["cacheWritten"], false);
    assert!(!out
        .stdout
        .windows(ACCOUNT.len())
        .any(|b| b == ACCOUNT.as_bytes()));
    assert!(!f.root.join("output").exists());
    assert!(!f.profile().join("image-keys").exists());
}
