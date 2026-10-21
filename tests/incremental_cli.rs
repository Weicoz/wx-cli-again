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
        if account == "wxid_alpha" {
            contacts
                .execute(
                    "INSERT INTO contact VALUES('empty@chatroom','empty group')",
                    [],
                )
                .unwrap();
        }
        drop(contacts);
        let messages = Connection::open(db_dir.join("message/message_0.db")).unwrap();
        let table = format!("Msg_{:x}", md5::compute("123@chatroom"));
        messages.execute_batch(&format!("CREATE TABLE Name2Id(user_name TEXT); INSERT INTO Name2Id VALUES('{account}'); CREATE TABLE [{table}](local_id INTEGER PRIMARY KEY AUTOINCREMENT,server_id INTEGER,local_type INTEGER,create_time INTEGER,real_sender_id INTEGER,message_content TEXT,WCDB_CT_message_content INTEGER,source BLOB,WCDB_CT_source INTEGER); INSERT INTO [{table}](server_id,local_type,create_time,real_sender_id,message_content,WCDB_CT_message_content) VALUES({server},1,100,1,'one',0),({},1,100,1,'two',0);",server+1)).unwrap();
        let source =
            "<msgsource><atuserlist><![CDATA[wxid_service,wxid_friend]]></atuserlist></msgsource>";
        messages
            .execute(
                &format!("UPDATE [{table}] SET source=?1,WCDB_CT_source=4 WHERE server_id=?2"),
                rusqlite::params![zstd::encode_all(source.as_bytes(), 1).unwrap(), server],
            )
            .unwrap();
        let quote = format!("<msg><appmsg><type>57</type><refermsg><svrid>{server}</svrid><fromusr>wxid_wrong</fromusr></refermsg></appmsg></msg>");
        messages
            .execute(
                &format!(
                    "UPDATE [{table}] SET message_content=?1,local_type=49 WHERE server_id=?2"
                ),
                rusqlite::params![quote, server + 1],
            )
            .unwrap();
        drop(messages);
    }
    let invoke_chat = |account: &str, pid: &str, chat: &str, cursor: Option<&str>| {
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
                chat,
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
    let invoke = |account: &str, pid: &str, cursor: Option<&str>| {
        invoke_chat(account, pid, "123@chatroom", cursor)
    };
    let (ok, alpha) = invoke("wxid_alpha", "4242", None);
    assert!(ok, "{alpha}");
    assert_eq!(alpha["messages"][0]["server_id"], "101");
    assert_eq!(
        alpha["messages"][0]["mentions"],
        json!(["wxid_service", "wxid_friend"])
    );
    assert_eq!(
        alpha["messages"][0]["raw"],
        json!({"message_content":"one","source":"<msgsource><atuserlist><![CDATA[wxid_service,wxid_friend]]></atuserlist></msgsource>"})
    );
    assert!(alpha["messages"][0]
        .as_object()
        .unwrap()
        .contains_key("quote"));
    assert_eq!(alpha["messages"][0]["quote"], Value::Null);
    let (ok, beta) = invoke("wxid_beta", "4242", None);
    assert!(ok, "{beta}");
    assert_eq!(beta["messages"][0]["server_id"], "201");
    let (ok, next) = invoke("wxid_alpha", "4242", None);
    assert!(ok, "{next}");
    assert_eq!(next["messages"][0]["server_id"], "102");
    assert_eq!(
        next["messages"][0]["quote"],
        json!({"message_id":"101","sender_wxid":"wxid_alpha"})
    );
    assert_eq!(next["messages"][0]["mentions"], json!([]));
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
    let (ok, unknown) = invoke_chat("wxid_beta", "4242", "empty@chatroom", Some(""));
    assert!(!ok);
    assert_eq!(unknown["error"]["code"], "CHAT_NOT_FOUND");
    let (ok, first_empty) = invoke_chat("wxid_alpha", "4242", "empty@chatroom", Some(""));
    assert!(ok, "{first_empty}");
    assert_eq!(first_empty["account_wxid"], "wxid_alpha");
    assert_eq!(first_empty["chat_wxid"], "empty@chatroom");
    assert_eq!(first_empty["messages"], json!([]));
    assert_eq!(first_empty["has_more"], false);
    assert!(!first_empty["next_cursor"].as_str().unwrap().is_empty());
    let (ok, replay) = invoke_chat("wxid_alpha", "4242", "empty@chatroom", Some(""));
    assert!(ok, "{replay}");
    assert_eq!(replay, first_empty);
    let (ok, next_empty) = invoke_chat(
        "wxid_alpha",
        "4242",
        "empty@chatroom",
        first_empty["next_cursor"].as_str(),
    );
    assert!(ok, "{next_empty}");
    assert_eq!(next_empty["messages"], json!([]));
    assert_eq!(next_empty["has_more"], false);
    assert_ne!(next_empty["next_cursor"], first_empty["next_cursor"]);
    let messages =
        Connection::open(root.join("data/wxid_alpha/db_storage/message/message_0.db")).unwrap();
    let table = format!("Msg_{:x}", md5::compute("empty@chatroom"));
    messages.execute_batch(&format!("CREATE TABLE [{table}](local_id INTEGER PRIMARY KEY AUTOINCREMENT,server_id INTEGER,local_type INTEGER,create_time INTEGER,real_sender_id INTEGER,message_content TEXT,WCDB_CT_message_content INTEGER); INSERT INTO [{table}](server_id,local_type,create_time,real_sender_id,message_content,WCDB_CT_message_content) VALUES(301,1,100,1,'first group message',0),(302,1,100,1,'second group message',0);")).unwrap();
    drop(messages);
    for (cursor, expected) in [
        (Some(""), &first_empty),
        (first_empty["next_cursor"].as_str(), &next_empty),
    ] {
        let (ok, replay) = invoke_chat("wxid_alpha", "4242", "empty@chatroom", cursor);
        assert!(ok, "{replay}");
        assert_eq!(&replay, expected);
    }
    let (ok, first_message) = invoke_chat(
        "wxid_alpha",
        "4242",
        "empty@chatroom",
        next_empty["next_cursor"].as_str(),
    );
    assert!(ok, "{first_message}");
    assert_eq!(first_message["messages"].as_array().unwrap().len(), 1);
    assert_eq!(first_message["messages"][0]["server_id"], "301");
    assert_eq!(first_message["messages"][0]["local_id"], 1);
    assert_eq!(first_message["has_more"], true);
    let (ok, second_message) = invoke_chat(
        "wxid_alpha",
        "4242",
        "empty@chatroom",
        first_message["next_cursor"].as_str(),
    );
    assert!(ok, "{second_message}");
    assert_eq!(second_message["messages"].as_array().unwrap().len(), 1);
    assert_eq!(second_message["messages"][0]["server_id"], "302");
    assert_eq!(second_message["has_more"], false);
    let (ok, drained) = invoke_chat(
        "wxid_alpha",
        "4242",
        "empty@chatroom",
        second_message["next_cursor"].as_str(),
    );
    assert!(ok, "{drained}");
    assert_eq!(drained["messages"], json!([]));
    assert_eq!(drained["has_more"], false);

    let db_dir = root.join("data/wxid_alpha/db_storage");
    let contacts = Connection::open(db_dir.join("contact/contact.db")).unwrap();
    contacts
        .execute(
            "INSERT INTO contact VALUES('unborn@chatroom','new group')",
            [],
        )
        .unwrap();
    drop(contacts);
    std::fs::remove_dir_all(db_dir.join("message")).unwrap();
    let (ok, no_directory) = invoke_chat("wxid_alpha", "4242", "unborn@chatroom", None);
    assert!(ok, "{no_directory}");
    assert_eq!(no_directory["messages"], json!([]));
    assert_eq!(no_directory["has_more"], false);
    std::fs::create_dir_all(db_dir.join("message")).unwrap();
    let (ok, no_shards) = invoke_chat("wxid_alpha", "4242", "unborn@chatroom", None);
    assert!(ok, "{no_shards}");
    assert_eq!(no_shards["messages"], json!([]));
    assert_eq!(no_shards["has_more"], false);
    assert_ne!(no_shards["next_cursor"], no_directory["next_cursor"]);
    let (ok, unknown) = invoke_chat("wxid_alpha", "4242", "unknown@chatroom", Some(""));
    assert!(!ok);
    assert_eq!(unknown["error"]["code"], "CHAT_NOT_FOUND");
    let messages = Connection::open(db_dir.join("message/message_0.db")).unwrap();
    let table = format!("Msg_{:x}", md5::compute("unborn@chatroom"));
    messages.execute_batch(&format!("CREATE TABLE Name2Id(user_name TEXT); INSERT INTO Name2Id VALUES('wxid_alpha'); CREATE TABLE [{table}](local_id INTEGER PRIMARY KEY AUTOINCREMENT,server_id INTEGER,local_type INTEGER,create_time INTEGER,real_sender_id INTEGER,message_content TEXT,WCDB_CT_message_content INTEGER); INSERT INTO [{table}](server_id,local_type,create_time,real_sender_id,message_content,WCDB_CT_message_content) VALUES(401,1,100,1,'first shard message',0);")).unwrap();
    drop(messages);
    for (cursor, expected) in [
        (Some(""), &no_directory),
        (no_directory["next_cursor"].as_str(), &no_shards),
    ] {
        let (ok, replay) = invoke_chat("wxid_alpha", "4242", "unborn@chatroom", cursor);
        assert!(ok, "{replay}");
        assert_eq!(&replay, expected);
    }
    let (ok, first_shard) = invoke_chat("wxid_alpha", "4242", "unborn@chatroom", None);
    assert!(ok, "{first_shard}");
    assert_eq!(first_shard["messages"].as_array().unwrap().len(), 1);
    assert_eq!(first_shard["messages"][0]["server_id"], "401");
    assert_eq!(first_shard["messages"][0]["local_id"], 1);
    let (ok, drained) = invoke_chat("wxid_alpha", "4242", "unborn@chatroom", None);
    assert!(ok, "{drained}");
    assert_eq!(drained["messages"], json!([]));
    for account in ["wxid_alpha", "wxid_beta"] {
        assert!(!root
            .join("accounts")
            .join(format!("{:x}", Sha256::digest(account)))
            .join("daemon.pid")
            .exists());
    }
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
