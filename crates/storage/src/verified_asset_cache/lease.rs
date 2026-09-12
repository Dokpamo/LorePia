//! A digest's seek position and verified identity have one lock, independent of other assets.

use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use super::{AssetFileSnapshot, CacheLookup};
use lorepia_domain::AssetDescriptor;

pub(crate) struct AssetLease {
    pub(super) descriptor: AssetDescriptor,
    state: Mutex<LeaseState>,
}

pub(crate) struct LeaseState {
    file: Option<AssetFileSnapshot>,
    pub(super) verified_at: Instant,
    ttl: Duration,
}

impl AssetLease {
    pub(super) fn new(descriptor: AssetDescriptor, ttl: Duration) -> Self {
        Self {
            descriptor,
            state: Mutex::new(LeaseState {
                file: None,
                verified_at: Instant::now(),
                ttl,
            }),
        }
    }

    pub(crate) fn lock(&self) -> io::Result<MutexGuard<'_, LeaseState>> {
        self.state
            .lock()
            .map_err(|_| io::Error::other("verified asset lease lock was poisoned"))
    }
}

impl LeaseState {
    pub(crate) fn contains_verified(&mut self) -> io::Result<CacheLookup<()>> {
        self.with_file(|_| Ok(()))
    }

    pub(crate) fn read_range(
        &mut self,
        start: u64,
        length: u64,
    ) -> io::Result<CacheLookup<Vec<u8>>> {
        self.with_file(|file| {
            let capacity = usize::try_from(length)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "range is too large"))?;
            let mut bytes = vec![0; capacity];
            file.seek(SeekFrom::Start(start))?;
            file.read_exact(&mut bytes)?;
            Ok(bytes)
        })
    }

    pub(crate) fn insert(&mut self, file: AssetFileSnapshot) -> io::Result<()> {
        file.ensure_unchanged()?;
        self.file = Some(file);
        self.verified_at = Instant::now();
        Ok(())
    }

    fn with_file<T>(
        &mut self,
        operation: impl FnOnce(&mut File) -> io::Result<T>,
    ) -> io::Result<CacheLookup<T>> {
        let Some(mut file) = self.file.take() else {
            return Ok(CacheLookup::Miss);
        };
        if self.verified_at.elapsed() >= self.ttl {
            return Ok(CacheLookup::Miss);
        }
        if file.ensure_unchanged().is_err() {
            return Ok(CacheLookup::Changed);
        }
        let result = operation(file.file_mut())?;
        if file.ensure_unchanged().is_err() {
            return Ok(CacheLookup::Changed);
        }
        self.file = Some(file);
        Ok(CacheLookup::Hit(result))
    }
}

/// The global map lock admits a job; drop returns its permit without reacquiring that lock.
pub(crate) struct VerificationJob(pub(super) Arc<AtomicUsize>);

impl Drop for VerificationJob {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
