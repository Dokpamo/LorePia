//! Path-free asset metadata for the webview and bounded bytes for the native
//! custom-protocol handler.

use lorepia_core::{AssetDeliveryDescriptor, AssetDeliveryKind, AssetId, CoreError, Sha256Digest};
use serde::{Deserialize, Serialize};

use crate::{ShellApi, ShellError, ShellResult, api::validate_identifier};

const ASSET_PROTOCOL_PREFIX: &str = "lorepia-asset://sha256/";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AssetDeliverySelector {
    AssetId { asset_id: String },
    Sha256 { sha256: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolveAssetDeliveryInput {
    pub selector: AssetDeliverySelector,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetDeliveryKindDto {
    Image,
    Audio,
    Video,
}

/// Bounded metadata and an opaque content-addressed renderer URL.
///
/// `url` contains only a canonical digest. It never contains an absolute path,
/// package logical path, credential, or unrestricted file capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetDeliveryDto {
    pub asset_id: String,
    pub sha256: String,
    pub media_type: String,
    pub kind: AssetDeliveryKindDto,
    pub size_bytes: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_ms: Option<u64>,
    pub url: String,
}

/// Rust-only protocol payload. This type is deliberately not serializable and
/// is not registered in the Tauri invoke allowlist.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetProtocolRange {
    pub descriptor: AssetDeliveryDto,
    pub start: u64,
    pub bytes: Vec<u8>,
}

impl From<AssetDeliveryKind> for AssetDeliveryKindDto {
    fn from(value: AssetDeliveryKind) -> Self {
        match value {
            AssetDeliveryKind::Image => Self::Image,
            AssetDeliveryKind::Audio => Self::Audio,
            AssetDeliveryKind::Video => Self::Video,
        }
    }
}

impl From<AssetDeliveryDescriptor> for AssetDeliveryDto {
    fn from(value: AssetDeliveryDescriptor) -> Self {
        let url = format!("{ASSET_PROTOCOL_PREFIX}{}", value.sha256.as_str());
        Self {
            asset_id: value.asset_id.0,
            sha256: value.sha256.into_inner(),
            media_type: value.media_type,
            kind: value.kind.into(),
            size_bytes: value.size_bytes,
            width: value.width,
            height: value.height,
            duration_ms: value.duration_ms,
            url,
        }
    }
}

impl ShellApi {
    pub fn resolve_asset_delivery(
        &self,
        input: ResolveAssetDeliveryInput,
    ) -> ShellResult<AssetDeliveryDto> {
        let descriptor = match input.selector {
            AssetDeliverySelector::AssetId { asset_id } => {
                validate_identifier("asset_id", &asset_id)?;
                self.core
                    .resolve_asset_delivery_by_id(&AssetId::from(asset_id))
            }
            AssetDeliverySelector::Sha256 { sha256 } => {
                let digest = parse_sha256(&sha256)?;
                self.core.resolve_asset_delivery_by_sha256(&digest)
            }
        }
        .map_err(ShellError::from)?;
        Ok(descriptor.into())
    }

    /// Resolves one digest for HEAD and response-policy decisions inside the
    /// native custom protocol. This is never an invoke command.
    #[doc(hidden)]
    pub fn resolve_asset_protocol_sha256(&self, sha256: &str) -> ShellResult<AssetDeliveryDto> {
        let digest = parse_sha256(sha256)?;
        self.core
            .resolve_asset_delivery_by_sha256(&digest)
            .map(Into::into)
            .map_err(ShellError::from)
    }

    /// Reads one bounded range from a short-lived verified native handle. Raw
    /// bytes never cross the invoke serialization boundary.
    #[doc(hidden)]
    pub fn read_asset_protocol_range(
        &self,
        sha256: &str,
        start: u64,
        requested_bytes: u64,
    ) -> ShellResult<AssetProtocolRange> {
        let digest = parse_sha256(sha256)?;
        let range = self
            .core
            .read_asset_delivery_range(&digest, start, requested_bytes)
            .map_err(ShellError::from)?;
        Ok(AssetProtocolRange {
            descriptor: range.descriptor.into(),
            start: range.start,
            bytes: range.bytes,
        })
    }
}

fn parse_sha256(value: &str) -> ShellResult<Sha256Digest> {
    Sha256Digest::parse(value)
        .map_err(|_| ShellError::from(CoreError::invalid("sha256 is not a canonical digest")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selector_rejects_ambiguous_or_extra_fields() {
        for invalid in [
            r#"{"selector":{"kind":"asset_id","asset_id":"asset","sha256":"00"}}"#,
            r#"{"selector":{"kind":"sha256","sha256":"00","asset_id":"asset"}}"#,
            r#"{"selector":{"kind":"path","path":"/tmp/asset"}}"#,
            r#"{"selector":{"kind":"asset_id","asset_id":"asset"},"path":"/tmp/asset"}"#,
        ] {
            assert!(
                serde_json::from_str::<ResolveAssetDeliveryInput>(invalid).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn descriptor_url_contains_only_the_canonical_digest() {
        let digest = "ab".repeat(32);
        let descriptor = AssetDeliveryDto::from(AssetDeliveryDescriptor {
            asset_id: AssetId::from("asset"),
            sha256: Sha256Digest::parse(&digest).expect("digest"),
            media_type: "image/png".to_owned(),
            kind: AssetDeliveryKind::Image,
            size_bytes: 8,
            width: Some(1),
            height: Some(1),
            duration_ms: None,
        });
        assert_eq!(descriptor.url, format!("lorepia-asset://sha256/{digest}"));
        let serialized = serde_json::to_string(&descriptor).expect("serialize");
        assert!(!serialized.contains("/Users/"));
        assert!(!serialized.contains("logical_path"));
        assert!(!serialized.contains("payload"));
    }

    #[test]
    fn protocol_range_is_not_serializable_by_construction() {
        fn assert_not_an_invoke_dto(_: &AssetProtocolRange) {}
        let range = AssetProtocolRange {
            descriptor: AssetDeliveryDto {
                asset_id: "asset".to_owned(),
                sha256: "ab".repeat(32),
                media_type: "image/png".to_owned(),
                kind: AssetDeliveryKindDto::Image,
                size_bytes: 1,
                width: None,
                height: None,
                duration_ms: None,
                url: format!("lorepia-asset://sha256/{}", "ab".repeat(32)),
            },
            start: 0,
            bytes: vec![0],
        };
        assert_not_an_invoke_dto(&range);
    }
}
