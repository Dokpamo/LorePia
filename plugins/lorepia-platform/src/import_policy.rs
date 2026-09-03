use tauri::Runtime;

use crate::{LorepiaPlatform, PlatformError, PlatformErrorCode, PlatformResult, StagedImport};

/// Default native staging ceiling for ordinary imports.
pub const MAXIMUM_STANDARD_IMPORT_BYTES: u64 = 256 * 1024 * 1024;
/// Fixed ceiling available only through the explicit large-import UI path.
pub const MAXIMUM_USER_APPROVED_IMPORT_BYTES: u64 = 16 * 1024 * 1024 * 1024;

impl<R: Runtime> LorepiaPlatform<R> {
    pub async fn pick_import(&self) -> PlatformResult<Option<StagedImport>> {
        self.pick_import_with_limit(MAXIMUM_STANDARD_IMPORT_BYTES)
            .await
    }

    pub async fn pick_import_with_limit(
        &self,
        maximum_bytes: u64,
    ) -> PlatformResult<Option<StagedImport>> {
        if !matches!(
            maximum_bytes,
            MAXIMUM_STANDARD_IMPORT_BYTES | MAXIMUM_USER_APPROVED_IMPORT_BYTES
        ) {
            return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
        }
        self.inner.pick_import(maximum_bytes).await
    }
}
