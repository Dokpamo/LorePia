//! Verified, path-private access to committed source bytes for native export.
//!
//! The webview never receives this type. Core may use it to hand one exact CAS
//! object to the native save boundary after storage has revalidated the source
//! row, canonical path, size, and SHA-256 digest.

use std::{fmt, path::Path};

use lorepia_domain::{CoreError, CoreErrorCode, CoreResult, Sha256Digest};
use rusqlite::OptionalExtension;

use crate::Storage;

/// One exact committed character source reopened from project-owned CAS.
///
/// This type deliberately implements neither `Serialize` nor `Clone`. Its
/// debug representation redacts the absolute path.
pub struct VerifiedContentSource {
    sha256: Sha256Digest,
    size_bytes: u64,
    path: std::path::PathBuf,
}

impl VerifiedContentSource {
    pub fn sha256(&self) -> &Sha256Digest {
        &self.sha256
    }

    pub const fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    #[doc(hidden)]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl fmt::Debug for VerifiedContentSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedContentSource")
            .field("sha256", &self.sha256)
            .field("size_bytes", &self.size_bytes)
            .field("path", &"[REDACTED]")
            .finish()
    }
}

impl Storage {
    /// Reopens the exact immutable source committed for one character.
    pub fn verified_character_source(
        &self,
        character_id: &str,
    ) -> CoreResult<VerifiedContentSource> {
        let character = self.get_character(character_id)?;
        let sha256 = Sha256Digest::parse(character.source_hash).map_err(|error| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                format!("stored character source digest is invalid: {error}"),
                false,
            )
        })?;
        let connection = self.connection()?;
        let stored_size = connection
            .query_row(
                "SELECT size_bytes FROM content_sources WHERE sha256 = ?1",
                [sha256.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|error| {
                CoreError::new(
                    CoreErrorCode::StorageUnavailable,
                    format!("cannot resolve committed character source: {error}"),
                    true,
                )
            })?
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "committed character source row is missing",
                    false,
                )
            })?;
        drop(connection);
        let size_bytes = u64::try_from(stored_size).map_err(|_| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "committed character source size is invalid",
                false,
            )
        })?;
        if size_bytes == 0 {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "committed character source is empty",
                false,
            ));
        }
        let path = self.package_source_path(sha256.as_str(), size_bytes)?;
        Ok(VerifiedContentSource {
            sha256,
            size_bytes,
            path,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_not_serializable(_: &VerifiedContentSource) {}

    #[test]
    fn verified_source_debug_redacts_its_path() {
        let source = VerifiedContentSource {
            sha256: Sha256Digest::parse("ab".repeat(32)).expect("digest"),
            size_bytes: 7,
            path: "/Users/synthetic/private/source.json".into(),
        };
        assert_not_serializable(&source);
        let debug = format!("{source:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("/Users/"));
    }
}
