use lorepia_domain::{CoreError, CoreErrorCode, CoreResult, ImportLimits};

pub(super) fn checked_entry_size(
    current: u64,
    read: usize,
    name: &str,
    limits: ImportLimits,
) -> CoreResult<u64> {
    let updated = current
        .checked_add(read as u64)
        .ok_or_else(|| unsafe_archive("archive entry size overflow"))?;
    if updated > limits.max_entry_bytes {
        return Err(resource_limit(format!(
            "archive entry exceeds size limit while decoding: {name}"
        )));
    }
    Ok(updated)
}

pub(super) fn checked_total_size(
    current: u64,
    read: usize,
    limits: ImportLimits,
) -> CoreResult<u64> {
    let updated = current
        .checked_add(read as u64)
        .ok_or_else(|| unsafe_archive("archive size overflow"))?;
    if updated > limits.max_total_uncompressed_bytes {
        return Err(resource_limit(
            "archive exceeds total uncompressed size limit while decoding".to_owned(),
        ));
    }
    Ok(updated)
}

pub(super) fn resource_limit(message: String) -> CoreError {
    CoreError::new(CoreErrorCode::UnsupportedContent, message, true)
}

fn unsafe_archive(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::UnsafeArchive, message, false)
}
