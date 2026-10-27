//! DPAPI 缓存与轮询：完整解码成功后才持久化新 key；每轮持有同一账号锁。
#![cfg_attr(not(any(windows, test)), allow(dead_code))]
use super::Failure;
use crate::attachment::decoder::{self, V2_MAGIC};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use zeroize::Zeroize;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct Key {
    aes: [u8; 16],
    xor: u8,
    validated_at: i64,
}
impl Drop for Key {
    fn drop(&mut self) {
        self.aes.zeroize();
        self.xor.zeroize();
    }
}
trait Vault {
    fn load(&mut self, scope: &str) -> Result<Vec<Key>, Failure>;
    fn save(&mut self, scope: &str, keys: &[Key]) -> Result<(), Failure>;
}

pub(super) fn validate_target(dat: &[u8]) -> Result<(), Failure> {
    validate_layout(dat)
}
fn validate_layout(dat: &[u8]) -> Result<(), Failure> {
    if !dat.starts_with(&V2_MAGIC) {
        return Err(Failure::FormatUnsupported);
    }
    if dat.len() < 31 {
        return Err(Failure::Incomplete(Stage::AesHead));
    }
    let size = u32::from_le_bytes(dat[6..10].try_into().unwrap()) as u64;
    let xor = u32::from_le_bytes(dat[10..14].try_into().unwrap()) as u64;
    let aes_end = 15 + size + (16 - size % 16);
    if aes_end > dat.len() as u64 {
        return Err(Failure::Incomplete(Stage::AesBody));
    }
    if aes_end + xor > dat.len() as u64 {
        return Err(Failure::Incomplete(Stage::XorTail));
    }
    Ok(())
}
#[derive(serde::Serialize, serde::Deserialize)]
struct Record {
    version: u32,
    scope: String,
    keys: Vec<Key>,
}
trait Protector {
    fn protect(&self, data: &[u8], scope: &str) -> Result<Vec<u8>, Failure>;
    fn unprotect(&self, data: &[u8], scope: &str) -> Result<Vec<u8>, Failure>;
}
struct ProtectedVault<P> {
    dir: PathBuf,
    protector: P,
}
impl<P: Protector> Vault for ProtectedVault<P> {
    fn load(&mut self, scope: &str) -> Result<Vec<Key>, Failure> {
        let path = self.dir.join("image-keys.dpapi");
        if !path.exists() {
            return Ok(Vec::new());
        }
        if super::secure_fs::is_link(&path).map_err(|_| Failure::ImageKeyUnavailable)? {
            return Err(Failure::ImageKeyUnavailable);
        }
        let data = std::fs::read(path).map_err(|_| Failure::ImageKeyUnavailable)?;
        if data.len() > 16384 {
            return Err(Failure::ImageKeyUnavailable);
        }
        let plain = zeroize::Zeroizing::new(self.protector.unprotect(&data, scope)?);
        let record: Record =
            serde_json::from_slice(&plain).map_err(|_| Failure::ImageKeyUnavailable)?;
        if record.version != 1 || record.keys.len() > 4 {
            return Err(Failure::ImageKeyUnavailable);
        }
        if record.scope != scope {
            return Err(Failure::AccountMismatch);
        }
        Ok(record.keys)
    }
    fn save(&mut self, scope: &str, keys: &[Key]) -> Result<(), Failure> {
        use std::io::Write;
        let plain = zeroize::Zeroizing::new(
            serde_json::to_vec(&Record {
                version: 1,
                scope: scope.into(),
                keys: keys.to_vec(),
            })
            .map_err(|_| Failure::ImageKeyUnavailable)?,
        );
        let protected = self.protector.protect(&plain, scope)?;
        let mut tmp =
            tempfile::NamedTempFile::new_in(&self.dir).map_err(|_| Failure::ImageKeyUnavailable)?;
        tmp.write_all(&protected)
            .and_then(|_| tmp.as_file().sync_all())
            .map_err(|_| Failure::ImageKeyUnavailable)?;
        tmp.persist(self.dir.join("image-keys.dpapi"))
            .map_err(|_| Failure::ImageKeyUnavailable)?;
        Ok(())
    }
}

pub(super) struct Provider {
    scope: String,
    db_dir: PathBuf,
    scanned: bool,
    wait_deadline: Option<Instant>,
}
impl Provider {
    pub fn new(account: &str, db_dir: &Path) -> anyhow::Result<Self> {
        let db_dir = db_dir.canonicalize()?;
        let scope = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(account, &db_dir))?)
        );
        Ok(Self {
            scope,
            db_dir,
            scanned: false,
            wait_deadline: None,
        })
    }
    pub fn decode_wait(&mut self, dat: &[u8], wait_seconds: u64) -> Result<Vec<u8>, Failure> {
        let mut report = super::harvest::Report::default();
        let remaining = if wait_seconds > 0 {
            let deadline = match self.wait_deadline {
                Some(deadline) => deadline,
                None => {
                    let deadline = Instant::now()
                        .checked_add(Duration::from_secs(wait_seconds))
                        .ok_or(Failure::Timeout)?;
                    self.wait_deadline = Some(deadline);
                    deadline
                }
            };
            deadline.saturating_duration_since(Instant::now())
        } else {
            Duration::ZERO
        };
        self.poll_for(dat, remaining, 5000, false, &mut report)
    }
    pub fn poll(
        &mut self,
        dat: &[u8],
        seconds: u64,
        interval_ms: u64,
        harvest: bool,
        report: &mut super::harvest::Report,
    ) -> Result<Vec<u8>, Failure> {
        self.poll_for(
            dat,
            Duration::from_secs(seconds),
            interval_ms,
            harvest,
            report,
        )
    }
    fn poll_for(
        &mut self,
        dat: &[u8],
        wait: Duration,
        interval_ms: u64,
        harvest: bool,
        report: &mut super::harvest::Report,
    ) -> Result<Vec<u8>, Failure> {
        validate_layout(dat)?;
        let budget = if harvest || !wait.is_zero() { 30 } else { 3 };
        let allow_scan = (harvest || !self.scanned || !wait.is_zero())
            && (harvest || self.wait_deadline.is_none_or(|d| Instant::now() < d));
        let mut scanner = super::harvest::NativeScanner {
            db_dir: &self.db_dir,
            scanned: false,
        };
        let result = poll(
            &mut super::harvest::SystemClock,
            wait,
            interval_ms,
            budget,
            harvest,
            report,
            &mut |deadline, candidates| {
                #[cfg(windows)]
                {
                    let dir = super::secure_fs::private_dir(
                        &crate::config::cli_dir()
                            .join("image-keys")
                            .join(&self.scope),
                    )
                    .map_err(|_| Failure::ImageKeyUnavailable)?;
                    let _lock = provider_lock(&dir, deadline)?;
                    let mut vault = ProtectedVault {
                        dir,
                        protector: Dpapi,
                    };
                    attempt(
                        dat,
                        &self.scope,
                        &mut vault,
                        &mut scanner,
                        deadline,
                        candidates,
                        allow_scan,
                    )
                }
                #[cfg(not(windows))]
                {
                    let _ = (&self.scope, &mut scanner, deadline, candidates, allow_scan);
                    scanner.scanned = true;
                    Err(Failure::ImageKeyUnavailable)
                }
            },
        );
        self.scanned |= scanner.scanned;
        result
    }
}

/// 同账号的锁在缓存读取、整个扫描和缓存原子替换期间一直持有。
fn provider_lock(dir: &Path, deadline: std::time::Instant) -> Result<std::fs::File, Failure> {
    let path = dir.join("provider.lock");
    if path.exists()
        && super::secure_fs::is_link(&path).map_err(|_| Failure::ImageKeyUnavailable)?
    {
        return Err(Failure::ImageKeyUnavailable);
    }
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|_| Failure::ImageKeyUnavailable)?;
    let until = deadline.min(std::time::Instant::now() + std::time::Duration::from_millis(500));
    loop {
        match lock.try_lock() {
            Ok(()) => return Ok(lock),
            Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < until => {
                std::thread::sleep(std::time::Duration::from_millis(10))
            }
            Err(std::fs::TryLockError::WouldBlock) => return Err(Failure::Timeout),
            Err(_) => return Err(Failure::ImageKeyUnavailable),
        }
    }
}

use super::{
    harvest::{Candidates, Clock, Report, Scanner},
    Stage,
};
use std::time::{Duration, Instant};

#[allow(clippy::too_many_arguments)]
fn poll(
    clock: &mut impl Clock,
    wait: Duration,
    interval_ms: u64,
    budget: u64,
    harvest: bool,
    report: &mut Report,
    attempt: &mut impl FnMut(Instant, &mut Candidates) -> Result<(Vec<u8>, bool), Failure>,
) -> Result<Vec<u8>, Failure> {
    let start = clock.now();
    let end = start.checked_add(wait).ok_or(Failure::Timeout)?;
    let mut candidates = Candidates::default();
    let result = loop {
        if report.attempts > 0 && clock.now() >= end {
            break Err(if harvest {
                Failure::HarvestTimeout
            } else if let Some(stage) = report.stage {
                Failure::Stage(stage)
            } else {
                Failure::ImageKeyUnavailable
            });
        }
        report.attempts += 1;
        let deadline = clock
            .now()
            .checked_add(Duration::from_secs(budget))
            .ok_or(Failure::Timeout)?;
        let deadline = if wait.is_zero() && !harvest {
            deadline
        } else {
            deadline.min(end)
        };
        match attempt(deadline, &mut candidates) {
            Ok((data, written)) => {
                report.cache_written = written;
                break Ok(data);
            }
            Err(Failure::Stage(stage)) => {
                report.stage = Some(stage);
            }
            Err(Failure::Timeout) if wait.is_zero() && !harvest => break Err(Failure::Timeout),
            Err(Failure::ImageKeyUnavailable | Failure::Timeout) => {}
            Err(e) => break Err(e),
        }
        if wait.is_zero() || clock.now() >= end {
            break Err(if harvest {
                Failure::HarvestTimeout
            } else if let Some(stage) = report.stage {
                Failure::Stage(stage)
            } else {
                Failure::ImageKeyUnavailable
            });
        }
        clock.sleep(
            Duration::from_millis(interval_ms.max(1))
                .min(end.saturating_duration_since(clock.now())),
        );
    };
    report.elapsed_ms = clock
        .now()
        .duration_since(start)
        .as_millis()
        .min(u64::MAX as u128) as u64;
    report.candidate_counts = candidates.counts;
    match result {
        Ok(_) => report.status = "ready".into(),
        Err(e) => report.failed(e),
    }
    if result.is_ok() {
        report.stage = None;
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn attempt(
    dat: &[u8],
    scope: &str,
    vault: &mut impl Vault,
    scanner: &mut impl Scanner,
    deadline: Instant,
    candidates: &mut Candidates,
    allow_scan: bool,
) -> Result<(Vec<u8>, bool), Failure> {
    validate_layout(dat)?;
    let mut keys = vault.load(scope)?;
    for key in &keys {
        if let Some(data) = open_complete(dat, key, deadline)? {
            return Ok((data, false));
        }
    }
    if !allow_scan {
        return Err(Failure::ImageKeyUnavailable);
    }
    let mut success = None;
    let mut last = Failure::ImageKeyUnavailable;
    let mut heads = Vec::new();
    let scan_result = scanner.scan(dat, deadline, candidates, &mut |mut aes| {
        use aes::cipher::{BlockDecrypt, KeyInit};
        let cipher = aes::Aes128::new((&aes).into());
        let mut first = aes::cipher::generic_array::GenericArray::clone_from_slice(&dat[15..31]);
        cipher.decrypt_block(&mut first);
        let format = decoder::detect_image_format(&first);
        if format == "bin" {
            aes.zeroize();
            return false;
        }
        heads.push(sha2::Sha256::digest(aes).into());
        let xor = match infer_xor(dat, format) {
            Ok(xor) => xor,
            Err(e) => {
                last = e;
                aes.zeroize();
                return false;
            }
        };
        let key = Key {
            aes,
            xor,
            validated_at: chrono::Utc::now().timestamp(),
        };
        aes.zeroize();
        match open_complete(dat, &key, deadline) {
            Ok(Some(data)) => {
                success = Some((key, data));
                true
            }
            Ok(None) => false,
            Err(e) => {
                last = e;
                false
            }
        }
    });
    for hash in heads {
        candidates.head_hash(hash);
    }
    if let Some((key, data)) = success {
        keys.retain(|k| k.aes != key.aes || k.xor != key.xor);
        keys.insert(0, key);
        keys.truncate(4);
        vault.save(scope, &keys)?;
        return Ok((data, true));
    }
    // 即使扫描预算耗尽，也保留已经确定的完整解码阶段。
    if last != Failure::ImageKeyUnavailable {
        return Err(last);
    }
    scan_result?;
    Err(Failure::ImageKeyUnavailable)
}

fn infer_xor(dat: &[u8], format: &str) -> Result<u8, Failure> {
    let footer: &[u8] = match format {
        "jpg" => &[0xff, 0xd9],
        "png" => &[0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82],
        "gif" => &[0x3b],
        _ => &[],
    };
    let count = (u32::from_le_bytes(dat[10..14].try_into().unwrap()) as usize).min(footer.len());
    if count == 0 {
        return Ok(0x88);
    }
    let xor = dat[dat.len() - 1] ^ footer[footer.len() - 1];
    if !dat[dat.len() - count..]
        .iter()
        .zip(&footer[footer.len() - count..])
        .all(|(a, b)| a ^ b == xor)
    {
        return Err(Failure::Stage(Stage::XorTail));
    }
    Ok(xor)
}

fn open_complete(dat: &[u8], key: &Key, deadline: Instant) -> Result<Option<Vec<u8>>, Failure> {
    let Some(data) = open_staged(dat, key)? else {
        return Ok(None);
    };
    // 标准图片的完整解码与 WXGF 转码均须成功，才允许把新 key 写进缓存。
    super::codec::standard(&data, deadline)?;
    Ok(Some(data))
}
fn open_staged(dat: &[u8], key: &Key) -> Result<Option<Vec<u8>>, Failure> {
    use aes::cipher::{BlockDecrypt, KeyInit};
    validate_layout(dat)?;
    let cipher = aes::Aes128::new((&key.aes).into());
    let size = u32::from_le_bytes(dat[6..10].try_into().unwrap()) as usize;
    let aligned = size + 16 - size % 16;
    let mut plain = zeroize::Zeroizing::new(dat[15..15 + aligned].to_vec());
    for block in plain.as_chunks_mut::<16>().0 {
        cipher.decrypt_block(block.into());
    }
    let recognized = decoder::detect_image_format(&plain[..16]) != "bin";
    let pad = *plain.last().unwrap() as usize;
    if pad == 0
        || pad > 16
        || !plain[plain.len() - pad..]
            .iter()
            .all(|b| *b as usize == pad)
    {
        return if recognized {
            Err(Failure::Stage(Stage::Pkcs7))
        } else {
            Ok(None)
        };
    }
    if plain.len() - pad != size {
        return Err(Failure::Stage(Stage::AesBody));
    }
    if !recognized {
        return Err(Failure::Stage(Stage::FormatDetect));
    }
    plain.truncate(size);
    let xor_size = u32::from_le_bytes(dat[10..14].try_into().unwrap()) as usize;
    let raw_end = dat.len() - xor_size;
    plain.extend_from_slice(&dat[15 + aligned..raw_end]);
    plain.extend(dat[raw_end..].iter().map(|b| b ^ key.xor));
    Ok(Some(plain.to_vec()))
}

#[cfg(windows)]
struct Dpapi;
#[cfg(windows)]
impl Protector for Dpapi {
    fn protect(&self, data: &[u8], scope: &str) -> Result<Vec<u8>, Failure> {
        dpapi(data, scope, true)
    }
    fn unprotect(&self, data: &[u8], scope: &str) -> Result<Vec<u8>, Failure> {
        dpapi(data, scope, false)
    }
}
#[cfg(windows)]
fn dpapi(data: &[u8], scope: &str, protect: bool) -> Result<Vec<u8>, Failure> {
    use std::{ffi::c_void, ptr};
    #[repr(C)]
    struct Blob {
        len: u32,
        data: *mut u8,
    }
    #[link(name = "crypt32")]
    extern "system" {
        fn CryptProtectData(
            input: *const Blob,
            description: *const u16,
            entropy: *const Blob,
            reserved: *mut c_void,
            prompt: *mut c_void,
            flags: u32,
            output: *mut Blob,
        ) -> i32;
        fn CryptUnprotectData(
            input: *const Blob,
            description: *mut *mut u16,
            entropy: *const Blob,
            reserved: *mut c_void,
            prompt: *mut c_void,
            flags: u32,
            output: *mut Blob,
        ) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn LocalFree(p: *mut c_void) -> *mut c_void;
    }
    let input = Blob {
        len: data
            .len()
            .try_into()
            .map_err(|_| Failure::ImageKeyUnavailable)?,
        data: data.as_ptr().cast_mut(),
    };
    let entropy = Blob {
        len: scope.len() as u32,
        data: scope.as_ptr().cast_mut(),
    };
    let mut output = Blob {
        len: 0,
        data: ptr::null_mut(),
    };
    unsafe {
        // CRYPTPROTECT_UI_FORBIDDEN=1；不设 LOCAL_MACHINE，限定 CurrentUser。
        let ok = if protect {
            CryptProtectData(
                &input,
                ptr::null(),
                &entropy,
                ptr::null_mut(),
                ptr::null_mut(),
                1,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                ptr::null_mut(),
                &entropy,
                ptr::null_mut(),
                ptr::null_mut(),
                1,
                &mut output,
            )
        };
        if ok == 0 {
            return Err(Failure::ImageKeyUnavailable);
        }
        let result = std::slice::from_raw_parts(output.data, output.len as usize).to_vec();
        std::slice::from_raw_parts_mut(output.data, output.len as usize).zeroize();
        LocalFree(output.data.cast());
        Ok(result)
    }
}
#[cfg(test)]
fn decode(
    dat: &[u8],
    scope: &str,
    vault: &mut impl Vault,
    scan: &mut impl FnMut(&[u8]) -> Result<Key, Failure>,
) -> Result<Vec<u8>, Failure> {
    struct Adapter<'a, F>(&'a mut F);
    impl<F: FnMut(&[u8]) -> Result<Key, Failure>> Scanner for Adapter<'_, F> {
        fn scan(
            &mut self,
            dat: &[u8],
            _: Instant,
            _: &mut Candidates,
            accept: &mut dyn FnMut([u8; 16]) -> bool,
        ) -> Result<(), Failure> {
            let key = (self.0)(dat)?;
            accept(key.aes);
            Ok(())
        }
    }
    attempt(
        dat,
        scope,
        vault,
        &mut Adapter(scan),
        Instant::now() + Duration::from_secs(3),
        &mut Candidates::default(),
        true,
    )
    .map(|(data, _)| data)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::attachment::decoder::V2_MAGIC;
    use std::cell::Cell;

    fn material(seed: u8) -> Key {
        let digest = sha2::Sha256::digest([seed]);
        Key {
            aes: digest[..16].try_into().unwrap(),
            xor: 0x88,
            validated_at: 0,
        }
    }
    fn encrypted(key: &Key) -> Vec<u8> {
        use aes::cipher::{BlockEncrypt, KeyInit};
        let mut image = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(2, 2)
            .write_to(&mut image, image::ImageFormat::Png)
            .unwrap();
        let mut plain = image.into_inner();
        let size = plain.len();
        let pad = 16 - size % 16;
        plain.resize(size + pad, pad as u8);
        let aes = aes::Aes128::new((&key.aes).into());
        for b in plain.as_chunks_mut::<16>().0.iter_mut() {
            aes.encrypt_block(b.into());
        }
        let mut dat = V2_MAGIC.to_vec();
        dat.extend_from_slice(&(size as u32).to_le_bytes());
        dat.extend_from_slice(&0u32.to_le_bytes());
        dat.push(0);
        dat.extend(plain);
        dat
    }
    #[derive(Default)]
    struct MemoryVault {
        keys: Vec<Key>,
        scope: String,
        denied: bool,
    }
    impl Vault for MemoryVault {
        fn load(&mut self, scope: &str) -> Result<Vec<Key>, Failure> {
            if self.denied {
                return Err(Failure::ImageKeyUnavailable);
            }
            if !self.scope.is_empty() && self.scope != scope {
                return Err(Failure::AccountMismatch);
            }
            Ok(self.keys.clone())
        }
        fn save(&mut self, scope: &str, keys: &[Key]) -> Result<(), Failure> {
            self.scope = scope.into();
            self.keys = keys.to_vec();
            Ok(())
        }
    }
    #[test]
    fn target_revalidation_rescans_once_and_retains_history() {
        let old = material(1);
        let new = material(2);
        let dat = encrypted(&new);
        let mut v = MemoryVault {
            keys: vec![old],
            ..Default::default()
        };
        let scans = Cell::new(0);
        decode(&dat, "account+directory", &mut v, &mut |_| {
            scans.set(scans.get() + 1);
            Ok(new.clone())
        })
        .unwrap();
        assert_eq!(scans.get(), 1);
        assert_eq!(v.keys.len(), 2);
        decode(&dat, "account+directory", &mut v, &mut |_| {
            panic!("cached target should verify")
        })
        .unwrap();
        let historical = encrypted(&material(1));
        decode(&historical, "account+directory", &mut v, &mut |_| {
            panic!("historical key should verify")
        })
        .unwrap();
    }
    #[test]
    fn truncated_or_unknown_container_never_scans() {
        let mut v = MemoryVault::default();
        let mut dat = V2_MAGIC.to_vec();
        dat.extend_from_slice(&[0; 8]);
        assert!(matches!(
            decode(&dat, "a", &mut v, &mut |_| panic!("truncated")),
            Err(Failure::Incomplete(Stage::AesHead))
        ));
        assert!(matches!(
            decode(b"unknown", "a", &mut v, &mut |_| panic!("unknown")),
            Err(Failure::FormatUnsupported)
        ));
    }
    #[test]
    fn wrong_scope_or_dpapi_identity_is_refused() {
        let dat = encrypted(&material(3));
        let mut v = MemoryVault {
            scope: "a".into(),
            ..Default::default()
        };
        assert!(matches!(
            decode(&dat, "b", &mut v, &mut |_| panic!("wrong account")),
            Err(Failure::AccountMismatch)
        ));
        v.denied = true;
        assert!(matches!(
            decode(&dat, "a", &mut v, &mut |_| panic!("wrong DPAPI user")),
            Err(Failure::ImageKeyUnavailable)
        ));
    }
    #[test]
    fn unsuccessful_scan_is_bounded_and_does_not_replace_history() {
        let mut v = MemoryVault {
            keys: vec![material(1)],
            ..Default::default()
        };
        let n = Cell::new(0);
        assert!(matches!(
            decode(&encrypted(&material(2)), "a", &mut v, &mut |_| {
                n.set(n.get() + 1);
                Ok(material(3))
            }),
            Err(Failure::ImageKeyUnavailable)
        ));
        assert_eq!(n.get(), 1);
        assert_eq!(v.keys.len(), 1);
    }
    struct FakeUser(u8);
    impl Protector for FakeUser {
        fn protect(&self, data: &[u8], _scope: &str) -> Result<Vec<u8>, Failure> {
            Ok(std::iter::once(self.0)
                .chain(data.iter().map(|b| b ^ 0xa5))
                .collect())
        }
        fn unprotect(&self, data: &[u8], _scope: &str) -> Result<Vec<u8>, Failure> {
            if data.first() != Some(&self.0) {
                return Err(Failure::ImageKeyUnavailable);
            }
            Ok(data[1..].iter().map(|b| b ^ 0xa5).collect())
        }
    }
    #[test]
    fn protected_records_survive_provider_restart_but_not_other_user_or_scope() {
        let temp = tempfile::tempdir().unwrap();
        let mut vault = ProtectedVault {
            dir: temp.path().into(),
            protector: FakeUser(1),
        };
        let key = material(4);
        let dat = encrypted(&key);
        decode(&dat, "a+directory", &mut vault, &mut |_| Ok(key.clone())).unwrap();
        let disk = std::fs::read(temp.path().join("image-keys.dpapi")).unwrap();
        assert!(serde_json::from_slice::<serde_json::Value>(&disk).is_err());
        let mut reopened = ProtectedVault {
            dir: temp.path().into(),
            protector: FakeUser(1),
        };
        decode(&dat, "a+directory", &mut reopened, &mut |_| {
            panic!("reused after restart")
        })
        .unwrap();
        assert!(matches!(
            reopened.load("b+directory"),
            Err(Failure::AccountMismatch)
        ));
        let mut other = ProtectedVault {
            dir: temp.path().into(),
            protector: FakeUser(2),
        };
        assert!(matches!(
            other.load("a+directory"),
            Err(Failure::ImageKeyUnavailable)
        ));
    }
    #[test]
    fn corrupt_body_with_valid_target_header_does_not_trigger_rescan() {
        let key = material(5);
        let mut dat = encrypted(&key);
        let n = dat.len() - 1;
        dat[n] ^= 0x80;
        let mut vault = MemoryVault {
            keys: vec![key],
            ..Default::default()
        };
        assert!(matches!(
            decode(&dat, "a", &mut vault, &mut |_| panic!(
                "body corruption is not key invalidation"
            )),
            Err(Failure::Stage(Stage::Pkcs7))
        ));
    }
    #[test]
    fn history_is_bounded_to_four_versions() {
        let mut vault = MemoryVault::default();
        for n in 1..=8 {
            let key = material(n);
            decode(&encrypted(&key), "a", &mut vault, &mut |_| Ok(key.clone())).unwrap();
        }
        assert_eq!(vault.keys.len(), 4);
    }

    #[cfg(windows)]
    #[test]
    fn native_dpapi_current_user_and_entropy_scope_roundtrip() {
        let data = zeroize::Zeroizing::new(
            serde_json::to_vec(&Record {
                version: 1,
                scope: "synthetic-a".into(),
                keys: vec![material(9)],
            })
            .unwrap(),
        );
        let protected = Dpapi.protect(&data, "synthetic-a").unwrap();
        let recovered =
            zeroize::Zeroizing::new(Dpapi.unprotect(&protected, "synthetic-a").unwrap());
        assert!(data.as_slice() == recovered.as_slice());
        assert!(Dpapi.unprotect(&protected, "synthetic-b").is_err());
    }
}

#[cfg(test)]
#[path = "harvest_tests.rs"]
mod harvest_tests;
