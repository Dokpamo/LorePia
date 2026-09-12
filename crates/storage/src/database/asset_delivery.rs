use std::{io::Seek, sync::MutexGuard};

#[cfg(test)]
use std::sync::atomic::Ordering;

use lorepia_domain::{
    AssetDescriptor, AssetId, CoreError, CoreErrorCode, CoreResult, Sha256Digest,
};
use rusqlite::OptionalExtension;

use crate::verified_asset_cache::{
    AssetFileSnapshot, CacheLookup, VerifiedAssetCache, open_asset_file,
};

mod image_validation;
#[cfg(test)]
mod policy_tests;

const MAX_APPROVED_ASSET_READ_BYTES: u64 = 64 * 1_024 * 1_024;
const MAX_APPROVED_IMAGE_BYTES: u64 = 16 * 1_024 * 1_024;

/// One verified, bounded byte range from an approved content-addressed asset.
///
/// The storage-owned path and database row never leave this crate. Callers
/// receive only immutable descriptor metadata and bytes from the exact
/// digest-addressed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovedAssetRange {
    pub descriptor: AssetDescriptor,
    pub start: u64,
    pub bytes: Vec<u8>,
    /// Same-handle display checks, never accepted from renderer input.
    pub image_validation_policy: u32,
}

impl ApprovedAssetRange {
    /// Storage owns this policy; Core/Shell forward it only to native Rust callers.
    pub const IMAGE_VALIDATION_POLICY: u32 = 1;
}

use super::package_cas::{validate_renderer_media_type, verify_open_file_media_type_signature};

use super::{
    Storage, content_relative_path, ensure_real_directory, hash_open_file, i64_to_u64,
    storage_corrupted, storage_db_error, storage_io_error,
};

impl Storage {
    /// Resolves an immutable descriptor only when its CAS bytes still match
    /// the exact database hash, size, media type, and safe renderer allowlist.
    pub fn resolve_approved_asset_by_id(&self, asset_id: &AssetId) -> CoreResult<AssetDescriptor> {
        let record = self.approved_asset_record(
            "SELECT ad.payload_json, a.relative_path, a.media_type, a.size_bytes
             FROM asset_descriptors ad
             JOIN assets a ON a.sha256 = ad.asset_hash
             WHERE ad.id = ?1",
            asset_id.as_str(),
        )?;
        if &record.0.id != asset_id {
            return Err(storage_corrupted(
                "approved asset descriptor identity diverges from its row",
            ));
        }
        self.verify_approved_asset(&record.0, &record.1)?;
        Ok(record.0)
    }

    /// Resolves a digest only when at least one immutable approved descriptor
    /// names the same exact CAS object.
    pub fn resolve_approved_asset_by_sha256(
        &self,
        sha256: &Sha256Digest,
    ) -> CoreResult<AssetDescriptor> {
        let record = self.approved_asset_record(
            "SELECT ad.payload_json, a.relative_path, a.media_type, a.size_bytes
             FROM asset_descriptors ad
             JOIN assets a ON a.sha256 = ad.asset_hash
             WHERE ad.asset_hash = ?1
             ORDER BY ad.id
             LIMIT 1",
            sha256.as_str(),
        )?;
        if &record.0.sha256 != sha256 {
            return Err(storage_corrupted(
                "approved asset descriptor digest diverges from its row",
            ));
        }
        self.verify_approved_asset(&record.0, &record.1)?;
        Ok(record.0)
    }

    /// Reads one bounded range from a short-lived verified CAS handle.
    ///
    /// Cache misses hash and signature-check the complete file. Cache hits
    /// reuse that exact handle, revalidate its file identity before and after
    /// the read, and seek only the requested range.
    pub fn read_approved_asset_range(
        &self,
        sha256: &Sha256Digest,
        start: u64,
        requested_bytes: u64,
    ) -> CoreResult<ApprovedAssetRange> {
        if requested_bytes == 0 || requested_bytes > MAX_APPROVED_ASSET_READ_BYTES {
            return Err(CoreError::invalid(
                "approved asset range length is outside the bounded limit",
            ));
        }
        let (descriptor, relative_path) = self.approved_asset_record(
            "SELECT ad.payload_json, a.relative_path, a.media_type, a.size_bytes
             FROM asset_descriptors ad
             JOIN assets a ON a.sha256 = ad.asset_hash
             WHERE ad.asset_hash = ?1
             ORDER BY ad.id
             LIMIT 1",
            sha256.as_str(),
        )?;
        if &descriptor.sha256 != sha256 {
            return Err(storage_corrupted(
                "approved asset descriptor digest diverges from its row",
            ));
        }
        self.validate_approved_asset_relative_path(&descriptor, &relative_path)?;
        if start >= descriptor.size_bytes {
            return Err(CoreError::invalid(
                "approved asset range starts beyond the content length",
            ));
        }
        let available = descriptor.size_bytes - start;
        let length = requested_bytes.min(available);
        let lease = self
            .verified_asset_cache()?
            .lease(&descriptor)
            .map_err(asset_cache_error)?;
        let mut entry = lease.lock().map_err(storage_io_error)?;
        let bytes = match entry.read_range(start, length).map_err(storage_io_error)? {
            CacheLookup::Hit(bytes) => bytes,
            CacheLookup::Changed => {
                return Err(storage_corrupted(
                    "approved asset changed after it was verified",
                ));
            }
            CacheLookup::Miss => {
                let _job = self
                    .verified_asset_cache()?
                    .begin_verification(descriptor.size_bytes)
                    .map_err(storage_io_error)?;
                let (file, validated_png) =
                    self.open_verified_approved_asset(&descriptor, &relative_path)?;
                entry.insert(file).map_err(storage_io_error)?;
                let read = if let Some(bytes) = validated_png {
                    // Cold CRC already read these exact bytes from this handle.
                    // Keep a bounded response allocation, never cache the full PNG.
                    match entry.contains_verified().map_err(storage_io_error)? {
                        CacheLookup::Hit(()) => {
                            CacheLookup::Hit(verified_png_range(bytes, start, length)?)
                        }
                        CacheLookup::Miss => CacheLookup::Miss,
                        CacheLookup::Changed => CacheLookup::Changed,
                    }
                } else {
                    entry.read_range(start, length).map_err(storage_io_error)?
                };
                match read {
                    CacheLookup::Hit(bytes) => bytes,
                    CacheLookup::Miss | CacheLookup::Changed => {
                        return Err(storage_corrupted(
                            "approved asset verification lease could not be established",
                        ));
                    }
                }
            }
        };
        Ok(ApprovedAssetRange {
            image_validation_policy: if descriptor.media_type.starts_with("image/") {
                ApprovedAssetRange::IMAGE_VALIDATION_POLICY
            } else {
                0
            },
            descriptor,
            start,
            bytes,
        })
    }

    fn approved_asset_record(
        &self,
        query: &str,
        key: &str,
    ) -> CoreResult<(AssetDescriptor, String)> {
        let row = self
            .connection()?
            .prepare_cached(query)
            .map_err(storage_db_error)?
            .query_row([key], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::NotFound,
                    "approved asset was not found",
                    false,
                )
            })?;
        let descriptor = serde_json::from_str::<AssetDescriptor>(&row.0).map_err(|error| {
            storage_corrupted(format!(
                "approved asset descriptor cannot be decoded: {error}"
            ))
        })?;
        if descriptor.media_type != row.2
            || descriptor.size_bytes != i64_to_u64("approved asset size", row.3)?
        {
            return Err(storage_corrupted(
                "approved asset descriptor diverges from CAS metadata",
            ));
        }
        validate_renderer_media_type(&descriptor.media_type)?;
        if descriptor.size_bytes == 0
            || descriptor.size_bytes > MAX_APPROVED_ASSET_READ_BYTES
            || (descriptor.media_type.starts_with("image/")
                && descriptor.size_bytes > MAX_APPROVED_IMAGE_BYTES)
        {
            return Err(CoreError::new(
                CoreErrorCode::UnsafeArchive,
                "approved asset size is outside renderer bounds",
                false,
            ));
        }
        Ok((descriptor, row.1))
    }

    fn verify_approved_asset(
        &self,
        descriptor: &AssetDescriptor,
        relative_path: &str,
    ) -> CoreResult<()> {
        self.validate_approved_asset_relative_path(descriptor, relative_path)?;
        let lease = self
            .verified_asset_cache()?
            .lease(descriptor)
            .map_err(asset_cache_error)?;
        let mut entry = lease.lock().map_err(storage_io_error)?;
        match entry.contains_verified().map_err(storage_io_error)? {
            CacheLookup::Hit(()) => Ok(()),
            CacheLookup::Changed => Err(storage_corrupted(
                "approved asset changed after it was verified",
            )),
            CacheLookup::Miss => {
                let _job = self
                    .verified_asset_cache()?
                    .begin_verification(descriptor.size_bytes)
                    .map_err(storage_io_error)?;
                let (file, _) = self.open_verified_approved_asset(descriptor, relative_path)?;
                entry.insert(file).map_err(storage_io_error)
            }
        }
    }

    fn validate_approved_asset_relative_path(
        &self,
        descriptor: &AssetDescriptor,
        relative_path: &str,
    ) -> CoreResult<()> {
        let expected = format!(
            "assets/{}",
            content_relative_path(descriptor.sha256.as_str())?
        );
        if relative_path != expected {
            return Err(storage_corrupted(
                "approved asset CAS path does not match its digest",
            ));
        }
        let prefix = descriptor
            .sha256
            .as_str()
            .get(..2)
            .ok_or_else(|| storage_corrupted("approved asset digest is malformed"))?;
        ensure_real_directory(&self.root.join("assets"))?;
        ensure_real_directory(&self.root.join("assets/sha256"))?;
        ensure_real_directory(&self.root.join("assets/sha256").join(prefix))
    }

    fn open_verified_approved_asset(
        &self,
        descriptor: &AssetDescriptor,
        relative_path: &str,
    ) -> CoreResult<(AssetFileSnapshot, Option<Vec<u8>>)> {
        self.validate_approved_asset_relative_path(descriptor, relative_path)?;
        let file =
            open_asset_file(&self.root, descriptor.sha256.as_str()).map_err(storage_io_error)?;
        let metadata = file.metadata().map_err(storage_io_error)?;
        if !metadata.is_file() || metadata.len() != descriptor.size_bytes {
            return Err(storage_corrupted(
                "approved asset file size does not match its descriptor",
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;

            if metadata.nlink() != 1 {
                return Err(storage_corrupted(
                    "approved asset must not have hard-link aliases",
                ));
            }
        }
        let mut file = AssetFileSnapshot::capture(file).map_err(storage_io_error)?;
        #[cfg(test)]
        self.approved_asset_hash_verifications
            .fetch_add(1, Ordering::Relaxed);
        let (actual_sha256, actual_size) = hash_open_file(file.file_mut())?;
        if actual_sha256 != descriptor.sha256.as_str() || actual_size != descriptor.size_bytes {
            return Err(storage_corrupted(
                "approved asset bytes do not match their descriptor digest",
            ));
        }
        file.file_mut().rewind().map_err(storage_io_error)?;
        verify_open_file_media_type_signature(file.file_mut(), &descriptor.media_type)?;
        file.file_mut().rewind().map_err(storage_io_error)?;
        let validated_png = image_validation::validate(file.file_mut(), descriptor)?;
        file.file_mut().rewind().map_err(storage_io_error)?;
        file.ensure_unchanged().map_err(|error| {
            storage_corrupted(format!(
                "approved asset changed while it was being verified: {error}"
            ))
        })?;
        Ok((file, validated_png))
    }

    fn verified_asset_cache(&self) -> CoreResult<MutexGuard<'_, VerifiedAssetCache>> {
        self.verified_asset_cache.lock().map_err(|_| {
            CoreError::new(
                CoreErrorCode::StorageUnavailable,
                "verified asset cache lock was poisoned",
                true,
            )
        })
    }

    #[cfg(test)]
    pub(super) fn approved_asset_hash_verification_count(&self) -> usize {
        self.approved_asset_hash_verifications
            .load(Ordering::Relaxed)
    }
}

fn asset_cache_error(error: std::io::Error) -> CoreError {
    if error.kind() == std::io::ErrorKind::InvalidData {
        storage_corrupted(error.to_string())
    } else {
        storage_io_error(error)
    }
}

fn verified_png_range(bytes: Vec<u8>, start: u64, length: u64) -> CoreResult<Vec<u8>> {
    let start = usize::try_from(start).map_err(|_| storage_corrupted("image offset overflow"))?;
    let length = usize::try_from(length).map_err(|_| storage_corrupted("image length overflow"))?;
    if start == 0 && length == bytes.len() {
        return Ok(bytes);
    }
    let end = start
        .checked_add(length)
        .ok_or_else(|| storage_corrupted("image range overflow"))?;
    bytes
        .get(start..end)
        .map(<[u8]>::to_vec)
        .ok_or_else(|| storage_corrupted("verified image range exceeds its bytes"))
}
