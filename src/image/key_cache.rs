//! 单次请求最多扫描一次；历史版本仅对当前目标反验，不按 PID 清空。
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

fn validate_layout(dat: &[u8]) -> Result<(), Failure> {
    if !dat.starts_with(&V2_MAGIC) {
        return Err(Failure::FormatUnsupported);
    }
    if dat.len() < 15 {
        return Err(Failure::ResourceIncomplete);
    }
    let size = u32::from_le_bytes(dat[6..10].try_into().unwrap()) as u64;
    let xor = u32::from_le_bytes(dat[10..14].try_into().unwrap()) as u64;
    let needed = 15 + size + (16 - size % 16) + xor;
    if needed > dat.len() as u64 {
        return Err(Failure::ResourceIncomplete);
    }
    Ok(())
}
// AES 首块已识别时，后续损坏属于资源错误，不再把它当 key 失效。
fn open(dat: &[u8], key: &Key) -> Result<Option<Vec<u8>>, Failure> {
    use aes::cipher::{BlockDecrypt, KeyInit};
    let aes = aes::Aes128::new((&key.aes).into());
    let mut first = aes::cipher::generic_array::GenericArray::clone_from_slice(&dat[15..31]);
    aes.decrypt_block(&mut first);
    if decoder::detect_image_format(&first) == "bin" {
        // 对未知格式验证整个 AES 段的 PKCS7，避免已知历史 key 因不支持格式而被重扫。
        let size = u32::from_le_bytes(dat[6..10].try_into().unwrap()) as usize;
        let aligned = size + (16 - size % 16);
        let mut plain = zeroize::Zeroizing::new(dat[15..15 + aligned].to_vec());
        for b in plain.as_chunks_mut::<16>().0.iter_mut() {
            aes.decrypt_block(b.into());
        }
        let pad = *plain.last().unwrap() as usize;
        if pad > 0
            && pad <= 16
            && pad <= plain.len()
            && plain[plain.len() - pad..]
                .iter()
                .all(|&b| b as usize == pad)
            && plain.len() - pad == size
        {
            return Err(Failure::FormatUnsupported);
        }
        return Ok(None);
    }
    decoder::dispatch(
        dat,
        decoder::V2KeyMaterial {
            aes_key: Some(&key.aes),
            xor_key: key.xor,
        },
    )
    .map(|d| Some(d.data))
    .map_err(|_| Failure::DecodeFailed)
}
fn decode(
    dat: &[u8],
    scope: &str,
    vault: &mut impl Vault,
    scan: &mut impl FnMut(&[u8]) -> Result<Key, Failure>,
) -> Result<Vec<u8>, Failure> {
    validate_layout(dat)?;
    let mut keys = vault.load(scope)?;
    for i in 0..keys.len() {
        if let Some(data) = open(dat, &keys[i])? {
            let mut verified = keys.remove(i);
            verified.validated_at = chrono::Utc::now().timestamp();
            keys.insert(0, verified);
            vault.save(scope, &keys)?;
            return Ok(data);
        }
    }
    let mut key = scan(dat)?;
    let data = open(dat, &key)?.ok_or(Failure::ImageKeyUnavailable)?;
    key.validated_at = chrono::Utc::now().timestamp();
    keys.retain(|k| k.aes != key.aes || k.xor != key.xor);
    keys.insert(0, key);
    keys.truncate(4);
    vault.save(scope, &keys)?;
    Ok(data)
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
        })
    }
    pub fn decode(&mut self, dat: &[u8]) -> Result<Vec<u8>, Failure> {
        validate_layout(dat)?;
        #[cfg(windows)]
        {
            let dir = super::secure_fs::private_dir(
                &crate::config::cli_dir()
                    .join("image-keys")
                    .join(&self.scope),
            )
            .map_err(|_| Failure::ImageKeyUnavailable)?;
            let lock_path = dir.join("provider.lock");
            if lock_path.exists()
                && super::secure_fs::is_link(&lock_path)
                    .map_err(|_| Failure::ImageKeyUnavailable)?
            {
                return Err(Failure::ImageKeyUnavailable);
            }
            let lock = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(lock_path)
                .map_err(|_| Failure::ImageKeyUnavailable)?;
            let wait_until = std::time::Instant::now() + std::time::Duration::from_millis(500);
            loop {
                match lock.try_lock() {
                    Ok(()) => break,
                    Err(std::fs::TryLockError::WouldBlock)
                        if std::time::Instant::now() < wait_until =>
                    {
                        std::thread::sleep(std::time::Duration::from_millis(10))
                    }
                    Err(std::fs::TryLockError::WouldBlock) => return Err(Failure::Timeout),
                    Err(_) => return Err(Failure::ImageKeyUnavailable),
                }
            }
            let mut vault = ProtectedVault {
                dir,
                protector: Dpapi,
            };
            decode(dat, &self.scope, &mut vault, &mut |target| {
                if self.scanned {
                    return Err(Failure::ImageKeyUnavailable);
                }
                self.scanned = true;
                let k =
                    crate::attachment::image_key::windows::scan_for_target(&self.db_dir, target)
                        .map_err(|e| {
                            if e.downcast_ref::<crate::cli::account::ReaderError>()
                                .is_some_and(|e| e.code == "TIMEOUT")
                            {
                                Failure::Timeout
                            } else {
                                Failure::ImageKeyUnavailable
                            }
                        })?;
                Ok(Key {
                    aes: k.aes_key,
                    xor: k.xor_key,
                    validated_at: 0,
                })
            })
        }
        #[cfg(not(windows))]
        {
            // 非 Windows 不做不安全的磁盘替代；真实 DPAPI 仅在 Windows 启用。
            let _ = (&self.scope, &self.db_dir, self.scanned);
            Err(Failure::ImageKeyUnavailable)
        }
    }
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
        let mut plain = b"\x89PNG\r\n\x1a\nsynthetic".to_vec();
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
            Err(Failure::ResourceIncomplete)
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
            Err(Failure::DecodeFailed)
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
