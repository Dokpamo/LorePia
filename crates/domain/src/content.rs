use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use uuid::Uuid;

use crate::orchestration::AssetId;

/// The encoded length of a SHA-256 digest.
pub const SHA256_HEX_LENGTH: usize = 64;

/// A canonical, lowercase SHA-256 digest.
///
/// Content hashes cross storage and package trust boundaries, so malformed or
/// unbounded strings are rejected during construction and deserialization.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct Sha256Digest(String);

impl Sha256Digest {
    pub fn parse(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        if value.len() != SHA256_HEX_LENGTH || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(format!(
                "SHA-256 digest must contain exactly {SHA256_HEX_LENGTH} hexadecimal characters"
            ));
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for Sha256Digest {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl<'de> Deserialize<'de> for Sha256Digest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

/// Product-facing purpose of an imported or locally created asset.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetRole {
    Avatar,
    Icon,
    Background,
    UserIcon,
    Emotion,
    Expression,
    Illustration,
    Audio,
    Voice,
    Video,
    StatusPanel,
    Attachment,
    #[default]
    Other,
}

/// Provenance category for an asset.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetSourceKind {
    CharacterCard,
    CharxPackage,
    LorepiaPackage,
    ContentModule,
    UserSelected,
    Generated,
    #[default]
    Unknown,
}

/// Provenance pointers for an asset.
///
/// `logical_path` is a normalized package identifier, never an unrestricted
/// host filesystem path. Callers must validate it before constructing a
/// descriptor from untrusted input.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetSource {
    #[serde(default)]
    pub kind: AssetSourceKind,
    #[serde(default)]
    pub source_sha256: Option<Sha256Digest>,
    #[serde(default)]
    pub logical_path: Option<String>,
}

/// Provider-neutral metadata for a content-addressed asset.
///
/// This descriptor contains no staged or absolute filesystem path and grants
/// no ability to open the asset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetDescriptor {
    pub id: AssetId,
    pub sha256: Sha256Digest,
    pub media_type: String,
    #[serde(default)]
    pub role: AssetRole,
    pub name: String,
    pub size_bytes: u64,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub source: AssetSource,
}

/// Stable identifier for an inspected staging file.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InspectionId(pub String);

impl InspectionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl Default for InspectionId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    CharacterCardV3,
    CharacterCardPng,
    CharxPackage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportWarning {
    pub code: String,
    pub message: String,
}

/// Platform-neutral metadata for an image inside an inspected archive.
///
/// `logical_asset_id` is the validated, normalized archive path. It is not a
/// host filesystem path and does not grant access to staged bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportImagePreview {
    pub logical_asset_id: String,
    pub media_type: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportInspection {
    pub id: InspectionId,
    pub kind: ContentKind,
    pub display_name: String,
    pub description: String,
    pub representative_image: Option<ImportImagePreview>,
    pub source_sha256: String,
    pub source_size: u64,
    pub estimated_stored_size: u64,
    pub asset_count: u32,
    pub warnings: Vec<ImportWarning>,
    pub blocked_reasons: Vec<String>,
    pub unsupported_optional_fields: Vec<String>,
}

impl ImportInspection {
    pub fn is_allowed(&self) -> bool {
        self.blocked_reasons.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportLimits {
    pub max_source_bytes: u64,
    pub max_entries: usize,
    pub max_entry_bytes: u64,
    pub max_total_uncompressed_bytes: u64,
    pub max_compression_ratio: u64,
}

impl Default for ImportLimits {
    fn default() -> Self {
        Self {
            max_source_bytes: 128 * 1024 * 1024,
            max_entries: 2_048,
            max_entry_bytes: 64 * 1024 * 1024,
            max_total_uncompressed_bytes: 512 * 1024 * 1024,
            max_compression_ratio: 100,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AssetDescriptor, AssetRole, AssetSource, AssetSourceKind, ContentKind, ImportInspection,
        ImportLimits, InspectionId, Sha256Digest,
    };
    use crate::orchestration::AssetId;

    #[test]
    fn sha256_digest_is_canonical_and_rejects_malformed_wire_values() {
        let uppercase = "AB".repeat(32);
        let digest = Sha256Digest::parse(&uppercase).expect("valid SHA-256");

        assert_eq!(digest.as_str(), "ab".repeat(32));
        assert!(Sha256Digest::parse("00").is_err());
        assert!(Sha256Digest::parse("g0".repeat(32)).is_err());
        assert!(serde_json::from_str::<Sha256Digest>("\"00\"").is_err());
    }

    #[test]
    fn asset_descriptor_round_trips_without_host_paths() {
        let descriptor = AssetDescriptor {
            id: AssetId::from("asset"),
            sha256: Sha256Digest::parse("12".repeat(32)).expect("digest"),
            media_type: "image/png".into(),
            role: AssetRole::Expression,
            name: "smile.png".into(),
            size_bytes: 42,
            width: Some(128),
            height: Some(128),
            duration_ms: None,
            source: AssetSource {
                kind: AssetSourceKind::CharxPackage,
                source_sha256: Some(Sha256Digest::parse("34".repeat(32)).expect("source digest")),
                logical_path: Some("assets/smile.png".into()),
            },
        };

        let json = serde_json::to_string(&descriptor).expect("serialize descriptor");
        assert!(!json.contains("/Users/"));
        assert_eq!(
            serde_json::from_str::<AssetDescriptor>(&json).expect("deserialize descriptor"),
            descriptor
        );
    }

    #[test]
    fn import_is_allowed_only_without_blocked_reasons() {
        let mut inspection = ImportInspection {
            id: InspectionId::new(),
            kind: ContentKind::CharacterCardV3,
            display_name: "Synthetic character".into(),
            description: String::new(),
            representative_image: None,
            source_sha256: "00".repeat(32),
            source_size: 1,
            estimated_stored_size: 1,
            asset_count: 0,
            warnings: Vec::new(),
            blocked_reasons: Vec::new(),
            unsupported_optional_fields: Vec::new(),
        };

        assert!(inspection.is_allowed());
        inspection.blocked_reasons.push("unsafe input".into());
        assert!(!inspection.is_allowed());
    }

    #[test]
    fn default_import_limits_are_finite_and_internally_ordered() {
        let limits = ImportLimits::default();

        assert!(limits.max_source_bytes > 0);
        assert!(limits.max_entries > 0);
        assert!(limits.max_entry_bytes <= limits.max_total_uncompressed_bytes);
        assert!(limits.max_compression_ratio > 1);
    }
}
