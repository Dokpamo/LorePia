use std::path::Path;

use lorepia_domain::{CoreError, CoreErrorCode, CoreResult, ImportLimits};

pub(crate) fn validated_source_metadata(
    path: &Path,
    limits: ImportLimits,
) -> CoreResult<std::fs::Metadata> {
    let source_metadata = path.symlink_metadata().map_err(|error| {
        CoreError::new(
            CoreErrorCode::StorageUnavailable,
            format!("cannot read staging file metadata: {error}"),
            true,
        )
    })?;
    if source_metadata.file_type().is_symlink() {
        return Err(CoreError::new(
            CoreErrorCode::UnsafeArchive,
            "the import source must not be a symbolic link",
            false,
        ));
    }
    if !source_metadata.is_file() {
        return Err(CoreError::invalid(
            "the import source is not a regular file",
        ));
    }
    if source_metadata.len() == 0 {
        return Err(CoreError::new(
            CoreErrorCode::UnsupportedContent,
            "the import source is empty",
            false,
        ));
    }
    if source_metadata.len() > limits.max_source_bytes {
        return Err(CoreError::new(
            CoreErrorCode::UnsupportedContent,
            format!(
                "source is {} bytes; maximum is {} bytes",
                source_metadata.len(),
                limits.max_source_bytes
            ),
            true,
        ));
    }
    Ok(source_metadata)
}
