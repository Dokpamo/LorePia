//! Offline provider/model catalog verification, merge, history, and rollback.
//!
//! Catalog updates enter through a manually selected local file. This module
//! contains no HTTP client and performs no network access. Signed payload bytes
//! are canonical JSON wrapped in base64; an Ed25519 signature covers an exact,
//! domain-separated binary frame containing the envelope version, signing key
//! ID, and payload length.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use chrono::{DateTime, Duration, Utc};
use lorepia_domain::{
    ApiFamily, AuthBinding, CanonicalOrigin, DecoderId, EndpointPath, EndpointSpec, HttpMethod,
    HttpUrl, ManifestDecoders, ManifestEndpoints, ParameterSpec, ProviderManifest,
    ProviderParameterMapping, ProviderTemplate, ProviderTemplateId, TemplateSource,
};
use ring::{digest, signature};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::{
    manifest_validator::{validate_connection_fields, validate_manifest},
    registry::{AdapterRegistry, BuiltInTemplateId},
};

pub const CATALOG_ENVELOPE_VERSION: u32 = 1;
pub const CATALOG_SCHEMA_VERSION: u32 = 1;
pub const CATALOG_HISTORY_SCHEMA_VERSION: u32 = 1;
pub const CATALOG_DIFF_SCHEMA_VERSION: u32 = 1;
pub const CATALOG_ROLLBACK_PLAN_VERSION: u32 = 1;
pub const CATALOG_ID: &str = "lorepia-provider-model-catalog";
pub const BUNDLED_CATALOG_REVISION: u64 = 1;
pub const BUNDLED_CATALOG_VERIFIED_AT: &str = "2026-07-31T00:00:00Z";
/// Schema v1 uses only the low 48 bits for its monotonic sequence. The high
/// 16 bits stay reserved for an explicit future epoch/schema transition rather
/// than allowing one signed value to consume `SQLite`'s complete integer range.
pub const CATALOG_REVISION_V1_MAX: u64 = (1_u64 << 48) - 1;
/// Reviews may skip releases, but one import cannot burn an unbounded portion
/// of the remaining schema-v1 revision namespace.
pub const CATALOG_REVISION_MAX_ADVANCE: u64 = 1_000_000;

const SIGNATURE_DOMAIN: &[u8] = b"LorePia/provider-model-catalog/update\0";
const BUNDLED_CATALOG_LAYER_ID: &str = "bundled:compiled:1";
const BUNDLED_SOURCE_ORIGIN: &str = "https://catalog.lorepia.invalid";
const MAX_ENVELOPE_BYTES: usize = 2 * 1024 * 1024;
const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;
const MAX_PAYLOAD_BASE64_BYTES: usize = 1_398_104;
const SIGNATURE_BASE64_BYTES: usize = 88;
const MAX_KEY_ID_BYTES: usize = 64;
const ED25519_SIGNATURE_BYTES: usize = 64;
const MAX_CATALOG_MODELS: usize = 4_096;
const MAX_CATALOG_MANIFESTS: usize = 256;
const MAX_MODEL_ENTRY_ID_BYTES: usize = 160;
const MAX_DISPLAY_NAME_BYTES: usize = 256;
const MAX_MODEL_PATTERN_BYTES: usize = 256;
const MAX_CAPABILITIES: usize = 128;
const MAX_ENUM_VALUES: usize = 128;
const MAX_ENUM_VALUE_BYTES: usize = 256;
const MAX_PARAMETERS: usize = 128;
const MAX_SOURCES: usize = 32;
const MAX_HISTORY_SNAPSHOTS: usize = 32;
const MAX_CATALOG_VALIDITY_DAYS: i64 = 400;
const CLOCK_SKEW_MINUTES: i64 = 5;
const ROLLBACK_PLAN_LIFETIME_MINUTES: i64 = 15;

// Public verification material only. No corresponding private key is compiled
// into or read by the application.
const BUNDLED_SIGNING_KEYS: [CatalogTrustKey; 1] = [CatalogTrustKey {
    key_id: "lorepia-catalog-2026-01",
    public_key: [
        0x6f, 0x98, 0xf6, 0x96, 0x03, 0x44, 0x85, 0xb2, 0x4f, 0x54, 0xc1, 0xf4, 0xba, 0x0d, 0x94,
        0x23, 0xbb, 0xd1, 0x21, 0x8a, 0xc0, 0x6c, 0xd6, 0x22, 0x5d, 0x18, 0x2a, 0x92, 0x6e, 0x56,
        0xd2, 0xf2,
    ],
}];

#[derive(Debug, Clone, Copy)]
struct CatalogTrustKey {
    key_id: &'static str,
    public_key: [u8; 32],
}

/// Immutable Ed25519 public-key trust store used for catalog imports.
///
/// Production construction is limited to the public keys compiled into the
/// application. Private signing material is never part of this type.
#[derive(Debug, Clone)]
pub struct CatalogTrustStore {
    keys: Vec<CatalogTrustKey>,
}

impl CatalogTrustStore {
    pub fn bundled() -> Self {
        Self {
            keys: BUNDLED_SIGNING_KEYS.to_vec(),
        }
    }

    fn public_key(&self, key_id: &str) -> Option<&[u8; 32]> {
        self.keys
            .iter()
            .find(|key| key.key_id == key_id)
            .map(|key| &key.public_key)
    }

    #[cfg(test)]
    fn for_test(key_id: &'static str, public_key: [u8; 32]) -> Self {
        Self {
            keys: vec![CatalogTrustKey { key_id, public_key }],
        }
    }
}

impl Default for CatalogTrustStore {
    fn default() -> Self {
        Self::bundled()
    }
}

/// Secret-free stable reason a catalog operation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogErrorKind {
    InputTooLarge,
    MalformedEnvelope,
    UnsupportedEnvelopeVersion,
    InvalidKeyId,
    UnknownSigningKey,
    InvalidBase64,
    InvalidSignature,
    PayloadTooLarge,
    NonCanonicalPayload,
    MalformedPayload,
    UnsupportedSchema,
    InvalidCatalogId,
    InvalidRevision,
    RevisionRollback,
    InvalidTimeWindow,
    NotYetEffective,
    Expired,
    EmptyCatalog,
    TooManyEntries,
    DuplicateEntry,
    InvalidManifest,
    InvalidModelEntry,
    InvalidModelMatch,
    InvalidCapability,
    InvalidParameter,
    InvalidSource,
    InvalidLayer,
    HistoryFull,
    HistoryConflict,
    RollbackTargetMissing,
    RollbackPlanStale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogError {
    kind: CatalogErrorKind,
}

impl CatalogError {
    fn new(kind: CatalogErrorKind) -> Self {
        Self { kind }
    }

    pub fn kind(self) -> CatalogErrorKind {
        self.kind
    }
}

impl fmt::Display for CatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.kind {
            CatalogErrorKind::InputTooLarge => "catalog input exceeds the size limit",
            CatalogErrorKind::MalformedEnvelope => "catalog envelope is malformed",
            CatalogErrorKind::UnsupportedEnvelopeVersion => {
                "catalog envelope version is unsupported"
            }
            CatalogErrorKind::InvalidKeyId => "catalog signing key ID is invalid",
            CatalogErrorKind::UnknownSigningKey => "catalog signing key is not trusted",
            CatalogErrorKind::InvalidBase64 => "catalog envelope contains invalid base64",
            CatalogErrorKind::InvalidSignature => "catalog signature verification failed",
            CatalogErrorKind::PayloadTooLarge => "catalog payload exceeds the size limit",
            CatalogErrorKind::NonCanonicalPayload => "catalog payload is not canonical JSON",
            CatalogErrorKind::MalformedPayload => "catalog payload is malformed",
            CatalogErrorKind::UnsupportedSchema => "catalog schema version is unsupported",
            CatalogErrorKind::InvalidCatalogId => "catalog identifier is invalid",
            CatalogErrorKind::InvalidRevision => "catalog revision is invalid",
            CatalogErrorKind::RevisionRollback => {
                "catalog revision would replay or roll back a signed update"
            }
            CatalogErrorKind::InvalidTimeWindow => "catalog time window is invalid",
            CatalogErrorKind::NotYetEffective => "catalog update is not effective yet",
            CatalogErrorKind::Expired => "catalog update has expired",
            CatalogErrorKind::EmptyCatalog => "catalog contains no manifest or model entries",
            CatalogErrorKind::TooManyEntries => "catalog contains too many entries",
            CatalogErrorKind::DuplicateEntry => "catalog contains a duplicate stable entry",
            CatalogErrorKind::InvalidManifest => "catalog contains an invalid provider manifest",
            CatalogErrorKind::InvalidModelEntry => "catalog contains invalid model metadata",
            CatalogErrorKind::InvalidModelMatch => "catalog contains an unsafe model match",
            CatalogErrorKind::InvalidCapability => "catalog contains an invalid capability",
            CatalogErrorKind::InvalidParameter => "catalog contains an invalid parameter",
            CatalogErrorKind::InvalidSource => "catalog contains an invalid evidence source",
            CatalogErrorKind::InvalidLayer => "catalog merge layer is invalid",
            CatalogErrorKind::HistoryFull => "catalog history reached its bounded limit",
            CatalogErrorKind::HistoryConflict => "catalog history revision conflicts",
            CatalogErrorKind::RollbackTargetMissing => "catalog rollback target does not exist",
            CatalogErrorKind::RollbackPlanStale => "catalog rollback plan is stale",
        })
    }
}

impl Error for CatalogError {}

/// JSON file wrapper for one manually imported signed update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedCatalogEnvelope {
    pub envelope_version: u32,
    pub signing_key_id: String,
    pub payload_base64: String,
    pub signature_base64: String,
}

/// Canonical, signed catalog content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogPayload {
    pub schema_version: u32,
    pub catalog_id: String,
    pub revision: u64,
    pub issued_at: DateTime<Utc>,
    pub effective_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub manifests: Vec<CatalogManifestEntry>,
    pub models: Vec<CatalogModelEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogManifestEntry {
    pub template: ProviderTemplate,
    pub verified_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub sources: Vec<CatalogSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogSource {
    pub url: HttpUrl,
    pub content_sha256: String,
    pub verified_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModelMatch {
    /// A family-level fallback which applies to every model listed by the
    /// associated provider template. It carries no model-name assumptions.
    AnyModel,
    Exact {
        model_id: String,
    },
    Glob {
        pattern: String,
    },
}

impl ModelMatch {
    pub fn matches(&self, model_id: &str) -> bool {
        match self {
            Self::AnyModel => true,
            Self::Exact { model_id: expected } => expected == model_id,
            Self::Glob { pattern } => {
                let Some((prefix, suffix)) = pattern.split_once('*') else {
                    return false;
                };
                model_id.len() >= prefix.len() + suffix.len()
                    && model_id.starts_with(prefix)
                    && model_id.ends_with(suffix)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogCapabilityKey {
    Streaming,
    Reasoning,
    StructuredOutput,
    ToolCalling,
    PromptCaching,
    ContextTokens,
    MaxOutputTokens,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum CatalogCapabilityValue {
    /// The bundled family contract does not establish a model-specific value.
    ///
    /// A higher-authority signed catalog, provider API, probe, or user
    /// override may replace this with observed metadata.
    Unknown,
    Boolean(bool),
    Integer(u64),
    EnumValues(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogCapability {
    pub key: CatalogCapabilityKey,
    pub value: CatalogCapabilityValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelLifecycle {
    Unknown,
    Active,
    Preview,
    Deprecated,
    Retired,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogModelEntry {
    pub id: String,
    pub provider_template_id: ProviderTemplateId,
    pub model_match: ModelMatch,
    pub api_family: ApiFamily,
    pub metadata_version: u32,
    pub capabilities: Vec<CatalogCapability>,
    pub parameters: Vec<ParameterSpec>,
    pub lifecycle: ModelLifecycle,
    pub sources: Vec<CatalogSource>,
    pub verified_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

/// Persisted anti-rollback state. Local history rollback does not lower it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogRevisionGuard {
    pub highest_accepted_revision: u64,
    pub latest_issued_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VerifiedCatalogUpdate {
    signing_key_id: String,
    payload_sha256: String,
    payload: CatalogPayload,
    /// Must be committed atomically with the accepted payload after review.
    next_revision_guard: CatalogRevisionGuard,
}

impl VerifiedCatalogUpdate {
    pub fn signing_key_id(&self) -> &str {
        &self.signing_key_id
    }

    pub fn payload_sha256(&self) -> &str {
        &self.payload_sha256
    }

    pub fn payload(&self) -> &CatalogPayload {
        &self.payload
    }

    pub fn next_revision_guard(&self) -> &CatalogRevisionGuard {
        &self.next_revision_guard
    }

    /// Return the exact canonical JSON bytes whose SHA-256 is exposed by
    /// [`Self::payload_sha256`].
    ///
    /// Durable storage uses these bytes rather than reserializing the typed
    /// payload with an encoder whose object-key order may differ.
    pub fn canonical_payload_json(&self) -> Result<Vec<u8>, CatalogError> {
        canonical_json(&self.payload)
    }

    /// Split a verified update into storage-friendly owned values.
    ///
    /// The payload and revision guard must be committed atomically. Reloading
    /// persisted external content must start from the original signed envelope
    /// and call [`verify_manual_catalog_import`] again; this type intentionally
    /// does not implement `Deserialize`.
    pub fn into_parts(self) -> (String, String, CatalogPayload, CatalogRevisionGuard) {
        (
            self.signing_key_id,
            self.payload_sha256,
            self.payload,
            self.next_revision_guard,
        )
    }
}

/// Material handed to an offline Ed25519 signer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CatalogSigningRequest {
    pub envelope_version: u32,
    pub signing_key_id: String,
    pub payload_base64: String,
    pub payload_sha256: String,
    #[serde(skip)]
    signature_frame: Vec<u8>,
}

impl CatalogSigningRequest {
    pub fn signature_frame(&self) -> &[u8] {
        &self.signature_frame
    }

    pub fn into_envelope(
        self,
        signature_bytes: &[u8],
    ) -> Result<SignedCatalogEnvelope, CatalogError> {
        if signature_bytes.len() != ED25519_SIGNATURE_BYTES {
            return Err(CatalogError::new(CatalogErrorKind::InvalidSignature));
        }
        Ok(SignedCatalogEnvelope {
            envelope_version: self.envelope_version,
            signing_key_id: self.signing_key_id,
            payload_base64: self.payload_base64,
            signature_base64: BASE64.encode(signature_bytes),
        })
    }
}

/// Produce deterministic bytes for an offline catalog signing workflow.
pub fn prepare_catalog_signing(
    payload: &CatalogPayload,
    signing_key_id: &str,
) -> Result<CatalogSigningRequest, CatalogError> {
    validate_key_id(signing_key_id)?;
    validate_payload_structure(payload, CatalogPayloadSource::Signed)?;
    validate_new_revision_namespace(payload.revision)?;
    let payload_bytes = canonical_json(payload)?;
    if payload_bytes.len() > MAX_PAYLOAD_BYTES {
        return Err(CatalogError::new(CatalogErrorKind::PayloadTooLarge));
    }
    let signature_frame =
        signature_frame(CATALOG_ENVELOPE_VERSION, signing_key_id, &payload_bytes)?;
    Ok(CatalogSigningRequest {
        envelope_version: CATALOG_ENVELOPE_VERSION,
        signing_key_id: signing_key_id.to_owned(),
        payload_base64: BASE64.encode(&payload_bytes),
        payload_sha256: sha256_hex(&payload_bytes),
        signature_frame,
    })
}

/// Verify a manually selected local catalog file against bundled trust roots.
pub fn verify_manual_catalog_import(
    envelope_json: &[u8],
    revision_guard: &CatalogRevisionGuard,
    now: DateTime<Utc>,
) -> Result<VerifiedCatalogUpdate, CatalogError> {
    verify_catalog_import_with_trust(
        envelope_json,
        &CatalogTrustStore::bundled(),
        revision_guard,
        now,
    )
}

/// Re-verify an update that was already accepted by an earlier `LorePia`
/// release. Stored history predates the schema-v1 namespace reservation, so
/// replay preserves the old positive, monotonic revision contract while still
/// rechecking the exact signature, canonical payload, and acceptance window.
pub fn verify_stored_catalog_update(
    envelope_json: &[u8],
    revision_guard: &CatalogRevisionGuard,
    accepted_at: DateTime<Utc>,
) -> Result<VerifiedCatalogUpdate, CatalogError> {
    verify_stored_catalog_update_with_trust(
        envelope_json,
        &CatalogTrustStore::bundled(),
        revision_guard,
        accepted_at,
    )
}

fn verify_catalog_import_with_trust(
    envelope_json: &[u8],
    trust_store: &CatalogTrustStore,
    revision_guard: &CatalogRevisionGuard,
    now: DateTime<Utc>,
) -> Result<VerifiedCatalogUpdate, CatalogError> {
    verify_catalog_update_with_trust(
        envelope_json,
        trust_store,
        revision_guard,
        now,
        CatalogRevisionPolicy::NewImport,
    )
}

fn verify_stored_catalog_update_with_trust(
    envelope_json: &[u8],
    trust_store: &CatalogTrustStore,
    revision_guard: &CatalogRevisionGuard,
    accepted_at: DateTime<Utc>,
) -> Result<VerifiedCatalogUpdate, CatalogError> {
    verify_catalog_update_with_trust(
        envelope_json,
        trust_store,
        revision_guard,
        accepted_at,
        CatalogRevisionPolicy::StoredReplay,
    )
}

fn verify_catalog_update_with_trust(
    envelope_json: &[u8],
    trust_store: &CatalogTrustStore,
    revision_guard: &CatalogRevisionGuard,
    now: DateTime<Utc>,
    revision_policy: CatalogRevisionPolicy,
) -> Result<VerifiedCatalogUpdate, CatalogError> {
    if envelope_json.len() > MAX_ENVELOPE_BYTES {
        return Err(CatalogError::new(CatalogErrorKind::InputTooLarge));
    }
    let envelope: SignedCatalogEnvelope = serde_json::from_slice(envelope_json)
        .map_err(|_| CatalogError::new(CatalogErrorKind::MalformedEnvelope))?;
    if envelope.envelope_version != CATALOG_ENVELOPE_VERSION {
        return Err(CatalogError::new(
            CatalogErrorKind::UnsupportedEnvelopeVersion,
        ));
    }
    validate_key_id(&envelope.signing_key_id)?;
    if envelope.payload_base64.len() > MAX_PAYLOAD_BASE64_BYTES {
        return Err(CatalogError::new(CatalogErrorKind::PayloadTooLarge));
    }

    let public_key = trust_store
        .public_key(&envelope.signing_key_id)
        .ok_or_else(|| CatalogError::new(CatalogErrorKind::UnknownSigningKey))?;
    let payload_bytes = decode_canonical_base64(&envelope.payload_base64)?;
    if payload_bytes.len() > MAX_PAYLOAD_BYTES {
        return Err(CatalogError::new(CatalogErrorKind::PayloadTooLarge));
    }
    if envelope.signature_base64.len() != SIGNATURE_BASE64_BYTES {
        return Err(CatalogError::new(CatalogErrorKind::InvalidSignature));
    }
    let signature_bytes = decode_canonical_base64(&envelope.signature_base64)?;
    if signature_bytes.len() != ED25519_SIGNATURE_BYTES {
        return Err(CatalogError::new(CatalogErrorKind::InvalidSignature));
    }
    let frame = signature_frame(
        envelope.envelope_version,
        &envelope.signing_key_id,
        &payload_bytes,
    )?;
    signature::UnparsedPublicKey::new(&signature::ED25519, public_key)
        .verify(&frame, &signature_bytes)
        .map_err(|_| CatalogError::new(CatalogErrorKind::InvalidSignature))?;

    let payload: CatalogPayload = serde_json::from_slice(&payload_bytes)
        .map_err(|_| CatalogError::new(CatalogErrorKind::MalformedPayload))?;
    let canonical = canonical_json(&payload)?;
    if canonical != payload_bytes {
        return Err(CatalogError::new(CatalogErrorKind::NonCanonicalPayload));
    }
    validate_payload_structure(&payload, CatalogPayloadSource::Signed)?;
    validate_signed_time_window(&payload, now)?;
    validate_revision(&payload, revision_guard, revision_policy)?;

    Ok(VerifiedCatalogUpdate {
        signing_key_id: envelope.signing_key_id,
        payload_sha256: sha256_hex(&payload_bytes),
        next_revision_guard: CatalogRevisionGuard {
            highest_accepted_revision: payload.revision,
            latest_issued_at: Some(payload.issued_at),
        },
        payload,
    })
}

fn decode_canonical_base64(value: &str) -> Result<Vec<u8>, CatalogError> {
    let decoded = BASE64
        .decode(value)
        .map_err(|_| CatalogError::new(CatalogErrorKind::InvalidBase64))?;
    if BASE64.encode(&decoded) != value {
        return Err(CatalogError::new(CatalogErrorKind::InvalidBase64));
    }
    Ok(decoded)
}

fn signature_frame(
    envelope_version: u32,
    signing_key_id: &str,
    payload: &[u8],
) -> Result<Vec<u8>, CatalogError> {
    validate_key_id(signing_key_id)?;
    let key_length = u16::try_from(signing_key_id.len())
        .map_err(|_| CatalogError::new(CatalogErrorKind::InvalidKeyId))?;
    let payload_length = u64::try_from(payload.len())
        .map_err(|_| CatalogError::new(CatalogErrorKind::PayloadTooLarge))?;
    let mut frame = Vec::with_capacity(
        SIGNATURE_DOMAIN.len() + 4 + 2 + signing_key_id.len() + 8 + payload.len(),
    );
    frame.extend_from_slice(SIGNATURE_DOMAIN);
    frame.extend_from_slice(&envelope_version.to_be_bytes());
    frame.extend_from_slice(&key_length.to_be_bytes());
    frame.extend_from_slice(signing_key_id.as_bytes());
    frame.extend_from_slice(&payload_length.to_be_bytes());
    frame.extend_from_slice(payload);
    Ok(frame)
}

fn validate_key_id(value: &str) -> Result<(), CatalogError> {
    if value.is_empty()
        || value.len() > MAX_KEY_ID_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(CatalogError::new(CatalogErrorKind::InvalidKeyId));
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum CatalogPayloadSource {
    Bundled,
    Signed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CatalogRevisionPolicy {
    NewImport,
    StoredReplay,
}

fn validate_payload_structure(
    payload: &CatalogPayload,
    source: CatalogPayloadSource,
) -> Result<(), CatalogError> {
    if payload.schema_version != CATALOG_SCHEMA_VERSION {
        return Err(CatalogError::new(CatalogErrorKind::UnsupportedSchema));
    }
    if payload.catalog_id != CATALOG_ID {
        return Err(CatalogError::new(CatalogErrorKind::InvalidCatalogId));
    }
    if payload.revision == 0 {
        return Err(CatalogError::new(CatalogErrorKind::InvalidRevision));
    }
    if payload.manifests.is_empty() && payload.models.is_empty() {
        return Err(CatalogError::new(CatalogErrorKind::EmptyCatalog));
    }
    if payload.manifests.len() > MAX_CATALOG_MANIFESTS || payload.models.len() > MAX_CATALOG_MODELS
    {
        return Err(CatalogError::new(CatalogErrorKind::TooManyEntries));
    }
    validate_payload_time_order(payload)?;

    let expected_template_source = match source {
        CatalogPayloadSource::Bundled => TemplateSource::BuiltIn,
        CatalogPayloadSource::Signed => TemplateSource::SignedCatalog,
    };
    let mut manifest_keys = BTreeSet::new();
    for entry in &payload.manifests {
        let key = (
            entry.template.id.as_str().to_owned(),
            entry.template.manifest_version,
        );
        if !manifest_keys.insert(key) {
            return Err(CatalogError::new(CatalogErrorKind::DuplicateEntry));
        }
        validate_manifest_entry(entry, expected_template_source, payload)?;
    }

    let mut model_ids = BTreeSet::new();
    for entry in &payload.models {
        if !model_ids.insert(entry.id.as_str()) {
            return Err(CatalogError::new(CatalogErrorKind::DuplicateEntry));
        }
        validate_model_entry(entry, payload)?;
    }
    Ok(())
}

fn validate_payload_time_order(payload: &CatalogPayload) -> Result<(), CatalogError> {
    if payload.issued_at > payload.effective_at
        || payload.effective_at >= payload.expires_at
        || payload.expires_at - payload.issued_at > Duration::days(MAX_CATALOG_VALIDITY_DAYS)
    {
        return Err(CatalogError::new(CatalogErrorKind::InvalidTimeWindow));
    }
    Ok(())
}

fn validate_signed_time_window(
    payload: &CatalogPayload,
    now: DateTime<Utc>,
) -> Result<(), CatalogError> {
    if payload.issued_at > now + Duration::minutes(CLOCK_SKEW_MINUTES) {
        return Err(CatalogError::new(CatalogErrorKind::InvalidTimeWindow));
    }
    if payload.effective_at > now {
        return Err(CatalogError::new(CatalogErrorKind::NotYetEffective));
    }
    if payload.expires_at <= now {
        return Err(CatalogError::new(CatalogErrorKind::Expired));
    }
    Ok(())
}

fn validate_revision(
    payload: &CatalogPayload,
    guard: &CatalogRevisionGuard,
    policy: CatalogRevisionPolicy,
) -> Result<(), CatalogError> {
    if payload.revision <= guard.highest_accepted_revision
        || guard
            .latest_issued_at
            .is_some_and(|issued_at| payload.issued_at < issued_at)
    {
        return Err(CatalogError::new(CatalogErrorKind::RevisionRollback));
    }
    if policy == CatalogRevisionPolicy::NewImport
        && (payload.revision > CATALOG_REVISION_V1_MAX
            || guard.highest_accepted_revision > CATALOG_REVISION_V1_MAX
            || payload.revision - guard.highest_accepted_revision > CATALOG_REVISION_MAX_ADVANCE)
    {
        return Err(CatalogError::new(CatalogErrorKind::InvalidRevision));
    }
    Ok(())
}

fn validate_new_revision_namespace(revision: u64) -> Result<(), CatalogError> {
    if revision > CATALOG_REVISION_V1_MAX {
        return Err(CatalogError::new(CatalogErrorKind::InvalidRevision));
    }
    Ok(())
}

fn validate_manifest_entry(
    entry: &CatalogManifestEntry,
    expected_source: TemplateSource,
    payload: &CatalogPayload,
) -> Result<(), CatalogError> {
    if !is_catalog_identifier(entry.template.id.as_str(), MAX_MODEL_ENTRY_ID_BYTES)
        || entry.template.display_name.trim().is_empty()
        || entry.template.display_name.len() > MAX_DISPLAY_NAME_BYTES
        || entry.template.display_name.chars().any(char::is_control)
        || entry.template.manifest_version == 0
        || entry.template.source != expected_source
        || entry.template.api_family != entry.template.default_manifest.api_family
        || entry.verified_at > payload.issued_at + Duration::minutes(CLOCK_SKEW_MINUTES)
        || entry.expires_at.is_some_and(|expires_at| {
            expires_at <= entry.verified_at || expires_at > payload.expires_at
        })
    {
        return Err(CatalogError::new(CatalogErrorKind::InvalidManifest));
    }
    validate_connection_fields(&entry.template.connection_fields)
        .map_err(|_| CatalogError::new(CatalogErrorKind::InvalidManifest))?;
    validate_manifest(&entry.template.default_manifest)
        .map_err(|_| CatalogError::new(CatalogErrorKind::InvalidManifest))?;
    validate_catalog_sources(&entry.sources, payload.issued_at)
}

fn validate_model_entry(
    entry: &CatalogModelEntry,
    payload: &CatalogPayload,
) -> Result<(), CatalogError> {
    if !is_catalog_identifier(&entry.id, MAX_MODEL_ENTRY_ID_BYTES)
        || !is_catalog_identifier(
            entry.provider_template_id.as_str(),
            MAX_MODEL_ENTRY_ID_BYTES,
        )
        || entry.metadata_version == 0
        || entry.capabilities.len() > MAX_CAPABILITIES
        || entry.parameters.len() > MAX_PARAMETERS
        || entry.verified_at > payload.issued_at + Duration::minutes(CLOCK_SKEW_MINUTES)
        || entry.expires_at.is_some_and(|expires_at| {
            expires_at <= entry.verified_at || expires_at > payload.expires_at
        })
    {
        return Err(CatalogError::new(CatalogErrorKind::InvalidModelEntry));
    }
    validate_model_match(&entry.model_match)?;
    validate_capabilities(&entry.capabilities)?;
    validate_model_parameters(entry.api_family, &entry.parameters)?;
    validate_catalog_sources(&entry.sources, payload.issued_at)
}

fn validate_model_match(model_match: &ModelMatch) -> Result<(), CatalogError> {
    let value = match model_match {
        ModelMatch::AnyModel => return Ok(()),
        ModelMatch::Exact { model_id } => {
            if model_id.contains('*') {
                return Err(CatalogError::new(CatalogErrorKind::InvalidModelMatch));
            }
            model_id
        }
        ModelMatch::Glob { pattern } => {
            if pattern.matches('*').count() != 1 || pattern == "*" {
                return Err(CatalogError::new(CatalogErrorKind::InvalidModelMatch));
            }
            pattern
        }
    };
    if value.is_empty()
        || value.len() > MAX_MODEL_PATTERN_BYTES
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/' | b'@' | b'*')
        })
    {
        return Err(CatalogError::new(CatalogErrorKind::InvalidModelMatch));
    }
    Ok(())
}

fn validate_capabilities(capabilities: &[CatalogCapability]) -> Result<(), CatalogError> {
    let mut keys = BTreeSet::new();
    for capability in capabilities {
        if !keys.insert(capability.key) {
            return Err(CatalogError::new(CatalogErrorKind::DuplicateEntry));
        }
        match (&capability.key, &capability.value) {
            (_, CatalogCapabilityValue::Unknown)
            | (
                CatalogCapabilityKey::Streaming
                | CatalogCapabilityKey::Reasoning
                | CatalogCapabilityKey::StructuredOutput
                | CatalogCapabilityKey::ToolCalling
                | CatalogCapabilityKey::PromptCaching,
                CatalogCapabilityValue::Boolean(_),
            ) => {}
            (
                CatalogCapabilityKey::ContextTokens | CatalogCapabilityKey::MaxOutputTokens,
                CatalogCapabilityValue::Integer(value),
            ) if *value > 0 => {}
            (_, CatalogCapabilityValue::EnumValues(values))
                if !values.is_empty() && values.len() <= MAX_ENUM_VALUES =>
            {
                let mut unique = BTreeSet::new();
                for value in values {
                    if value.is_empty()
                        || value.len() > MAX_ENUM_VALUE_BYTES
                        || value.chars().any(char::is_control)
                        || !unique.insert(value)
                    {
                        return Err(CatalogError::new(CatalogErrorKind::InvalidCapability));
                    }
                }
            }
            _ => return Err(CatalogError::new(CatalogErrorKind::InvalidCapability)),
        }
    }
    Ok(())
}

fn validate_model_parameters(
    api_family: ApiFamily,
    parameters: &[ParameterSpec],
) -> Result<(), CatalogError> {
    let (response, streaming) = match api_family {
        ApiFamily::OpenAiResponses | ApiFamily::OpenAiChatCompletions => {
            (DecoderId::OpenAiJsonV1, DecoderId::OpenAiSseV1)
        }
        ApiFamily::AnthropicMessages => (DecoderId::AnthropicJsonV1, DecoderId::AnthropicSseV1),
        ApiFamily::GeminiGenerateContent => (DecoderId::GeminiJsonV1, DecoderId::GeminiSseV1),
        ApiFamily::OllamaNative => (DecoderId::OllamaJsonV1, DecoderId::OllamaJsonlV1),
    };
    let validation_manifest = ProviderManifest {
        schema_version: 1,
        api_family,
        sources: Vec::new(),
        default_api_origin: None,
        auth: AuthBinding::None,
        endpoints: ManifestEndpoints {
            models: None,
            generate: EndpointSpec {
                method: HttpMethod::Post,
                path: EndpointPath::parse("/catalog-validation")
                    .map_err(|_| CatalogError::new(CatalogErrorKind::InvalidParameter))?,
            },
            embeddings: None,
        },
        decoders: ManifestDecoders {
            response,
            streaming: Some(streaming),
        },
        parameters: parameters.to_vec(),
    };
    validate_manifest(&validation_manifest)
        .map_err(|_| CatalogError::new(CatalogErrorKind::InvalidParameter))?;
    Ok(())
}

fn validate_catalog_sources(
    sources: &[CatalogSource],
    payload_issued_at: DateTime<Utc>,
) -> Result<(), CatalogError> {
    validate_catalog_source_basics(sources)?;
    if sources.iter().any(|source| {
        source.verified_at > payload_issued_at + Duration::minutes(CLOCK_SKEW_MINUTES)
    }) {
        return Err(CatalogError::new(CatalogErrorKind::InvalidSource));
    }
    Ok(())
}

fn validate_catalog_source_basics(sources: &[CatalogSource]) -> Result<(), CatalogError> {
    if sources.is_empty() || sources.len() > MAX_SOURCES {
        return Err(CatalogError::new(CatalogErrorKind::InvalidSource));
    }
    let mut urls = BTreeSet::new();
    for source in sources {
        let url = Url::parse(source.url.as_str())
            .map_err(|_| CatalogError::new(CatalogErrorKind::InvalidSource))?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || url.query().is_some()
            || url.fragment().is_some()
            || !urls.insert(source.url.as_str())
            || !is_lower_hex_sha256(&source.content_sha256)
        {
            return Err(CatalogError::new(CatalogErrorKind::InvalidSource));
        }
    }
    Ok(())
}

fn is_catalog_identifier(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
        })
}

fn is_lower_hex_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogAuthority {
    Bundled,
    SignedCatalog,
    OfficialDocumentation,
    ProviderApi,
    CapabilityProbe,
    UserOverride,
}

impl CatalogAuthority {
    fn precedence(self) -> u8 {
        match self {
            Self::Bundled => 0,
            Self::SignedCatalog => 1,
            Self::OfficialDocumentation => 2,
            Self::ProviderApi => 3,
            Self::CapabilityProbe => 4,
            Self::UserOverride => 5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogFreshness {
    Current,
    Stale,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogFreshnessPolicy {
    pub bundled_max_age_seconds: u64,
    pub signed_max_age_seconds: u64,
    pub documentation_max_age_seconds: u64,
    pub provider_api_max_age_seconds: u64,
    pub probe_max_age_seconds: u64,
}

impl Default for CatalogFreshnessPolicy {
    fn default() -> Self {
        Self {
            bundled_max_age_seconds: 180 * 24 * 60 * 60,
            signed_max_age_seconds: 90 * 24 * 60 * 60,
            documentation_max_age_seconds: 60 * 24 * 60 * 60,
            provider_api_max_age_seconds: 7 * 24 * 60 * 60,
            probe_max_age_seconds: 30 * 24 * 60 * 60,
        }
    }
}

impl CatalogFreshnessPolicy {
    fn max_age_seconds(&self, authority: CatalogAuthority) -> Option<u64> {
        Some(match authority {
            CatalogAuthority::Bundled => self.bundled_max_age_seconds,
            CatalogAuthority::SignedCatalog => self.signed_max_age_seconds,
            CatalogAuthority::OfficialDocumentation => self.documentation_max_age_seconds,
            CatalogAuthority::ProviderApi => self.provider_api_max_age_seconds,
            CatalogAuthority::CapabilityProbe => self.probe_max_age_seconds,
            CatalogAuthority::UserOverride => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogLayer {
    pub id: String,
    pub authority: CatalogAuthority,
    pub revision: u64,
    pub effective_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub manifests: Vec<CatalogManifestEntry>,
    pub models: Vec<CatalogModelEntry>,
}

/// Build-trusted provider baseline compiled into this binary.
///
/// The baseline is constructed exclusively from the closed
/// [`AdapterRegistry`] contracts and never reads a file, database, or network
/// resource. The hash covers the complete canonical layer, including its fixed
/// provenance timestamp.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BundledCatalogBaseline {
    layer: CatalogLayer,
    canonical_sha256: String,
}

impl BundledCatalogBaseline {
    pub fn layer(&self) -> &CatalogLayer {
        &self.layer
    }

    pub fn canonical_sha256(&self) -> &str {
        &self.canonical_sha256
    }

    pub fn into_layer(self) -> CatalogLayer {
        self.layer
    }
}

impl CatalogLayer {
    pub fn from_bundled(payload: CatalogPayload) -> Result<Self, CatalogError> {
        validate_payload_structure(&payload, CatalogPayloadSource::Bundled)?;
        Ok(Self {
            id: format!("bundled:{}", payload.revision),
            authority: CatalogAuthority::Bundled,
            revision: payload.revision,
            effective_at: payload.effective_at,
            expires_at: Some(payload.expires_at),
            manifests: payload.manifests,
            models: payload.models,
        })
    }

    /// Return a deterministic canonical hash after validating this layer.
    pub fn canonical_sha256(&self) -> Result<String, CatalogError> {
        validate_layers(std::slice::from_ref(self))?;
        canonical_hash(self)
    }

    pub fn from_verified_update(update: VerifiedCatalogUpdate) -> Self {
        let payload = update.payload;
        Self {
            id: format!("signed:{}:{}", update.signing_key_id, payload.revision),
            authority: CatalogAuthority::SignedCatalog,
            revision: payload.revision,
            effective_at: payload.effective_at,
            expires_at: Some(payload.expires_at),
            manifests: payload.manifests,
            models: payload.models,
        }
    }
}

/// Construct the offline baseline for every compiled-in provider family.
///
/// Each model entry is an explicit family fallback (`AnyModel`). It publishes
/// only the parameter mappings already enforced by the compiled adapter and
/// records model-specific capabilities and token limits as `Unknown`.
pub fn bundled_catalog_baseline() -> Result<BundledCatalogBaseline, CatalogError> {
    let verified_at = DateTime::parse_from_rfc3339(BUNDLED_CATALOG_VERIFIED_AT)
        .map_err(|_| CatalogError::new(CatalogErrorKind::InvalidLayer))?
        .with_timezone(&Utc);
    let templates = AdapterRegistry::built_in_templates()
        .map_err(|_| CatalogError::new(CatalogErrorKind::InvalidManifest))?;
    if templates.len() != BuiltInTemplateId::ALL.len() {
        return Err(CatalogError::new(CatalogErrorKind::InvalidLayer));
    }

    let mut manifests = Vec::with_capacity(templates.len());
    let mut models = Vec::with_capacity(templates.len());
    for template in templates {
        let template_id = template.id.as_str().to_owned();
        let mut manifest_entry = CatalogManifestEntry {
            template: template.clone(),
            verified_at,
            expires_at: None,
            sources: Vec::new(),
        };
        let manifest_hash = canonical_hash(&manifest_entry)?;
        manifest_entry.sources.push(bundled_source(
            &template_id,
            "manifest",
            manifest_hash,
            verified_at,
        )?);

        let mut model_entry = CatalogModelEntry {
            id: format!("builtin:{template_id}:family-default"),
            provider_template_id: template.id.clone(),
            model_match: ModelMatch::AnyModel,
            api_family: template.api_family,
            metadata_version: template.manifest_version,
            capabilities: generic_unknown_capabilities(),
            parameters: template.default_manifest.parameters.clone(),
            lifecycle: ModelLifecycle::Unknown,
            sources: Vec::new(),
            verified_at,
            expires_at: None,
        };
        let model_hash = canonical_hash(&model_entry)?;
        model_entry.sources.push(bundled_source(
            &template_id,
            "model-family-default",
            model_hash,
            verified_at,
        )?);

        manifests.push(manifest_entry);
        models.push(model_entry);
    }

    let layer = CatalogLayer {
        id: BUNDLED_CATALOG_LAYER_ID.to_owned(),
        authority: CatalogAuthority::Bundled,
        revision: BUNDLED_CATALOG_REVISION,
        effective_at: verified_at,
        expires_at: None,
        manifests,
        models,
    };
    validate_layers(std::slice::from_ref(&layer))?;
    let canonical_sha256 = canonical_hash(&layer)?;
    Ok(BundledCatalogBaseline {
        layer,
        canonical_sha256,
    })
}

/// Merge the compiled baseline with verified signed updates and local evidence.
///
/// `runtime_layers` may contain documentation, provider API, capability probe,
/// and user override observations. Bundled or signed authorities are rejected
/// there: compiled content must come from [`bundled_catalog_baseline`], while
/// external signed content must first pass [`verify_manual_catalog_import`].
pub fn merge_with_bundled_catalog(
    verified_signed_updates: &[VerifiedCatalogUpdate],
    runtime_layers: &[CatalogLayer],
    now: DateTime<Utc>,
    freshness_policy: &CatalogFreshnessPolicy,
) -> Result<MergedCatalog, CatalogError> {
    if runtime_layers.iter().any(|layer| {
        matches!(
            layer.authority,
            CatalogAuthority::Bundled | CatalogAuthority::SignedCatalog
        )
    }) {
        return Err(CatalogError::new(CatalogErrorKind::InvalidLayer));
    }

    let mut layers = Vec::with_capacity(1 + verified_signed_updates.len() + runtime_layers.len());
    layers.push(bundled_catalog_baseline()?.into_layer());
    layers.extend(
        verified_signed_updates
            .iter()
            .cloned()
            .map(CatalogLayer::from_verified_update),
    );
    layers.extend_from_slice(runtime_layers);
    merge_catalog_layers(&layers, now, freshness_policy)
}

fn generic_unknown_capabilities() -> Vec<CatalogCapability> {
    [
        CatalogCapabilityKey::Streaming,
        CatalogCapabilityKey::Reasoning,
        CatalogCapabilityKey::StructuredOutput,
        CatalogCapabilityKey::ToolCalling,
        CatalogCapabilityKey::PromptCaching,
        CatalogCapabilityKey::ContextTokens,
        CatalogCapabilityKey::MaxOutputTokens,
    ]
    .into_iter()
    .map(|key| CatalogCapability {
        key,
        value: CatalogCapabilityValue::Unknown,
    })
    .collect()
}

fn bundled_source(
    template_id: &str,
    record_kind: &str,
    content_sha256: String,
    verified_at: DateTime<Utc>,
) -> Result<CatalogSource, CatalogError> {
    let url = HttpUrl::parse(&format!(
        "{BUNDLED_SOURCE_ORIGIN}/bundled/{template_id}/{record_kind}"
    ))
    .map_err(|_| CatalogError::new(CatalogErrorKind::InvalidSource))?;
    Ok(CatalogSource {
        url,
        content_sha256,
        verified_at,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogFieldProvenance {
    pub authority: CatalogAuthority,
    pub layer_id: String,
    pub revision: u64,
    pub verified_at: DateTime<Utc>,
    pub freshness: CatalogFreshness,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MergedCatalogManifest {
    pub entry: CatalogManifestEntry,
    pub provenance: CatalogFieldProvenance,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MergedCatalogModel {
    pub entry: CatalogModelEntry,
    pub metadata_provenance: CatalogFieldProvenance,
    pub capability_provenance: BTreeMap<CatalogCapabilityKey, CatalogFieldProvenance>,
    pub parameter_provenance: BTreeMap<String, CatalogFieldProvenance>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CatalogMergeReport {
    pub ignored_expired_layers: Vec<String>,
    pub ignored_expired_manifest_ids: Vec<String>,
    pub ignored_expired_model_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MergedCatalog {
    pub manifests: Vec<MergedCatalogManifest>,
    pub models: Vec<MergedCatalogModel>,
    pub report: CatalogMergeReport,
}

/// Merge trusted inputs with explicit authority and freshness provenance.
///
/// Higher authority overrides lower authority. Within one authority, later
/// revisions win. Stale values remain visible and marked; expired values are
/// excluded so a lower, non-expired value can remain active.
pub fn merge_catalog_layers(
    layers: &[CatalogLayer],
    now: DateTime<Utc>,
    freshness_policy: &CatalogFreshnessPolicy,
) -> Result<MergedCatalog, CatalogError> {
    validate_layers(layers)?;
    let mut ordered = layers.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| {
        (
            left.authority.precedence(),
            left.revision,
            left.effective_at,
            left.id.as_str(),
        )
            .cmp(&(
                right.authority.precedence(),
                right.revision,
                right.effective_at,
                right.id.as_str(),
            ))
    });

    let mut manifests = BTreeMap::<String, MergedCatalogManifest>::new();
    let mut models = BTreeMap::<String, MergedCatalogModel>::new();
    let mut report = CatalogMergeReport::default();

    for layer in ordered {
        if layer.effective_at > now {
            continue;
        }
        if layer.expires_at.is_some_and(|expires_at| expires_at <= now) {
            report.ignored_expired_layers.push(layer.id.clone());
            continue;
        }
        merge_manifest_layer(layer, now, freshness_policy, &mut manifests, &mut report)?;
        merge_model_layer(layer, now, freshness_policy, &mut models, &mut report)?;
    }

    Ok(MergedCatalog {
        manifests: manifests.into_values().collect(),
        models: models.into_values().collect(),
        report,
    })
}

fn validate_layers(layers: &[CatalogLayer]) -> Result<(), CatalogError> {
    let mut ids = BTreeSet::new();
    for layer in layers {
        if !is_catalog_identifier(&layer.id, MAX_MODEL_ENTRY_ID_BYTES)
            || layer.revision == 0
            || !ids.insert(layer.id.as_str())
            || layer
                .expires_at
                .is_some_and(|expires_at| expires_at <= layer.effective_at)
        {
            return Err(CatalogError::new(CatalogErrorKind::InvalidLayer));
        }
        let mut manifest_ids = BTreeSet::new();
        for entry in &layer.manifests {
            if !manifest_ids.insert(entry.template.id.as_str()) {
                return Err(CatalogError::new(CatalogErrorKind::DuplicateEntry));
            }
            validate_layer_manifest(entry, layer)?;
        }
        let mut model_ids = BTreeSet::new();
        for entry in &layer.models {
            if !model_ids.insert(entry.id.as_str()) {
                return Err(CatalogError::new(CatalogErrorKind::DuplicateEntry));
            }
            validate_layer_model(entry, layer)?;
        }
    }
    Ok(())
}

fn validate_layer_manifest(
    entry: &CatalogManifestEntry,
    layer: &CatalogLayer,
) -> Result<(), CatalogError> {
    let expected_source = match layer.authority {
        CatalogAuthority::Bundled => TemplateSource::BuiltIn,
        CatalogAuthority::SignedCatalog => TemplateSource::SignedCatalog,
        CatalogAuthority::OfficialDocumentation
        | CatalogAuthority::ProviderApi
        | CatalogAuthority::CapabilityProbe
        | CatalogAuthority::UserOverride => TemplateSource::UserDiscovered,
    };
    if !is_catalog_identifier(entry.template.id.as_str(), MAX_MODEL_ENTRY_ID_BYTES)
        || entry.template.display_name.trim().is_empty()
        || entry.template.display_name.len() > MAX_DISPLAY_NAME_BYTES
        || entry.template.display_name.chars().any(char::is_control)
        || entry.template.source != expected_source
        || entry.template.manifest_version == 0
        || entry.template.api_family != entry.template.default_manifest.api_family
        || entry.verified_at > layer.effective_at + Duration::minutes(CLOCK_SKEW_MINUTES)
        || entry
            .expires_at
            .is_some_and(|expires_at| expires_at <= entry.verified_at)
        || matches!(
            (entry.expires_at, layer.expires_at),
            (Some(entry_expiry), Some(layer_expiry)) if entry_expiry > layer_expiry
        )
    {
        return Err(CatalogError::new(CatalogErrorKind::InvalidLayer));
    }
    validate_connection_fields(&entry.template.connection_fields)
        .map_err(|_| CatalogError::new(CatalogErrorKind::InvalidManifest))?;
    validate_manifest(&entry.template.default_manifest)
        .map_err(|_| CatalogError::new(CatalogErrorKind::InvalidManifest))?;
    validate_entry_sources(&entry.sources, entry.verified_at)
}

fn validate_layer_model(
    entry: &CatalogModelEntry,
    layer: &CatalogLayer,
) -> Result<(), CatalogError> {
    if !is_catalog_identifier(&entry.id, MAX_MODEL_ENTRY_ID_BYTES)
        || !is_catalog_identifier(
            entry.provider_template_id.as_str(),
            MAX_MODEL_ENTRY_ID_BYTES,
        )
        || entry.metadata_version == 0
        || entry.verified_at > layer.effective_at + Duration::minutes(CLOCK_SKEW_MINUTES)
        || entry
            .expires_at
            .is_some_and(|expires_at| expires_at <= entry.verified_at)
        || matches!(
            (entry.expires_at, layer.expires_at),
            (Some(entry_expiry), Some(layer_expiry)) if entry_expiry > layer_expiry
        )
    {
        return Err(CatalogError::new(CatalogErrorKind::InvalidLayer));
    }
    validate_model_match(&entry.model_match)?;
    validate_capabilities(&entry.capabilities)?;
    validate_model_parameters(entry.api_family, &entry.parameters)?;
    validate_entry_sources(&entry.sources, entry.verified_at)
}

fn validate_entry_sources(
    sources: &[CatalogSource],
    entry_verified_at: DateTime<Utc>,
) -> Result<(), CatalogError> {
    validate_catalog_source_basics(sources)?;
    if sources.iter().any(|source| {
        source.verified_at > entry_verified_at + Duration::minutes(CLOCK_SKEW_MINUTES)
    }) {
        return Err(CatalogError::new(CatalogErrorKind::InvalidSource));
    }
    Ok(())
}

fn merge_manifest_layer(
    layer: &CatalogLayer,
    now: DateTime<Utc>,
    policy: &CatalogFreshnessPolicy,
    merged: &mut BTreeMap<String, MergedCatalogManifest>,
    report: &mut CatalogMergeReport,
) -> Result<(), CatalogError> {
    for entry in &layer.manifests {
        let freshness = freshness(
            layer.authority,
            entry.verified_at,
            minimum_expiry(layer.expires_at, entry.expires_at),
            now,
            policy,
        );
        if freshness == CatalogFreshness::Expired {
            report
                .ignored_expired_manifest_ids
                .push(entry.template.id.as_str().to_owned());
            continue;
        }
        if let Some(previous) = merged.get(entry.template.id.as_str())
            && (previous.entry.template.api_family != entry.template.api_family
                || (previous.provenance.authority == layer.authority
                    && entry.template.manifest_version < previous.entry.template.manifest_version))
        {
            return Err(CatalogError::new(CatalogErrorKind::InvalidLayer));
        }
        merged.insert(
            entry.template.id.as_str().to_owned(),
            MergedCatalogManifest {
                entry: entry.clone(),
                provenance: provenance(layer, entry.verified_at, freshness),
            },
        );
    }
    Ok(())
}

fn merge_model_layer(
    layer: &CatalogLayer,
    now: DateTime<Utc>,
    policy: &CatalogFreshnessPolicy,
    merged: &mut BTreeMap<String, MergedCatalogModel>,
    report: &mut CatalogMergeReport,
) -> Result<(), CatalogError> {
    for entry in &layer.models {
        let freshness = freshness(
            layer.authority,
            entry.verified_at,
            minimum_expiry(layer.expires_at, entry.expires_at),
            now,
            policy,
        );
        if freshness == CatalogFreshness::Expired {
            report.ignored_expired_model_ids.push(entry.id.clone());
            continue;
        }
        let source = provenance(layer, entry.verified_at, freshness);
        if let Some(previous) = merged.get(&entry.id)
            && (previous.entry.provider_template_id != entry.provider_template_id
                || previous.entry.model_match != entry.model_match
                || previous.entry.api_family != entry.api_family
                || (previous.metadata_provenance.authority == layer.authority
                    && entry.metadata_version < previous.entry.metadata_version))
        {
            return Err(CatalogError::new(CatalogErrorKind::InvalidLayer));
        }
        let target = merged
            .entry(entry.id.clone())
            .or_insert_with(|| MergedCatalogModel {
                entry: entry.clone(),
                metadata_provenance: source.clone(),
                capability_provenance: BTreeMap::new(),
                parameter_provenance: BTreeMap::new(),
            });

        let mut capabilities = target
            .entry
            .capabilities
            .iter()
            .map(|capability| (capability.key, capability.clone()))
            .collect::<BTreeMap<_, _>>();
        for capability in &entry.capabilities {
            capabilities.insert(capability.key, capability.clone());
            target
                .capability_provenance
                .insert(capability.key, source.clone());
        }

        let mut parameters = target
            .entry
            .parameters
            .iter()
            .map(|parameter| (parameter.id.as_str().to_owned(), parameter.clone()))
            .collect::<BTreeMap<_, _>>();
        for parameter in &entry.parameters {
            parameters.insert(parameter.id.as_str().to_owned(), parameter.clone());
            target
                .parameter_provenance
                .insert(parameter.id.as_str().to_owned(), source.clone());
        }

        target.entry = entry.clone();
        target.entry.capabilities = capabilities.into_values().collect();
        target.entry.parameters = parameters.into_values().collect();
        target.metadata_provenance = source;

        for capability in &target.entry.capabilities {
            target
                .capability_provenance
                .entry(capability.key)
                .or_insert_with(|| target.metadata_provenance.clone());
        }
        for parameter in &target.entry.parameters {
            target
                .parameter_provenance
                .entry(parameter.id.as_str().to_owned())
                .or_insert_with(|| target.metadata_provenance.clone());
        }
    }
    Ok(())
}

fn provenance(
    layer: &CatalogLayer,
    verified_at: DateTime<Utc>,
    freshness: CatalogFreshness,
) -> CatalogFieldProvenance {
    CatalogFieldProvenance {
        authority: layer.authority,
        layer_id: layer.id.clone(),
        revision: layer.revision,
        verified_at,
        freshness,
    }
}

fn minimum_expiry(
    left: Option<DateTime<Utc>>,
    right: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn freshness(
    authority: CatalogAuthority,
    verified_at: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    policy: &CatalogFreshnessPolicy,
) -> CatalogFreshness {
    if expires_at.is_some_and(|expires_at| expires_at <= now) {
        return CatalogFreshness::Expired;
    }
    let Some(max_age) = policy.max_age_seconds(authority) else {
        return CatalogFreshness::Current;
    };
    if now.signed_duration_since(verified_at).num_seconds()
        > i64::try_from(max_age).unwrap_or(i64::MAX)
    {
        CatalogFreshness::Stale
    } else {
        CatalogFreshness::Current
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogRevisionSnapshot {
    pub snapshot_schema_version: u32,
    pub revision: u64,
    pub captured_at: DateTime<Utc>,
    pub manifests: Vec<CatalogManifestEntry>,
    pub models: Vec<CatalogModelEntry>,
}

impl CatalogRevisionSnapshot {
    pub fn from_merged(
        revision: u64,
        captured_at: DateTime<Utc>,
        catalog: &MergedCatalog,
    ) -> Result<Self, CatalogError> {
        if revision == 0 {
            return Err(CatalogError::new(CatalogErrorKind::InvalidRevision));
        }
        let snapshot = Self {
            snapshot_schema_version: CATALOG_HISTORY_SCHEMA_VERSION,
            revision,
            captured_at,
            manifests: catalog
                .manifests
                .iter()
                .map(|manifest| manifest.entry.clone())
                .collect(),
            models: catalog
                .models
                .iter()
                .map(|model| model.entry.clone())
                .collect(),
        };
        validate_snapshot(&snapshot)?;
        Ok(snapshot)
    }

    pub fn sha256(&self) -> Result<String, CatalogError> {
        Ok(sha256_hex(&canonical_json(self)?))
    }

    /// Canonical JSON bytes used by [`Self::sha256`].
    pub fn canonical_json(&self) -> Result<Vec<u8>, CatalogError> {
        canonical_json(self)
    }

    pub fn diff(&self, newer: &Self) -> Result<CatalogDiffDto, CatalogError> {
        diff_snapshots(self, newer)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogChangeKind {
    Added,
    Updated,
    Removed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestChangedSection {
    DisplayName,
    ManifestVersion,
    ConnectionFields,
    ApiFamily,
    Sources,
    Origin,
    Authentication,
    Endpoints,
    Decoders,
    Parameters,
    Freshness,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelChangedSection {
    Match,
    ApiFamily,
    MetadataVersion,
    Capabilities,
    Parameters,
    Lifecycle,
    Sources,
    Freshness,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestDiffDto {
    pub provider_template_id: ProviderTemplateId,
    pub change: CatalogChangeKind,
    pub previous_manifest_version: Option<u32>,
    pub next_manifest_version: Option<u32>,
    pub previous_sha256: Option<String>,
    pub next_sha256: Option<String>,
    pub changed_sections: Vec<ManifestChangedSection>,
    /// Secret-free authority surface shown before activation. This additive
    /// optional field keeps persisted schema-v1 diffs readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security_review: Option<ManifestSecurityReviewDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestSecurityReviewDto {
    pub before: Option<ManifestSecuritySurfaceDto>,
    pub after: Option<ManifestSecuritySurfaceDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestSecuritySurfaceDto {
    pub origin: Option<CanonicalOrigin>,
    pub authentication: AuthBinding,
    pub endpoints: ManifestEndpoints,
    pub decoders: ManifestDecoders,
    pub parameter_mappings: Vec<ManifestParameterMappingDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestParameterMappingDto {
    pub parameter_id: String,
    pub mapping: ProviderParameterMapping,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelMetadataDiffDto {
    pub model_entry_id: String,
    pub provider_template_id: ProviderTemplateId,
    pub change: CatalogChangeKind,
    pub previous_metadata_version: Option<u32>,
    pub next_metadata_version: Option<u32>,
    pub previous_sha256: Option<String>,
    pub next_sha256: Option<String>,
    pub changed_sections: Vec<ModelChangedSection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogDiffDto {
    pub diff_schema_version: u32,
    pub from_revision: u64,
    pub to_revision: u64,
    pub manifest_changes: Vec<ManifestDiffDto>,
    pub model_changes: Vec<ModelMetadataDiffDto>,
}

fn diff_snapshots(
    previous: &CatalogRevisionSnapshot,
    next: &CatalogRevisionSnapshot,
) -> Result<CatalogDiffDto, CatalogError> {
    validate_snapshot(previous)?;
    validate_snapshot(next)?;
    let previous_manifests = previous
        .manifests
        .iter()
        .map(|entry| (entry.template.id.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let next_manifests = next
        .manifests
        .iter()
        .map(|entry| (entry.template.id.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let previous_models = previous
        .models
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let next_models = next
        .models
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect::<BTreeMap<_, _>>();

    Ok(CatalogDiffDto {
        diff_schema_version: CATALOG_DIFF_SCHEMA_VERSION,
        from_revision: previous.revision,
        to_revision: next.revision,
        manifest_changes: diff_manifests(&previous_manifests, &next_manifests)?,
        model_changes: diff_models(&previous_models, &next_models)?,
    })
}

fn diff_manifests(
    previous: &BTreeMap<&str, &CatalogManifestEntry>,
    next: &BTreeMap<&str, &CatalogManifestEntry>,
) -> Result<Vec<ManifestDiffDto>, CatalogError> {
    let keys = previous
        .keys()
        .chain(next.keys())
        .copied()
        .collect::<BTreeSet<_>>();
    let mut changes = Vec::new();
    for key in keys {
        match (previous.get(key), next.get(key)) {
            (None, Some(added)) => changes.push(manifest_diff(None, Some(added))?),
            (Some(removed), None) => changes.push(manifest_diff(Some(removed), None)?),
            (Some(previous), Some(next)) if *previous != *next => {
                changes.push(manifest_diff(Some(previous), Some(next))?);
            }
            _ => {}
        }
    }
    Ok(changes)
}

fn manifest_diff(
    previous: Option<&CatalogManifestEntry>,
    next: Option<&CatalogManifestEntry>,
) -> Result<ManifestDiffDto, CatalogError> {
    let identity = next
        .or(previous)
        .ok_or_else(|| CatalogError::new(CatalogErrorKind::HistoryConflict))?;
    let change = change_kind(previous.is_some(), next.is_some());
    let changed_sections = match (previous, next) {
        (Some(previous), Some(next)) => changed_manifest_sections(previous, next),
        _ => vec![
            ManifestChangedSection::DisplayName,
            ManifestChangedSection::ManifestVersion,
            ManifestChangedSection::ConnectionFields,
            ManifestChangedSection::ApiFamily,
            ManifestChangedSection::Sources,
            ManifestChangedSection::Origin,
            ManifestChangedSection::Authentication,
            ManifestChangedSection::Endpoints,
            ManifestChangedSection::Decoders,
            ManifestChangedSection::Parameters,
            ManifestChangedSection::Freshness,
        ],
    };
    Ok(ManifestDiffDto {
        provider_template_id: identity.template.id.clone(),
        change,
        previous_manifest_version: previous.map(|entry| entry.template.manifest_version),
        next_manifest_version: next.map(|entry| entry.template.manifest_version),
        previous_sha256: previous.map(canonical_hash).transpose()?,
        next_sha256: next.map(canonical_hash).transpose()?,
        security_review: security_review(previous, next, &changed_sections),
        changed_sections,
    })
}

fn security_review(
    previous: Option<&CatalogManifestEntry>,
    next: Option<&CatalogManifestEntry>,
    changed_sections: &[ManifestChangedSection],
) -> Option<ManifestSecurityReviewDto> {
    let security_authority_changed = changed_sections.iter().any(|section| {
        matches!(
            section,
            ManifestChangedSection::Origin
                | ManifestChangedSection::Authentication
                | ManifestChangedSection::Endpoints
                | ManifestChangedSection::Decoders
                | ManifestChangedSection::Parameters
        )
    });
    security_authority_changed.then(|| ManifestSecurityReviewDto {
        before: previous.map(manifest_security_surface),
        after: next.map(manifest_security_surface),
    })
}

fn manifest_security_surface(entry: &CatalogManifestEntry) -> ManifestSecuritySurfaceDto {
    let manifest = &entry.template.default_manifest;
    ManifestSecuritySurfaceDto {
        origin: manifest.default_api_origin.clone(),
        authentication: manifest.auth.clone(),
        endpoints: manifest.endpoints.clone(),
        decoders: manifest.decoders.clone(),
        parameter_mappings: manifest
            .parameters
            .iter()
            .map(|parameter| ManifestParameterMappingDto {
                parameter_id: parameter.id.as_str().to_owned(),
                mapping: parameter.provider_mapping.clone(),
            })
            .collect(),
    }
}

fn changed_manifest_sections(
    previous: &CatalogManifestEntry,
    next: &CatalogManifestEntry,
) -> Vec<ManifestChangedSection> {
    let mut changed = Vec::new();
    let previous_template = &previous.template;
    let next_template = &next.template;
    push_if_changed(
        &mut changed,
        previous_template.display_name != next_template.display_name,
        ManifestChangedSection::DisplayName,
    );
    push_if_changed(
        &mut changed,
        previous_template.manifest_version != next_template.manifest_version,
        ManifestChangedSection::ManifestVersion,
    );
    push_if_changed(
        &mut changed,
        previous_template.connection_fields != next_template.connection_fields,
        ManifestChangedSection::ConnectionFields,
    );
    push_if_changed(
        &mut changed,
        previous_template.api_family != next_template.api_family,
        ManifestChangedSection::ApiFamily,
    );
    let previous_manifest = &previous_template.default_manifest;
    let next_manifest = &next_template.default_manifest;
    push_if_changed(
        &mut changed,
        previous.sources != next.sources
            || previous_template.source != next_template.source
            || previous_manifest.sources != next_manifest.sources,
        ManifestChangedSection::Sources,
    );
    push_if_changed(
        &mut changed,
        previous_manifest.default_api_origin != next_manifest.default_api_origin,
        ManifestChangedSection::Origin,
    );
    push_if_changed(
        &mut changed,
        previous_manifest.auth != next_manifest.auth,
        ManifestChangedSection::Authentication,
    );
    push_if_changed(
        &mut changed,
        previous_manifest.endpoints != next_manifest.endpoints,
        ManifestChangedSection::Endpoints,
    );
    push_if_changed(
        &mut changed,
        previous_manifest.decoders != next_manifest.decoders,
        ManifestChangedSection::Decoders,
    );
    push_if_changed(
        &mut changed,
        previous_manifest.parameters != next_manifest.parameters,
        ManifestChangedSection::Parameters,
    );
    push_if_changed(
        &mut changed,
        previous.verified_at != next.verified_at || previous.expires_at != next.expires_at,
        ManifestChangedSection::Freshness,
    );
    changed
}

fn diff_models(
    previous: &BTreeMap<&str, &CatalogModelEntry>,
    next: &BTreeMap<&str, &CatalogModelEntry>,
) -> Result<Vec<ModelMetadataDiffDto>, CatalogError> {
    let keys = previous
        .keys()
        .chain(next.keys())
        .copied()
        .collect::<BTreeSet<_>>();
    let mut changes = Vec::new();
    for key in keys {
        match (previous.get(key), next.get(key)) {
            (None, Some(added)) => changes.push(model_diff(None, Some(added))?),
            (Some(removed), None) => changes.push(model_diff(Some(removed), None)?),
            (Some(previous), Some(next)) if *previous != *next => {
                changes.push(model_diff(Some(previous), Some(next))?);
            }
            _ => {}
        }
    }
    Ok(changes)
}

fn model_diff(
    previous: Option<&CatalogModelEntry>,
    next: Option<&CatalogModelEntry>,
) -> Result<ModelMetadataDiffDto, CatalogError> {
    let identity = next
        .or(previous)
        .ok_or_else(|| CatalogError::new(CatalogErrorKind::HistoryConflict))?;
    let changed_sections = match (previous, next) {
        (Some(previous), Some(next)) => changed_model_sections(previous, next),
        _ => vec![
            ModelChangedSection::Match,
            ModelChangedSection::ApiFamily,
            ModelChangedSection::MetadataVersion,
            ModelChangedSection::Capabilities,
            ModelChangedSection::Parameters,
            ModelChangedSection::Lifecycle,
            ModelChangedSection::Sources,
            ModelChangedSection::Freshness,
        ],
    };
    Ok(ModelMetadataDiffDto {
        model_entry_id: identity.id.clone(),
        provider_template_id: identity.provider_template_id.clone(),
        change: change_kind(previous.is_some(), next.is_some()),
        previous_metadata_version: previous.map(|entry| entry.metadata_version),
        next_metadata_version: next.map(|entry| entry.metadata_version),
        previous_sha256: previous.map(canonical_hash).transpose()?,
        next_sha256: next.map(canonical_hash).transpose()?,
        changed_sections,
    })
}

fn changed_model_sections(
    previous: &CatalogModelEntry,
    next: &CatalogModelEntry,
) -> Vec<ModelChangedSection> {
    let mut changed = Vec::new();
    push_if_changed(
        &mut changed,
        previous.model_match != next.model_match,
        ModelChangedSection::Match,
    );
    push_if_changed(
        &mut changed,
        previous.api_family != next.api_family,
        ModelChangedSection::ApiFamily,
    );
    push_if_changed(
        &mut changed,
        previous.metadata_version != next.metadata_version,
        ModelChangedSection::MetadataVersion,
    );
    push_if_changed(
        &mut changed,
        previous.capabilities != next.capabilities,
        ModelChangedSection::Capabilities,
    );
    push_if_changed(
        &mut changed,
        previous.parameters != next.parameters,
        ModelChangedSection::Parameters,
    );
    push_if_changed(
        &mut changed,
        previous.lifecycle != next.lifecycle,
        ModelChangedSection::Lifecycle,
    );
    push_if_changed(
        &mut changed,
        previous.sources != next.sources,
        ModelChangedSection::Sources,
    );
    push_if_changed(
        &mut changed,
        previous.verified_at != next.verified_at || previous.expires_at != next.expires_at,
        ModelChangedSection::Freshness,
    );
    changed
}

fn push_if_changed<T>(values: &mut Vec<T>, changed: bool, value: T) {
    if changed {
        values.push(value);
    }
}

fn change_kind(previous: bool, next: bool) -> CatalogChangeKind {
    match (previous, next) {
        (false, true) => CatalogChangeKind::Added,
        (true, false) => CatalogChangeKind::Removed,
        (true, true) => CatalogChangeKind::Updated,
        (false, false) => unreachable!("a diff must contain at least one side"),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogHistory {
    pub history_schema_version: u32,
    pub active_revision: u64,
    pub snapshots: Vec<CatalogRevisionSnapshot>,
}

impl CatalogHistory {
    pub fn new(initial: CatalogRevisionSnapshot) -> Result<Self, CatalogError> {
        validate_snapshot(&initial)?;
        Ok(Self {
            history_schema_version: CATALOG_HISTORY_SCHEMA_VERSION,
            active_revision: initial.revision,
            snapshots: vec![initial],
        })
    }

    pub fn active(&self) -> Result<&CatalogRevisionSnapshot, CatalogError> {
        self.validate()?;
        self.active_unchecked()
    }

    fn active_unchecked(&self) -> Result<&CatalogRevisionSnapshot, CatalogError> {
        self.snapshots
            .iter()
            .find(|snapshot| snapshot.revision == self.active_revision)
            .ok_or_else(|| CatalogError::new(CatalogErrorKind::HistoryConflict))
    }

    pub fn record(&mut self, snapshot: CatalogRevisionSnapshot) -> Result<(), CatalogError> {
        self.validate()?;
        validate_snapshot(&snapshot)?;
        if self.snapshots.len() >= MAX_HISTORY_SNAPSHOTS {
            return Err(CatalogError::new(CatalogErrorKind::HistoryFull));
        }
        let highest_revision = self
            .snapshots
            .iter()
            .map(|snapshot| snapshot.revision)
            .max()
            .unwrap_or(0);
        if snapshot.revision <= highest_revision {
            return Err(CatalogError::new(CatalogErrorKind::HistoryConflict));
        }
        self.active_revision = snapshot.revision;
        self.snapshots.push(snapshot);
        Ok(())
    }

    pub fn prepare_rollback(
        &self,
        target_revision: u64,
        now: DateTime<Utc>,
    ) -> Result<CatalogRollbackPlanDto, CatalogError> {
        self.validate()?;
        let current = self.active_unchecked()?;
        let target = self
            .snapshots
            .iter()
            .find(|snapshot| snapshot.revision == target_revision)
            .ok_or_else(|| CatalogError::new(CatalogErrorKind::RollbackTargetMissing))?;
        if current.revision == target.revision {
            return Err(CatalogError::new(CatalogErrorKind::HistoryConflict));
        }
        Ok(CatalogRollbackPlanDto {
            rollback_plan_version: CATALOG_ROLLBACK_PLAN_VERSION,
            from_revision: current.revision,
            to_revision: target.revision,
            expected_active_sha256: current.sha256()?,
            target_sha256: target.sha256()?,
            created_at: now,
            expires_at: now + Duration::minutes(ROLLBACK_PLAN_LIFETIME_MINUTES),
            diff: current.diff(target)?,
        })
    }

    pub fn apply_rollback(
        &mut self,
        plan: &CatalogRollbackPlanDto,
        now: DateTime<Utc>,
    ) -> Result<(), CatalogError> {
        self.validate()?;
        if plan.rollback_plan_version != CATALOG_ROLLBACK_PLAN_VERSION
            || plan.expires_at <= now
            || plan.expires_at
                != plan.created_at + Duration::minutes(ROLLBACK_PLAN_LIFETIME_MINUTES)
            || plan.created_at > now + Duration::minutes(CLOCK_SKEW_MINUTES)
            || self.active_revision != plan.from_revision
            || self.active_unchecked()?.sha256()? != plan.expected_active_sha256
        {
            return Err(CatalogError::new(CatalogErrorKind::RollbackPlanStale));
        }
        let target = self
            .snapshots
            .iter()
            .find(|snapshot| snapshot.revision == plan.to_revision)
            .ok_or_else(|| CatalogError::new(CatalogErrorKind::RollbackTargetMissing))?;
        if target.sha256()? != plan.target_sha256
            || self.active_unchecked()?.diff(target)? != plan.diff
        {
            return Err(CatalogError::new(CatalogErrorKind::RollbackPlanStale));
        }
        self.active_revision = target.revision;
        Ok(())
    }

    fn validate(&self) -> Result<(), CatalogError> {
        if self.history_schema_version != CATALOG_HISTORY_SCHEMA_VERSION
            || self.snapshots.is_empty()
            || self.snapshots.len() > MAX_HISTORY_SNAPSHOTS
        {
            return Err(CatalogError::new(CatalogErrorKind::HistoryConflict));
        }
        let mut revisions = BTreeSet::new();
        for snapshot in &self.snapshots {
            validate_snapshot(snapshot)?;
            if !revisions.insert(snapshot.revision) {
                return Err(CatalogError::new(CatalogErrorKind::HistoryConflict));
            }
        }
        if !revisions.contains(&self.active_revision) {
            return Err(CatalogError::new(CatalogErrorKind::HistoryConflict));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogRollbackPlanDto {
    pub rollback_plan_version: u32,
    pub from_revision: u64,
    pub to_revision: u64,
    pub expected_active_sha256: String,
    pub target_sha256: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub diff: CatalogDiffDto,
}

fn validate_snapshot(snapshot: &CatalogRevisionSnapshot) -> Result<(), CatalogError> {
    if snapshot.snapshot_schema_version != CATALOG_HISTORY_SCHEMA_VERSION
        || snapshot.revision == 0
        || snapshot.manifests.len() > MAX_CATALOG_MANIFESTS
        || snapshot.models.len() > MAX_CATALOG_MODELS
    {
        return Err(CatalogError::new(CatalogErrorKind::HistoryConflict));
    }
    let mut manifest_ids = BTreeSet::new();
    for entry in &snapshot.manifests {
        if !manifest_ids.insert(entry.template.id.as_str())
            || !is_catalog_identifier(entry.template.id.as_str(), MAX_MODEL_ENTRY_ID_BYTES)
            || entry.template.display_name.trim().is_empty()
            || entry.template.display_name.len() > MAX_DISPLAY_NAME_BYTES
            || entry.template.display_name.chars().any(char::is_control)
            || entry.template.manifest_version == 0
            || entry.template.api_family != entry.template.default_manifest.api_family
            || entry.verified_at > snapshot.captured_at + Duration::minutes(CLOCK_SKEW_MINUTES)
            || entry
                .expires_at
                .is_some_and(|expires_at| expires_at <= entry.verified_at)
        {
            return Err(CatalogError::new(CatalogErrorKind::HistoryConflict));
        }
        validate_connection_fields(&entry.template.connection_fields)
            .map_err(|_| CatalogError::new(CatalogErrorKind::HistoryConflict))?;
        validate_manifest(&entry.template.default_manifest)
            .map_err(|_| CatalogError::new(CatalogErrorKind::HistoryConflict))?;
        validate_entry_sources(&entry.sources, entry.verified_at)
            .map_err(|_| CatalogError::new(CatalogErrorKind::HistoryConflict))?;
    }
    let mut model_ids = BTreeSet::new();
    for entry in &snapshot.models {
        if !model_ids.insert(entry.id.as_str())
            || !is_catalog_identifier(&entry.id, MAX_MODEL_ENTRY_ID_BYTES)
            || !manifest_ids.contains(entry.provider_template_id.as_str())
            || entry.metadata_version == 0
            || entry.verified_at > snapshot.captured_at + Duration::minutes(CLOCK_SKEW_MINUTES)
            || entry
                .expires_at
                .is_some_and(|expires_at| expires_at <= entry.verified_at)
        {
            return Err(CatalogError::new(CatalogErrorKind::HistoryConflict));
        }
        validate_model_match(&entry.model_match)
            .map_err(|_| CatalogError::new(CatalogErrorKind::HistoryConflict))?;
        validate_capabilities(&entry.capabilities)
            .map_err(|_| CatalogError::new(CatalogErrorKind::HistoryConflict))?;
        validate_model_parameters(entry.api_family, &entry.parameters)
            .map_err(|_| CatalogError::new(CatalogErrorKind::HistoryConflict))?;
        validate_entry_sources(&entry.sources, entry.verified_at)
            .map_err(|_| CatalogError::new(CatalogErrorKind::HistoryConflict))?;
    }
    Ok(())
}

fn canonical_hash<T: Serialize>(value: &T) -> Result<String, CatalogError> {
    Ok(sha256_hex(&canonical_json(value)?))
}

fn canonical_json<T: Serialize>(value: &T) -> Result<Vec<u8>, CatalogError> {
    let value = serde_json::to_value(value)
        .map_err(|_| CatalogError::new(CatalogErrorKind::MalformedPayload))?;
    let mut output = Vec::new();
    write_canonical_json(&value, &mut output)?;
    Ok(output)
}

fn write_canonical_json(value: &Value, output: &mut Vec<u8>) -> Result<(), CatalogError> {
    match value {
        Value::Null => output.extend_from_slice(b"null"),
        Value::Bool(true) => output.extend_from_slice(b"true"),
        Value::Bool(false) => output.extend_from_slice(b"false"),
        Value::Number(number) => output.extend_from_slice(number.to_string().as_bytes()),
        Value::String(string) => serde_json::to_writer(output, string)
            .map_err(|_| CatalogError::new(CatalogErrorKind::MalformedPayload))?,
        Value::Array(values) => {
            output.push(b'[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                write_canonical_json(value, output)?;
            }
            output.push(b']');
        }
        Value::Object(object) => {
            output.push(b'{');
            let mut entries = object.iter().collect::<Vec<_>>();
            entries.sort_unstable_by_key(|(key, _)| *key);
            for (index, (key, value)) in entries.into_iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                serde_json::to_writer(&mut *output, key)
                    .map_err(|_| CatalogError::new(CatalogErrorKind::MalformedPayload))?;
                output.push(b':');
                write_canonical_json(value, output)?;
            }
            output.push(b'}');
        }
    }
    Ok(())
}

fn sha256_hex(value: &[u8]) -> String {
    let digest = digest::digest(&digest::SHA256, value);
    let mut encoded = String::with_capacity(64);
    for byte in digest.as_ref() {
        use fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use lorepia_domain::{
        ConnectionFieldSpec, ConnectionFieldType, CredentialRedirectPolicy, CredentialScope,
        ManifestSource, ManifestSourceKind,
    };
    use ring::signature::{Ed25519KeyPair, KeyPair as _};

    const TEST_KEY_ID: &str = "test-catalog-key";
    const TEST_PRIVATE_SEED: [u8; 32] = [
        0x11, 0x9f, 0x45, 0x09, 0xda, 0x8e, 0x31, 0xb7, 0x70, 0x65, 0x81, 0x43, 0x30, 0x2e, 0x12,
        0xb9, 0xa8, 0x04, 0xd2, 0xfd, 0xeb, 0x99, 0x1c, 0xea, 0x65, 0xe4, 0x99, 0x3a, 0x50, 0x7d,
        0x1b, 0x72,
    ];

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-07-31T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn source() -> CatalogSource {
        CatalogSource {
            url: HttpUrl::parse("https://docs.example.test/models").unwrap(),
            content_sha256: "a".repeat(64),
            verified_at: now() - Duration::days(1),
        }
    }

    fn manifest(template_source: TemplateSource, version: u32) -> CatalogManifestEntry {
        let family = ApiFamily::OpenAiChatCompletions;
        CatalogManifestEntry {
            template: ProviderTemplate {
                id: ProviderTemplateId::from("example-provider"),
                display_name: "Example".to_owned(),
                manifest_version: version,
                source: template_source,
                api_family: family,
                connection_fields: vec![ConnectionFieldSpec {
                    key: "credential".to_owned(),
                    label_key: "provider.credential".to_owned(),
                    description_key: None,
                    value_type: ConnectionFieldType::Credential,
                    required: true,
                }],
                default_manifest: ProviderManifest {
                    schema_version: 1,
                    api_family: family,
                    sources: vec![ManifestSource {
                        kind: ManifestSourceKind::SignedCatalog,
                        url: HttpUrl::parse("https://docs.example.test/api").unwrap(),
                        content_sha256: Some("b".repeat(64)),
                    }],
                    default_api_origin: None,
                    auth: AuthBinding::BearerHeader,
                    endpoints: ManifestEndpoints {
                        models: Some(EndpointSpec {
                            method: HttpMethod::Get,
                            path: EndpointPath::parse("/v1/models").unwrap(),
                        }),
                        generate: EndpointSpec {
                            method: HttpMethod::Post,
                            path: EndpointPath::parse("/v1/chat/completions").unwrap(),
                        },
                        embeddings: None,
                    },
                    decoders: ManifestDecoders {
                        response: DecoderId::OpenAiJsonV1,
                        streaming: Some(DecoderId::OpenAiSseV1),
                    },
                    parameters: Vec::new(),
                },
            },
            verified_at: now() - Duration::days(1),
            expires_at: None,
            sources: vec![source()],
        }
    }

    fn model(metadata_version: u32, reasoning: bool) -> CatalogModelEntry {
        CatalogModelEntry {
            id: "example-provider:model-1".to_owned(),
            provider_template_id: ProviderTemplateId::from("example-provider"),
            model_match: ModelMatch::Exact {
                model_id: "model-1".to_owned(),
            },
            api_family: ApiFamily::OpenAiChatCompletions,
            metadata_version,
            capabilities: vec![
                CatalogCapability {
                    key: CatalogCapabilityKey::Streaming,
                    value: CatalogCapabilityValue::Boolean(true),
                },
                CatalogCapability {
                    key: CatalogCapabilityKey::Reasoning,
                    value: CatalogCapabilityValue::Boolean(reasoning),
                },
            ],
            parameters: Vec::new(),
            lifecycle: ModelLifecycle::Active,
            sources: vec![source()],
            verified_at: now() - Duration::days(1),
            expires_at: None,
        }
    }

    fn payload(revision: u64) -> CatalogPayload {
        let metadata_version = u32::try_from(revision).unwrap();
        CatalogPayload {
            schema_version: CATALOG_SCHEMA_VERSION,
            catalog_id: CATALOG_ID.to_owned(),
            revision,
            issued_at: now() - Duration::hours(1),
            effective_at: now() - Duration::minutes(30),
            expires_at: now() + Duration::days(30),
            manifests: vec![manifest(TemplateSource::SignedCatalog, metadata_version)],
            models: vec![model(metadata_version, true)],
        }
    }

    fn test_key_pair() -> Ed25519KeyPair {
        Ed25519KeyPair::from_seed_unchecked(&TEST_PRIVATE_SEED).unwrap()
    }

    fn trust_store() -> CatalogTrustStore {
        let key_pair = test_key_pair();
        CatalogTrustStore::for_test(
            TEST_KEY_ID,
            key_pair.public_key().as_ref().try_into().unwrap(),
        )
    }

    fn signed_document(payload: &CatalogPayload) -> Vec<u8> {
        let key_pair = test_key_pair();
        let request = prepare_catalog_signing(payload, TEST_KEY_ID).unwrap();
        let signature = key_pair.sign(request.signature_frame());
        let envelope = request.into_envelope(signature.as_ref()).unwrap();
        serde_json::to_vec(&envelope).unwrap()
    }

    fn signed_document_without_preflight(payload: &CatalogPayload) -> Vec<u8> {
        let key_pair = test_key_pair();
        let payload_bytes = canonical_json(payload).unwrap();
        let frame = signature_frame(CATALOG_ENVELOPE_VERSION, TEST_KEY_ID, &payload_bytes).unwrap();
        let envelope = SignedCatalogEnvelope {
            envelope_version: CATALOG_ENVELOPE_VERSION,
            signing_key_id: TEST_KEY_ID.to_owned(),
            payload_base64: BASE64.encode(payload_bytes),
            signature_base64: BASE64.encode(key_pair.sign(&frame).as_ref()),
        };
        serde_json::to_vec(&envelope).unwrap()
    }

    fn signed_bundled_overlay() -> VerifiedCatalogUpdate {
        let baseline = bundled_catalog_baseline().unwrap();
        let bundled = baseline
            .layer()
            .models
            .iter()
            .find(|model| {
                model.provider_template_id.as_str() == BuiltInTemplateId::OpenAiResponses.as_str()
            })
            .unwrap();
        let overlay = CatalogModelEntry {
            id: bundled.id.clone(),
            provider_template_id: bundled.provider_template_id.clone(),
            model_match: bundled.model_match.clone(),
            api_family: bundled.api_family,
            metadata_version: bundled.metadata_version + 1,
            capabilities: vec![CatalogCapability {
                key: CatalogCapabilityKey::Reasoning,
                value: CatalogCapabilityValue::Boolean(true),
            }],
            parameters: Vec::new(),
            lifecycle: ModelLifecycle::Active,
            sources: vec![source()],
            verified_at: now() - Duration::hours(1),
            expires_at: None,
        };
        let signed_payload = CatalogPayload {
            schema_version: CATALOG_SCHEMA_VERSION,
            catalog_id: CATALOG_ID.to_owned(),
            revision: 20,
            issued_at: now() - Duration::minutes(30),
            effective_at: now() - Duration::minutes(20),
            expires_at: now() + Duration::days(30),
            manifests: Vec::new(),
            models: vec![overlay],
        };
        verify_catalog_import_with_trust(
            &signed_document(&signed_payload),
            &trust_store(),
            &CatalogRevisionGuard::default(),
            now(),
        )
        .unwrap()
    }

    #[test]
    fn bundled_baseline_is_deterministic_offline_and_covers_all_five_families() {
        let first = bundled_catalog_baseline().unwrap();
        let second = bundled_catalog_baseline().unwrap();

        assert_eq!(first, second);
        assert_eq!(first.canonical_sha256().len(), 64);
        assert_eq!(
            first.canonical_sha256(),
            first.layer().canonical_sha256().unwrap()
        );
        assert_eq!(first.layer().manifests.len(), BuiltInTemplateId::ALL.len());
        assert_eq!(first.layer().models.len(), BuiltInTemplateId::ALL.len());
        assert_eq!(first.layer().authority, CatalogAuthority::Bundled);
        assert!(first.layer().expires_at.is_none());

        for id in BuiltInTemplateId::ALL {
            let template = AdapterRegistry::built_in_template(id).unwrap();
            let manifest = first
                .layer()
                .manifests
                .iter()
                .find(|entry| entry.template.id == template.id)
                .unwrap();
            let model = first
                .layer()
                .models
                .iter()
                .find(|entry| entry.provider_template_id == template.id)
                .unwrap();

            assert_eq!(manifest.template, template);
            assert_eq!(model.api_family, id.family());
            assert_eq!(model.model_match, ModelMatch::AnyModel);
            assert!(model.model_match.matches("any-provider-model-id"));
            assert_eq!(model.lifecycle, ModelLifecycle::Unknown);
            assert_eq!(
                model.parameters,
                manifest.template.default_manifest.parameters
            );
            assert!(!model.parameters.is_empty());
            assert!(
                model
                    .capabilities
                    .iter()
                    .all(|capability| capability.value == CatalogCapabilityValue::Unknown)
            );
            assert!(
                model
                    .sources
                    .iter()
                    .chain(&manifest.sources)
                    .all(|source| source.url.as_str().starts_with(BUNDLED_SOURCE_ORIGIN))
            );
        }
    }

    #[test]
    fn bundled_provenance_remains_visible_when_stale() {
        let verified_at = DateTime::parse_from_rfc3339(BUNDLED_CATALOG_VERIFIED_AT)
            .unwrap()
            .with_timezone(&Utc);
        let stale_now = verified_at + Duration::days(181);
        let merged =
            merge_with_bundled_catalog(&[], &[], stale_now, &CatalogFreshnessPolicy::default())
                .unwrap();

        assert_eq!(merged.manifests.len(), BuiltInTemplateId::ALL.len());
        assert_eq!(merged.models.len(), BuiltInTemplateId::ALL.len());
        for manifest in &merged.manifests {
            assert_eq!(manifest.provenance.authority, CatalogAuthority::Bundled);
            assert_eq!(manifest.provenance.freshness, CatalogFreshness::Stale);
        }
        for model in &merged.models {
            assert_eq!(model.metadata_provenance.freshness, CatalogFreshness::Stale);
            assert!(
                model
                    .capability_provenance
                    .values()
                    .all(|provenance| provenance.freshness == CatalogFreshness::Stale)
            );
            assert!(
                model
                    .parameter_provenance
                    .values()
                    .all(|provenance| provenance.freshness == CatalogFreshness::Stale)
            );
        }
    }

    #[test]
    fn verified_signed_overlay_replaces_only_supported_generic_facts() {
        let verified = signed_bundled_overlay();
        let overlay_id = verified.payload().models[0].id.clone();
        let merged =
            merge_with_bundled_catalog(&[verified], &[], now(), &CatalogFreshnessPolicy::default())
                .unwrap();
        let model = merged
            .models
            .iter()
            .find(|model| model.entry.id == overlay_id)
            .unwrap();

        assert_eq!(
            model
                .entry
                .capabilities
                .iter()
                .find(|capability| capability.key == CatalogCapabilityKey::Reasoning)
                .unwrap()
                .value,
            CatalogCapabilityValue::Boolean(true)
        );
        assert_eq!(
            model.capability_provenance[&CatalogCapabilityKey::Reasoning].authority,
            CatalogAuthority::SignedCatalog
        );
        assert_eq!(
            model.capability_provenance[&CatalogCapabilityKey::ContextTokens].authority,
            CatalogAuthority::Bundled
        );
        assert!(
            model
                .parameter_provenance
                .values()
                .all(|provenance| provenance.authority == CatalogAuthority::Bundled)
        );

        let forged_runtime_layer = CatalogLayer::from_verified_update(signed_bundled_overlay());
        assert_eq!(
            merge_with_bundled_catalog(
                &[],
                &[forged_runtime_layer],
                now(),
                &CatalogFreshnessPolicy::default()
            )
            .unwrap_err()
            .kind(),
            CatalogErrorKind::InvalidLayer
        );
    }

    #[test]
    fn bundled_and_signed_snapshots_use_existing_diff_and_rollback_engine() {
        let baseline =
            merge_with_bundled_catalog(&[], &[], now(), &CatalogFreshnessPolicy::default())
                .unwrap();
        let with_signed = merge_with_bundled_catalog(
            &[signed_bundled_overlay()],
            &[],
            now(),
            &CatalogFreshnessPolicy::default(),
        )
        .unwrap();
        let first = CatalogRevisionSnapshot::from_merged(1, now(), &baseline).unwrap();
        let second = CatalogRevisionSnapshot::from_merged(2, now(), &with_signed).unwrap();
        let first_hash = first.sha256().unwrap();

        let diff = first.diff(&second).unwrap();
        assert_eq!(diff.model_changes.len(), 1);
        assert!(
            diff.model_changes[0]
                .changed_sections
                .contains(&ModelChangedSection::Capabilities)
        );

        let mut history = CatalogHistory::new(first).unwrap();
        history.record(second).unwrap();
        let plan = history.prepare_rollback(1, now()).unwrap();
        history.apply_rollback(&plan, now()).unwrap();
        assert_eq!(history.active().unwrap().sha256().unwrap(), first_hash);
    }

    #[test]
    fn verifies_exact_framed_signature_and_returns_next_guard() {
        let document = signed_document(&payload(7));
        let verified = verify_catalog_import_with_trust(
            &document,
            &trust_store(),
            &CatalogRevisionGuard {
                highest_accepted_revision: 6,
                latest_issued_at: None,
            },
            now(),
        )
        .unwrap();

        assert_eq!(verified.payload.revision, 7);
        assert_eq!(verified.next_revision_guard.highest_accepted_revision, 7);
        assert_eq!(verified.payload_sha256.len(), 64);
    }

    #[test]
    fn rejects_revision_values_that_consume_the_reserved_epoch_namespace() {
        let mut exhausting = payload(7);
        exhausting.revision = i64::MAX as u64;
        let document = signed_document_without_preflight(&exhausting);

        assert_eq!(
            verify_catalog_import_with_trust(
                &document,
                &trust_store(),
                &CatalogRevisionGuard {
                    highest_accepted_revision: 6,
                    latest_issued_at: None,
                },
                now(),
            )
            .unwrap_err()
            .kind(),
            CatalogErrorKind::InvalidRevision
        );

        let mut jumping = payload(7);
        jumping.revision = 6 + CATALOG_REVISION_MAX_ADVANCE + 1;
        let document = signed_document_without_preflight(&jumping);
        assert_eq!(
            verify_catalog_import_with_trust(
                &document,
                &trust_store(),
                &CatalogRevisionGuard {
                    highest_accepted_revision: 6,
                    latest_issued_at: None,
                },
                now(),
            )
            .unwrap_err()
            .kind(),
            CatalogErrorKind::InvalidRevision
        );
    }

    #[test]
    fn stored_replay_preserves_pre_bound_high_revision_catalogs() {
        let mut legacy = payload(7);
        legacy.revision = i64::MAX as u64;
        let document = signed_document_without_preflight(&legacy);

        assert_eq!(
            verify_catalog_import_with_trust(
                &document,
                &trust_store(),
                &CatalogRevisionGuard::default(),
                now(),
            )
            .expect_err("the same high revision must remain blocked for a new import")
            .kind(),
            CatalogErrorKind::InvalidRevision
        );

        let verified = verify_stored_catalog_update_with_trust(
            &document,
            &trust_store(),
            &CatalogRevisionGuard::default(),
            now(),
        )
        .expect("catalog accepted before revision bounds must remain replayable");

        assert_eq!(verified.payload().revision, i64::MAX as u64);
        assert_eq!(
            verified.next_revision_guard().highest_accepted_revision,
            i64::MAX as u64
        );
    }

    #[test]
    fn rejects_tampering_key_substitution_and_noncanonical_payload() {
        let valid = signed_document(&payload(2));
        let mut envelope: SignedCatalogEnvelope = serde_json::from_slice(&valid).unwrap();
        let mut payload_bytes = BASE64.decode(&envelope.payload_base64).unwrap();
        let byte = payload_bytes.last_mut().unwrap();
        *byte ^= 1;
        envelope.payload_base64 = BASE64.encode(payload_bytes);
        let tampered = serde_json::to_vec(&envelope).unwrap();
        assert_eq!(
            verify_catalog_import_with_trust(
                &tampered,
                &trust_store(),
                &CatalogRevisionGuard::default(),
                now()
            )
            .unwrap_err()
            .kind(),
            CatalogErrorKind::InvalidSignature
        );

        let mut envelope: SignedCatalogEnvelope = serde_json::from_slice(&valid).unwrap();
        envelope.signing_key_id = "other-key".to_owned();
        let substituted = serde_json::to_vec(&envelope).unwrap();
        assert_eq!(
            verify_catalog_import_with_trust(
                &substituted,
                &trust_store(),
                &CatalogRevisionGuard::default(),
                now()
            )
            .unwrap_err()
            .kind(),
            CatalogErrorKind::UnknownSigningKey
        );

        let key_pair = test_key_pair();
        let pretty_payload = serde_json::to_vec_pretty(&payload(2)).unwrap();
        let frame =
            signature_frame(CATALOG_ENVELOPE_VERSION, TEST_KEY_ID, &pretty_payload).unwrap();
        let noncanonical = SignedCatalogEnvelope {
            envelope_version: CATALOG_ENVELOPE_VERSION,
            signing_key_id: TEST_KEY_ID.to_owned(),
            payload_base64: BASE64.encode(&pretty_payload),
            signature_base64: BASE64.encode(key_pair.sign(&frame).as_ref()),
        };
        assert_eq!(
            verify_catalog_import_with_trust(
                &serde_json::to_vec(&noncanonical).unwrap(),
                &trust_store(),
                &CatalogRevisionGuard::default(),
                now()
            )
            .unwrap_err()
            .kind(),
            CatalogErrorKind::NonCanonicalPayload
        );
    }

    #[test]
    fn rejects_expired_future_and_rollback_updates() {
        let guard = CatalogRevisionGuard {
            highest_accepted_revision: 5,
            latest_issued_at: None,
        };
        assert_eq!(
            verify_catalog_import_with_trust(
                &signed_document(&payload(5)),
                &trust_store(),
                &guard,
                now()
            )
            .unwrap_err()
            .kind(),
            CatalogErrorKind::RevisionRollback
        );

        let mut expired = payload(6);
        expired.issued_at = now() - Duration::days(2);
        expired.effective_at = now() - Duration::days(2);
        expired.expires_at = now() - Duration::days(1);
        expired.manifests[0].verified_at = now() - Duration::days(3);
        expired.manifests[0].sources[0].verified_at = now() - Duration::days(3);
        expired.models[0].verified_at = now() - Duration::days(3);
        expired.models[0].sources[0].verified_at = now() - Duration::days(3);
        assert_eq!(
            verify_catalog_import_with_trust(
                &signed_document(&expired),
                &trust_store(),
                &guard,
                now()
            )
            .unwrap_err()
            .kind(),
            CatalogErrorKind::Expired
        );

        let mut future = payload(6);
        future.issued_at = now();
        future.effective_at = now() + Duration::hours(1);
        assert_eq!(
            verify_catalog_import_with_trust(
                &signed_document(&future),
                &trust_store(),
                &guard,
                now()
            )
            .unwrap_err()
            .kind(),
            CatalogErrorKind::NotYetEffective
        );
    }

    #[test]
    fn safe_glob_is_bounded_and_does_not_use_regex() {
        let matcher = ModelMatch::Glob {
            pattern: "model-*-preview".to_owned(),
        };
        validate_model_match(&matcher).unwrap();
        assert!(matcher.matches("model-42-preview"));
        assert!(!matcher.matches("model-42"));

        assert_eq!(
            validate_model_match(&ModelMatch::Glob {
                pattern: "**".to_owned()
            })
            .unwrap_err()
            .kind(),
            CatalogErrorKind::InvalidModelMatch
        );
    }

    #[test]
    fn merge_uses_precedence_preserves_lower_fields_and_marks_stale() {
        let mut bundled_model = model(1, false);
        bundled_model.verified_at = now() - Duration::days(200);
        bundled_model.sources[0].verified_at = now() - Duration::days(200);
        let bundled = CatalogLayer {
            id: "bundled-1".to_owned(),
            authority: CatalogAuthority::Bundled,
            revision: 1,
            effective_at: now() - Duration::days(200),
            expires_at: None,
            manifests: Vec::new(),
            models: vec![bundled_model],
        };
        let mut api_model = model(2, true);
        api_model.capabilities = vec![CatalogCapability {
            key: CatalogCapabilityKey::Reasoning,
            value: CatalogCapabilityValue::Boolean(true),
        }];
        api_model.verified_at = now() - Duration::days(1);
        let provider_api = CatalogLayer {
            id: "provider-api-2".to_owned(),
            authority: CatalogAuthority::ProviderApi,
            revision: 2,
            effective_at: now() - Duration::days(1),
            expires_at: None,
            manifests: Vec::new(),
            models: vec![api_model],
        };

        let merged = merge_catalog_layers(
            &[bundled, provider_api],
            now(),
            &CatalogFreshnessPolicy::default(),
        )
        .unwrap();
        let model = &merged.models[0];
        assert_eq!(model.entry.capabilities.len(), 2);
        assert_eq!(
            model.metadata_provenance.authority,
            CatalogAuthority::ProviderApi
        );
        assert_eq!(
            model.capability_provenance[&CatalogCapabilityKey::Streaming].freshness,
            CatalogFreshness::Stale
        );
        assert_eq!(
            model.capability_provenance[&CatalogCapabilityKey::Reasoning].freshness,
            CatalogFreshness::Current
        );
    }

    #[test]
    fn merge_rejects_stable_identity_drift_and_same_authority_version_rollback() {
        let base = CatalogLayer {
            id: "provider-api-2".to_owned(),
            authority: CatalogAuthority::ProviderApi,
            revision: 2,
            effective_at: now() - Duration::hours(2),
            expires_at: None,
            manifests: Vec::new(),
            models: vec![model(2, true)],
        };
        let mut rolled_back = model(1, false);
        rolled_back.verified_at = now() - Duration::hours(1);
        rolled_back.sources[0].verified_at = now() - Duration::hours(1);
        let rollback_layer = CatalogLayer {
            id: "provider-api-3".to_owned(),
            authority: CatalogAuthority::ProviderApi,
            revision: 3,
            effective_at: now() - Duration::hours(1),
            expires_at: None,
            manifests: Vec::new(),
            models: vec![rolled_back],
        };
        assert_eq!(
            merge_catalog_layers(
                &[base.clone(), rollback_layer],
                now(),
                &CatalogFreshnessPolicy::default()
            )
            .unwrap_err()
            .kind(),
            CatalogErrorKind::InvalidLayer
        );

        let mut drifted = model(3, true);
        drifted.model_match = ModelMatch::Exact {
            model_id: "different-model".to_owned(),
        };
        drifted.verified_at = now() - Duration::minutes(30);
        drifted.sources[0].verified_at = now() - Duration::minutes(30);
        let override_layer = CatalogLayer {
            id: "user-override-3".to_owned(),
            authority: CatalogAuthority::UserOverride,
            revision: 3,
            effective_at: now() - Duration::minutes(30),
            expires_at: None,
            manifests: Vec::new(),
            models: vec![drifted],
        };
        assert_eq!(
            merge_catalog_layers(
                &[base, override_layer],
                now(),
                &CatalogFreshnessPolicy::default()
            )
            .unwrap_err()
            .kind(),
            CatalogErrorKind::InvalidLayer
        );
    }

    #[test]
    fn history_diff_and_reviewed_rollback_preserve_anti_rollback_guard() {
        let first = CatalogRevisionSnapshot {
            snapshot_schema_version: CATALOG_HISTORY_SCHEMA_VERSION,
            revision: 1,
            captured_at: now() - Duration::days(1),
            manifests: vec![manifest(TemplateSource::SignedCatalog, 1)],
            models: vec![model(1, false)],
        };
        let mut second = first.clone();
        second.revision = 2;
        second.captured_at = now();
        second.models[0] = model(2, true);

        let diff = first.diff(&second).unwrap();
        assert_eq!(diff.model_changes.len(), 1);
        assert_eq!(diff.model_changes[0].change, CatalogChangeKind::Updated);
        assert!(
            diff.model_changes[0]
                .changed_sections
                .contains(&ModelChangedSection::Capabilities)
        );

        let mut history = CatalogHistory::new(first).unwrap();
        history.record(second).unwrap();
        let plan = history.prepare_rollback(1, now()).unwrap();
        let mut tampered_plan = plan.clone();
        tampered_plan.diff.model_changes.clear();
        assert_eq!(
            history
                .apply_rollback(&tampered_plan, now())
                .unwrap_err()
                .kind(),
            CatalogErrorKind::RollbackPlanStale
        );
        history.apply_rollback(&plan, now()).unwrap();
        assert_eq!(history.active_revision, 1);
        assert_eq!(
            history.apply_rollback(&plan, now()).unwrap_err().kind(),
            CatalogErrorKind::RollbackPlanStale
        );

        let revision_guard = CatalogRevisionGuard {
            highest_accepted_revision: 2,
            latest_issued_at: Some(now()),
        };
        assert_eq!(revision_guard.highest_accepted_revision, 2);
    }

    #[test]
    fn manifest_diff_exposes_bounded_security_authority_before_and_after_values() {
        let mut before = manifest(TemplateSource::SignedCatalog, 1);
        let mut after = before.clone();
        before.template.default_manifest.default_api_origin =
            Some(lorepia_domain::CanonicalOrigin::parse("https://old.example.test").unwrap());
        after.template.default_manifest.default_api_origin =
            Some(lorepia_domain::CanonicalOrigin::parse("https://new.example.test").unwrap());
        after.template.default_manifest.auth = AuthBinding::HeaderApiKey {
            header_name: lorepia_domain::HeaderName::parse("x-provider-key").unwrap(),
        };
        after.template.default_manifest.endpoints.generate.path =
            EndpointPath::parse("/v2/chat/completions").unwrap();
        after.template.default_manifest.decoders.streaming = None;
        let mut parameter =
            AdapterRegistry::built_in_template(BuiltInTemplateId::OpenAiChatCompatible)
                .unwrap()
                .default_manifest
                .parameters
                .into_iter()
                .next()
                .unwrap();
        before.template.default_manifest.parameters = vec![parameter.clone()];
        parameter.provider_mapping.field_name = "renamed_parameter".to_owned();
        after.template.default_manifest.parameters = vec![parameter];

        let previous = CatalogRevisionSnapshot {
            snapshot_schema_version: CATALOG_HISTORY_SCHEMA_VERSION,
            revision: 1,
            captured_at: now() - Duration::minutes(1),
            manifests: vec![before],
            models: Vec::new(),
        };
        let next = CatalogRevisionSnapshot {
            snapshot_schema_version: CATALOG_HISTORY_SCHEMA_VERSION,
            revision: 2,
            captured_at: now(),
            manifests: vec![after],
            models: Vec::new(),
        };

        let value = serde_json::to_value(previous.diff(&next).unwrap()).unwrap();
        let review = &value["manifest_changes"][0]["security_review"];
        assert_eq!(
            review["before"]["origin"],
            Value::String("https://old.example.test".to_owned())
        );
        assert_eq!(
            review["after"]["origin"],
            Value::String("https://new.example.test".to_owned())
        );
        assert_eq!(
            review["after"]["authentication"]["kind"],
            Value::String("header_api_key".to_owned())
        );
        assert_eq!(
            review["after"]["endpoints"]["generate"]["path"],
            Value::String("/v2/chat/completions".to_owned())
        );
        assert_eq!(review["after"]["decoders"]["streaming"], Value::Null);
        assert_eq!(
            review["after"]["parameter_mappings"][0]["mapping"]["field_name"],
            Value::String("renamed_parameter".to_owned())
        );
        assert!(review.get("sources").is_none());
    }

    #[test]
    fn history_rejects_corrupt_cross_template_model_reference() {
        let mut snapshot = CatalogRevisionSnapshot {
            snapshot_schema_version: CATALOG_HISTORY_SCHEMA_VERSION,
            revision: 1,
            captured_at: now(),
            manifests: vec![manifest(TemplateSource::SignedCatalog, 1)],
            models: vec![model(1, true)],
        };
        snapshot.models[0].provider_template_id = ProviderTemplateId::from("missing-template");

        assert_eq!(
            CatalogHistory::new(snapshot).unwrap_err().kind(),
            CatalogErrorKind::HistoryConflict
        );
    }

    #[test]
    fn test_private_material_is_not_part_of_serialized_trust_or_updates() {
        let seed_canary = BASE64.encode(TEST_PRIVATE_SEED);
        let document = signed_document(&payload(3));
        let verified = verify_catalog_import_with_trust(
            &document,
            &trust_store(),
            &CatalogRevisionGuard::default(),
            now(),
        )
        .unwrap();

        assert!(!String::from_utf8(document).unwrap().contains(&seed_canary));
        assert!(!format!("{verified:?}").contains(&seed_canary));
        assert!(
            !serde_json::to_string(&verified)
                .unwrap()
                .contains(&seed_canary)
        );
    }

    #[test]
    fn credential_scope_types_remain_outside_catalog_payload() {
        let scope = CredentialScope {
            allowed_origins: vec![
                lorepia_domain::CanonicalOrigin::parse("https://api.example.test").unwrap(),
            ],
            auth_binding: AuthBinding::BearerHeader,
            redirect_policy: CredentialRedirectPolicy::Deny,
        };
        let serialized = serde_json::to_string(&payload(4)).unwrap();

        assert!(!serialized.contains(scope.allowed_origins[0].as_str()));
    }
}
