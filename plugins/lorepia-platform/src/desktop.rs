use std::path::{Path, PathBuf};

#[cfg(any(windows, test))]
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, Write},
};

#[cfg(any(target_os = "macos", windows))]
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(any(target_os = "macos", windows, test))]
use std::time::{Duration, SystemTime};

#[cfg(any(target_os = "macos", windows))]
use tauri::Manager;
use tauri::{AppHandle, Runtime};

#[cfg(any(windows, test))]
use sha2::{Digest, Sha256};
#[cfg(any(windows, test))]
use uuid::{Uuid, Version};

use crate::{
    CredentialStatus, NativeCaptureStatus, NativeCredential, NativeCredentialEffect,
    NativeCredentialEffectContext, NativeSensitiveText, PlatformError, PlatformErrorCode,
    PlatformResult, StagedImport,
    model::NativeSavedContentSource,
    validation::{MAXIMUM_CREDENTIAL_WRITE_BYTES, validate_reference},
};

#[cfg(any(target_os = "macos", windows))]
use crate::validation::{
    validate_content_export_destination, validate_export_receipt_display_name,
    validate_sensitive_capture, verify_content_source_for_export,
};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
pub(crate) mod windows;

pub(crate) struct DesktopPlatform<R: Runtime> {
    #[cfg(any(target_os = "macos", windows))]
    app: AppHandle<R>,
    #[cfg(not(any(target_os = "macos", windows)))]
    _runtime: std::marker::PhantomData<fn() -> R>,
    data_root: PathBuf,
    staging_root: PathBuf,
    #[cfg(any(target_os = "macos", windows))]
    export_in_flight: AtomicBool,
    #[cfg(any(target_os = "macos", windows))]
    sensitive_capture_in_flight: AtomicBool,
    #[cfg(any(target_os = "macos", windows))]
    credential_namespace: &'static str,
    #[cfg(target_os = "macos")]
    migrate_legacy_credentials: bool,
}

#[cfg(any(windows, test))]
const WINDOWS_BOUND_LOCATOR_DIRECTORY: &str = "credential-locators-v1";
#[cfg(any(windows, test))]
const WINDOWS_BOUND_LOCATOR_MAGIC: &str = "lorepia-windows-bound-credential-locator\nv1\n";
#[cfg(any(windows, test))]
const WINDOWS_BOUND_LOCATOR_V2_MAGIC: &str = "lorepia-windows-bound-credential-locator\nv2\n";
#[cfg(any(windows, test))]
const WINDOWS_BOUND_LOCATOR_HASH_DOMAIN: &[u8] =
    b"dev.lorepia.windows-bound-credential-locator.path.v1\0";
#[cfg(any(windows, test))]
const WINDOWS_BOUND_LOCATOR_CHECKSUM_DOMAIN: &[u8] =
    b"dev.lorepia.windows-bound-credential-locator.record.v1\0";
#[cfg(any(windows, test))]
const WINDOWS_BOUND_LOCATOR_V2_CHECKSUM_DOMAIN: &[u8] =
    b"dev.lorepia.windows-bound-credential-locator.record.v2\0";
#[cfg(any(windows, test))]
const WINDOWS_BOUND_DELETE_INTENT_MAGIC: &str =
    "lorepia-windows-bound-credential-delete-intent\nv1\n";
#[cfg(any(windows, test))]
const WINDOWS_BOUND_DELETE_INTENT_CHECKSUM_DOMAIN: &[u8] =
    b"dev.lorepia.windows-bound-credential-delete-intent.record.v1\0";
#[cfg(any(windows, test))]
const WINDOWS_BOUND_DELETE_COMPLETE_MAGIC: &str =
    "lorepia-windows-bound-credential-delete-complete\nv1\n";
#[cfg(any(windows, test))]
const WINDOWS_BOUND_DELETE_COMPLETE_CHECKSUM_DOMAIN: &[u8] =
    b"dev.lorepia.windows-bound-credential-delete-complete.record.v1\0";
#[cfg(any(windows, test))]
const WINDOWS_BOUND_DELETE_INTENT_V2_MAGIC: &str =
    "lorepia-windows-bound-credential-delete-intent\nv2\n";
#[cfg(any(windows, test))]
const WINDOWS_BOUND_DELETE_INTENT_V2_CHECKSUM_DOMAIN: &[u8] =
    b"dev.lorepia.windows-bound-credential-delete-intent.record.v2\0";
#[cfg(any(windows, test))]
const WINDOWS_BOUND_DELETE_COMPLETE_V2_MAGIC: &str =
    "lorepia-windows-bound-credential-delete-complete\nv2\n";
#[cfg(any(windows, test))]
const WINDOWS_BOUND_DELETE_COMPLETE_V2_CHECKSUM_DOMAIN: &[u8] =
    b"dev.lorepia.windows-bound-credential-delete-complete.record.v2\0";
#[cfg(any(windows, test))]
const WINDOWS_BOUND_MUTEX_HASH_DOMAIN: &[u8] =
    b"dev.lorepia.windows-bound-credential.operation-lock.v1\0";
#[cfg(any(windows, test))]
const WINDOWS_BOUND_USERNAME_PREFIX: &str = "lpcw1-";
#[cfg(any(windows, test))]
const WINDOWS_BOUND_GENERATION_PREFIX: &str = "lpcg1-";
#[cfg(any(windows, test))]
const WINDOWS_BOUND_VALUE_MAGIC: &str = "lorepia-windows-bound-value\nv1\n";
#[cfg(any(windows, test))]
const WINDOWS_BOUND_RECORD_MAXIMUM_BYTES: usize = 640;
#[cfg(any(windows, test))]
const WINDOWS_BOUND_FILE_BACKEND: &str = "dpapi-current-user-file-v1";
#[cfg(any(windows, test))]
const WINDOWS_BOUND_FILE_SLOT_PREFIX: &str = "lpcf1-";
#[cfg(any(windows, test))]
const WINDOWS_BOUND_FILE_PATH_DOMAIN: &[u8] =
    b"dev.lorepia.windows-bound-credential.file.path.v2\0";
#[cfg(any(windows, test))]
const WINDOWS_BOUND_FILE_RECORD_DOMAIN: &[u8] =
    b"dev.lorepia.windows-bound-credential.file.record.v2\0";
#[cfg(any(windows, test))]
const WINDOWS_BOUND_FILE_MAGIC: &[u8] = b"lorepia-windows-bound-credential-file\nv2\n";
#[cfg(any(windows, test))]
const WINDOWS_BOUND_FILE_PLAINTEXT_MAGIC: &[u8] = b"lorepia-windows-bound-file-plaintext\nv2\n";
#[cfg(any(windows, test))]
const WINDOWS_BOUND_FILE_ENTROPY_DOMAIN: &[u8] =
    b"dev.lorepia.windows-bound-credential.dpapi-entropy.v2\0";
#[cfg(any(windows, test))]
const WINDOWS_BOUND_FILE_MAXIMUM_BYTES: usize = 64 * 1024;
#[cfg(any(windows, test))]
const WINDOWS_BOUND_FILE_STAGE_PREFIX: &str = "lpcw-credential-stage-v2-";
#[cfg(any(windows, test))]
const WINDOWS_BOUND_FILE_STAGE_SUFFIX: &str = ".tmp";
#[cfg(any(windows, test))]
const WINDOWS_BOUND_FILE_STAGE_MINIMUM_AGE: Duration = Duration::from_hours(24);
#[cfg(any(windows, test))]
const WINDOWS_BOUND_FILE_STAGE_MAXIMUM_SCAN: usize = 256;
#[cfg(any(windows, test))]
const WINDOWS_BOUND_FILE_STAGE_MAXIMUM_DELETE: usize = 32;
#[cfg(any(windows, test))]
const WINDOWS_BOUND_PHYSICAL_REFERENCE_PREFIX: &str = "lpc2-";

#[cfg(any(windows, test))]
#[derive(Debug)]
struct WindowsBoundCredentialLocatorClaim {
    _exclusive_file: File,
}

#[cfg(all(test, not(windows)))]
struct WindowsBoundCredentialOperationGuard {
    _guard: std::sync::MutexGuard<'static, ()>,
}

#[cfg(all(test, not(windows)))]
static WINDOWS_BOUND_CREDENTIAL_OPERATION_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(any(windows, test))]
#[derive(Clone, Debug, PartialEq, Eq)]
struct WindowsBoundCredentialClaim {
    username: String,
    generation: String,
    file_record_sha256: Option<String>,
}

#[cfg(any(windows, test))]
#[derive(Clone, Debug, PartialEq, Eq)]
struct WindowsBoundCredentialFileSeed {
    slot: String,
    generation: String,
}

#[cfg(any(windows, test))]
#[derive(Clone, Debug, PartialEq, Eq)]
enum WindowsBoundDeleteTarget {
    Claimed(WindowsBoundCredentialClaim),
    Legacy,
}

#[cfg(any(windows, test))]
enum WindowsBoundCredentialLifecycle {
    Missing,
    Active(WindowsBoundCredentialClaim),
    Deleting(WindowsBoundDeleteTarget),
    Deleted,
}

#[cfg(any(windows, test))]
fn windows_bound_credential_locator_path(data_root: &Path, reference: &str) -> PathBuf {
    windows_bound_credential_record_path(data_root, reference, "lpcw-locator-v1")
}

#[cfg(any(windows, test))]
fn windows_bound_credential_delete_intent_path(data_root: &Path, reference: &str) -> PathBuf {
    windows_bound_credential_record_path(data_root, reference, "lpcw-delete-intent-v1")
}

#[cfg(any(windows, test))]
fn windows_bound_credential_delete_completion_path(data_root: &Path, reference: &str) -> PathBuf {
    windows_bound_credential_record_path(data_root, reference, "lpcw-delete-complete-v1")
}

#[cfg(any(windows, test))]
fn windows_bound_credential_record_path(
    data_root: &Path,
    reference: &str,
    prefix: &str,
) -> PathBuf {
    let mut digest = Sha256::new();
    digest.update(WINDOWS_BOUND_LOCATOR_HASH_DOMAIN);
    digest.update((reference.len() as u64).to_be_bytes());
    digest.update(reference.as_bytes());
    data_root
        .join(WINDOWS_BOUND_LOCATOR_DIRECTORY)
        .join(format!("{prefix}-{:x}.record", digest.finalize()))
}

#[cfg(any(windows, test))]
fn windows_bound_credential_mutex_name(
    data_root: &Path,
    reference: &str,
) -> PlatformResult<String> {
    validate_reference(reference)?;
    if !data_root.is_absolute() {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    validate_windows_locator_directory(data_root)?;
    let canonical_root = std::fs::canonicalize(data_root)
        .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    let mut digest = Sha256::new();
    digest.update(WINDOWS_BOUND_MUTEX_HASH_DOMAIN);
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;

        let root = canonical_root.as_os_str().encode_wide().collect::<Vec<_>>();
        digest.update((root.len() as u64).to_be_bytes());
        for code_unit in root {
            digest.update(code_unit.to_le_bytes());
        }
    }
    #[cfg(not(windows))]
    {
        let root = canonical_root.as_os_str().as_encoded_bytes();
        digest.update((root.len() as u64).to_be_bytes());
        digest.update(root);
    }
    digest.update((reference.len() as u64).to_be_bytes());
    digest.update(reference.as_bytes());
    Ok(format!(
        "Global\\LorePia.ProviderCredential.Lock.v1.{:x}",
        digest.finalize()
    ))
}

#[cfg(windows)]
fn lock_windows_bound_credential_operation(
    data_root: &Path,
    reference: &str,
) -> PlatformResult<windows::BoundCredentialOperationGuard> {
    windows::lock_bound_credential_operation(&windows_bound_credential_mutex_name(
        data_root, reference,
    )?)
}

#[cfg(all(test, not(windows)))]
fn lock_windows_bound_credential_operation(
    data_root: &Path,
    reference: &str,
) -> PlatformResult<WindowsBoundCredentialOperationGuard> {
    let _name = windows_bound_credential_mutex_name(data_root, reference)?;
    WINDOWS_BOUND_CREDENTIAL_OPERATION_MUTEX
        .lock()
        .map(|guard| WindowsBoundCredentialOperationGuard { _guard: guard })
        .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))
}

#[cfg(any(windows, test))]
fn new_windows_bound_credential_file_seed() -> WindowsBoundCredentialFileSeed {
    WindowsBoundCredentialFileSeed {
        slot: format!("{WINDOWS_BOUND_FILE_SLOT_PREFIX}{}", Uuid::new_v4()),
        generation: format!("{WINDOWS_BOUND_GENERATION_PREFIX}{}", Uuid::new_v4()),
    }
}

#[cfg(any(windows, test))]
fn validate_windows_bound_credential_file_seed(
    seed: &WindowsBoundCredentialFileSeed,
) -> PlatformResult<()> {
    validate_windows_bound_uuid(&seed.slot, WINDOWS_BOUND_FILE_SLOT_PREFIX)?;
    validate_windows_bound_credential_generation(&seed.generation)
}

#[cfg(any(windows, test))]
fn validate_windows_bound_credential_claim(
    claim: &WindowsBoundCredentialClaim,
) -> PlatformResult<()> {
    match claim.file_record_sha256.as_deref() {
        None => validate_windows_bound_credential_username(&claim.username)?,
        Some(digest) => {
            validate_windows_bound_uuid(&claim.username, WINDOWS_BOUND_FILE_SLOT_PREFIX)?;
            validate_windows_bound_sha256(digest)?;
        }
    }
    validate_windows_bound_credential_generation(&claim.generation)
}

#[cfg(any(windows, test))]
fn validate_windows_bound_sha256(value: &str) -> PlatformResult<()> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ))
    }
}

#[cfg(any(windows, test))]
fn validate_windows_raw_credential_reference(reference: &str) -> PlatformResult<()> {
    validate_reference(reference)?;
    let reserved_bound_physical_reference = reference
        .strip_prefix(WINDOWS_BOUND_PHYSICAL_REFERENCE_PREFIX)
        .is_some_and(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        });
    if reserved_bound_physical_reference {
        Err(PlatformError::new(PlatformErrorCode::InvalidInput))
    } else {
        Ok(())
    }
}

#[cfg(any(windows, test))]
fn with_validated_windows_raw_credential_reference<T>(
    reference: &str,
    operation: impl FnOnce() -> PlatformResult<T>,
) -> PlatformResult<T> {
    validate_windows_raw_credential_reference(reference)?;
    operation()
}

#[cfg(any(windows, test))]
fn validate_windows_bound_credential_username(username: &str) -> PlatformResult<()> {
    validate_windows_bound_uuid(username, WINDOWS_BOUND_USERNAME_PREFIX)
}

#[cfg(any(windows, test))]
fn validate_windows_bound_credential_generation(generation: &str) -> PlatformResult<()> {
    validate_windows_bound_uuid(generation, WINDOWS_BOUND_GENERATION_PREFIX)
}

#[cfg(any(windows, test))]
fn validate_windows_bound_uuid(value: &str, prefix: &str) -> PlatformResult<()> {
    let Some(uuid_text) = value.strip_prefix(prefix) else {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    };
    let uuid = Uuid::parse_str(uuid_text)
        .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    if uuid.get_version() == Some(Version::Random) && uuid.hyphenated().to_string() == uuid_text {
        Ok(())
    } else {
        Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ))
    }
}

#[cfg(any(windows, test))]
fn encode_windows_bound_credential_locator_for_claim(
    reference: &str,
    claim: &WindowsBoundCredentialClaim,
) -> Vec<u8> {
    validate_windows_bound_credential_claim(claim).expect("validated Windows credential claim");
    let (magic, domain, payload) = match claim.file_record_sha256.as_deref() {
        None => (
            WINDOWS_BOUND_LOCATOR_MAGIC,
            WINDOWS_BOUND_LOCATOR_CHECKSUM_DOMAIN,
            format!(
                "{WINDOWS_BOUND_LOCATOR_MAGIC}{reference}\n{}\n{}\n",
                claim.username, claim.generation
            ),
        ),
        Some(record_sha256) => (
            WINDOWS_BOUND_LOCATOR_V2_MAGIC,
            WINDOWS_BOUND_LOCATOR_V2_CHECKSUM_DOMAIN,
            format!(
                "{WINDOWS_BOUND_LOCATOR_V2_MAGIC}{reference}\n{WINDOWS_BOUND_FILE_BACKEND}\n{}\n{}\n{record_sha256}\n",
                claim.username, claim.generation
            ),
        ),
    };
    debug_assert!(payload.starts_with(magic));
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update(payload.as_bytes());
    format!("{payload}{:x}\n", digest.finalize()).into_bytes()
}

#[cfg(any(windows, test))]
fn encode_windows_bound_delete_record(
    magic: &str,
    checksum_domain: &[u8],
    reference: &str,
    target: &WindowsBoundDeleteTarget,
) -> Vec<u8> {
    let (kind, username, generation, file_record_sha256) = match target {
        WindowsBoundDeleteTarget::Claimed(claim) => match claim.file_record_sha256.as_deref() {
            Some(record_sha256) => (
                WINDOWS_BOUND_FILE_BACKEND,
                claim.username.as_str(),
                claim.generation.as_str(),
                record_sha256,
            ),
            None => (
                "claimed",
                claim.username.as_str(),
                claim.generation.as_str(),
                "-",
            ),
        },
        WindowsBoundDeleteTarget::Legacy => ("legacy", "-", "-", "-"),
    };
    let payload = if magic.ends_with("v2\n") {
        format!("{magic}{reference}\n{kind}\n{username}\n{generation}\n{file_record_sha256}\n")
    } else {
        debug_assert_eq!(file_record_sha256, "-");
        format!("{magic}{reference}\n{kind}\n{username}\n{generation}\n")
    };
    let mut digest = Sha256::new();
    digest.update(checksum_domain);
    digest.update(payload.as_bytes());
    format!("{payload}{:x}\n", digest.finalize()).into_bytes()
}

#[cfg(any(windows, test))]
fn decode_windows_bound_credential_locator(
    bytes: &[u8],
    expected_reference: &str,
) -> PlatformResult<WindowsBoundCredentialClaim> {
    if bytes.is_empty() || bytes.len() > WINDOWS_BOUND_RECORD_MAXIMUM_BYTES {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    if let Some(without_magic) = text.strip_prefix(WINDOWS_BOUND_LOCATOR_V2_MAGIC) {
        return decode_windows_bound_credential_locator_v2(without_magic, expected_reference);
    }
    let Some(without_magic) = text.strip_prefix(WINDOWS_BOUND_LOCATOR_MAGIC) else {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    };
    let mut lines = without_magic.split('\n');
    let (Some(reference), Some(username), Some(generation), Some(checksum), Some("")) = (
        lines.next(),
        lines.next(),
        lines.next(),
        lines.next(),
        lines.next(),
    ) else {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    };
    if lines.next().is_some() || reference != expected_reference || checksum.len() != 64 {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    if !checksum
        .as_bytes()
        .iter()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    validate_windows_bound_credential_username(username)?;
    validate_windows_bound_credential_generation(generation)?;
    let payload = format!("{WINDOWS_BOUND_LOCATOR_MAGIC}{reference}\n{username}\n{generation}\n");
    let mut digest = Sha256::new();
    digest.update(WINDOWS_BOUND_LOCATOR_CHECKSUM_DOMAIN);
    digest.update(payload.as_bytes());
    let expected_checksum = format!("{:x}", digest.finalize());
    if checksum != expected_checksum {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    Ok(WindowsBoundCredentialClaim {
        username: username.to_owned(),
        generation: generation.to_owned(),
        file_record_sha256: None,
    })
}

#[cfg(any(windows, test))]
fn decode_windows_bound_credential_locator_v2(
    without_magic: &str,
    expected_reference: &str,
) -> PlatformResult<WindowsBoundCredentialClaim> {
    let mut lines = without_magic.split('\n');
    let (
        Some(reference),
        Some(backend),
        Some(slot),
        Some(generation),
        Some(record_sha256),
        Some(checksum),
        Some(""),
    ) = (
        lines.next(),
        lines.next(),
        lines.next(),
        lines.next(),
        lines.next(),
        lines.next(),
        lines.next(),
    )
    else {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    };
    if lines.next().is_some()
        || reference != expected_reference
        || backend != WINDOWS_BOUND_FILE_BACKEND
    {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    let claim = WindowsBoundCredentialClaim {
        username: slot.to_owned(),
        generation: generation.to_owned(),
        file_record_sha256: Some(record_sha256.to_owned()),
    };
    validate_windows_bound_credential_claim(&claim)?;
    validate_windows_bound_sha256(checksum)?;
    let payload = format!(
        "{WINDOWS_BOUND_LOCATOR_V2_MAGIC}{reference}\n{backend}\n{slot}\n{generation}\n{record_sha256}\n"
    );
    let mut digest = Sha256::new();
    digest.update(WINDOWS_BOUND_LOCATOR_V2_CHECKSUM_DOMAIN);
    digest.update(payload.as_bytes());
    if checksum != format!("{:x}", digest.finalize()) {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    Ok(claim)
}

#[cfg(any(windows, test))]
fn decode_windows_bound_delete_record(
    bytes: &[u8],
    magic: &str,
    checksum_domain: &[u8],
    expected_reference: &str,
) -> PlatformResult<WindowsBoundDeleteTarget> {
    if bytes.is_empty() || bytes.len() > WINDOWS_BOUND_RECORD_MAXIMUM_BYTES {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    let Some(without_magic) = text.strip_prefix(magic) else {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    };
    let mut lines = without_magic.split('\n');
    let (Some(reference), Some(kind), Some(username), Some(generation), Some(checksum), Some("")) = (
        lines.next(),
        lines.next(),
        lines.next(),
        lines.next(),
        lines.next(),
        lines.next(),
    ) else {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    };
    if lines.next().is_some()
        || reference != expected_reference
        || checksum.len() != 64
        || !checksum
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    let target = match (kind, username, generation) {
        ("claimed", username, generation) => {
            validate_windows_bound_credential_username(username)?;
            validate_windows_bound_credential_generation(generation)?;
            WindowsBoundDeleteTarget::Claimed(WindowsBoundCredentialClaim {
                username: username.to_owned(),
                generation: generation.to_owned(),
                file_record_sha256: None,
            })
        }
        ("legacy", "-", "-") => WindowsBoundDeleteTarget::Legacy,
        _ => {
            return Err(PlatformError::new(
                PlatformErrorCode::CredentialRecoveryRequired,
            ));
        }
    };
    let payload = format!("{magic}{reference}\n{kind}\n{username}\n{generation}\n");
    let mut digest = Sha256::new();
    digest.update(checksum_domain);
    digest.update(payload.as_bytes());
    if checksum != format!("{:x}", digest.finalize()) {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    Ok(target)
}

#[cfg(any(windows, test))]
fn decode_windows_bound_delete_record_v2(
    bytes: &[u8],
    magic: &str,
    checksum_domain: &[u8],
    expected_reference: &str,
) -> PlatformResult<WindowsBoundDeleteTarget> {
    if bytes.is_empty() || bytes.len() > WINDOWS_BOUND_RECORD_MAXIMUM_BYTES {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    let without_magic = text
        .strip_prefix(magic)
        .ok_or_else(|| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    let mut lines = without_magic.split('\n');
    let (
        Some(reference),
        Some(kind),
        Some(username),
        Some(generation),
        Some(record_sha256),
        Some(checksum),
        Some(""),
    ) = (
        lines.next(),
        lines.next(),
        lines.next(),
        lines.next(),
        lines.next(),
        lines.next(),
        lines.next(),
    )
    else {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    };
    if lines.next().is_some() || reference != expected_reference {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    let target = match (kind, username, generation, record_sha256) {
        (WINDOWS_BOUND_FILE_BACKEND, username, generation, digest) => {
            let claim = WindowsBoundCredentialClaim {
                username: username.to_owned(),
                generation: generation.to_owned(),
                file_record_sha256: Some(digest.to_owned()),
            };
            validate_windows_bound_credential_claim(&claim)?;
            WindowsBoundDeleteTarget::Claimed(claim)
        }
        ("legacy", "-", "-", "-") => WindowsBoundDeleteTarget::Legacy,
        _ => {
            return Err(PlatformError::new(
                PlatformErrorCode::CredentialRecoveryRequired,
            ));
        }
    };
    validate_windows_bound_sha256(checksum)?;
    let payload =
        format!("{magic}{reference}\n{kind}\n{username}\n{generation}\n{record_sha256}\n");
    let mut digest = Sha256::new();
    digest.update(checksum_domain);
    digest.update(payload.as_bytes());
    if checksum != format!("{:x}", digest.finalize()) {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    Ok(target)
}

#[cfg(test)]
fn encode_windows_bound_credential_value(
    generation: &str,
    value: &NativeCredential,
) -> PlatformResult<NativeCredential> {
    validate_windows_bound_credential_generation(generation)?;
    validate_windows_bound_credential_value_size(value)?;
    let encoded = zeroize::Zeroizing::new(format!(
        "{WINDOWS_BOUND_VALUE_MAGIC}{generation}\n{}\n{}",
        value.expose().len(),
        value.expose()
    ));
    crate::validation::validate_credential_read(encoded.as_str())
        .map_err(|_| PlatformError::new(PlatformErrorCode::InvalidInput))?;
    Ok(NativeCredential::from_zeroizing(encoded))
}

#[cfg(any(windows, test))]
fn validate_windows_bound_credential_value_size(value: &NativeCredential) -> PlatformResult<()> {
    crate::validation::validate_credential_write(value.expose())?;
    let maximum_encoded = WINDOWS_BOUND_VALUE_MAGIC.len()
        + WINDOWS_BOUND_GENERATION_PREFIX.len()
        + 36
        + 1
        + usize::MAX.to_string().len()
        + 1
        + value.expose().len();
    if maximum_encoded <= crate::validation::MAXIMUM_CREDENTIAL_READ_BYTES {
        Ok(())
    } else {
        Err(PlatformError::new(PlatformErrorCode::InvalidInput))
    }
}

#[cfg(any(windows, test))]
fn decode_windows_bound_credential_value(
    expected_generation: &str,
    value: NativeCredential,
) -> PlatformResult<NativeCredential> {
    validate_windows_bound_credential_generation(expected_generation)?;
    let value = zeroize::Zeroizing::new(value.into_secret_string());
    crate::validation::validate_credential_read(value.as_str())
        .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    let Some(rest) = value.strip_prefix(WINDOWS_BOUND_VALUE_MAGIC) else {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    };
    let mut parts = rest.splitn(3, '\n');
    let (Some(generation), Some(length_text), Some(inner)) =
        (parts.next(), parts.next(), parts.next())
    else {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    };
    let length = length_text
        .parse::<usize>()
        .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    if generation != expected_generation
        || length_text != length.to_string()
        || inner.len() != length
        || crate::validation::validate_credential_write(inner).is_err()
    {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    Ok(NativeCredential::new(inner.to_owned()))
}

#[cfg(any(windows, test))]
fn append_windows_bound_frame(output: &mut Vec<u8>, bytes: &[u8]) {
    output.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    output.extend_from_slice(bytes);
}

#[cfg(any(windows, test))]
fn take_windows_bound_frame<'a>(input: &mut &'a [u8]) -> PlatformResult<&'a [u8]> {
    let length_bytes = input
        .get(..8)
        .ok_or_else(|| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    let length = usize::try_from(u64::from_be_bytes(
        length_bytes.try_into().expect("an exact eight-byte prefix"),
    ))
    .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    let frame = input
        .get(8..8_usize.saturating_add(length))
        .ok_or_else(|| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    *input = input
        .get(8_usize.saturating_add(length)..)
        .ok_or_else(|| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    Ok(frame)
}

#[cfg(any(windows, test))]
fn windows_bound_file_record_digest(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(WINDOWS_BOUND_FILE_RECORD_DOMAIN);
    digest.update((bytes.len() as u64).to_be_bytes());
    digest.update(bytes);
    format!("{:x}", digest.finalize())
}

#[cfg(any(windows, test))]
fn windows_bound_credential_file_path(
    data_root: &Path,
    reference: &str,
    slot: &str,
    generation: &str,
) -> PlatformResult<PathBuf> {
    validate_reference(reference)?;
    validate_windows_bound_uuid(slot, WINDOWS_BOUND_FILE_SLOT_PREFIX)?;
    validate_windows_bound_credential_generation(generation)?;
    let mut digest = Sha256::new();
    digest.update(WINDOWS_BOUND_FILE_PATH_DOMAIN);
    for component in [reference.as_bytes(), slot.as_bytes(), generation.as_bytes()] {
        digest.update((component.len() as u64).to_be_bytes());
        digest.update(component);
    }
    Ok(data_root
        .join(WINDOWS_BOUND_LOCATOR_DIRECTORY)
        .join(format!("lpcw-credential-v2-{:x}.blob", digest.finalize())))
}

#[cfg(any(windows, test))]
fn encode_windows_bound_file_plaintext(
    resource: &str,
    reference: &str,
    seed: &WindowsBoundCredentialFileSeed,
    value: &NativeCredential,
) -> PlatformResult<zeroize::Zeroizing<Vec<u8>>> {
    validate_reference(resource)?;
    validate_reference(reference)?;
    validate_windows_bound_credential_file_seed(seed)?;
    validate_windows_bound_credential_value_size(value)?;
    let mut encoded = zeroize::Zeroizing::new(Vec::with_capacity(
        WINDOWS_BOUND_FILE_PLAINTEXT_MAGIC.len() + value.expose().len() + 256,
    ));
    encoded.extend_from_slice(WINDOWS_BOUND_FILE_PLAINTEXT_MAGIC);
    for component in [
        resource.as_bytes(),
        reference.as_bytes(),
        seed.slot.as_bytes(),
        seed.generation.as_bytes(),
        value.expose().as_bytes(),
    ] {
        append_windows_bound_frame(&mut encoded, component);
    }
    if encoded.len() > crate::validation::MAXIMUM_CREDENTIAL_READ_BYTES {
        return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
    }
    Ok(encoded)
}

#[cfg(any(windows, test))]
fn decode_windows_bound_file_plaintext(
    resource: &str,
    reference: &str,
    claim: &WindowsBoundCredentialClaim,
    plaintext: zeroize::Zeroizing<Vec<u8>>,
) -> PlatformResult<NativeCredential> {
    validate_windows_bound_credential_claim(claim)?;
    let Some(mut remaining) = plaintext.strip_prefix(WINDOWS_BOUND_FILE_PLAINTEXT_MAGIC) else {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    };
    let expected = [
        resource.as_bytes(),
        reference.as_bytes(),
        claim.username.as_bytes(),
        claim.generation.as_bytes(),
    ];
    for expected in expected {
        if take_windows_bound_frame(&mut remaining)? != expected {
            return Err(PlatformError::new(
                PlatformErrorCode::CredentialRecoveryRequired,
            ));
        }
    }
    let envelope = take_windows_bound_frame(&mut remaining)?;
    if !remaining.is_empty() {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    let envelope = std::str::from_utf8(envelope)
        .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    crate::validation::validate_credential_write(envelope)
        .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    Ok(NativeCredential::new(envelope.to_owned()))
}

#[cfg(any(windows, test))]
fn encode_windows_bound_credential_file_record(
    reference: &str,
    seed: &WindowsBoundCredentialFileSeed,
    ciphertext: &[u8],
) -> PlatformResult<(WindowsBoundCredentialClaim, Vec<u8>)> {
    validate_reference(reference)?;
    validate_windows_bound_credential_file_seed(seed)?;
    if ciphertext.is_empty() || ciphertext.len() > WINDOWS_BOUND_FILE_MAXIMUM_BYTES {
        return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
    }
    let mut record = Vec::with_capacity(WINDOWS_BOUND_FILE_MAGIC.len() + ciphertext.len() + 256);
    record.extend_from_slice(WINDOWS_BOUND_FILE_MAGIC);
    for component in [
        reference.as_bytes(),
        seed.slot.as_bytes(),
        seed.generation.as_bytes(),
        ciphertext,
    ] {
        append_windows_bound_frame(&mut record, component);
    }
    if record.len() > WINDOWS_BOUND_FILE_MAXIMUM_BYTES {
        return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
    }
    let claim = WindowsBoundCredentialClaim {
        username: seed.slot.clone(),
        generation: seed.generation.clone(),
        file_record_sha256: Some(windows_bound_file_record_digest(&record)),
    };
    validate_windows_bound_credential_claim(&claim)?;
    Ok((claim, record))
}

#[cfg(any(windows, test))]
fn decode_windows_bound_credential_file_record(
    reference: &str,
    claim: &WindowsBoundCredentialClaim,
    bytes: &[u8],
) -> PlatformResult<zeroize::Zeroizing<Vec<u8>>> {
    validate_reference(reference)?;
    validate_windows_bound_credential_claim(claim)?;
    if bytes.is_empty()
        || bytes.len() > WINDOWS_BOUND_FILE_MAXIMUM_BYTES
        || claim.file_record_sha256.as_deref() != Some(&windows_bound_file_record_digest(bytes))
    {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    let Some(mut remaining) = bytes.strip_prefix(WINDOWS_BOUND_FILE_MAGIC) else {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    };
    for expected in [
        reference.as_bytes(),
        claim.username.as_bytes(),
        claim.generation.as_bytes(),
    ] {
        if take_windows_bound_frame(&mut remaining)? != expected {
            return Err(PlatformError::new(
                PlatformErrorCode::CredentialRecoveryRequired,
            ));
        }
    }
    let ciphertext = take_windows_bound_frame(&mut remaining)?;
    if ciphertext.is_empty() || !remaining.is_empty() {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    Ok(zeroize::Zeroizing::new(ciphertext.to_vec()))
}

#[cfg(any(windows, test))]
fn windows_bound_file_entropy(
    data_root: &Path,
    resource: &str,
    reference: &str,
    slot: &str,
    generation: &str,
) -> PlatformResult<Vec<u8>> {
    validate_reference(resource)?;
    validate_reference(reference)?;
    validate_windows_bound_uuid(slot, WINDOWS_BOUND_FILE_SLOT_PREFIX)?;
    validate_windows_bound_credential_generation(generation)?;
    validate_windows_locator_directory(data_root)?;
    let canonical_root = std::fs::canonicalize(data_root)
        .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    let mut entropy = Vec::with_capacity(512);
    entropy.extend_from_slice(WINDOWS_BOUND_FILE_ENTROPY_DOMAIN);
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;

        let root = canonical_root.as_os_str().encode_wide().collect::<Vec<_>>();
        entropy.extend_from_slice(&(root.len() as u64).to_be_bytes());
        for unit in root {
            entropy.extend_from_slice(&unit.to_le_bytes());
        }
    }
    #[cfg(not(windows))]
    append_windows_bound_frame(&mut entropy, canonical_root.as_os_str().as_encoded_bytes());
    for component in [
        resource.as_bytes(),
        reference.as_bytes(),
        slot.as_bytes(),
        generation.as_bytes(),
    ] {
        append_windows_bound_frame(&mut entropy, component);
    }
    Ok(entropy)
}

#[cfg(any(windows, test))]
fn prepare_windows_bound_credential_file_with(
    data_root: &Path,
    resource: &str,
    reference: &str,
    seed: &WindowsBoundCredentialFileSeed,
    value: &NativeCredential,
    protect: impl FnOnce(&[u8], &[u8]) -> PlatformResult<zeroize::Zeroizing<Vec<u8>>>,
) -> PlatformResult<(WindowsBoundCredentialClaim, Vec<u8>)> {
    let plaintext = encode_windows_bound_file_plaintext(resource, reference, seed, value)?;
    let entropy =
        windows_bound_file_entropy(data_root, resource, reference, &seed.slot, &seed.generation)?;
    let ciphertext = protect(plaintext.as_slice(), &entropy)
        .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    encode_windows_bound_credential_file_record(reference, seed, ciphertext.as_slice())
}

#[cfg(any(windows, test))]
fn read_windows_bound_credential_file_value_with(
    data_root: &Path,
    resource: &str,
    reference: &str,
    claim: &WindowsBoundCredentialClaim,
    delete_access: bool,
    unprotect: impl FnOnce(&[u8], &[u8], usize) -> PlatformResult<zeroize::Zeroizing<Vec<u8>>>,
) -> PlatformResult<Option<(File, NativeCredential)>> {
    let Some((file, ciphertext)) =
        read_windows_bound_credential_file(data_root, reference, claim, delete_access)?
    else {
        return Ok(None);
    };
    let entropy = windows_bound_file_entropy(
        data_root,
        resource,
        reference,
        &claim.username,
        &claim.generation,
    )?;
    let plaintext = unprotect(
        ciphertext.as_slice(),
        &entropy,
        crate::validation::MAXIMUM_CREDENTIAL_READ_BYTES,
    )
    .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    let value = decode_windows_bound_file_plaintext(resource, reference, claim, plaintext)?;
    Ok(Some((file, value)))
}

#[cfg(any(windows, test))]
fn metadata_is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        metadata.file_attributes()
            & ::windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT.0
            != 0
    }
    #[cfg(not(windows))]
    {
        let _ = metadata;
        false
    }
}

#[cfg(any(windows, test))]
fn validate_windows_locator_directory(path: &Path) -> PlatformResult<()> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    if metadata.is_dir()
        && !metadata.file_type().is_symlink()
        && !metadata_is_reparse_point(&metadata)
    {
        Ok(())
    } else {
        Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ))
    }
}

#[cfg(any(windows, test))]
fn validate_windows_locator_file(metadata: &std::fs::Metadata) -> PlatformResult<()> {
    if metadata.is_file()
        && !metadata.file_type().is_symlink()
        && !metadata_is_reparse_point(metadata)
    {
        Ok(())
    } else {
        Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ))
    }
}

#[cfg(any(windows, test))]
fn stabilize_windows_locator_directory(path: &Path) -> PlatformResult<()> {
    #[cfg(windows)]
    {
        // Windows documents FlushFileBuffers for file (or privileged volume)
        // handles, not directory handles. The locator file itself is opened
        // with WRITE_THROUGH and flushed below; directory-entry durability is
        // therefore gated by the real Windows hosted regression rather than a
        // synthetic directory FlushFileBuffers call that may be unsupported.
        validate_windows_locator_directory(path)
    }
    #[cfg(not(windows))]
    {
        let directory = File::open(path)
            .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
        let metadata = directory
            .metadata()
            .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
        if !metadata.is_dir() || metadata_is_reparse_point(&metadata) {
            return Err(PlatformError::new(
                PlatformErrorCode::CredentialRecoveryRequired,
            ));
        }
        directory
            .sync_all()
            .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))
    }
}

#[cfg(any(windows, test))]
fn ensure_windows_bound_locator_directory(data_root: &Path) -> PlatformResult<PathBuf> {
    if !data_root.is_absolute() {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    validate_windows_locator_directory(data_root)?;
    let locator_root = data_root.join(WINDOWS_BOUND_LOCATOR_DIRECTORY);
    match std::fs::create_dir(&locator_root) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => {
            return Err(PlatformError::new(
                PlatformErrorCode::CredentialRecoveryRequired,
            ));
        }
    }
    validate_windows_locator_directory(&locator_root)?;
    // Every claimant revalidates this parent cutpoint, including the
    // AlreadyExists path. The non-Windows regression harness also fsyncs it;
    // Windows relies on the documented WRITE_THROUGH file create/write and
    // the hosted runtime gate because directory FlushFileBuffers is undefined.
    stabilize_windows_locator_directory(data_root)?;
    Ok(locator_root)
}

#[cfg(any(windows, test))]
fn claim_windows_bound_credential_locator_for_claim(
    data_root: &Path,
    reference: &str,
    claim: &WindowsBoundCredentialClaim,
) -> PlatformResult<WindowsBoundCredentialLocatorClaim> {
    claim_windows_bound_credential_locator_for_claim_with_writer(
        data_root,
        reference,
        claim,
        |file, bytes| file.write_all(bytes).and_then(|()| file.sync_all()),
    )
}

#[cfg(any(windows, test))]
fn claim_windows_bound_credential_locator_for_claim_with_writer(
    data_root: &Path,
    reference: &str,
    claim: &WindowsBoundCredentialClaim,
    write_staged: impl FnOnce(&mut File, &[u8]) -> std::io::Result<()>,
) -> PlatformResult<WindowsBoundCredentialLocatorClaim> {
    validate_reference(reference)?;
    validate_windows_bound_credential_claim(claim)?;
    let path = windows_bound_credential_locator_path(data_root, reference);
    let file = publish_windows_bound_record_with_writer(
        data_root,
        &path,
        &encode_windows_bound_credential_locator_for_claim(reference, claim),
        write_staged,
    )?;
    Ok(WindowsBoundCredentialLocatorClaim {
        _exclusive_file: file,
    })
}

#[cfg(any(windows, test))]
fn publish_windows_bound_record(
    data_root: &Path,
    path: &Path,
    bytes: &[u8],
) -> PlatformResult<File> {
    publish_windows_bound_record_with(
        data_root,
        path,
        bytes,
        |file, bytes| file.write_all(bytes).and_then(|()| file.sync_all()),
        publish_windows_bound_staged_record,
    )
}

#[cfg(any(windows, test))]
fn publish_windows_bound_record_with_writer(
    data_root: &Path,
    path: &Path,
    bytes: &[u8],
    write_staged: impl FnOnce(&mut File, &[u8]) -> std::io::Result<()>,
) -> PlatformResult<File> {
    publish_windows_bound_record_with(
        data_root,
        path,
        bytes,
        write_staged,
        publish_windows_bound_staged_record,
    )
}

#[cfg(any(windows, test))]
fn publish_windows_bound_record_with(
    data_root: &Path,
    path: &Path,
    bytes: &[u8],
    write_staged: impl FnOnce(&mut File, &[u8]) -> std::io::Result<()>,
    publish: impl FnOnce(&Path, &Path) -> PlatformResult<()>,
) -> PlatformResult<File> {
    let locator_root = ensure_windows_bound_locator_directory(data_root)?;
    if path.parent() != Some(locator_root.as_path()) || bytes.is_empty() {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    let staging_path = locator_root.join(format!("lpcw-stage-v1-{}.tmp", Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        options.share_mode(0).custom_flags(
            (::windows::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT
                | ::windows::Win32::Storage::FileSystem::FILE_FLAG_WRITE_THROUGH)
                .0,
        );
    }
    let mut file = options
        .open(&staging_path)
        .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    validate_windows_locator_file(
        &file
            .metadata()
            .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?,
    )?;
    if write_staged(&mut file, bytes).is_err() {
        drop(file);
        // The nonsecret, UUID-named staging file is never authoritative.
        // Leave it in place: closing and then deleting by path would create
        // the same ABA window the immutable final records are designed to
        // avoid. No native credential operation runs before publication.
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    drop(file);
    validate_windows_locator_directory(&locator_root)?;
    stabilize_windows_locator_directory(&locator_root)?;

    if publish(&staging_path, path).is_err() {
        // MoveFileEx may have committed before surfacing an error. Never
        // roll back either path; reconcile only an exact final record.
        let Ok((file, actual)) = read_windows_bound_record_file(path) else {
            return Err(PlatformError::new(
                PlatformErrorCode::CredentialRecoveryRequired,
            ));
        };
        if actual == bytes {
            return Ok(file);
        }
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    stabilize_windows_locator_directory(&locator_root)?;

    let (mut file, actual) = read_windows_bound_record_file(path)?;
    if actual != bytes {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    file.rewind()
        .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    Ok(file)
}

#[cfg(windows)]
fn publish_windows_bound_staged_record(source: &Path, destination: &Path) -> PlatformResult<()> {
    windows::publish_file_no_replace(source, destination)
}

#[cfg(any(windows, test))]
fn publish_windows_bound_credential_file(
    data_root: &Path,
    reference: &str,
    claim: &WindowsBoundCredentialClaim,
    bytes: &[u8],
) -> PlatformResult<()> {
    publish_windows_bound_credential_file_with(
        data_root,
        reference,
        claim,
        bytes,
        publish_windows_bound_staged_record,
    )
}

#[cfg(any(windows, test))]
fn publish_windows_bound_credential_file_with(
    data_root: &Path,
    reference: &str,
    claim: &WindowsBoundCredentialClaim,
    bytes: &[u8],
    publish: impl FnOnce(&Path, &Path) -> PlatformResult<()>,
) -> PlatformResult<()> {
    validate_windows_bound_credential_claim(claim)?;
    if claim.file_record_sha256.as_deref() != Some(&windows_bound_file_record_digest(bytes))
        || bytes.len() > WINDOWS_BOUND_FILE_MAXIMUM_BYTES
    {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    let path = windows_bound_credential_file_path(
        data_root,
        reference,
        &claim.username,
        &claim.generation,
    )?;
    let locator_root = ensure_windows_bound_locator_directory(data_root)?;
    if path.parent() != Some(locator_root.as_path()) {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    let stage = locator_root.join(format!(
        "{WINDOWS_BOUND_FILE_STAGE_PREFIX}{}{WINDOWS_BOUND_FILE_STAGE_SUFFIX}",
        Uuid::new_v4()
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        options.share_mode(0).custom_flags(
            (::windows::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT
                | ::windows::Win32::Storage::FileSystem::FILE_FLAG_WRITE_THROUGH)
                .0,
        );
    }
    let mut file = options
        .open(&stage)
        .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    validate_windows_locator_file(
        &file
            .metadata()
            .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?,
    )?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    drop(file);
    stabilize_windows_locator_directory(&locator_root)?;
    if publish(&stage, &path).is_err() {
        let Ok((_, actual)) =
            read_windows_bound_record_file_with_limit(&path, WINDOWS_BOUND_FILE_MAXIMUM_BYTES)
        else {
            return Err(PlatformError::new(
                PlatformErrorCode::CredentialRecoveryRequired,
            ));
        };
        if actual != bytes {
            return Err(PlatformError::new(
                PlatformErrorCode::CredentialRecoveryRequired,
            ));
        }
    }
    stabilize_windows_locator_directory(&locator_root)?;
    let (_, actual) =
        read_windows_bound_record_file_with_limit(&path, WINDOWS_BOUND_FILE_MAXIMUM_BYTES)?;
    if actual == bytes {
        Ok(())
    } else {
        Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ))
    }
}

#[cfg(any(windows, test))]
fn read_windows_bound_credential_file(
    data_root: &Path,
    reference: &str,
    claim: &WindowsBoundCredentialClaim,
    delete_access: bool,
) -> PlatformResult<Option<(File, zeroize::Zeroizing<Vec<u8>>)>> {
    validate_windows_bound_credential_claim(claim)?;
    if claim.file_record_sha256.is_none() {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    let path = windows_bound_credential_file_path(
        data_root,
        reference,
        &claim.username,
        &claim.generation,
    )?;
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(PlatformError::new(
                PlatformErrorCode::CredentialRecoveryRequired,
            ));
        }
    };
    validate_windows_locator_file(&metadata)?;
    let (file, bytes) = read_windows_bound_record_file_with_limit_and_access(
        &path,
        WINDOWS_BOUND_FILE_MAXIMUM_BYTES,
        delete_access,
    )?;
    let ciphertext = decode_windows_bound_credential_file_record(reference, claim, &bytes)?;
    Ok(Some((file, ciphertext)))
}

#[cfg(any(windows, test))]
fn decode_windows_bound_credential_stage_record(
    bytes: &[u8],
) -> PlatformResult<(
    String,
    WindowsBoundCredentialClaim,
    zeroize::Zeroizing<Vec<u8>>,
)> {
    if bytes.is_empty() || bytes.len() > WINDOWS_BOUND_FILE_MAXIMUM_BYTES {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    let Some(mut remaining) = bytes.strip_prefix(WINDOWS_BOUND_FILE_MAGIC) else {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    };
    let reference = std::str::from_utf8(take_windows_bound_frame(&mut remaining)?)
        .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    validate_reference(reference)?;
    let slot = std::str::from_utf8(take_windows_bound_frame(&mut remaining)?)
        .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    let generation = std::str::from_utf8(take_windows_bound_frame(&mut remaining)?)
        .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    let ciphertext = take_windows_bound_frame(&mut remaining)?;
    if ciphertext.is_empty() || !remaining.is_empty() {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    let claim = WindowsBoundCredentialClaim {
        username: slot.to_owned(),
        generation: generation.to_owned(),
        file_record_sha256: Some(windows_bound_file_record_digest(bytes)),
    };
    validate_windows_bound_credential_claim(&claim)?;
    Ok((
        reference.to_owned(),
        claim,
        zeroize::Zeroizing::new(ciphertext.to_vec()),
    ))
}

#[cfg(any(windows, test))]
fn is_windows_bound_credential_stage_name(name: &std::ffi::OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    let Some(uuid) = name
        .strip_prefix(WINDOWS_BOUND_FILE_STAGE_PREFIX)
        .and_then(|name| name.strip_suffix(WINDOWS_BOUND_FILE_STAGE_SUFFIX))
    else {
        return false;
    };
    Uuid::parse_str(uuid).is_ok_and(|parsed| {
        parsed.get_version() == Some(Version::Random) && parsed.hyphenated().to_string() == uuid
    })
}

#[cfg(any(windows, test))]
#[derive(Default, Debug, PartialEq, Eq)]
struct WindowsBoundCredentialStageCleanup {
    scanned: usize,
    deleted: usize,
}

#[cfg(any(windows, test))]
fn cleanup_windows_bound_credential_staging_with(
    data_root: &Path,
    resource: &str,
    now: SystemTime,
    mut open_candidate: impl FnMut(&Path) -> PlatformResult<File>,
    mut delete_candidate: impl FnMut(&Path, &File) -> PlatformResult<()>,
    mut unprotect: impl FnMut(&[u8], &[u8], usize) -> PlatformResult<zeroize::Zeroizing<Vec<u8>>>,
) -> WindowsBoundCredentialStageCleanup {
    let mut outcome = WindowsBoundCredentialStageCleanup::default();
    let locator_root = data_root.join(WINDOWS_BOUND_LOCATOR_DIRECTORY);
    if validate_reference(resource).is_err()
        || validate_windows_locator_directory(data_root).is_err()
        || validate_windows_locator_directory(&locator_root).is_err()
    {
        return outcome;
    }
    let Ok(entries) = std::fs::read_dir(&locator_root) else {
        return outcome;
    };
    for entry in entries.take(WINDOWS_BOUND_FILE_STAGE_MAXIMUM_SCAN) {
        outcome.scanned += 1;
        if outcome.deleted >= WINDOWS_BOUND_FILE_STAGE_MAXIMUM_DELETE {
            break;
        }
        let Ok(entry) = entry else {
            continue;
        };
        if !is_windows_bound_credential_stage_name(&entry.file_name()) {
            continue;
        }
        let path = entry.path();
        if path.parent() != Some(locator_root.as_path()) {
            continue;
        }
        let Ok(mut file) = open_candidate(&path) else {
            continue;
        };
        let Ok(metadata) = file.metadata() else {
            continue;
        };
        if validate_windows_locator_file(&metadata).is_err()
            || metadata
                .modified()
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_none_or(|age| age < WINDOWS_BOUND_FILE_STAGE_MINIMUM_AGE)
        {
            continue;
        }
        let mut bytes = Vec::new();
        if Read::take(&mut file, (WINDOWS_BOUND_FILE_MAXIMUM_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .is_err()
            || bytes.len() > WINDOWS_BOUND_FILE_MAXIMUM_BYTES
        {
            continue;
        }
        let Ok((reference, claim, ciphertext)) =
            decode_windows_bound_credential_stage_record(&bytes)
        else {
            continue;
        };
        let Ok(entropy) = windows_bound_file_entropy(
            data_root,
            resource,
            &reference,
            &claim.username,
            &claim.generation,
        ) else {
            continue;
        };
        let Ok(plaintext) = unprotect(
            ciphertext.as_slice(),
            &entropy,
            crate::validation::MAXIMUM_CREDENTIAL_READ_BYTES,
        ) else {
            continue;
        };
        if decode_windows_bound_file_plaintext(resource, &reference, &claim, plaintext).is_err() {
            continue;
        }
        if delete_candidate(&path, &file).is_ok() {
            outcome.deleted += 1;
        }
    }
    outcome
}

#[cfg(windows)]
fn cleanup_windows_bound_credential_staging(data_root: &Path, resource: &str) {
    let _ = cleanup_windows_bound_credential_staging_with(
        data_root,
        resource,
        SystemTime::now(),
        windows::open_verified_staging_file_for_delete,
        |_, file| windows::delete_verified_file(file),
        windows::unprotect_current_user_data,
    );
}

#[cfg(all(test, not(windows)))]
fn publish_windows_bound_staged_record(source: &Path, destination: &Path) -> PlatformResult<()> {
    if std::fs::hard_link(source, destination).is_err() {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    std::fs::remove_file(source)
        .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))
}

#[cfg(any(windows, test))]
fn read_windows_bound_record_file(path: &Path) -> PlatformResult<(File, Vec<u8>)> {
    read_windows_bound_record_file_with_limit(path, WINDOWS_BOUND_RECORD_MAXIMUM_BYTES)
}

#[cfg(any(windows, test))]
fn read_windows_bound_record_file_with_limit(
    path: &Path,
    maximum_bytes: usize,
) -> PlatformResult<(File, Vec<u8>)> {
    read_windows_bound_record_file_with_limit_and_access(path, maximum_bytes, false)
}

#[cfg(any(windows, test))]
fn read_windows_bound_record_file_with_limit_and_access(
    path: &Path,
    maximum_bytes: usize,
    delete_access: bool,
) -> PlatformResult<(File, Vec<u8>)> {
    validate_windows_locator_file(
        &std::fs::symlink_metadata(path)
            .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?,
    )?;
    let mut options = OpenOptions::new();
    #[cfg(windows)]
    {
        use ::windows::Win32::{Foundation::GENERIC_READ, Storage::FileSystem::DELETE};
        use std::os::windows::fs::OpenOptionsExt;

        options.access_mode(GENERIC_READ.0 | if delete_access { DELETE.0 } else { 0 });
        options
            .share_mode(0)
            .custom_flags(::windows::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT.0);
    }
    #[cfg(not(windows))]
    {
        let _ = delete_access;
        options.read(true);
    }
    let mut file = options
        .open(path)
        .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    validate_windows_locator_file(
        &file
            .metadata()
            .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?,
    )?;
    let mut bytes = Vec::new();
    Read::take(&mut file, (maximum_bytes + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    if bytes.is_empty() || bytes.len() > maximum_bytes {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    Ok((file, bytes))
}

#[cfg(any(windows, test))]
fn read_windows_bound_record(
    data_root: &Path,
    reference: &str,
    path: &Path,
) -> PlatformResult<Option<Vec<u8>>> {
    validate_reference(reference)?;
    if !data_root.is_absolute() {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    validate_windows_locator_directory(data_root)?;
    let locator_root = data_root.join(WINDOWS_BOUND_LOCATOR_DIRECTORY);
    match std::fs::symlink_metadata(&locator_root) {
        Ok(_) => validate_windows_locator_directory(&locator_root)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(PlatformError::new(
                PlatformErrorCode::CredentialRecoveryRequired,
            ));
        }
    }
    if path.parent() != Some(locator_root.as_path()) {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(PlatformError::new(
                PlatformErrorCode::CredentialRecoveryRequired,
            ));
        }
    };
    validate_windows_locator_file(&metadata)?;
    read_windows_bound_record_file(path).map(|(_, bytes)| Some(bytes))
}

#[cfg(any(windows, test))]
fn read_windows_bound_credential_locator(
    data_root: &Path,
    reference: &str,
) -> PlatformResult<Option<WindowsBoundCredentialClaim>> {
    read_windows_bound_record(
        data_root,
        reference,
        &windows_bound_credential_locator_path(data_root, reference),
    )?
    .map(|bytes| decode_windows_bound_credential_locator(&bytes, reference))
    .transpose()
}

#[cfg(any(windows, test))]
fn read_windows_bound_credential_lifecycle(
    data_root: &Path,
    reference: &str,
) -> PlatformResult<WindowsBoundCredentialLifecycle> {
    let locator = read_windows_bound_credential_locator(data_root, reference)?;
    let intent = read_windows_bound_record(
        data_root,
        reference,
        &windows_bound_credential_delete_intent_path(data_root, reference),
    )?
    .map(|bytes| {
        if bytes.starts_with(WINDOWS_BOUND_DELETE_INTENT_V2_MAGIC.as_bytes()) {
            decode_windows_bound_delete_record_v2(
                &bytes,
                WINDOWS_BOUND_DELETE_INTENT_V2_MAGIC,
                WINDOWS_BOUND_DELETE_INTENT_V2_CHECKSUM_DOMAIN,
                reference,
            )
        } else {
            decode_windows_bound_delete_record(
                &bytes,
                WINDOWS_BOUND_DELETE_INTENT_MAGIC,
                WINDOWS_BOUND_DELETE_INTENT_CHECKSUM_DOMAIN,
                reference,
            )
        }
    })
    .transpose()?;
    let completion = read_windows_bound_record(
        data_root,
        reference,
        &windows_bound_credential_delete_completion_path(data_root, reference),
    )?
    .map(|bytes| {
        if bytes.starts_with(WINDOWS_BOUND_DELETE_COMPLETE_V2_MAGIC.as_bytes()) {
            decode_windows_bound_delete_record_v2(
                &bytes,
                WINDOWS_BOUND_DELETE_COMPLETE_V2_MAGIC,
                WINDOWS_BOUND_DELETE_COMPLETE_V2_CHECKSUM_DOMAIN,
                reference,
            )
        } else {
            decode_windows_bound_delete_record(
                &bytes,
                WINDOWS_BOUND_DELETE_COMPLETE_MAGIC,
                WINDOWS_BOUND_DELETE_COMPLETE_CHECKSUM_DOMAIN,
                reference,
            )
        }
    })
    .transpose()?;

    let target_matches_locator = |target: &WindowsBoundDeleteTarget| match (target, &locator) {
        (WindowsBoundDeleteTarget::Claimed(target), Some(locator)) => target == locator,
        (WindowsBoundDeleteTarget::Legacy, None) => true,
        _ => false,
    };
    match (&locator, &intent, &completion) {
        (None, None, None) => Ok(WindowsBoundCredentialLifecycle::Missing),
        (Some(claim), None, None) => Ok(WindowsBoundCredentialLifecycle::Active(claim.clone())),
        (_, Some(target), None) if target_matches_locator(target) => {
            Ok(WindowsBoundCredentialLifecycle::Deleting(target.clone()))
        }
        (_, Some(intent), Some(completion))
            if intent == completion && target_matches_locator(intent) =>
        {
            Ok(WindowsBoundCredentialLifecycle::Deleted)
        }
        _ => Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        )),
    }
}

#[cfg(any(windows, test))]
fn publish_windows_bound_delete_record(
    data_root: &Path,
    reference: &str,
    target: &WindowsBoundDeleteTarget,
    complete: bool,
) -> PlatformResult<()> {
    let v2 = matches!(
        target,
        WindowsBoundDeleteTarget::Claimed(WindowsBoundCredentialClaim {
            file_record_sha256: Some(_),
            ..
        })
    );
    let (path, magic, domain) = if complete && v2 {
        (
            windows_bound_credential_delete_completion_path(data_root, reference),
            WINDOWS_BOUND_DELETE_COMPLETE_V2_MAGIC,
            WINDOWS_BOUND_DELETE_COMPLETE_V2_CHECKSUM_DOMAIN,
        )
    } else if complete {
        (
            windows_bound_credential_delete_completion_path(data_root, reference),
            WINDOWS_BOUND_DELETE_COMPLETE_MAGIC,
            WINDOWS_BOUND_DELETE_COMPLETE_CHECKSUM_DOMAIN,
        )
    } else if v2 {
        (
            windows_bound_credential_delete_intent_path(data_root, reference),
            WINDOWS_BOUND_DELETE_INTENT_V2_MAGIC,
            WINDOWS_BOUND_DELETE_INTENT_V2_CHECKSUM_DOMAIN,
        )
    } else {
        (
            windows_bound_credential_delete_intent_path(data_root, reference),
            WINDOWS_BOUND_DELETE_INTENT_MAGIC,
            WINDOWS_BOUND_DELETE_INTENT_CHECKSUM_DOMAIN,
        )
    };
    let bytes = encode_windows_bound_delete_record(magic, domain, reference, target);
    drop(publish_windows_bound_record(data_root, &path, &bytes)?);
    Ok(())
}

#[cfg(any(windows, test))]
fn store_prevalidated_windows_bound_credential_claim_with(
    data_root: &Path,
    reference: &str,
    claim: &WindowsBoundCredentialClaim,
    expected: &NativeCredential,
    add: impl FnOnce(&WindowsBoundCredentialClaim) -> PlatformResult<()>,
    mut read_claimed: impl FnMut(
        &WindowsBoundCredentialClaim,
    ) -> PlatformResult<Option<NativeCredential>>,
    read_legacy: impl FnOnce() -> PlatformResult<Option<NativeCredential>>,
) -> PlatformResult<()> {
    let _guard = lock_windows_bound_credential_operation(data_root, reference)?;
    if !matches!(
        read_windows_bound_credential_lifecycle(data_root, reference)?,
        WindowsBoundCredentialLifecycle::Missing
    ) || !matches!(read_legacy(), Ok(None))
    {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    let _locator = claim_windows_bound_credential_locator_for_claim(data_root, reference, claim)?;
    if !matches!(read_claimed(claim), Ok(None)) {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    if add(claim).is_err() {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    let actual = read_claimed(claim)
        .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    if actual
        .as_ref()
        .is_some_and(|actual| actual.expose() == expected.expose())
    {
        Ok(())
    } else {
        Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ))
    }
}

#[cfg(any(windows, test))]
fn read_windows_bound_credential_claim_with(
    data_root: &Path,
    reference: &str,
    mut read_claimed: impl FnMut(
        &WindowsBoundCredentialClaim,
    ) -> PlatformResult<Option<NativeCredential>>,
    read_legacy: impl FnOnce() -> PlatformResult<Option<NativeCredential>>,
) -> PlatformResult<Option<NativeCredential>> {
    let _guard = lock_windows_bound_credential_operation(data_root, reference)?;
    match read_windows_bound_credential_lifecycle(data_root, reference)? {
        WindowsBoundCredentialLifecycle::Missing => read_legacy()
            .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired)),
        WindowsBoundCredentialLifecycle::Active(claim) => read_claimed(&claim)
            .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?
            .map(Some)
            .ok_or_else(|| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired)),
        WindowsBoundCredentialLifecycle::Deleting(_) => Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        )),
        WindowsBoundCredentialLifecycle::Deleted => Ok(None),
    }
}

#[cfg(any(windows, test))]
fn delete_windows_bound_credential_claim_with<T>(
    data_root: &Path,
    reference: &str,
    mut read_claimed: impl FnMut(&WindowsBoundCredentialClaim) -> PlatformResult<Option<T>>,
    mut remove_claimed: impl FnMut(&WindowsBoundCredentialClaim, &T) -> PlatformResult<()>,
    mut read_legacy: impl FnMut() -> PlatformResult<Option<T>>,
    mut remove_legacy: impl FnMut(&T) -> PlatformResult<()>,
) -> PlatformResult<()> {
    let _guard = lock_windows_bound_credential_operation(data_root, reference)?;
    let (target, retry) = match read_windows_bound_credential_lifecycle(data_root, reference)? {
        WindowsBoundCredentialLifecycle::Deleted => return Ok(()),
        WindowsBoundCredentialLifecycle::Deleting(target) => (target, true),
        WindowsBoundCredentialLifecycle::Active(claim) => {
            let target = WindowsBoundDeleteTarget::Claimed(claim);
            publish_windows_bound_delete_record(data_root, reference, &target, false)?;
            (target, false)
        }
        WindowsBoundCredentialLifecycle::Missing => {
            let target = WindowsBoundDeleteTarget::Legacy;
            publish_windows_bound_delete_record(data_root, reference, &target, false)?;
            (target, false)
        }
    };

    let credential = match &target {
        WindowsBoundDeleteTarget::Claimed(claim) => read_claimed(claim),
        WindowsBoundDeleteTarget::Legacy => read_legacy(),
    }
    .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    if retry && matches!(target, WindowsBoundDeleteTarget::Legacy) && credential.is_some() {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    if let Some(credential) = credential.as_ref() {
        match &target {
            WindowsBoundDeleteTarget::Claimed(claim) => remove_claimed(claim, credential),
            WindowsBoundDeleteTarget::Legacy => remove_legacy(credential),
        }
        .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
        let missing = match &target {
            WindowsBoundDeleteTarget::Claimed(claim) => read_claimed(claim),
            WindowsBoundDeleteTarget::Legacy => read_legacy(),
        };
        if !matches!(missing, Ok(None)) {
            return Err(PlatformError::new(
                PlatformErrorCode::CredentialRecoveryRequired,
            ));
        }
    }
    publish_windows_bound_delete_record(data_root, reference, &target, true)
}

// Existing host-side tests use a deterministic generation so their fixture
// setup stays compact. Production never derives a generation from a username;
// it always calls `new_windows_bound_credential_claim` and passes the complete
// claim through the locator, wrapper, and native callbacks.
#[cfg(test)]
fn test_windows_bound_credential_claim_from_username(
    username: &str,
) -> PlatformResult<WindowsBoundCredentialClaim> {
    validate_windows_bound_credential_username(username)?;
    let uuid = username
        .strip_prefix(WINDOWS_BOUND_USERNAME_PREFIX)
        .ok_or_else(|| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    let claim = WindowsBoundCredentialClaim {
        username: username.to_owned(),
        generation: format!("{WINDOWS_BOUND_GENERATION_PREFIX}{uuid}"),
        file_record_sha256: None,
    };
    validate_windows_bound_credential_claim(&claim)?;
    Ok(claim)
}

#[cfg(test)]
fn encode_windows_bound_credential_locator(reference: &str, username: &str) -> Vec<u8> {
    encode_windows_bound_credential_locator_for_claim(
        reference,
        &test_windows_bound_credential_claim_from_username(username)
            .expect("validated Windows credential test claim"),
    )
}

#[cfg(test)]
fn claim_windows_bound_credential_locator(
    data_root: &Path,
    reference: &str,
    username: &str,
) -> PlatformResult<WindowsBoundCredentialLocatorClaim> {
    claim_windows_bound_credential_locator_for_claim(
        data_root,
        reference,
        &test_windows_bound_credential_claim_from_username(username)?,
    )
}

#[cfg(test)]
fn claim_windows_bound_credential_locator_with_writer(
    data_root: &Path,
    reference: &str,
    username: &str,
    write_staged: impl FnOnce(&mut File, &[u8]) -> std::io::Result<()>,
) -> PlatformResult<WindowsBoundCredentialLocatorClaim> {
    claim_windows_bound_credential_locator_for_claim_with_writer(
        data_root,
        reference,
        &test_windows_bound_credential_claim_from_username(username)?,
        write_staged,
    )
}

#[cfg(test)]
fn store_prevalidated_windows_bound_credential_with(
    data_root: &Path,
    reference: &str,
    username: &str,
    expected: &NativeCredential,
    add: impl FnOnce(&str) -> PlatformResult<()>,
    mut read_claimed: impl FnMut(&str) -> PlatformResult<Option<NativeCredential>>,
    read_legacy: impl FnOnce() -> PlatformResult<Option<NativeCredential>>,
) -> PlatformResult<()> {
    let claim = test_windows_bound_credential_claim_from_username(username)?;
    store_prevalidated_windows_bound_credential_claim_with(
        data_root,
        reference,
        &claim,
        expected,
        |claim| add(&claim.username),
        |claim| read_claimed(&claim.username),
        read_legacy,
    )
}

#[cfg(test)]
fn read_windows_bound_credential_with(
    data_root: &Path,
    reference: &str,
    mut read_claimed: impl FnMut(&str) -> PlatformResult<Option<NativeCredential>>,
    read_legacy: impl FnOnce() -> PlatformResult<Option<NativeCredential>>,
) -> PlatformResult<Option<NativeCredential>> {
    read_windows_bound_credential_claim_with(
        data_root,
        reference,
        |claim| read_claimed(&claim.username),
        read_legacy,
    )
}

#[cfg(test)]
fn delete_windows_bound_credential_with<T>(
    data_root: &Path,
    reference: &str,
    mut read_claimed: impl FnMut(&str) -> PlatformResult<Option<T>>,
    mut remove_claimed: impl FnMut(&str, &T) -> PlatformResult<()>,
    read_legacy: impl FnMut() -> PlatformResult<Option<T>>,
    remove_legacy: impl FnMut(&T) -> PlatformResult<()>,
) -> PlatformResult<()> {
    delete_windows_bound_credential_claim_with(
        data_root,
        reference,
        |claim| read_claimed(&claim.username),
        |claim, credential| remove_claimed(&claim.username, credential),
        read_legacy,
        remove_legacy,
    )
}

impl<R: Runtime> DesktopPlatform<R> {
    pub(crate) fn new(app: AppHandle<R>) -> PlatformResult<Self> {
        #[cfg(any(target_os = "macos", windows))]
        {
            let policy = platform_policy(&app.config().identifier)?;
            #[cfg(windows)]
            debug_assert!(!policy.migrate_legacy_credentials);
            let staging_root = policy.data_root.join(policy.staging_name);
            std::fs::create_dir_all(&policy.data_root)
                .and_then(|()| std::fs::create_dir_all(&staging_root))
                .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
            cleanup_abandoned_staging(&staging_root, Duration::from_hours(24));
            Ok(Self {
                app,
                data_root: policy.data_root,
                staging_root,
                export_in_flight: AtomicBool::new(false),
                sensitive_capture_in_flight: AtomicBool::new(false),
                credential_namespace: policy.credential_namespace,
                #[cfg(target_os = "macos")]
                migrate_legacy_credentials: policy.migrate_legacy_credentials,
            })
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            drop(app);
            unsupported_platform()
        }
    }

    pub(crate) fn data_root(&self) -> &Path {
        &self.data_root
    }

    #[cfg(any(target_os = "macos", windows))]
    pub(crate) async fn confirm_credential_effect(
        &self,
        context: &NativeCredentialEffectContext,
    ) -> PlatformResult<()> {
        let (title, informative_text) = credential_confirmation_copy(context);
        #[cfg(target_os = "macos")]
        {
            let (sender, receiver) = tokio::sync::oneshot::channel();
            let app = self.app.clone();
            self.app
                .run_on_main_thread(move || {
                    let result = ensure_main_window_focused(&app)
                        .and_then(|()| macos::confirm_credential_effect(&title, &informative_text));
                    let _ = sender.send(result);
                })
                .map_err(|_| PlatformError::new(PlatformErrorCode::PermissionDenied))?;
            receiver
                .await
                .map_err(|_| PlatformError::new(PlatformErrorCode::PermissionDenied))?
        }
        #[cfg(windows)]
        windows::confirm_credential_effect(&self.app, &title, &informative_text).await
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    pub(crate) fn confirm_credential_effect(
        &self,
        _context: &NativeCredentialEffectContext,
    ) -> std::future::Ready<PlatformResult<()>> {
        std::future::ready(self.unsupported())
    }

    #[cfg(any(target_os = "macos", windows))]
    pub(crate) async fn pick_import(&self) -> PlatformResult<Option<StagedImport>> {
        #[cfg(target_os = "macos")]
        let selection = macos::pick_file(&self.app).await?;
        #[cfg(windows)]
        let selection = windows::pick_file(&self.app).await?;

        let Some(selection) = selection else {
            return Ok(None);
        };
        stage_selected_file(selection, self.staging_root.clone()).await
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    pub(crate) fn pick_import(&self) -> std::future::Ready<PlatformResult<Option<StagedImport>>> {
        std::future::ready(self.unsupported())
    }

    pub(crate) fn discard_staged_import(&self, staged: &StagedImport) -> PlatformResult<()> {
        if staged.path().parent() != Some(self.staging_root.as_path()) {
            return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
        }
        match std::fs::remove_file(staged.path()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(PlatformError::new(PlatformErrorCode::StorageUnavailable)),
        }
    }

    #[cfg(any(target_os = "macos", windows))]
    pub(crate) async fn save_content_source(
        &self,
        source_path: &Path,
        suggested_name: &str,
        expected_size_bytes: u64,
        expected_sha256: &str,
    ) -> PlatformResult<Option<NativeSavedContentSource>> {
        verify_content_source_for_export(
            source_path,
            &self.data_root,
            expected_size_bytes,
            expected_sha256,
            suggested_name,
        )?;
        let _export = self.begin_export()?;

        #[cfg(target_os = "macos")]
        let destination = macos::pick_export_destination(&self.app, suggested_name).await?;
        #[cfg(windows)]
        let destination = windows::pick_export_destination(&self.app, suggested_name).await?;
        let Some(destination) = destination else {
            return Ok(None);
        };
        let destination = validate_content_export_destination(&destination, &self.data_root)?;

        let source_path = source_path.to_owned();
        let data_root = self.data_root.clone();
        let suggested_name = suggested_name.to_owned();
        let expected_sha256 = expected_sha256.to_owned();
        let saved = tokio::task::spawn_blocking(move || {
            // Re-verify after the user-controlled picker delay and hash the
            // source again while creating the same-directory temporary file.
            verify_content_source_for_export(
                &source_path,
                &data_root,
                expected_size_bytes,
                &expected_sha256,
                &suggested_name,
            )?;
            crate::staging::atomic_export_to_destination(
                &source_path,
                &destination,
                expected_size_bytes,
                &expected_sha256,
            )?;
            let display_name = destination
                .file_name()
                .to_str()
                .ok_or_else(|| PlatformError::new(PlatformErrorCode::SelectionFailed))?
                .to_owned();
            validate_export_receipt_display_name(&display_name)?;
            Ok(NativeSavedContentSource::new(
                display_name,
                expected_size_bytes,
                expected_sha256,
            ))
        })
        .await
        .map_err(|_| PlatformError::new(PlatformErrorCode::Internal))??;
        Ok(Some(saved))
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    pub(crate) fn save_content_source(
        &self,
        source_path: &Path,
        suggested_name: &str,
        expected_size_bytes: u64,
        expected_sha256: &str,
    ) -> std::future::Ready<PlatformResult<Option<NativeSavedContentSource>>> {
        let _ = (
            source_path,
            suggested_name,
            expected_size_bytes,
            expected_sha256,
        );
        std::future::ready(self.unsupported())
    }

    pub(crate) fn credential_status(&self, reference: &str) -> PlatformResult<CredentialStatus> {
        #[cfg(target_os = "macos")]
        return macos::credential_status(
            self.credential_namespace,
            self.migrate_legacy_credentials,
            reference,
        );
        #[cfg(windows)]
        return windows::credential_status(self.credential_namespace, reference);
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            let _ = reference;
            self.unsupported()
        }
    }

    pub(crate) fn bound_credential_status(
        &self,
        reference: &str,
    ) -> PlatformResult<CredentialStatus> {
        #[cfg(target_os = "macos")]
        return macos::bound_credential_status(self.credential_namespace, reference);
        #[cfg(windows)]
        return windows::bound_credential_status(
            &self.data_root,
            self.credential_namespace,
            reference,
        );
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            let _ = reference;
            self.unsupported()
        }
    }

    pub(crate) fn read_bound_credential(
        &self,
        reference: &str,
    ) -> PlatformResult<Option<NativeCredential>> {
        #[cfg(target_os = "macos")]
        return macos::read_bound_credential(self.credential_namespace, reference);
        #[cfg(windows)]
        return windows::read_bound_credential(
            &self.data_root,
            self.credential_namespace,
            reference,
        );
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            let _ = reference;
            self.unsupported()
        }
    }

    pub(crate) fn read_credential(
        &self,
        reference: &str,
    ) -> PlatformResult<Option<NativeCredential>> {
        #[cfg(target_os = "macos")]
        return macos::read_credential(
            self.credential_namespace,
            self.migrate_legacy_credentials,
            reference,
        );
        #[cfg(windows)]
        return windows::read_credential(self.credential_namespace, reference);
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            let _ = reference;
            self.unsupported()
        }
    }

    pub(crate) fn store_credential(
        &self,
        reference: &str,
        value: NativeCredential,
    ) -> PlatformResult<()> {
        self.validate_credential_store(reference, &value)?;
        self.store_prevalidated_credential(reference, value)
    }

    pub(crate) fn validate_credential_store(
        &self,
        reference: &str,
        value: &NativeCredential,
    ) -> PlatformResult<()> {
        let _ = self;
        #[cfg(target_os = "macos")]
        return macos::validate_credential_store(reference, value);
        #[cfg(windows)]
        return windows::validate_credential_store(self.credential_namespace, reference, value);
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            let _ = reference;
            let _ = value;
            self.unsupported()
        }
    }

    pub(crate) fn store_prevalidated_credential(
        &self,
        reference: &str,
        value: NativeCredential,
    ) -> PlatformResult<()> {
        #[cfg(target_os = "macos")]
        return macos::store_prevalidated_credential(
            self.credential_namespace,
            self.migrate_legacy_credentials,
            reference,
            value,
        );
        #[cfg(windows)]
        return windows::store_prevalidated_credential(self.credential_namespace, reference, value);
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            let _ = reference;
            drop(value);
            self.unsupported()
        }
    }

    pub(crate) fn store_prevalidated_bound_credential(
        &self,
        reference: &str,
        value: NativeCredential,
    ) -> PlatformResult<()> {
        #[cfg(target_os = "macos")]
        return macos::store_prevalidated_bound_credential(
            self.credential_namespace,
            reference,
            value,
        );
        #[cfg(windows)]
        return windows::store_prevalidated_bound_credential(
            &self.data_root,
            self.credential_namespace,
            reference,
            value,
        );
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            let _ = reference;
            drop(value);
            self.unsupported()
        }
    }

    pub(crate) fn delete_credential(&self, reference: &str) -> PlatformResult<()> {
        #[cfg(target_os = "macos")]
        return macos::delete_credential(
            self.credential_namespace,
            self.migrate_legacy_credentials,
            reference,
        );
        #[cfg(windows)]
        return windows::delete_credential(self.credential_namespace, reference);
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            let _ = reference;
            self.unsupported()
        }
    }

    pub(crate) fn delete_bound_credential(&self, reference: &str) -> PlatformResult<()> {
        #[cfg(target_os = "macos")]
        return macos::delete_bound_credential(self.credential_namespace, reference);
        #[cfg(windows)]
        return windows::delete_bound_credential(
            &self.data_root,
            self.credential_namespace,
            reference,
        );
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            let _ = reference;
            self.unsupported()
        }
    }

    pub(crate) async fn capture_credential_from_clipboard(
        &self,
        reference: &str,
    ) -> PlatformResult<NativeCaptureStatus> {
        validate_reference(reference)?;
        let captured = self
            .capture_sensitive_text_from_clipboard(MAXIMUM_CREDENTIAL_WRITE_BYTES)
            .await?;
        let status = captured.status();
        self.store_credential(
            reference,
            NativeCredential::new(captured.into_secret_string()),
        )?;
        Ok(status)
    }

    #[cfg(target_os = "macos")]
    pub(crate) async fn capture_sensitive_text_from_clipboard(
        &self,
        maximum_bytes: usize,
    ) -> PlatformResult<NativeSensitiveText> {
        validate_sensitive_capture_limit(maximum_bytes)?;
        let _capture = self.begin_sensitive_capture()?;
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let app = self.app.clone();
        self.app
            .run_on_main_thread(move || {
                let result = ensure_main_window_focused(&app)
                    .and_then(|()| macos::capture_clipboard_text(maximum_bytes));
                let _ = sender.send(result);
            })
            .map_err(|_| PlatformError::new(PlatformErrorCode::Internal))?;
        receiver
            .await
            .map_err(|_| PlatformError::new(PlatformErrorCode::Internal))?
    }

    #[cfg(windows)]
    pub(crate) async fn capture_sensitive_text_from_clipboard(
        &self,
        maximum_bytes: usize,
    ) -> PlatformResult<NativeSensitiveText> {
        validate_sensitive_capture_limit(maximum_bytes)?;
        let _capture = self.begin_sensitive_capture()?;
        let app = self.app.clone();
        tokio::task::spawn_blocking(move || {
            ensure_main_window_focused(&app)?;
            windows::capture_clipboard_text(maximum_bytes)
        })
        .await
        .map_err(|_| PlatformError::new(PlatformErrorCode::Internal))?
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    pub(crate) fn capture_sensitive_text_from_clipboard(
        &self,
        maximum_bytes: usize,
    ) -> std::future::Ready<PlatformResult<NativeSensitiveText>> {
        let _ = maximum_bytes;
        std::future::ready(self.unsupported())
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    fn unsupported<T>(&self) -> PlatformResult<T> {
        let _ = &self._runtime;
        unsupported_platform()
    }

    #[cfg(any(target_os = "macos", windows))]
    fn begin_sensitive_capture(&self) -> PlatformResult<SensitiveCaptureGuard<'_>> {
        self.sensitive_capture_in_flight
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .map_err(|_| PlatformError::new(PlatformErrorCode::Busy))?;
        Ok(SensitiveCaptureGuard {
            active: &self.sensitive_capture_in_flight,
        })
    }

    #[cfg(any(target_os = "macos", windows))]
    fn begin_export(&self) -> PlatformResult<ExportGuard<'_>> {
        self.export_in_flight
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .map_err(|_| PlatformError::new(PlatformErrorCode::Busy))?;
        Ok(ExportGuard {
            active: &self.export_in_flight,
        })
    }
}

#[cfg(any(target_os = "macos", windows, test))]
fn credential_confirmation_copy(context: &NativeCredentialEffectContext) -> (String, String) {
    let (title, effect) = match context.effect() {
        NativeCredentialEffect::CaptureOrReplace => (
            "Allow credential capture?",
            "read one credential from the clipboard and store it. If an older credential exists, it will be deleted only after the replacement is stored",
        ),
        NativeCredentialEffect::Delete => (
            "Delete stored credential?",
            "permanently delete the stored credential",
        ),
        NativeCredentialEffect::Archive => (
            "Archive connection and delete credential?",
            "archive this provider connection and permanently delete its stored credential",
        ),
        NativeCredentialEffect::DiscoveryCompensation => (
            "Remove uncommitted credential?",
            "permanently delete the credential created by the cancelled or failed discovery",
        ),
    };
    (
        title.to_owned(),
        format!(
            "LorePia will {effect}.\n\nTarget: {}\nOrigin: {}\nRevision: {}\n\nApprove only if these exact details match your intended action.",
            context.target_id(),
            context.origin(),
            context.revision(),
        ),
    )
}

#[cfg(any(target_os = "macos", windows))]
struct ExportGuard<'a> {
    active: &'a AtomicBool,
}

#[cfg(any(target_os = "macos", windows))]
impl Drop for ExportGuard<'_> {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
    }
}

#[cfg(any(target_os = "macos", windows))]
struct SensitiveCaptureGuard<'a> {
    active: &'a AtomicBool,
}

#[cfg(any(target_os = "macos", windows))]
impl Drop for SensitiveCaptureGuard<'_> {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
    }
}

#[cfg(any(target_os = "macos", windows))]
fn ensure_main_window_focused<R: Runtime>(app: &AppHandle<R>) -> PlatformResult<()> {
    let focused = app
        .get_webview_window("main")
        .ok_or_else(|| PlatformError::new(PlatformErrorCode::PermissionDenied))?
        .is_focused()
        .map_err(|_| PlatformError::new(PlatformErrorCode::PermissionDenied))?;
    if focused {
        Ok(())
    } else {
        Err(PlatformError::new(PlatformErrorCode::PermissionDenied))
    }
}

#[cfg(any(target_os = "macos", windows))]
fn validate_sensitive_capture_limit(maximum_bytes: usize) -> PlatformResult<()> {
    // Validate the caller-controlled limit before any native clipboard access.
    // A one-byte synthetic value exercises the same hard upper bound without
    // touching sensitive content.
    validate_sensitive_capture("x", maximum_bytes)
}

/// Best-effort cleanup is deliberately restricted to old, regular files with
/// the random prefix created by this plugin. A fresh file may belong to
/// another process which has not opened Core yet, so it is never removed.
#[cfg(any(target_os = "macos", windows, test))]
fn cleanup_abandoned_staging(staging_root: &Path, minimum_age: Duration) {
    let Ok(entries) = std::fs::read_dir(staging_root) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !name.starts_with(crate::staging::OWNED_STAGING_PREFIX) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|metadata| metadata.modified()) else {
            continue;
        };
        if now
            .duration_since(modified)
            .is_ok_and(|age| age >= minimum_age)
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(any(target_os = "macos", windows, test))]
struct DesktopPolicy {
    data_root: PathBuf,
    staging_name: &'static str,
    credential_namespace: &'static str,
    migrate_legacy_credentials: bool,
}

#[cfg(target_os = "macos")]
fn platform_policy(identifier: &str) -> PlatformResult<DesktopPolicy> {
    use objc2_foundation::{
        NSSearchPathDirectory, NSSearchPathDomainMask, NSSearchPathForDirectoriesInDomains,
    };

    let application_support = NSSearchPathForDirectoriesInDomains(
        NSSearchPathDirectory::ApplicationSupportDirectory,
        NSSearchPathDomainMask::UserDomainMask,
        true,
    )
    .firstObject()
    .map(|value| PathBuf::from(value.to_string()))
    .filter(|path| path.is_absolute())
    .ok_or_else(|| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
    macos_policy_from_application_support(application_support, identifier)
}

#[cfg(any(target_os = "macos", test))]
fn macos_policy_from_application_support(
    application_support: PathBuf,
    identifier: &str,
) -> PlatformResult<DesktopPolicy> {
    match identifier {
        "dev.lorepia.mac" => Ok(DesktopPolicy {
            data_root: application_support.join("LorePia"),
            staging_name: "native-staging",
            credential_namespace: "dev.lorepia.provider-credentials",
            migrate_legacy_credentials: true,
        }),
        "dev.lorepia.mac.dev" => Ok(DesktopPolicy {
            data_root: application_support.join("LorePia Development"),
            staging_name: "native-staging",
            credential_namespace: "dev.lorepia.provider-credentials.dev",
            migrate_legacy_credentials: false,
        }),
        _ => Err(PlatformError::new(PlatformErrorCode::Internal)),
    }
}

#[cfg(windows)]
fn platform_policy(identifier: &str) -> PlatformResult<DesktopPolicy> {
    windows_policy_from_local_app_data(windows_local_app_data()?, identifier)
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn windows_local_app_data() -> PlatformResult<PathBuf> {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt};

    use ::windows::Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{FOLDERID_LocalAppData, KNOWN_FOLDER_FLAG, SHGetKnownFolderPath},
    };

    // SAFETY: `SHGetKnownFolderPath` initializes an owned, NUL-terminated
    // buffer on success. We copy its UTF-16 contents before releasing that
    // buffer exactly once with `CoTaskMemFree`.
    let raw_path =
        unsafe { SHGetKnownFolderPath(&FOLDERID_LocalAppData, KNOWN_FOLDER_FLAG(0), None) }
            .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
    if raw_path.is_null() {
        return Err(PlatformError::new(PlatformErrorCode::StorageUnavailable));
    }
    // SAFETY: the successful API result is valid through the terminating NUL
    // until the matching `CoTaskMemFree` below.
    let path = PathBuf::from(OsString::from_wide(unsafe { raw_path.as_wide() }));
    // SAFETY: the pointer was allocated by `SHGetKnownFolderPath` and has not
    // been freed or transferred.
    unsafe {
        CoTaskMemFree(Some(
            raw_path.as_ptr().cast::<std::ffi::c_void>().cast_const(),
        ));
    }
    if !path.is_absolute() {
        return Err(PlatformError::new(PlatformErrorCode::StorageUnavailable));
    }
    Ok(path)
}

#[cfg(any(windows, test))]
fn windows_policy_from_local_app_data(
    local_app_data: PathBuf,
    identifier: &str,
) -> PlatformResult<DesktopPolicy> {
    match identifier {
        "dev.lorepia.windows" => Ok(DesktopPolicy {
            data_root: local_app_data.join("LorePia"),
            staging_name: "transport-staging",
            credential_namespace: "LorePia.ProviderCredential",
            migrate_legacy_credentials: false,
        }),
        "dev.lorepia.windows.dev" => Ok(DesktopPolicy {
            data_root: local_app_data.join("LorePia Development"),
            staging_name: "transport-staging",
            credential_namespace: "LorePia.ProviderCredential.Development",
            migrate_legacy_credentials: false,
        }),
        _ => Err(PlatformError::new(PlatformErrorCode::Internal)),
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn unsupported_platform<T>() -> PlatformResult<T> {
    Err(PlatformError::new(PlatformErrorCode::UnsupportedPlatform))
}

#[cfg(any(target_os = "macos", windows))]
async fn stage_selected_file(
    selection: PathBuf,
    staging_root: PathBuf,
) -> PlatformResult<Option<StagedImport>> {
    tokio::task::spawn_blocking(move || {
        crate::staging::stage_file(&selection, &staging_root, 256 * 1024 * 1024).map(Some)
    })
    .await
    .map_err(|_| PlatformError::new(PlatformErrorCode::Internal))?
}

#[cfg(test)]
mod tests {
    use std::{
        cell::RefCell,
        collections::HashMap,
        io::Write,
        path::PathBuf,
        sync::{Arc, Barrier, Mutex, mpsc},
        time::Duration,
    };

    use sha2::{Digest, Sha256};
    use tempfile::tempdir;

    use crate::{NativeCredential, PlatformError, PlatformErrorCode};

    #[cfg(unix)]
    use super::{WINDOWS_BOUND_FILE_STAGE_MAXIMUM_DELETE, WINDOWS_BOUND_FILE_STAGE_MAXIMUM_SCAN};

    use super::{
        WINDOWS_BOUND_DELETE_COMPLETE_CHECKSUM_DOMAIN, WINDOWS_BOUND_DELETE_COMPLETE_MAGIC,
        WINDOWS_BOUND_DELETE_COMPLETE_V2_CHECKSUM_DOMAIN, WINDOWS_BOUND_DELETE_COMPLETE_V2_MAGIC,
        WINDOWS_BOUND_DELETE_INTENT_CHECKSUM_DOMAIN, WINDOWS_BOUND_DELETE_INTENT_MAGIC,
        WINDOWS_BOUND_DELETE_INTENT_V2_CHECKSUM_DOMAIN, WINDOWS_BOUND_DELETE_INTENT_V2_MAGIC,
        WINDOWS_BOUND_FILE_ENTROPY_DOMAIN, WINDOWS_BOUND_FILE_SLOT_PREFIX,
        WINDOWS_BOUND_FILE_STAGE_PREFIX, WINDOWS_BOUND_FILE_STAGE_SUFFIX,
        WINDOWS_BOUND_GENERATION_PREFIX, WINDOWS_BOUND_LOCATOR_DIRECTORY,
        WINDOWS_BOUND_LOCATOR_HASH_DOMAIN, WINDOWS_BOUND_MUTEX_HASH_DOMAIN,
        WINDOWS_BOUND_PHYSICAL_REFERENCE_PREFIX, WINDOWS_BOUND_RECORD_MAXIMUM_BYTES,
        WINDOWS_BOUND_USERNAME_PREFIX, WINDOWS_BOUND_VALUE_MAGIC, WindowsBoundCredentialClaim,
        WindowsBoundCredentialFileSeed, WindowsBoundDeleteTarget, append_windows_bound_frame,
        claim_windows_bound_credential_locator, claim_windows_bound_credential_locator_for_claim,
        claim_windows_bound_credential_locator_with_writer, cleanup_abandoned_staging,
        cleanup_windows_bound_credential_staging_with, decode_windows_bound_credential_file_record,
        decode_windows_bound_credential_locator, decode_windows_bound_credential_value,
        decode_windows_bound_delete_record, decode_windows_bound_delete_record_v2,
        delete_windows_bound_credential_claim_with, delete_windows_bound_credential_with,
        encode_windows_bound_credential_file_record, encode_windows_bound_credential_locator,
        encode_windows_bound_credential_locator_for_claim, encode_windows_bound_credential_value,
        encode_windows_bound_delete_record, encode_windows_bound_file_plaintext,
        macos_policy_from_application_support, new_windows_bound_credential_file_seed,
        prepare_windows_bound_credential_file_with, publish_windows_bound_credential_file,
        publish_windows_bound_credential_file_with, publish_windows_bound_delete_record,
        publish_windows_bound_record_with, read_windows_bound_credential_file_value_with,
        read_windows_bound_credential_with, store_prevalidated_windows_bound_credential_with,
        test_windows_bound_credential_claim_from_username, validate_windows_bound_uuid,
        validate_windows_raw_credential_reference, windows_bound_credential_delete_completion_path,
        windows_bound_credential_delete_intent_path, windows_bound_credential_file_path,
        windows_bound_credential_locator_path, windows_bound_credential_mutex_name,
        windows_policy_from_local_app_data, with_validated_windows_raw_credential_reference,
    };

    const RECOVERY_COMPATIBILITY_VECTORS: &str =
        include_str!("../../../testdata/tauri-upgrade/recovery-compatibility-v1-vectors.json");

    fn recovery_vector_str<'a>(value: &'a serde_json::Value, field: &str) -> &'a str {
        value[field]
            .as_str()
            .unwrap_or_else(|| panic!("recovery compatibility vector field {field}"))
    }

    #[derive(Default)]
    struct FakeAddOnlyCredentialBackend {
        values: HashMap<String, String>,
        injected_value_after_add: Option<String>,
        add_calls: u32,
        read_calls: u32,
        remove_calls: u32,
    }

    fn store_bound_with_fake(
        data_root: &std::path::Path,
        reference: &str,
        username: &str,
        backend: &RefCell<FakeAddOnlyCredentialBackend>,
        value: &NativeCredential,
    ) -> crate::PlatformResult<()> {
        store_prevalidated_windows_bound_credential_with(
            data_root,
            reference,
            username,
            value,
            |username| {
                let mut backend = backend.borrow_mut();
                backend.add_calls += 1;
                backend
                    .values
                    .insert(username.to_owned(), value.expose().to_owned());
                if let Some(injected) = backend.injected_value_after_add.take() {
                    backend.values.insert(username.to_owned(), injected);
                }
                Ok(())
            },
            |username| {
                let mut backend = backend.borrow_mut();
                backend.read_calls += 1;
                Ok(backend
                    .values
                    .get(username)
                    .map(|stored| NativeCredential::new(stored.clone())))
            },
            || {
                let mut backend = backend.borrow_mut();
                backend.read_calls += 1;
                Ok(backend
                    .values
                    .get(reference)
                    .map(|stored| NativeCredential::new(stored.clone())))
            },
        )
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn windows_recovery_compatibility_v1_known_vector_matches_durable_slot_protocol() {
        let vectors: serde_json::Value = serde_json::from_str(RECOVERY_COMPATIBILITY_VECTORS)
            .expect("recovery compatibility vectors must be JSON");
        let bound = &vectors["bound_credential"];
        let vector = &vectors["windows_bound_credential"];
        let reference = recovery_vector_str(vector, "physical_reference");
        assert_eq!(reference, recovery_vector_str(bound, "physical_reference"));

        let claim = WindowsBoundCredentialClaim {
            username: recovery_vector_str(vector, "username").to_owned(),
            generation: recovery_vector_str(vector, "generation").to_owned(),
            file_record_sha256: None,
        };
        assert_ne!(
            claim
                .username
                .strip_prefix(WINDOWS_BOUND_USERNAME_PREFIX)
                .expect("username prefix"),
            claim
                .generation
                .strip_prefix(WINDOWS_BOUND_GENERATION_PREFIX)
                .expect("generation prefix"),
            "the username and generation are independently sampled claims"
        );

        let directory = tempdir().expect("temporary directory");
        for (path, vector_path) in [
            (
                windows_bound_credential_locator_path(directory.path(), reference),
                recovery_vector_str(&vector["locator"], "relative_path"),
            ),
            (
                windows_bound_credential_delete_intent_path(directory.path(), reference),
                recovery_vector_str(&vector["delete_intent"], "relative_path"),
            ),
            (
                windows_bound_credential_delete_completion_path(directory.path(), reference),
                recovery_vector_str(&vector["delete_completion"], "relative_path"),
            ),
        ] {
            assert_eq!(
                path.strip_prefix(directory.path()).expect("relative path"),
                std::path::Path::new(vector_path)
            );
        }

        let locator = encode_windows_bound_credential_locator_for_claim(reference, &claim);
        assert!(locator.len() <= WINDOWS_BOUND_RECORD_MAXIMUM_BYTES);
        assert_eq!(
            locator,
            recovery_vector_str(&vector["locator"], "record").as_bytes()
        );
        assert_eq!(
            decode_windows_bound_credential_locator(&locator, reference)
                .expect("decode locator vector"),
            claim
        );

        let claimed_target = WindowsBoundDeleteTarget::Claimed(claim.clone());
        for (record_vector, magic, checksum_domain) in [
            (
                &vector["delete_intent"],
                WINDOWS_BOUND_DELETE_INTENT_MAGIC,
                WINDOWS_BOUND_DELETE_INTENT_CHECKSUM_DOMAIN,
            ),
            (
                &vector["delete_completion"],
                WINDOWS_BOUND_DELETE_COMPLETE_MAGIC,
                WINDOWS_BOUND_DELETE_COMPLETE_CHECKSUM_DOMAIN,
            ),
        ] {
            let claimed = encode_windows_bound_delete_record(
                magic,
                checksum_domain,
                reference,
                &claimed_target,
            );
            assert!(claimed.len() <= WINDOWS_BOUND_RECORD_MAXIMUM_BYTES);
            assert_eq!(
                claimed,
                recovery_vector_str(record_vector, "record").as_bytes()
            );
            assert_eq!(
                decode_windows_bound_delete_record(&claimed, magic, checksum_domain, reference)
                    .expect("decode claimed delete record vector"),
                claimed_target
            );

            let legacy = encode_windows_bound_delete_record(
                magic,
                checksum_domain,
                reference,
                &WindowsBoundDeleteTarget::Legacy,
            );
            assert!(legacy.len() <= WINDOWS_BOUND_RECORD_MAXIMUM_BYTES);
            assert_eq!(
                legacy,
                recovery_vector_str(record_vector, "legacy_record").as_bytes()
            );
            assert_eq!(
                decode_windows_bound_delete_record(&legacy, magic, checksum_domain, reference)
                    .expect("decode legacy delete record vector"),
                WindowsBoundDeleteTarget::Legacy
            );
        }

        let inner = recovery_vector_str(bound, "encoded_envelope");
        assert_eq!(
            inner.len() as u64,
            vector["random_slot_value"]["inner_envelope_utf8_bytes"]
                .as_u64()
                .expect("inner envelope byte length")
        );
        let wrapped = encode_windows_bound_credential_value(
            &claim.generation,
            &NativeCredential::new(inner.to_owned()),
        )
        .expect("encode random-slot value vector");
        assert_eq!(
            wrapped.expose(),
            recovery_vector_str(&vector["random_slot_value"], "encoded_value")
        );
        assert_eq!(
            decode_windows_bound_credential_value(&claim.generation, wrapped)
                .expect("decode random-slot value vector")
                .expose(),
            inner
        );

        let mut path_digest = Sha256::new();
        path_digest.update(WINDOWS_BOUND_LOCATOR_HASH_DOMAIN);
        path_digest.update((reference.len() as u64).to_be_bytes());
        path_digest.update(reference.as_bytes());
        assert_eq!(
            format!("{:x}", path_digest.finalize()),
            recovery_vector_str(vector, "reference_digest_sha256")
        );

        // The production Windows branch obtains these code units with
        // `canonicalize(...).as_os_str().encode_wide()`. This synthetic root
        // intentionally includes BMP and surrogate-pair characters so the
        // vector distinguishes the reviewed UTF-16LE framing from UTF-8.
        let root = recovery_vector_str(&vector["mutex"], "canonical_windows_data_root");
        let root_code_units = root.encode_utf16().collect::<Vec<_>>();
        let mut mutex_digest = Sha256::new();
        mutex_digest.update(WINDOWS_BOUND_MUTEX_HASH_DOMAIN);
        mutex_digest.update((root_code_units.len() as u64).to_be_bytes());
        for code_unit in root_code_units {
            mutex_digest.update(code_unit.to_le_bytes());
        }
        mutex_digest.update((reference.len() as u64).to_be_bytes());
        mutex_digest.update(reference.as_bytes());
        let mutex_digest = format!("{:x}", mutex_digest.finalize());
        assert_eq!(
            mutex_digest,
            recovery_vector_str(&vector["mutex"], "digest_sha256")
        );
        assert_eq!(
            format!("Global\\LorePia.ProviderCredential.Lock.v1.{mutex_digest}"),
            recovery_vector_str(&vector["mutex"], "name")
        );

        let file_vector = &vectors["windows_dpapi_credential_file"];
        let file_reference = recovery_vector_str(file_vector, "physical_reference");
        assert_eq!(file_reference, reference);
        assert!(validate_windows_raw_credential_reference(file_reference).is_err());
        let file_seed = WindowsBoundCredentialFileSeed {
            slot: recovery_vector_str(file_vector, "slot").to_owned(),
            generation: recovery_vector_str(file_vector, "generation").to_owned(),
        };
        assert_ne!(
            file_seed
                .slot
                .strip_prefix(WINDOWS_BOUND_FILE_SLOT_PREFIX)
                .expect("file slot prefix"),
            file_seed
                .generation
                .strip_prefix(WINDOWS_BOUND_GENERATION_PREFIX)
                .expect("file generation prefix")
        );
        let decode_hex = |encoded: &str| {
            assert_eq!(encoded.len() % 2, 0, "hex vector must have whole bytes");
            encoded
                .as_bytes()
                .chunks_exact(2)
                .map(|pair| {
                    u8::from_str_radix(std::str::from_utf8(pair).expect("ASCII hex vector"), 16)
                        .expect("lowercase hex vector")
                })
                .collect::<Vec<_>>()
        };

        let file_path = windows_bound_credential_file_path(
            directory.path(),
            file_reference,
            &file_seed.slot,
            &file_seed.generation,
        )
        .expect("DPAPI credential-file path vector");
        assert_eq!(
            file_path
                .strip_prefix(directory.path())
                .expect("relative DPAPI credential-file path"),
            std::path::Path::new(recovery_vector_str(&file_vector["path"], "relative_path"))
        );

        let plaintext = encode_windows_bound_file_plaintext(
            recovery_vector_str(file_vector, "credential_resource"),
            file_reference,
            &file_seed,
            &NativeCredential::new(inner.to_owned()),
        )
        .expect("encode DPAPI plaintext vector");
        assert_eq!(
            plaintext.as_slice(),
            decode_hex(recovery_vector_str(
                &file_vector["plaintext"],
                "encoded_hex"
            ))
        );
        assert_eq!(
            format!("{:x}", Sha256::digest(plaintext.as_slice())),
            recovery_vector_str(&file_vector["plaintext"], "sha256")
        );

        // DPAPI ciphertext itself is intentionally non-deterministic and bound
        // to a Windows user and machine. A public synthetic byte string freezes
        // only the surrounding owner codec and its domain-separated digest.
        let synthetic_ciphertext = decode_hex(recovery_vector_str(
            &file_vector["synthetic_ciphertext"],
            "encoded_hex",
        ));
        let (file_claim, file_record) = encode_windows_bound_credential_file_record(
            file_reference,
            &file_seed,
            &synthetic_ciphertext,
        )
        .expect("encode DPAPI outer-record vector");
        assert_eq!(
            file_record,
            decode_hex(recovery_vector_str(&file_vector["record"], "encoded_hex"))
        );
        assert_eq!(
            file_claim.file_record_sha256.as_deref(),
            Some(recovery_vector_str(&file_vector["record"], "digest_sha256"))
        );
        assert_eq!(
            decode_windows_bound_credential_file_record(file_reference, &file_claim, &file_record,)
                .expect("decode DPAPI outer-record vector")
                .as_slice(),
            synthetic_ciphertext
        );

        let file_locator =
            encode_windows_bound_credential_locator_for_claim(file_reference, &file_claim);
        assert_eq!(
            file_locator,
            recovery_vector_str(&file_vector["locator"], "record").as_bytes()
        );
        assert_eq!(
            decode_windows_bound_credential_locator(&file_locator, file_reference)
                .expect("decode DPAPI locator vector"),
            file_claim
        );
        let file_target = WindowsBoundDeleteTarget::Claimed(file_claim.clone());
        for (record_vector, magic, checksum_domain) in [
            (
                &file_vector["delete_intent"],
                WINDOWS_BOUND_DELETE_INTENT_V2_MAGIC,
                WINDOWS_BOUND_DELETE_INTENT_V2_CHECKSUM_DOMAIN,
            ),
            (
                &file_vector["delete_completion"],
                WINDOWS_BOUND_DELETE_COMPLETE_V2_MAGIC,
                WINDOWS_BOUND_DELETE_COMPLETE_V2_CHECKSUM_DOMAIN,
            ),
        ] {
            let record = encode_windows_bound_delete_record(
                magic,
                checksum_domain,
                file_reference,
                &file_target,
            );
            assert_eq!(
                record,
                recovery_vector_str(record_vector, "record").as_bytes()
            );
            assert_eq!(
                decode_windows_bound_delete_record_v2(
                    &record,
                    magic,
                    checksum_domain,
                    file_reference,
                )
                .expect("decode DPAPI delete record vector"),
                file_target
            );
        }

        // This mirrors the Windows-only `encode_wide` branch with an explicit
        // synthetic UTF-16 oracle, without claiming that DPAPI ran on this host.
        let root = recovery_vector_str(file_vector, "canonical_windows_data_root");
        let root_code_units = root.encode_utf16().collect::<Vec<_>>();
        let mut entropy = WINDOWS_BOUND_FILE_ENTROPY_DOMAIN.to_vec();
        entropy.extend_from_slice(&(root_code_units.len() as u64).to_be_bytes());
        for code_unit in root_code_units {
            entropy.extend_from_slice(&code_unit.to_le_bytes());
        }
        for component in [
            recovery_vector_str(file_vector, "credential_resource").as_bytes(),
            file_reference.as_bytes(),
            file_seed.slot.as_bytes(),
            file_seed.generation.as_bytes(),
        ] {
            append_windows_bound_frame(&mut entropy, component);
        }
        assert_eq!(
            entropy,
            decode_hex(recovery_vector_str(&file_vector["entropy"], "encoded_hex"))
        );
        assert_eq!(
            format!("{:x}", Sha256::digest(&entropy)),
            recovery_vector_str(&file_vector["entropy"], "sha256")
        );
    }

    #[test]
    fn windows_bound_store_claim_race_has_zero_native_ops_and_preserves_every_winner() {
        let directory = tempdir().expect("temporary directory");
        let data_root = directory.path();
        let reference = "lpc2-logical-operation-a";
        let claimed_username = "lpcw1-11111111-1111-4111-8111-111111111111";
        drop(
            claim_windows_bound_credential_locator(data_root, reference, claimed_username)
                .expect("first locator claim"),
        );
        let locator_path = windows_bound_credential_locator_path(data_root, reference);
        let locator_before = std::fs::read(&locator_path).expect("winning locator bytes");
        let backend = RefCell::new(FakeAddOnlyCredentialBackend {
            values: HashMap::from([
                (reference.to_owned(), "deterministic-winner".to_owned()),
                (claimed_username.to_owned(), "claimed-winner".to_owned()),
            ]),
            ..FakeAddOnlyCredentialBackend::default()
        });

        let error = store_bound_with_fake(
            data_root,
            reference,
            "lpcw1-22222222-2222-4222-8222-222222222222",
            &backend,
            &NativeCredential::new("new-bound-envelope".to_owned()),
        )
        .expect_err("an existing durable claim must require recovery");

        let backend = backend.into_inner();
        assert_eq!(error.code(), PlatformErrorCode::CredentialRecoveryRequired);
        assert_eq!(
            backend.values.get(reference).map(String::as_str),
            Some("deterministic-winner")
        );
        assert_eq!(
            backend.values.get(claimed_username).map(String::as_str),
            Some("claimed-winner")
        );
        assert_eq!(backend.add_calls, 0);
        assert_eq!(backend.read_calls, 0);
        assert_eq!(backend.remove_calls, 0);
        assert_eq!(
            std::fs::read(&locator_path).expect("retained locator bytes"),
            locator_before
        );
    }

    #[test]
    fn windows_bound_store_post_add_race_never_rolls_back_the_winning_item() {
        let directory = tempdir().expect("temporary directory");
        let reference = "lpc2-logical-operation-b";
        let username = "lpcw1-33333333-3333-4333-8333-333333333333";
        let backend = RefCell::new(FakeAddOnlyCredentialBackend {
            injected_value_after_add: Some("post-add-winning-envelope".to_owned()),
            ..FakeAddOnlyCredentialBackend::default()
        });

        let error = store_bound_with_fake(
            directory.path(),
            reference,
            username,
            &backend,
            &NativeCredential::new("new-bound-envelope".to_owned()),
        )
        .expect_err("post-add mismatch must require recovery");

        let backend = backend.into_inner();
        assert_eq!(error.code(), PlatformErrorCode::CredentialRecoveryRequired);
        assert_eq!(
            backend.values.get(username).map(String::as_str),
            Some("post-add-winning-envelope")
        );
        assert_eq!(backend.add_calls, 1);
        assert_eq!(backend.read_calls, 3);
        assert_eq!(backend.remove_calls, 0);
        assert!(windows_bound_credential_locator_path(directory.path(), reference).exists());
    }

    #[test]
    fn windows_bound_store_random_username_collision_never_calls_add_or_changes_winner() {
        let directory = tempdir().expect("temporary directory");
        let reference = "lpc2-random-username-collision";
        let username = "lpcw1-88888888-8888-4888-8888-888888888888";
        let backend = RefCell::new(FakeAddOnlyCredentialBackend {
            values: HashMap::from([(username.to_owned(), "random-slot-winner".to_owned())]),
            ..FakeAddOnlyCredentialBackend::default()
        });

        let error = store_bound_with_fake(
            directory.path(),
            reference,
            username,
            &backend,
            &NativeCredential::new("new-bound-envelope".to_owned()),
        )
        .expect_err("random UUID collision must fail before PasswordVault.Add");

        let backend = backend.into_inner();
        assert_eq!(error.code(), PlatformErrorCode::CredentialRecoveryRequired);
        assert_eq!(
            backend.values.get(username).map(String::as_str),
            Some("random-slot-winner")
        );
        assert_eq!(backend.read_calls, 2);
        assert_eq!(backend.add_calls, 0);
        assert_eq!(backend.remove_calls, 0);
        let locator_path = windows_bound_credential_locator_path(directory.path(), reference);
        assert_eq!(
            std::fs::read(&locator_path).expect("collision locator"),
            encode_windows_bound_credential_locator(reference, username)
        );
    }

    #[test]
    fn windows_bound_store_rejects_populated_legacy_slot_before_claim_or_add() {
        let directory = tempdir().expect("temporary directory");
        let reference = "lpc2-populated-legacy-slot";
        let username = "lpcw1-89898989-8989-4989-8989-898989898989";
        let backend = RefCell::new(FakeAddOnlyCredentialBackend {
            values: HashMap::from([(reference.to_owned(), "legacy-winner".to_owned())]),
            ..FakeAddOnlyCredentialBackend::default()
        });

        let error = store_bound_with_fake(
            directory.path(),
            reference,
            username,
            &backend,
            &NativeCredential::new("new-bound-envelope".to_owned()),
        )
        .expect_err("new install must not orphan or shadow an old deterministic item");

        let backend = backend.into_inner();
        assert_eq!(error.code(), PlatformErrorCode::CredentialRecoveryRequired);
        assert_eq!(
            backend.values.get(reference).map(String::as_str),
            Some("legacy-winner")
        );
        assert_eq!(backend.read_calls, 1);
        assert_eq!(backend.add_calls, 0);
        assert_eq!(backend.remove_calls, 0);
        assert!(!windows_bound_credential_locator_path(directory.path(), reference).exists());
    }

    #[test]
    fn windows_bound_store_add_error_after_side_effect_never_removes_or_rolls_back() {
        let directory = tempdir().expect("temporary directory");
        let reference = "lpc2-add-error-after-side-effect";
        let username = "lpcw1-99999999-9999-4999-8999-999999999999";
        let values = RefCell::new(HashMap::<String, String>::new());
        let add_calls = RefCell::new(0_u32);
        let read_calls = RefCell::new(0_u32);
        let remove_calls = RefCell::new(0_u32);
        let expected = NativeCredential::new("attempted-envelope".to_owned());

        let error = store_prevalidated_windows_bound_credential_with(
            directory.path(),
            reference,
            username,
            &expected,
            |username| {
                *add_calls.borrow_mut() += 1;
                values
                    .borrow_mut()
                    .insert(username.to_owned(), "post-error-winner".to_owned());
                Err(PlatformError::new(PlatformErrorCode::CredentialUnavailable))
            },
            |username| {
                *read_calls.borrow_mut() += 1;
                Ok(values
                    .borrow()
                    .get(username)
                    .cloned()
                    .map(NativeCredential::new))
            },
            || {
                *read_calls.borrow_mut() += 1;
                Ok(None)
            },
        )
        .expect_err("side-effecting Add error has an unknown outcome");

        assert_eq!(error.code(), PlatformErrorCode::CredentialRecoveryRequired);
        assert_eq!(
            values.borrow().get(username).map(String::as_str),
            Some("post-error-winner")
        );
        assert_eq!(*add_calls.borrow(), 1);
        assert_eq!(*read_calls.borrow(), 2);
        assert_eq!(*remove_calls.borrow(), 0);
        let locator_path = windows_bound_credential_locator_path(directory.path(), reference);
        assert_eq!(
            std::fs::read(&locator_path).expect("side-effect locator"),
            encode_windows_bound_credential_locator(reference, username)
        );
    }

    #[test]
    fn windows_bound_store_adds_and_verifies_one_fresh_random_slot() {
        let directory = tempdir().expect("temporary directory");
        let reference = "lpc2-logical-operation-c";
        let username = "lpcw1-44444444-4444-4444-8444-444444444444";
        let backend = RefCell::new(FakeAddOnlyCredentialBackend::default());

        store_bound_with_fake(
            directory.path(),
            reference,
            username,
            &backend,
            &NativeCredential::new("new-bound-envelope".to_owned()),
        )
        .expect("missing bound item can be installed");

        let backend = backend.into_inner();
        assert_eq!(
            backend.values.get(username).map(String::as_str),
            Some("new-bound-envelope")
        );
        assert!(!backend.values.contains_key(reference));
        assert_eq!(backend.add_calls, 1);
        assert_eq!(backend.read_calls, 3);
        assert_eq!(backend.remove_calls, 0);
    }

    #[test]
    fn windows_bound_store_external_winner_between_preflight_and_add_is_never_overwritten() {
        let directory = tempdir().expect("temporary directory");
        let reference = "lpc2-external-between-preflight-and-add";
        let resource = "LorePia.ProviderCredential";
        let seed = WindowsBoundCredentialFileSeed {
            slot: "lpcf1-46464646-4646-4646-8646-464646464646".to_owned(),
            generation: "lpcg1-47474747-4747-4747-8747-474747474747".to_owned(),
        };
        let attempted = NativeCredential::new("attempted-envelope".to_owned());
        let (claim, attempted_record) = prepare_windows_bound_credential_file_with(
            directory.path(),
            resource,
            reference,
            &seed,
            &attempted,
            |plaintext, _| Ok(zeroize::Zeroizing::new(plaintext.to_vec())),
        )
        .expect("prepare attempted encrypted record");
        let (_, winner_record) = encode_windows_bound_credential_file_record(
            reference,
            &seed,
            b"external-winner-ciphertext",
        )
        .expect("prepare external winner record");
        let publish_calls = RefCell::new(0_u32);

        let error = super::store_prevalidated_windows_bound_credential_claim_with(
            directory.path(),
            reference,
            &claim,
            &attempted,
            |claimed| {
                *publish_calls.borrow_mut() += 1;
                publish_windows_bound_credential_file_with(
                    directory.path(),
                    reference,
                    claimed,
                    &attempted_record,
                    |_source, destination| {
                        // A noncooperating same-user writer wins after the
                        // missing preflight. The no-replace publication must
                        // preserve these bytes rather than behaving like
                        // PasswordVault.Add's replacement-capable upsert.
                        std::fs::write(destination, &winner_record)
                            .expect("publish external winner");
                        Err(PlatformError::new(
                            PlatformErrorCode::CredentialRecoveryRequired,
                        ))
                    },
                )
            },
            |claimed| {
                read_windows_bound_credential_file_value_with(
                    directory.path(),
                    resource,
                    reference,
                    claimed,
                    false,
                    |ciphertext, _, _| Ok(zeroize::Zeroizing::new(ciphertext.to_vec())),
                )
                .map(|stored| stored.map(|(_, value)| value))
            },
            || Ok(None),
        )
        .expect_err("external winner must make the install outcome uncertain");

        assert_eq!(error.code(), PlatformErrorCode::CredentialRecoveryRequired);
        assert_eq!(*publish_calls.borrow(), 1);
        let final_path = windows_bound_credential_file_path(
            directory.path(),
            reference,
            &seed.slot,
            &seed.generation,
        )
        .expect("credential file path");
        assert_eq!(
            std::fs::read(final_path).expect("preserved external winner"),
            winner_record
        );
    }

    #[test]
    fn windows_dpapi_bound_file_store_round_trip_succeeds_without_password_vault_add() {
        let directory = tempdir().expect("temporary directory");
        let resource = "LorePia.ProviderCredential";
        let reference = "lpc2-dpapi-file-success";
        let seed = WindowsBoundCredentialFileSeed {
            slot: "lpcf1-48484848-4848-4848-8848-484848484848".to_owned(),
            generation: "lpcg1-49494949-4949-4949-8949-494949494949".to_owned(),
        };
        let value = NativeCredential::new("synthetic-bound-envelope".to_owned());
        let transform = |bytes: &[u8]| bytes.iter().map(|byte| byte ^ 0xa5).collect::<Vec<_>>();
        let (claim, record) = prepare_windows_bound_credential_file_with(
            directory.path(),
            resource,
            reference,
            &seed,
            &value,
            |plaintext, _| Ok(zeroize::Zeroizing::new(transform(plaintext))),
        )
        .expect("prepare encrypted file");

        super::store_prevalidated_windows_bound_credential_claim_with(
            directory.path(),
            reference,
            &claim,
            &value,
            |claimed| {
                super::publish_windows_bound_credential_file(
                    directory.path(),
                    reference,
                    claimed,
                    &record,
                )
            },
            |claimed| {
                read_windows_bound_credential_file_value_with(
                    directory.path(),
                    resource,
                    reference,
                    claimed,
                    false,
                    |ciphertext, _, _| Ok(zeroize::Zeroizing::new(transform(ciphertext))),
                )
                .map(|stored| stored.map(|(_, value)| value))
            },
            || Ok(None),
        )
        .expect("add-only encrypted file install");

        let path = windows_bound_credential_file_path(
            directory.path(),
            reference,
            &seed.slot,
            &seed.generation,
        )
        .expect("credential file path");
        let bytes = std::fs::read(path).expect("encrypted credential record");
        assert!(
            !bytes
                .windows(value.expose().len())
                .any(|window| window == value.expose().as_bytes())
        );
    }

    #[test]
    fn windows_dpapi_locator_without_file_is_settled_only_by_explicit_delete_tombstone() {
        let directory = tempdir().expect("temporary directory");
        let resource = "LorePia.ProviderCredential";
        let reference = "lpc2-dpapi-pre-file-crash";
        let seed = WindowsBoundCredentialFileSeed {
            slot: "lpcf1-50505050-5050-4050-8050-505050505050".to_owned(),
            generation: "lpcg1-51515151-5151-4151-8151-515151515151".to_owned(),
        };
        let value = NativeCredential::new("crash-cutpoint-envelope".to_owned());
        let (claim, _) = prepare_windows_bound_credential_file_with(
            directory.path(),
            resource,
            reference,
            &seed,
            &value,
            |plaintext, _| Ok(zeroize::Zeroizing::new(plaintext.to_vec())),
        )
        .expect("prepare claim");
        drop(
            claim_windows_bound_credential_locator_for_claim(directory.path(), reference, &claim)
                .expect("publish durable locator before simulated crash"),
        );

        let read_error = super::read_windows_bound_credential_claim_with(
            directory.path(),
            reference,
            |claimed| {
                read_windows_bound_credential_file_value_with(
                    directory.path(),
                    resource,
                    reference,
                    claimed,
                    false,
                    |ciphertext, _, _| Ok(zeroize::Zeroizing::new(ciphertext.to_vec())),
                )
                .map(|stored| stored.map(|(_, value)| value))
            },
            || panic!("v2 locator must never fall back to PasswordVault"),
        )
        .expect_err("locator without encrypted file is recovery-required");
        assert_eq!(
            read_error.code(),
            PlatformErrorCode::CredentialRecoveryRequired
        );

        let removals = RefCell::new(0_u32);
        delete_windows_bound_credential_claim_with(
            directory.path(),
            reference,
            |claimed| {
                read_windows_bound_credential_file_value_with(
                    directory.path(),
                    resource,
                    reference,
                    claimed,
                    false,
                    |ciphertext, _, _| Ok(zeroize::Zeroizing::new(ciphertext.to_vec())),
                )
                .map(|stored| stored.map(|_| ()))
            },
            |_, ()| {
                *removals.borrow_mut() += 1;
                Ok(())
            },
            || panic!("claimed v2 delete must not inspect PasswordVault fallback"),
            |()| panic!("claimed v2 delete must not mutate PasswordVault fallback"),
        )
        .expect("explicit delete settles a pre-file crash without native mutation");
        assert_eq!(*removals.borrow(), 0);
        assert!(windows_bound_credential_locator_path(directory.path(), reference).exists());
        assert!(windows_bound_credential_delete_intent_path(directory.path(), reference).exists());
        assert!(
            windows_bound_credential_delete_completion_path(directory.path(), reference).exists()
        );
        let reopened = super::read_windows_bound_credential_claim_with(
            directory.path(),
            reference,
            |_| panic!("completed tombstone must not read the encrypted file"),
            || panic!("completed tombstone must not read PasswordVault"),
        )
        .expect("completed tombstone is observable");
        assert!(reopened.is_none());
    }

    #[test]
    fn windows_dpapi_tampered_encrypted_record_is_preserved_and_fails_closed() {
        let directory = tempdir().expect("temporary directory");
        let resource = "LorePia.ProviderCredential";
        let reference = "lpc2-dpapi-tampered-record";
        let seed = WindowsBoundCredentialFileSeed {
            slot: "lpcf1-52525252-5252-4252-8252-525252525252".to_owned(),
            generation: "lpcg1-53535353-5353-4353-8353-535353535353".to_owned(),
        };
        let value = NativeCredential::new("tamper-target-envelope".to_owned());
        let (claim, record) = prepare_windows_bound_credential_file_with(
            directory.path(),
            resource,
            reference,
            &seed,
            &value,
            |plaintext, _| Ok(zeroize::Zeroizing::new(plaintext.to_vec())),
        )
        .expect("prepare encrypted record");
        drop(
            claim_windows_bound_credential_locator_for_claim(directory.path(), reference, &claim)
                .expect("publish locator"),
        );
        publish_windows_bound_credential_file(directory.path(), reference, &claim, &record)
            .expect("publish encrypted record");
        let path = windows_bound_credential_file_path(
            directory.path(),
            reference,
            &seed.slot,
            &seed.generation,
        )
        .expect("credential file path");
        let mut tampered = std::fs::read(&path).expect("credential record");
        let last = tampered.last_mut().expect("nonempty record");
        *last ^= 1;
        std::fs::write(&path, &tampered).expect("tamper fixture");

        let error = super::read_windows_bound_credential_claim_with(
            directory.path(),
            reference,
            |claimed| {
                read_windows_bound_credential_file_value_with(
                    directory.path(),
                    resource,
                    reference,
                    claimed,
                    false,
                    |ciphertext, _, _| Ok(zeroize::Zeroizing::new(ciphertext.to_vec())),
                )
                .map(|stored| stored.map(|(_, value)| value))
            },
            || panic!("v2 tamper must not fall back to PasswordVault"),
        )
        .expect_err("tampered record must fail closed");
        assert_eq!(error.code(), PlatformErrorCode::CredentialRecoveryRequired);
        assert_eq!(
            std::fs::read(path).expect("preserved tamper evidence"),
            tampered
        );
    }

    #[test]
    fn windows_dpapi_delete_retry_preserves_post_delete_different_record_winner() {
        let directory = tempdir().expect("temporary directory");
        let resource = "LorePia.ProviderCredential";
        let reference = "lpc2-dpapi-delete-winner";
        let seed = WindowsBoundCredentialFileSeed {
            slot: "lpcf1-54545454-5454-4454-8454-545454545454".to_owned(),
            generation: "lpcg1-55555555-5555-4555-8555-555555555555".to_owned(),
        };
        let value = NativeCredential::new("original-file-envelope".to_owned());
        let (claim, record) = prepare_windows_bound_credential_file_with(
            directory.path(),
            resource,
            reference,
            &seed,
            &value,
            |plaintext, _| Ok(zeroize::Zeroizing::new(plaintext.to_vec())),
        )
        .expect("prepare encrypted record");
        drop(
            claim_windows_bound_credential_locator_for_claim(directory.path(), reference, &claim)
                .expect("publish locator"),
        );
        publish_windows_bound_credential_file(directory.path(), reference, &claim, &record)
            .expect("publish encrypted record");
        let path = windows_bound_credential_file_path(
            directory.path(),
            reference,
            &seed.slot,
            &seed.generation,
        )
        .expect("credential file path");
        let (_, winner_record) = encode_windows_bound_credential_file_record(
            reference,
            &seed,
            b"different-post-delete-winner",
        )
        .expect("winner record");
        let removals = RefCell::new(0_u32);
        let read = |claimed: &WindowsBoundCredentialClaim| {
            read_windows_bound_credential_file_value_with(
                directory.path(),
                resource,
                reference,
                claimed,
                false,
                |ciphertext, _, _| Ok(zeroize::Zeroizing::new(ciphertext.to_vec())),
            )
            .map(|stored| stored.map(|_| ()))
        };

        let first = delete_windows_bound_credential_claim_with(
            directory.path(),
            reference,
            &read,
            |_, ()| {
                *removals.borrow_mut() += 1;
                std::fs::remove_file(&path).expect("remove verified original fixture");
                std::fs::write(&path, &winner_record).expect("publish post-delete winner");
                Ok(())
            },
            || panic!("v2 delete must not inspect PasswordVault fallback"),
            |()| panic!("v2 delete must not mutate PasswordVault fallback"),
        )
        .expect_err("post-delete winner prevents completion");
        assert_eq!(first.code(), PlatformErrorCode::CredentialRecoveryRequired);
        assert_eq!(*removals.borrow(), 1);

        let second = delete_windows_bound_credential_claim_with(
            directory.path(),
            reference,
            &read,
            |_, ()| {
                *removals.borrow_mut() += 1;
                Ok(())
            },
            || panic!("v2 retry must not inspect PasswordVault fallback"),
            |()| panic!("v2 retry must not mutate PasswordVault fallback"),
        )
        .expect_err("mismatched winner remains recovery-required on retry");
        assert_eq!(second.code(), PlatformErrorCode::CredentialRecoveryRequired);
        assert_eq!(*removals.borrow(), 1);
        assert_eq!(
            std::fs::read(path).expect("preserved post-delete winner"),
            winner_record
        );
    }

    #[test]
    fn windows_locator_create_new_allows_exactly_one_filesystem_claim() {
        let directory = tempdir().expect("temporary directory");
        let data_root = Arc::new(directory.path().to_path_buf());
        let barrier = Arc::new(Barrier::new(8));
        let mut threads = Vec::new();
        for index in 0..8_u8 {
            let data_root = Arc::clone(&data_root);
            let barrier = Arc::clone(&barrier);
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                claim_windows_bound_credential_locator(
                    data_root.as_path(),
                    "lpc2-racing-logical-operation",
                    &format!("lpcw1-00000000-0000-4000-8000-{index:012}"),
                )
                .map(drop)
            }));
        }
        let outcomes = threads
            .into_iter()
            .map(|thread| thread.join().expect("claim thread"))
            .collect::<Vec<_>>();
        assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1);
        assert!(
            outcomes
                .iter()
                .filter(|outcome| outcome.is_err())
                .all(|outcome| {
                    outcome.as_ref().expect_err("failed claim").code()
                        == PlatformErrorCode::CredentialRecoveryRequired
                })
        );
    }

    #[test]
    fn windows_locator_partial_staging_write_never_publishes_final() {
        let directory = tempdir().expect("temporary directory");
        let reference = "lpc2-partial-staging-write";
        let username = "lpcw1-12121212-1212-4212-8212-121212121212";

        let error = claim_windows_bound_credential_locator_with_writer(
            directory.path(),
            reference,
            username,
            |file, bytes| {
                file.write_all(&bytes[..bytes.len() / 2])?;
                file.sync_all()?;
                Err(std::io::Error::other("injected staged write failure"))
            },
        )
        .expect_err("partial staging record must never become authoritative");

        assert_eq!(error.code(), PlatformErrorCode::CredentialRecoveryRequired);
        assert!(!windows_bound_credential_locator_path(directory.path(), reference).exists());
        let staged = std::fs::read_dir(directory.path().join(WINDOWS_BOUND_LOCATOR_DIRECTORY))
            .expect("locator directory")
            .map(|entry| entry.expect("staging entry").path())
            .collect::<Vec<_>>();
        assert_eq!(staged.len(), 1);
        assert!(
            staged[0]
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("lpcw-stage-v1-"))
        );
        assert!(
            staged[0]
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("tmp"))
        );
    }

    #[test]
    fn windows_locator_publish_error_after_commit_reconciles_exact_record_without_rollback() {
        let directory = tempdir().expect("temporary directory");
        let reference = "lpc2-publish-error-after-commit";
        let username = "lpcw1-13131313-1313-4313-8313-131313131313";
        let path = windows_bound_credential_locator_path(directory.path(), reference);
        let bytes = encode_windows_bound_credential_locator(reference, username);

        drop(
            publish_windows_bound_record_with(
                directory.path(),
                &path,
                &bytes,
                |file, bytes| file.write_all(bytes).and_then(|()| file.sync_all()),
                |source, destination| {
                    std::fs::hard_link(source, destination).expect("injected committed publish");
                    Err(PlatformError::new(PlatformErrorCode::CredentialUnavailable))
                },
            )
            .expect("exact committed final reconciles an ambiguous publish result"),
        );

        assert_eq!(std::fs::read(path).expect("retained final locator"), bytes);
    }

    #[test]
    fn windows_locator_no_replace_publish_preserves_different_race_winner() {
        let directory = tempdir().expect("temporary directory");
        let reference = "lpc2-no-replace-race-winner";
        let winner = "lpcw1-14141414-1414-4414-8414-141414141414";
        let attempted = "lpcw1-15151515-1515-4515-8515-151515151515";
        drop(
            claim_windows_bound_credential_locator(directory.path(), reference, winner)
                .expect("winning locator"),
        );
        let path = windows_bound_credential_locator_path(directory.path(), reference);
        let winner_bytes = std::fs::read(&path).expect("winner bytes");
        let attempted_bytes = encode_windows_bound_credential_locator(reference, attempted);

        let error = publish_windows_bound_record_with(
            directory.path(),
            &path,
            &attempted_bytes,
            |file, bytes| file.write_all(bytes).and_then(|()| file.sync_all()),
            |source, destination| {
                std::fs::hard_link(source, destination)
                    .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))
            },
        )
        .expect_err("a different final record must never be replaced or accepted");

        assert_eq!(error.code(), PlatformErrorCode::CredentialRecoveryRequired);
        assert_eq!(std::fs::read(path).expect("preserved winner"), winner_bytes);
    }

    #[test]
    fn windows_named_mutex_is_global_versioned_and_hashes_root_and_reference() {
        let directory = tempdir().expect("temporary directory");
        let other_directory = tempdir().expect("other temporary directory");
        let first = windows_bound_credential_mutex_name(directory.path(), "lpc2-mutex-alpha")
            .expect("first mutex name");
        let repeated = windows_bound_credential_mutex_name(directory.path(), "lpc2-mutex-alpha")
            .expect("repeated mutex name");
        let second = windows_bound_credential_mutex_name(directory.path(), "lpc2-mutex-beta")
            .expect("second mutex name");
        let other_root =
            windows_bound_credential_mutex_name(other_directory.path(), "lpc2-mutex-alpha")
                .expect("other root mutex name");

        assert_eq!(first, repeated);
        assert_ne!(first, second);
        assert_ne!(first, other_root);
        assert!(first.starts_with("Global\\LorePia.ProviderCredential.Lock.v1."));
        assert!(!first.contains("lpc2-mutex-alpha"));
        assert_eq!(
            first.len(),
            "Global\\LorePia.ProviderCredential.Lock.v1.".len() + 64
        );
    }

    #[test]
    fn windows_random_file_slot_generation_is_v4_and_collision_resistant() {
        let claims = (0..4096)
            .map(|_| new_windows_bound_credential_file_seed())
            .collect::<Vec<_>>();
        let usernames = claims
            .iter()
            .map(|claim| claim.slot.as_str())
            .collect::<std::collections::HashSet<_>>();
        let generations = claims
            .iter()
            .map(|claim| claim.generation.as_str())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(usernames.len(), 4096);
        assert_eq!(generations.len(), 4096);
        assert!(
            claims
                .iter()
                .all(|claim| claim.slot[WINDOWS_BOUND_FILE_SLOT_PREFIX.len()..]
                    != claim.generation[WINDOWS_BOUND_GENERATION_PREFIX.len()..])
        );
        assert!(
            usernames.iter().all(|slot| validate_windows_bound_uuid(
                slot,
                WINDOWS_BOUND_FILE_SLOT_PREFIX
            )
            .is_ok())
        );
        for noncanonical in [
            "lpcf1-20202020202040208020202020202020",
            "lpcf1-20202020-2020-4020-8020-20202020202A",
        ] {
            assert!(
                validate_windows_bound_uuid(noncanonical, WINDOWS_BOUND_FILE_SLOT_PREFIX).is_err()
            );
        }
    }

    #[test]
    fn windows_raw_operations_reject_reserved_bound_physical_reference_before_native_access() {
        let reserved = format!(
            "{WINDOWS_BOUND_PHYSICAL_REFERENCE_PREFIX}{}",
            "ab".repeat(32)
        );
        for operation in ["status", "read", "store", "delete"] {
            let native_calls = RefCell::new(0_u32);
            let error = with_validated_windows_raw_credential_reference(&reserved, || {
                *native_calls.borrow_mut() += 1;
                Ok(())
            })
            .expect_err("reserved bound physical reference must be rejected");
            assert_eq!(error.code(), PlatformErrorCode::InvalidInput, "{operation}");
            assert_eq!(*native_calls.borrow(), 0, "{operation}");
        }

        for legacy_control in [
            "lpc2-legacy-profile".to_owned(),
            format!("lpc2-{}", "AB".repeat(32)),
            format!("lpc2-{}x", "ab".repeat(32)),
        ] {
            assert!(validate_windows_raw_credential_reference(&legacy_control).is_ok());
        }
    }

    fn windows_dpapi_test_stage_record(
        directory: &std::path::Path,
        resource: &str,
        reference: &str,
        index: u128,
    ) -> Vec<u8> {
        let seed = WindowsBoundCredentialFileSeed {
            slot: format!(
                "{WINDOWS_BOUND_FILE_SLOT_PREFIX}{}",
                uuid::Uuid::from_u128(index | (4_u128 << 76) | (2_u128 << 62))
            ),
            generation: format!(
                "{WINDOWS_BOUND_GENERATION_PREFIX}{}",
                uuid::Uuid::from_u128((index + 10_000) | (4_u128 << 76) | (2_u128 << 62))
            ),
        };
        let value = NativeCredential::new(format!("synthetic-stage-envelope-{index}"));
        let (_, record) = prepare_windows_bound_credential_file_with(
            directory,
            resource,
            reference,
            &seed,
            &value,
            |plaintext, _| Ok(zeroize::Zeroizing::new(plaintext.to_vec())),
        )
        .expect("prepare synthetic encrypted stage");
        record
    }

    fn windows_dpapi_test_stage_path(directory: &std::path::Path, uuid: uuid::Uuid) -> PathBuf {
        directory
            .join(WINDOWS_BOUND_LOCATOR_DIRECTORY)
            .join(format!(
                "{WINDOWS_BOUND_FILE_STAGE_PREFIX}{uuid}{WINDOWS_BOUND_FILE_STAGE_SUFFIX}"
            ))
    }

    #[cfg(unix)]
    fn windows_dpapi_test_open_stage(
        path: &std::path::Path,
    ) -> crate::PlatformResult<std::fs::File> {
        use std::os::unix::fs::OpenOptionsExt;

        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))
    }

    #[cfg(unix)]
    fn windows_dpapi_test_delete_stage(
        path: &std::path::Path,
        file: &std::fs::File,
    ) -> crate::PlatformResult<()> {
        use std::os::unix::fs::MetadataExt;

        let opened = file
            .metadata()
            .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
        let current = std::fs::symlink_metadata(path)
            .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
        if opened.dev() != current.dev() || opened.ino() != current.ino() {
            return Err(PlatformError::new(
                PlatformErrorCode::CredentialRecoveryRequired,
            ));
        }
        std::fs::remove_file(path)
            .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))
    }

    fn windows_dpapi_mark_stage_old(path: &std::path::Path, now: std::time::SystemTime) {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("open stage for timestamp");
        file.set_times(
            std::fs::FileTimes::new()
                .set_modified(now - Duration::from_hours(48))
                .set_accessed(now - Duration::from_hours(48)),
        )
        .expect("age synthetic stage");
    }

    #[cfg(unix)]
    #[test]
    fn windows_dpapi_stage_gc_deletes_only_old_valid_encrypted_stages() {
        let directory = tempdir().expect("temporary directory");
        let resource = "LorePia.ProviderCredential";
        let reference = "lpc2-stage-gc-old";
        let locator_root = super::ensure_windows_bound_locator_directory(directory.path())
            .expect("locator directory");
        let now = std::time::SystemTime::now();
        let stage = windows_dpapi_test_stage_path(
            directory.path(),
            uuid::Uuid::from_u128(0x4444 | (4_u128 << 76) | (2_u128 << 62)),
        );
        std::fs::write(
            &stage,
            windows_dpapi_test_stage_record(directory.path(), resource, reference, 0x4444),
        )
        .expect("old valid encrypted stage");
        windows_dpapi_mark_stage_old(&stage, now);
        let authoritative = [
            locator_root.join("lpcw-locator-v1-authoritative.record"),
            locator_root.join("lpcw-delete-intent-v1-authoritative.record"),
            locator_root.join("lpcw-delete-complete-v1-authoritative.record"),
            locator_root.join("lpcw-credential-v2-authoritative.blob"),
        ];
        for path in &authoritative {
            std::fs::write(path, b"must-never-be-gc-targeted").expect("authoritative fixture");
        }

        let outcome = cleanup_windows_bound_credential_staging_with(
            directory.path(),
            resource,
            now,
            windows_dpapi_test_open_stage,
            windows_dpapi_test_delete_stage,
            |ciphertext, _, _| Ok(zeroize::Zeroizing::new(ciphertext.to_vec())),
        );
        assert_eq!(outcome.deleted, 1);
        assert!(!stage.exists());
        assert!(authoritative.iter().all(|path| path.exists()));
    }

    #[cfg(unix)]
    #[test]
    fn windows_dpapi_stage_gc_preserves_young_in_use_symlink_unknown_and_tampered_entries() {
        let directory = tempdir().expect("temporary directory");
        let resource = "LorePia.ProviderCredential";
        let reference = "lpc2-stage-gc-preserve";
        let locator_root = super::ensure_windows_bound_locator_directory(directory.path())
            .expect("locator directory");
        let now = std::time::SystemTime::now();
        let valid_record =
            windows_dpapi_test_stage_record(directory.path(), resource, reference, 0x5555);
        let young = windows_dpapi_test_stage_path(
            directory.path(),
            uuid::Uuid::from_u128(0x5555 | (4_u128 << 76) | (2_u128 << 62)),
        );
        std::fs::write(&young, &valid_record).expect("young stage");
        let in_use = windows_dpapi_test_stage_path(
            directory.path(),
            uuid::Uuid::from_u128(0x5556 | (4_u128 << 76) | (2_u128 << 62)),
        );
        std::fs::write(&in_use, &valid_record).expect("in-use stage");
        windows_dpapi_mark_stage_old(&in_use, now);
        let tampered = windows_dpapi_test_stage_path(
            directory.path(),
            uuid::Uuid::from_u128(0x5557 | (4_u128 << 76) | (2_u128 << 62)),
        );
        std::fs::write(&tampered, b"not-an-encrypted-stage-record").expect("tampered stage");
        windows_dpapi_mark_stage_old(&tampered, now);
        let unknown = locator_root.join("lpcw-credential-stage-v2-not-a-uuid.tmp");
        std::fs::write(&unknown, &valid_record).expect("unknown-name stage");
        windows_dpapi_mark_stage_old(&unknown, now);
        let symlink = windows_dpapi_test_stage_path(
            directory.path(),
            uuid::Uuid::from_u128(0x5558 | (4_u128 << 76) | (2_u128 << 62)),
        );
        std::os::unix::fs::symlink(&tampered, &symlink).expect("stage symlink");

        let outcome = cleanup_windows_bound_credential_staging_with(
            directory.path(),
            resource,
            now,
            |path| {
                if path == in_use {
                    Err(PlatformError::new(PlatformErrorCode::Busy))
                } else {
                    windows_dpapi_test_open_stage(path)
                }
            },
            windows_dpapi_test_delete_stage,
            |ciphertext, _, _| Ok(zeroize::Zeroizing::new(ciphertext.to_vec())),
        );
        assert_eq!(outcome.deleted, 0);
        for path in [young, in_use, tampered, unknown, symlink] {
            assert!(path.exists(), "preserve {}", path.display());
        }
    }

    #[cfg(unix)]
    #[test]
    fn windows_dpapi_stage_gc_enforces_scan_and_delete_caps() {
        let directory = tempdir().expect("temporary directory");
        let resource = "LorePia.ProviderCredential";
        let reference = "lpc2-stage-gc-caps";
        super::ensure_windows_bound_locator_directory(directory.path()).expect("locator directory");
        let now = std::time::SystemTime::now();
        for index in 0..(WINDOWS_BOUND_FILE_STAGE_MAXIMUM_SCAN + 44) {
            let path = windows_dpapi_test_stage_path(
                directory.path(),
                uuid::Uuid::from_u128(index as u128 | (4_u128 << 76) | (2_u128 << 62)),
            );
            std::fs::write(&path, b"invalid-encrypted-stage").expect("invalid stage");
            windows_dpapi_mark_stage_old(&path, now);
        }
        let scan_outcome = cleanup_windows_bound_credential_staging_with(
            directory.path(),
            resource,
            now,
            windows_dpapi_test_open_stage,
            windows_dpapi_test_delete_stage,
            |ciphertext, _, _| Ok(zeroize::Zeroizing::new(ciphertext.to_vec())),
        );
        assert_eq!(scan_outcome.scanned, WINDOWS_BOUND_FILE_STAGE_MAXIMUM_SCAN);
        assert_eq!(scan_outcome.deleted, 0);

        let deletion_directory = tempdir().expect("deletion cap directory");
        super::ensure_windows_bound_locator_directory(deletion_directory.path())
            .expect("locator directory");
        for index in 0..(WINDOWS_BOUND_FILE_STAGE_MAXIMUM_DELETE + 8) {
            let path = windows_dpapi_test_stage_path(
                deletion_directory.path(),
                uuid::Uuid::from_u128((index + 1_000) as u128 | (4_u128 << 76) | (2_u128 << 62)),
            );
            std::fs::write(
                &path,
                windows_dpapi_test_stage_record(
                    deletion_directory.path(),
                    resource,
                    reference,
                    index as u128 + 1_000,
                ),
            )
            .expect("valid stage");
            windows_dpapi_mark_stage_old(&path, now);
        }
        let delete_outcome = cleanup_windows_bound_credential_staging_with(
            deletion_directory.path(),
            resource,
            now,
            windows_dpapi_test_open_stage,
            windows_dpapi_test_delete_stage,
            |ciphertext, _, _| Ok(zeroize::Zeroizing::new(ciphertext.to_vec())),
        );
        assert_eq!(
            delete_outcome.deleted,
            WINDOWS_BOUND_FILE_STAGE_MAXIMUM_DELETE
        );
        assert!(delete_outcome.scanned <= WINDOWS_BOUND_FILE_STAGE_MAXIMUM_SCAN);
    }

    #[test]
    fn windows_bound_value_wrapper_round_trips_only_the_pinned_generation() {
        let username = "lpcw1-20202020-2020-4020-8020-202020202020";
        let claim =
            test_windows_bound_credential_claim_from_username(username).expect("canonical claim");
        let wrapped = encode_windows_bound_credential_value(
            &claim.generation,
            &NativeCredential::new("synthetic-bound-envelope".to_owned()),
        )
        .expect("wrapped value");
        assert!(
            wrapped
                .expose()
                .starts_with("lorepia-windows-bound-value\nv1\n")
        );
        assert!(!wrapped.expose().starts_with("synthetic-bound-envelope"));

        let decoded = decode_windows_bound_credential_value(&claim.generation, wrapped)
            .expect("matching generation");
        assert_eq!(decoded.expose(), "synthetic-bound-envelope");
    }

    #[test]
    fn windows_bound_value_wrapper_rejects_mismatch_malformed_and_legacy_values() {
        let expected = "lpcg1-21212121-2121-4121-8121-212121212121";
        let alternate = "lpcg1-22222222-2222-4222-8222-222222222222";
        let alternate_value = encode_windows_bound_credential_value(
            alternate,
            &NativeCredential::new("alternate-envelope".to_owned()),
        )
        .expect("alternate wrapper");
        let fixtures = [
            alternate_value.into_secret_string(),
            format!("{WINDOWS_BOUND_VALUE_MAGIC}{expected}\n018\nsynthetic-envelope"),
            "raw-legacy-or-bound-envelope".to_owned(),
            format!("{WINDOWS_BOUND_VALUE_MAGIC}{expected}\n999\nshort"),
        ];

        for fixture in fixtures {
            let error =
                decode_windows_bound_credential_value(expected, NativeCredential::new(fixture))
                    .expect_err("noncanonical or mismatched wrapper must fail closed");
            assert_eq!(error.code(), PlatformErrorCode::CredentialRecoveryRequired);
        }
    }

    #[test]
    fn windows_delete_retry_never_removes_same_username_new_generation_winner() {
        let directory = tempdir().expect("temporary directory");
        let reference = "lpc2-delete-generation-winner";
        let username = "lpcw1-23232323-2323-4323-8323-232323232323";
        let claim = WindowsBoundCredentialClaim {
            username: username.to_owned(),
            generation: "lpcg1-aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_owned(),
            file_record_sha256: None,
        };
        drop(
            claim_windows_bound_credential_locator_for_claim(directory.path(), reference, &claim)
                .expect("durable locator"),
        );
        let original = encode_windows_bound_credential_value(
            &claim.generation,
            &NativeCredential::new("original-envelope".to_owned()),
        )
        .expect("original wrapper")
        .into_secret_string();
        let winner = encode_windows_bound_credential_value(
            "lpcg1-24242424-2424-4424-8424-242424242424",
            &NativeCredential::new("winner-envelope".to_owned()),
        )
        .expect("winner wrapper")
        .into_secret_string();
        let stored = RefCell::new(Some(original));
        let removes = RefCell::new(0_u32);
        let read = |expected: &WindowsBoundCredentialClaim| {
            assert_eq!(expected.username, username);
            stored
                .borrow()
                .clone()
                .map(NativeCredential::new)
                .map(|wrapped| decode_windows_bound_credential_value(&expected.generation, wrapped))
                .transpose()
                .map(|value| value.map(NativeCredential::into_secret_string))
        };

        let first = delete_windows_bound_credential_claim_with(
            directory.path(),
            reference,
            &read,
            |expected_claim, expected| {
                assert_eq!(expected_claim, &claim);
                assert_eq!(expected, "original-envelope");
                *removes.borrow_mut() += 1;
                *stored.borrow_mut() = Some(winner.clone());
                Ok(())
            },
            || panic!("claimed delete must not read legacy"),
            |_| panic!("claimed delete must not remove legacy"),
        )
        .expect_err("post-remove generation winner makes outcome uncertain");
        assert_eq!(first.code(), PlatformErrorCode::CredentialRecoveryRequired);
        assert_eq!(*removes.borrow(), 1);

        let second = delete_windows_bound_credential_claim_with(
            directory.path(),
            reference,
            &read,
            |_, _| {
                *removes.borrow_mut() += 1;
                Ok(())
            },
            || panic!("claimed retry must not read legacy"),
            |_| panic!("claimed retry must not remove legacy"),
        )
        .expect_err("mismatched generation is never removed on retry");
        assert_eq!(second.code(), PlatformErrorCode::CredentialRecoveryRequired);
        assert_eq!(*removes.borrow(), 1);
        assert_eq!(stored.borrow().as_deref(), Some(winner.as_str()));
    }

    #[test]
    fn windows_locator_existing_directory_cutpoint_precedes_write_through_claim() {
        let directory = tempdir().expect("temporary directory");
        let locator_root = directory.path().join(WINDOWS_BOUND_LOCATOR_DIRECTORY);
        std::fs::create_dir(&locator_root).expect("simulated concurrent first creator");

        drop(
            claim_windows_bound_credential_locator(
                directory.path(),
                "lpc2-first-create-cutpoint",
                "lpcw1-77777777-7777-4777-8777-777777777777",
            )
            .expect("AlreadyExists path is revalidated before write-through claim"),
        );

        assert!(
            windows_bound_credential_locator_path(directory.path(), "lpc2-first-create-cutpoint")
                .is_file()
        );
    }

    #[test]
    fn windows_locator_path_hashes_untrusted_reference_without_traversal() {
        let directory = tempdir().expect("temporary directory");
        let path = windows_bound_credential_locator_path(
            directory.path(),
            "../../outside\\or:reserved*name",
        );
        assert!(path.starts_with(directory.path()));
        assert_eq!(
            path.parent().and_then(std::path::Path::parent),
            Some(directory.path())
        );
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("name");
        assert!(name.starts_with("lpcw-locator-v1-"));
        assert!(name.ends_with(".record"));
        assert!(!name.contains(".."));
        assert!(!name.contains('\\'));
    }

    #[test]
    fn windows_locator_present_vault_missing_is_observation_only_recovery() {
        let directory = tempdir().expect("temporary directory");
        let reference = "lpc2-install-crash";
        drop(
            claim_windows_bound_credential_locator(
                directory.path(),
                reference,
                "lpcw1-55555555-5555-4555-8555-555555555555",
            )
            .expect("durable locator"),
        );
        let reads = RefCell::new(0_u32);
        let legacy_reads = RefCell::new(0_u32);
        let error = read_windows_bound_credential_with(
            directory.path(),
            reference,
            |_| {
                *reads.borrow_mut() += 1;
                Ok(None)
            },
            || {
                *legacy_reads.borrow_mut() += 1;
                Ok(None)
            },
        )
        .expect_err("locator without vault item is an unknown install/delete outcome");
        assert_eq!(error.code(), PlatformErrorCode::CredentialRecoveryRequired);
        assert_eq!(*reads.borrow(), 1);
        assert_eq!(*legacy_reads.borrow(), 0);
        assert!(windows_bound_credential_locator_path(directory.path(), reference).exists());
    }

    #[test]
    fn windows_locator_partial_or_tampered_record_fails_before_native_access() {
        for (reference, bytes) in [
            ("lpc2-partial", b"lorepia-windows-bound-credential-locator\nv1\n".as_slice()),
            (
                "lpc2-tampered",
                b"lorepia-windows-bound-credential-locator\nv1\nlpc2-tampered\nlpcw1-aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa\nffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff\n"
                    .as_slice(),
            ),
        ] {
            let directory = tempdir().expect("temporary directory");
            let path = windows_bound_credential_locator_path(directory.path(), reference);
            std::fs::create_dir(path.parent().expect("locator root")).expect("locator root");
            std::fs::write(&path, bytes).expect("damaged locator fixture");
            let native_reads = RefCell::new(0_u32);
            let error = read_windows_bound_credential_with(
                directory.path(),
                reference,
                |_| {
                    *native_reads.borrow_mut() += 1;
                    Ok(None)
                },
                || {
                    *native_reads.borrow_mut() += 1;
                    Ok(None)
                },
            )
            .expect_err("partial or tampered locator must fail closed");
            assert_eq!(error.code(), PlatformErrorCode::CredentialRecoveryRequired);
            assert_eq!(*native_reads.borrow(), 0);
            assert_eq!(std::fs::read(&path).expect("locator retained"), bytes);
        }
    }

    #[test]
    fn windows_invalid_lifecycle_record_combinations_fail_before_native_access() {
        for case in 0..4_u8 {
            let directory = tempdir().expect("temporary directory");
            let reference = format!("lpc2-invalid-lifecycle-{case}");
            let username = "lpcw1-25252525-2525-4525-8525-252525252525";
            let claim = test_windows_bound_credential_claim_from_username(username)
                .expect("canonical claim");
            let alternate = test_windows_bound_credential_claim_from_username(
                "lpcw1-26262626-2626-4626-8626-262626262626",
            )
            .expect("alternate claim");
            if case != 0 {
                drop(
                    claim_windows_bound_credential_locator(directory.path(), &reference, username)
                        .expect("locator fixture"),
                );
            } else {
                std::fs::create_dir(directory.path().join(WINDOWS_BOUND_LOCATOR_DIRECTORY))
                    .expect("locator directory");
            }

            match case {
                0 => std::fs::write(
                    windows_bound_credential_delete_completion_path(directory.path(), &reference),
                    encode_windows_bound_delete_record(
                        WINDOWS_BOUND_DELETE_COMPLETE_MAGIC,
                        WINDOWS_BOUND_DELETE_COMPLETE_CHECKSUM_DOMAIN,
                        &reference,
                        &WindowsBoundDeleteTarget::Legacy,
                    ),
                )
                .expect("completion-only fixture"),
                1 => std::fs::write(
                    windows_bound_credential_delete_intent_path(directory.path(), &reference),
                    encode_windows_bound_delete_record(
                        WINDOWS_BOUND_DELETE_INTENT_MAGIC,
                        WINDOWS_BOUND_DELETE_INTENT_CHECKSUM_DOMAIN,
                        &reference,
                        &WindowsBoundDeleteTarget::Legacy,
                    ),
                )
                .expect("legacy intent with locator fixture"),
                2 => {
                    std::fs::write(
                        windows_bound_credential_delete_intent_path(directory.path(), &reference),
                        encode_windows_bound_delete_record(
                            WINDOWS_BOUND_DELETE_INTENT_MAGIC,
                            WINDOWS_BOUND_DELETE_INTENT_CHECKSUM_DOMAIN,
                            &reference,
                            &WindowsBoundDeleteTarget::Claimed(claim.clone()),
                        ),
                    )
                    .expect("claimed intent fixture");
                    std::fs::write(
                        windows_bound_credential_delete_completion_path(
                            directory.path(),
                            &reference,
                        ),
                        encode_windows_bound_delete_record(
                            WINDOWS_BOUND_DELETE_COMPLETE_MAGIC,
                            WINDOWS_BOUND_DELETE_COMPLETE_CHECKSUM_DOMAIN,
                            &reference,
                            &WindowsBoundDeleteTarget::Legacy,
                        ),
                    )
                    .expect("mismatched completion fixture");
                }
                3 => std::fs::write(
                    windows_bound_credential_delete_intent_path(directory.path(), &reference),
                    encode_windows_bound_delete_record(
                        WINDOWS_BOUND_DELETE_INTENT_MAGIC,
                        WINDOWS_BOUND_DELETE_INTENT_CHECKSUM_DOMAIN,
                        &reference,
                        &WindowsBoundDeleteTarget::Claimed(alternate),
                    ),
                )
                .expect("mismatched claim fixture"),
                _ => unreachable!(),
            }

            let native_reads = RefCell::new(0_u32);
            let error = read_windows_bound_credential_with(
                directory.path(),
                &reference,
                |_| {
                    *native_reads.borrow_mut() += 1;
                    Ok(None)
                },
                || {
                    *native_reads.borrow_mut() += 1;
                    Ok(None)
                },
            )
            .expect_err("invalid lifecycle combination must fail closed");
            assert_eq!(error.code(), PlatformErrorCode::CredentialRecoveryRequired);
            assert_eq!(*native_reads.borrow(), 0);
        }
    }

    #[test]
    fn windows_no_locator_bound_read_uses_legacy_deterministic_slot_read_only() {
        let directory = tempdir().expect("temporary directory");
        let claimed_reads = RefCell::new(0_u32);
        let legacy_reads = RefCell::new(0_u32);

        let exact = read_windows_bound_credential_with(
            directory.path(),
            "lpc2-old-bound-exact",
            |_| {
                *claimed_reads.borrow_mut() += 1;
                Ok(None)
            },
            || {
                *legacy_reads.borrow_mut() += 1;
                Ok(Some(NativeCredential::new("old-envelope".to_owned())))
            },
        )
        .expect("legacy compatibility read")
        .expect("legacy bound item");
        assert_eq!(exact.expose(), "old-envelope");
        assert_eq!(*claimed_reads.borrow(), 0);
        assert_eq!(*legacy_reads.borrow(), 1);

        let missing = read_windows_bound_credential_with(
            directory.path(),
            "lpc2-old-bound-missing",
            |_| panic!("claimed slot must not be queried without a locator"),
            || Ok(None),
        )
        .expect("missing legacy bound slot");
        assert!(missing.is_none());

        let error = read_windows_bound_credential_with(
            directory.path(),
            "lpc2-old-bound-malformed",
            |_| panic!("claimed slot must not be queried without a locator"),
            || Err(PlatformError::new(PlatformErrorCode::CredentialUnavailable)),
        )
        .expect_err("malformed deterministic bound value must fail closed");
        assert_eq!(error.code(), PlatformErrorCode::CredentialRecoveryRequired);
    }

    #[test]
    fn windows_explicit_delete_retry_completes_tombstone_only_after_vault_is_missing() {
        let directory = tempdir().expect("temporary directory");
        let reference = "lpc2-delete-crash";
        let username = "lpcw1-66666666-6666-4666-8666-666666666666";
        drop(
            claim_windows_bound_credential_locator(directory.path(), reference, username)
                .expect("durable locator"),
        );
        let vault_value = RefCell::new(None::<()>);
        let remove_calls = RefCell::new(0_u32);

        delete_windows_bound_credential_with(
            directory.path(),
            reference,
            |_| Ok(*vault_value.borrow()),
            |_, ()| {
                *remove_calls.borrow_mut() += 1;
                *vault_value.borrow_mut() = None;
                Ok(())
            },
            || panic!("claimed delete must not inspect the legacy slot"),
            |()| panic!("claimed delete must not remove the legacy slot"),
        )
        .expect("journaled explicit retry can settle a crash after native delete");

        assert_eq!(*remove_calls.borrow(), 0);
        assert!(windows_bound_credential_locator_path(directory.path(), reference).exists());
        assert!(
            windows_bound_credential_delete_completion_path(directory.path(), reference).exists()
        );
    }

    #[test]
    fn windows_claimed_delete_intent_before_remove_retries_exact_item_and_completes() {
        let directory = tempdir().expect("temporary directory");
        let reference = "lpc2-delete-intent-before-remove";
        let username = "lpcw1-16161616-1616-4616-8616-161616161616";
        drop(
            claim_windows_bound_credential_locator(directory.path(), reference, username)
                .expect("durable locator"),
        );
        publish_windows_bound_delete_record(
            directory.path(),
            reference,
            &WindowsBoundDeleteTarget::Claimed(
                test_windows_bound_credential_claim_from_username(username).expect("claim"),
            ),
            false,
        )
        .expect("simulated durable delete intent");
        let value = RefCell::new(Some("bound-envelope".to_owned()));
        let removes = RefCell::new(0_u32);

        delete_windows_bound_credential_with(
            directory.path(),
            reference,
            |_| Ok(value.borrow().clone()),
            |_, expected| {
                assert_eq!(expected, "bound-envelope");
                *removes.borrow_mut() += 1;
                *value.borrow_mut() = None;
                Ok(())
            },
            || panic!("claimed retry must not read legacy"),
            |_| panic!("claimed retry must not remove legacy"),
        )
        .expect("intent-before-remove cutpoint is idempotently recoverable");

        assert_eq!(*removes.borrow(), 1);
        assert!(value.borrow().is_none());
        assert!(
            windows_bound_credential_delete_completion_path(directory.path(), reference).exists()
        );
    }

    #[test]
    fn windows_legacy_delete_intent_before_remove_preserves_ambiguous_existing_item() {
        let directory = tempdir().expect("temporary directory");
        let reference = "lpc2-legacy-delete-intent-before-remove";
        publish_windows_bound_delete_record(
            directory.path(),
            reference,
            &WindowsBoundDeleteTarget::Legacy,
            false,
        )
        .expect("simulated durable legacy delete intent");
        let value = RefCell::new(Some("legacy-envelope".to_owned()));
        let removes = RefCell::new(0_u32);

        let error = delete_windows_bound_credential_with(
            directory.path(),
            reference,
            |_| panic!("legacy retry must not read claimed"),
            |_, _: &String| panic!("legacy retry must not remove claimed"),
            || Ok(value.borrow().clone()),
            |expected| {
                assert_eq!(expected, "legacy-envelope");
                *removes.borrow_mut() += 1;
                *value.borrow_mut() = None;
                Ok(())
            },
        )
        .expect_err("legacy intent cannot identify an item across a crash cutpoint");
        assert_eq!(error.code(), PlatformErrorCode::CredentialRecoveryRequired);
        assert_eq!(*removes.borrow(), 0);
        assert_eq!(value.borrow().as_deref(), Some("legacy-envelope"));
        assert!(
            !windows_bound_credential_delete_completion_path(directory.path(), reference).exists()
        );
    }

    #[test]
    fn windows_legacy_delete_retry_never_removes_a_post_remove_winner() {
        let directory = tempdir().expect("temporary directory");
        let reference = "lpc2-legacy-post-remove-winner";
        let value = RefCell::new(Some("legacy-envelope".to_owned()));
        let removes = RefCell::new(0_u32);

        let first = delete_windows_bound_credential_with(
            directory.path(),
            reference,
            |_| panic!("legacy delete must not read claimed"),
            |_, _: &String| panic!("legacy delete must not remove claimed"),
            || Ok(value.borrow().clone()),
            |_| {
                *removes.borrow_mut() += 1;
                *value.borrow_mut() = Some("post-remove-winner".to_owned());
                Ok(())
            },
        )
        .expect_err("post-remove legacy winner makes outcome uncertain");
        assert_eq!(first.code(), PlatformErrorCode::CredentialRecoveryRequired);
        assert_eq!(*removes.borrow(), 1);

        let second = delete_windows_bound_credential_with(
            directory.path(),
            reference,
            |_| panic!("legacy retry must not read claimed"),
            |_, _: &String| panic!("legacy retry must not remove claimed"),
            || Ok(value.borrow().clone()),
            |_| {
                *removes.borrow_mut() += 1;
                Ok(())
            },
        )
        .expect_err("ambiguous legacy retry preserves the current item");
        assert_eq!(second.code(), PlatformErrorCode::CredentialRecoveryRequired);
        assert_eq!(*removes.borrow(), 1);
        assert_eq!(value.borrow().as_deref(), Some("post-remove-winner"));
    }

    #[test]
    fn windows_bound_delete_removes_exact_claimed_item_then_completes_tombstone() {
        let directory = tempdir().expect("temporary directory");
        let reference = "lpc2-normal-delete";
        let username = "lpcw1-cacacaca-caca-4aca-8aca-cacacacacaca";
        drop(
            claim_windows_bound_credential_locator(directory.path(), reference, username)
                .expect("durable locator"),
        );
        let value = RefCell::new(Some("bound-envelope".to_owned()));
        let reads = RefCell::new(0_u32);
        let removes = RefCell::new(0_u32);
        let legacy_deletes = RefCell::new(0_u32);

        delete_windows_bound_credential_with(
            directory.path(),
            reference,
            |claimed_username| {
                assert_eq!(claimed_username, username);
                *reads.borrow_mut() += 1;
                Ok(value.borrow().clone())
            },
            |claimed_username, expected| {
                assert_eq!(claimed_username, username);
                assert_eq!(expected, "bound-envelope");
                *removes.borrow_mut() += 1;
                *value.borrow_mut() = None;
                Ok(())
            },
            || {
                *legacy_deletes.borrow_mut() += 1;
                Ok(None)
            },
            |_| panic!("claimed delete must not remove the legacy slot"),
        )
        .expect("normal bound delete");

        assert_eq!(*reads.borrow(), 2);
        assert_eq!(*removes.borrow(), 1);
        assert_eq!(*legacy_deletes.borrow(), 0);
        assert!(value.borrow().is_none());
        assert!(windows_bound_credential_locator_path(directory.path(), reference).exists());
        assert!(
            windows_bound_credential_delete_completion_path(directory.path(), reference).exists()
        );
    }

    #[test]
    fn windows_bound_delete_reopen_never_reexposes_legacy_fallback() {
        let directory = tempdir().expect("temporary directory");
        let reference = "lpc2-delete-reopen";
        let username = "lpcw1-cdcdcdcd-cdcd-4dcd-8dcd-cdcdcdcdcdcd";
        drop(
            claim_windows_bound_credential_locator(directory.path(), reference, username)
                .expect("durable locator"),
        );
        let claimed = RefCell::new(Some("bound-envelope".to_owned()));
        let legacy_reads = RefCell::new(0_u32);

        delete_windows_bound_credential_with(
            directory.path(),
            reference,
            |_| Ok(claimed.borrow().clone()),
            |_, _| {
                *claimed.borrow_mut() = None;
                Ok(())
            },
            || panic!("locator-present delete must not read legacy slot"),
            |_| panic!("locator-present delete must not remove legacy slot"),
        )
        .expect("bound delete");

        let reopened = read_windows_bound_credential_with(
            directory.path(),
            reference,
            |_| panic!("deleted claimed slot must not be read"),
            || {
                *legacy_reads.borrow_mut() += 1;
                Ok(Some(NativeCredential::new("legacy-orphan".to_owned())))
            },
        )
        .expect("deleted lifecycle observation");
        assert!(reopened.is_none());
        assert_eq!(*legacy_reads.borrow(), 0);

        let native_reads = RefCell::new(0_u32);
        let add_calls = RefCell::new(0_u32);
        let error = store_prevalidated_windows_bound_credential_with(
            directory.path(),
            reference,
            "lpcw1-17171717-1717-4717-8717-171717171717",
            &NativeCredential::new("reinstall-envelope".to_owned()),
            |_| {
                *add_calls.borrow_mut() += 1;
                Ok(())
            },
            |_| {
                *native_reads.borrow_mut() += 1;
                Ok(None)
            },
            || panic!("deleted lifecycle must not inspect legacy"),
        )
        .expect_err("a completed delete tombstone permanently blocks slot reuse");
        assert_eq!(error.code(), PlatformErrorCode::CredentialRecoveryRequired);
        assert_eq!(*native_reads.borrow(), 0);
        assert_eq!(*add_calls.borrow(), 0);
    }

    #[test]
    fn windows_bound_delete_store_aba_is_serialized_and_preserves_immutable_locator() {
        let directory = tempdir().expect("temporary directory");
        let data_root = Arc::new(directory.path().to_path_buf());
        let reference = "lpc2-delete-store-aba".to_owned();
        let username = "lpcw1-18181818-1818-4818-8818-181818181818";
        drop(
            claim_windows_bound_credential_locator(data_root.as_path(), &reference, username)
                .expect("durable locator"),
        );
        let locator_path = windows_bound_credential_locator_path(data_root.as_path(), &reference);
        let locator_before = std::fs::read(&locator_path).expect("locator bytes");
        let value = Arc::new(Mutex::new(Some("bound-envelope".to_owned())));
        let (remove_entered_sender, remove_entered_receiver) = mpsc::channel();
        let (continue_sender, continue_receiver) = mpsc::channel();

        let delete_root = Arc::clone(&data_root);
        let delete_reference = reference.clone();
        let delete_value = Arc::clone(&value);
        let delete_thread = std::thread::spawn(move || {
            delete_windows_bound_credential_with(
                delete_root.as_path(),
                &delete_reference,
                |_| Ok(delete_value.lock().expect("value lock").clone()),
                |_, _| {
                    *delete_value.lock().expect("value lock") = None;
                    remove_entered_sender.send(()).expect("remove entered");
                    continue_receiver.recv().expect("continue delete");
                    Ok(())
                },
                || Err(PlatformError::new(PlatformErrorCode::Internal)),
                |_| Err(PlatformError::new(PlatformErrorCode::Internal)),
            )
        });
        remove_entered_receiver
            .recv()
            .expect("delete reached remove");

        let add_calls = Arc::new(Mutex::new(0_u32));
        let store_root = Arc::clone(&data_root);
        let store_reference = reference.clone();
        let store_add_calls = Arc::clone(&add_calls);
        let (store_entered_sender, store_entered_receiver) = mpsc::channel();
        let store_thread = std::thread::spawn(move || {
            store_entered_sender.send(()).expect("store entered");
            store_prevalidated_windows_bound_credential_with(
                store_root.as_path(),
                &store_reference,
                "lpcw1-19191919-1919-4919-8919-191919191919",
                &NativeCredential::new("replacement-envelope".to_owned()),
                |_| {
                    *store_add_calls.lock().expect("add calls") += 1;
                    Ok(())
                },
                |_| Ok(None),
                || panic!("deleted lifecycle must not inspect legacy"),
            )
        });
        store_entered_receiver.recv().expect("store attempted lock");
        continue_sender.send(()).expect("continue delete");

        delete_thread
            .join()
            .expect("delete thread")
            .expect("delete completes");
        let store_error = store_thread
            .join()
            .expect("store thread")
            .expect_err("serialized store observes completed tombstone");
        assert_eq!(
            store_error.code(),
            PlatformErrorCode::CredentialRecoveryRequired
        );
        assert_eq!(*add_calls.lock().expect("add calls"), 0);
        assert_eq!(
            std::fs::read(locator_path).expect("immutable locator"),
            locator_before
        );
    }

    #[test]
    fn windows_bound_delete_post_remove_winner_keeps_locator_and_never_restores() {
        let directory = tempdir().expect("temporary directory");
        let reference = "lpc2-delete-post-remove-winner";
        let username = "lpcw1-cbcbcbcb-cbcb-4bcb-8bcb-cbcbcbcbcbcb";
        drop(
            claim_windows_bound_credential_locator(directory.path(), reference, username)
                .expect("durable locator"),
        );
        let locator_path = windows_bound_credential_locator_path(directory.path(), reference);
        let locator_before = std::fs::read(&locator_path).expect("locator bytes");
        let value = RefCell::new(Some("bound-envelope".to_owned()));
        let removes = RefCell::new(0_u32);
        let restores = RefCell::new(0_u32);

        let error = delete_windows_bound_credential_with(
            directory.path(),
            reference,
            |_| Ok(value.borrow().clone()),
            |_, _| {
                *removes.borrow_mut() += 1;
                *value.borrow_mut() = Some("post-remove-winning-envelope".to_owned());
                Ok(())
            },
            || panic!("locator-present delete must not read legacy path"),
            |_| panic!("locator-present delete must not remove legacy path"),
        )
        .expect_err("post-remove winner makes the native outcome uncertain");

        assert_eq!(error.code(), PlatformErrorCode::CredentialRecoveryRequired);
        assert_eq!(*removes.borrow(), 1);
        assert_eq!(*restores.borrow(), 0);
        assert_eq!(
            value.borrow().as_deref(),
            Some("post-remove-winning-envelope")
        );
        assert_eq!(
            std::fs::read(&locator_path).expect("retained locator"),
            locator_before
        );
    }

    #[test]
    fn windows_delete_crash_after_native_remove_is_observed_then_explicitly_retried() {
        let directory = tempdir().expect("temporary directory");
        let reference = "lpc2-delete-crash-observe";
        let username = "lpcw1-abababab-abab-4bab-8bab-abababababab";
        drop(
            claim_windows_bound_credential_locator(directory.path(), reference, username)
                .expect("durable locator"),
        );
        let observation_error = read_windows_bound_credential_with(
            directory.path(),
            reference,
            |_| Ok(None),
            || panic!("locator-present observation must not inspect legacy slot"),
        )
        .expect_err("observation cannot distinguish install crash from delete crash");
        assert_eq!(
            observation_error.code(),
            PlatformErrorCode::CredentialRecoveryRequired
        );
        assert!(windows_bound_credential_locator_path(directory.path(), reference).exists());

        delete_windows_bound_credential_with(
            directory.path(),
            reference,
            |_| Ok(None::<()>),
            |_, ()| panic!("already missing native item must not be removed again"),
            || panic!("locator-present delete retry must not read legacy slot"),
            |()| panic!("locator-present delete retry must not remove legacy slot"),
        )
        .expect("explicit journaled delete retry settles the missing native item");
        assert!(windows_bound_credential_locator_path(directory.path(), reference).exists());
        assert!(
            windows_bound_credential_delete_completion_path(directory.path(), reference).exists()
        );
    }

    #[cfg(unix)]
    #[test]
    fn windows_locator_symlink_is_rejected_without_following_or_native_access() {
        use std::os::unix::fs::symlink;

        let directory = tempdir().expect("temporary directory");
        let outside = directory.path().join("outside");
        std::fs::write(&outside, b"outside").expect("outside sentinel");
        let path = windows_bound_credential_locator_path(directory.path(), "lpc2-symlink");
        std::fs::create_dir(path.parent().expect("locator root")).expect("locator root");
        symlink(&outside, &path).expect("locator symlink");
        let native_reads = RefCell::new(0_u32);

        let error = read_windows_bound_credential_with(
            directory.path(),
            "lpc2-symlink",
            |_| {
                *native_reads.borrow_mut() += 1;
                Ok(None)
            },
            || {
                *native_reads.borrow_mut() += 1;
                Ok(None)
            },
        )
        .expect_err("locator symlink must fail closed");
        assert_eq!(error.code(), PlatformErrorCode::CredentialRecoveryRequired);
        assert_eq!(*native_reads.borrow(), 0);
        assert_eq!(
            std::fs::read(&outside).expect("outside sentinel"),
            b"outside"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_locator_reparse_point_is_rejected_without_native_access() {
        let directory = tempdir().expect("temporary directory");
        let outside = directory.path().join("outside-directory");
        std::fs::create_dir(&outside).expect("outside directory");
        let sentinel = outside.join("sentinel");
        std::fs::write(&sentinel, b"outside").expect("outside sentinel");
        let locator_root = directory.path().join(WINDOWS_BOUND_LOCATOR_DIRECTORY);
        // A directory junction is an ordinary-user reparse fixture and does
        // not depend on Developer Mode or elevated symlink privileges.
        let created = std::process::Command::new("cmd.exe")
            .args(["/D", "/C", "mklink", "/J"])
            .arg(&locator_root)
            .arg(&outside)
            .status()
            .expect("execute mklink junction fixture")
            .success();
        assert!(
            created,
            "Windows hosted gate must create and execute the junction fixture"
        );
        let native_reads = RefCell::new(0_u32);

        let error = read_windows_bound_credential_with(
            directory.path(),
            "lpc2-reparse",
            |_| {
                *native_reads.borrow_mut() += 1;
                Ok(None)
            },
            || {
                *native_reads.borrow_mut() += 1;
                Ok(None)
            },
        )
        .expect_err("locator reparse point must fail closed");
        assert_eq!(error.code(), PlatformErrorCode::CredentialRecoveryRequired);
        assert_eq!(*native_reads.borrow(), 0);
        assert_eq!(
            std::fs::read(&sentinel).expect("outside sentinel"),
            b"outside"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_dpapi_stage_gc_in_use_handle_is_preserved_until_exclusive_open_succeeds() {
        use std::os::windows::fs::OpenOptionsExt;

        let directory = tempdir().expect("temporary directory");
        let resource = "LorePia.ProviderCredential";
        let reference = "lpc2-stage-gc-windows-in-use";
        super::ensure_windows_bound_locator_directory(directory.path()).expect("locator directory");
        let now = std::time::SystemTime::now();
        let stage = windows_dpapi_test_stage_path(
            directory.path(),
            uuid::Uuid::from_u128(0x7777 | (4_u128 << 76) | (2_u128 << 62)),
        );
        std::fs::write(
            &stage,
            windows_dpapi_test_stage_record(directory.path(), resource, reference, 0x7777),
        )
        .expect("valid encrypted stage");
        windows_dpapi_mark_stage_old(&stage, now);
        let in_use = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(&stage)
            .expect("hold stage open");

        let first = cleanup_windows_bound_credential_staging_with(
            directory.path(),
            resource,
            now,
            super::windows::open_verified_staging_file_for_delete,
            |_, file| super::windows::delete_verified_file(file),
            |ciphertext, _, _| Ok(zeroize::Zeroizing::new(ciphertext.to_vec())),
        );
        assert_eq!(first.deleted, 0);
        assert!(stage.exists());
        drop(in_use);

        let second = cleanup_windows_bound_credential_staging_with(
            directory.path(),
            resource,
            now,
            super::windows::open_verified_staging_file_for_delete,
            |_, file| super::windows::delete_verified_file(file),
            |ciphertext, _, _| Ok(zeroize::Zeroizing::new(ciphertext.to_vec())),
        );
        assert_eq!(second.deleted, 1);
        assert!(!stage.exists());
    }

    #[test]
    fn root_policy_keeps_development_away_from_production_data_and_credentials() {
        let home = PathBuf::from("/synthetic/home");
        let application_support = home.join("Library/Application Support");
        let production =
            macos_policy_from_application_support(application_support.clone(), "dev.lorepia.mac")
                .expect("production");
        let development =
            macos_policy_from_application_support(application_support, "dev.lorepia.mac.dev")
                .expect("development");

        assert_eq!(
            production.data_root,
            home.join("Library/Application Support/LorePia")
        );
        assert_eq!(
            development.data_root,
            home.join("Library/Application Support/LorePia Development")
        );
        assert_ne!(production.data_root, development.data_root);
        assert_ne!(
            production.credential_namespace,
            development.credential_namespace
        );
        assert_eq!(production.staging_name, "native-staging");
        assert_eq!(development.staging_name, "native-staging");
        assert!(production.migrate_legacy_credentials);
        assert!(!development.migrate_legacy_credentials);
    }

    #[test]
    fn root_policy_rejects_an_unrecognized_identifier() {
        assert!(
            macos_policy_from_application_support(
                PathBuf::from("/synthetic/home/Library/Application Support"),
                "dev.lorepia.app",
            )
            .is_err()
        );
    }

    #[test]
    fn windows_root_policy_preserves_legacy_production_root_and_isolates_dev() {
        let local_app_data = PathBuf::from("C:/synthetic/LocalAppData");
        let production =
            windows_policy_from_local_app_data(local_app_data.clone(), "dev.lorepia.windows")
                .expect("production");
        let development =
            windows_policy_from_local_app_data(local_app_data.clone(), "dev.lorepia.windows.dev")
                .expect("development");

        assert_eq!(production.data_root, local_app_data.join("LorePia"));
        assert_eq!(
            development.data_root,
            local_app_data.join("LorePia Development")
        );
        assert_ne!(production.data_root, development.data_root);
        assert_eq!(
            production.credential_namespace,
            "LorePia.ProviderCredential"
        );
        assert_eq!(
            development.credential_namespace,
            "LorePia.ProviderCredential.Development"
        );
        assert_eq!(production.staging_name, "transport-staging");
        assert_eq!(development.staging_name, "transport-staging");
    }

    #[test]
    fn abandoned_cleanup_stays_inside_owned_regular_files() {
        let root = tempdir().expect("root");
        let staging = root.path().join("staging");
        std::fs::create_dir(&staging).expect("staging");
        let owned = staging.join("lorepia-tauri-synthetic.json");
        let unrelated = staging.join("user-file.json");
        let owned_directory = staging.join("lorepia-tauri-directory");
        std::fs::write(&owned, b"owned").expect("owned");
        std::fs::write(&unrelated, b"unrelated").expect("unrelated");
        std::fs::create_dir(&owned_directory).expect("owned directory");

        cleanup_abandoned_staging(&staging, Duration::ZERO);

        assert!(!owned.exists());
        assert!(unrelated.exists());
        assert!(owned_directory.is_dir());
    }

    #[test]
    fn abandoned_cleanup_preserves_fresh_owned_files() {
        let root = tempdir().expect("root");
        let staging = root.path().join("staging");
        std::fs::create_dir(&staging).expect("staging");
        let fresh = staging.join("lorepia-tauri-fresh.partial");
        std::fs::write(&fresh, b"fresh").expect("fresh");

        cleanup_abandoned_staging(&staging, Duration::from_mins(1));

        assert!(fresh.exists());
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    #[test]
    fn unsupported_desktop_platform_fails_closed() {
        let error = super::unsupported_platform::<()>().expect_err("unsupported platform");

        assert_eq!(error.code(), crate::PlatformErrorCode::UnsupportedPlatform);
    }
}
