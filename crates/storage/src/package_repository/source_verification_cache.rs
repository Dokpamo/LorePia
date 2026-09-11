//! Bounded leases for the few large source archives, separate from renderer assets.

use std::{
    collections::VecDeque,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use super::{CoreResult, storage_corrupted};
use crate::verified_asset_cache::AssetFileSnapshot;

struct Entry {
    sha256: String,
    file: AssetFileSnapshot,
    verified_at: Instant,
}

static CACHE: OnceLock<Mutex<VecDeque<Entry>>> = OnceLock::new();

// Every lookup receives a newly no-follow-opened, path/size/link-checked handle.
// Only a still-open verified handle with exactly that identity can satisfy it.
pub(super) fn lookup(
    sha256: &str,
    current: &AssetFileSnapshot,
) -> CoreResult<Option<AssetFileSnapshot>> {
    let mut cache = CACHE
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| storage_corrupted("source verification cache is poisoned"))?;
    cache.retain(|entry| entry.verified_at.elapsed() < Duration::from_mins(1));
    for entry in cache.iter() {
        if entry.sha256 == sha256
            && entry
                .file
                .same_verified_identity(current)
                .map_err(|error| {
                    storage_corrupted(format!("verified source identity changed: {error}"))
                })?
        {
            return entry.file.verified_clone().map(Some).map_err(|error| {
                storage_corrupted(format!("cannot retain verified source: {error}"))
            });
        }
    }
    Ok(None)
}

pub(super) fn insert(sha256: &str, file: &AssetFileSnapshot) -> CoreResult<()> {
    let retained = file.verified_clone().map_err(|error| {
        storage_corrupted(format!("source changed after verification: {error}"))
    })?;
    let mut cache = CACHE
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| storage_corrupted("source verification cache is poisoned"))?;
    while cache.len() >= 4 {
        cache.pop_front();
    }
    cache.push_back(Entry {
        sha256: sha256.to_owned(),
        file: retained,
        verified_at: Instant::now(),
    });
    Ok(())
}
