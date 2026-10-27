//! 可注入的单次扫描接口。只暴露计数，候选不实现 Debug、不会序列化。
#![cfg_attr(not(any(windows, test)), allow(dead_code))]
use super::Failure;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

#[derive(Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Counts {
    ascii32: u64,
    ascii16: u64,
    utf16le32: u64,
    windows: u64,
    head_matches: u64,
}
#[derive(Default)]
pub(crate) struct Candidates {
    seen: [HashSet<[u8; 32]>; 4],
    heads: HashSet<[u8; 32]>,
    pub counts: Counts,
}
impl Candidates {
    fn record(&mut self, class: usize, bytes: &[u8]) {
        if self.seen[class].insert(Sha256::digest(bytes).into()) {
            match class {
                0 => self.counts.ascii32 += 1,
                1 => self.counts.ascii16 += 1,
                2 => self.counts.utf16le32 += 1,
                _ => self.counts.windows += 1,
            }
        }
    }
    pub fn head_hash(&mut self, hash: [u8; 32]) {
        if self.heads.insert(hash) {
            self.counts.head_matches += 1;
        }
    }
    /// tried 每次扫描清空，seen 跨轮去重。窗口只计数，绝不进入反验回调。
    pub fn buffer(
        &mut self,
        buf: &[u8],
        tried: &mut HashSet<[u8; 32]>,
        accept: &mut dyn FnMut([u8; 16]) -> bool,
    ) -> bool {
        for (class, len) in [(0, 32), (1, 16)] {
            for candidate in crate::attachment::image_key::ascii_alnum_candidates(buf, len) {
                self.record(class, candidate);
                let mut key = zeroize::Zeroizing::new([0u8; 16]);
                key.copy_from_slice(&candidate[..16]);
                if tried.insert(Sha256::digest(*key).into()) && accept(*key) {
                    return true;
                }
            }
        }
        for start in 0..buf.len().saturating_sub(63) {
            let candidate = &buf[start..start + 64];
            if !candidate
                .as_chunks::<2>()
                .0
                .iter()
                .all(|c| c[0].is_ascii_alphanumeric() && c[1] == 0)
            {
                continue;
            }
            let left =
                start < 2 || !(buf[start - 2].is_ascii_alphanumeric() && buf[start - 1] == 0);
            let right = start + 65 >= buf.len()
                || !(buf[start + 64].is_ascii_alphanumeric() && buf[start + 65] == 0);
            if !left || !right {
                continue;
            }
            self.record(2, candidate);
            let mut key = zeroize::Zeroizing::new([0u8; 16]);
            for (i, b) in key.iter_mut().enumerate() {
                *b = candidate[i * 2];
            }
            if tried.insert(Sha256::digest(*key).into()) && accept(*key) {
                return true;
            }
        }
        for run in buf
            .split(|b| !b.is_ascii_alphanumeric())
            .filter(|r| r.len() > 32)
        {
            for window in run.windows(16) {
                self.record(3, window);
            }
        }
        false
    }
}

pub(crate) trait Scanner {
    fn scan(
        &mut self,
        dat: &[u8],
        deadline: Instant,
        candidates: &mut Candidates,
        accept: &mut dyn FnMut([u8; 16]) -> bool,
    ) -> Result<(), Failure>;
}
pub(crate) trait Clock {
    fn now(&self) -> Instant;
    fn sleep(&mut self, duration: Duration);
}
pub(crate) struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
    fn sleep(&mut self, duration: Duration) {
        std::thread::sleep(duration)
    }
}
#[derive(Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Report {
    pub status: String,
    pub attempts: u64,
    pub elapsed_ms: u64,
    pub candidate_counts: Counts,
    pub cache_written: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<super::Stage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
}

impl Report {
    pub fn failed(&mut self, failure: Failure) {
        self.status = "unavailable".into();
        self.error_code = Some(failure.code().into());
        self.retryable = Some(failure.retryable());
        if let Failure::Stage(stage) | Failure::Incomplete(stage) = failure {
            self.stage = Some(stage);
        }
    }
}

pub(crate) struct NativeScanner<'a> {
    pub db_dir: &'a std::path::Path,
    pub scanned: bool,
}
impl Scanner for NativeScanner<'_> {
    fn scan(
        &mut self,
        dat: &[u8],
        deadline: Instant,
        candidates: &mut Candidates,
        accept: &mut dyn FnMut([u8; 16]) -> bool,
    ) -> Result<(), Failure> {
        self.scanned = true;
        #[cfg(windows)]
        {
            crate::attachment::image_key::windows::scan_target(
                self.db_dir,
                dat,
                deadline,
                candidates,
                accept,
            )
        }
        #[cfg(not(windows))]
        {
            let _ = (self.db_dir, dat, deadline, candidates, accept);
            Err(Failure::ImageKeyUnavailable)
        }
    }
}
