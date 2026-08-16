#[cfg(any(mobile, target_os = "macos", windows, test))]
use std::{
    fs::OpenOptions,
    io::Read,
    path::{Path, PathBuf},
};

#[cfg(all(unix, any(mobile, target_os = "macos", test)))]
use std::os::unix::fs::OpenOptionsExt;

#[cfg(any(mobile, target_os = "macos", windows, test))]
use sha2::{Digest, Sha256};

use crate::{PlatformError, PlatformErrorCode, PlatformResult};

pub(crate) const MAXIMUM_REFERENCE_BYTES: usize = 256;
#[cfg(any(mobile, target_os = "macos", windows, test))]
pub(crate) const MAXIMUM_CREDENTIAL_READ_BYTES: usize = 32 * 1024;
pub(crate) const MAXIMUM_CREDENTIAL_WRITE_BYTES: usize = 16 * 1024;
#[cfg(any(mobile, target_os = "macos", windows, test))]
pub(crate) const MAXIMUM_SENSITIVE_CAPTURE_BYTES: usize = 1024 * 1024;
#[cfg(any(mobile, target_os = "macos", windows, test))]
pub(crate) const MAXIMUM_EXPORT_NAME_BYTES: usize = 128;
#[cfg(any(mobile, target_os = "macos", windows, test))]
pub(crate) const MAXIMUM_RECEIPT_NAME_CHARACTERS: usize = 255;

pub(crate) fn validate_reference(reference: &str) -> PlatformResult<()> {
    if reference.trim().is_empty() || reference.len() > MAXIMUM_REFERENCE_BYTES {
        return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
    }
    Ok(())
}

#[cfg(any(mobile, target_os = "macos", windows, test))]
pub(crate) fn validate_credential_read(value: &str) -> PlatformResult<()> {
    if value.trim().is_empty() || value.len() > MAXIMUM_CREDENTIAL_READ_BYTES {
        return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
    }
    Ok(())
}

#[cfg(any(mobile, target_os = "macos", windows, test))]
pub(crate) fn validate_credential_write(value: &str) -> PlatformResult<()> {
    if value.trim().is_empty() || value.len() > MAXIMUM_CREDENTIAL_WRITE_BYTES {
        return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
    }
    Ok(())
}

#[cfg(any(mobile, target_os = "macos", windows, test))]
pub(crate) fn validate_sensitive_capture(value: &str, maximum_bytes: usize) -> PlatformResult<()> {
    if maximum_bytes == 0
        || maximum_bytes > MAXIMUM_SENSITIVE_CAPTURE_BYTES
        || value.trim().is_empty()
        || value.len() > maximum_bytes
    {
        return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
    }
    Ok(())
}

#[cfg(any(mobile, target_os = "macos", windows, test))]
pub(crate) fn validate_export_sha256(value: &str) -> PlatformResult<()> {
    if value.len() != 64
        || !value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
    }
    Ok(())
}

/// Suggested names are deliberately stricter than display names returned by
/// an OS picker. They cross a native boundary and may become a filesystem
/// component on every supported platform.
#[cfg(any(mobile, target_os = "macos", windows, test))]
pub(crate) fn validate_export_suggested_name(value: &str) -> PlatformResult<()> {
    let valid_bytes = !value.is_empty()
        && value.len() <= MAXIMUM_EXPORT_NAME_BYTES
        && value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'.' | b'_' | b'-'));
    let stem = value.split('.').next().unwrap_or_default();
    let reserved_windows_stem = matches!(
        stem.to_ascii_uppercase().as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    );
    if !valid_bytes
        || value.starts_with('.')
        || value.ends_with('.')
        || value.contains("..")
        || reserved_windows_stem
    {
        return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
    }
    Ok(())
}

#[cfg(any(mobile, target_os = "macos", windows, test))]
pub(crate) fn validate_export_receipt_display_name(value: &str) -> PlatformResult<()> {
    let valid = !value.trim().is_empty()
        && value != "."
        && value != ".."
        && value.chars().count() <= MAXIMUM_RECEIPT_NAME_CHARACTERS
        && !value
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\'));
    if !valid {
        return Err(PlatformError::new(PlatformErrorCode::SelectionFailed));
    }
    Ok(())
}

/// Verify that a source is the exact immutable CAS object named by its digest.
///
/// The caller receives no file handle because desktop and mobile operations
/// must re-open and re-verify immediately before copying, closing the picker
/// delay as a mutation window.
#[cfg(any(mobile, target_os = "macos", windows, test))]
pub(crate) fn verify_content_source_for_export(
    source_path: &Path,
    data_root: &Path,
    expected_size_bytes: u64,
    expected_sha256: &str,
    suggested_name: &str,
) -> PlatformResult<()> {
    validate_export_sha256(expected_sha256)?;
    validate_export_suggested_name(suggested_name)?;
    if expected_size_bytes == 0
        || expected_size_bytes > i64::MAX as u64
        || !source_path.is_absolute()
    {
        return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
    }

    let canonical_root = std::fs::canonicalize(data_root)
        .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
    let expected_path = expected_content_source_path(&canonical_root, expected_sha256);
    let canonical_source = std::fs::canonicalize(source_path)
        .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
    let source_metadata = std::fs::symlink_metadata(source_path)
        .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
    if canonical_source != expected_path
        || source_metadata.file_type().is_symlink()
        || !source_metadata.file_type().is_file()
        || source_metadata.len() != expected_size_bytes
    {
        return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
    }

    let (actual_sha256, actual_size_bytes) = hash_regular_file(source_path)?;
    if actual_sha256 != expected_sha256 || actual_size_bytes != expected_size_bytes {
        return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
    }
    Ok(())
}

/// Reject destinations inside the application data root.
///
/// A save picker is a user-selected export surface, not a way for the export
/// command to overwrite the database or another immutable CAS object. The
/// destination itself may not exist yet, so its existing parent is the
/// canonical authority used for this containment check.
#[cfg(any(target_os = "macos", windows, test))]
pub(crate) fn validate_content_export_destination(
    destination: &Path,
    data_root: &Path,
) -> PlatformResult<()> {
    if !destination.is_absolute() || destination.file_name().is_none() {
        return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| PlatformError::new(PlatformErrorCode::InvalidInput))?;
    let canonical_parent = std::fs::canonicalize(parent)
        .map_err(|_| PlatformError::new(PlatformErrorCode::SelectionFailed))?;
    let canonical_data_root = std::fs::canonicalize(data_root)
        .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
    if canonical_parent.starts_with(&canonical_data_root) {
        return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
    }
    Ok(())
}

#[cfg(any(mobile, target_os = "macos", windows, test))]
pub(crate) fn hash_regular_file(path: &Path) -> PlatformResult<(String, u64)> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    let mut file = options
        .open(path)
        .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
    let metadata = file
        .metadata()
        .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
    if !metadata.is_file() {
        return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
    }

    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
    let mut total = 0_u64;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(u64::try_from(read).unwrap_or(u64::MAX))
            .ok_or_else(|| PlatformError::new(PlatformErrorCode::InvalidInput))?;
        hasher.update(&buffer[..read]);
    }
    Ok((format!("{:x}", hasher.finalize()), total))
}

#[cfg(any(mobile, target_os = "macos", windows, test))]
fn expected_content_source_path(canonical_data_root: &Path, sha256: &str) -> PathBuf {
    canonical_data_root
        .join("sources/sha256")
        .join(&sha256[..2])
        .join(&sha256[2..])
}

#[cfg(any(target_os = "macos", windows, test))]
pub(crate) fn sanitize_display_name(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|character| {
            if character.is_control() {
                '\u{fffd}'
            } else {
                character
            }
        })
        .take(255)
        .collect();
    if cleaned.trim().is_empty() {
        "selected-file".to_owned()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAXIMUM_SENSITIVE_CAPTURE_BYTES, sanitize_display_name,
        validate_content_export_destination, validate_credential_read, validate_credential_write,
        validate_export_receipt_display_name, validate_export_sha256,
        validate_export_suggested_name, validate_reference, validate_sensitive_capture,
        verify_content_source_for_export,
    };
    use sha2::{Digest, Sha256};
    use tempfile::tempdir;

    #[test]
    fn rejects_empty_and_oversized_sensitive_inputs() {
        assert!(validate_reference("").is_err());
        assert!(validate_reference(&"r".repeat(257)).is_err());
        assert!(validate_credential_write("  ").is_err());
        assert!(validate_credential_write(&"s".repeat(16 * 1024)).is_ok());
        assert!(validate_credential_write(&"s".repeat(16 * 1024 + 1)).is_err());
        assert!(validate_credential_read(&"s".repeat(32 * 1024)).is_ok());
        assert!(validate_credential_read(&"s".repeat(32 * 1024 + 1)).is_err());
        assert!(validate_sensitive_capture("curl https://example.test", 1024).is_ok());
        assert!(validate_sensitive_capture("", 1024).is_err());
        assert!(validate_sensitive_capture("x", 0).is_err());
        assert!(validate_sensitive_capture("x", MAXIMUM_SENSITIVE_CAPTURE_BYTES + 1).is_err());
    }

    #[test]
    fn display_name_drops_control_characters_and_is_bounded() {
        let output = sanitize_display_name(&format!("a\u{0}{}", "b".repeat(300)));
        assert!(!output.contains('\u{0}'));
        assert_eq!(output.chars().count(), 255);
    }

    #[test]
    fn export_identity_and_names_are_strict_and_portable() {
        assert!(validate_export_sha256(&"a".repeat(64)).is_ok());
        assert!(validate_export_sha256(&"A".repeat(64)).is_err());
        assert!(validate_export_sha256(&"a".repeat(63)).is_err());
        assert!(validate_export_suggested_name("lorepia-character-a.json").is_ok());
        assert!(validate_export_suggested_name("../card.json").is_err());
        assert!(validate_export_suggested_name("card name.json").is_err());
        assert!(validate_export_suggested_name("NUL.json").is_err());
        assert!(validate_export_receipt_display_name("캐릭터.json").is_ok());
        assert!(validate_export_receipt_display_name("folder/card.json").is_err());
    }

    #[test]
    fn export_source_must_be_the_exact_verified_cas_object() {
        let root = tempdir().expect("root");
        let data_root = root.path().join("data");
        std::fs::create_dir(&data_root).expect("data root");
        let bytes = b"synthetic-content-source";
        let digest = format!("{:x}", Sha256::digest(bytes));
        let source = data_root
            .join("sources/sha256")
            .join(&digest[..2])
            .join(&digest[2..]);
        std::fs::create_dir_all(source.parent().expect("source parent")).expect("cas parents");
        std::fs::write(&source, bytes).expect("source");

        verify_content_source_for_export(
            &source,
            &data_root,
            bytes.len() as u64,
            &digest,
            "character.json",
        )
        .expect("verified source");
        assert!(
            verify_content_source_for_export(
                &source,
                &data_root,
                bytes.len() as u64 + 1,
                &digest,
                "character.json",
            )
            .is_err()
        );

        let outside = root.path().join("outside");
        std::fs::write(&outside, bytes).expect("outside");
        assert!(
            verify_content_source_for_export(
                &outside,
                &data_root,
                bytes.len() as u64,
                &digest,
                "character.json",
            )
            .is_err()
        );
    }

    #[test]
    fn export_destination_cannot_overwrite_application_data() {
        let root = tempdir().expect("root");
        let data_root = root.path().join("data");
        let external_root = root.path().join("exports");
        std::fs::create_dir_all(data_root.join("sources")).expect("data root");
        std::fs::create_dir(&external_root).expect("external root");

        assert!(
            validate_content_export_destination(&external_root.join("character.json"), &data_root,)
                .is_ok()
        );
        assert!(
            validate_content_export_destination(&data_root.join("database.sqlite3"), &data_root)
                .is_err()
        );
        assert!(
            validate_content_export_destination(
                &data_root.join("sources/other-cas-object"),
                &data_root,
            )
            .is_err()
        );

        #[cfg(unix)]
        {
            let linked_data_root = root.path().join("linked-data-root");
            std::os::unix::fs::symlink(&data_root, &linked_data_root)
                .expect("link to application data root");
            assert!(
                validate_content_export_destination(
                    &linked_data_root.join("database.sqlite3"),
                    &data_root,
                )
                .is_err()
            );
        }
    }
}
