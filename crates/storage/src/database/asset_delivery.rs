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

use super::{
    ApprovedAssetRange, MAX_APPROVED_ASSET_READ_BYTES, Storage, content_relative_path,
    ensure_real_directory, hash_open_file, i64_to_u64, storage_corrupted, storage_db_error,
    storage_io_error, validate_renderer_media_type, verify_open_file_media_type_signature,
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
        let mut cache = self.verified_asset_cache()?;
        let bytes = match cache
            .read_range(&descriptor, start, length)
            .map_err(storage_io_error)?
        {
            CacheLookup::Hit(bytes) => bytes,
            CacheLookup::Changed => {
                return Err(storage_corrupted(
                    "approved asset changed after it was verified",
                ));
            }
            CacheLookup::Miss => {
                cache.begin_verification().map_err(storage_io_error)?;
                let file = self.open_verified_approved_asset(&descriptor, &relative_path)?;
                cache
                    .insert(descriptor.clone(), file)
                    .map_err(storage_io_error)?;
                match cache
                    .read_range(&descriptor, start, length)
                    .map_err(storage_io_error)?
                {
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
            .query_row(query, [key], |row| {
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
        Ok((descriptor, row.1))
    }

    fn verify_approved_asset(
        &self,
        descriptor: &AssetDescriptor,
        relative_path: &str,
    ) -> CoreResult<()> {
        self.validate_approved_asset_relative_path(descriptor, relative_path)?;
        let mut cache = self.verified_asset_cache()?;
        match cache
            .contains_verified(descriptor)
            .map_err(storage_io_error)?
        {
            CacheLookup::Hit(()) => Ok(()),
            CacheLookup::Changed => Err(storage_corrupted(
                "approved asset changed after it was verified",
            )),
            CacheLookup::Miss => {
                cache.begin_verification().map_err(storage_io_error)?;
                let file = self.open_verified_approved_asset(descriptor, relative_path)?;
                cache
                    .insert(descriptor.clone(), file)
                    .map_err(storage_io_error)
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
    ) -> CoreResult<AssetFileSnapshot> {
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
        file.ensure_unchanged().map_err(|error| {
            storage_corrupted(format!(
                "approved asset changed while it was being verified: {error}"
            ))
        })?;
        Ok(file)
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
