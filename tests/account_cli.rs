use std::process::Command;

#[test]
fn whoami_json_reports_missing_profile_without_reading_legacy_config() {
    let root = std::env::temp_dir().join(format!("wx-account-cli-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("config.json"), "not valid legacy config").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_wx"))
        .env("WX_CLI_HOME", &root)
        .env_remove("WX_DAEMON_MODE")
        .env_remove("WX_ACCOUNT")
        .env_remove("WX_PID")
        .args(["--wxid", "wxid_missing", "whoami", "--json"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["error"]["code"], "LOGIN_REQUIRED");
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn selected_profile_reads_its_own_identity_and_groups_without_messages() {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;
    let root = std::env::temp_dir().join(format!("wx-catalog-cli-{}", std::process::id()));
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    for name in ["pgrep", "lsof"] {
        let path = bin.join(name);
        std::fs::write(&path, "#!/bin/sh\necho 4242\n").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut outputs = Vec::new();
    for (wxid, nickname, group) in [
        ("wxid_alpha", "甲", "101@chatroom"),
        ("wxid_beta", "乙", "202@chatroom"),
    ] {
        let profile = root
            .join("accounts")
            .join(format!("{:x}", Sha256::digest(wxid.as_bytes())));
        let db_dir = root
            .join("data")
            .join(format!("{wxid}_abcd"))
            .join("db_storage");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::create_dir_all(db_dir.join("contact")).unwrap();
        std::fs::write(
            profile.join("config.json"),
            serde_json::json!({"db_dir":db_dir}).to_string(),
        )
        .unwrap();
        let conn = rusqlite::Connection::open(db_dir.join("contact/contact.db")).unwrap();
        conn.execute_batch("CREATE TABLE contact(id INTEGER PRIMARY KEY, username TEXT, nick_name TEXT, remark TEXT); CREATE TABLE chat_room(id INTEGER PRIMARY KEY, username TEXT, ext_buffer BLOB); CREATE TABLE chatroom_member(room_id INTEGER, member_id INTEGER);").unwrap();
        conn.execute("INSERT INTO contact VALUES(1,?1,?2,'')", [wxid, nickname])
            .unwrap();
        conn.execute("INSERT INTO contact VALUES(2,?1,'客服群','')", [group])
            .unwrap();
        conn.execute("INSERT INTO contact VALUES(3,'friend','朋友','备注')", [])
            .unwrap();
        conn.execute("INSERT INTO chat_room VALUES(8,?1,NULL)", [group])
            .unwrap();
        conn.execute_batch("INSERT INTO chatroom_member VALUES(8,1),(8,3);")
            .unwrap();
        // protobuf: member(username=friend, group_nickname=Group Card)
        let card = b"\x0a\x14\x0a\x06friend\x12\x0aGroup Card";
        conn.execute("UPDATE chat_room SET ext_buffer=?1", [card.as_slice()])
            .unwrap();
        drop(conn);
        let invoke = |command: &str| {
            let out = Command::new(env!("CARGO_BIN_EXE_wx"))
                .env("WX_CLI_HOME", &root)
                .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
                .env_remove("WX_DAEMON_MODE")
                .env_remove("WX_ACCOUNT")
                .env_remove("WX_PID")
                .args(["--wxid", wxid, command, "--json"])
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stdout)
            );
            serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap()
        };
        assert_eq!(
            invoke("whoami"),
            serde_json::json!({"wxid":wxid,"displayName":nickname})
        );
        let groups = invoke("groups");
        assert_eq!(
            groups,
            serde_json::json!([{"wxid":group,"displayName":"客服群","memberCount":2,"members":[{"wxid":"friend","displayName":"Group Card"},{"wxid":wxid,"displayName":nickname}]}])
        );
        outputs.push(groups);
    }
    assert_ne!(outputs[0], outputs[1]);
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn json_contract_distinguishes_keys_schema_login_and_internal_errors() {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;
    let root = std::env::temp_dir().join(format!("wx-errors-cli-{}", std::process::id()));
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    for name in ["pgrep", "lsof"] {
        let path = bin.join(name);
        std::fs::write(&path, "#!/bin/sh\necho 4242\n").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let profile = root
        .join("accounts")
        .join(format!("{:x}", Sha256::digest(b"wxid_errors")));
    let db_dir = root.join("data/wxid_errors/db_storage");
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::create_dir_all(db_dir.join("contact")).unwrap();
    std::fs::write(
        profile.join("config.json"),
        serde_json::json!({"db_dir":db_dir}).to_string(),
    )
    .unwrap();
    let invoke = |command: &str, wxid: &str, code: &str| {
        let out = Command::new(env!("CARGO_BIN_EXE_wx"))
            .env("WX_CLI_HOME", &root)
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env_remove("WX_DAEMON_MODE")
            .env_remove("WX_ACCOUNT")
            .env_remove("WX_PID")
            .args(["--wxid", wxid, command, "--json"])
            .output()
            .unwrap();
        assert!(!out.status.success());
        let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(value["error"]["code"], code, "{value}");
        assert!(!value["error"]["message"].as_str().unwrap().is_empty());
    };
    let path = db_dir.join("contact/contact.db");
    std::fs::write(&path, [0xabu8; 4096]).unwrap();
    invoke("whoami", "wxid_errors", "KEY_FAILED");
    std::fs::remove_file(&path).unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE wrong_version(id INTEGER)")
        .unwrap();
    drop(conn);
    invoke("whoami", "wxid_errors", "VERSION_UNSUPPORTED");
    invoke("groups", "wxid_errors", "VERSION_UNSUPPORTED");
    invoke("whoami", "../invalid", "INTERNAL");
    let overlap = root.join("data/wxid_errors_abcd/db_storage");
    std::fs::create_dir_all(overlap.join("contact")).unwrap();
    let conn = rusqlite::Connection::open(overlap.join("contact/contact.db")).unwrap();
    conn.execute_batch("CREATE TABLE contact(username TEXT,nick_name TEXT); INSERT INTO contact VALUES('wxid_errors_abcd','other account');").unwrap();
    drop(conn);
    std::fs::write(
        profile.join("config.json"),
        serde_json::json!({"db_dir":overlap}).to_string(),
    )
    .unwrap();
    invoke("whoami", "wxid_errors", "INTERNAL");
    invoke("groups", "wxid_errors", "INTERNAL");
    let out = Command::new(env!("CARGO_BIN_EXE_wx"))
        .env("WX_CLI_HOME", &root)
        .env_remove("WX_DAEMON_MODE")
        .env_remove("WX_ACCOUNT")
        .env_remove("WX_PID")
        .args(["--wxid", "wxid_errors", "contacts", "--json"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(!profile.join("daemon.pid").exists());
    std::fs::write(profile.join("all_keys.json"), "{}").unwrap();
    let export = root.join("must-not-export.silk");
    let output_path = export.to_str().unwrap();
    for args in [
        vec!["init"],
        vec!["media", "voice", "1", "--output", output_path],
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_wx"))
            .env("WX_CLI_HOME", &root)
            .env_remove("WX_DAEMON_MODE")
            .env_remove("WX_ACCOUNT")
            .env_remove("WX_PID")
            .args(["--wxid", "wxid_errors"])
            .args(args)
            .output()
            .unwrap();
        assert!(!out.status.success());
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("实际账号与 --wxid 不一致"),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    assert!(!export.exists());

    // 保留离线库但撤销占用：不得把配置中的账号冒充当前登录身份。
    std::fs::write(bin.join("lsof"), "#!/bin/sh\nexit 1\n").unwrap();
    invoke("whoami", "wxid_errors", "LOGIN_REQUIRED");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn captured_cli_returns_while_two_account_daemons_keep_running() {
    use sha2::{Digest, Sha256};
    use std::time::Duration;
    let root = std::env::temp_dir().join(format!("wx-daemons-cli-{}", std::process::id()));
    #[cfg(unix)]
    {
        match Command::new("ps")
            .args(["-o", "command=", "-p", &std::process::id().to_string()])
            .output()
        {
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!("SKIP daemon IPC integration: sandbox denies ps: {error}");
                return;
            }
            Ok(output) => assert!(output.status.success(), "ps preflight failed: {output:?}"),
            Err(error) => panic!("ps preflight failed: {error}"),
        }
        std::fs::create_dir_all(&root).unwrap();
        let probe = root.join("probe.sock");
        match std::os::unix::net::UnixListener::bind(&probe) {
            Ok(listener) => {
                drop(listener);
                std::fs::remove_file(probe).unwrap();
            }
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!("SKIP daemon IPC integration: sandbox denies socket bind: {error}");
                std::fs::remove_dir_all(root).unwrap();
                return;
            }
            Err(error) => panic!("IPC preflight failed: {error}"),
        }
    }
    let command = |wxid: &str| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_wx"));
        command
            .env("WX_CLI_HOME", &root)
            .env_remove("WX_DAEMON_MODE")
            .env_remove("WX_ACCOUNT")
            .env_remove("WX_PID")
            .args(["--wxid", wxid]);
        command
    };
    let mut pids = Vec::new();
    for wxid in ["wxid_first", "wxid_second"] {
        let profile = root
            .join("accounts")
            .join(format!("{:x}", Sha256::digest(wxid.as_bytes())));
        let db_dir = root.join("data").join(wxid).join("db_storage");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::create_dir_all(db_dir.join("contact")).unwrap();
        let conn = rusqlite::Connection::open(db_dir.join("contact/contact.db")).unwrap();
        conn.execute_batch("CREATE TABLE contact(username TEXT, nick_name TEXT)")
            .unwrap();
        conn.execute("INSERT INTO contact VALUES(?1,'fixture')", [wxid])
            .unwrap();
        drop(conn);
        std::fs::write(
            profile.join("config.json"),
            serde_json::json!({"db_dir":db_dir}).to_string(),
        )
        .unwrap();
        std::fs::write(profile.join("all_keys.json"), "{}").unwrap();
        let child = command(wxid)
            .args(["contacts", "--json"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(child.wait_with_output());
        });
        let result = rx.recv_timeout(Duration::from_secs(20));
        if result.is_err() {
            for account in ["wxid_first", "wxid_second"] {
                let _ = command(account).args(["daemon", "stop"]).output();
            }
            panic!("captured client pipes remained open after daemon startup");
        }
        let output = result.unwrap().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
            serde_json::json!([])
        );
        let pid: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(profile.join("daemon.pid")).unwrap())
                .unwrap();
        pids.push(pid["pid"].as_u64().unwrap());
        assert!(profile.join("cache").is_dir());
    }
    assert_ne!(pids[0], pids[1]);
    for wxid in ["wxid_first", "wxid_second"] {
        let status = command(wxid).args(["daemon", "status"]).output().unwrap();
        assert!(String::from_utf8_lossy(&status.stdout).contains("运行中"));
        let stop = command(wxid).args(["daemon", "stop"]).output().unwrap();
        assert!(
            stop.status.success(),
            "{}",
            String::from_utf8_lossy(&stop.stderr)
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}
