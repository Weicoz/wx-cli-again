//! 图片与受保护密钥的受限目录；不复用调用者给出的文件名。
use anyhow::{ensure, Result};
use std::path::{Path, PathBuf};

pub(super) fn private_dir(path: &Path) -> Result<PathBuf> {
    ensure!(path.is_absolute(), "目录必须为绝对路径");
    let existed = path.exists();
    if existed {
        ensure!(!is_link(path)?, "目录不得为链接");
        ensure!(path.is_dir(), "不是目录");
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let m = std::fs::metadata(path)?;
            ensure!(
                m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o077 == 0,
                "目录必须仅当前用户可访问"
            );
        }
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(path)?;
        }
        #[cfg(windows)]
        std::fs::create_dir_all(path)?;
    }
    #[cfg(windows)]
    restrict_windows(path)?;
    Ok(path.canonicalize()?)
}

pub(super) fn is_link(path: &Path) -> Result<bool> {
    let m = std::fs::symlink_metadata(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        Ok(m.file_attributes() & 0x400 != 0)
    }
    #[cfg(not(windows))]
    {
        Ok(m.file_type().is_symlink())
    }
}

#[cfg(windows)]
fn restrict_windows(path: &Path) -> Result<()> {
    use std::{ffi::c_void, os::windows::ffi::OsStrExt, ptr};
    #[repr(C)]
    struct SidAndAttributes {
        sid: *mut c_void,
        attributes: u32,
    }
    #[link(name = "advapi32")]
    extern "system" {
        fn OpenProcessToken(process: isize, access: u32, token: *mut isize) -> i32;
        fn GetTokenInformation(
            token: isize,
            class: u32,
            info: *mut c_void,
            size: u32,
            needed: *mut u32,
        ) -> i32;
        fn ConvertSidToStringSidW(sid: *mut c_void, text: *mut *mut u16) -> i32;
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text: *const u16,
            revision: u32,
            sd: *mut *mut c_void,
            size: *mut u32,
        ) -> i32;
        fn SetFileSecurityW(path: *const u16, information: u32, sd: *mut c_void) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> isize;
        fn CloseHandle(h: isize) -> i32;
        fn LocalFree(p: *mut c_void) -> *mut c_void;
    }
    unsafe {
        let mut token = 0;
        ensure!(
            OpenProcessToken(GetCurrentProcess(), 8, &mut token) != 0,
            "无法取得当前用户令牌"
        );
        let mut needed = 0;
        GetTokenInformation(token, 1, ptr::null_mut(), 0, &mut needed);
        let mut buf = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
        let ok = GetTokenInformation(token, 1, buf.as_mut_ptr().cast(), needed, &mut needed);
        CloseHandle(token);
        ensure!(ok != 0, "无法取得当前用户 SID");
        let sid = (*(buf.as_ptr().cast::<SidAndAttributes>())).sid;
        let mut sid_text = ptr::null_mut();
        ensure!(
            ConvertSidToStringSidW(sid, &mut sid_text) != 0,
            "无法转换当前用户 SID"
        );
        let mut len = 0;
        while *sid_text.add(len) != 0 {
            len += 1;
        }
        let sid_string = String::from_utf16_lossy(std::slice::from_raw_parts(sid_text, len));
        LocalFree(sid_text.cast());
        let sddl: Vec<u16> = format!("D:P(A;OICI;FA;;;{sid_string})")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut sd = ptr::null_mut();
        ensure!(
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut sd,
                ptr::null_mut()
            ) != 0,
            "无法建立目录 ACL"
        );
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let result = SetFileSecurityW(wide.as_ptr(), 0x80000004, sd);
        LocalFree(sd);
        ensure!(result != 0, "无法限制目录 ACL");
    }
    Ok(())
}
