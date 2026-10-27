//! 全部使用合成内存与运行时图片，不读取微信、不包含真实身份或密钥。
use super::*;
use crate::image::harvest::{Candidates, Clock, Report, Scanner};
use std::{
    collections::HashSet,
    io::Cursor,
    time::{Duration, Instant},
};

fn key(seed: u32) -> Key {
    let digest = Sha256::digest(seed.to_le_bytes());
    Key {
        aes: std::array::from_fn(|i| b'A' + digest[i] % 26),
        xor: 0x88,
        validated_at: 0,
    }
}
fn png() -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(2, 2)
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}
fn encrypt(data: &[u8], key: &Key, size: usize, xor_size: usize) -> Vec<u8> {
    use aes::cipher::{BlockEncrypt, KeyInit};
    let mut plain = data[..size].to_vec();
    let pad = 16 - size % 16;
    plain.resize(size + pad, pad as u8);
    let aes = aes::Aes128::new((&key.aes).into());
    for b in plain.as_chunks_mut::<16>().0 {
        aes.encrypt_block(b.into());
    }
    let mut dat = V2_MAGIC.to_vec();
    dat.extend_from_slice(&(size as u32).to_le_bytes());
    dat.extend_from_slice(&(xor_size as u32).to_le_bytes());
    dat.push(0);
    dat.extend(plain);
    dat.extend_from_slice(&data[size..data.len() - xor_size]);
    dat.extend(data[data.len() - xor_size..].iter().map(|b| b ^ key.xor));
    dat
}
#[derive(Default)]
struct VaultStub {
    keys: Vec<Key>,
    writes: usize,
}
impl Vault for VaultStub {
    fn load(&mut self, _: &str) -> Result<Vec<Key>, Failure> {
        Ok(self.keys.clone())
    }
    fn save(&mut self, _: &str, keys: &[Key]) -> Result<(), Failure> {
        self.writes += 1;
        self.keys = keys.to_vec();
        Ok(())
    }
}
struct FakeClock(Instant);
impl Clock for FakeClock {
    fn now(&self) -> Instant {
        self.0
    }
    fn sleep(&mut self, d: Duration) {
        self.0 += d;
    }
}
struct Buffers {
    calls: usize,
    hit_at: usize,
    buffers: Vec<Vec<u8>>,
}
impl Scanner for Buffers {
    fn scan(
        &mut self,
        _: &[u8],
        _: Instant,
        candidates: &mut Candidates,
        accept: &mut dyn FnMut([u8; 16]) -> bool,
    ) -> Result<(), Failure> {
        self.calls += 1;
        if self.calls >= self.hit_at {
            let mut tried = HashSet::new();
            for buf in &self.buffers {
                if candidates.buffer(buf, &mut tried, accept) {
                    break;
                }
            }
        }
        Ok(())
    }
}
fn run(
    k: usize,
    seconds: u64,
    vault: &mut VaultStub,
    scanner: &mut Buffers,
    dat: &[u8],
) -> (Result<Vec<u8>, Failure>, Report) {
    let mut report = Report::default();
    scanner.hit_at = k;
    let result = poll(
        &mut FakeClock(Instant::now()),
        Duration::from_secs(seconds),
        1000,
        30,
        true,
        &mut report,
        &mut |deadline, counts| {
            attempt(
                dat,
                "synthetic-scope",
                vault,
                scanner,
                deadline,
                counts,
                true,
            )
        },
    );
    (result, report)
}
#[test]
fn kth_hit_fully_decodes_and_writes_only_once_without_secret_output() {
    let key = key(101);
    let dat = encrypt(&png(), &key, 16, 8);
    let mut vault = VaultStub::default();
    let mut scanner = Buffers {
        calls: 0,
        hit_at: 3,
        buffers: vec![key.aes.repeat(2)],
    };
    let (result, report) = run(3, 10, &mut vault, &mut scanner, &dat);
    assert!(result.is_ok());
    assert_eq!(report.attempts, 3);
    assert_eq!(report.elapsed_ms, 2000);
    assert!(report.cache_written);
    assert_eq!(vault.writes, 1);
    let value = serde_json::to_value(&report).unwrap();
    let names: HashSet<_> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        names,
        HashSet::from([
            "status",
            "attempts",
            "elapsedMs",
            "candidateCounts",
            "cacheWritten"
        ])
    );
    assert!(value["candidateCounts"]
        .as_object()
        .unwrap()
        .values()
        .all(serde_json::Value::is_u64));
    let encoded = serde_json::to_vec(&report).unwrap();
    assert!(!encoded.windows(16).any(|b| b == key.aes));
    let (result, report) = run(99, 10, &mut vault, &mut scanner, &dat);
    assert!(result.is_ok());
    assert!(!report.cache_written);
    assert_eq!(vault.writes, 1);
    assert_eq!(scanner.calls, 3);
}
#[test]
fn timeout_has_exact_fake_elapsed_and_no_cache_write() {
    let dat = encrypt(&png(), &key(102), 16, 0);
    let mut vault = VaultStub::default();
    let mut scanner = Buffers {
        calls: 0,
        hit_at: 99,
        buffers: vec![],
    };
    let (result, report) = run(99, 3, &mut vault, &mut scanner, &dat);
    assert_eq!(result.err(), Some(Failure::HarvestTimeout));
    assert!(Failure::HarvestTimeout.retryable());
    let value = serde_json::to_value(&report).unwrap();
    assert_eq!(value["errorCode"], "IMAGE_KEY_HARVEST_TIMEOUT");
    assert_eq!(value["retryable"], true);
    assert_eq!(report.elapsed_ms, 3000);
    assert_eq!(report.attempts, 3);
    assert_eq!(vault.writes, 0);
}
#[test]
fn lock_contention_is_bounded_and_poll_retries_without_scanning() {
    let dir = tempfile::tempdir().unwrap();
    let first = provider_lock(dir.path(), Instant::now() + Duration::from_secs(1)).unwrap();
    let start = Instant::now();
    assert!(matches!(
        provider_lock(dir.path(), start + Duration::from_millis(20)),
        Err(Failure::Timeout)
    ));
    assert!(start.elapsed() < Duration::from_secs(1));
    let mut report = Report::default();
    let mut n = 0;
    let result = poll(
        &mut FakeClock(Instant::now()),
        Duration::from_secs(3),
        1000,
        30,
        true,
        &mut report,
        &mut |_, _| {
            n += 1;
            if n == 2 {
                Ok((vec![], false))
            } else {
                Err(Failure::Timeout)
            }
        },
    );
    assert!(result.is_ok());
    assert_eq!(report.attempts, 2);
    drop(first);
    assert!(provider_lock(dir.path(), Instant::now() + Duration::from_millis(20)).is_ok());
}
#[test]
fn windows_are_counted_but_never_verified_and_all_classes_deduplicate() {
    let mut candidates = Candidates::default();
    let mut n = 0;
    let long = vec![b'Q'; 80];
    candidates.buffer(&long, &mut HashSet::new(), &mut |_| {
        n += 1;
        false
    });
    assert_eq!(n, 0);
    let a = key(111).aes;
    let b = key(112).aes.repeat(2);
    let utf: Vec<u8> = b.iter().flat_map(|b| [*b, 0]).collect();
    for _ in 0..2 {
        let mut tried = HashSet::new();
        for buf in [&a[..], &b[..], &utf[..]] {
            candidates.buffer(buf, &mut tried, &mut |_| {
                n += 1;
                false
            });
        }
    }
    let v = serde_json::to_value(&candidates.counts).unwrap();
    assert_eq!(v["windows"], 1);
    assert_eq!(v["ascii16"], 1);
    assert_eq!(v["ascii32"], 1);
    assert_eq!(v["utf16le32"], 1);
    assert_eq!(n, 4); // ascii32 与 utf16le32 同一个 AES 候选，每轮只反验一次。
}
#[test]
fn utf16_odd_alignment_and_chunk_overlap_are_supported() {
    let mut buf = vec![0xff];
    let bytes = key(113).aes.repeat(2);
    buf.extend(bytes.iter().flat_map(|b| [*b, 0]));
    buf.push(0xff);
    let mut candidates = Candidates::default();
    let mut tried = HashSet::new();
    let mut n = 0;
    // 模拟 65 字节重叠；首块末尾只包含部分候选，第二块包含完整边界。
    candidates.buffer(&buf[..40], &mut tried, &mut |_| {
        n += 1;
        false
    });
    candidates.buffer(&buf, &mut tried, &mut |_| {
        n += 1;
        false
    });
    assert_eq!(n, 1);
    assert_eq!(
        serde_json::to_value(&candidates.counts).unwrap()["utf16le32"],
        1
    );
    // 长 UTF-16LE 串内部的 32 字符也不能变成独立候选。
    let extended: Vec<_> = bytes
        .iter()
        .chain(bytes.iter())
        .flat_map(|b| [*b, 0])
        .collect();
    let mut n = 0;
    candidates.buffer(&extended, &mut HashSet::new(), &mut |_| {
        n += 1;
        false
    });
    assert_eq!(n, 0);
}
#[test]
fn multiple_head_matches_continue_after_bad_padding_until_full_decode() {
    use aes::cipher::{BlockDecrypt, KeyInit};
    let valid = key(114);
    let dat = encrypt(&png(), &valid, 16, 0);
    // 运行时构造一个 BMP 两字节首块误报，绝不把候选写入夹具或日志。
    let false_key = (0..2_000_000u32)
        .find_map(|seed| {
            let mut candidate = [b'Q'; 16];
            for (i, b) in candidate[..8].iter_mut().enumerate() {
                *b = b'A' + ((seed >> (i * 4)) & 15) as u8;
            }
            let aes = aes::Aes128::new((&candidate).into());
            let mut first =
                aes::cipher::generic_array::GenericArray::clone_from_slice(&dat[15..31]);
            aes.decrypt_block(&mut first);
            (decoder::detect_image_format(&first) == "bmp").then_some(candidate)
        })
        .expect("synthetic collision search should find a short-magic false positive");
    let mut vault = VaultStub::default();
    let mut scanner = Buffers {
        calls: 0,
        hit_at: 1,
        buffers: vec![false_key.to_vec(), valid.aes.repeat(2)],
    };
    let (result, report) = run(1, 10, &mut vault, &mut scanner, &dat);
    assert!(result.is_ok());
    assert_eq!(
        serde_json::to_value(&report).unwrap()["candidateCounts"]["headMatches"],
        2
    );
    assert_eq!(vault.writes, 1);
    assert!(vault.keys[0].aes == valid.aes);
}
#[test]
fn all_failed_head_matches_never_write_cache_and_keep_stage() {
    let valid = key(115);
    let dat = encrypt(b"\x89PNG\r\n\x1a\nsynthetic invalid image", &valid, 16, 0);
    let mut scanner = Buffers {
        calls: 0,
        hit_at: 1,
        buffers: vec![valid.aes.repeat(2)],
    };
    let mut vault = VaultStub::default();
    let (result, report) = run(1, 2, &mut vault, &mut scanner, &dat);
    assert_eq!(result.err(), Some(Failure::HarvestTimeout));
    assert_eq!(report.stage, Some(Stage::ImageDecode));
    assert_eq!(vault.writes, 0);
    assert_eq!(
        serde_json::to_value(&report).unwrap()["candidateCounts"]["headMatches"],
        1
    );
}
#[test]
fn resolve_default_scans_once_and_wait_mode_retries() {
    let mut report = Report::default();
    let result = poll(
        &mut FakeClock(Instant::now()),
        Duration::ZERO,
        5000,
        3,
        false,
        &mut report,
        &mut |_, _| Err(Failure::ImageKeyUnavailable),
    );
    assert_eq!(result.err(), Some(Failure::ImageKeyUnavailable));
    assert_eq!(report.attempts, 1);
    let mut report = Report::default();
    let result = poll(
        &mut FakeClock(Instant::now()),
        Duration::from_secs(11),
        5000,
        30,
        false,
        &mut report,
        &mut |_, _| Err(Failure::ImageKeyUnavailable),
    );
    assert_eq!(result.err(), Some(Failure::ImageKeyUnavailable));
    assert_eq!(report.attempts, 3);
    assert_eq!(report.elapsed_ms, 11000);
}
#[test]
fn synthetic_full_decode_failures_expose_only_stage_codes() {
    let valid = key(116);
    let dat = encrypt(&png(), &valid, 16, 0);
    assert_eq!(
        validate_layout(&V2_MAGIC).err(),
        Some(Failure::Incomplete(Stage::AesHead))
    );
    let mut truncated = dat.clone();
    truncated[6..10].copy_from_slice(&1000u32.to_le_bytes());
    assert_eq!(
        validate_layout(&truncated).err(),
        Some(Failure::Incomplete(Stage::AesBody))
    );
    let mut overlap = dat.clone();
    overlap[10..14].copy_from_slice(&1000u32.to_le_bytes());
    assert_eq!(
        validate_layout(&overlap).err(),
        Some(Failure::Incomplete(Stage::XorTail))
    );
    let mut wrong = key(117);
    assert!(open_staged(&dat, &wrong).unwrap().is_none()); // aes_head 未命中不误报资源损坏。
    let mut bad = dat.clone();
    bad[6..10].copy_from_slice(&17u32.to_le_bytes());
    assert_eq!(
        open_staged(&bad, &valid).err(),
        Some(Failure::Stage(Stage::AesBody))
    );
    let mut bad = dat.clone();
    bad[46] ^= 0x80;
    assert_eq!(
        open_staged(&bad, &valid).err(),
        Some(Failure::Stage(Stage::Pkcs7))
    );
    let mut bad = encrypt(&png(), &valid, 16, 8);
    let last = bad.len() - 2;
    bad[last] ^= 1;
    assert_eq!(
        infer_xor(&bad, "png").err(),
        Some(Failure::Stage(Stage::XorTail))
    );
    let unknown = encrypt(&[0; 32], &valid, 16, 0);
    assert_eq!(
        open_staged(&unknown, &valid).err(),
        Some(Failure::Stage(Stage::FormatDetect))
    );
    let deadline = Instant::now() + Duration::from_secs(1);
    assert_eq!(
        super::super::codec::standard(b"unknown", deadline).err(),
        Some(Failure::Stage(Stage::FormatDetect))
    );
    assert_eq!(
        super::super::codec::standard(b"\x89PNG\r\n\x1a\nsynthetic", deadline).err(),
        Some(Failure::Stage(Stage::ImageDecode))
    );
    wrong.aes.zeroize();
    for (stage, name) in [
        (Stage::AesHead, "aes_head"),
        (Stage::AesBody, "aes_body"),
        (Stage::Pkcs7, "pkcs7"),
        (Stage::XorTail, "xor_tail"),
        (Stage::FormatDetect, "format_detect"),
        (Stage::WxgfTranscode, "wxgf_transcode"),
        (Stage::ImageDecode, "image_decode"),
        (Stage::Other, "other"),
    ] {
        let response = crate::image::Response::new("synthetic-account", "synthetic-group", "1")
            .failed(Failure::Stage(stage));
        let v = serde_json::to_value(response).unwrap();
        assert_eq!(v["stage"], name);
    }
}

#[test]
fn protected_cache_is_written_once_and_reopened_without_scanning() {
    use std::cell::Cell;
    struct CountingProtector<'a>(&'a Cell<usize>);
    impl Protector for CountingProtector<'_> {
        fn protect(&self, data: &[u8], _: &str) -> Result<Vec<u8>, Failure> {
            self.0.set(self.0.get() + 1);
            Ok(data.iter().map(|b| b ^ 0xa5).collect())
        }
        fn unprotect(&self, data: &[u8], _: &str) -> Result<Vec<u8>, Failure> {
            Ok(data.iter().map(|b| b ^ 0xa5).collect())
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let writes = Cell::new(0);
    let key = key(118);
    let dat = encrypt(&png(), &key, 16, 0);
    let mut scanner = Buffers {
        calls: 0,
        hit_at: 1,
        buffers: vec![key.aes.repeat(2)],
    };
    let mut vault = ProtectedVault {
        dir: dir.path().into(),
        protector: CountingProtector(&writes),
    };
    let deadline = Instant::now() + Duration::from_secs(3);
    assert!(
        attempt(
            &dat,
            "synthetic",
            &mut vault,
            &mut scanner,
            deadline,
            &mut Candidates::default(),
            true
        )
        .unwrap()
        .1
    );
    let disk = std::fs::read(dir.path().join("image-keys.dpapi")).unwrap();
    assert!(serde_json::from_slice::<serde_json::Value>(&disk).is_err());
    assert!(!disk.windows(16).any(|b| b == key.aes));
    let mut reopened = ProtectedVault {
        dir: dir.path().into(),
        protector: CountingProtector(&writes),
    };
    assert!(
        !attempt(
            &dat,
            "synthetic",
            &mut reopened,
            &mut scanner,
            deadline,
            &mut Candidates::default(),
            true
        )
        .unwrap()
        .1
    );
    assert_eq!(writes.get(), 1);
    assert_eq!(scanner.calls, 1);
}

#[test]
fn default_resolve_preserves_retryable_lock_or_scan_timeout() {
    let mut report = Report::default();
    assert_eq!(
        poll(
            &mut FakeClock(Instant::now()),
            Duration::ZERO,
            5000,
            3,
            false,
            &mut report,
            &mut |_, _| Err(Failure::Timeout)
        )
        .err(),
        Some(Failure::Timeout)
    );
    assert_eq!(report.attempts, 1);
}

#[test]
fn remaining_fractional_budget_caps_attempt_deadlines_and_sleep() {
    let start = Instant::now();
    let mut clock = FakeClock(start);
    let mut report = Report::default();
    let end = start + Duration::from_millis(2500);
    let result = poll(
        &mut clock,
        Duration::from_millis(2500),
        1000,
        30,
        false,
        &mut report,
        &mut |deadline, _| {
            assert!(deadline <= end);
            Err(Failure::ImageKeyUnavailable)
        },
    );
    assert_eq!(result.err(), Some(Failure::ImageKeyUnavailable));
    assert_eq!(report.attempts, 3);
    assert_eq!(report.elapsed_ms, 2500);
}
