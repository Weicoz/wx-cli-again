#![cfg(unix)]

use rusqlite::Connection;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, process::Command};

const ACCOUNT: &str = "wxid_contract_self";
const CHAT: &str = "10001@chatroom";
const EMPTY_CHAT: &str = "10002@chatroom";

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("wx-contract-{name}-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let fixture = Self { root };
        let bin = fixture.root.join("bin");
        fs::create_dir(&bin).unwrap();
        for (name, script) in [
            (
                "pgrep",
                "#!/bin/sh\ncase \"$2\" in WeChat|wechat) echo 4242;; *) exit 1;; esac\n",
            ),
            ("lsof", "#!/bin/sh\necho 4242\n"),
        ] {
            let path = bin.join(name);
            fs::write(&path, script).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::create_dir_all(fixture.profile()).unwrap();
        let db_dir = fixture.db_dir();
        fs::create_dir_all(db_dir.join("contact")).unwrap();
        fs::write(
            fixture.profile().join("config.json"),
            json!({"db_dir":db_dir}).to_string(),
        )
        .unwrap();
        let contacts = Connection::open(db_dir.join("contact/contact.db")).unwrap();
        contacts
            .execute_batch("CREATE TABLE contact(id INTEGER PRIMARY KEY, username TEXT, nick_name TEXT, remark TEXT);")
            .unwrap();
        contacts
            .execute("INSERT INTO contact VALUES(1,?1,'合成用户','')", [ACCOUNT])
            .unwrap();
        fixture
    }

    fn profile(&self) -> PathBuf {
        self.root
            .join("accounts")
            .join(format!("{:x}", Sha256::digest(ACCOUNT)))
    }

    fn db_dir(&self) -> PathBuf {
        self.root.join("data").join(ACCOUNT).join("db_storage")
    }

    fn add_groups(&self) {
        let contacts = Connection::open(self.db_dir().join("contact/contact.db")).unwrap();
        contacts
            .execute_batch(
                "CREATE TABLE chat_room(id INTEGER PRIMARY KEY, username TEXT, ext_buffer BLOB);
            CREATE TABLE chatroom_member(room_id INTEGER, member_id INTEGER);
            INSERT INTO contact VALUES(2,'wxid_contract_friend','合成好友','');
            INSERT INTO chatroom_member VALUES(8,1),(8,2);",
            )
            .unwrap();
        contacts
            .execute("INSERT INTO contact VALUES(3,?1,'合成完整群','')", [CHAT])
            .unwrap();
        contacts
            .execute(
                "INSERT INTO contact VALUES(4,?1,'合成待补群','')",
                [EMPTY_CHAT],
            )
            .unwrap();
        contacts
            .execute("INSERT INTO chat_room VALUES(8,?1,NULL)", [CHAT])
            .unwrap();
    }

    #[cfg(target_os = "macos")]
    fn add_messages(&self) {
        use std::os::fd::AsRawFd;

        let directory = self.db_dir().join("message");
        fs::create_dir(&directory).unwrap();
        let path = directory.join("message_0.db");
        let messages = Connection::open(&path).unwrap();
        let table = format!("Msg_{:x}", md5::compute(CHAT));
        messages.execute_batch(&format!("CREATE TABLE Name2Id(user_name TEXT);
            INSERT INTO Name2Id VALUES('{ACCOUNT}'),('wxid_contract_friend');
            CREATE TABLE [{table}](local_id INTEGER PRIMARY KEY, server_id INTEGER, local_type INTEGER,
                create_time INTEGER, real_sender_id INTEGER, message_content TEXT,
                WCDB_CT_message_content INTEGER, source TEXT, WCDB_CT_source INTEGER);")).unwrap();
        let mention = "<msgsource><atuserlist><![CDATA[wxid_contract_friend,notify@all]]></atuserlist></msgsource>";
        let known_quote = "<msg><appmsg><type>57</type><title>合成引用</title><refermsg><svrid>9007199254740993</svrid><chatusr>wxid_contract_friend</chatusr></refermsg></appmsg></msg>";
        let unknown_quote = "<msg><appmsg><type>57</type><title>合成缺失引用</title><refermsg><svrid>9007199254740999</svrid></refermsg></appmsg></msg>";
        for (index, (kind, sender, text, source)) in [
            (1, 2, "wxid_contract_friend:\n合成文本", None),
            (1, 1, "合成全体提醒", Some(mention)),
            (49, 2, known_quote, None),
            (49, 1, unknown_quote, None),
            (10000, 0, "合成系统消息", None),
            (1, 2, "合成下一页", None),
        ]
        .into_iter()
        .enumerate()
        {
            messages
                .execute(
                    &format!("INSERT INTO [{table}] VALUES(?1,?2,?3,1700000000,?4,?5,0,?6,0)"),
                    rusqlite::params![
                        index as i64 + 1,
                        9007199254740993_i64 + index as i64,
                        kind,
                        sender,
                        text,
                        source
                    ],
                )
                .unwrap();
        }
        drop(messages);

        // 固定合成库的 birthtime，使真实 CLI 游标稳定；不得改写 stdout。
        let file = fs::OpenOptions::new().write(true).open(&path).unwrap();
        let mut attributes = libc::attrlist {
            bitmapcount: libc::ATTR_BIT_MAP_COUNT,
            reserved: 0,
            commonattr: libc::ATTR_CMN_CRTIME,
            volattr: 0,
            dirattr: 0,
            fileattr: 0,
            forkattr: 0,
        };
        let mut created = libc::timespec {
            tv_sec: 1700000000,
            tv_nsec: 0,
        };
        // SAFETY: 有效 fd；属性位图对应一个 timespec，缓冲区存活至调用结束。
        let result = unsafe {
            libc::fsetattrlist(
                file.as_raw_fd(),
                (&mut attributes as *mut libc::attrlist).cast(),
                (&mut created as *mut libc::timespec).cast(),
                std::mem::size_of_val(&created),
                0,
            )
        };
        assert_eq!(result, 0, "{}", std::io::Error::last_os_error());
        assert_eq!(
            fs::metadata(path).unwrap().created().unwrap(),
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(1700000000)
        );
    }

    fn invoke(&self, args: &[&str], exit_code: i32) -> Vec<u8> {
        let output = Command::new(env!("CARGO_BIN_EXE_wx"))
            .env("WX_CLI_HOME", &self.root)
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.root.join("bin").display()),
            )
            .env_remove("WX_DAEMON_MODE")
            .env_remove("WX_ACCOUNT")
            .env_remove("WX_PID")
            .args(["--wxid", ACCOUNT, "--pid", "4242"])
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(exit_code), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        assert!(!self.profile().join("daemon.pid").exists());
        output.stdout
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

fn golden(name: &str, stdout: &[u8]) -> Value {
    let value = serde_json::from_slice(stdout).expect("CLI stdout must be JSON");
    assert!(stdout.ends_with(b"\n"), "CLI stdout must end with LF");
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/contract")
        .join(name);
    if std::env::var("UPDATE_CONTRACT_FIXTURES").as_deref() == Ok("1") {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, stdout).unwrap();
    }
    let expected = fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "{}: {error}; generate with UPDATE_CONTRACT_FIXTURES=1 cargo test --test contract_cli",
            path.display()
        )
    });
    if expected != stdout {
        let byte = expected
            .iter()
            .zip(stdout)
            .position(|(a, b)| a != b)
            .unwrap_or(expected.len().min(stdout.len()));
        panic!("{} differs at byte {byte} (expected {} bytes, actual {} bytes)\n--- fixture\n{}\n+++ CLI stdout\n{}",
            path.display(), expected.len(), stdout.len(), String::from_utf8_lossy(&expected), String::from_utf8_lossy(stdout));
    }
    value
}

#[test]
fn whoami_matches_contract() {
    let fixture = Fixture::new("whoami");
    let stdout = fixture.invoke(&["whoami", "--json"], 0);
    let value = golden("whoami.json", &stdout);
    assert_eq!(value, json!({"wxid":ACCOUNT,"displayName":"合成用户"}));
}

#[test]
fn groups_match_contract_including_incomplete_members() {
    let fixture = Fixture::new("groups");
    fixture.add_groups();
    let stdout = fixture.invoke(&["groups", "--json"], 0);
    let value = golden("groups.json", &stdout);
    assert_eq!(value.as_array().unwrap().len(), 2);
    assert_eq!(value[0]["membersComplete"], true);
    assert_eq!(value[0]["memberCount"], 2);
    assert_eq!(
        value[1],
        json!({"wxid":EMPTY_CHAT,"displayName":"合成待补群","memberCount":0,"members":[],"membersComplete":false})
    );
}

#[test]
fn missing_chat_matches_error_contract() {
    let fixture = Fixture::new("missing-chat");
    let stdout = fixture.invoke(&["incremental", "99999@chatroom", "--cursor", ""], 1);
    let value = golden("error-chat-not-found.json", &stdout);
    assert_eq!(value["error"]["code"], "CHAT_NOT_FOUND");
}

#[test]
fn logged_out_account_matches_error_contract() {
    let fixture = Fixture::new("logged-out");
    fs::write(fixture.root.join("bin/lsof"), "#!/bin/sh\nexit 1\n").unwrap();
    let stdout = fixture.invoke(&["whoami", "--json"], 1);
    let value = golden("error-login-required.json", &stdout);
    assert_eq!(value["error"]["code"], "LOGIN_REQUIRED");
}

#[test]
fn known_group_without_message_table_matches_empty_contract() {
    let fixture = Fixture::new("empty");
    fixture.add_groups();
    let stdout = fixture.invoke(&["incremental", EMPTY_CHAT, "--cursor", ""], 0);
    let value = golden("incremental-empty.json", &stdout);
    assert_eq!(value["chat_wxid"], EMPTY_CHAT);
    assert_eq!(value["messages"], json!([]));
    assert_eq!(value["has_more"], false);
    assert!(!value["next_cursor"].as_str().unwrap().is_empty());
}

#[cfg(target_os = "macos")]
#[test]
fn incremental_matches_contract_with_mentions_quotes_and_system_message() {
    let fixture = Fixture::new("incremental");
    fixture.add_messages();
    let stdout = fixture.invoke(&["incremental", CHAT, "--cursor", "", "--limit", "5"], 0);
    let value = golden("incremental.json", &stdout);
    let messages = value["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 5);
    assert_eq!(messages[0]["type"], 1);
    assert_eq!(messages[0]["text"], "合成文本");
    assert_eq!(messages[0]["server_id"], "9007199254740993");
    assert_eq!(
        messages[1]["mentions"],
        json!(["wxid_contract_friend", "notify@all"])
    );
    assert_eq!(messages[1]["is_self"], true);
    assert_eq!(
        messages[2]["quote"],
        json!({"message_id":"9007199254740993","sender_wxid":"wxid_contract_friend"})
    );
    assert_eq!(
        messages[3]["quote"],
        json!({"message_id":"9007199254740999","sender_wxid":null})
    );
    assert_eq!(messages[4]["type"], 10000);
    assert!(messages[4]["sender_wxid"].is_null());
    assert!(messages[4]["is_self"].is_null());
    assert!(messages
        .iter()
        .all(|message| message["timestamp"] == 1700000000));
    assert_eq!(value["has_more"], true);
}
