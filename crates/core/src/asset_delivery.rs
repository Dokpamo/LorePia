//! Trusted resolution of approved content-addressed media.
//!
//! The webview receives only immutable metadata and an opaque digest URL.
//! Storage owns every database lookup, path calculation, complete-file hash
//! check, media-signature check, and bounded range read.

use lorepia_domain::{AssetDescriptor, AssetId, CoreResult, Sha256Digest};
use serde::{Deserialize, Serialize};

use crate::Core;

/// Renderer class admitted by the fixed inert-media allowlist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetDeliveryKind {
    Image,
    Audio,
    Video,
}

/// Credential-free, path-free metadata for one verified CAS object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetDeliveryDescriptor {
    pub asset_id: AssetId,
    pub sha256: Sha256Digest,
    pub media_type: String,
    pub kind: AssetDeliveryKind,
    pub size_bytes: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_ms: Option<u64>,
}

/// One Rust-only bounded range from a freshly revalidated CAS object.
///
/// This type is not registered as a Tauri command result. The native custom
/// protocol consumes it directly and hands the body to the webview decoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetDeliveryRange {
    pub descriptor: AssetDeliveryDescriptor,
    pub start: u64,
    pub bytes: Vec<u8>,
}

impl TryFrom<AssetDescriptor> for AssetDeliveryDescriptor {
    type Error = lorepia_domain::CoreError;

    fn try_from(value: AssetDescriptor) -> Result<Self, Self::Error> {
        let kind = asset_delivery_kind(&value.media_type)?;
        Ok(Self {
            asset_id: value.id,
            sha256: value.sha256,
            media_type: value.media_type,
            kind,
            size_bytes: value.size_bytes,
            width: value.width,
            height: value.height,
            duration_ms: value.duration_ms,
        })
    }
}

impl Core {
    pub fn resolve_asset_delivery_by_id(
        &self,
        asset_id: &AssetId,
    ) -> CoreResult<AssetDeliveryDescriptor> {
        self.storage()
            .resolve_approved_asset_by_id(asset_id)?
            .try_into()
    }

    pub fn resolve_asset_delivery_by_sha256(
        &self,
        sha256: &Sha256Digest,
    ) -> CoreResult<AssetDeliveryDescriptor> {
        self.storage()
            .resolve_approved_asset_by_sha256(sha256)?
            .try_into()
    }

    pub fn read_asset_delivery_range(
        &self,
        sha256: &Sha256Digest,
        start: u64,
        requested_bytes: u64,
    ) -> CoreResult<AssetDeliveryRange> {
        let range = self
            .storage()
            .read_approved_asset_range(sha256, start, requested_bytes)?;
        Ok(AssetDeliveryRange {
            descriptor: range.descriptor.try_into()?,
            start: range.start,
            bytes: range.bytes,
        })
    }
}

fn asset_delivery_kind(media_type: &str) -> CoreResult<AssetDeliveryKind> {
    match media_type {
        "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/avif" => {
            Ok(AssetDeliveryKind::Image)
        }
        "audio/mpeg" | "audio/wav" | "audio/ogg" => Ok(AssetDeliveryKind::Audio),
        "video/mp4" | "video/webm" => Ok(AssetDeliveryKind::Video),
        _ => Err(lorepia_domain::CoreError::new(
            lorepia_domain::CoreErrorCode::UnsafeArchive,
            "asset media type is not allowed in the renderer",
            false,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renderer_allowlist_excludes_active_and_attachment_media() {
        for media_type in [
            "text/html",
            "image/svg+xml",
            "application/pdf",
            "font/woff2",
            "application/javascript",
            "application/wasm",
        ] {
            assert!(asset_delivery_kind(media_type).is_err(), "{media_type}");
        }
        for media_type in [
            "image/png",
            "image/jpeg",
            "image/gif",
            "image/webp",
            "image/avif",
            "audio/mpeg",
            "audio/wav",
            "audio/ogg",
            "video/mp4",
            "video/webm",
        ] {
            assert!(asset_delivery_kind(media_type).is_ok(), "{media_type}");
        }
    }
}
