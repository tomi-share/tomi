//! Free disk space probe (`min_free_disk_bytes`, `api-v1.md` §8.4), replaceable in tests.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// Free space of the filesystem holding a directory.
pub trait DiskProbe: Send + Sync {
    fn available_bytes(&self, dir: &Path) -> u64;
}

/// The real filesystem.
#[derive(Debug, Default, Clone, Copy)]
pub struct RealDisk;

impl DiskProbe for RealDisk {
    fn available_bytes(&self, dir: &Path) -> u64 {
        // An unreadable probe must not turn into "disk full" nor "infinite space": report
        // a large value and let the actual write fail if the disk really is full.
        fs4::available_space(dir).unwrap_or(u64::MAX)
    }
}

/// A probe whose answer the test sets.
#[derive(Debug)]
pub struct FixedDisk(AtomicU64);

impl FixedDisk {
    pub fn new(bytes: u64) -> Self {
        Self(AtomicU64::new(bytes))
    }
    pub fn set(&self, bytes: u64) {
        self.0.store(bytes, Ordering::SeqCst);
    }
}

impl DiskProbe for FixedDisk {
    fn available_bytes(&self, _dir: &Path) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_probe_reports_some_space() {
        let dir = tempfile::tempdir().unwrap();
        assert!(RealDisk.available_bytes(dir.path()) > 0);
    }

    #[test]
    fn fixed_probe_is_settable() {
        let d = FixedDisk::new(10);
        assert_eq!(d.available_bytes(Path::new(".")), 10);
        d.set(3);
        assert_eq!(d.available_bytes(Path::new(".")), 3);
    }
}
