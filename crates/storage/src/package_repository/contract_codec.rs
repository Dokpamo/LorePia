//! Canonical package storage encoding and validation helpers.

use chrono::{DateTime, Utc};
use lorepia_domain::{CoreError, CoreErrorCode, CoreResult};
use lorepia_orchestration::PackageComponentKind;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::orchestration::PackageImportStatus;

use super::{
    MAX_PACKAGE_JSON_BYTES, MAX_PACKAGE_JSON_DEPTH, MAX_PACKAGE_JSON_NODES, PackageCapability,
    PackageCapabilitySupport,
};

pub(super) const fn component_kind_str(kind: PackageComponentKind) -> &'static str {
    match kind {
        PackageComponentKind::PromptPreset => "prompt_preset",
        PackageComponentKind::MemoryProfile => "memory_profile",
        PackageComponentKind::KnowledgeBook => "knowledge_book",
        PackageComponentKind::TransformSet => "transform_set",
        PackageComponentKind::InteractionRuleSet => "interaction_rule_set",
        PackageComponentKind::ContentModule => "content_module",
        PackageComponentKind::AssetIndex => "asset",
        PackageComponentKind::RawExtension => "raw_extension",
    }
}

pub(super) const fn import_status_str(status: PackageImportStatus) -> &'static str {
    match status {
        PackageImportStatus::Inspected => "inspected",
        PackageImportStatus::AwaitingReview => "awaiting_review",
        PackageImportStatus::Approved => "approved",
        PackageImportStatus::Committing => "committing",
        PackageImportStatus::Completed => "completed",
        PackageImportStatus::Failed => "failed",
        PackageImportStatus::Discarded => "discarded",
        PackageImportStatus::RolledBack => "rolled_back",
    }
}

pub(super) fn parse_import_status(value: &str) -> CoreResult<PackageImportStatus> {
    match value {
        "inspected" => Ok(PackageImportStatus::Inspected),
        "awaiting_review" => Ok(PackageImportStatus::AwaitingReview),
        "approved" => Ok(PackageImportStatus::Approved),
        "committing" => Ok(PackageImportStatus::Committing),
        "completed" => Ok(PackageImportStatus::Completed),
        "failed" => Ok(PackageImportStatus::Failed),
        "discarded" => Ok(PackageImportStatus::Discarded),
        "rolled_back" => Ok(PackageImportStatus::RolledBack),
        _ => Err(storage_corrupted("stored package import state is invalid")),
    }
}

pub(super) fn parse_package_capability(value: &str) -> CoreResult<PackageCapability> {
    match value {
        "prompt_fragments" => Ok(PackageCapability::PromptFragments),
        "knowledge" => Ok(PackageCapability::Knowledge),
        "variables" => Ok(PackageCapability::Variables),
        "transforms" => Ok(PackageCapability::Transforms),
        "declarative_interactions" => Ok(PackageCapability::DeclarativeInteractions),
        "image_assets" => Ok(PackageCapability::ImageAssets),
        "audio_assets" => Ok(PackageCapability::AudioAssets),
        "video_assets" => Ok(PackageCapability::VideoAssets),
        "attachment_assets" => Ok(PackageCapability::AttachmentAssets),
        "high_risk_assets" => Ok(PackageCapability::HighRiskAssets),
        "external_urls" => Ok(PackageCapability::ExternalUrls),
        "html" => Ok(PackageCapability::Html),
        "script" => Ok(PackageCapability::Script),
        "native_code" => Ok(PackageCapability::NativeCode),
        "network" => Ok(PackageCapability::Network),
        "filesystem" => Ok(PackageCapability::Filesystem),
        "shell" => Ok(PackageCapability::Shell),
        "credentials" => Ok(PackageCapability::Credentials),
        _ => Err(storage_corrupted(
            "stored package capability name is invalid",
        )),
    }
}

pub(super) fn parse_capability_support(value: &str) -> CoreResult<PackageCapabilitySupport> {
    match value {
        "supported" => Ok(PackageCapabilitySupport::Supported),
        "unsupported" => Ok(PackageCapabilitySupport::Unsupported),
        "approval_required" => Ok(PackageCapabilitySupport::ApprovalRequired),
        _ => Err(storage_corrupted(
            "stored package capability support is invalid",
        )),
    }
}

pub(super) fn license_fields(license: &str) -> (Option<&str>, &'static str) {
    let license = license.trim();
    if license.is_empty() {
        (None, "missing")
    } else if license.eq_ignore_ascii_case("unknown")
        || license.eq_ignore_ascii_case("LicenseRef-Unknown")
    {
        (Some(license), "unknown")
    } else {
        (Some(license), "declared")
    }
}

pub(super) fn encode_json<T: Serialize>(label: &str, value: &T) -> CoreResult<String> {
    let json = serde_json::to_string(value)
        .map_err(|error| CoreError::invalid(format!("{label} cannot be encoded: {error}")))?;
    validate_json(label, &json)?;
    Ok(json)
}

pub(super) fn decode_json<T: DeserializeOwned>(label: &str, json: &str) -> CoreResult<T> {
    validate_json(label, json).map_err(|error| {
        storage_corrupted(format!(
            "{label} violates storage bounds: {}",
            error.message
        ))
    })?;
    serde_json::from_str(json)
        .map_err(|error| storage_corrupted(format!("stored {label} is invalid: {error}")))
}

pub(super) fn validate_json(label: &str, json: &str) -> CoreResult<()> {
    if json.len() > MAX_PACKAGE_JSON_BYTES {
        return Err(CoreError::invalid(format!(
            "{label} exceeds the package JSON limit"
        )));
    }
    let value: Value = serde_json::from_str(json)
        .map_err(|error| CoreError::invalid(format!("{label} is invalid JSON: {error}")))?;
    let mut pending = vec![(&value, 0_usize)];
    let mut nodes = 0_usize;
    while let Some((value, depth)) = pending.pop() {
        nodes = nodes.saturating_add(1);
        if nodes > MAX_PACKAGE_JSON_NODES || depth > MAX_PACKAGE_JSON_DEPTH {
            return Err(CoreError::invalid(format!(
                "{label} exceeds package JSON structural limits"
            )));
        }
        match value {
            Value::Object(object) => {
                for (key, child) in object {
                    if is_secret_key(key) {
                        return Err(CoreError::invalid(format!(
                            "{label} contains a raw credential field"
                        )));
                    }
                    pending.push((child, depth.saturating_add(1)));
                }
            }
            Value::Array(array) => {
                pending.extend(array.iter().map(|child| (child, depth.saturating_add(1))));
            }
            _ => {}
        }
    }
    Ok(())
}

pub(super) fn is_secret_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "api_key"
            | "authorization"
            | "password"
            | "private_key"
            | "client_secret"
            | "access_token"
            | "refresh_token"
            | "credential"
    )
}

pub(super) fn validate_identifier(label: &str, value: &str) -> CoreResult<()> {
    if value.is_empty()
        || value.len() > 256
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(CoreError::invalid(format!("{label} identifier is invalid")));
    }
    Ok(())
}

pub(super) fn validate_sha256(label: &str, value: &str) -> CoreResult<()> {
    if value.len() != 64
        || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
        || value.bytes().any(|byte| byte.is_ascii_uppercase())
    {
        return Err(CoreError::invalid(format!(
            "{label} SHA-256 digest is invalid"
        )));
    }
    Ok(())
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub(super) fn parse_datetime(label: &str, value: &str) -> CoreResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| storage_corrupted(format!("stored {label} is invalid: {error}")))
}

pub(super) fn u64_from_i64(label: &str, value: i64) -> CoreResult<u64> {
    u64::try_from(value).map_err(|_| storage_corrupted(format!("stored {label} is negative")))
}

pub(super) fn u32_from_i64(label: &str, value: i64) -> CoreResult<u32> {
    u32::try_from(value).map_err(|_| storage_corrupted(format!("stored {label} is out of range")))
}

pub(super) fn i64_from_u64(label: &str, value: u64) -> CoreResult<i64> {
    i64::try_from(value).map_err(|_| CoreError::invalid(format!("{label} exceeds SQLite range")))
}

pub(super) fn not_found(kind: &str) -> CoreError {
    CoreError::new(
        CoreErrorCode::NotFound,
        format!("{kind} was not found"),
        false,
    )
}

pub(super) fn revision_conflict(
    kind: &str,
    id: &str,
    expected: Option<u64>,
    actual: Option<u64>,
) -> CoreError {
    CoreError::new(
        CoreErrorCode::InvalidInput,
        format!(
            "{kind} revision conflict for {id}: expected {}, current {}",
            expected.map_or_else(|| "new".to_owned(), |value| value.to_string()),
            actual.map_or_else(|| "missing".to_owned(), |value| value.to_string())
        ),
        true,
    )
}

pub(super) fn storage_corrupted(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::StorageCorrupted, message, false)
}
