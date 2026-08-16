//! Durable, non-secret provider discovery contracts.
//!
//! Discovery can cross process lifetimes and includes network reads, billable
//! requests, user approvals, and a multi-store commit. This module keeps the
//! transition rules pure so storage can apply a transition, its idempotency
//! receipt, audit entry, and outbox event in one `SQLite` transaction.

use std::{collections::BTreeSet, fmt, net::IpAddr};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use sha2::{Digest, Sha256};
use thiserror::Error;
use url::Url;
use uuid::Uuid;

use crate::{
    AuthBinding, CanonicalOrigin, ConnectionConfigEntry, ConnectionConfigValue, CredentialRef,
    DiscoverySessionId, EndpointPath, EvidenceId, GenerationPresetId, HttpUrl, ModelRouteId,
    ProviderConnectionId, ProviderLocalNetworkApproval, ProviderNetworkMode, ProviderTemplateId,
};

pub const PROVIDER_DISCOVERY_EVENT_VERSION: u32 = 2;
pub const DEFAULT_PROVIDER_DISCOVERY_TIMEOUT_SECONDS: u32 = 60;
pub const MAX_PROVIDER_DISCOVERY_CONNECTION_VALUES: usize = 256;
pub const MAX_PROVIDER_DISCOVERY_CONNECTION_TEXT_CHARS: usize = 8 * 1024;

macro_rules! discovery_id {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4().to_string())
            }

            pub fn parse(value: impl Into<String>) -> Result<Self, DiscoveryContractError> {
                let value = value.into();
                validate_opaque_id(stringify!($name), &value)?;
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::parse(value).map_err(D::Error::custom)
            }
        }
    };
}

discovery_id!(DiscoveryActionId);
discovery_id!(DiscoveryApprovalId);
discovery_id!(DiscoveryCandidateId);
discovery_id!(DiscoveryCommitAttemptId);
discovery_id!(DiscoveryEventId);
discovery_id!(DiscoveryOperationId);

fn validate_opaque_id(label: &str, value: &str) -> Result<(), DiscoveryContractError> {
    if value.is_empty()
        || value.len() > 128
        || value.chars().any(char::is_control)
        || value.trim() != value
    {
        return Err(DiscoveryContractError::InvalidField {
            field: label.to_owned(),
            reason: "must be 1..=128 characters without surrounding whitespace or controls"
                .to_owned(),
        });
    }
    Ok(())
}

fn validate_sha256(field: &str, value: &str) -> Result<(), DiscoveryContractError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(DiscoveryContractError::InvalidField {
            field: field.to_owned(),
            reason: "must be a lowercase hexadecimal SHA-256 digest".to_owned(),
        });
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DiscoveryContractError {
    #[error("{field}: {reason}")]
    InvalidField { field: String, reason: String },
}

/// Secret-free connection settings retained throughout provider discovery.
///
/// Credential material has no representation in this contract. Provider
/// specific values remain typed, while network authority is an explicit mode
/// plus an exact-origin, finite-address grant for approved LAN access.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderDiscoveryConnectionOptions {
    #[serde(default)]
    pub values: Vec<ConnectionConfigEntry>,
    #[serde(default)]
    pub api_base_path: Option<EndpointPath>,
    #[serde(default = "default_provider_discovery_timeout_seconds")]
    pub timeout_seconds: u32,
    #[serde(default)]
    pub network_mode: ProviderNetworkMode,
    #[serde(default)]
    pub local_network_approval: Option<ProviderLocalNetworkApproval>,
}

impl Default for ProviderDiscoveryConnectionOptions {
    fn default() -> Self {
        Self {
            values: Vec::new(),
            api_base_path: None,
            timeout_seconds: DEFAULT_PROVIDER_DISCOVERY_TIMEOUT_SECONDS,
            network_mode: ProviderNetworkMode::Public,
            local_network_approval: None,
        }
    }
}

impl ProviderDiscoveryConnectionOptions {
    pub fn validate(&self) -> Result<(), DiscoveryContractError> {
        if !(1..=600).contains(&self.timeout_seconds) {
            return Err(DiscoveryContractError::InvalidField {
                field: "connection_options.timeout_seconds".to_owned(),
                reason: "must be from 1 to 600 seconds".to_owned(),
            });
        }
        if self.values.len() > MAX_PROVIDER_DISCOVERY_CONNECTION_VALUES {
            return Err(DiscoveryContractError::InvalidField {
                field: "connection_options.values".to_owned(),
                reason: format!(
                    "exceeds the {MAX_PROVIDER_DISCOVERY_CONNECTION_VALUES}-entry limit"
                ),
            });
        }

        let mut keys = BTreeSet::new();
        for entry in &self.values {
            validate_connection_option_entry(entry)?;
            if !keys.insert(entry.key.as_str()) {
                return Err(DiscoveryContractError::InvalidField {
                    field: "connection_options.values".to_owned(),
                    reason: format!("contains duplicate key {:?}", entry.key),
                });
            }
        }

        match (self.network_mode, &self.local_network_approval) {
            (ProviderNetworkMode::ApprovedLocalNetwork, Some(approval)) => {
                validate_local_network_approval(approval)?;
            }
            (ProviderNetworkMode::ApprovedLocalNetwork, None) => {
                return Err(DiscoveryContractError::InvalidField {
                    field: "connection_options.local_network_approval".to_owned(),
                    reason: "is required for approved_local_network mode".to_owned(),
                });
            }
            (ProviderNetworkMode::Public | ProviderNetworkMode::LocalLoopback, Some(_)) => {
                return Err(DiscoveryContractError::InvalidField {
                    field: "connection_options.local_network_approval".to_owned(),
                    reason: "must be absent unless network_mode is approved_local_network"
                        .to_owned(),
                });
            }
            (ProviderNetworkMode::Public | ProviderNetworkMode::LocalLoopback, None) => {}
        }

        Ok(())
    }
}

const fn default_provider_discovery_timeout_seconds() -> u32 {
    DEFAULT_PROVIDER_DISCOVERY_TIMEOUT_SECONDS
}

/// Input that is safe to persist.
///
/// Pasted cURL, request headers, credential material, cookies, and document
/// bodies intentionally have no representation here. `credential_ref` is an
/// opaque logical slot owned by the native credential broker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SanitizedDiscoveryInput {
    /// User-selected durable identity for the connection created by this flow.
    pub connection_id: ProviderConnectionId,
    /// User-visible connection label; never inferred from provider metadata.
    pub display_name: String,
    pub site_url: HttpUrl,
    pub docs_url: Option<HttpUrl>,
    pub credential_ref: Option<CredentialRef>,
    pub preferred_assistant: Option<ModelRouteId>,
    #[serde(default)]
    pub connection_options: ProviderDiscoveryConnectionOptions,
    #[serde(default)]
    pub supplied_evidence_ids: Vec<EvidenceId>,
}

impl SanitizedDiscoveryInput {
    pub fn validate(&self) -> Result<(), DiscoveryContractError> {
        validate_bounded_text(
            "connection_id",
            self.connection_id.as_str(),
            128,
            "must be a bounded trimmed opaque connection identifier",
        )?;
        validate_bounded_text(
            "display_name",
            &self.display_name,
            120,
            "must be a nonempty trimmed display name of at most 120 characters",
        )?;
        validate_redacted_url("site_url", &self.site_url)?;
        if let Some(docs_url) = &self.docs_url {
            validate_redacted_url("docs_url", docs_url)?;
        }
        if let Some(credential_ref) = &self.credential_ref {
            validate_opaque_reference("credential_ref", credential_ref.as_str())?;
            if credential_ref.as_str() != self.connection_id.as_str() {
                return Err(DiscoveryContractError::InvalidField {
                    field: "credential_ref".to_owned(),
                    reason: "must use the selected connection identifier as its native slot"
                        .to_owned(),
                });
            }
        }
        self.connection_options.validate()?;
        if self.supplied_evidence_ids.len() > 4_096 {
            return Err(DiscoveryContractError::InvalidField {
                field: "supplied_evidence_ids".to_owned(),
                reason: "exceeds the 4096 evidence-reference limit".to_owned(),
            });
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SanitizedDiscoveryInputWire {
    connection_id: ProviderConnectionId,
    display_name: String,
    site_url: HttpUrl,
    docs_url: Option<HttpUrl>,
    credential_ref: Option<CredentialRef>,
    preferred_assistant: Option<ModelRouteId>,
    #[serde(default)]
    connection_options: Option<ProviderDiscoveryConnectionOptions>,
    /// Read-only compatibility field for discovery records written before
    /// `ProviderNetworkMode` became the single network authority.
    #[serde(default)]
    local_network_mode: Option<bool>,
    #[serde(default)]
    supplied_evidence_ids: Vec<EvidenceId>,
}

impl<'de> Deserialize<'de> for SanitizedDiscoveryInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = SanitizedDiscoveryInputWire::deserialize(deserializer)?;
        let connection_options =
            migrate_legacy_network_mode(wire.connection_options, wire.local_network_mode)
                .map_err(D::Error::custom)?;
        let input = Self {
            connection_id: wire.connection_id,
            display_name: wire.display_name,
            site_url: wire.site_url,
            docs_url: wire.docs_url,
            credential_ref: wire.credential_ref,
            preferred_assistant: wire.preferred_assistant,
            connection_options,
            supplied_evidence_ids: wire.supplied_evidence_ids,
        };
        input.validate().map_err(D::Error::custom)?;
        Ok(input)
    }
}

fn migrate_legacy_network_mode(
    connection_options: Option<ProviderDiscoveryConnectionOptions>,
    legacy_local_network_mode: Option<bool>,
) -> Result<ProviderDiscoveryConnectionOptions, DiscoveryContractError> {
    match (connection_options, legacy_local_network_mode) {
        (Some(options), Some(legacy_local_network_mode)) => {
            let legacy_mode = if legacy_local_network_mode {
                ProviderNetworkMode::LocalLoopback
            } else {
                ProviderNetworkMode::Public
            };
            if options.network_mode != legacy_mode {
                return Err(DiscoveryContractError::InvalidField {
                    field: "connection_options.network_mode".to_owned(),
                    reason: "conflicts with the legacy local_network_mode value".to_owned(),
                });
            }
            Ok(options)
        }
        (Some(options), None) => Ok(options),
        (None, legacy_local_network_mode) => {
            let mut options = ProviderDiscoveryConnectionOptions::default();
            if legacy_local_network_mode.unwrap_or(false) {
                options.network_mode = ProviderNetworkMode::LocalLoopback;
            }
            Ok(options)
        }
    }
}

fn validate_connection_option_entry(
    entry: &ConnectionConfigEntry,
) -> Result<(), DiscoveryContractError> {
    validate_bounded_text(
        "connection_options.values.key",
        &entry.key,
        128,
        "must be a nonempty trimmed key of at most 128 characters",
    )?;
    if is_sensitive_configuration_key(&entry.key) {
        return Err(DiscoveryContractError::InvalidField {
            field: "connection_options.values.key".to_owned(),
            reason: "credential-like keys are forbidden; use credential_ref".to_owned(),
        });
    }
    if let ConnectionConfigValue::Text(value) = &entry.value {
        if value.trim() != value
            || value.chars().count() > MAX_PROVIDER_DISCOVERY_CONNECTION_TEXT_CHARS
            || value.chars().any(char::is_control)
        {
            return Err(DiscoveryContractError::InvalidField {
                field: format!("connection_options.values[{}]", entry.key),
                reason: format!(
                    "text must be trimmed, control-free, and at most \
                     {MAX_PROVIDER_DISCOVERY_CONNECTION_TEXT_CHARS} characters"
                ),
            });
        }
        if looks_like_credential_material(value) {
            return Err(DiscoveryContractError::InvalidField {
                field: format!("connection_options.values[{}]", entry.key),
                reason: "credential-like material is forbidden; use credential_ref".to_owned(),
            });
        }
    }
    Ok(())
}

fn validate_local_network_approval(
    approval: &ProviderLocalNetworkApproval,
) -> Result<(), DiscoveryContractError> {
    if !(1..=16).contains(&approval.addresses.len()) {
        return Err(DiscoveryContractError::InvalidField {
            field: "connection_options.local_network_approval.addresses".to_owned(),
            reason: "must contain from 1 to 16 exact private IP addresses".to_owned(),
        });
    }
    let mut addresses = BTreeSet::new();
    for address in &approval.addresses {
        if !is_private_network_address(*address) {
            return Err(DiscoveryContractError::InvalidField {
                field: "connection_options.local_network_approval.addresses".to_owned(),
                reason: format!("{address} is not an RFC1918 IPv4 or ULA IPv6 address"),
            });
        }
        if !addresses.insert(*address) {
            return Err(DiscoveryContractError::InvalidField {
                field: "connection_options.local_network_approval.addresses".to_owned(),
                reason: format!("contains duplicate address {address}"),
            });
        }
    }
    Ok(())
}

fn is_private_network_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => address.is_private(),
        IpAddr::V6(address) => address.is_unique_local(),
    }
}

fn is_sensitive_configuration_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect::<String>();
    normalized == "authorization"
        || normalized == "apikey"
        || normalized.ends_with("apikey")
        || normalized.contains("password")
        || normalized.contains("secret")
        || normalized.ends_with("token")
        || normalized.contains("credential")
        || normalized == "cookie"
}

fn looks_like_credential_material(value: &str) -> bool {
    const SECRET_PREFIXES: [&str; 10] = [
        "sk-proj-",
        "sk-ant-",
        "sk-or-",
        "sk-",
        "AIza",
        "xoxb-",
        "xoxp-",
        "ghp_",
        "github_pat_",
        "AKIA",
    ];
    let lower = value.to_ascii_lowercase();
    lower.starts_with("bearer ")
        || lower.starts_with("basic ")
        || lower.contains("api_key=")
        || lower.contains("apikey=")
        || lower.contains("access_token=")
        || lower.contains("secret=")
        || lower.contains("password=")
        || lower.contains("-----begin private key-----")
        || lower.contains("-----begin rsa private key-----")
        || SECRET_PREFIXES
            .iter()
            .any(|prefix| value.starts_with(prefix))
}

fn validate_bounded_text(
    field: &str,
    value: &str,
    max_chars: usize,
    reason: &str,
) -> Result<(), DiscoveryContractError> {
    if value.is_empty()
        || value.trim() != value
        || value.chars().count() > max_chars
        || value.chars().any(char::is_control)
    {
        return Err(DiscoveryContractError::InvalidField {
            field: field.to_owned(),
            reason: reason.to_owned(),
        });
    }
    Ok(())
}

fn validate_redacted_url(field: &str, value: &HttpUrl) -> Result<(), DiscoveryContractError> {
    let parsed =
        Url::parse(value.as_str()).map_err(|error| DiscoveryContractError::InvalidField {
            field: field.to_owned(),
            reason: error.to_string(),
        })?;
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(DiscoveryContractError::InvalidField {
            field: field.to_owned(),
            reason: "persisted discovery URLs must not contain a query or fragment".to_owned(),
        });
    }
    Ok(())
}

fn validate_opaque_reference(field: &str, value: &str) -> Result<(), DiscoveryContractError> {
    if value.is_empty()
        || value.len() > 256
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(DiscoveryContractError::InvalidField {
            field: field.to_owned(),
            reason: "must be a bounded opaque reference".to_owned(),
        });
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryState {
    Draft,
    ResolvingKnownProvider,
    AwaitingTemplateSelection,
    FetchingDocuments,
    ExtractingEvidence,
    AwaitingMoreEvidence,
    AwaitingAssistantConsent,
    BuildingDeterministicManifestDraft,
    BuildingAssistantManifestDraft,
    ValidatingManifest,
    AwaitingCredentialOriginApproval,
    ListingModels,
    AwaitingProbeConsent,
    ProbingCapabilities,
    AwaitingReview,
    Committing,
    Compensating,
    Ready,
    Failed,
    Cancelled,
    Interrupted,
    UnknownOutcome,
}

impl DiscoveryState {
    pub const ALL: [Self; 22] = [
        Self::Draft,
        Self::ResolvingKnownProvider,
        Self::AwaitingTemplateSelection,
        Self::FetchingDocuments,
        Self::ExtractingEvidence,
        Self::AwaitingMoreEvidence,
        Self::AwaitingAssistantConsent,
        Self::BuildingDeterministicManifestDraft,
        Self::BuildingAssistantManifestDraft,
        Self::ValidatingManifest,
        Self::AwaitingCredentialOriginApproval,
        Self::ListingModels,
        Self::AwaitingProbeConsent,
        Self::ProbingCapabilities,
        Self::AwaitingReview,
        Self::Committing,
        Self::Compensating,
        Self::Ready,
        Self::Failed,
        Self::Cancelled,
        Self::Interrupted,
        Self::UnknownOutcome,
    ];

    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Ready | Self::Failed | Self::Cancelled)
    }

    pub const fn is_waiting_for_user(self) -> bool {
        matches!(
            self,
            Self::AwaitingTemplateSelection
                | Self::AwaitingMoreEvidence
                | Self::AwaitingAssistantConsent
                | Self::AwaitingCredentialOriginApproval
                | Self::AwaitingProbeConsent
                | Self::AwaitingReview
                | Self::Interrupted
                | Self::UnknownOutcome
        )
    }

    pub const fn operation(self) -> Option<DiscoveryOperationKind> {
        match self {
            Self::ResolvingKnownProvider => Some(DiscoveryOperationKind::ResolveKnownProvider),
            Self::FetchingDocuments => Some(DiscoveryOperationKind::FetchDocuments),
            Self::ExtractingEvidence => Some(DiscoveryOperationKind::ExtractEvidence),
            Self::BuildingDeterministicManifestDraft => {
                Some(DiscoveryOperationKind::BuildDeterministicManifestDraft)
            }
            Self::BuildingAssistantManifestDraft => {
                Some(DiscoveryOperationKind::BuildAssistantManifestDraft)
            }
            Self::ValidatingManifest => Some(DiscoveryOperationKind::ValidateManifest),
            Self::ListingModels => Some(DiscoveryOperationKind::ListModels),
            Self::ProbingCapabilities => Some(DiscoveryOperationKind::ProbeCapabilities),
            Self::Committing => Some(DiscoveryOperationKind::AtomicCommit),
            Self::Compensating => Some(DiscoveryOperationKind::Compensation),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryOperationKind {
    ResolveKnownProvider,
    FetchDocuments,
    ExtractEvidence,
    BuildDeterministicManifestDraft,
    BuildAssistantManifestDraft,
    ValidateManifest,
    ListModels,
    ProbeCapabilities,
    AtomicCommit,
    Compensation,
}

impl DiscoveryOperationKind {
    pub const fn side_effect_class(self) -> DiscoverySideEffectClass {
        match self {
            Self::BuildDeterministicManifestDraft => DiscoverySideEffectClass::LocalDeterministic,
            Self::ResolveKnownProvider
            | Self::FetchDocuments
            | Self::ExtractEvidence
            | Self::ValidateManifest
            | Self::ListModels => DiscoverySideEffectClass::ReadOnly,
            Self::BuildAssistantManifestDraft | Self::ProbeCapabilities => {
                DiscoverySideEffectClass::BillableExternal
            }
            Self::AtomicCommit | Self::Compensation => DiscoverySideEffectClass::Persistent,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoverySideEffectClass {
    LocalDeterministic,
    ReadOnly,
    BillableExternal,
    Persistent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryInterruptionOutcome {
    ConfirmedNoExternalEffect,
    ExternalOutcomeUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryRecoveryCheckpoint {
    pub interrupted_state: DiscoveryState,
    pub operation: DiscoveryOperationKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderDiscoverySession {
    pub id: DiscoverySessionId,
    pub input: SanitizedDiscoveryInput,
    pub state: DiscoveryState,
    /// Compare-and-swap revision. Every accepted action increments it exactly once.
    pub revision: u64,
    /// Sequence assigned to the next event in this session's durable outbox.
    pub next_event_sequence: u64,
    pub recovery: Option<DiscoveryRecoveryCheckpoint>,
    pub unknown_operation: Option<DiscoveryOperationKind>,
    pub manifest_sha256: Option<String>,
    pub commit_plan_sha256: Option<String>,
    pub commit_attempt_id: Option<DiscoveryCommitAttemptId>,
    pub committed_connection_id: Option<ProviderConnectionId>,
    pub cancellation_pending: bool,
    /// Exact immutable approval record that authorizes the current billable effect.
    pub active_effect_approval: Option<DiscoveryApprovalBinding>,
    /// Stable, redacted failure retained across process restarts.
    pub failure: Option<DiscoveryFailure>,
}

impl ProviderDiscoverySession {
    pub fn new(
        id: DiscoverySessionId,
        input: SanitizedDiscoveryInput,
    ) -> Result<Self, DiscoveryContractError> {
        input.validate()?;
        Ok(Self {
            id,
            input,
            state: DiscoveryState::Draft,
            revision: 0,
            next_event_sequence: 1,
            recovery: None,
            unknown_operation: None,
            manifest_sha256: None,
            commit_plan_sha256: None,
            commit_attempt_id: None,
            committed_connection_id: None,
            cancellation_pending: false,
            active_effect_approval: None,
            failure: None,
        })
    }

    #[allow(clippy::too_many_lines)]
    pub fn validate(&self) -> Result<(), DiscoveryContractError> {
        self.input.validate()?;
        if self.next_event_sequence == 0 {
            return Err(DiscoveryContractError::InvalidField {
                field: "next_event_sequence".to_owned(),
                reason: "must be positive".to_owned(),
            });
        }
        if let Some(hash) = &self.commit_plan_sha256 {
            validate_sha256("commit_plan_sha256", hash)?;
        }
        if let Some(hash) = &self.manifest_sha256 {
            validate_sha256("manifest_sha256", hash)?;
        }
        if let Some(binding) = &self.active_effect_approval {
            binding.validate()?;
        }
        if let Some(failure) = &self.failure {
            failure.validate()?;
            if !matches!(
                self.state,
                DiscoveryState::Compensating | DiscoveryState::Failed | DiscoveryState::Cancelled
            ) {
                return Err(DiscoveryContractError::InvalidField {
                    field: "failure".to_owned(),
                    reason: "is only valid for compensating, failed, or cancelled sessions"
                        .to_owned(),
                });
            }
        }
        if self.state == DiscoveryState::Interrupted {
            let checkpoint =
                self.recovery
                    .as_ref()
                    .ok_or_else(|| DiscoveryContractError::InvalidField {
                        field: "recovery".to_owned(),
                        reason: "interrupted sessions require a recovery checkpoint".to_owned(),
                    })?;
            if checkpoint.interrupted_state.operation() != Some(checkpoint.operation) {
                return Err(DiscoveryContractError::InvalidField {
                    field: "recovery.operation".to_owned(),
                    reason: "must match the interrupted state".to_owned(),
                });
            }
        } else if self.recovery.is_some() {
            return Err(DiscoveryContractError::InvalidField {
                field: "recovery".to_owned(),
                reason: "is only valid while interrupted".to_owned(),
            });
        }
        if self.state == DiscoveryState::UnknownOutcome {
            if self.unknown_operation.is_none() {
                return Err(DiscoveryContractError::InvalidField {
                    field: "unknown_operation".to_owned(),
                    reason: "unknown outcomes must identify the interrupted operation".to_owned(),
                });
            }
        } else if self.unknown_operation.is_some() {
            return Err(DiscoveryContractError::InvalidField {
                field: "unknown_operation".to_owned(),
                reason: "is only valid for unknown outcomes".to_owned(),
            });
        }
        let approval_required = match self.state {
            DiscoveryState::BuildingAssistantManifestDraft
            | DiscoveryState::ProbingCapabilities => true,
            DiscoveryState::Interrupted => self.recovery.as_ref().is_some_and(|checkpoint| {
                matches!(
                    checkpoint.operation,
                    DiscoveryOperationKind::BuildAssistantManifestDraft
                        | DiscoveryOperationKind::ProbeCapabilities
                )
            }),
            _ => false,
        };
        let approval_allowed = approval_required
            || (self.state == DiscoveryState::UnknownOutcome
                && matches!(
                    self.unknown_operation,
                    Some(
                        DiscoveryOperationKind::BuildAssistantManifestDraft
                            | DiscoveryOperationKind::ProbeCapabilities
                    )
                ));
        if approval_required && self.active_effect_approval.is_none()
            || !approval_allowed && self.active_effect_approval.is_some()
        {
            return Err(DiscoveryContractError::InvalidField {
                field: "active_effect_approval".to_owned(),
                reason:
                    "must exist exactly while an approved billable effect is active or recoverable"
                        .to_owned(),
            });
        }
        if matches!(
            self.state,
            DiscoveryState::Committing | DiscoveryState::Compensating
        ) && (self.commit_plan_sha256.is_none() || self.commit_attempt_id.is_none())
        {
            return Err(DiscoveryContractError::InvalidField {
                field: "commit".to_owned(),
                reason: "commit and compensation states require a plan and attempt".to_owned(),
            });
        }
        if self.state == DiscoveryState::Ready && self.committed_connection_id.is_none() {
            return Err(DiscoveryContractError::InvalidField {
                field: "committed_connection_id".to_owned(),
                reason: "ready sessions must identify the committed connection".to_owned(),
            });
        }
        Ok(())
    }

    /// Evaluates one revision-checked action.
    ///
    /// The global interruption, cancellation, and failure rules remain next to
    /// ordinary transitions so a reviewer can audit every state-changing path.
    #[allow(clippy::too_many_lines)]
    pub fn apply(
        &self,
        envelope: &DiscoveryActionEnvelope,
    ) -> Result<DiscoveryTransition, DiscoveryTransitionError> {
        self.validate()
            .map_err(DiscoveryTransitionError::InvalidSession)?;
        envelope
            .validate()
            .map_err(DiscoveryTransitionError::InvalidAction)?;
        if envelope.expected_revision != self.revision {
            return Err(DiscoveryTransitionError::RevisionConflict {
                expected: envelope.expected_revision,
                actual: self.revision,
            });
        }

        let mut next = self.clone();
        let mut effect = DiscoveryEffect::None;
        let mut warning = None;

        match &envelope.action {
            ProviderDiscoveryAction::Cancel => {
                if self.state.is_terminal() {
                    return Err(invalid_transition(self.state, envelope.action.kind()));
                }
                match self.state {
                    DiscoveryState::UnknownOutcome => {
                        return Err(DiscoveryTransitionError::UnknownOutcomeRequiresReconciliation);
                    }
                    DiscoveryState::Compensating => {
                        // Compensation is already the safe cancellation path.
                        // A second cancel action records the intent but never
                        // starts a duplicate external operation.
                        next.cancellation_pending = true;
                        warning = Some(DiscoveryWarning::CompensationRequired);
                    }
                    state if state.operation().is_some() => {
                        if self.cancellation_pending {
                            // The cancellation signal has already been issued.
                            // A repeated action must not issue it twice.
                            effect = DiscoveryEffect::None;
                        } else {
                            let operation = state.operation().ok_or_else(|| {
                                invalid_transition(self.state, envelope.action.kind())
                            })?;
                            next.cancellation_pending = true;
                            effect = DiscoveryEffect::RequestCancellation { operation };
                        }
                    }
                    _ => {
                        next.state = DiscoveryState::Cancelled;
                        clear_recovery_markers(&mut next);
                    }
                }
            }
            ProviderDiscoveryAction::Interrupt { operation, outcome } => {
                if self.state.operation() != Some(*operation)
                    || (self.state == DiscoveryState::Compensating && self.failure.is_some())
                {
                    return Err(invalid_transition(self.state, envelope.action.kind()));
                }
                match outcome {
                    DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect => {
                        if self.cancellation_pending {
                            if matches!(
                                *operation,
                                DiscoveryOperationKind::AtomicCommit
                                    | DiscoveryOperationKind::Compensation
                            ) {
                                // A native credential slot can exist before the
                                // database graph is applied. Even a definitely
                                // unstarted commit therefore needs its typed
                                // cleanup recipe and an explicit second action.
                                next.state = DiscoveryState::Interrupted;
                                next.recovery = Some(DiscoveryRecoveryCheckpoint {
                                    interrupted_state: DiscoveryState::Compensating,
                                    operation: DiscoveryOperationKind::Compensation,
                                });
                                next.unknown_operation = None;
                                warning = Some(DiscoveryWarning::ExplicitRestartRequired);
                            } else {
                                next.state = DiscoveryState::Cancelled;
                                next.cancellation_pending = false;
                                clear_recovery_markers(&mut next);
                            }
                        } else {
                            next.state = DiscoveryState::Interrupted;
                            next.recovery = Some(DiscoveryRecoveryCheckpoint {
                                interrupted_state: self.state,
                                operation: *operation,
                            });
                            next.unknown_operation = None;
                            warning = Some(DiscoveryWarning::ExplicitRestartRequired);
                        }
                    }
                    DiscoveryInterruptionOutcome::ExternalOutcomeUnknown => {
                        next.state = DiscoveryState::UnknownOutcome;
                        next.recovery = None;
                        next.unknown_operation = Some(*operation);
                        warning = Some(DiscoveryWarning::UnknownExternalOutcome);
                    }
                }
            }
            ProviderDiscoveryAction::Fail { failure } => {
                if self.state.is_terminal()
                    || matches!(
                        self.state,
                        DiscoveryState::Committing
                            | DiscoveryState::Compensating
                            | DiscoveryState::UnknownOutcome
                    )
                {
                    return Err(invalid_transition(self.state, envelope.action.kind()));
                }
                failure
                    .validate()
                    .map_err(DiscoveryTransitionError::InvalidAction)?;
                next.state = if self.cancellation_pending {
                    DiscoveryState::Cancelled
                } else {
                    DiscoveryState::Failed
                };
                next.failure = Some(failure.clone());
                next.cancellation_pending = false;
                clear_recovery_markers(&mut next);
            }
            action => {
                if self.cancellation_pending
                    && self.state.operation().is_some()
                    && !matches!(
                        self.state,
                        DiscoveryState::Committing | DiscoveryState::Compensating
                    )
                {
                    if !is_operation_completion(self.state, action) {
                        return Err(invalid_transition(self.state, action.kind()));
                    }
                    next.state = DiscoveryState::Cancelled;
                    next.cancellation_pending = false;
                    clear_recovery_markers(&mut next);
                } else {
                    apply_state_action(self, &mut next, action, &mut effect, &mut warning)?;
                }
            }
        }

        next.revision = self
            .revision
            .checked_add(1)
            .ok_or(DiscoveryTransitionError::RevisionExhausted)?;
        let event_sequence = self.next_event_sequence;
        next.next_event_sequence = event_sequence
            .checked_add(1)
            .ok_or(DiscoveryTransitionError::SequenceExhausted)?;
        next.validate()
            .map_err(DiscoveryTransitionError::InvalidSession)?;

        let event = ProviderDiscoveryEvent {
            version: PROVIDER_DISCOVERY_EVENT_VERSION,
            id: DiscoveryEventId::new(),
            session_id: self.id.clone(),
            sequence: event_sequence,
            session_revision: next.revision,
            state: next.state,
            progress: progress_for_action(&envelope.action),
            action_required: action_required_for(&next),
            warning,
            action_id: envelope.id.clone(),
            failure: next.failure.clone(),
        };
        let receipt = DiscoveryActionReceipt {
            action_id: envelope.id.clone(),
            session_id: self.id.clone(),
            action_kind: envelope.action.kind().to_owned(),
            request_sha256: envelope.request_sha256.clone(),
            expected_revision: envelope.expected_revision,
            resulting_revision: next.revision,
            event_sequence,
            outcome: DiscoveryActionReceiptOutcome::Applied,
        };

        Ok(DiscoveryTransition {
            previous_revision: self.revision,
            session: next,
            event,
            receipt,
            effect,
        })
    }
}

fn is_operation_completion(state: DiscoveryState, action: &ProviderDiscoveryAction) -> bool {
    matches!(
        (state, action),
        (
            DiscoveryState::ResolvingKnownProvider,
            ProviderDiscoveryAction::KnownProviderCandidatesResolved { .. }
        ) | (
            DiscoveryState::FetchingDocuments,
            ProviderDiscoveryAction::DocumentsFetched { .. }
        ) | (
            DiscoveryState::ExtractingEvidence,
            ProviderDiscoveryAction::EvidenceExtracted { .. }
                | ProviderDiscoveryAction::AssistantResumedWithEvidence { .. }
        ) | (
            DiscoveryState::BuildingDeterministicManifestDraft
                | DiscoveryState::BuildingAssistantManifestDraft,
            ProviderDiscoveryAction::ManifestDraftBuilt { .. }
        ) | (
            DiscoveryState::BuildingAssistantManifestDraft,
            ProviderDiscoveryAction::AssistantRequestedMoreEvidence { .. }
        ) | (
            DiscoveryState::ValidatingManifest,
            ProviderDiscoveryAction::ManifestValidated { .. }
        ) | (
            DiscoveryState::ListingModels,
            ProviderDiscoveryAction::ModelsListed { .. }
        ) | (
            DiscoveryState::ProbingCapabilities,
            ProviderDiscoveryAction::ProbesCompleted
        )
    )
}

fn clear_recovery_markers(session: &mut ProviderDiscoverySession) {
    session.recovery = None;
    session.unknown_operation = None;
    session.active_effect_approval = None;
}

// This is an intentionally explicit transition table. Keeping each
// (state, action) pair in one match makes accidental fall-through auditable.
#[allow(clippy::too_many_lines)]
fn apply_state_action(
    current: &ProviderDiscoverySession,
    next: &mut ProviderDiscoverySession,
    action: &ProviderDiscoveryAction,
    effect: &mut DiscoveryEffect,
    warning: &mut Option<DiscoveryWarning>,
) -> Result<(), DiscoveryTransitionError> {
    match (current.state, action) {
        (DiscoveryState::Draft, ProviderDiscoveryAction::Begin) => {
            next.state = DiscoveryState::ResolvingKnownProvider;
            *effect = DiscoveryEffect::ResolveKnownProvider;
        }
        (
            DiscoveryState::ResolvingKnownProvider,
            ProviderDiscoveryAction::KnownProviderCandidatesResolved { candidate_count },
        ) => {
            if *candidate_count == 0 {
                next.state = DiscoveryState::FetchingDocuments;
                *effect = DiscoveryEffect::FetchDocuments;
            } else {
                next.state = DiscoveryState::AwaitingTemplateSelection;
            }
        }
        (
            DiscoveryState::AwaitingTemplateSelection,
            ProviderDiscoveryAction::SelectTemplate { .. },
        ) => {
            next.state = DiscoveryState::ValidatingManifest;
            *effect = DiscoveryEffect::ValidateManifest;
        }
        (
            DiscoveryState::AwaitingTemplateSelection,
            ProviderDiscoveryAction::ContinueWithoutTemplate,
        ) => {
            next.state = DiscoveryState::FetchingDocuments;
            *effect = DiscoveryEffect::FetchDocuments;
        }
        (DiscoveryState::FetchingDocuments, ProviderDiscoveryAction::DocumentsFetched { .. }) => {
            next.state = DiscoveryState::ExtractingEvidence;
            *effect = DiscoveryEffect::ExtractEvidence;
        }
        (
            DiscoveryState::ExtractingEvidence,
            ProviderDiscoveryAction::EvidenceExtracted { resolution },
        ) => match resolution {
            DiscoveryEvidenceResolution::DeterministicDraftAvailable => {
                next.state = DiscoveryState::BuildingDeterministicManifestDraft;
                *effect = DiscoveryEffect::BuildDeterministicManifestDraft;
            }
            DiscoveryEvidenceResolution::AssistantRecommended => {
                next.state = DiscoveryState::AwaitingAssistantConsent;
            }
            DiscoveryEvidenceResolution::MoreEvidenceRequired => {
                next.state = DiscoveryState::AwaitingMoreEvidence;
            }
        },
        (
            DiscoveryState::ExtractingEvidence,
            ProviderDiscoveryAction::AssistantResumedWithEvidence {
                approval_id,
                approval_grant_sha256,
            },
        ) => {
            validate_sha256("approval_grant_sha256", approval_grant_sha256)
                .map_err(DiscoveryTransitionError::InvalidAction)?;
            let approval = DiscoveryApprovalBinding {
                approval_id: approval_id.clone(),
                grant_sha256: approval_grant_sha256.clone(),
            };
            approval
                .validate()
                .map_err(DiscoveryTransitionError::InvalidAction)?;
            next.active_effect_approval = Some(approval.clone());
            next.state = DiscoveryState::BuildingAssistantManifestDraft;
            *effect = DiscoveryEffect::BuildAssistantManifestDraft { approval };
        }
        (
            DiscoveryState::AwaitingMoreEvidence,
            ProviderDiscoveryAction::SupplyMoreEvidence { evidence_ids },
        ) => {
            validate_supplied_evidence_ids(evidence_ids)
                .map_err(DiscoveryTransitionError::InvalidAction)?;
            next.state = DiscoveryState::ExtractingEvidence;
            *effect = DiscoveryEffect::ExtractEvidence;
        }
        (
            DiscoveryState::AwaitingMoreEvidence,
            ProviderDiscoveryAction::SupplyFreshEvidence {
                evidence_ids,
                source,
            },
        ) => {
            validate_supplied_evidence_ids(evidence_ids)
                .map_err(DiscoveryTransitionError::InvalidAction)?;
            let source_allowed = match current.input.connection_options.network_mode {
                ProviderNetworkMode::Public => true,
                ProviderNetworkMode::LocalLoopback => {
                    let input_origin = canonical_origin_for_http_url(&current.input.site_url)
                        .map_err(DiscoveryTransitionError::InvalidAction)?;
                    source.origin() == &input_origin
                }
                ProviderNetworkMode::ApprovedLocalNetwork => matches!(
                    source,
                    DiscoveryFreshEvidenceSource::SanitizedCurl { origin }
                        if current
                            .input
                            .connection_options
                            .local_network_approval
                            .as_ref()
                            .is_some_and(|approval| origin == &approval.origin)
                ),
            };
            if !source_allowed {
                return Err(DiscoveryTransitionError::InvalidAction(
                    DiscoveryContractError::InvalidField {
                        field: "fresh_evidence.source.origin".to_owned(),
                        reason: "is outside the session network authority".to_owned(),
                    },
                ));
            }
            next.state = DiscoveryState::ExtractingEvidence;
            *effect = DiscoveryEffect::ExtractEvidence;
        }
        (DiscoveryState::AwaitingMoreEvidence, ProviderDiscoveryAction::RequestAssistant) => {
            next.state = DiscoveryState::AwaitingAssistantConsent;
        }
        (
            DiscoveryState::AwaitingAssistantConsent,
            ProviderDiscoveryAction::ApproveAssistant {
                approval_id,
                approval_grant_sha256,
            },
        ) => {
            validate_sha256("approval_grant_sha256", approval_grant_sha256)
                .map_err(DiscoveryTransitionError::InvalidAction)?;
            let approval = DiscoveryApprovalBinding {
                approval_id: approval_id.clone(),
                grant_sha256: approval_grant_sha256.clone(),
            };
            next.active_effect_approval = Some(approval.clone());
            next.state = DiscoveryState::BuildingAssistantManifestDraft;
            *effect = DiscoveryEffect::BuildAssistantManifestDraft { approval };
        }
        (DiscoveryState::AwaitingAssistantConsent, ProviderDiscoveryAction::DeclineAssistant) => {
            next.state = DiscoveryState::AwaitingMoreEvidence;
            *warning = Some(DiscoveryWarning::AssistantDeclined);
        }
        (
            DiscoveryState::BuildingDeterministicManifestDraft
            | DiscoveryState::BuildingAssistantManifestDraft,
            ProviderDiscoveryAction::ManifestDraftBuilt { manifest_sha256 },
        ) => {
            validate_sha256("manifest_sha256", manifest_sha256)
                .map_err(DiscoveryTransitionError::InvalidAction)?;
            next.manifest_sha256 = Some(manifest_sha256.clone());
            next.active_effect_approval = None;
            next.state = DiscoveryState::ValidatingManifest;
            *effect = DiscoveryEffect::ValidateManifest;
        }
        (
            DiscoveryState::BuildingAssistantManifestDraft,
            ProviderDiscoveryAction::AssistantCheckpointed { .. },
        ) => {
            // The setup assistant is host-driven. These self-transitions make
            // its bounded, secret-free checkpoint durable without completing
            // or replaying the active billable operation.
        }
        (
            DiscoveryState::BuildingAssistantManifestDraft,
            ProviderDiscoveryAction::AssistantRequestedMoreEvidence { question_count },
        ) => {
            if *question_count == 0 || *question_count > 128 {
                return Err(DiscoveryTransitionError::InvalidAction(
                    DiscoveryContractError::InvalidField {
                        field: "assistant_more_evidence.question_count".to_owned(),
                        reason: "must contain between 1 and 128 questions".to_owned(),
                    },
                ));
            }
            next.active_effect_approval = None;
            next.state = DiscoveryState::AwaitingMoreEvidence;
        }
        (
            DiscoveryState::ValidatingManifest,
            ProviderDiscoveryAction::ManifestValidated {
                manifest_sha256,
                credential_origin_approval_required,
            },
        ) => {
            validate_sha256("manifest_sha256", manifest_sha256)
                .map_err(DiscoveryTransitionError::InvalidAction)?;
            if let Some(draft_sha256) = &current.manifest_sha256
                && draft_sha256 != manifest_sha256
            {
                return Err(DiscoveryTransitionError::InvalidAction(
                    DiscoveryContractError::InvalidField {
                        field: "manifest_sha256".to_owned(),
                        reason: "validated manifest does not match the persisted draft".to_owned(),
                    },
                ));
            }
            next.manifest_sha256 = Some(manifest_sha256.clone());
            if *credential_origin_approval_required {
                next.state = DiscoveryState::AwaitingCredentialOriginApproval;
            } else {
                next.state = DiscoveryState::ListingModels;
                *effect = DiscoveryEffect::ListModels;
            }
        }
        (
            DiscoveryState::AwaitingCredentialOriginApproval,
            ProviderDiscoveryAction::ApproveCredentialOrigin { .. },
        ) => {
            next.state = DiscoveryState::ListingModels;
            *effect = DiscoveryEffect::ListModels;
        }
        (
            DiscoveryState::ListingModels,
            ProviderDiscoveryAction::ModelsListed {
                probe_candidate_count,
                ..
            },
        ) => {
            if *probe_candidate_count == 0 {
                next.state = DiscoveryState::AwaitingReview;
            } else {
                next.state = DiscoveryState::AwaitingProbeConsent;
            }
        }
        (
            DiscoveryState::AwaitingProbeConsent,
            ProviderDiscoveryAction::ApproveProbes {
                approval_id,
                approval_grant_sha256,
            },
        ) => {
            validate_sha256("approval_grant_sha256", approval_grant_sha256)
                .map_err(DiscoveryTransitionError::InvalidAction)?;
            let approval = DiscoveryApprovalBinding {
                approval_id: approval_id.clone(),
                grant_sha256: approval_grant_sha256.clone(),
            };
            next.active_effect_approval = Some(approval.clone());
            next.state = DiscoveryState::ProbingCapabilities;
            *effect = DiscoveryEffect::ProbeCapabilities { approval };
        }
        (DiscoveryState::AwaitingProbeConsent, ProviderDiscoveryAction::SkipProbes) => {
            next.state = DiscoveryState::AwaitingReview;
            *warning = Some(DiscoveryWarning::ProbesSkipped);
        }
        (DiscoveryState::ProbingCapabilities, ProviderDiscoveryAction::ProbesCompleted) => {
            next.active_effect_approval = None;
            next.state = DiscoveryState::AwaitingReview;
        }
        (
            DiscoveryState::AwaitingReview,
            ProviderDiscoveryAction::ApproveReview {
                commit_attempt_id,
                commit_plan_sha256,
                ..
            },
        ) => {
            validate_sha256("commit_plan_sha256", commit_plan_sha256)
                .map_err(DiscoveryTransitionError::InvalidAction)?;
            next.commit_plan_sha256 = Some(commit_plan_sha256.clone());
            next.commit_attempt_id = Some(commit_attempt_id.clone());
            next.state = DiscoveryState::Committing;
            *effect = DiscoveryEffect::CommitAtomically {
                commit_attempt_id: commit_attempt_id.clone(),
                plan_sha256: commit_plan_sha256.clone(),
            };
        }
        (
            DiscoveryState::Committing,
            ProviderDiscoveryAction::CommitSucceeded { connection_id },
        ) => {
            if current.cancellation_pending {
                next.state = DiscoveryState::Compensating;
                *effect = DiscoveryEffect::RunCompensation {
                    commit_attempt_id: current.commit_attempt_id.clone().ok_or_else(|| {
                        DiscoveryTransitionError::InvalidSession(
                            DiscoveryContractError::InvalidField {
                                field: "commit_attempt_id".to_owned(),
                                reason: "is required to compensate".to_owned(),
                            },
                        )
                    })?,
                };
                *warning = Some(DiscoveryWarning::CompensationRequired);
            } else {
                next.state = DiscoveryState::Ready;
                next.committed_connection_id = Some(connection_id.clone());
                next.cancellation_pending = false;
            }
        }
        (
            DiscoveryState::Committing,
            ProviderDiscoveryAction::CommitFailedBeforeApply { failure },
        ) => {
            failure
                .validate()
                .map_err(DiscoveryTransitionError::InvalidAction)?;
            next.state = if current.cancellation_pending {
                DiscoveryState::Cancelled
            } else {
                DiscoveryState::Failed
            };
            next.failure = Some(failure.clone());
            next.cancellation_pending = false;
            next.active_effect_approval = None;
        }
        (DiscoveryState::Compensating, ProviderDiscoveryAction::CompensationFailed { failure }) => {
            if current.failure.is_some() {
                return Err(invalid_transition(current.state, action.kind()));
            }
            failure
                .validate()
                .map_err(DiscoveryTransitionError::InvalidAction)?;
            // A confirmed failure is not proof that every previously applied
            // effect has been reverted. Keep the saga recoverable and require
            // an explicit resume action; never start another attempt here.
            next.failure = Some(failure.clone());
        }
        (DiscoveryState::Compensating, ProviderDiscoveryAction::ResumeCompensation) => {
            if current.failure.is_none() {
                return Err(invalid_transition(current.state, action.kind()));
            }
            next.failure = None;
            *effect = DiscoveryEffect::RunCompensation {
                commit_attempt_id: current.commit_attempt_id.clone().ok_or_else(|| {
                    DiscoveryTransitionError::InvalidSession(DiscoveryContractError::InvalidField {
                        field: "commit_attempt_id".to_owned(),
                        reason: "is required to resume compensation".to_owned(),
                    })
                })?,
            };
        }
        (DiscoveryState::Committing, ProviderDiscoveryAction::CompensationRequired) => {
            next.state = DiscoveryState::Compensating;
            *effect = DiscoveryEffect::RunCompensation {
                commit_attempt_id: current.commit_attempt_id.clone().ok_or_else(|| {
                    DiscoveryTransitionError::InvalidSession(DiscoveryContractError::InvalidField {
                        field: "commit_attempt_id".to_owned(),
                        reason: "is required to compensate".to_owned(),
                    })
                })?,
            };
            *warning = Some(DiscoveryWarning::CompensationRequired);
        }
        (DiscoveryState::Compensating, ProviderDiscoveryAction::CompensationSucceeded) => {
            if current.failure.is_some() {
                return Err(invalid_transition(current.state, action.kind()));
            }
            next.state = if current.cancellation_pending {
                DiscoveryState::Cancelled
            } else {
                DiscoveryState::Failed
            };
            next.cancellation_pending = false;
            next.active_effect_approval = None;
        }
        (
            DiscoveryState::Committing | DiscoveryState::Compensating,
            ProviderDiscoveryAction::ExternalOutcomeBecameUnknown,
        ) => {
            if current.state == DiscoveryState::Compensating && current.failure.is_some() {
                return Err(invalid_transition(current.state, action.kind()));
            }
            let operation = current
                .state
                .operation()
                .ok_or_else(|| invalid_transition(current.state, action.kind()))?;
            next.state = DiscoveryState::UnknownOutcome;
            next.unknown_operation = Some(operation);
            next.recovery = None;
            *warning = Some(DiscoveryWarning::UnknownExternalOutcome);
        }
        (DiscoveryState::Interrupted, ProviderDiscoveryAction::RestartInterrupted) => {
            let checkpoint = current.recovery.as_ref().ok_or_else(|| {
                DiscoveryTransitionError::InvalidSession(DiscoveryContractError::InvalidField {
                    field: "recovery".to_owned(),
                    reason: "is required to restart".to_owned(),
                })
            })?;
            next.state = checkpoint.interrupted_state;
            next.recovery = None;
            next.unknown_operation = None;
            *effect = effect_for_operation(current, checkpoint.operation)?;
        }
        (
            DiscoveryState::UnknownOutcome,
            ProviderDiscoveryAction::ResolveUnknownOutcome { resolution, .. },
        ) => match resolution {
            DiscoveryUnknownOutcomeResolution::ConfirmedNoEffect => {
                let operation = current.unknown_operation.ok_or_else(|| {
                    DiscoveryTransitionError::InvalidSession(DiscoveryContractError::InvalidField {
                        field: "unknown_operation".to_owned(),
                        reason: "is required to reconcile".to_owned(),
                    })
                })?;
                next.unknown_operation = None;
                if current.cancellation_pending {
                    if matches!(
                        operation,
                        DiscoveryOperationKind::AtomicCommit | DiscoveryOperationKind::Compensation
                    ) {
                        // Cancellation can still require native credential or
                        // graph cleanup even when the unknown operation itself
                        // is confirmed to have had no effect. Make cleanup an
                        // explicit second action; never terminalize a saga with
                        // an unapplied compensation recipe.
                        next.state = DiscoveryState::Interrupted;
                        next.recovery = Some(DiscoveryRecoveryCheckpoint {
                            interrupted_state: DiscoveryState::Compensating,
                            operation: DiscoveryOperationKind::Compensation,
                        });
                        *warning = Some(DiscoveryWarning::ExplicitRestartRequired);
                    } else {
                        next.state = DiscoveryState::Cancelled;
                        next.recovery = None;
                        next.cancellation_pending = false;
                    }
                } else {
                    let interrupted_state = state_for_operation(operation);
                    next.state = DiscoveryState::Interrupted;
                    next.recovery = Some(DiscoveryRecoveryCheckpoint {
                        interrupted_state,
                        operation,
                    });
                    *warning = Some(DiscoveryWarning::ExplicitRestartRequired);
                }
            }
            DiscoveryUnknownOutcomeResolution::ConfirmedCommitCompleted { connection_id } => {
                if current.unknown_operation != Some(DiscoveryOperationKind::AtomicCommit) {
                    return Err(invalid_transition(current.state, action.kind()));
                }
                next.unknown_operation = None;
                if current.cancellation_pending {
                    next.state = DiscoveryState::Compensating;
                    *effect = DiscoveryEffect::RunCompensation {
                        commit_attempt_id: current.commit_attempt_id.clone().ok_or_else(|| {
                            DiscoveryTransitionError::InvalidSession(
                                DiscoveryContractError::InvalidField {
                                    field: "commit_attempt_id".to_owned(),
                                    reason: "is required to compensate".to_owned(),
                                },
                            )
                        })?,
                    };
                    *warning = Some(DiscoveryWarning::CompensationRequired);
                } else {
                    next.state = DiscoveryState::Ready;
                    next.committed_connection_id = Some(connection_id.clone());
                    next.cancellation_pending = false;
                }
            }
            DiscoveryUnknownOutcomeResolution::ConfirmedCompensated => {
                if current.unknown_operation != Some(DiscoveryOperationKind::Compensation)
                    && current.unknown_operation != Some(DiscoveryOperationKind::AtomicCommit)
                {
                    return Err(invalid_transition(current.state, action.kind()));
                }
                next.state = if current.cancellation_pending {
                    DiscoveryState::Cancelled
                } else {
                    DiscoveryState::Failed
                };
                next.unknown_operation = None;
                next.cancellation_pending = false;
                next.active_effect_approval = None;
            }
            DiscoveryUnknownOutcomeResolution::ManuallyReconciledAsFailed => {
                next.state = if current.cancellation_pending {
                    DiscoveryState::Cancelled
                } else {
                    DiscoveryState::Failed
                };
                next.unknown_operation = None;
                next.cancellation_pending = false;
                next.active_effect_approval = None;
            }
        },
        _ => return Err(invalid_transition(current.state, action.kind())),
    }
    Ok(())
}

fn state_for_operation(operation: DiscoveryOperationKind) -> DiscoveryState {
    match operation {
        DiscoveryOperationKind::ResolveKnownProvider => DiscoveryState::ResolvingKnownProvider,
        DiscoveryOperationKind::FetchDocuments => DiscoveryState::FetchingDocuments,
        DiscoveryOperationKind::ExtractEvidence => DiscoveryState::ExtractingEvidence,
        DiscoveryOperationKind::BuildDeterministicManifestDraft => {
            DiscoveryState::BuildingDeterministicManifestDraft
        }
        DiscoveryOperationKind::BuildAssistantManifestDraft => {
            DiscoveryState::BuildingAssistantManifestDraft
        }
        DiscoveryOperationKind::ValidateManifest => DiscoveryState::ValidatingManifest,
        DiscoveryOperationKind::ListModels => DiscoveryState::ListingModels,
        DiscoveryOperationKind::ProbeCapabilities => DiscoveryState::ProbingCapabilities,
        DiscoveryOperationKind::AtomicCommit => DiscoveryState::Committing,
        DiscoveryOperationKind::Compensation => DiscoveryState::Compensating,
    }
}

fn effect_for_operation(
    session: &ProviderDiscoverySession,
    operation: DiscoveryOperationKind,
) -> Result<DiscoveryEffect, DiscoveryTransitionError> {
    match operation {
        DiscoveryOperationKind::ResolveKnownProvider => Ok(DiscoveryEffect::ResolveKnownProvider),
        DiscoveryOperationKind::FetchDocuments => Ok(DiscoveryEffect::FetchDocuments),
        DiscoveryOperationKind::ExtractEvidence => Ok(DiscoveryEffect::ExtractEvidence),
        DiscoveryOperationKind::BuildDeterministicManifestDraft => {
            Ok(DiscoveryEffect::BuildDeterministicManifestDraft)
        }
        DiscoveryOperationKind::BuildAssistantManifestDraft => {
            let approval = session.active_effect_approval.clone().ok_or_else(|| {
                DiscoveryTransitionError::InvalidSession(DiscoveryContractError::InvalidField {
                    field: "active_effect_approval".to_owned(),
                    reason: "is required to restart assistant manifest generation".to_owned(),
                })
            })?;
            Ok(DiscoveryEffect::BuildAssistantManifestDraft { approval })
        }
        DiscoveryOperationKind::ValidateManifest => Ok(DiscoveryEffect::ValidateManifest),
        DiscoveryOperationKind::ListModels => Ok(DiscoveryEffect::ListModels),
        DiscoveryOperationKind::ProbeCapabilities => {
            let approval = session.active_effect_approval.clone().ok_or_else(|| {
                DiscoveryTransitionError::InvalidSession(DiscoveryContractError::InvalidField {
                    field: "active_effect_approval".to_owned(),
                    reason: "is required to restart capability probes".to_owned(),
                })
            })?;
            Ok(DiscoveryEffect::ProbeCapabilities { approval })
        }
        DiscoveryOperationKind::AtomicCommit => {
            let commit_attempt_id = session.commit_attempt_id.clone().ok_or_else(|| {
                DiscoveryTransitionError::InvalidSession(DiscoveryContractError::InvalidField {
                    field: "commit_attempt_id".to_owned(),
                    reason: "is required to restart a commit".to_owned(),
                })
            })?;
            let plan_sha256 = session.commit_plan_sha256.clone().ok_or_else(|| {
                DiscoveryTransitionError::InvalidSession(DiscoveryContractError::InvalidField {
                    field: "commit_plan_sha256".to_owned(),
                    reason: "is required to restart a commit".to_owned(),
                })
            })?;
            Ok(DiscoveryEffect::CommitAtomically {
                commit_attempt_id,
                plan_sha256,
            })
        }
        DiscoveryOperationKind::Compensation => {
            let commit_attempt_id = session.commit_attempt_id.clone().ok_or_else(|| {
                DiscoveryTransitionError::InvalidSession(DiscoveryContractError::InvalidField {
                    field: "commit_attempt_id".to_owned(),
                    reason: "is required to resume compensation".to_owned(),
                })
            })?;
            Ok(DiscoveryEffect::RunCompensation { commit_attempt_id })
        }
    }
}

fn invalid_transition(state: DiscoveryState, action: &'static str) -> DiscoveryTransitionError {
    DiscoveryTransitionError::InvalidTransition { state, action }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryActionEnvelope {
    pub id: DiscoveryActionId,
    pub expected_revision: u64,
    /// Hash of the canonical, redacted action payload used for idempotency.
    pub request_sha256: String,
    pub action: ProviderDiscoveryAction,
}

impl DiscoveryActionEnvelope {
    pub fn validate(&self) -> Result<(), DiscoveryContractError> {
        validate_sha256("request_sha256", &self.request_sha256)?;
        match &self.action {
            ProviderDiscoveryAction::ApproveAssistant {
                approval_grant_sha256,
                ..
            }
            | ProviderDiscoveryAction::ApproveProbes {
                approval_grant_sha256,
                ..
            } => validate_sha256("approval_grant_sha256", approval_grant_sha256),
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProviderDiscoveryAction {
    Begin,
    KnownProviderCandidatesResolved {
        candidate_count: u32,
    },
    SelectTemplate {
        candidate_id: DiscoveryCandidateId,
    },
    ContinueWithoutTemplate,
    DocumentsFetched {
        evidence_count: u32,
    },
    EvidenceExtracted {
        resolution: DiscoveryEvidenceResolution,
    },
    SupplyMoreEvidence {
        evidence_ids: Vec<EvidenceId>,
    },
    /// Records evidence produced from one bounded, deterministic Core
    /// collection. The raw document body or pasted cURL has no representation
    /// in this durable action.
    SupplyFreshEvidence {
        evidence_ids: Vec<EvidenceId>,
        source: DiscoveryFreshEvidenceSource,
    },
    RequestAssistant,
    ApproveAssistant {
        approval_id: DiscoveryApprovalId,
        /// SHA-256 of the exact immutable typed grant persisted with this action.
        approval_grant_sha256: String,
    },
    DeclineAssistant,
    AssistantCheckpointed {
        checkpoint: DiscoveryAssistantCheckpoint,
    },
    /// Suspends the current assistant attempt while preserving its bounded,
    /// origin-scoped approval for safe evidence supplied through Core.
    AssistantRequestedMoreEvidence {
        question_count: u32,
    },
    /// Completes deterministic extraction of newly supplied evidence and
    /// resumes the existing assistant attempt under its exact prior approval.
    ///
    /// This is an internal Core completion action. The setup assistant accepts
    /// only redacted evidence from an origin already covered by that approval.
    AssistantResumedWithEvidence {
        approval_id: DiscoveryApprovalId,
        approval_grant_sha256: String,
    },
    ManifestDraftBuilt {
        manifest_sha256: String,
    },
    ManifestValidated {
        manifest_sha256: String,
        credential_origin_approval_required: bool,
    },
    ApproveCredentialOrigin {
        approval_id: DiscoveryApprovalId,
    },
    ModelsListed {
        model_count: u32,
        probe_candidate_count: u32,
    },
    ApproveProbes {
        approval_id: DiscoveryApprovalId,
        /// SHA-256 of the exact immutable typed grant persisted with this action.
        approval_grant_sha256: String,
    },
    SkipProbes,
    ProbesCompleted,
    ApproveReview {
        approval_id: DiscoveryApprovalId,
        commit_attempt_id: DiscoveryCommitAttemptId,
        commit_plan_sha256: String,
        graph_sha256: String,
    },
    CommitSucceeded {
        connection_id: ProviderConnectionId,
    },
    CommitFailedBeforeApply {
        failure: DiscoveryFailure,
    },
    CompensationRequired,
    CompensationSucceeded,
    CompensationFailed {
        failure: DiscoveryFailure,
    },
    /// Explicitly resumes a previously failed compensation attempt.
    ResumeCompensation,
    ExternalOutcomeBecameUnknown,
    Interrupt {
        operation: DiscoveryOperationKind,
        outcome: DiscoveryInterruptionOutcome,
    },
    RestartInterrupted,
    ResolveUnknownOutcome {
        approval_id: DiscoveryApprovalId,
        resolution: DiscoveryUnknownOutcomeResolution,
    },
    Fail {
        failure: DiscoveryFailure,
    },
    Cancel,
}

impl ProviderDiscoveryAction {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Begin => "begin",
            Self::KnownProviderCandidatesResolved { .. } => "known_provider_candidates_resolved",
            Self::SelectTemplate { .. } => "select_template",
            Self::ContinueWithoutTemplate => "continue_without_template",
            Self::DocumentsFetched { .. } => "documents_fetched",
            Self::EvidenceExtracted { .. } => "evidence_extracted",
            Self::SupplyMoreEvidence { .. } => "supply_more_evidence",
            Self::SupplyFreshEvidence { .. } => "supply_fresh_evidence",
            Self::RequestAssistant => "request_assistant",
            Self::ApproveAssistant { .. } => "approve_assistant",
            Self::DeclineAssistant => "decline_assistant",
            Self::AssistantCheckpointed { .. } => "assistant_checkpointed",
            Self::AssistantRequestedMoreEvidence { .. } => "assistant_requested_more_evidence",
            Self::AssistantResumedWithEvidence { .. } => "assistant_resumed_with_evidence",
            Self::ManifestDraftBuilt { .. } => "manifest_draft_built",
            Self::ManifestValidated { .. } => "manifest_validated",
            Self::ApproveCredentialOrigin { .. } => "approve_credential_origin",
            Self::ModelsListed { .. } => "models_listed",
            Self::ApproveProbes { .. } => "approve_probes",
            Self::SkipProbes => "skip_probes",
            Self::ProbesCompleted => "probes_completed",
            Self::ApproveReview { .. } => "approve_review",
            Self::CommitSucceeded { .. } => "commit_succeeded",
            Self::CommitFailedBeforeApply { .. } => "commit_failed_before_apply",
            Self::CompensationRequired => "compensation_required",
            Self::CompensationSucceeded => "compensation_succeeded",
            Self::CompensationFailed { .. } => "compensation_failed",
            Self::ResumeCompensation => "resume_compensation",
            Self::ExternalOutcomeBecameUnknown => "external_outcome_became_unknown",
            Self::Interrupt { .. } => "interrupt",
            Self::RestartInterrupted => "restart_interrupted",
            Self::ResolveUnknownOutcome { .. } => "resolve_unknown_outcome",
            Self::Fail { .. } => "fail",
            Self::Cancel => "cancel",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DiscoveryFreshEvidenceSource {
    DocumentUrl { origin: CanonicalOrigin },
    SanitizedCurl { origin: CanonicalOrigin },
}

impl DiscoveryFreshEvidenceSource {
    pub const fn origin(&self) -> &CanonicalOrigin {
        match self {
            Self::DocumentUrl { origin } | Self::SanitizedCurl { origin } => origin,
        }
    }
}

fn validate_supplied_evidence_ids(
    evidence_ids: &[EvidenceId],
) -> Result<(), DiscoveryContractError> {
    if evidence_ids.is_empty() || evidence_ids.len() > 4_096 {
        return Err(DiscoveryContractError::InvalidField {
            field: "evidence_ids".to_owned(),
            reason: "must contain between 1 and 4096 evidence references".to_owned(),
        });
    }
    let unique = evidence_ids
        .iter()
        .collect::<std::collections::BTreeSet<_>>();
    if unique.len() != evidence_ids.len() {
        return Err(DiscoveryContractError::InvalidField {
            field: "evidence_ids".to_owned(),
            reason: "must not contain duplicate evidence references".to_owned(),
        });
    }
    Ok(())
}

fn canonical_origin_for_http_url(
    value: &HttpUrl,
) -> Result<CanonicalOrigin, DiscoveryContractError> {
    let parsed =
        Url::parse(value.as_str()).map_err(|error| DiscoveryContractError::InvalidField {
            field: "site_url".to_owned(),
            reason: error.to_string(),
        })?;
    CanonicalOrigin::parse(&parsed.origin().ascii_serialization()).map_err(|error| {
        DiscoveryContractError::InvalidField {
            field: "site_url".to_owned(),
            reason: error.clone(),
        }
    })
}

/// Host-driven setup-assistant progress which is safe to persist.
///
/// The prompt, model credential, raw model response, and tool payloads are
/// deliberately absent. Their bounded redacted state lives in `draft_json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryAssistantCheckpoint {
    Ready,
    AwaitingAssistant,
    AwaitingToolResult,
    AwaitingMoreEvidence,
    AwaitingRetryConsent,
    DraftReady,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryEvidenceResolution {
    DeterministicDraftAvailable,
    AssistantRecommended,
    MoreEvidenceRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "resolution", rename_all = "snake_case", deny_unknown_fields)]
pub enum DiscoveryUnknownOutcomeResolution {
    ConfirmedNoEffect,
    ConfirmedCommitCompleted { connection_id: ProviderConnectionId },
    ConfirmedCompensated,
    ManuallyReconciledAsFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryFailure {
    /// Stable non-sensitive diagnostic code.
    pub code: String,
    /// Localization key, never a raw provider response or request dump.
    pub message_key: String,
    pub recoverable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryApprovalBinding {
    pub approval_id: DiscoveryApprovalId,
    pub grant_sha256: String,
}

impl DiscoveryApprovalBinding {
    pub fn validate(&self) -> Result<(), DiscoveryContractError> {
        validate_sha256("approval_binding.grant_sha256", &self.grant_sha256)
    }
}

impl DiscoveryFailure {
    pub fn validate(&self) -> Result<(), DiscoveryContractError> {
        for (field, value) in [
            ("failure.code", self.code.as_str()),
            ("failure.message_key", self.message_key.as_str()),
        ] {
            if value.is_empty()
                || value.len() > 128
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
            {
                return Err(DiscoveryContractError::InvalidField {
                    field: field.to_owned(),
                    reason: "must be a bounded stable identifier".to_owned(),
                });
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "effect", rename_all = "snake_case", deny_unknown_fields)]
pub enum DiscoveryEffect {
    None,
    ResolveKnownProvider,
    FetchDocuments,
    ExtractEvidence,
    BuildDeterministicManifestDraft,
    BuildAssistantManifestDraft {
        approval: DiscoveryApprovalBinding,
    },
    ValidateManifest,
    ListModels,
    ProbeCapabilities {
        approval: DiscoveryApprovalBinding,
    },
    RequestCancellation {
        operation: DiscoveryOperationKind,
    },
    CommitAtomically {
        commit_attempt_id: DiscoveryCommitAttemptId,
        plan_sha256: String,
    },
    RunCompensation {
        commit_attempt_id: DiscoveryCommitAttemptId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryTransition {
    pub previous_revision: u64,
    pub session: ProviderDiscoverySession,
    pub event: ProviderDiscoveryEvent,
    pub receipt: DiscoveryActionReceipt,
    /// The caller executes this only after the state/outbox/receipt transaction commits.
    pub effect: DiscoveryEffect,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DiscoveryTransitionError {
    #[error("discovery revision conflict: expected {expected}, current {actual}")]
    RevisionConflict { expected: u64, actual: u64 },
    #[error("action {action} is invalid while discovery is {state:?}")]
    InvalidTransition {
        state: DiscoveryState,
        action: &'static str,
    },
    #[error("unknown external outcome must be reconciled explicitly")]
    UnknownOutcomeRequiresReconciliation,
    #[error("discovery revision exhausted")]
    RevisionExhausted,
    #[error("discovery event sequence exhausted")]
    SequenceExhausted,
    #[error("invalid discovery session: {0}")]
    InvalidSession(DiscoveryContractError),
    #[error("invalid discovery action: {0}")]
    InvalidAction(DiscoveryContractError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderDiscoveryEvent {
    pub version: u32,
    pub id: DiscoveryEventId,
    pub session_id: DiscoverySessionId,
    pub sequence: u64,
    pub session_revision: u64,
    pub state: DiscoveryState,
    pub progress: Option<DiscoveryProgress>,
    pub action_required: Option<DiscoveryActionRequired>,
    pub warning: Option<DiscoveryWarning>,
    pub action_id: DiscoveryActionId,
    /// The redacted stable failure is part of the durable event, not a transient log.
    pub failure: Option<DiscoveryFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryProgress {
    pub phase: DiscoveryProgressPhase,
    pub completed: u32,
    pub total: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryProgressPhase {
    ProviderCandidates,
    Documents,
    Evidence,
    Models,
    Probes,
}

fn progress_for_action(action: &ProviderDiscoveryAction) -> Option<DiscoveryProgress> {
    match action {
        ProviderDiscoveryAction::KnownProviderCandidatesResolved { candidate_count } => {
            Some(DiscoveryProgress {
                phase: DiscoveryProgressPhase::ProviderCandidates,
                completed: *candidate_count,
                total: Some(*candidate_count),
            })
        }
        ProviderDiscoveryAction::DocumentsFetched { evidence_count } => Some(DiscoveryProgress {
            phase: DiscoveryProgressPhase::Documents,
            completed: *evidence_count,
            total: None,
        }),
        ProviderDiscoveryAction::ModelsListed { model_count, .. } => Some(DiscoveryProgress {
            phase: DiscoveryProgressPhase::Models,
            completed: *model_count,
            total: Some(*model_count),
        }),
        ProviderDiscoveryAction::ProbesCompleted => Some(DiscoveryProgress {
            phase: DiscoveryProgressPhase::Probes,
            completed: 1,
            total: Some(1),
        }),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DiscoveryActionRequired {
    SelectTemplate,
    SupplyMoreEvidence,
    ApproveAssistant,
    ApproveCredentialOrigin,
    ApproveProbes,
    Review,
    RestartInterrupted { operation: DiscoveryOperationKind },
    ReconcileUnknownOutcome { operation: DiscoveryOperationKind },
}

fn action_required_for(session: &ProviderDiscoverySession) -> Option<DiscoveryActionRequired> {
    match session.state {
        DiscoveryState::AwaitingTemplateSelection => Some(DiscoveryActionRequired::SelectTemplate),
        DiscoveryState::AwaitingMoreEvidence => Some(DiscoveryActionRequired::SupplyMoreEvidence),
        DiscoveryState::AwaitingAssistantConsent => Some(DiscoveryActionRequired::ApproveAssistant),
        DiscoveryState::AwaitingCredentialOriginApproval => {
            Some(DiscoveryActionRequired::ApproveCredentialOrigin)
        }
        DiscoveryState::AwaitingProbeConsent => Some(DiscoveryActionRequired::ApproveProbes),
        DiscoveryState::AwaitingReview => Some(DiscoveryActionRequired::Review),
        DiscoveryState::Interrupted => session.recovery.as_ref().map(|checkpoint| {
            DiscoveryActionRequired::RestartInterrupted {
                operation: checkpoint.operation,
            }
        }),
        DiscoveryState::UnknownOutcome => session
            .unknown_operation
            .map(|operation| DiscoveryActionRequired::ReconcileUnknownOutcome { operation }),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryWarning {
    AssistantDeclined,
    ProbesSkipped,
    CompensationRequired,
    ExplicitRestartRequired,
    UnknownExternalOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryActionReceiptOutcome {
    Applied,
    Rejected,
    OutcomeUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryActionReceipt {
    pub action_id: DiscoveryActionId,
    pub session_id: DiscoverySessionId,
    pub action_kind: String,
    pub request_sha256: String,
    pub expected_revision: u64,
    pub resulting_revision: u64,
    pub event_sequence: u64,
    pub outcome: DiscoveryActionReceiptOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryCandidateKind {
    ProviderTemplate,
    ApiOrigin,
    OfficialDocument,
    ModelRoute,
    ManifestDraft,
}

/// A bounded, non-secret candidate summary. Full extracted evidence stays in the
/// evidence table and is referred to by ID and hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DiscoveryCandidateSummary {
    ProviderTemplate {
        template_id: ProviderTemplateId,
        template_version: u32,
    },
    ApiOrigin {
        origin: CanonicalOrigin,
    },
    OfficialDocument {
        url: HttpUrl,
        content_sha256: String,
    },
    ModelRoute {
        model_id: String,
    },
    ManifestDraft {
        schema_version: u32,
        manifest_sha256: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryCandidate {
    pub id: DiscoveryCandidateId,
    pub session_id: DiscoverySessionId,
    pub summary: DiscoveryCandidateSummary,
    #[serde(default)]
    pub evidence_ids: Vec<EvidenceId>,
    pub created_at: DateTime<Utc>,
}

impl DiscoveryCandidate {
    pub fn validate(&self) -> Result<(), DiscoveryContractError> {
        if self.evidence_ids.len() > 4_096 {
            return Err(DiscoveryContractError::InvalidField {
                field: "candidate.evidence_ids".to_owned(),
                reason: "exceeds the 4096 evidence-reference limit".to_owned(),
            });
        }
        match &self.summary {
            DiscoveryCandidateSummary::ProviderTemplate {
                template_version, ..
            } if *template_version == 0 => Err(DiscoveryContractError::InvalidField {
                field: "candidate.template_version".to_owned(),
                reason: "must be positive".to_owned(),
            }),
            DiscoveryCandidateSummary::OfficialDocument {
                url,
                content_sha256,
            } => {
                validate_redacted_url("candidate.url", url)?;
                validate_sha256("candidate.content_sha256", content_sha256)
            }
            DiscoveryCandidateSummary::ModelRoute { model_id }
                if model_id.is_empty()
                    || model_id.len() > 512
                    || model_id.chars().any(char::is_control) =>
            {
                Err(DiscoveryContractError::InvalidField {
                    field: "candidate.model_id".to_owned(),
                    reason: "must be a bounded non-empty model identifier".to_owned(),
                })
            }
            DiscoveryCandidateSummary::ManifestDraft {
                schema_version,
                manifest_sha256,
            } => {
                if *schema_version == 0 {
                    return Err(DiscoveryContractError::InvalidField {
                        field: "candidate.schema_version".to_owned(),
                        reason: "must be positive".to_owned(),
                    });
                }
                validate_sha256("candidate.manifest_sha256", manifest_sha256)
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryApprovalDecision {
    Approved,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DiscoveryApprovalGrant {
    TemplateSelection {
        candidate_id: DiscoveryCandidateId,
    },
    AssistantConsent {
        assistant_route_id: ModelRouteId,
        evidence_ids: Vec<EvidenceId>,
        allowed_document_origins: Vec<CanonicalOrigin>,
        max_calls: u32,
        max_input_tokens: u32,
        max_output_tokens: u32,
        max_tool_calls: u32,
        max_retries: u32,
        max_cost_micro_units: u64,
    },
    CredentialOrigin {
        origin: CanonicalOrigin,
        auth_binding: AuthBinding,
        manifest_sha256: String,
    },
    CapabilityProbe {
        model_route_ids: Vec<ModelRouteId>,
        budget: DiscoveryProbeBudget,
    },
    Review {
        review_sha256: String,
        graph_sha256: String,
    },
    UnknownOutcomeResolution {
        operation: DiscoveryOperationKind,
        resolution: DiscoveryUnknownOutcomeResolution,
    },
}

/// Exact per-request limits and aggregate request count authorized for one
/// capability-probe operation.
///
/// The operation-wide upper bound for tokens, cost, and duration is the
/// corresponding per-request value multiplied by `max_requests`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryProbeBudget {
    pub max_requests: u32,
    pub max_total_tokens_per_request: u64,
    pub max_output_tokens_per_request: u64,
    pub max_cost_micro_usd_per_request: u64,
    pub max_duration_millis_per_request: u64,
    pub max_calls_per_request: u32,
}

impl DiscoveryProbeBudget {
    pub const PROBES_PER_ROUTE: u32 = 5;
    pub const TOTAL_TOKENS_PER_REQUEST: u64 = 512;
    pub const OUTPUT_TOKENS_PER_REQUEST: u64 = 32;
    pub const COST_MICRO_USD_PER_REQUEST: u64 = 100_000;
    pub const DURATION_MILLIS_PER_REQUEST: u64 = 30_000;
    pub const CALLS_PER_REQUEST: u32 = 1;

    pub fn standard_for_route_count(route_count: usize) -> Result<Self, DiscoveryContractError> {
        Self::standard_for_plan(route_count, Self::PROBES_PER_ROUTE as usize)
    }

    pub fn standard_for_plan(
        route_count: usize,
        probes_per_route: usize,
    ) -> Result<Self, DiscoveryContractError> {
        if probes_per_route != Self::PROBES_PER_ROUTE as usize {
            return Err(DiscoveryContractError::InvalidField {
                field: "approval.capability_probe.probe_plan".to_owned(),
                reason: "probe plan does not match the supported capability set".to_owned(),
            });
        }
        let route_count =
            u32::try_from(route_count).map_err(|_| DiscoveryContractError::InvalidField {
                field: "approval.capability_probe.routes".to_owned(),
                reason: "route count exceeds the supported range".to_owned(),
            })?;
        let probes_per_route =
            u32::try_from(probes_per_route).map_err(|_| DiscoveryContractError::InvalidField {
                field: "approval.capability_probe.probe_plan".to_owned(),
                reason: "probe count exceeds the supported range".to_owned(),
            })?;
        let max_requests = route_count
            .checked_mul(probes_per_route)
            .filter(|value| *value > 0)
            .ok_or_else(|| DiscoveryContractError::InvalidField {
                field: "approval.capability_probe.max_requests".to_owned(),
                reason: "request count exceeds the supported range".to_owned(),
            })?;
        let budget = Self {
            max_requests,
            max_total_tokens_per_request: Self::TOTAL_TOKENS_PER_REQUEST,
            max_output_tokens_per_request: Self::OUTPUT_TOKENS_PER_REQUEST,
            max_cost_micro_usd_per_request: Self::COST_MICRO_USD_PER_REQUEST,
            max_duration_millis_per_request: Self::DURATION_MILLIS_PER_REQUEST,
            max_calls_per_request: Self::CALLS_PER_REQUEST,
        };
        budget.validate()?;
        Ok(budget)
    }

    pub fn validate(self) -> Result<(), DiscoveryContractError> {
        if self.max_requests == 0
            || self.max_requests > 10_000
            || self.max_total_tokens_per_request == 0
            || self.max_total_tokens_per_request > 4_096
            || self.max_output_tokens_per_request == 0
            || self.max_output_tokens_per_request > 1_024
            || self.max_output_tokens_per_request > self.max_total_tokens_per_request
            || self.max_cost_micro_usd_per_request > 100_000
            || self.max_duration_millis_per_request == 0
            || self.max_duration_millis_per_request > 60_000
            || self.max_calls_per_request != 1
        {
            return Err(DiscoveryContractError::InvalidField {
                field: "approval.capability_probe.budget".to_owned(),
                reason:
                    "requires bounded requests, tokens, cost, duration, and one call per request"
                        .to_owned(),
            });
        }
        for (field, value) in [
            (
                "approval.capability_probe.aggregate_total_tokens",
                self.max_total_tokens_per_request,
            ),
            (
                "approval.capability_probe.aggregate_output_tokens",
                self.max_output_tokens_per_request,
            ),
            (
                "approval.capability_probe.aggregate_cost_micro_usd",
                self.max_cost_micro_usd_per_request,
            ),
            (
                "approval.capability_probe.aggregate_duration_millis",
                self.max_duration_millis_per_request,
            ),
        ] {
            value
                .checked_mul(u64::from(self.max_requests))
                .ok_or_else(|| DiscoveryContractError::InvalidField {
                    field: field.to_owned(),
                    reason: "aggregate budget overflows its durable integer representation"
                        .to_owned(),
                })?;
        }
        Ok(())
    }
}

impl DiscoveryApprovalGrant {
    pub fn validate(&self) -> Result<(), DiscoveryContractError> {
        match self {
            Self::AssistantConsent {
                evidence_ids,
                allowed_document_origins,
                max_calls,
                max_input_tokens,
                max_output_tokens,
                max_tool_calls,
                max_retries,
                max_cost_micro_units,
                ..
            } => {
                if evidence_ids.is_empty()
                    || evidence_ids.len() > 128
                    || allowed_document_origins.is_empty()
                    || allowed_document_origins.len() > 32
                    || *max_calls == 0
                    || *max_calls > 64
                    || *max_input_tokens == 0
                    || *max_output_tokens == 0
                    || *max_tool_calls == 0
                    || *max_tool_calls > 32
                    || *max_retries > 3
                    || *max_cost_micro_units == 0
                    || *max_cost_micro_units > 100_000_000
                {
                    return Err(DiscoveryContractError::InvalidField {
                        field: "approval.assistant_consent".to_owned(),
                        reason: "requires bounded evidence, origins, calls, tokens, tools, retries, and cost"
                            .to_owned(),
                    });
                }
                let unique_evidence = evidence_ids
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>();
                if unique_evidence.len() != evidence_ids.len()
                    || !evidence_ids.windows(2).all(|pair| pair[0] < pair[1])
                {
                    return Err(DiscoveryContractError::InvalidField {
                        field: "approval.assistant_consent.evidence_ids".to_owned(),
                        reason: "must be unique and sorted".to_owned(),
                    });
                }
                Ok(())
            }
            Self::CredentialOrigin {
                manifest_sha256, ..
            } => validate_sha256("approval.manifest_sha256", manifest_sha256),
            Self::CapabilityProbe {
                model_route_ids,
                budget,
            } => {
                if model_route_ids.is_empty() || model_route_ids.len() > 1_000 {
                    return Err(DiscoveryContractError::InvalidField {
                        field: "approval.capability_probe".to_owned(),
                        reason: "requires a bounded route set".to_owned(),
                    });
                }
                if !model_route_ids.windows(2).all(|pair| pair[0] < pair[1]) {
                    return Err(DiscoveryContractError::InvalidField {
                        field: "approval.capability_probe.model_route_ids".to_owned(),
                        reason: "must be unique and sorted".to_owned(),
                    });
                }
                budget.validate()?;
                let expected =
                    DiscoveryProbeBudget::standard_for_route_count(model_route_ids.len())?;
                if *budget != expected {
                    return Err(DiscoveryContractError::InvalidField {
                        field: "approval.capability_probe.budget".to_owned(),
                        reason: "must equal the standard budget for the approved route set"
                            .to_owned(),
                    });
                }
                Ok(())
            }
            Self::Review {
                review_sha256,
                graph_sha256,
            } => {
                validate_sha256("approval.review_sha256", review_sha256)?;
                validate_sha256("approval.graph_sha256", graph_sha256)
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryApprovalRecord {
    pub id: DiscoveryApprovalId,
    pub session_id: DiscoverySessionId,
    pub session_revision: u64,
    pub decision: DiscoveryApprovalDecision,
    pub grant: DiscoveryApprovalGrant,
    pub created_at: DateTime<Utc>,
}

impl DiscoveryApprovalRecord {
    pub fn validate(&self) -> Result<(), DiscoveryContractError> {
        self.grant.validate()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryReviewChangeKind {
    Add,
    Update,
    Deprecate,
    PreserveMissing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryReviewChange {
    pub kind: DiscoveryReviewChangeKind,
    pub target_kind: String,
    pub target_id: String,
    pub summary_key: String,
    #[serde(default)]
    pub evidence_ids: Vec<EvidenceId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryReviewDiff {
    pub sha256: String,
    /// Digest of the exact sanitized provider graph described by this review.
    pub graph_sha256: String,
    pub changes: Vec<DiscoveryReviewChange>,
    pub unresolved_question_count: u32,
    pub warning_count: u32,
}

impl DiscoveryReviewDiff {
    pub fn new(
        graph_sha256: String,
        changes: Vec<DiscoveryReviewChange>,
        unresolved_question_count: u32,
        warning_count: u32,
    ) -> Result<Self, DiscoveryContractError> {
        validate_sha256("review.graph_sha256", &graph_sha256)?;
        if changes.len() > 4_096 {
            return Err(DiscoveryContractError::InvalidField {
                field: "review.changes".to_owned(),
                reason: "exceeds the 4096 change limit".to_owned(),
            });
        }
        let sha256 = discovery_review_sha256(
            &graph_sha256,
            &changes,
            unresolved_question_count,
            warning_count,
        )?;
        Ok(Self {
            sha256,
            graph_sha256,
            changes,
            unresolved_question_count,
            warning_count,
        })
    }

    pub fn computed_sha256(&self) -> Result<String, DiscoveryContractError> {
        discovery_review_sha256(
            &self.graph_sha256,
            &self.changes,
            self.unresolved_question_count,
            self.warning_count,
        )
    }

    pub fn validate(&self) -> Result<(), DiscoveryContractError> {
        validate_sha256("review.sha256", &self.sha256)?;
        validate_sha256("review.graph_sha256", &self.graph_sha256)?;
        if self.changes.len() > 4_096 {
            return Err(DiscoveryContractError::InvalidField {
                field: "review.changes".to_owned(),
                reason: "exceeds the 4096 change limit".to_owned(),
            });
        }
        if self.computed_sha256()? != self.sha256 {
            return Err(DiscoveryContractError::InvalidField {
                field: "review.sha256".to_owned(),
                reason: "does not match the canonical review payload".to_owned(),
            });
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct DiscoveryReviewDigestPayload<'a> {
    graph_sha256: &'a str,
    changes: &'a [DiscoveryReviewChange],
    unresolved_question_count: u32,
    warning_count: u32,
}

fn discovery_review_sha256(
    graph_sha256: &str,
    changes: &[DiscoveryReviewChange],
    unresolved_question_count: u32,
    warning_count: u32,
) -> Result<String, DiscoveryContractError> {
    let payload = DiscoveryReviewDigestPayload {
        graph_sha256,
        changes,
        unresolved_question_count,
        warning_count,
    };
    let canonical =
        serde_json::to_vec(&payload).map_err(|error| DiscoveryContractError::InvalidField {
            field: "review".to_owned(),
            reason: format!("could not encode canonical review payload: {error}"),
        })?;
    Ok(hex::encode(Sha256::digest(canonical)))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryCommitPlan {
    pub attempt_id: DiscoveryCommitAttemptId,
    pub session_id: DiscoverySessionId,
    pub expected_revision: u64,
    pub manifest_sha256: String,
    /// SHA-256 of the canonical sanitized template/connection/routes/
    /// observations/presets graph approved by the user.
    pub graph_sha256: String,
    pub template_id: ProviderTemplateId,
    pub template_version: u32,
    pub connection_id: ProviderConnectionId,
    pub model_route_ids: Vec<ModelRouteId>,
    pub credential_ref: Option<CredentialRef>,
    pub credential_approval_id: Option<DiscoveryApprovalId>,
    pub review_sha256: String,
    /// Exact application selection that must be restored if commit
    /// compensation reaches the settings phase.
    pub previous_selection: DiscoveryPreviousSelection,
}

impl DiscoveryCommitPlan {
    pub fn validate(&self) -> Result<(), DiscoveryContractError> {
        validate_sha256("commit.manifest_sha256", &self.manifest_sha256)?;
        validate_sha256("commit.graph_sha256", &self.graph_sha256)?;
        validate_sha256("commit.review_sha256", &self.review_sha256)?;
        if self.template_version == 0 {
            return Err(DiscoveryContractError::InvalidField {
                field: "commit.template_version".to_owned(),
                reason: "must be positive".to_owned(),
            });
        }
        if self.model_route_ids.is_empty() {
            return Err(DiscoveryContractError::InvalidField {
                field: "commit.model_route_ids".to_owned(),
                reason: "must contain at least one route".to_owned(),
            });
        }
        if self.credential_ref.is_some() != self.credential_approval_id.is_some() {
            return Err(DiscoveryContractError::InvalidField {
                field: "commit.credential_approval_id".to_owned(),
                reason: "must be present exactly when a credential reference is linked".to_owned(),
            });
        }
        if self
            .credential_ref
            .as_ref()
            .is_some_and(|reference| reference.as_str() != self.connection_id.as_str())
        {
            return Err(DiscoveryContractError::InvalidField {
                field: "commit.credential_ref".to_owned(),
                reason: "must use the committed connection identifier as its native slot"
                    .to_owned(),
            });
        }
        self.previous_selection.validate()?;
        Ok(())
    }
}

/// Immutable application selection captured before a provider graph commit.
///
/// Keeping the route and preset in one tagged value prevents partial snapshots
/// such as a route without its generation preset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DiscoveryPreviousSelection {
    None,
    RouteAndPreset {
        /// Retained legacy profile identity, when the selected route is the
        /// profile's current migrated sibling rather than the profile id.
        ///
        /// Older durable plans omit this field; `None` preserves their exact
        /// serialized representation and the legacy route-id inference path.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selected_provider_profile_id: Option<String>,
        model_route_id: ModelRouteId,
        generation_preset_id: GenerationPresetId,
    },
}

impl DiscoveryPreviousSelection {
    pub fn validate(&self) -> Result<(), DiscoveryContractError> {
        match self {
            Self::None => Ok(()),
            Self::RouteAndPreset {
                selected_provider_profile_id,
                model_route_id,
                generation_preset_id,
            } => {
                if let Some(profile_id) = selected_provider_profile_id {
                    validate_opaque_reference(
                        "previous_selection.selected_provider_profile_id",
                        profile_id,
                    )?;
                }
                validate_opaque_reference(
                    "previous_selection.model_route_id",
                    model_route_id.as_str(),
                )?;
                validate_opaque_reference(
                    "previous_selection.generation_preset_id",
                    generation_preset_id.as_str(),
                )
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryCommitPhase {
    Prepared,
    DatabaseApplied,
    CredentialReferenceApplied,
    Completed,
    CompensationRequired,
    Compensating,
    Compensated,
    OutcomeUnknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryCompensationKind {
    RemoveCredentialSlot,
    RemoveConnectionGraph,
    RestorePreviousSelection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryCompensationStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    OutcomeUnknown,
}

/// Typed target of an immutable compensation step.
///
/// Each payload is derivable from the commit plan. Storage validates that
/// relationship before persisting the recipe, so changing a step cannot retarget
/// a previously approved commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DiscoveryCompensationTarget {
    RemoveCredentialSlot {
        connection_id: ProviderConnectionId,
        credential_ref: CredentialRef,
    },
    RemoveConnectionGraph {
        connection_id: ProviderConnectionId,
    },
    RestorePreviousSelection {
        previous_selection: DiscoveryPreviousSelection,
    },
}

impl DiscoveryCompensationTarget {
    pub const fn kind(&self) -> DiscoveryCompensationKind {
        match self {
            Self::RemoveCredentialSlot { .. } => DiscoveryCompensationKind::RemoveCredentialSlot,
            Self::RemoveConnectionGraph { .. } => DiscoveryCompensationKind::RemoveConnectionGraph,
            Self::RestorePreviousSelection { .. } => {
                DiscoveryCompensationKind::RestorePreviousSelection
            }
        }
    }

    pub fn validate_against(
        &self,
        plan: &DiscoveryCommitPlan,
    ) -> Result<(), DiscoveryContractError> {
        match self {
            Self::RemoveCredentialSlot {
                connection_id,
                credential_ref,
            } => {
                if connection_id != &plan.connection_id
                    || plan.credential_ref.as_ref() != Some(credential_ref)
                {
                    return Err(DiscoveryContractError::InvalidField {
                        field: "compensation.target".to_owned(),
                        reason:
                            "credential removal must match the approved connection and credential reference"
                                .to_owned(),
                    });
                }
                validate_opaque_reference(
                    "compensation.target.credential_ref",
                    credential_ref.as_str(),
                )
            }
            Self::RemoveConnectionGraph { connection_id } => {
                if connection_id != &plan.connection_id {
                    return Err(DiscoveryContractError::InvalidField {
                        field: "compensation.target.connection_id".to_owned(),
                        reason: "graph removal must match the approved connection".to_owned(),
                    });
                }
                validate_opaque_reference(
                    "compensation.target.connection_id",
                    connection_id.as_str(),
                )
            }
            Self::RestorePreviousSelection { previous_selection } => {
                if previous_selection != &plan.previous_selection {
                    return Err(DiscoveryContractError::InvalidField {
                        field: "compensation.target.previous_selection".to_owned(),
                        reason: "selection restore must match the approved pre-commit snapshot"
                            .to_owned(),
                    });
                }
                previous_selection.validate()
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryCompensationStep {
    pub action_id: DiscoveryActionId,
    pub ordinal: u32,
    pub kind: DiscoveryCompensationKind,
    pub target: DiscoveryCompensationTarget,
    pub status: DiscoveryCompensationStatus,
}

impl DiscoveryCompensationStep {
    pub fn validate_against(
        &self,
        plan: &DiscoveryCommitPlan,
    ) -> Result<(), DiscoveryContractError> {
        if self.kind != self.target.kind() {
            return Err(DiscoveryContractError::InvalidField {
                field: "compensation.kind".to_owned(),
                reason: "must match the typed compensation target".to_owned(),
            });
        }
        self.target.validate_against(plan)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryAuditKind {
    SessionCreated,
    TransitionApplied,
    CandidateRecorded,
    ApprovalRecorded,
    OperationStarted,
    OperationInterrupted,
    CommitPrepared,
    CompensationStarted,
    UnknownOutcomeReconciled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryAuditRecord {
    pub session_id: DiscoverySessionId,
    pub sequence: u64,
    pub revision: u64,
    pub kind: DiscoveryAuditKind,
    pub action_id: Option<DiscoveryActionId>,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn input() -> SanitizedDiscoveryInput {
        SanitizedDiscoveryInput {
            connection_id: ProviderConnectionId::from("connection-1"),
            display_name: "My Provider".to_owned(),
            site_url: HttpUrl::parse("https://provider.example/").unwrap(),
            docs_url: Some(HttpUrl::parse("https://docs.provider.example/api").unwrap()),
            credential_ref: Some(CredentialRef("connection-1".to_owned())),
            preferred_assistant: Some(ModelRouteId::from("assistant-route")),
            connection_options: ProviderDiscoveryConnectionOptions::default(),
            supplied_evidence_ids: vec![EvidenceId::from("evidence-1")],
        }
    }

    fn session() -> ProviderDiscoverySession {
        ProviderDiscoverySession::new(DiscoverySessionId::from("session-1"), input()).unwrap()
    }

    fn commit_plan() -> DiscoveryCommitPlan {
        DiscoveryCommitPlan {
            attempt_id: DiscoveryCommitAttemptId::parse("attempt-1").unwrap(),
            session_id: DiscoverySessionId::from("session-1"),
            expected_revision: 7,
            manifest_sha256: HASH.to_owned(),
            graph_sha256: HASH.to_owned(),
            template_id: ProviderTemplateId::from("template-1"),
            template_version: 1,
            connection_id: ProviderConnectionId::from("connection-1"),
            model_route_ids: vec![ModelRouteId::from("route-1")],
            credential_ref: Some(CredentialRef("connection-1".to_owned())),
            credential_approval_id: Some(DiscoveryApprovalId::parse("approval-1").unwrap()),
            review_sha256: HASH.to_owned(),
            previous_selection: DiscoveryPreviousSelection::RouteAndPreset {
                selected_provider_profile_id: None,
                model_route_id: ModelRouteId::from("prior-route"),
                generation_preset_id: GenerationPresetId::from("prior-preset"),
            },
        }
    }

    fn envelope(
        session: &ProviderDiscoverySession,
        action: ProviderDiscoveryAction,
    ) -> DiscoveryActionEnvelope {
        DiscoveryActionEnvelope {
            id: DiscoveryActionId::new(),
            expected_revision: session.revision,
            request_sha256: HASH.to_owned(),
            action,
        }
    }

    #[test]
    fn deterministic_flow_reaches_ready_with_monotonic_revision_and_sequence() {
        let mut current = session();
        let actions = [
            ProviderDiscoveryAction::Begin,
            ProviderDiscoveryAction::KnownProviderCandidatesResolved { candidate_count: 0 },
            ProviderDiscoveryAction::DocumentsFetched { evidence_count: 2 },
            ProviderDiscoveryAction::EvidenceExtracted {
                resolution: DiscoveryEvidenceResolution::DeterministicDraftAvailable,
            },
            ProviderDiscoveryAction::ManifestDraftBuilt {
                manifest_sha256: HASH.to_owned(),
            },
            ProviderDiscoveryAction::ManifestValidated {
                manifest_sha256: HASH.to_owned(),
                credential_origin_approval_required: false,
            },
            ProviderDiscoveryAction::ModelsListed {
                model_count: 3,
                probe_candidate_count: 0,
            },
            ProviderDiscoveryAction::ApproveReview {
                approval_id: DiscoveryApprovalId::new(),
                commit_attempt_id: DiscoveryCommitAttemptId::new(),
                commit_plan_sha256: HASH.to_owned(),
                graph_sha256: HASH.to_owned(),
            },
            ProviderDiscoveryAction::CommitSucceeded {
                connection_id: ProviderConnectionId::from("connection-1"),
            },
        ];

        for (index, action) in actions.into_iter().enumerate() {
            let transition = current.apply(&envelope(&current, action)).unwrap();
            assert_eq!(transition.previous_revision, index as u64);
            assert_eq!(transition.session.revision, index as u64 + 1);
            assert_eq!(transition.event.sequence, index as u64 + 1);
            assert_eq!(
                transition.session.next_event_sequence,
                transition.event.sequence + 1
            );
            assert_eq!(
                transition.receipt.resulting_revision,
                transition.session.revision
            );
            current = transition.session;
        }

        assert_eq!(current.state, DiscoveryState::Ready);
        assert_eq!(
            current.committed_connection_id,
            Some(ProviderConnectionId::from("connection-1"))
        );
    }

    #[test]
    fn local_draft_and_billable_assistant_draft_are_distinct_effects() {
        let extracting = valid_session_in(DiscoveryState::ExtractingEvidence);
        let local = extracting
            .apply(&envelope(
                &extracting,
                ProviderDiscoveryAction::EvidenceExtracted {
                    resolution: DiscoveryEvidenceResolution::DeterministicDraftAvailable,
                },
            ))
            .unwrap();
        assert_eq!(
            local.session.state,
            DiscoveryState::BuildingDeterministicManifestDraft
        );
        assert_eq!(
            local.effect,
            DiscoveryEffect::BuildDeterministicManifestDraft
        );
        assert_eq!(local.session.active_effect_approval, None);

        let awaiting = valid_session_in(DiscoveryState::AwaitingAssistantConsent);
        let approval_id = DiscoveryApprovalId::new();
        let assistant = awaiting
            .apply(&envelope(
                &awaiting,
                ProviderDiscoveryAction::ApproveAssistant {
                    approval_id: approval_id.clone(),
                    approval_grant_sha256: HASH.to_owned(),
                },
            ))
            .unwrap();
        assert_eq!(
            assistant.session.state,
            DiscoveryState::BuildingAssistantManifestDraft
        );
        assert_eq!(
            assistant.effect,
            DiscoveryEffect::BuildAssistantManifestDraft {
                approval: DiscoveryApprovalBinding {
                    approval_id,
                    grant_sha256: HASH.to_owned(),
                }
            }
        );
    }

    #[test]
    fn review_hash_is_recomputed_from_the_canonical_payload() {
        let mut review = DiscoveryReviewDiff::new(HASH.to_owned(), Vec::new(), 0, 0).unwrap();
        assert_eq!(
            review.sha256,
            "abb8bda2360b026a390418abb0222c0f098e0e3d59579968011dbc9036454125"
        );
        review.validate().unwrap();

        review.warning_count = 1;
        assert!(review.validate().is_err());
    }

    #[test]
    fn typed_compensation_targets_are_bound_to_the_approved_commit_plan() {
        let plan = commit_plan();
        plan.validate().unwrap();

        let credential = DiscoveryCompensationStep {
            action_id: DiscoveryActionId::parse("remove-credential").unwrap(),
            ordinal: 2,
            kind: DiscoveryCompensationKind::RemoveCredentialSlot,
            target: DiscoveryCompensationTarget::RemoveCredentialSlot {
                connection_id: plan.connection_id.clone(),
                credential_ref: plan.credential_ref.clone().unwrap(),
            },
            status: DiscoveryCompensationStatus::Pending,
        };
        let graph = DiscoveryCompensationStep {
            action_id: DiscoveryActionId::parse("remove-graph").unwrap(),
            ordinal: 1,
            kind: DiscoveryCompensationKind::RemoveConnectionGraph,
            target: DiscoveryCompensationTarget::RemoveConnectionGraph {
                connection_id: plan.connection_id.clone(),
            },
            status: DiscoveryCompensationStatus::Pending,
        };
        let selection = DiscoveryCompensationStep {
            action_id: DiscoveryActionId::parse("restore-selection").unwrap(),
            ordinal: 0,
            kind: DiscoveryCompensationKind::RestorePreviousSelection,
            target: DiscoveryCompensationTarget::RestorePreviousSelection {
                previous_selection: plan.previous_selection.clone(),
            },
            status: DiscoveryCompensationStatus::Pending,
        };

        credential.validate_against(&plan).unwrap();
        graph.validate_against(&plan).unwrap();
        selection.validate_against(&plan).unwrap();

        let retargeted = DiscoveryCompensationStep {
            target: DiscoveryCompensationTarget::RemoveConnectionGraph {
                connection_id: ProviderConnectionId::from("different-connection"),
            },
            ..graph.clone()
        };
        assert!(retargeted.validate_against(&plan).is_err());

        let mismatched_kind = DiscoveryCompensationStep {
            kind: DiscoveryCompensationKind::RemoveCredentialSlot,
            ..graph
        };
        assert!(mismatched_kind.validate_against(&plan).is_err());

        let retargeted_selection = DiscoveryCompensationStep {
            target: DiscoveryCompensationTarget::RestorePreviousSelection {
                previous_selection: DiscoveryPreviousSelection::None,
            },
            ..selection
        };
        assert!(retargeted_selection.validate_against(&plan).is_err());
    }

    #[test]
    fn legacy_previous_selection_json_keeps_its_exact_hash_input() {
        let legacy = r#"{"kind":"route_and_preset","model_route_id":"prior-route","generation_preset_id":"prior-preset"}"#;
        let selection = serde_json::from_str::<DiscoveryPreviousSelection>(legacy)
            .expect("legacy previous selection");

        assert_eq!(
            selection,
            DiscoveryPreviousSelection::RouteAndPreset {
                selected_provider_profile_id: None,
                model_route_id: ModelRouteId::from("prior-route"),
                generation_preset_id: GenerationPresetId::from("prior-preset"),
            }
        );
        assert_eq!(
            serde_json::to_string(&selection).expect("previous selection JSON"),
            legacy,
            "adding the optional profile identity must not change existing durable plan hashes",
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn fresh_evidence_origin_follows_the_session_network_authority() {
        let mut current = session();
        current.state = DiscoveryState::AwaitingMoreEvidence;
        let public_document_origin = CanonicalOrigin::parse("https://docs.example").unwrap();
        let transition = current
            .apply(&envelope(
                &current,
                ProviderDiscoveryAction::SupplyFreshEvidence {
                    evidence_ids: vec![EvidenceId::from("fresh-evidence")],
                    source: DiscoveryFreshEvidenceSource::DocumentUrl {
                        origin: public_document_origin,
                    },
                },
            ))
            .unwrap();
        assert_eq!(transition.session.state, DiscoveryState::ExtractingEvidence);
        assert_eq!(transition.effect, DiscoveryEffect::ExtractEvidence);

        current
            .apply(&envelope(
                &current,
                ProviderDiscoveryAction::SupplyFreshEvidence {
                    evidence_ids: vec![EvidenceId::from("fresh-evidence")],
                    source: DiscoveryFreshEvidenceSource::SanitizedCurl {
                        origin: CanonicalOrigin::parse("https://api.example").unwrap(),
                    },
                },
            ))
            .unwrap();

        let empty_ids = current
            .apply(&envelope(
                &current,
                ProviderDiscoveryAction::SupplyFreshEvidence {
                    evidence_ids: Vec::new(),
                    source: DiscoveryFreshEvidenceSource::DocumentUrl {
                        origin: CanonicalOrigin::parse("https://docs.example").unwrap(),
                    },
                },
            ))
            .unwrap_err();
        assert!(matches!(
            empty_ids,
            DiscoveryTransitionError::InvalidAction(_)
        ));

        let mut loopback = current.clone();
        loopback.input.site_url = HttpUrl::parse("http://127.0.0.1:11434/").unwrap();
        loopback.input.connection_options.network_mode = ProviderNetworkMode::LocalLoopback;
        loopback
            .apply(&envelope(
                &loopback,
                ProviderDiscoveryAction::SupplyFreshEvidence {
                    evidence_ids: vec![EvidenceId::from("loopback-evidence")],
                    source: DiscoveryFreshEvidenceSource::DocumentUrl {
                        origin: CanonicalOrigin::parse("http://127.0.0.1:11434").unwrap(),
                    },
                },
            ))
            .unwrap();
        assert!(
            loopback
                .apply(&envelope(
                    &loopback,
                    ProviderDiscoveryAction::SupplyFreshEvidence {
                        evidence_ids: vec![EvidenceId::from("other-loopback-evidence")],
                        source: DiscoveryFreshEvidenceSource::DocumentUrl {
                            origin: CanonicalOrigin::parse("http://127.0.0.1:11435").unwrap(),
                        },
                    },
                ))
                .is_err()
        );

        let mut lan = current;
        let approved_origin = CanonicalOrigin::parse("http://models.lan:8080").unwrap();
        lan.input.connection_options.network_mode = ProviderNetworkMode::ApprovedLocalNetwork;
        lan.input.connection_options.local_network_approval = Some(ProviderLocalNetworkApproval {
            origin: approved_origin.clone(),
            addresses: vec!["192.168.10.20".parse().unwrap()],
        });
        lan.apply(&envelope(
            &lan,
            ProviderDiscoveryAction::SupplyFreshEvidence {
                evidence_ids: vec![EvidenceId::from("lan-evidence")],
                source: DiscoveryFreshEvidenceSource::SanitizedCurl {
                    origin: approved_origin,
                },
            },
        ))
        .unwrap();
        assert!(
            lan.apply(&envelope(
                &lan,
                ProviderDiscoveryAction::SupplyFreshEvidence {
                    evidence_ids: vec![EvidenceId::from("lan-document-evidence")],
                    source: DiscoveryFreshEvidenceSource::DocumentUrl {
                        origin: CanonicalOrigin::parse("http://models.lan:8080").unwrap(),
                    },
                },
            ))
            .is_err()
        );
        assert!(
            lan.apply(&envelope(
                &lan,
                ProviderDiscoveryAction::SupplyFreshEvidence {
                    evidence_ids: vec![EvidenceId::from("unapproved-lan-evidence")],
                    source: DiscoveryFreshEvidenceSource::SanitizedCurl {
                        origin: CanonicalOrigin::parse("http://other.lan:8080").unwrap(),
                    },
                },
            ))
            .is_err()
        );
    }

    #[test]
    fn capability_probe_grant_binds_every_executed_budget_dimension() {
        let budget = DiscoveryProbeBudget::standard_for_route_count(2).unwrap();
        assert_eq!(budget.max_requests, 10);
        assert_eq!(budget.max_total_tokens_per_request, 512);
        assert_eq!(budget.max_output_tokens_per_request, 32);
        assert_eq!(budget.max_cost_micro_usd_per_request, 100_000);
        assert_eq!(budget.max_duration_millis_per_request, 30_000);
        assert_eq!(budget.max_calls_per_request, 1);
        DiscoveryApprovalGrant::CapabilityProbe {
            model_route_ids: vec![ModelRouteId::from("route-1"), ModelRouteId::from("route-2")],
            budget,
        }
        .validate()
        .unwrap();

        let mut changed_cost = budget;
        changed_cost.max_cost_micro_usd_per_request -= 1;
        assert!(
            DiscoveryApprovalGrant::CapabilityProbe {
                model_route_ids: vec![ModelRouteId::from("route-1"), ModelRouteId::from("route-2")],
                budget: changed_cost,
            }
            .validate()
            .is_err()
        );
        let mut changed_duration = budget;
        changed_duration.max_duration_millis_per_request -= 1;
        assert!(
            DiscoveryApprovalGrant::CapabilityProbe {
                model_route_ids: vec![ModelRouteId::from("route-1"), ModelRouteId::from("route-2")],
                budget: changed_duration,
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn billable_approval_binding_survives_unknown_outcome_until_explicit_restart() {
        let awaiting = valid_session_in(DiscoveryState::AwaitingAssistantConsent);
        let effect_approval_id = DiscoveryApprovalId::new();
        let started = awaiting
            .apply(&envelope(
                &awaiting,
                ProviderDiscoveryAction::ApproveAssistant {
                    approval_id: effect_approval_id.clone(),
                    approval_grant_sha256: HASH.to_owned(),
                },
            ))
            .unwrap()
            .session;

        let unknown = started
            .apply(&envelope(
                &started,
                ProviderDiscoveryAction::Interrupt {
                    operation: DiscoveryOperationKind::BuildAssistantManifestDraft,
                    outcome: DiscoveryInterruptionOutcome::ExternalOutcomeUnknown,
                },
            ))
            .unwrap()
            .session;
        assert_eq!(unknown.state, DiscoveryState::UnknownOutcome);
        assert_eq!(
            unknown.active_effect_approval,
            Some(DiscoveryApprovalBinding {
                approval_id: effect_approval_id.clone(),
                grant_sha256: HASH.to_owned(),
            })
        );

        let interrupted = unknown
            .apply(&envelope(
                &unknown,
                ProviderDiscoveryAction::ResolveUnknownOutcome {
                    approval_id: DiscoveryApprovalId::new(),
                    resolution: DiscoveryUnknownOutcomeResolution::ConfirmedNoEffect,
                },
            ))
            .unwrap()
            .session;
        assert_eq!(interrupted.state, DiscoveryState::Interrupted);

        let restarted = interrupted
            .apply(&envelope(
                &interrupted,
                ProviderDiscoveryAction::RestartInterrupted,
            ))
            .unwrap();
        assert_eq!(
            restarted.effect,
            DiscoveryEffect::BuildAssistantManifestDraft {
                approval: DiscoveryApprovalBinding {
                    approval_id: effect_approval_id,
                    grant_sha256: HASH.to_owned(),
                },
            }
        );
    }

    #[test]
    fn stale_revision_is_rejected_before_every_action_kind() {
        let current = session();
        for action in representative_actions() {
            let mut request = envelope(&current, action);
            request.expected_revision = current.revision + 1;
            assert!(matches!(
                current.apply(&request),
                Err(DiscoveryTransitionError::RevisionConflict { .. })
            ));
        }
    }

    #[test]
    fn exhaustive_accepted_transitions_preserve_session_and_event_invariants() {
        for state in DiscoveryState::ALL {
            for action in representative_actions() {
                let current = valid_session_in(state);
                let request = envelope(&current, action);
                if let Ok(transition) = current.apply(&request) {
                    transition.session.validate().unwrap();
                    assert_eq!(transition.session.revision, current.revision + 1);
                    assert_eq!(
                        transition.session.next_event_sequence,
                        current.next_event_sequence + 1
                    );
                    assert_eq!(transition.event.session_id, current.id);
                    assert_eq!(transition.event.state, transition.session.state);
                    assert_eq!(
                        transition.event.session_revision,
                        transition.session.revision
                    );
                    assert_eq!(transition.receipt.event_sequence, transition.event.sequence);
                }
            }
        }
    }

    #[test]
    fn every_nonterminal_nonunknown_state_has_safe_cancel_semantics() {
        for state in DiscoveryState::ALL {
            if state.is_terminal()
                || matches!(
                    state,
                    DiscoveryState::UnknownOutcome | DiscoveryState::Compensating
                )
            {
                continue;
            }
            let mut current = valid_session_in(state);
            let transition = current
                .apply(&envelope(&current, ProviderDiscoveryAction::Cancel))
                .unwrap();
            if let Some(operation) = state.operation() {
                assert_eq!(transition.session.state, state);
                assert!(transition.session.cancellation_pending);
                assert_eq!(
                    transition.effect,
                    DiscoveryEffect::RequestCancellation { operation }
                );
                let settled = transition
                    .session
                    .apply(&envelope(
                        &transition.session,
                        ProviderDiscoveryAction::Interrupt {
                            operation,
                            outcome: DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
                        },
                    ))
                    .unwrap();
                if operation == DiscoveryOperationKind::AtomicCommit {
                    assert_eq!(settled.session.state, DiscoveryState::Interrupted);
                    assert_eq!(
                        settled.session.recovery,
                        Some(DiscoveryRecoveryCheckpoint {
                            interrupted_state: DiscoveryState::Compensating,
                            operation: DiscoveryOperationKind::Compensation,
                        })
                    );
                } else {
                    assert_eq!(settled.session.state, DiscoveryState::Cancelled);
                }
                current = settled.session;
            } else {
                assert_eq!(transition.session.state, DiscoveryState::Cancelled);
                assert!(matches!(transition.effect, DiscoveryEffect::None));
                current = transition.session;
            }
            current.validate().unwrap();
        }
    }

    #[test]
    fn interruption_never_retries_without_a_second_explicit_action() {
        for state in DiscoveryState::ALL {
            let Some(operation) = state.operation() else {
                continue;
            };
            let current = valid_session_in(state);
            let interrupted = current
                .apply(&envelope(
                    &current,
                    ProviderDiscoveryAction::Interrupt {
                        operation,
                        outcome: DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
                    },
                ))
                .unwrap();
            assert_eq!(interrupted.session.state, DiscoveryState::Interrupted);
            assert_eq!(interrupted.effect, DiscoveryEffect::None);
            assert!(matches!(
                interrupted.event.action_required,
                Some(DiscoveryActionRequired::RestartInterrupted { .. })
            ));

            let restarted = interrupted
                .session
                .apply(&envelope(
                    &interrupted.session,
                    ProviderDiscoveryAction::RestartInterrupted,
                ))
                .unwrap();
            assert_eq!(restarted.session.state, state);
            assert!(!matches!(restarted.effect, DiscoveryEffect::None));
        }
    }

    #[test]
    fn unknown_outcome_cannot_be_cancelled_or_restarted() {
        let current = valid_session_in(DiscoveryState::ProbingCapabilities);
        let unknown = current
            .apply(&envelope(
                &current,
                ProviderDiscoveryAction::Interrupt {
                    operation: DiscoveryOperationKind::ProbeCapabilities,
                    outcome: DiscoveryInterruptionOutcome::ExternalOutcomeUnknown,
                },
            ))
            .unwrap()
            .session;
        assert_eq!(unknown.state, DiscoveryState::UnknownOutcome);

        for action in [
            ProviderDiscoveryAction::Cancel,
            ProviderDiscoveryAction::RestartInterrupted,
            ProviderDiscoveryAction::ProbesCompleted,
        ] {
            assert!(unknown.apply(&envelope(&unknown, action)).is_err());
        }

        let reconciled = unknown
            .apply(&envelope(
                &unknown,
                ProviderDiscoveryAction::ResolveUnknownOutcome {
                    approval_id: DiscoveryApprovalId::new(),
                    resolution: DiscoveryUnknownOutcomeResolution::ConfirmedNoEffect,
                },
            ))
            .unwrap();
        assert_eq!(reconciled.session.state, DiscoveryState::Interrupted);
        assert_eq!(reconciled.effect, DiscoveryEffect::None);
    }

    #[test]
    fn validated_manifest_must_match_the_persisted_draft_hash() {
        let mut current = valid_session_in(DiscoveryState::BuildingDeterministicManifestDraft);
        current = current
            .apply(&envelope(
                &current,
                ProviderDiscoveryAction::ManifestDraftBuilt {
                    manifest_sha256: HASH.to_owned(),
                },
            ))
            .unwrap()
            .session;
        assert_eq!(current.state, DiscoveryState::ValidatingManifest);
        assert!(
            current
                .apply(&envelope(
                    &current,
                    ProviderDiscoveryAction::ManifestValidated {
                        manifest_sha256:
                            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                                .to_owned(),
                        credential_origin_approval_required: false,
                    },
                ))
                .is_err()
        );
    }

    #[test]
    fn commit_cancel_waits_for_commit_outcome_then_compensates_before_terminal_cancel() {
        let committing = valid_session_in(DiscoveryState::Committing);
        let cancelling = committing
            .apply(&envelope(&committing, ProviderDiscoveryAction::Cancel))
            .unwrap();
        assert_eq!(cancelling.session.state, DiscoveryState::Committing);
        assert!(cancelling.session.cancellation_pending);
        assert_eq!(
            cancelling.effect,
            DiscoveryEffect::RequestCancellation {
                operation: DiscoveryOperationKind::AtomicCommit
            }
        );

        let compensating = cancelling
            .session
            .apply(&envelope(
                &cancelling.session,
                ProviderDiscoveryAction::CommitSucceeded {
                    connection_id: ProviderConnectionId::from("connection"),
                },
            ))
            .unwrap();
        assert_eq!(compensating.session.state, DiscoveryState::Compensating);
        assert!(matches!(
            compensating.effect,
            DiscoveryEffect::RunCompensation { .. }
        ));

        let cancelled = compensating
            .session
            .apply(&envelope(
                &compensating.session,
                ProviderDiscoveryAction::CompensationSucceeded,
            ))
            .unwrap();
        assert_eq!(cancelled.session.state, DiscoveryState::Cancelled);

        let committing = valid_session_in(DiscoveryState::Committing);
        let cancelling = committing
            .apply(&envelope(&committing, ProviderDiscoveryAction::Cancel))
            .unwrap()
            .session;
        let unstarted = cancelling
            .apply(&envelope(
                &cancelling,
                ProviderDiscoveryAction::Interrupt {
                    operation: DiscoveryOperationKind::AtomicCommit,
                    outcome: DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
                },
            ))
            .unwrap();
        assert_eq!(unstarted.session.state, DiscoveryState::Interrupted);
        assert!(unstarted.session.cancellation_pending);
        assert_eq!(
            unstarted.session.recovery,
            Some(DiscoveryRecoveryCheckpoint {
                interrupted_state: DiscoveryState::Compensating,
                operation: DiscoveryOperationKind::Compensation,
            })
        );
    }

    #[test]
    fn confirmed_compensation_failure_is_durable_and_requires_explicit_resume() {
        let compensating = valid_session_in(DiscoveryState::Compensating);
        let failed = compensating
            .apply(&envelope(
                &compensating,
                ProviderDiscoveryAction::CompensationFailed { failure: failure() },
            ))
            .unwrap();
        assert_eq!(failed.session.state, DiscoveryState::Compensating);
        assert_eq!(failed.session.failure, Some(failure()));
        assert_eq!(failed.event.failure, Some(failure()));
        assert_eq!(failed.effect, DiscoveryEffect::None);
        assert!(
            failed
                .session
                .apply(&envelope(
                    &failed.session,
                    ProviderDiscoveryAction::CompensationSucceeded,
                ))
                .is_err()
        );

        let resumed = failed
            .session
            .apply(&envelope(
                &failed.session,
                ProviderDiscoveryAction::ResumeCompensation,
            ))
            .unwrap();
        assert_eq!(resumed.session.state, DiscoveryState::Compensating);
        assert_eq!(resumed.session.failure, None);
        assert_eq!(resumed.event.failure, None);
        assert!(matches!(
            resumed.effect,
            DiscoveryEffect::RunCompensation { .. }
        ));

        let mut cancelling = valid_session_in(DiscoveryState::Compensating);
        cancelling.cancellation_pending = true;
        let failed_while_cancelling = cancelling
            .apply(&envelope(
                &cancelling,
                ProviderDiscoveryAction::CompensationFailed { failure: failure() },
            ))
            .unwrap();
        assert_eq!(
            failed_while_cancelling.session.state,
            DiscoveryState::Compensating
        );
        assert!(failed_while_cancelling.session.cancellation_pending);
        assert_eq!(failed_while_cancelling.session.failure, Some(failure()));
        assert_eq!(failed_while_cancelling.effect, DiscoveryEffect::None);

        let compensating = valid_session_in(DiscoveryState::Compensating);
        let cancelling = compensating
            .apply(&envelope(&compensating, ProviderDiscoveryAction::Cancel))
            .unwrap()
            .session;
        let unstarted = cancelling
            .apply(&envelope(
                &cancelling,
                ProviderDiscoveryAction::Interrupt {
                    operation: DiscoveryOperationKind::Compensation,
                    outcome: DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
                },
            ))
            .unwrap();
        assert_eq!(unstarted.session.state, DiscoveryState::Interrupted);
        assert!(unstarted.session.cancellation_pending);
        assert_eq!(
            unstarted.session.recovery,
            Some(DiscoveryRecoveryCheckpoint {
                interrupted_state: DiscoveryState::Compensating,
                operation: DiscoveryOperationKind::Compensation,
            })
        );
    }

    #[test]
    fn completed_read_only_work_is_discarded_after_cancellation_without_duplicate_effects() {
        let fetching = valid_session_in(DiscoveryState::FetchingDocuments);
        let cancelling = fetching
            .apply(&envelope(&fetching, ProviderDiscoveryAction::Cancel))
            .unwrap();
        assert!(cancelling.session.cancellation_pending);
        assert!(matches!(
            cancelling.effect,
            DiscoveryEffect::RequestCancellation { .. }
        ));

        let repeated = cancelling
            .session
            .apply(&envelope(
                &cancelling.session,
                ProviderDiscoveryAction::Cancel,
            ))
            .unwrap();
        assert_eq!(repeated.session.state, DiscoveryState::FetchingDocuments);
        assert_eq!(repeated.effect, DiscoveryEffect::None);

        let completed = repeated
            .session
            .apply(&envelope(
                &repeated.session,
                ProviderDiscoveryAction::DocumentsFetched { evidence_count: 2 },
            ))
            .unwrap();
        assert_eq!(completed.session.state, DiscoveryState::Cancelled);
        assert_eq!(completed.effect, DiscoveryEffect::None);
    }

    #[test]
    fn cancelled_unknown_persistent_work_requires_cleanup_before_terminal_state() {
        fn cancelled_unknown_commit() -> ProviderDiscoverySession {
            let committing = valid_session_in(DiscoveryState::Committing);
            let cancelling = committing
                .apply(&envelope(&committing, ProviderDiscoveryAction::Cancel))
                .unwrap()
                .session;
            cancelling
                .apply(&envelope(
                    &cancelling,
                    ProviderDiscoveryAction::ExternalOutcomeBecameUnknown,
                ))
                .unwrap()
                .session
        }

        let no_effect = cancelled_unknown_commit();
        let cleanup_required = no_effect
            .apply(&envelope(
                &no_effect,
                ProviderDiscoveryAction::ResolveUnknownOutcome {
                    approval_id: DiscoveryApprovalId::new(),
                    resolution: DiscoveryUnknownOutcomeResolution::ConfirmedNoEffect,
                },
            ))
            .unwrap();
        assert_eq!(cleanup_required.session.state, DiscoveryState::Interrupted);
        assert_eq!(cleanup_required.effect, DiscoveryEffect::None);
        assert_eq!(
            cleanup_required.session.recovery,
            Some(DiscoveryRecoveryCheckpoint {
                interrupted_state: DiscoveryState::Compensating,
                operation: DiscoveryOperationKind::Compensation,
            })
        );
        let restarted = cleanup_required
            .session
            .apply(&envelope(
                &cleanup_required.session,
                ProviderDiscoveryAction::RestartInterrupted,
            ))
            .unwrap();
        assert_eq!(restarted.session.state, DiscoveryState::Compensating);
        assert!(restarted.session.cancellation_pending);
        assert!(matches!(
            restarted.effect,
            DiscoveryEffect::RunCompensation { .. }
        ));

        let committed = cancelled_unknown_commit();
        let compensating = committed
            .apply(&envelope(
                &committed,
                ProviderDiscoveryAction::ResolveUnknownOutcome {
                    approval_id: DiscoveryApprovalId::new(),
                    resolution: DiscoveryUnknownOutcomeResolution::ConfirmedCommitCompleted {
                        connection_id: ProviderConnectionId::from("connection-after-race"),
                    },
                },
            ))
            .unwrap();
        assert_eq!(compensating.session.state, DiscoveryState::Compensating);
        assert_eq!(compensating.session.committed_connection_id, None);
        assert!(matches!(
            compensating.effect,
            DiscoveryEffect::RunCompensation { .. }
        ));

        let mut compensating = valid_session_in(DiscoveryState::Compensating);
        compensating.cancellation_pending = true;
        let unknown = compensating
            .apply(&envelope(
                &compensating,
                ProviderDiscoveryAction::ExternalOutcomeBecameUnknown,
            ))
            .unwrap()
            .session;
        let reconciled = unknown
            .apply(&envelope(
                &unknown,
                ProviderDiscoveryAction::ResolveUnknownOutcome {
                    approval_id: DiscoveryApprovalId::new(),
                    resolution: DiscoveryUnknownOutcomeResolution::ConfirmedNoEffect,
                },
            ))
            .unwrap();
        assert_eq!(reconciled.session.state, DiscoveryState::Interrupted);
        assert!(reconciled.session.cancellation_pending);
        assert_eq!(
            reconciled.session.recovery,
            Some(DiscoveryRecoveryCheckpoint {
                interrupted_state: DiscoveryState::Compensating,
                operation: DiscoveryOperationKind::Compensation,
            })
        );
    }

    #[test]
    fn persistent_dtos_have_no_raw_secret_fields() {
        let json = serde_json::to_string(&session()).unwrap();
        for prohibited in [
            "api_key",
            "authorization",
            "cookie",
            "raw_credential",
            "pasted_curl",
            "document_body",
        ] {
            assert!(!json.contains(prohibited));
        }
        assert!(json.contains("credential_ref"));
    }

    #[test]
    fn connection_options_are_typed_bounded_and_backward_compatible() {
        let mut configured = input();
        configured.connection_options = ProviderDiscoveryConnectionOptions {
            values: vec![
                ConnectionConfigEntry {
                    key: "deployment_id".to_owned(),
                    value: ConnectionConfigValue::Text("production".to_owned()),
                },
                ConnectionConfigEntry {
                    key: "parallel_requests".to_owned(),
                    value: ConnectionConfigValue::Integer(4),
                },
                ConnectionConfigEntry {
                    key: "prompt_cache".to_owned(),
                    value: ConnectionConfigValue::Boolean(true),
                },
            ],
            api_base_path: Some(EndpointPath::parse("/v1").unwrap()),
            timeout_seconds: 90,
            network_mode: ProviderNetworkMode::Public,
            local_network_approval: None,
        };
        ProviderDiscoverySession::new(
            DiscoverySessionId::from("session-typed-options"),
            configured.clone(),
        )
        .unwrap();

        let json = serde_json::to_value(configured).unwrap();
        assert!(json["connection_options"]["values"].is_array());
        assert_eq!(
            json["connection_options"]["values"][1]["value"]["type"],
            "integer"
        );
        assert_eq!(json["connection_options"]["api_base_path"], "/v1");

        let mut legacy_json = serde_json::to_value(input()).unwrap();
        legacy_json
            .as_object_mut()
            .unwrap()
            .remove("connection_options");
        let legacy: SanitizedDiscoveryInput = serde_json::from_value(legacy_json).unwrap();
        assert_eq!(
            legacy.connection_options,
            ProviderDiscoveryConnectionOptions::default()
        );
        legacy.validate().unwrap();
    }

    #[test]
    fn connection_options_reject_invalid_timeouts_duplicate_keys_and_credentials() {
        let mut invalid = input();
        invalid.connection_options.timeout_seconds = 0;
        assert!(invalid.validate().is_err());
        invalid.connection_options.timeout_seconds = 601;
        assert!(invalid.validate().is_err());

        let mut invalid = input();
        invalid.connection_options.values = vec![
            ConnectionConfigEntry {
                key: "region".to_owned(),
                value: ConnectionConfigValue::Text("us-east".to_owned()),
            },
            ConnectionConfigEntry {
                key: "region".to_owned(),
                value: ConnectionConfigValue::Text("us-west".to_owned()),
            },
        ];
        assert!(invalid.validate().is_err());

        let mut invalid = input();
        invalid.connection_options.values = vec![ConnectionConfigEntry {
            key: "api_key".to_owned(),
            value: ConnectionConfigValue::Text("redacted".to_owned()),
        }];
        assert!(invalid.validate().is_err());

        let mut invalid = input();
        invalid.connection_options.values = vec![ConnectionConfigEntry {
            key: "custom_value".to_owned(),
            value: ConnectionConfigValue::Text("Bearer must-not-persist".to_owned()),
        }];
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn legacy_network_mode_migrates_but_never_serializes_as_public_contract() {
        let mut legacy_true = serde_json::to_value(input()).unwrap();
        let legacy_true = legacy_true.as_object_mut().unwrap();
        legacy_true.remove("connection_options");
        legacy_true.insert(
            "local_network_mode".to_owned(),
            serde_json::Value::Bool(true),
        );
        let migrated_true: SanitizedDiscoveryInput =
            serde_json::from_value(serde_json::Value::Object(legacy_true.clone())).unwrap();
        assert_eq!(
            migrated_true.connection_options.network_mode,
            ProviderNetworkMode::LocalLoopback
        );
        assert!(
            serde_json::to_value(&migrated_true)
                .unwrap()
                .get("local_network_mode")
                .is_none()
        );

        let mut legacy_false = serde_json::to_value(input()).unwrap();
        let legacy_false = legacy_false.as_object_mut().unwrap();
        legacy_false.remove("connection_options");
        legacy_false.insert(
            "local_network_mode".to_owned(),
            serde_json::Value::Bool(false),
        );
        let migrated_false: SanitizedDiscoveryInput =
            serde_json::from_value(serde_json::Value::Object(legacy_false.clone())).unwrap();
        assert_eq!(
            migrated_false.connection_options.network_mode,
            ProviderNetworkMode::Public
        );
        assert!(
            serde_json::to_value(&migrated_false)
                .unwrap()
                .get("local_network_mode")
                .is_none()
        );
    }

    #[test]
    fn legacy_and_typed_network_authorities_fail_closed_when_they_conflict() {
        let mut conflicting = serde_json::to_value(input()).unwrap();
        conflicting.as_object_mut().unwrap().insert(
            "local_network_mode".to_owned(),
            serde_json::Value::Bool(true),
        );
        assert!(serde_json::from_value::<SanitizedDiscoveryInput>(conflicting).is_err());

        let mut matching = input();
        matching.connection_options.network_mode = ProviderNetworkMode::LocalLoopback;
        let mut matching = serde_json::to_value(matching).unwrap();
        matching.as_object_mut().unwrap().insert(
            "local_network_mode".to_owned(),
            serde_json::Value::Bool(true),
        );
        let migrated: SanitizedDiscoveryInput = serde_json::from_value(matching).unwrap();
        assert_eq!(
            migrated.connection_options.network_mode,
            ProviderNetworkMode::LocalLoopback
        );
    }

    #[test]
    fn approved_lan_options_require_an_exact_origin_and_private_address_set() {
        let mut approved = input();
        approved.connection_options.network_mode = ProviderNetworkMode::ApprovedLocalNetwork;
        approved.connection_options.local_network_approval = Some(ProviderLocalNetworkApproval {
            origin: CanonicalOrigin::parse("http://model-server.lan:11434").unwrap(),
            addresses: vec![
                "192.168.10.20".parse::<IpAddr>().unwrap(),
                "fd12:3456::20".parse::<IpAddr>().unwrap(),
            ],
        });
        approved.validate().unwrap();

        let mut missing_grant = input();
        missing_grant.connection_options.network_mode = ProviderNetworkMode::ApprovedLocalNetwork;
        assert!(missing_grant.validate().is_err());

        let mut public_with_grant = approved.clone();
        public_with_grant.connection_options.network_mode = ProviderNetworkMode::Public;
        assert!(public_with_grant.validate().is_err());

        let mut no_addresses = approved.clone();
        no_addresses
            .connection_options
            .local_network_approval
            .as_mut()
            .unwrap()
            .addresses
            .clear();
        assert!(no_addresses.validate().is_err());

        let mut public_address = approved.clone();
        public_address
            .connection_options
            .local_network_approval
            .as_mut()
            .unwrap()
            .addresses = vec!["8.8.8.8".parse::<IpAddr>().unwrap()];
        assert!(public_address.validate().is_err());

        let mut too_many_addresses = approved.clone();
        too_many_addresses
            .connection_options
            .local_network_approval
            .as_mut()
            .unwrap()
            .addresses = (1_u8..=17)
            .map(|host| IpAddr::from([10, 0, 0, host]))
            .collect();
        assert!(too_many_addresses.validate().is_err());

        let mut duplicate_address = approved;
        duplicate_address
            .connection_options
            .local_network_approval
            .as_mut()
            .unwrap()
            .addresses = vec![
            "10.0.0.2".parse::<IpAddr>().unwrap(),
            "10.0.0.2".parse::<IpAddr>().unwrap(),
        ];
        assert!(duplicate_address.validate().is_err());
    }

    #[test]
    fn persisted_input_rejects_unsafe_urls_identity_names_and_credential_slots() {
        let mut unsafe_input = input();
        unsafe_input.site_url =
            HttpUrl::parse("https://provider.example/setup?api_key=must-not-persist").unwrap();
        assert!(
            ProviderDiscoverySession::new(DiscoverySessionId::from("session-query"), unsafe_input)
                .is_err()
        );

        let mut unsafe_input = input();
        unsafe_input.credential_ref = Some(CredentialRef("x".repeat(257)));
        assert!(
            ProviderDiscoverySession::new(
                DiscoverySessionId::from("session-credential-ref"),
                unsafe_input
            )
            .is_err()
        );

        let mut unsafe_input = input();
        unsafe_input.credential_ref = Some(CredentialRef("different-native-slot".to_owned()));
        assert!(
            ProviderDiscoverySession::new(
                DiscoverySessionId::from("session-mismatched-slot"),
                unsafe_input
            )
            .is_err()
        );

        let mut unsafe_input = input();
        unsafe_input.display_name = " \n".to_owned();
        assert!(
            ProviderDiscoverySession::new(
                DiscoverySessionId::from("session-display-name"),
                unsafe_input
            )
            .is_err()
        );

        let mut unsafe_input = input();
        unsafe_input.connection_id = ProviderConnectionId::from("x".repeat(129));
        unsafe_input.credential_ref = None;
        assert!(
            ProviderDiscoverySession::new(
                DiscoverySessionId::from("session-connection-id"),
                unsafe_input
            )
            .is_err()
        );
    }

    #[test]
    fn discovery_ids_validate_during_deserialization() {
        assert!(serde_json::from_str::<DiscoveryActionId>("\"\"").is_err());
        assert!(serde_json::from_str::<DiscoveryActionId>("\" action \"").is_err());
        assert_eq!(
            serde_json::from_str::<DiscoveryActionId>("\"action-1\"")
                .unwrap()
                .as_str(),
            "action-1"
        );
    }

    #[test]
    fn discovery_state_wire_values_cover_all_variants() {
        let values = DiscoveryState::ALL
            .into_iter()
            .map(|state| serde_json::to_string(&state).unwrap())
            .collect::<Vec<_>>();
        assert!(values.contains(&"\"awaiting_template_selection\"".to_owned()));
        assert!(values.contains(&"\"awaiting_more_evidence\"".to_owned()));
        assert!(values.contains(&"\"interrupted\"".to_owned()));
        assert!(values.contains(&"\"unknown_outcome\"".to_owned()));
        assert_eq!(values.len(), 22);
    }

    fn representative_actions() -> Vec<ProviderDiscoveryAction> {
        vec![
            ProviderDiscoveryAction::Begin,
            ProviderDiscoveryAction::KnownProviderCandidatesResolved { candidate_count: 0 },
            ProviderDiscoveryAction::SelectTemplate {
                candidate_id: DiscoveryCandidateId::new(),
            },
            ProviderDiscoveryAction::ContinueWithoutTemplate,
            ProviderDiscoveryAction::DocumentsFetched { evidence_count: 0 },
            ProviderDiscoveryAction::EvidenceExtracted {
                resolution: DiscoveryEvidenceResolution::MoreEvidenceRequired,
            },
            ProviderDiscoveryAction::SupplyMoreEvidence {
                evidence_ids: vec![EvidenceId::from("e")],
            },
            ProviderDiscoveryAction::RequestAssistant,
            ProviderDiscoveryAction::ApproveAssistant {
                approval_id: DiscoveryApprovalId::new(),
                approval_grant_sha256: HASH.to_owned(),
            },
            ProviderDiscoveryAction::DeclineAssistant,
            ProviderDiscoveryAction::ManifestDraftBuilt {
                manifest_sha256: HASH.to_owned(),
            },
            ProviderDiscoveryAction::ManifestValidated {
                manifest_sha256: HASH.to_owned(),
                credential_origin_approval_required: false,
            },
            ProviderDiscoveryAction::ApproveCredentialOrigin {
                approval_id: DiscoveryApprovalId::new(),
            },
            ProviderDiscoveryAction::ModelsListed {
                model_count: 0,
                probe_candidate_count: 0,
            },
            ProviderDiscoveryAction::ApproveProbes {
                approval_id: DiscoveryApprovalId::new(),
                approval_grant_sha256: HASH.to_owned(),
            },
            ProviderDiscoveryAction::SkipProbes,
            ProviderDiscoveryAction::ProbesCompleted,
            ProviderDiscoveryAction::ApproveReview {
                approval_id: DiscoveryApprovalId::new(),
                commit_attempt_id: DiscoveryCommitAttemptId::new(),
                commit_plan_sha256: HASH.to_owned(),
                graph_sha256: HASH.to_owned(),
            },
            ProviderDiscoveryAction::CommitSucceeded {
                connection_id: ProviderConnectionId::from("c"),
            },
            ProviderDiscoveryAction::CommitFailedBeforeApply { failure: failure() },
            ProviderDiscoveryAction::CompensationRequired,
            ProviderDiscoveryAction::CompensationSucceeded,
            ProviderDiscoveryAction::CompensationFailed { failure: failure() },
            ProviderDiscoveryAction::ResumeCompensation,
            ProviderDiscoveryAction::ExternalOutcomeBecameUnknown,
            ProviderDiscoveryAction::Interrupt {
                operation: DiscoveryOperationKind::FetchDocuments,
                outcome: DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
            },
            ProviderDiscoveryAction::RestartInterrupted,
            ProviderDiscoveryAction::ResolveUnknownOutcome {
                approval_id: DiscoveryApprovalId::new(),
                resolution: DiscoveryUnknownOutcomeResolution::ManuallyReconciledAsFailed,
            },
            ProviderDiscoveryAction::Fail { failure: failure() },
            ProviderDiscoveryAction::Cancel,
        ]
    }

    fn failure() -> DiscoveryFailure {
        DiscoveryFailure {
            code: "network.timeout".to_owned(),
            message_key: "discovery.error.timeout".to_owned(),
            recoverable: true,
        }
    }

    fn valid_session_in(state: DiscoveryState) -> ProviderDiscoverySession {
        let mut session = session();
        session.state = state;
        if state == DiscoveryState::Interrupted {
            session.recovery = Some(DiscoveryRecoveryCheckpoint {
                interrupted_state: DiscoveryState::FetchingDocuments,
                operation: DiscoveryOperationKind::FetchDocuments,
            });
        }
        if state == DiscoveryState::UnknownOutcome {
            session.unknown_operation = Some(DiscoveryOperationKind::ProbeCapabilities);
            session.active_effect_approval = Some(DiscoveryApprovalBinding {
                approval_id: DiscoveryApprovalId::new(),
                grant_sha256: HASH.to_owned(),
            });
        }
        if matches!(
            state,
            DiscoveryState::BuildingAssistantManifestDraft | DiscoveryState::ProbingCapabilities
        ) {
            session.active_effect_approval = Some(DiscoveryApprovalBinding {
                approval_id: DiscoveryApprovalId::new(),
                grant_sha256: HASH.to_owned(),
            });
        }
        if matches!(
            state,
            DiscoveryState::Committing | DiscoveryState::Compensating
        ) {
            session.commit_plan_sha256 = Some(HASH.to_owned());
            session.commit_attempt_id = Some(DiscoveryCommitAttemptId::new());
        }
        if state == DiscoveryState::Ready {
            session.committed_connection_id = Some(ProviderConnectionId::from("connection"));
        }
        session.validate().unwrap();
        session
    }
}
