#[cfg(unix)]
#[test]
fn incremental_json_selects_account_pid_and_resumes_after_restart() {
    use rusqlite::Connection;
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};
    use std::{os::unix::fs::PermissionsExt, process::Command};

    let root = std::env::temp_dir().join(format!("wx-incremental-cli-{}", std::process::id()));
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    for name in ["pgrep", "lsof"] {
        let path = bin.join(name);
        std::fs::write(&path, "#!/bin/sh\necho 4242\n").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    for (account, server) in [("wxid_alpha", 101), ("wxid_beta", 201)] {
        let profile = root
            .join("accounts")
            .join(format!("{:x}", Sha256::digest(account)));
        let db_dir = root.join("data").join(account).join("db_storage");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::create_dir_all(db_dir.join("contact")).unwrap();
        std::fs::create_dir_all(db_dir.join("message")).unwrap();
        std::fs::write(
            profile.join("config.json"),
            json!({"db_dir":db_dir}).to_string(),
        )
        .unwrap();
        let contacts = Connection::open(db_dir.join("contact/contact.db")).unwrap();
        contacts
            .execute_batch("CREATE TABLE contact(username TEXT,nick_name TEXT)")
            .unwrap();
        contacts
            .execute("INSERT INTO contact VALUES(?1,'fixture')", [account])
            .unwrap();
        drop(contacts);
        let messages = Connection::open(db_dir.join("message/message_0.db")).unwrap();
        let table = format!("Msg_{:x}", md5::compute("123@chatroom"));
        messages.execute_batch(&format!("CREATE TABLE Name2Id(user_name TEXT); INSERT INTO Name2Id VALUES('{account}'); CREATE TABLE [{table}](local_id INTEGER PRIMARY KEY AUTOINCREMENT,server_id INTEGER,local_type INTEGER,create_time INTEGER,real_sender_id INTEGER,message_content TEXT,WCDB_CT_message_content INTEGER); INSERT INTO [{table}](server_id,local_type,create_time,real_sender_id,message_content,WCDB_CT_message_content) VALUES({server},1,100,1,'one',0),({},1,100,1,'two',0);",server+1)).unwrap();
        drop(messages);
    }
    let invoke = |account: &str, pid: &str, cursor: Option<&str>| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_wx"));
        command
            .env("WX_CLI_HOME", &root)
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env_remove("WX_DAEMON_MODE")
            .env_remove("WX_ACCOUNT")
            .env_remove("WX_PID")
            .args([
                "--wxid",
                account,
                "--pid",
                pid,
                "incremental",
                "123@chatroom",
                "--limit",
                "1",
            ]);
        if let Some(cursor) = cursor {
            command.args(["--cursor", cursor]);
        }
        let out = command.output().unwrap();
        let value: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|_| {
            panic!(
                "stdout={} stderr={}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )
        });
        (out.status.success(), value)
    };
    let (ok, alpha) = invoke("wxid_alpha", "4242", None);
    assert!(ok, "{alpha}");
    assert_eq!(alpha["messages"][0]["server_id"], "101");
    let (ok, beta) = invoke("wxid_beta", "4242", None);
    assert!(ok, "{beta}");
    assert_eq!(beta["messages"][0]["server_id"], "201");
    let (ok, next) = invoke("wxid_alpha", "4242", None);
    assert!(ok, "{next}");
    assert_eq!(next["messages"][0]["server_id"], "102");
    let (ok, replay) = invoke("wxid_alpha", "4242", Some(""));
    assert!(ok, "{replay}");
    assert_eq!(replay, alpha);
    let (ok, mismatch) = invoke("wxid_beta", "4242", alpha["next_cursor"].as_str());
    assert!(!ok);
    assert_eq!(mismatch["error"]["code"], "CURSOR_ACCOUNT_MISMATCH");
    let (ok, wrong_pid) = invoke("wxid_alpha", "9999", None);
    assert!(!ok);
    assert_eq!(wrong_pid["error"]["code"], "LOGIN_REQUIRED");
    let (ok, empty) = invoke("wxid_alpha", "4242", None);
    assert!(ok, "{empty}");
    assert_eq!(empty["messages"], json!([]));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn incremental_without_account_returns_json_before_opening_any_source() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_wx"))
        .env_remove("WX_DAEMON_MODE")
        .env_remove("WX_ACCOUNT")
        .env_remove("WX_PID")
        .args(["incremental", "123@chatroom"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["code"], "ACCOUNT_REQUIRED");
}
