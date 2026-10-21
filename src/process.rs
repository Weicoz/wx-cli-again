//! 进程元数据与数据库占用；不读取进程内存。
use anyhow::Result;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct WechatProcess {
    pub pid: u32,
    pub name: String,
    pub command_line: Option<String>,
}

pub(crate) fn select_main_process(
    processes: &[WechatProcess],
    owners: Option<&[u32]>,
    pid: Option<u32>,
) -> Result<u32> {
    if processes.iter().any(|process| {
        process.name.eq_ignore_ascii_case("Weixin.exe")
            && process.command_line.is_none()
            && owners.is_none_or(|owners| owners.contains(&process.pid))
            && pid.is_none_or(|pid| pid == process.pid)
    }) {
        return Err(crate::cli::account::reader_error(
            "INTERNAL",
            "无法读取目标微信进程命令行，无法确认主进程",
        ));
    }
    let candidates: Vec<_> = processes
        .iter()
        .filter(|p| {
            let main = if p.name.eq_ignore_ascii_case("Weixin.exe") {
                p.command_line.as_deref().is_some_and(|cmd| {
                    !cmd.split_whitespace()
                        .map(|part| part.trim_matches('"'))
                        .any(|part| part == "--type" || part.starts_with("--type="))
                })
            } else {
                matches!(p.name.as_str(), "WeChat" | "wechat" | "weixin")
            };
            main && owners.is_none_or(|owners| owners.contains(&p.pid))
                && pid.is_none_or(|pid| pid == p.pid)
        })
        .collect();
    match candidates.as_slice() {
        [process] => Ok(process.pid),
        [] => Err(crate::cli::account::reader_error(
            "LOGIN_REQUIRED",
            "目标账号没有持有数据库的微信主进程，请登录该账号",
        )),
        _ => Err(crate::cli::account::reader_error(
            "INTERNAL",
            "存在多个微信主进程，请用 --wxid 和 --pid 明确选择",
        )),
    }
}

pub(crate) fn find_for_db(db_dir: Option<&std::path::Path>, pid: Option<u32>) -> Result<u32> {
    let processes = list_processes()?;
    let owners = db_dir.map(database_owners).transpose()?;
    select_main_process(&processes, owners.as_deref(), pid)
}

#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn parse_windows_processes(text: &str) -> Result<Vec<WechatProcess>> {
    let text = text.trim_start_matches('\u{feff}').trim();
    if text.is_empty() || text == "null" {
        return Ok(Vec::new());
    }
    if text.starts_with('[') {
        Ok(serde_json::from_str(text)?)
    } else {
        Ok(vec![serde_json::from_str(text)?])
    }
}

#[cfg(windows)]
fn list_processes() -> Result<Vec<WechatProcess>> {
    use anyhow::Context;
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", r#"[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); @(Get-CimInstance Win32_Process -Filter "Name='Weixin.exe'" -ErrorAction Stop | Select-Object @{n='pid';e={$_.ProcessId}}, @{n='name';e={$_.Name}}, @{n='command_line';e={$_.CommandLine}}) | ConvertTo-Json -Compress"#])
        .output().context("无法查询 Windows 微信主进程")?;
    anyhow::ensure!(output.status.success(), "Windows 微信进程查询失败");
    let text = String::from_utf8(output.stdout).context("Windows 进程列表不是 UTF-8")?;
    parse_windows_processes(&text).context("Windows 进程列表格式错误")
}

#[cfg(unix)]
fn list_processes() -> Result<Vec<WechatProcess>> {
    use anyhow::Context;
    let mut processes = Vec::new();
    let names: &[&str] = if cfg!(target_os = "macos") {
        &["WeChat"]
    } else {
        &["wechat", "weixin"]
    };
    for name in names {
        let output = std::process::Command::new("pgrep")
            .args(["-x", name])
            .output()
            .context("无法查询微信进程")?;
        anyhow::ensure!(
            output.status.success() || output.status.code() == Some(1),
            "微信进程查询失败"
        );
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let pid = line.trim().parse().context("微信 PID 格式错误")?;
            processes.push(WechatProcess {
                pid,
                name: (*name).into(),
                command_line: Some(String::new()),
            });
        }
    }
    Ok(processes)
}

fn database_files(db_dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    [
        "contact/contact.db",
        "contact/contact.db-wal",
        "session/session.db",
        "session/session.db-wal",
    ]
    .iter()
    .map(|file| db_dir.join(file))
    .filter(|path| path.is_file())
    .collect()
}

#[cfg(unix)]
fn database_owners(db_dir: &std::path::Path) -> Result<Vec<u32>> {
    use anyhow::Context;
    let files = database_files(db_dir);
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let output = std::process::Command::new("lsof")
        .args(["-t", "--"])
        .args(files)
        .output()
        .context("无法查询微信数据库占用（需要 lsof）")?;
    anyhow::ensure!(
        output.status.success() || output.status.code() == Some(1),
        "数据库占用查询失败"
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| line.trim().parse().context("数据库占用 PID 格式错误"))
        .collect()
}

#[cfg(windows)]
fn database_owners(db_dir: &std::path::Path) -> Result<Vec<u32>> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows::Win32::System::RestartManager::*;
    let files = database_files(db_dir);
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let files: Vec<Vec<u16>> = files
        .iter()
        .map(|path| path.as_os_str().encode_wide().chain(Some(0)).collect())
        .collect();
    let names: Vec<PCWSTR> = files.iter().map(|name| PCWSTR(name.as_ptr())).collect();
    let mut handle = 0;
    let mut session_key = [0u16; 33];
    // 仅注册资源并查询占用进程；不得调用 RmShutdown/RmRestart。
    unsafe {
        RmStartSession(&mut handle, 0, PWSTR(session_key.as_mut_ptr())).ok()?;
    }
    struct Session(u32);
    impl Drop for Session {
        fn drop(&mut self) {
            unsafe {
                let _ = RmEndSession(self.0);
            }
        }
    }
    let _session = Session(handle);
    unsafe {
        RmRegisterResources(handle, Some(&names), None, None).ok()?;
    }
    let mut count = 0;
    let mut needed = 0;
    let mut reasons = 0;
    let status = unsafe { RmGetList(handle, &mut needed, &mut count, None, &mut reasons) };
    if status == ERROR_SUCCESS {
        return Ok(Vec::new());
    }
    status.ok().or_else(|err| {
        if status == ERROR_MORE_DATA {
            Ok(())
        } else {
            Err(err)
        }
    })?;
    // 占用表在两次查询间可增长；有限重取，不能把失败当成未登录。
    for _ in 0..3 {
        let mut entries = vec![RM_PROCESS_INFO::default(); needed as usize];
        count = entries.len() as u32;
        let status = unsafe {
            RmGetList(
                handle,
                &mut needed,
                &mut count,
                Some(entries.as_mut_ptr()),
                &mut reasons,
            )
        };
        if status == ERROR_MORE_DATA {
            continue;
        }
        status.ok()?;
        return Ok(entries[..count as usize]
            .iter()
            .map(|entry| entry.Process.dwProcessId)
            .collect());
    }
    anyhow::bail!("微信数据库占用变化过快，无法确认身份")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn process(pid: u32, command: Option<&str>) -> WechatProcess {
        WechatProcess {
            pid,
            name: "Weixin.exe".into(),
            command_line: command.map(str::to_owned),
        }
    }
    #[test]
    fn account_selection_ignores_helpers_and_snapshot_order() {
        let processes = vec![
            process(10, Some("Weixin.exe --type=utility")),
            process(11, Some("Weixin.exe --scene=taskbarpins")),
            process(12, Some("Weixin.exe")),
        ];
        assert_eq!(
            select_main_process(&processes, Some(&[10, 12]), None).unwrap(),
            12
        );
        assert!(select_main_process(&processes, Some(&[11]), Some(12)).is_err());
    }
    #[test]
    fn unknown_command_lines_helpers_and_ambiguous_mains_fail_closed() {
        for command in [
            None,
            Some("Weixin.exe --type=wxocr"),
            Some("Weixin.exe \"--type=utility\""),
            Some("Weixin.exe --type renderer"),
        ] {
            assert!(select_main_process(&[process(1, command)], Some(&[1]), None).is_err());
        }
        assert!(select_main_process(
            &[
                process(1, Some("Weixin.exe")),
                process(2, Some("Weixin.exe --scene=taskbarpins"))
            ],
            None,
            None
        )
        .is_err());
        assert_eq!(
            select_main_process(
                &[
                    process(1, Some("Weixin.exe")),
                    process(2, Some("Weixin.exe"))
                ],
                None,
                Some(2)
            )
            .unwrap(),
            2
        );
    }

    #[test]
    fn windows_process_json_accepts_zero_one_and_many_processes() {
        assert!(parse_windows_processes("").unwrap().is_empty());
        assert!(parse_windows_processes("[]").unwrap().is_empty());
        let single = r#"{"pid":42,"name":"Weixin.exe","command_line":"C:\\微信\\Weixin.exe --scene=taskbarpins"}"#;
        assert_eq!(parse_windows_processes(single).unwrap()[0].pid, 42);
        let array = format!("\u{feff}[{single}]");
        assert_eq!(parse_windows_processes(&array).unwrap()[0].pid, 42);
    }
    #[test]
    fn unreadable_process_metadata_is_internal_error_not_logged_out() {
        let error = select_main_process(&[process(7, None)], Some(&[7]), Some(7)).unwrap_err();
        assert_eq!(
            crate::cli::account::error_json(&error)["error"]["code"],
            "INTERNAL"
        );
    }
}
