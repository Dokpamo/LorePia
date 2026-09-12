//! The map is never held while waiting for a digest or reading its file.
#[cfg(test)]
use super::{AssetFileSnapshot, CacheLookup};
use super::{AssetLease, VerificationBudget, VerificationJob, VerifiedAssetCache};
use lorepia_domain::AssetDescriptor;
use std::{
    collections::VecDeque,
    io,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

impl VerifiedAssetCache {
    pub(crate) fn new(max_handles: usize, lease_ttl: Duration) -> Self {
        Self {
            entries: VecDeque::new(),
            active_verifications: Arc::new(AtomicUsize::new(0)),
            verification_budget: VerificationBudget::new(Instant::now()),
            max_handles: max_handles.max(1),
            lease_ttl,
        }
    }

    /// Call under the map lock, then release it before waiting on this digest's lock.
    pub(crate) fn lease(&mut self, descriptor: &AssetDescriptor) -> io::Result<Arc<AssetLease>> {
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.descriptor.sha256 == descriptor.sha256)
        {
            let entry = self.entries.remove(index).expect("located cache slot");
            if !same_file_contract(&entry.descriptor, descriptor) {
                self.entries.push_back(entry);
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "approved asset contract changed",
                ));
            }
            self.entries.push_back(Arc::clone(&entry));
            return Ok(entry);
        }
        if self.entries.len() >= self.max_handles {
            let Some(index) = self
                .entries
                .iter()
                .position(|entry| Arc::strong_count(entry) == 1)
            else {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "verified asset handles are temporarily busy",
                ));
            };
            self.entries.remove(index);
        }
        let entry = Arc::new(AssetLease::new(descriptor.clone(), self.lease_ttl));
        self.entries.push_back(Arc::clone(&entry));
        Ok(entry)
    }

    pub(crate) fn begin_verification(&mut self, size_bytes: u64) -> io::Result<VerificationJob> {
        if self.active_verifications.load(Ordering::Acquire) >= 4 {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "verified asset jobs are temporarily busy",
            ));
        }
        self.verification_budget.admit(size_bytes, Instant::now())?;
        self.active_verifications.fetch_add(1, Ordering::AcqRel);
        Ok(VerificationJob(Arc::clone(&self.active_verifications)))
    }

    #[cfg(test)]
    pub(super) fn contains_verified(
        &mut self,
        descriptor: &AssetDescriptor,
    ) -> io::Result<CacheLookup<()>> {
        self.lease(descriptor)?.lock()?.contains_verified()
    }

    #[cfg(test)]
    pub(super) fn read_range(
        &mut self,
        descriptor: &AssetDescriptor,
        start: u64,
        length: u64,
    ) -> io::Result<CacheLookup<Vec<u8>>> {
        self.lease(descriptor)?.lock()?.read_range(start, length)
    }

    #[cfg(test)]
    pub(super) fn insert(
        &mut self,
        descriptor: AssetDescriptor,
        file: AssetFileSnapshot,
    ) -> io::Result<()> {
        self.lease(&descriptor)?.lock()?.insert(file)
    }
}

fn same_file_contract(left: &AssetDescriptor, right: &AssetDescriptor) -> bool {
    left.sha256 == right.sha256
        && left.size_bytes == right.size_bytes
        && left.media_type == right.media_type
}
