use std::net::IpAddr;

use chrono::{DateTime, Utc};
use lorepia_core::{
    ApiFamily, AppSettings, AuthBinding, CacheTtlBounds, CanonicalOrigin, CapabilityKey,
    CapabilityObservation, CapabilityValue, Confidence, ConnectionConfigEntry,
    ConnectionConfigValue, ConnectionFieldSpec, ConnectionFieldType, ConnectionStatus,
    CredentialRedirectPolicy, CredentialScope, EffectiveCapability, EndpointPath, GenerationPreset,
    GenerationPresetId, GenerationPromptCacheMode, GenerationPromptCacheSettings,
    GenerationPromptCacheTtl, GenerationReasoningEffort, GenerationReasoningMode,
    GenerationReasoningSettings, GenerationReasoningSummary, GenerationTarget, HttpMethod,
    ModelAvailability, ModelMetadataSource, ModelRoute, ModelRouteConfig, ModelRouteId,
    ObservationId, ObservationSource, ParameterChoice, ParameterCondition,
    ParameterConditionOperator, ParameterConflict, ParameterConflictKind, ParameterDefaultMode,
    ParameterIssue, ParameterIssueCode, ParameterLiteral, ParameterSpec, ParameterType,
    ParameterValue, ParameterValueState, PromptCacheControlModel, PromptCacheMode,
    PromptCacheSettings, PromptCacheTtl, ProviderConnection, ProviderConnectionDraft,
    ProviderConnectionId, ProviderCredentialObservedStatus, ProviderCredentialOperationKind,
    ProviderLocalNetworkApproval, ProviderNetworkMode, ProviderParameterMapping,
    ProviderParameterTarget, ProviderProfile, ProviderTemplateId, ProviderTemplateView,
    ReasoningControlModel, ReasoningEffort, ReasoningMode, ReasoningSettings, ReasoningSummaryMode,
    RequestBodyShape, RequestPreview, SupportStatus, TemplateSource, TokenBudgetBounds, ToolPolicy,
    UiControlState, UiFieldState, UiParameterLevel,
};
use serde::{Deserialize, Serialize};

use crate::{GenerationTargetDto, ShellApi, ShellError, ShellResult, api::validate_identifier};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppSettingsDto {
    pub preserve_partial_generations: bool,
    pub selected_provider_profile_id: Option<String>,
    pub selected_model_route_id: Option<String>,
    pub selected_generation_preset_id: Option<String>,
}

impl From<AppSettings> for AppSettingsDto {
    fn from(value: AppSettings) -> Self {
        Self {
            preserve_partial_generations: value.preserve_partial_generations,
            selected_provider_profile_id: value.selected_provider_profile_id,
            selected_model_route_id: value.selected_model_route_id.map(|id| id.0),
            selected_generation_preset_id: value.selected_generation_preset_id.map(|id| id.0),
        }
    }
}

impl From<AppSettingsDto> for AppSettings {
    fn from(value: AppSettingsDto) -> Self {
        Self {
            preserve_partial_generations: value.preserve_partial_generations,
            selected_provider_profile_id: value.selected_provider_profile_id,
            selected_model_route_id: value.selected_model_route_id.map(ModelRouteId::from),
            selected_generation_preset_id: value
                .selected_generation_preset_id
                .map(GenerationPresetId::from),
            ..AppSettings::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderNetworkModeInput {
    Public,
    LocalLoopback,
    ApprovedLocalNetwork,
}

impl From<ProviderNetworkModeInput> for ProviderNetworkMode {
    fn from(value: ProviderNetworkModeInput) -> Self {
        match value {
            ProviderNetworkModeInput::Public => Self::Public,
            ProviderNetworkModeInput::LocalLoopback => Self::LocalLoopback,
            ProviderNetworkModeInput::ApprovedLocalNetwork => Self::ApprovedLocalNetwork,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderLocalNetworkApprovalInput {
    pub origin: String,
    pub addresses: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateProviderConnectionInput {
    pub id: String,
    pub template_id: String,
    pub template_version: u32,
    pub display_name: String,
    pub api_origin: String,
    pub api_base_path: Option<String>,
    pub network_mode: ProviderNetworkModeInput,
    pub local_network_approval: Option<ProviderLocalNetworkApprovalInput>,
    pub values: Vec<ConnectionConfigEntryDto>,
    /// Exact origin approved for an opaque native credential slot.
    ///
    /// No credential value or credential reference can be represented here.
    pub approved_credential_origin: Option<String>,
    pub timeout_seconds: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateProviderConnectionInput {
    pub id: String,
    pub display_name: String,
    pub timeout_seconds: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionFieldSpecDto {
    pub key: String,
    pub label_key: String,
    pub description_key: Option<String>,
    pub value_type: String,
    pub required: bool,
}

impl From<ConnectionFieldSpec> for ConnectionFieldSpecDto {
    fn from(value: ConnectionFieldSpec) -> Self {
        Self {
            key: value.key,
            label_key: value.label_key,
            description_key: value.description_key,
            value_type: connection_field_type_name(value.value_type).to_owned(),
            required: value.required,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AuthBindingDto {
    None,
    BearerHeader,
    HeaderApiKey { header_name: String },
}

impl From<AuthBinding> for AuthBindingDto {
    fn from(value: AuthBinding) -> Self {
        match value {
            AuthBinding::None => Self::None,
            AuthBinding::BearerHeader => Self::BearerHeader,
            AuthBinding::HeaderApiKey { header_name } => Self::HeaderApiKey {
                header_name: header_name.as_str().to_owned(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderTemplateDto {
    pub id: String,
    pub display_name: String,
    pub manifest_version: u32,
    pub source: String,
    pub api_family: String,
    pub connection_fields: Vec<ConnectionFieldSpecDto>,
    pub default_network_mode: String,
    pub default_api_origin: Option<String>,
    pub credential_required: bool,
    pub supports_model_listing: bool,
    pub auth_binding: AuthBindingDto,
    pub parameters: Vec<ParameterSpecDto>,
}

impl From<ProviderTemplateView> for ProviderTemplateDto {
    fn from(value: ProviderTemplateView) -> Self {
        let template = value.template;
        let credential_required = !matches!(&template.default_manifest.auth, AuthBinding::None);
        let supports_model_listing = template.default_manifest.endpoints.models.is_some();
        Self {
            id: template.id.0,
            display_name: template.display_name,
            manifest_version: template.manifest_version,
            source: template_source_name(template.source).to_owned(),
            api_family: api_family_name(template.api_family).to_owned(),
            connection_fields: template
                .connection_fields
                .into_iter()
                .map(Into::into)
                .collect(),
            default_network_mode: network_mode_name(value.default_network_mode).to_owned(),
            default_api_origin: template
                .default_manifest
                .default_api_origin
                .map(|origin| origin.to_string()),
            credential_required,
            supports_model_listing,
            auth_binding: template.default_manifest.auth.into(),
            parameters: template
                .default_manifest
                .parameters
                .into_iter()
                .map(Into::into)
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ConnectionConfigValueDto {
    Text(String),
    Integer(i64),
    Boolean(bool),
}

impl From<ConnectionConfigValue> for ConnectionConfigValueDto {
    fn from(value: ConnectionConfigValue) -> Self {
        match value {
            ConnectionConfigValue::Text(value) => Self::Text(value),
            ConnectionConfigValue::Integer(value) => Self::Integer(value),
            ConnectionConfigValue::Boolean(value) => Self::Boolean(value),
        }
    }
}

impl From<ConnectionConfigValueDto> for ConnectionConfigValue {
    fn from(value: ConnectionConfigValueDto) -> Self {
        match value {
            ConnectionConfigValueDto::Text(value) => Self::Text(value),
            ConnectionConfigValueDto::Integer(value) => Self::Integer(value),
            ConnectionConfigValueDto::Boolean(value) => Self::Boolean(value),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionConfigEntryDto {
    pub key: String,
    pub value: ConnectionConfigValueDto,
}

impl From<ConnectionConfigEntry> for ConnectionConfigEntryDto {
    fn from(value: ConnectionConfigEntry) -> Self {
        Self {
            key: value.key,
            value: value.value.into(),
        }
    }
}

impl From<ConnectionConfigEntryDto> for ConnectionConfigEntry {
    fn from(value: ConnectionConfigEntryDto) -> Self {
        Self {
            key: value.key,
            value: value.value.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderLocalNetworkApprovalDto {
    pub origin: String,
    pub addresses: Vec<String>,
}

impl From<ProviderLocalNetworkApproval> for ProviderLocalNetworkApprovalDto {
    fn from(value: ProviderLocalNetworkApproval) -> Self {
        Self {
            origin: value.origin.to_string(),
            addresses: value
                .addresses
                .into_iter()
                .map(|address| address.to_string())
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialScopeDto {
    pub allowed_origins: Vec<String>,
    pub auth_binding: AuthBindingDto,
    pub redirect_policy: String,
}

impl From<CredentialScope> for CredentialScopeDto {
    fn from(value: CredentialScope) -> Self {
        Self {
            allowed_origins: value
                .allowed_origins
                .into_iter()
                .map(|origin| origin.to_string())
                .collect(),
            auth_binding: value.auth_binding.into(),
            redirect_policy: credential_redirect_policy_name(value.redirect_policy).to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConnectionDto {
    pub id: String,
    pub template_id: String,
    pub template_version: u32,
    pub display_name: String,
    pub api_origin: String,
    pub api_base_path: Option<String>,
    pub network_mode: String,
    pub local_network_approval: Option<ProviderLocalNetworkApprovalDto>,
    pub config_values: Vec<ConnectionConfigEntryDto>,
    /// Whether Core has an opaque credential binding for this connection.
    ///
    /// Actual vault availability is owned by the platform plugin and is not
    /// inferred here.
    pub credential_binding_required: bool,
    pub credential_scope: Option<CredentialScopeDto>,
    pub approved_credential_origins: Vec<String>,
    pub timeout_seconds: u32,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderCredentialOperationKindInput {
    Install,
    RemoveCredential,
    RemoveForArchive,
}

impl From<ProviderCredentialOperationKindInput> for ProviderCredentialOperationKind {
    fn from(value: ProviderCredentialOperationKindInput) -> Self {
        match value {
            ProviderCredentialOperationKindInput::Install => Self::Install,
            ProviderCredentialOperationKindInput::RemoveCredential => Self::RemoveCredential,
            ProviderCredentialOperationKindInput::RemoveForArchive => Self::RemoveForArchive,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderCredentialSlotStatusInput {
    Missing,
    Available,
    Unreadable,
}

impl From<ProviderCredentialSlotStatusInput> for ProviderCredentialObservedStatus {
    fn from(value: ProviderCredentialSlotStatusInput) -> Self {
        match value {
            ProviderCredentialSlotStatusInput::Missing => Self::Missing,
            ProviderCredentialSlotStatusInput::Available => Self::Available,
            ProviderCredentialSlotStatusInput::Unreadable => Self::Unreadable,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
// The two recovery flags are independent projections of the append-only slot
// ledger; collapsing them would reintroduce the replacement-barrier overwrite.
#[allow(clippy::struct_excessive_bools)]
pub struct ProviderCredentialOperationContext {
    pub operation_id: String,
    pub connection_id: String,
    pub operation_kind: ProviderCredentialOperationKindInput,
    pub plan_sha256: String,
    pub connection_binding_sha256: String,
    pub credential_authority_id: Option<String>,
    pub credential_authority_binding_sha256: Option<String>,
    pub predecessor_authority_id: Option<String>,
    pub predecessor_authority_binding_sha256: Option<String>,
    pub preflight_status: ProviderCredentialSlotStatusInput,
    pub native_effect_started: bool,
    pub cleanup_archives_connection: bool,
    pub status: String,
    /// Durable, non-secret reason attached to a settled or blocked operation.
    /// Native bootstrap uses this to preserve an explicit platform recovery
    /// barrier instead of treating immediate visibility as durability proof.
    pub outcome_code: Option<String>,
    pub operation_slot_recovery_required: bool,
    pub predecessor_slot_recovery_required: bool,
}

/// Backend-only native secure-item authority. It is intentionally not a
/// webview DTO and has no serialization implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCredentialAccessAuthorityContext {
    pub authority_id: String,
    pub connection_binding_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCredentialSlotGarbageContext {
    pub connection_id: String,
    pub authority_sequence: u64,
    pub authority: ProviderCredentialAccessAuthorityContext,
    pub status: String,
}

impl From<lorepia_core::ProviderCredentialSlotGarbage> for ProviderCredentialSlotGarbageContext {
    fn from(value: lorepia_core::ProviderCredentialSlotGarbage) -> Self {
        Self {
            connection_id: value.connection_id.0,
            authority_sequence: value.authority_sequence,
            authority: ProviderCredentialAccessAuthorityContext {
                authority_id: value.authority.authority_id,
                connection_binding_sha256: value.authority.connection_binding_sha256,
            },
            status: match value.status {
                lorepia_core::ProviderCredentialSlotGarbageStatus::Pending => "pending",
                lorepia_core::ProviderCredentialSlotGarbageStatus::Started => "started",
                lorepia_core::ProviderCredentialSlotGarbageStatus::Completed => "completed",
            }
            .to_owned(),
        }
    }
}

impl From<lorepia_core::ProviderCredentialOperationView> for ProviderCredentialOperationContext {
    fn from(value: lorepia_core::ProviderCredentialOperationView) -> Self {
        Self {
            operation_id: value.plan.operation_id,
            connection_id: value.plan.connection_id.0,
            operation_kind: match value.plan.operation_kind {
                ProviderCredentialOperationKind::Install => {
                    ProviderCredentialOperationKindInput::Install
                }
                ProviderCredentialOperationKind::RemoveCredential => {
                    ProviderCredentialOperationKindInput::RemoveCredential
                }
                ProviderCredentialOperationKind::RemoveForArchive => {
                    ProviderCredentialOperationKindInput::RemoveForArchive
                }
            },
            plan_sha256: value.plan_sha256,
            connection_binding_sha256: value.plan.connection_binding_sha256,
            credential_authority_id: value.plan.credential_authority_id,
            credential_authority_binding_sha256: value.plan.credential_authority_binding_sha256,
            predecessor_authority_id: value.plan.predecessor_authority_id,
            predecessor_authority_binding_sha256: value.plan.predecessor_authority_binding_sha256,
            preflight_status: match value.preflight_status {
                ProviderCredentialObservedStatus::Missing => {
                    ProviderCredentialSlotStatusInput::Missing
                }
                ProviderCredentialObservedStatus::Available => {
                    ProviderCredentialSlotStatusInput::Available
                }
                ProviderCredentialObservedStatus::Unreadable => {
                    ProviderCredentialSlotStatusInput::Unreadable
                }
            },
            native_effect_started: value.started_at.is_some(),
            cleanup_archives_connection: value.cleanup_archives_connection,
            status: match value.status {
                lorepia_core::ProviderCredentialOperationStatus::Prepared => "prepared",
                lorepia_core::ProviderCredentialOperationStatus::Started => "started",
                lorepia_core::ProviderCredentialOperationStatus::Succeeded => "succeeded",
                lorepia_core::ProviderCredentialOperationStatus::NoEffect => "no_effect",
                lorepia_core::ProviderCredentialOperationStatus::CleanupRequired => {
                    "cleanup_required"
                }
                lorepia_core::ProviderCredentialOperationStatus::OutcomeUnknown => {
                    "outcome_unknown"
                }
            }
            .to_owned(),
            outcome_code: value.outcome_code.map(|outcome| {
                match outcome {
                    lorepia_core::ProviderCredentialOutcomeCode::NativeEffectConfirmed => {
                        "native_effect_confirmed"
                    }
                    lorepia_core::ProviderCredentialOutcomeCode::NativeEffectAbsent => {
                        "native_effect_absent"
                    }
                    lorepia_core::ProviderCredentialOutcomeCode::NativeStatusUnreadable => {
                        "native_status_unreadable"
                    }
                    lorepia_core::ProviderCredentialOutcomeCode::NativeDurabilityUnknown => {
                        "native_durability_unknown"
                    }
                    lorepia_core::ProviderCredentialOutcomeCode::NativePredecessorDurabilityUnknown => {
                        "native_predecessor_durability_unknown"
                    }
                    lorepia_core::ProviderCredentialOutcomeCode::ConnectionChanged => {
                        "connection_changed"
                    }
                    lorepia_core::ProviderCredentialOutcomeCode::ArchiveCommitFailed => {
                        "archive_commit_failed"
                    }
                }
                .to_owned()
            }),
            operation_slot_recovery_required: value.operation_slot_recovery_required,
            predecessor_slot_recovery_required: value.predecessor_slot_recovery_required,
        }
    }
}

impl From<ProviderConnection> for ProviderConnectionDto {
    fn from(value: ProviderConnection) -> Self {
        let approved_credential_origins = value
            .credential_scope
            .as_ref()
            .map(|scope| {
                scope
                    .allowed_origins
                    .iter()
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default();
        Self {
            id: value.id.0,
            template_id: value.template_id.0,
            template_version: value.template_version,
            display_name: value.display_name,
            api_origin: value.api_origin.to_string(),
            api_base_path: value
                .config
                .api_base_path
                .as_ref()
                .map(|path| path.as_str().to_owned()),
            network_mode: network_mode_name(value.config.network_mode).to_owned(),
            local_network_approval: value.config.local_network_approval.map(Into::into),
            config_values: value.config.values.into_iter().map(Into::into).collect(),
            credential_binding_required: value.credential_ref.is_some(),
            credential_scope: value.credential_scope.map(Into::into),
            approved_credential_origins,
            timeout_seconds: value.timeout_seconds,
            status: connection_status_name(value.status).to_owned(),
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRouteConfigDto {
    pub deployment_id: Option<String>,
    pub region: Option<String>,
    pub endpoint_path: Option<String>,
    pub values: Vec<ConnectionConfigEntryDto>,
}

impl From<ModelRouteConfig> for ModelRouteConfigDto {
    fn from(value: ModelRouteConfig) -> Self {
        Self {
            deployment_id: value.deployment_id,
            region: value.region,
            endpoint_path: value
                .endpoint_path
                .as_ref()
                .map(|path| path.as_str().to_owned()),
            values: value.values.into_iter().map(Into::into).collect(),
        }
    }
}

impl TryFrom<ModelRouteConfigDto> for ModelRouteConfig {
    type Error = ShellError;

    fn try_from(value: ModelRouteConfigDto) -> Result<Self, Self::Error> {
        Ok(Self {
            deployment_id: value.deployment_id,
            region: value.region,
            endpoint_path: value
                .endpoint_path
                .map(|path| parse_endpoint_path("route_config.endpoint_path", &path))
                .transpose()?,
            values: value.values.into_iter().map(Into::into).collect(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRouteDto {
    pub id: String,
    pub connection_id: String,
    pub api_family: String,
    pub model_id: String,
    pub display_name: Option<String>,
    pub route_config: ModelRouteConfigDto,
    pub status: String,
    pub miss_count: u32,
    pub metadata_source: String,
    pub metadata_observed_at: Option<DateTime<Utc>>,
    pub first_seen_at: DateTime<Utc>,
    pub last_seen_at: Option<DateTime<Utc>>,
}

impl From<ModelRoute> for ModelRouteDto {
    fn from(value: ModelRoute) -> Self {
        Self {
            id: value.id.0,
            connection_id: value.connection_id.0,
            api_family: api_family_name(value.api_family).to_owned(),
            model_id: value.model_id,
            display_name: value.display_name,
            route_config: value.route_config.into(),
            status: model_availability_name(value.status).to_owned(),
            miss_count: value.miss_count,
            metadata_source: metadata_source_name(value.metadata_source).to_owned(),
            metadata_observed_at: value.metadata_observed_at,
            first_seen_at: value.first_seen_at,
            last_seen_at: value.last_seen_at,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiFamilyInput {
    OpenAiResponses,
    OpenAiChatCompletions,
    AnthropicMessages,
    GeminiGenerateContent,
    OllamaNative,
}

impl From<ApiFamilyInput> for ApiFamily {
    fn from(value: ApiFamilyInput) -> Self {
        match value {
            ApiFamilyInput::OpenAiResponses => Self::OpenAiResponses,
            ApiFamilyInput::OpenAiChatCompletions => Self::OpenAiChatCompletions,
            ApiFamilyInput::AnthropicMessages => Self::AnthropicMessages,
            ApiFamilyInput::GeminiGenerateContent => Self::GeminiGenerateContent,
            ApiFamilyInput::OllamaNative => Self::OllamaNative,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelAvailabilityInput {
    Available,
    MissingTemporarily,
    DocumentedOnly,
    AccessDenied,
    Deprecated,
    Retired,
    Unknown,
}

impl From<ModelAvailabilityInput> for ModelAvailability {
    fn from(value: ModelAvailabilityInput) -> Self {
        match value {
            ModelAvailabilityInput::Available => Self::Available,
            ModelAvailabilityInput::MissingTemporarily => Self::MissingTemporarily,
            ModelAvailabilityInput::DocumentedOnly => Self::DocumentedOnly,
            ModelAvailabilityInput::AccessDenied => Self::AccessDenied,
            ModelAvailabilityInput::Deprecated => Self::Deprecated,
            ModelAvailabilityInput::Retired => Self::Retired,
            ModelAvailabilityInput::Unknown => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum UpsertModelRouteInput {
    Create {
        id: String,
        connection_id: String,
        api_family: ApiFamilyInput,
        model_id: String,
        display_name: Option<String>,
        route_config: ModelRouteConfigDto,
        status: ModelAvailabilityInput,
    },
    Update {
        id: String,
        display_name: Option<String>,
        status: ModelAvailabilityInput,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ParameterLiteralDto {
    Boolean(bool),
    Integer(i64),
    Number(f64),
    String(String),
    Enum(String),
    StringList(Vec<String>),
    JsonSchema(String),
    StopSequenceList(Vec<String>),
    ToolPolicy(String),
}

impl From<ParameterLiteral> for ParameterLiteralDto {
    fn from(value: ParameterLiteral) -> Self {
        match value {
            ParameterLiteral::Boolean(value) => Self::Boolean(value),
            ParameterLiteral::Integer(value) => Self::Integer(value),
            ParameterLiteral::Number(value) => Self::Number(value),
            ParameterLiteral::String(value) => Self::String(value),
            ParameterLiteral::Enum(value) => Self::Enum(value),
            ParameterLiteral::StringList(value) => Self::StringList(value),
            ParameterLiteral::JsonSchema(value) => Self::JsonSchema(value),
            ParameterLiteral::StopSequenceList(value) => Self::StopSequenceList(value),
            ParameterLiteral::ToolPolicy(value) => {
                Self::ToolPolicy(tool_policy_name(value).to_owned())
            }
        }
    }
}

impl TryFrom<ParameterLiteralDto> for ParameterLiteral {
    type Error = ShellError;

    fn try_from(value: ParameterLiteralDto) -> Result<Self, Self::Error> {
        Ok(match value {
            ParameterLiteralDto::Boolean(value) => Self::Boolean(value),
            ParameterLiteralDto::Integer(value) => Self::Integer(value),
            ParameterLiteralDto::Number(value) => Self::Number(value),
            ParameterLiteralDto::String(value) => Self::String(value),
            ParameterLiteralDto::Enum(value) => Self::Enum(value),
            ParameterLiteralDto::StringList(value) => Self::StringList(value),
            ParameterLiteralDto::JsonSchema(value) => Self::JsonSchema(value),
            ParameterLiteralDto::StopSequenceList(value) => Self::StopSequenceList(value),
            ParameterLiteralDto::ToolPolicy(value) => Self::ToolPolicy(parse_tool_policy(&value)?),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterChoiceDto {
    pub value: ParameterLiteralDto,
    pub label_key: String,
}

impl From<ParameterChoice> for ParameterChoiceDto {
    fn from(value: ParameterChoice) -> Self {
        Self {
            value: value.value.into(),
            label_key: value.label_key,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterConditionDto {
    pub parameter_id: String,
    pub operator: String,
    pub value: ParameterLiteralDto,
}

impl From<ParameterCondition> for ParameterConditionDto {
    fn from(value: ParameterCondition) -> Self {
        Self {
            parameter_id: value.parameter_id.0,
            operator: parameter_condition_operator_name(value.operator).to_owned(),
            value: value.value.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterConflictDto {
    pub parameter_id: String,
    pub kind: String,
    pub message_key: String,
}

impl From<ParameterConflict> for ParameterConflictDto {
    fn from(value: ParameterConflict) -> Self {
        Self {
            parameter_id: value.parameter_id.0,
            kind: parameter_conflict_kind_name(value.kind).to_owned(),
            message_key: value.message_key,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderParameterMappingDto {
    pub target: String,
    pub field_name: String,
}

impl From<ProviderParameterMapping> for ProviderParameterMappingDto {
    fn from(value: ProviderParameterMapping) -> Self {
        Self {
            target: provider_parameter_target_name(value.target).to_owned(),
            field_name: value.field_name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterSpecDto {
    pub id: String,
    pub label_key: String,
    pub description_key: Option<String>,
    pub value_type: String,
    pub allowed_values: Vec<ParameterChoiceDto>,
    pub minimum: Option<f64>,
    pub maximum: Option<f64>,
    pub step: Option<f64>,
    pub default_mode: String,
    pub visibility: Option<ParameterConditionDto>,
    pub conflicts: Vec<ParameterConflictDto>,
    pub provider_mapping: ProviderParameterMappingDto,
    pub level: String,
}

impl From<ParameterSpec> for ParameterSpecDto {
    fn from(value: ParameterSpec) -> Self {
        Self {
            id: value.id.0,
            label_key: value.label_key,
            description_key: value.description_key,
            value_type: parameter_type_name(value.value_type).to_owned(),
            allowed_values: value.allowed_values.into_iter().map(Into::into).collect(),
            minimum: value.minimum,
            maximum: value.maximum,
            step: value.step,
            default_mode: parameter_default_mode_name(value.default_mode).to_owned(),
            visibility: value.visibility.map(Into::into),
            conflicts: value.conflicts.into_iter().map(Into::into).collect(),
            provider_mapping: value.provider_mapping.into(),
            level: ui_parameter_level_name(value.level).to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub enum ParameterValueStateDto {
    InheritProviderDefault,
    Explicit(ParameterLiteralDto),
}

impl From<ParameterValueState> for ParameterValueStateDto {
    fn from(value: ParameterValueState) -> Self {
        match value {
            ParameterValueState::InheritProviderDefault => Self::InheritProviderDefault,
            ParameterValueState::Explicit(value) => Self::Explicit(value.into()),
        }
    }
}

impl TryFrom<ParameterValueStateDto> for ParameterValueState {
    type Error = ShellError;

    fn try_from(value: ParameterValueStateDto) -> Result<Self, Self::Error> {
        match value {
            ParameterValueStateDto::InheritProviderDefault => Ok(Self::InheritProviderDefault),
            ParameterValueStateDto::Explicit(value) => Ok(Self::Explicit(value.try_into()?)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterValueDto {
    pub parameter_id: String,
    pub state: ParameterValueStateDto,
}

impl From<ParameterValue> for ParameterValueDto {
    fn from(value: ParameterValue) -> Self {
        Self {
            parameter_id: value.parameter_id.0,
            state: value.state.into(),
        }
    }
}

impl TryFrom<ParameterValueDto> for ParameterValue {
    type Error = ShellError;

    fn try_from(value: ParameterValueDto) -> Result<Self, Self::Error> {
        validate_identifier("parameter_id", &value.parameter_id)?;
        Ok(Self {
            parameter_id: value.parameter_id.into(),
            state: value.state.try_into()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityKeyInput {
    Streaming,
    Reasoning,
    PromptCaching,
    ToolCalling,
    ParallelToolCalling,
    StructuredOutput,
    JsonMode,
    ImageInput,
    AudioInput,
    AudioOutput,
    Logprobs,
    Seed,
    Batch,
    Background,
    ContextWindow,
    MaxOutputTokens,
}

impl From<CapabilityKeyInput> for CapabilityKey {
    fn from(value: CapabilityKeyInput) -> Self {
        match value {
            CapabilityKeyInput::Streaming => Self::Streaming,
            CapabilityKeyInput::Reasoning => Self::Reasoning,
            CapabilityKeyInput::PromptCaching => Self::PromptCaching,
            CapabilityKeyInput::ToolCalling => Self::ToolCalling,
            CapabilityKeyInput::ParallelToolCalling => Self::ParallelToolCalling,
            CapabilityKeyInput::StructuredOutput => Self::StructuredOutput,
            CapabilityKeyInput::JsonMode => Self::JsonMode,
            CapabilityKeyInput::ImageInput => Self::ImageInput,
            CapabilityKeyInput::AudioInput => Self::AudioInput,
            CapabilityKeyInput::AudioOutput => Self::AudioOutput,
            CapabilityKeyInput::Logprobs => Self::Logprobs,
            CapabilityKeyInput::Seed => Self::Seed,
            CapabilityKeyInput::Batch => Self::Batch,
            CapabilityKeyInput::Background => Self::Background,
            CapabilityKeyInput::ContextWindow => Self::ContextWindow,
            CapabilityKeyInput::MaxOutputTokens => Self::MaxOutputTokens,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum CapabilityValueDto {
    Boolean(bool),
    Integer(u64),
    EnumValues(Vec<String>),
    /// Bounded, normalized provider metadata from trusted Rust ingestion.
    ///
    /// This read-only variant is intentionally absent from
    /// [`CapabilityOverrideValueInput`].
    Structured(serde_json::Value),
}

impl From<CapabilityValue> for CapabilityValueDto {
    fn from(value: CapabilityValue) -> Self {
        match value {
            CapabilityValue::Boolean(value) => Self::Boolean(value),
            CapabilityValue::Integer(value) => Self::Integer(value),
            CapabilityValue::EnumValues(values) => Self::EnumValues(values),
            CapabilityValue::Structured(value) => Self::Structured(value),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum CapabilityOverrideValueInput {
    Boolean(bool),
    Integer(u64),
    EnumValues(Vec<String>),
}

impl TryFrom<CapabilityOverrideValueInput> for CapabilityValue {
    type Error = ShellError;

    fn try_from(value: CapabilityOverrideValueInput) -> Result<Self, Self::Error> {
        match value {
            CapabilityOverrideValueInput::Boolean(value) => Ok(Self::Boolean(value)),
            CapabilityOverrideValueInput::Integer(value) => Ok(Self::Integer(value)),
            CapabilityOverrideValueInput::EnumValues(values)
                if !values.is_empty() && values.iter().all(|value| !value.trim().is_empty()) =>
            {
                Ok(Self::EnumValues(values))
            }
            CapabilityOverrideValueInput::EnumValues(_) => Err(shell_invalid(
                "enum capability values must contain non-empty entries",
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityOverrideStatusInput {
    Verified,
    Unsupported,
    Unknown,
    Conditional,
}

impl From<CapabilityOverrideStatusInput> for SupportStatus {
    fn from(value: CapabilityOverrideStatusInput) -> Self {
        match value {
            CapabilityOverrideStatusInput::Verified => Self::Verified,
            CapabilityOverrideStatusInput::Unsupported => Self::Unsupported,
            CapabilityOverrideStatusInput::Unknown => Self::Unknown,
            CapabilityOverrideStatusInput::Conditional => Self::Conditional,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpsertCapabilityOverrideInput {
    pub id: String,
    pub model_route_id: String,
    pub key: CapabilityKeyInput,
    pub value: CapabilityOverrideValueInput,
    pub status: CapabilityOverrideStatusInput,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityObservationDto {
    pub id: String,
    pub model_route_id: String,
    pub key: String,
    pub value: CapabilityValueDto,
    pub status: String,
    pub source: String,
    pub confidence: String,
    pub observed_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    /// Logical evidence identity only; raw evidence never crosses this DTO.
    pub evidence_ref: Option<String>,
}

impl From<CapabilityObservation> for CapabilityObservationDto {
    fn from(value: CapabilityObservation) -> Self {
        Self {
            id: value.id.0,
            model_route_id: value.model_route_id.0,
            key: capability_key_name(value.key).to_owned(),
            value: value.value.into(),
            status: support_status_name(value.status).to_owned(),
            source: observation_source_name(value.source).to_owned(),
            confidence: confidence_name(value.confidence).to_owned(),
            observed_at: value.observed_at,
            expires_at: value.expires_at,
            evidence_ref: value.evidence_ref.map(|id| id.0),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveCapabilityDto {
    pub selected: CapabilityObservationDto,
    pub alternatives: Vec<CapabilityObservationDto>,
    pub evaluated_at: DateTime<Utc>,
    pub selected_is_stale: bool,
    pub has_conflict: bool,
}

impl From<EffectiveCapability> for EffectiveCapabilityDto {
    fn from(value: EffectiveCapability) -> Self {
        Self {
            selected: value.selected.into(),
            alternatives: value.alternatives.into_iter().map(Into::into).collect(),
            evaluated_at: value.evaluated_at,
            selected_is_stale: value.selected_is_stale,
            has_conflict: value.has_conflict,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationReasoningSettingsDto {
    pub mode: String,
    pub effort: Option<String>,
    pub budget_tokens: Option<u32>,
    pub summary: String,
    pub preserve_opaque_state: bool,
}

impl From<GenerationReasoningSettings> for GenerationReasoningSettingsDto {
    fn from(value: GenerationReasoningSettings) -> Self {
        Self {
            mode: reasoning_mode_name(value.mode).to_owned(),
            effort: value.effort.map(reasoning_effort_name).map(str::to_owned),
            budget_tokens: value.budget_tokens,
            summary: reasoning_summary_name(value.summary).to_owned(),
            preserve_opaque_state: value.preserve_opaque_state,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptCacheSettingsDto {
    pub mode: String,
    pub ttl_kind: String,
    pub ttl_seconds: Option<u32>,
    pub context_reference: Option<String>,
}

impl From<GenerationPromptCacheSettings> for PromptCacheSettingsDto {
    fn from(value: GenerationPromptCacheSettings) -> Self {
        let (ttl_kind, ttl_seconds) = prompt_cache_ttl(value.ttl);
        Self {
            mode: prompt_cache_mode_name(value.mode).to_owned(),
            ttl_kind: ttl_kind.to_owned(),
            ttl_seconds,
            context_reference: value.context_reference,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationPresetDto {
    pub id: String,
    pub model_route_id: String,
    pub display_name: String,
    pub values: Vec<ParameterValueDto>,
    pub reasoning: GenerationReasoningSettingsDto,
    pub prompt_cache: PromptCacheSettingsDto,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<GenerationPreset> for GenerationPresetDto {
    fn from(value: GenerationPreset) -> Self {
        Self {
            id: value.id.0,
            model_route_id: value.model_route_id.0,
            display_name: value.display_name,
            values: value.values.into_iter().map(Into::into).collect(),
            reasoning: value.reasoning.into(),
            prompt_cache: value.prompt_cache.into(),
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationPresetInput {
    pub id: String,
    pub model_route_id: String,
    pub display_name: String,
    pub values: Vec<ParameterValueDto>,
    pub reasoning: GenerationReasoningSettingsDto,
    pub prompt_cache: PromptCacheSettingsDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterIssueDto {
    pub code: String,
    pub parameter_id: Option<String>,
    pub related_parameter_id: Option<String>,
}

impl From<ParameterIssue> for ParameterIssueDto {
    fn from(value: ParameterIssue) -> Self {
        Self {
            code: parameter_issue_code_name(value.code).to_owned(),
            parameter_id: value.parameter_id.map(|id| id.0),
            related_parameter_id: value.related_parameter_id.map(|id| id.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenBudgetBoundsDto {
    pub minimum: u32,
    pub maximum: u32,
}

impl From<TokenBudgetBounds> for TokenBudgetBoundsDto {
    fn from(value: TokenBudgetBounds) -> Self {
        Self {
            minimum: value.minimum,
            maximum: value.maximum,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheTtlBoundsDto {
    pub minimum_seconds: u32,
    pub maximum_seconds: u32,
}

impl From<CacheTtlBounds> for CacheTtlBoundsDto {
    fn from(value: CacheTtlBounds) -> Self {
        Self {
            minimum_seconds: value.minimum_seconds,
            maximum_seconds: value.maximum_seconds,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReasoningControlDto {
    pub state: String,
    pub settings: GenerationReasoningSettingsDto,
    pub allowed_modes: Vec<String>,
    pub allowed_efforts: Vec<String>,
    pub allowed_summaries: Vec<String>,
    pub budget_bounds: Option<TokenBudgetBoundsDto>,
    pub effort_field: String,
    pub budget_field: String,
    pub summary_field: String,
    pub issues: Vec<ParameterIssueDto>,
}

impl From<ReasoningControlModel> for ReasoningControlDto {
    fn from(value: ReasoningControlModel) -> Self {
        Self {
            state: ui_control_state_name(value.state).to_owned(),
            settings: reasoning_settings_dto(value.settings),
            allowed_modes: value
                .allowed_modes
                .into_iter()
                .map(reasoning_control_mode_name)
                .map(str::to_owned)
                .collect(),
            allowed_efforts: value
                .allowed_efforts
                .into_iter()
                .map(reasoning_control_effort_name)
                .map(str::to_owned)
                .collect(),
            allowed_summaries: value
                .allowed_summaries
                .into_iter()
                .map(reasoning_control_summary_name)
                .map(str::to_owned)
                .collect(),
            budget_bounds: value.budget_bounds.map(Into::into),
            effort_field: ui_field_state_name(value.effort_field).to_owned(),
            budget_field: ui_field_state_name(value.budget_field).to_owned(),
            summary_field: ui_field_state_name(value.summary_field).to_owned(),
            issues: value.issues.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptCacheControlDto {
    pub state: String,
    pub settings: PromptCacheSettingsDto,
    pub allowed_modes: Vec<String>,
    pub allowed_ttls: Vec<PromptCacheTtlDto>,
    pub supports_custom_ttl: bool,
    pub custom_ttl_bounds: Option<CacheTtlBoundsDto>,
    pub ttl_field: String,
    pub context_reference_field: String,
    pub issues: Vec<ParameterIssueDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "seconds", rename_all = "snake_case")]
pub enum PromptCacheTtlDto {
    ProviderDefault,
    Short,
    Long,
    CustomSeconds(u32),
}

impl From<PromptCacheTtl> for PromptCacheTtlDto {
    fn from(value: PromptCacheTtl) -> Self {
        match value {
            PromptCacheTtl::ProviderDefault => Self::ProviderDefault,
            PromptCacheTtl::Short => Self::Short,
            PromptCacheTtl::Long => Self::Long,
            PromptCacheTtl::CustomSeconds(seconds) => Self::CustomSeconds(seconds),
        }
    }
}

impl From<PromptCacheControlModel> for PromptCacheControlDto {
    fn from(value: PromptCacheControlModel) -> Self {
        Self {
            state: ui_control_state_name(value.state).to_owned(),
            settings: prompt_cache_settings_dto(value.settings),
            allowed_modes: value
                .allowed_modes
                .into_iter()
                .map(prompt_cache_control_mode_name)
                .map(str::to_owned)
                .collect(),
            allowed_ttls: value.allowed_ttls.into_iter().map(Into::into).collect(),
            supports_custom_ttl: value.supports_custom_ttl,
            custom_ttl_bounds: value.custom_ttl_bounds.map(Into::into),
            ttl_field: ui_field_state_name(value.ttl_field).to_owned(),
            context_reference_field: ui_field_state_name(value.context_reference_field).to_owned(),
            issues: value.issues.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderProfileDto {
    pub id: String,
    pub display_name: String,
    pub base_url: String,
    pub model: String,
    pub timeout_seconds: u32,
}

impl From<ProviderProfile> for ProviderProfileDto {
    fn from(value: ProviderProfile) -> Self {
        Self {
            id: value.id,
            display_name: value.display_name,
            base_url: value.base_url,
            model: value.model,
            timeout_seconds: value.timeout_seconds,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RequestBodyShapeDto {
    Null,
    Boolean,
    Number,
    String,
    Array {
        items: Vec<Self>,
        truncated: bool,
    },
    Object {
        fields: Vec<RequestBodyFieldDto>,
        truncated: bool,
    },
    Redacted,
    Truncated,
}

impl From<&RequestBodyShape> for RequestBodyShapeDto {
    fn from(value: &RequestBodyShape) -> Self {
        match value {
            RequestBodyShape::Null => Self::Null,
            RequestBodyShape::Boolean => Self::Boolean,
            RequestBodyShape::Number => Self::Number,
            RequestBodyShape::String => Self::String,
            RequestBodyShape::Array { items, truncated } => Self::Array {
                items: items.iter().map(Into::into).collect(),
                truncated: *truncated,
            },
            RequestBodyShape::Object { fields, truncated } => Self::Object {
                fields: fields
                    .iter()
                    .map(|field| RequestBodyFieldDto {
                        name: field.name().to_owned(),
                        shape: field.shape().into(),
                    })
                    .collect(),
                truncated: *truncated,
            },
            RequestBodyShape::Redacted => Self::Redacted,
            RequestBodyShape::Truncated => Self::Truncated,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestBodyFieldDto {
    pub name: String,
    pub shape: RequestBodyShapeDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestPreviewDto {
    pub method: String,
    pub origin: String,
    pub path: String,
    pub query_parameter_names: Vec<String>,
    pub header_names: Vec<String>,
    pub body: Option<RequestBodyShapeDto>,
    pub body_truncated: bool,
}

impl From<RequestPreview> for RequestPreviewDto {
    fn from(value: RequestPreview) -> Self {
        Self {
            method: http_method_name(value.method()).to_owned(),
            origin: value.origin().to_string(),
            path: value.path().as_str().to_owned(),
            query_parameter_names: value.query_parameter_names().to_vec(),
            header_names: value
                .header_names()
                .iter()
                .map(|name| name.as_str().to_owned())
                .collect(),
            body: value.body().map(Into::into),
            body_truncated: value.body_truncated(),
        }
    }
}

impl ShellApi {
    pub fn get_settings(&self) -> ShellResult<AppSettingsDto> {
        self.core
            .get_settings()
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn update_settings(&self, settings: AppSettingsDto) -> ShellResult<AppSettingsDto> {
        validate_optional_provider_identifier(
            "selected_provider_profile_id",
            settings.selected_provider_profile_id.as_deref(),
        )?;
        validate_optional_provider_identifier(
            "selected_model_route_id",
            settings.selected_model_route_id.as_deref(),
        )?;
        validate_optional_provider_identifier(
            "selected_generation_preset_id",
            settings.selected_generation_preset_id.as_deref(),
        )?;
        self.core
            .update_settings(&AppSettings::from(settings))
            .map_err(ShellError::from)?;
        self.get_settings()
    }

    pub fn select_generation_target(
        &self,
        target: Option<GenerationTargetDto>,
    ) -> ShellResult<AppSettingsDto> {
        if let Some(target) = &target {
            validate_identifier("model_route_id", &target.model_route_id)?;
            validate_identifier("generation_preset_id", &target.generation_preset_id)?;
        }
        self.core
            .select_generation_target(target.map(Into::into))
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn list_provider_templates(&self) -> ShellResult<Vec<ProviderTemplateDto>> {
        self.core
            .list_provider_template_views()
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(ShellError::from)
    }

    pub fn list_provider_connections(&self) -> ShellResult<Vec<ProviderConnectionDto>> {
        self.core
            .list_provider_connections()
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(ShellError::from)
    }

    pub fn create_provider_connection(
        &self,
        input: CreateProviderConnectionInput,
    ) -> ShellResult<ProviderConnectionDto> {
        validate_identifier("connection_id", &input.id)?;
        validate_identifier("template_id", &input.template_id)?;
        let api_origin = parse_origin("api_origin", &input.api_origin)?;
        let local_network_approval = input
            .local_network_approval
            .map(parse_local_network_approval)
            .transpose()?;
        let approved_credential_origin = input
            .approved_credential_origin
            .map(|value| parse_origin("approved_credential_origin", &value))
            .transpose()?;
        let draft = ProviderConnectionDraft {
            id: ProviderConnectionId::from(input.id),
            template_id: ProviderTemplateId::from(input.template_id),
            template_version: input.template_version,
            display_name: input.display_name,
            api_origin,
            api_base_path: input
                .api_base_path
                .map(|path| parse_endpoint_path("api_base_path", &path))
                .transpose()?,
            network_mode: input.network_mode.into(),
            local_network_approval,
            values: input.values.into_iter().map(Into::into).collect(),
            approved_credential_origin,
            timeout_seconds: input.timeout_seconds,
        };
        self.core
            .create_provider_connection(draft)
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn upsert_provider_connection(
        &self,
        input: UpdateProviderConnectionInput,
    ) -> ShellResult<ProviderConnectionDto> {
        validate_identifier("connection_id", &input.id)?;
        let mut connection = self
            .core
            .list_provider_connections()
            .map_err(ShellError::from)?
            .into_iter()
            .find(|connection| connection.id.as_str() == input.id)
            .ok_or_else(|| shell_invalid("provider connection does not exist"))?;
        connection.display_name = input.display_name;
        connection.timeout_seconds = input.timeout_seconds;
        self.core
            .upsert_provider_connection(connection)
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn delete_provider_connection(&self, connection_id: &str) -> ShellResult<()> {
        validate_identifier("connection_id", connection_id)?;
        self.core
            .delete_provider_connection(&ProviderConnectionId::from(connection_id))
            .map_err(ShellError::from)
    }

    pub fn prepare_provider_credential_operation(
        &self,
        connection_id: &str,
        kind: ProviderCredentialOperationKindInput,
        preflight_status: ProviderCredentialSlotStatusInput,
    ) -> ShellResult<ProviderCredentialOperationContext> {
        validate_identifier("connection_id", connection_id)?;
        if kind == ProviderCredentialOperationKindInput::Install {
            return Err(shell_invalid(
                "generic provider credential installation is disabled; propose an authority and use the dedicated install preparation path",
            ));
        }
        self.core
            .prepare_provider_credential_operation(
                &ProviderConnectionId::from(connection_id),
                kind.into(),
                preflight_status.into(),
            )
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn propose_provider_credential_install_authority(
        &self,
        connection_id: &str,
    ) -> ShellResult<ProviderCredentialAccessAuthorityContext> {
        validate_identifier("connection_id", connection_id)?;
        self.core
            .propose_provider_credential_install_authority(&ProviderConnectionId::from(
                connection_id,
            ))
            .map(|authority| ProviderCredentialAccessAuthorityContext {
                authority_id: authority.authority_id,
                connection_binding_sha256: authority.connection_binding_sha256,
            })
            .map_err(ShellError::from)
    }

    pub fn prepare_provider_credential_install_operation(
        &self,
        connection_id: &str,
        authority: &ProviderCredentialAccessAuthorityContext,
        preflight_status: ProviderCredentialSlotStatusInput,
    ) -> ShellResult<ProviderCredentialOperationContext> {
        validate_identifier("connection_id", connection_id)?;
        validate_identifier("credential_authority_id", &authority.authority_id)?;
        validate_sha256(
            "credential_authority_binding_sha256",
            &authority.connection_binding_sha256,
        )?;
        self.core
            .prepare_provider_credential_install_operation(
                &ProviderConnectionId::from(connection_id),
                &lorepia_core::ProviderCredentialAccessAuthority {
                    authority_id: authority.authority_id.clone(),
                    connection_binding_sha256: authority.connection_binding_sha256.clone(),
                },
                preflight_status.into(),
            )
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn start_provider_credential_operation(
        &self,
        operation_id: &str,
        plan_sha256: &str,
    ) -> ShellResult<ProviderCredentialOperationContext> {
        validate_identifier("credential_operation_id", operation_id)?;
        validate_sha256("credential_operation_plan_sha256", plan_sha256)?;
        self.core
            .start_provider_credential_operation(operation_id, plan_sha256)
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn attest_provider_credential_predecessor_delete_intent(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        observed_status: ProviderCredentialSlotStatusInput,
    ) -> ShellResult<ProviderCredentialOperationContext> {
        validate_identifier("credential_operation_id", operation_id)?;
        validate_sha256("credential_operation_plan_sha256", plan_sha256)?;
        self.core
            .attest_provider_credential_predecessor_delete_intent(
                operation_id,
                plan_sha256,
                observed_status.into(),
            )
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn attest_provider_credential_predecessor_missing(
        &self,
        operation_id: &str,
        plan_sha256: &str,
    ) -> ShellResult<ProviderCredentialOperationContext> {
        validate_identifier("credential_operation_id", operation_id)?;
        validate_sha256("credential_operation_plan_sha256", plan_sha256)?;
        self.core
            .attest_provider_credential_predecessor_missing(operation_id, plan_sha256)
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn finish_provider_credential_operation(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        observed_status: ProviderCredentialSlotStatusInput,
    ) -> ShellResult<ProviderCredentialOperationContext> {
        validate_identifier("credential_operation_id", operation_id)?;
        validate_sha256("credential_operation_plan_sha256", plan_sha256)?;
        self.core
            .finish_provider_credential_operation(operation_id, plan_sha256, observed_status.into())
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn finish_provider_credential_archive(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        observed_status: ProviderCredentialSlotStatusInput,
    ) -> ShellResult<ProviderCredentialOperationContext> {
        validate_identifier("credential_operation_id", operation_id)?;
        validate_sha256("credential_operation_plan_sha256", plan_sha256)?;
        self.core
            .finish_provider_credential_archive(operation_id, plan_sha256, observed_status.into())
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn reconcile_provider_credential_operation(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        observed_status: ProviderCredentialSlotStatusInput,
    ) -> ShellResult<ProviderCredentialOperationContext> {
        validate_identifier("credential_operation_id", operation_id)?;
        validate_sha256("credential_operation_plan_sha256", plan_sha256)?;
        self.core
            .reconcile_provider_credential_operation(
                operation_id,
                plan_sha256,
                observed_status.into(),
            )
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn reconcile_provider_credential_archive(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        observed_status: ProviderCredentialSlotStatusInput,
    ) -> ShellResult<ProviderCredentialOperationContext> {
        validate_identifier("credential_operation_id", operation_id)?;
        validate_sha256("credential_operation_plan_sha256", plan_sha256)?;
        self.core
            .reconcile_provider_credential_archive(
                operation_id,
                plan_sha256,
                observed_status.into(),
            )
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn mark_provider_credential_cleanup_required(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        observed_status: ProviderCredentialSlotStatusInput,
        archive_connection: bool,
    ) -> ShellResult<ProviderCredentialOperationContext> {
        validate_identifier("credential_operation_id", operation_id)?;
        validate_sha256("credential_operation_plan_sha256", plan_sha256)?;
        self.core
            .mark_provider_credential_cleanup_required(
                operation_id,
                plan_sha256,
                observed_status.into(),
                archive_connection,
            )
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn mark_provider_credential_durability_recovery_required(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        archive_connection: bool,
    ) -> ShellResult<ProviderCredentialOperationContext> {
        validate_identifier("credential_operation_id", operation_id)?;
        validate_sha256("credential_operation_plan_sha256", plan_sha256)?;
        self.core
            .mark_provider_credential_durability_recovery_required(
                operation_id,
                plan_sha256,
                archive_connection,
            )
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn fence_started_provider_credential_operation_for_recovery(
        &self,
        operation_id: &str,
        plan_sha256: &str,
    ) -> ShellResult<ProviderCredentialOperationContext> {
        validate_identifier("credential_operation_id", operation_id)?;
        validate_sha256("credential_operation_plan_sha256", plan_sha256)?;
        self.core
            .fence_started_provider_credential_operation_for_recovery(operation_id, plan_sha256)
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn mark_provider_credential_predecessor_durability_recovery_required(
        &self,
        operation_id: &str,
        plan_sha256: &str,
        archive_connection: bool,
    ) -> ShellResult<ProviderCredentialOperationContext> {
        validate_identifier("credential_operation_id", operation_id)?;
        validate_sha256("credential_operation_plan_sha256", plan_sha256)?;
        self.core
            .mark_provider_credential_predecessor_durability_recovery_required(
                operation_id,
                plan_sha256,
                archive_connection,
            )
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn attest_provider_credential_durability_repaired(
        &self,
        operation_id: &str,
        plan_sha256: &str,
    ) -> ShellResult<ProviderCredentialOperationContext> {
        validate_identifier("credential_operation_id", operation_id)?;
        validate_sha256("credential_operation_plan_sha256", plan_sha256)?;
        self.core
            .attest_provider_credential_durability_repaired(operation_id, plan_sha256)
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn attest_provider_credential_predecessor_durability_repaired(
        &self,
        operation_id: &str,
        plan_sha256: &str,
    ) -> ShellResult<ProviderCredentialOperationContext> {
        validate_identifier("credential_operation_id", operation_id)?;
        validate_sha256("credential_operation_plan_sha256", plan_sha256)?;
        self.core
            .attest_provider_credential_predecessor_durability_repaired(operation_id, plan_sha256)
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn list_unresolved_provider_credential_operations(
        &self,
    ) -> ShellResult<Vec<ProviderCredentialOperationContext>> {
        self.core
            .list_unresolved_provider_credential_operations()
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(ShellError::from)
    }

    pub fn list_provider_credential_slot_garbage(
        &self,
    ) -> ShellResult<Vec<ProviderCredentialSlotGarbageContext>> {
        self.core
            .list_provider_credential_slot_garbage()
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(ShellError::from)
    }

    pub fn observe_provider_credential_slot_garbage(
        &self,
        connection_id: &str,
        authority_sequence: u64,
        observed_status: ProviderCredentialSlotStatusInput,
    ) -> ShellResult<ProviderCredentialSlotGarbageContext> {
        validate_identifier("connection_id", connection_id)?;
        if authority_sequence == 0 {
            return Err(shell_invalid(
                "credential slot gc authority sequence must be positive",
            ));
        }
        self.core
            .observe_provider_credential_slot_garbage(
                &ProviderConnectionId::from(connection_id),
                authority_sequence,
                observed_status.into(),
            )
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn ensure_provider_credential_access_settled(
        &self,
        connection_id: &str,
    ) -> ShellResult<ProviderCredentialAccessAuthorityContext> {
        validate_identifier("connection_id", connection_id)?;
        self.core
            .ensure_provider_credential_access_settled(&ProviderConnectionId::from(connection_id))
            .map(|authority| ProviderCredentialAccessAuthorityContext {
                authority_id: authority.authority_id,
                connection_binding_sha256: authority.connection_binding_sha256,
            })
            .map_err(ShellError::from)
    }

    pub fn ensure_legacy_profile_raw_credential_access(
        &self,
        provider_profile_id: &str,
    ) -> ShellResult<()> {
        validate_identifier("provider_profile_id", provider_profile_id)?;
        self.core
            .ensure_legacy_profile_raw_credential_access(provider_profile_id)
            .map_err(ShellError::from)
    }

    pub fn ensure_legacy_profile_credential_mutation_settled(
        &self,
        provider_profile_id: &str,
    ) -> ShellResult<()> {
        validate_identifier("provider_profile_id", provider_profile_id)?;
        self.core
            .ensure_legacy_profile_credential_mutation_settled(provider_profile_id)
            .map_err(ShellError::from)
    }

    pub fn provider_connection_uses_legacy_raw_credential(
        &self,
        connection_id: &str,
    ) -> ShellResult<bool> {
        validate_identifier("connection_id", connection_id)?;
        self.core
            .provider_connection_uses_legacy_raw_credential(&ProviderConnectionId::from(
                connection_id,
            ))
            .map_err(ShellError::from)
    }

    pub fn provider_credential_recovery_authority(
        &self,
        connection_id: &str,
    ) -> ShellResult<Option<ProviderCredentialAccessAuthorityContext>> {
        validate_identifier("connection_id", connection_id)?;
        self.core
            .provider_credential_recovery_authority(&ProviderConnectionId::from(connection_id))
            .map(|authority| {
                authority.map(|authority| ProviderCredentialAccessAuthorityContext {
                    authority_id: authority.authority_id,
                    connection_binding_sha256: authority.connection_binding_sha256,
                })
            })
            .map_err(ShellError::from)
    }

    pub fn list_provider_profiles(&self) -> ShellResult<Vec<ProviderProfileDto>> {
        self.core
            .list_provider_profiles()
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(ShellError::from)
    }

    pub fn list_model_routes(&self, connection_id: &str) -> ShellResult<Vec<ModelRouteDto>> {
        validate_identifier("connection_id", connection_id)?;
        self.core
            .list_model_routes(&ProviderConnectionId::from(connection_id))
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(ShellError::from)
    }

    pub fn upsert_model_route(&self, input: UpsertModelRouteInput) -> ShellResult<ModelRouteDto> {
        let route = match input {
            UpsertModelRouteInput::Create {
                id,
                connection_id,
                api_family,
                model_id,
                display_name,
                route_config,
                status,
            } => {
                validate_identifier("model_route_id", &id)?;
                validate_identifier("connection_id", &connection_id)?;
                let now = Utc::now();
                ModelRoute {
                    id: ModelRouteId::from(id),
                    connection_id: ProviderConnectionId::from(connection_id),
                    api_family: api_family.into(),
                    model_id,
                    display_name,
                    route_config: route_config.try_into()?,
                    status: status.into(),
                    miss_count: 0,
                    raw_metadata: None,
                    metadata_source: ModelMetadataSource::UserOverride,
                    metadata_observed_at: None,
                    last_reconciled_sync_job_id: None,
                    metadata_sync_job_id: None,
                    first_seen_at: now,
                    last_seen_at: Some(now),
                }
            }
            UpsertModelRouteInput::Update {
                id,
                display_name,
                status,
            } => {
                validate_identifier("model_route_id", &id)?;
                let mut route = self.find_model_route(&id)?;
                route.display_name = display_name;
                route.status = status.into();
                route
            }
        };
        self.core
            .upsert_model_route(route)
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn delete_model_route(&self, model_route_id: &str) -> ShellResult<()> {
        validate_identifier("model_route_id", model_route_id)?;
        self.core
            .delete_model_route(&ModelRouteId::from(model_route_id))
            .map_err(ShellError::from)
    }

    pub fn list_capability_observations(
        &self,
        model_route_id: &str,
    ) -> ShellResult<Vec<CapabilityObservationDto>> {
        validate_identifier("model_route_id", model_route_id)?;
        self.core
            .list_capability_observations(&ModelRouteId::from(model_route_id))
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(ShellError::from)
    }

    pub fn effective_capability(
        &self,
        model_route_id: &str,
        key: CapabilityKeyInput,
    ) -> ShellResult<Option<EffectiveCapabilityDto>> {
        validate_identifier("model_route_id", model_route_id)?;
        self.core
            .effective_capability(&ModelRouteId::from(model_route_id), key.into())
            .map(|value| value.map(Into::into))
            .map_err(ShellError::from)
    }

    pub fn effective_parameter_specs(
        &self,
        model_route_id: &str,
    ) -> ShellResult<Vec<ParameterSpecDto>> {
        validate_identifier("model_route_id", model_route_id)?;
        self.core
            .effective_parameter_specs(&ModelRouteId::from(model_route_id))
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(ShellError::from)
    }

    pub fn upsert_user_capability_override(
        &self,
        input: UpsertCapabilityOverrideInput,
    ) -> ShellResult<CapabilityObservationDto> {
        validate_identifier("capability_observation_id", &input.id)?;
        validate_identifier("model_route_id", &input.model_route_id)?;
        let observation = CapabilityObservation {
            id: ObservationId::from(input.id),
            model_route_id: ModelRouteId::from(input.model_route_id),
            key: input.key.into(),
            value: input.value.try_into()?,
            status: input.status.into(),
            source: ObservationSource::UserOverride,
            confidence: Confidence::High,
            observed_at: Utc::now(),
            expires_at: input.expires_at,
            evidence_ref: None,
        };
        self.core
            .upsert_user_capability_override(observation)
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn delete_user_capability_override(
        &self,
        model_route_id: &str,
        observation_id: &str,
    ) -> ShellResult<()> {
        validate_identifier("model_route_id", model_route_id)?;
        validate_identifier("capability_observation_id", observation_id)?;
        self.core
            .delete_user_capability_override(
                &ModelRouteId::from(model_route_id),
                &ObservationId::from(observation_id),
            )
            .map_err(ShellError::from)
    }

    pub fn list_generation_presets(
        &self,
        model_route_id: &str,
    ) -> ShellResult<Vec<GenerationPresetDto>> {
        validate_identifier("model_route_id", model_route_id)?;
        self.core
            .list_generation_presets(&ModelRouteId::from(model_route_id))
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(ShellError::from)
    }

    pub fn upsert_generation_preset(
        &self,
        input: GenerationPresetInput,
    ) -> ShellResult<GenerationPresetDto> {
        let preset = self.generation_preset_candidate(input)?;
        self.core
            .upsert_generation_preset(preset)
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn delete_generation_preset(&self, generation_preset_id: &str) -> ShellResult<()> {
        validate_identifier("generation_preset_id", generation_preset_id)?;
        self.core
            .delete_generation_preset(&GenerationPresetId::from(generation_preset_id))
            .map_err(ShellError::from)
    }

    pub fn validate_generation_preset_candidate(
        &self,
        input: GenerationPresetInput,
    ) -> ShellResult<()> {
        let preset = self.generation_preset_candidate(input)?;
        self.core
            .validate_generation_preset_candidate(&preset)
            .map_err(ShellError::from)
    }

    pub fn render_reasoning_control_for_preset(
        &self,
        input: GenerationPresetInput,
    ) -> ShellResult<ReasoningControlDto> {
        let preset = self.generation_preset_candidate(input)?;
        self.core
            .render_reasoning_control_for_preset(&preset)
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn render_prompt_cache_control_for_preset(
        &self,
        input: GenerationPresetInput,
    ) -> ShellResult<PromptCacheControlDto> {
        let preset = self.generation_preset_candidate(input)?;
        self.core
            .render_prompt_cache_control_for_preset(&preset)
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn preview_provider_request_candidate(
        &self,
        input: GenerationPresetInput,
    ) -> ShellResult<RequestPreviewDto> {
        let preset = self.generation_preset_candidate(input)?;
        self.core
            .preview_provider_request_candidate(&preset)
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn preview_provider_request(
        &self,
        target: GenerationTargetDto,
    ) -> ShellResult<RequestPreviewDto> {
        validate_identifier("model_route_id", &target.model_route_id)?;
        validate_identifier("generation_preset_id", &target.generation_preset_id)?;
        let target = GenerationTarget::from(target);
        self.core
            .preview_provider_request(&target.model_route_id, &target.generation_preset_id)
            .map(Into::into)
            .map_err(ShellError::from)
    }

    fn find_model_route(&self, model_route_id: &str) -> ShellResult<ModelRoute> {
        for connection in self
            .core
            .list_provider_connections()
            .map_err(ShellError::from)?
        {
            if let Some(route) = self
                .core
                .list_model_routes(&connection.id)
                .map_err(ShellError::from)?
                .into_iter()
                .find(|route| route.id.as_str() == model_route_id)
            {
                return Ok(route);
            }
        }
        Err(shell_invalid("model route does not exist"))
    }

    fn generation_preset_candidate(
        &self,
        input: GenerationPresetInput,
    ) -> ShellResult<GenerationPreset> {
        validate_identifier("generation_preset_id", &input.id)?;
        validate_identifier("model_route_id", &input.model_route_id)?;
        let now = Utc::now();
        let created_at = self
            .core
            .list_generation_presets(&ModelRouteId::from(input.model_route_id.as_str()))
            .map_err(ShellError::from)?
            .into_iter()
            .find(|preset| preset.id.as_str() == input.id)
            .map_or(now, |preset| preset.created_at);
        Ok(GenerationPreset {
            id: GenerationPresetId::from(input.id),
            model_route_id: ModelRouteId::from(input.model_route_id),
            display_name: input.display_name,
            values: input
                .values
                .into_iter()
                .map(TryInto::try_into)
                .collect::<ShellResult<Vec<_>>>()?,
            reasoning: parse_reasoning_settings(input.reasoning)?,
            prompt_cache: parse_prompt_cache_settings(input.prompt_cache)?,
            created_at,
            updated_at: now,
        })
    }
}

fn validate_optional_provider_identifier(field: &str, value: Option<&str>) -> ShellResult<()> {
    value.map_or(Ok(()), |value| validate_identifier(field, value))
}

fn parse_origin(field: &str, value: &str) -> ShellResult<CanonicalOrigin> {
    CanonicalOrigin::parse(value).map_err(|_| shell_invalid(&format!("{field} is invalid")))
}

fn parse_endpoint_path(field: &str, value: &str) -> ShellResult<EndpointPath> {
    EndpointPath::parse(value).map_err(|_| shell_invalid(&format!("{field} is invalid")))
}

fn parse_local_network_approval(
    value: ProviderLocalNetworkApprovalInput,
) -> ShellResult<ProviderLocalNetworkApproval> {
    Ok(ProviderLocalNetworkApproval {
        origin: parse_origin("local_network_approval.origin", &value.origin)?,
        addresses: value
            .addresses
            .into_iter()
            .map(|address| {
                address.parse::<IpAddr>().map_err(|_| {
                    shell_invalid("local_network_approval.addresses contains an invalid address")
                })
            })
            .collect::<ShellResult<Vec<_>>>()?,
    })
}

fn parse_tool_policy(value: &str) -> ShellResult<ToolPolicy> {
    match value {
        "none" => Ok(ToolPolicy::None),
        "auto" => Ok(ToolPolicy::Auto),
        "required" => Ok(ToolPolicy::Required),
        _ => Err(shell_invalid("tool policy is invalid")),
    }
}

fn parse_reasoning_settings(
    value: GenerationReasoningSettingsDto,
) -> ShellResult<GenerationReasoningSettings> {
    Ok(GenerationReasoningSettings {
        mode: match value.mode.as_str() {
            "provider_default" => GenerationReasoningMode::ProviderDefault,
            "disabled" => GenerationReasoningMode::Disabled,
            "automatic" => GenerationReasoningMode::Automatic,
            "enabled" => GenerationReasoningMode::Enabled,
            _ => return Err(shell_invalid("reasoning mode is invalid")),
        },
        effort: value
            .effort
            .map(|effort| match effort.as_str() {
                "minimal" => Ok(GenerationReasoningEffort::Minimal),
                "low" => Ok(GenerationReasoningEffort::Low),
                "medium" => Ok(GenerationReasoningEffort::Medium),
                "high" => Ok(GenerationReasoningEffort::High),
                "extra_high" => Ok(GenerationReasoningEffort::ExtraHigh),
                "maximum" => Ok(GenerationReasoningEffort::Maximum),
                _ => Err(shell_invalid("reasoning effort is invalid")),
            })
            .transpose()?,
        budget_tokens: value.budget_tokens,
        summary: match value.summary.as_str() {
            "provider_default" => GenerationReasoningSummary::ProviderDefault,
            "disabled" => GenerationReasoningSummary::Disabled,
            "automatic" => GenerationReasoningSummary::Automatic,
            "concise" => GenerationReasoningSummary::Concise,
            "detailed" => GenerationReasoningSummary::Detailed,
            _ => return Err(shell_invalid("reasoning summary is invalid")),
        },
        preserve_opaque_state: value.preserve_opaque_state,
    })
}

fn parse_prompt_cache_settings(
    value: PromptCacheSettingsDto,
) -> ShellResult<GenerationPromptCacheSettings> {
    let ttl = match (value.ttl_kind.as_str(), value.ttl_seconds) {
        ("provider_default", None) => GenerationPromptCacheTtl::ProviderDefault,
        ("short", None) => GenerationPromptCacheTtl::Short,
        ("long", None) => GenerationPromptCacheTtl::Long,
        ("custom_seconds", Some(seconds)) => GenerationPromptCacheTtl::CustomSeconds(seconds),
        _ => return Err(shell_invalid("prompt cache TTL is invalid")),
    };
    Ok(GenerationPromptCacheSettings {
        mode: match value.mode.as_str() {
            "provider_default" => GenerationPromptCacheMode::ProviderDefault,
            "automatic" => GenerationPromptCacheMode::Automatic,
            "explicit_breakpoints" => GenerationPromptCacheMode::ExplicitBreakpoints,
            "explicit_context" => GenerationPromptCacheMode::ExplicitContext,
            "disabled_if_supported" => GenerationPromptCacheMode::DisabledIfSupported,
            _ => return Err(shell_invalid("prompt cache mode is invalid")),
        },
        ttl,
        context_reference: value.context_reference,
    })
}

fn reasoning_settings_dto(value: ReasoningSettings) -> GenerationReasoningSettingsDto {
    GenerationReasoningSettingsDto {
        mode: reasoning_control_mode_name(value.mode).to_owned(),
        effort: value
            .effort
            .map(reasoning_control_effort_name)
            .map(str::to_owned),
        budget_tokens: value.budget_tokens,
        summary: reasoning_control_summary_name(value.summary).to_owned(),
        preserve_opaque_state: value.preserve_opaque_state,
    }
}

fn prompt_cache_settings_dto(value: PromptCacheSettings) -> PromptCacheSettingsDto {
    let (ttl_kind, ttl_seconds) = match value.ttl {
        PromptCacheTtl::ProviderDefault => ("provider_default", None),
        PromptCacheTtl::Short => ("short", None),
        PromptCacheTtl::Long => ("long", None),
        PromptCacheTtl::CustomSeconds(seconds) => ("custom_seconds", Some(seconds)),
    };
    PromptCacheSettingsDto {
        mode: prompt_cache_control_mode_name(value.mode).to_owned(),
        ttl_kind: ttl_kind.to_owned(),
        ttl_seconds,
        context_reference: value.context_reference,
    }
}

fn validate_sha256(field: &str, value: &str) -> ShellResult<()> {
    if value.len() != 64
        || value
            .as_bytes()
            .iter()
            .any(|byte| !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase())
    {
        return Err(shell_invalid(field));
    }
    Ok(())
}

fn shell_invalid(message: &str) -> ShellError {
    lorepia_core::CoreError::invalid(message).into()
}

const fn connection_field_type_name(value: ConnectionFieldType) -> &'static str {
    match value {
        ConnectionFieldType::Text => "text",
        ConnectionFieldType::Integer => "integer",
        ConnectionFieldType::Boolean => "boolean",
        ConnectionFieldType::Credential => "credential",
    }
}

const fn credential_redirect_policy_name(value: CredentialRedirectPolicy) -> &'static str {
    match value {
        CredentialRedirectPolicy::Deny => "deny",
        CredentialRedirectPolicy::FollowWithoutCredential => "follow_without_credential",
    }
}

const fn parameter_type_name(value: ParameterType) -> &'static str {
    match value {
        ParameterType::Boolean => "boolean",
        ParameterType::Integer => "integer",
        ParameterType::Number => "number",
        ParameterType::String => "string",
        ParameterType::Enum => "enum",
        ParameterType::StringList => "string_list",
        ParameterType::JsonSchema => "json_schema",
        ParameterType::StopSequenceList => "stop_sequence_list",
        ParameterType::ToolPolicy => "tool_policy",
    }
}

const fn parameter_default_mode_name(value: ParameterDefaultMode) -> &'static str {
    match value {
        ParameterDefaultMode::ProviderDefault => "provider_default",
        ParameterDefaultMode::ExplicitRequired => "explicit_required",
    }
}

const fn parameter_condition_operator_name(value: ParameterConditionOperator) -> &'static str {
    match value {
        ParameterConditionOperator::Equals => "equals",
        ParameterConditionOperator::NotEquals => "not_equals",
    }
}

const fn parameter_conflict_kind_name(value: ParameterConflictKind) -> &'static str {
    match value {
        ParameterConflictKind::MutuallyExclusive => "mutually_exclusive",
        ParameterConflictKind::Requires => "requires",
    }
}

const fn provider_parameter_target_name(value: ProviderParameterTarget) -> &'static str {
    match value {
        ProviderParameterTarget::RequestBody => "request_body",
        ProviderParameterTarget::RequestHeader => "request_header",
    }
}

const fn ui_parameter_level_name(value: UiParameterLevel) -> &'static str {
    match value {
        UiParameterLevel::Basic => "basic",
        UiParameterLevel::Advanced => "advanced",
        UiParameterLevel::Expert => "expert",
        UiParameterLevel::HiddenInternal => "hidden_internal",
    }
}

const fn capability_key_name(value: CapabilityKey) -> &'static str {
    match value {
        CapabilityKey::Streaming => "streaming",
        CapabilityKey::Reasoning => "reasoning",
        CapabilityKey::PromptCaching => "prompt_caching",
        CapabilityKey::ToolCalling => "tool_calling",
        CapabilityKey::ParallelToolCalling => "parallel_tool_calling",
        CapabilityKey::StructuredOutput => "structured_output",
        CapabilityKey::JsonMode => "json_mode",
        CapabilityKey::ImageInput => "image_input",
        CapabilityKey::AudioInput => "audio_input",
        CapabilityKey::AudioOutput => "audio_output",
        CapabilityKey::Logprobs => "logprobs",
        CapabilityKey::Seed => "seed",
        CapabilityKey::Batch => "batch",
        CapabilityKey::Background => "background",
        CapabilityKey::ContextWindow => "context_window",
        CapabilityKey::MaxOutputTokens => "max_output_tokens",
    }
}

const fn support_status_name(value: SupportStatus) -> &'static str {
    match value {
        SupportStatus::Verified => "verified",
        SupportStatus::Documented => "documented",
        SupportStatus::Inferred => "inferred",
        SupportStatus::Unsupported => "unsupported",
        SupportStatus::Unknown => "unknown",
        SupportStatus::Conditional => "conditional",
    }
}

const fn observation_source_name(value: ObservationSource) -> &'static str {
    match value {
        ObservationSource::ProviderApi => "provider_api",
        ObservationSource::OfficialDocumentation => "official_documentation",
        ObservationSource::SignedLorepiaCatalog => "signed_lorepia_catalog",
        ObservationSource::CapabilityProbe => "capability_probe",
        ObservationSource::UserOverride => "user_override",
        ObservationSource::LlmInference => "llm_inference",
    }
}

const fn confidence_name(value: Confidence) -> &'static str {
    match value {
        Confidence::Low => "low",
        Confidence::Medium => "medium",
        Confidence::High => "high",
    }
}

const fn template_source_name(value: TemplateSource) -> &'static str {
    match value {
        TemplateSource::BuiltIn => "built_in",
        TemplateSource::SignedCatalog => "signed_catalog",
        TemplateSource::UserDiscovered => "user_discovered",
    }
}

const fn api_family_name(value: ApiFamily) -> &'static str {
    match value {
        ApiFamily::OpenAiResponses => "openai_responses",
        ApiFamily::OpenAiChatCompletions => "openai_chat_completions",
        ApiFamily::AnthropicMessages => "anthropic_messages",
        ApiFamily::GeminiGenerateContent => "gemini_generate_content",
        ApiFamily::OllamaNative => "ollama_native",
    }
}

const fn network_mode_name(value: ProviderNetworkMode) -> &'static str {
    match value {
        ProviderNetworkMode::Public => "public",
        ProviderNetworkMode::LocalLoopback => "local_loopback",
        ProviderNetworkMode::ApprovedLocalNetwork => "approved_local_network",
    }
}

const fn connection_status_name(value: ConnectionStatus) -> &'static str {
    match value {
        ConnectionStatus::Untested => "untested",
        ConnectionStatus::Connected => "connected",
        ConnectionStatus::AuthFailed => "auth_failed",
        ConnectionStatus::Unavailable => "unavailable",
    }
}

const fn model_availability_name(value: ModelAvailability) -> &'static str {
    match value {
        ModelAvailability::Available => "available",
        ModelAvailability::MissingTemporarily => "missing_temporarily",
        ModelAvailability::DocumentedOnly => "documented_only",
        ModelAvailability::AccessDenied => "access_denied",
        ModelAvailability::Deprecated => "deprecated",
        ModelAvailability::Retired => "retired",
        ModelAvailability::Unknown => "unknown",
    }
}

const fn metadata_source_name(value: ModelMetadataSource) -> &'static str {
    match value {
        ModelMetadataSource::Legacy => "legacy",
        ModelMetadataSource::ProviderApi => "provider_api",
        ModelMetadataSource::OfficialDocumentation => "official_documentation",
        ModelMetadataSource::SignedCatalog => "signed_catalog",
        ModelMetadataSource::CapabilityProbe => "capability_probe",
        ModelMetadataSource::UserOverride => "user_override",
    }
}

const fn tool_policy_name(value: ToolPolicy) -> &'static str {
    match value {
        ToolPolicy::None => "none",
        ToolPolicy::Auto => "auto",
        ToolPolicy::Required => "required",
    }
}

const fn reasoning_mode_name(value: GenerationReasoningMode) -> &'static str {
    match value {
        GenerationReasoningMode::ProviderDefault => "provider_default",
        GenerationReasoningMode::Disabled => "disabled",
        GenerationReasoningMode::Automatic => "automatic",
        GenerationReasoningMode::Enabled => "enabled",
    }
}

const fn reasoning_effort_name(value: GenerationReasoningEffort) -> &'static str {
    match value {
        GenerationReasoningEffort::Minimal => "minimal",
        GenerationReasoningEffort::Low => "low",
        GenerationReasoningEffort::Medium => "medium",
        GenerationReasoningEffort::High => "high",
        GenerationReasoningEffort::ExtraHigh => "extra_high",
        GenerationReasoningEffort::Maximum => "maximum",
    }
}

const fn reasoning_summary_name(value: GenerationReasoningSummary) -> &'static str {
    match value {
        GenerationReasoningSummary::ProviderDefault => "provider_default",
        GenerationReasoningSummary::Disabled => "disabled",
        GenerationReasoningSummary::Automatic => "automatic",
        GenerationReasoningSummary::Concise => "concise",
        GenerationReasoningSummary::Detailed => "detailed",
    }
}

const fn prompt_cache_mode_name(value: GenerationPromptCacheMode) -> &'static str {
    match value {
        GenerationPromptCacheMode::ProviderDefault => "provider_default",
        GenerationPromptCacheMode::Automatic => "automatic",
        GenerationPromptCacheMode::ExplicitBreakpoints => "explicit_breakpoints",
        GenerationPromptCacheMode::ExplicitContext => "explicit_context",
        GenerationPromptCacheMode::DisabledIfSupported => "disabled_if_supported",
    }
}

const fn prompt_cache_ttl(value: GenerationPromptCacheTtl) -> (&'static str, Option<u32>) {
    match value {
        GenerationPromptCacheTtl::ProviderDefault => ("provider_default", None),
        GenerationPromptCacheTtl::Short => ("short", None),
        GenerationPromptCacheTtl::Long => ("long", None),
        GenerationPromptCacheTtl::CustomSeconds(seconds) => ("custom_seconds", Some(seconds)),
    }
}

const fn http_method_name(value: HttpMethod) -> &'static str {
    match value {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
    }
}

const fn ui_control_state_name(value: UiControlState) -> &'static str {
    match value {
        UiControlState::Hidden => "hidden",
        UiControlState::Ready => "ready",
        UiControlState::Invalid => "invalid",
    }
}

const fn ui_field_state_name(value: UiFieldState) -> &'static str {
    match value {
        UiFieldState::Hidden => "hidden",
        UiFieldState::Enabled => "enabled",
        UiFieldState::Required => "required",
    }
}

const fn reasoning_control_mode_name(value: ReasoningMode) -> &'static str {
    match value {
        ReasoningMode::ProviderDefault => "provider_default",
        ReasoningMode::Disabled => "disabled",
        ReasoningMode::Automatic => "automatic",
        ReasoningMode::Enabled => "enabled",
    }
}

const fn reasoning_control_effort_name(value: ReasoningEffort) -> &'static str {
    match value {
        ReasoningEffort::Minimal => "minimal",
        ReasoningEffort::Low => "low",
        ReasoningEffort::Medium => "medium",
        ReasoningEffort::High => "high",
        ReasoningEffort::ExtraHigh => "extra_high",
        ReasoningEffort::Maximum => "maximum",
    }
}

const fn reasoning_control_summary_name(value: ReasoningSummaryMode) -> &'static str {
    match value {
        ReasoningSummaryMode::ProviderDefault => "provider_default",
        ReasoningSummaryMode::Disabled => "disabled",
        ReasoningSummaryMode::Automatic => "automatic",
        ReasoningSummaryMode::Concise => "concise",
        ReasoningSummaryMode::Detailed => "detailed",
    }
}

const fn prompt_cache_control_mode_name(value: PromptCacheMode) -> &'static str {
    match value {
        PromptCacheMode::ProviderDefault => "provider_default",
        PromptCacheMode::Automatic => "automatic",
        PromptCacheMode::ExplicitBreakpoints => "explicit_breakpoints",
        PromptCacheMode::ExplicitContext => "explicit_context",
        PromptCacheMode::DisabledIfSupported => "disabled_if_supported",
    }
}

const fn parameter_issue_code_name(value: ParameterIssueCode) -> &'static str {
    match value {
        ParameterIssueCode::InvalidDefinition => "invalid_definition",
        ParameterIssueCode::DuplicateParameter => "duplicate_parameter",
        ParameterIssueCode::UnknownParameter => "unknown_parameter",
        ParameterIssueCode::DuplicateValue => "duplicate_value",
        ParameterIssueCode::RequiredValueMissing => "required_value_missing",
        ParameterIssueCode::TypeMismatch => "type_mismatch",
        ParameterIssueCode::OutOfBounds => "out_of_bounds",
        ParameterIssueCode::InvalidStep => "invalid_step",
        ParameterIssueCode::InvalidChoice => "invalid_choice",
        ParameterIssueCode::InvalidJsonSchema => "invalid_json_schema",
        ParameterIssueCode::HiddenValue => "hidden_value",
        ParameterIssueCode::MutuallyExclusive => "mutually_exclusive",
        ParameterIssueCode::MissingRequirement => "missing_requirement",
        ParameterIssueCode::UnsupportedMapping => "unsupported_mapping",
        ParameterIssueCode::ConflictingRequestField => "conflicting_request_field",
        ParameterIssueCode::UnsupportedReasoning => "unsupported_reasoning",
        ParameterIssueCode::UnsupportedPromptCache => "unsupported_prompt_cache",
        ParameterIssueCode::InvalidPromptCacheReference => "invalid_prompt_cache_reference",
    }
}

#[cfg(test)]
mod tests {
    use lorepia_core::{
        ApiFamily, AuthBinding, CanonicalOrigin, CoreConfig, CredentialRedirectPolicy,
        CredentialRef, CredentialScope, ProviderConnectionId, ProviderLocalNetworkApproval,
        ProviderNetworkMode,
    };
    use tempfile::tempdir;

    use crate::{
        ApiFamilyInput, AppSettingsDto, CapabilityKeyInput, CapabilityOverrideStatusInput,
        CapabilityOverrideValueInput, CreateProviderConnectionInput, GenerationPresetInput,
        GenerationReasoningSettingsDto, GenerationTargetDto, ModelAvailabilityInput,
        ModelRouteConfigDto, PromptCacheSettingsDto, ProviderConnectionDto,
        ProviderCredentialOperationKindInput, ProviderCredentialSlotStatusInput,
        ProviderNetworkModeInput, ShellApi, UpdateProviderConnectionInput,
        UpsertCapabilityOverrideInput, UpsertModelRouteInput,
    };

    #[test]
    fn empty_provider_read_models_are_real_core_results() {
        let root = tempdir().expect("temporary root");
        let shell = ShellApi::open(CoreConfig::new(root.path())).expect("shell");

        assert!(
            !shell
                .list_provider_templates()
                .expect("templates")
                .is_empty()
        );
        assert!(
            shell
                .list_provider_connections()
                .expect("connections")
                .is_empty()
        );
        assert!(shell.list_provider_profiles().expect("profiles").is_empty());
    }

    #[test]
    fn provider_connection_projection_never_contains_credential_reference() {
        let canary = "credential-reference-canary";
        let mut connection = synthetic_connection();
        connection.credential_ref = Some(CredentialRef(canary.to_owned()));
        connection.config.network_mode = ProviderNetworkMode::ApprovedLocalNetwork;
        connection.config.local_network_approval = Some(ProviderLocalNetworkApproval {
            origin: CanonicalOrigin::parse("http://192.168.1.20:11434").expect("LAN origin"),
            addresses: vec!["192.168.1.20".parse().expect("LAN address")],
        });
        connection.credential_scope = Some(CredentialScope {
            allowed_origins: vec![
                CanonicalOrigin::parse("https://example.test").expect("credential origin"),
            ],
            auth_binding: AuthBinding::BearerHeader,
            redirect_policy: CredentialRedirectPolicy::Deny,
        });
        let dto = ProviderConnectionDto::from(connection);
        let json = serde_json::to_string(&dto).expect("serialize");

        assert!(dto.credential_binding_required);
        assert_eq!(
            dto.local_network_approval
                .as_ref()
                .map(|approval| approval.addresses.as_slice()),
            Some(["192.168.1.20".to_owned()].as_slice())
        );
        assert_eq!(
            dto.approved_credential_origins,
            ["https://example.test".to_owned()]
        );
        assert_eq!(
            dto.credential_scope
                .as_ref()
                .map(|scope| scope.redirect_policy.as_str()),
            Some("deny")
        );
        assert!(!json.contains(canary));
        assert!(!json.contains("credential_ref"));
    }

    #[test]
    fn generic_install_fails_closed_before_journaling() {
        let root = tempdir().expect("temporary root");
        let shell = ShellApi::open(CoreConfig::new(root.path())).expect("shell");
        let connection_id = create_shell_credential_connection(&shell);

        shell
            .prepare_provider_credential_operation(
                &connection_id,
                ProviderCredentialOperationKindInput::Install,
                ProviderCredentialSlotStatusInput::Missing,
            )
            .expect_err("generic Shell install must require physical-slot authority");
        assert!(
            shell
                .list_unresolved_provider_credential_operations()
                .expect("list credential journal")
                .is_empty()
        );

        let authority = shell
            .propose_provider_credential_install_authority(&connection_id)
            .expect("propose install authority");
        let prepared = shell
            .prepare_provider_credential_install_operation(
                &connection_id,
                &authority,
                ProviderCredentialSlotStatusInput::Missing,
            )
            .expect("dedicated authority-bound install remains available");
        assert_eq!(prepared.operation_id, authority.authority_id);
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one vertical metadata workflow verifies Core-backed CRUD and target selection"
    )]
    fn provider_metadata_crud_and_default_target_are_core_backed() {
        let root = tempdir().expect("temporary root");
        let shell = ShellApi::open(CoreConfig::new(root.path())).expect("shell");
        let template_view = shell
            .core
            .list_provider_template_views()
            .expect("template views")
            .into_iter()
            .find(|view| {
                view.default_network_mode == ProviderNetworkMode::Public
                    && view.template.default_manifest.default_api_origin.is_some()
                    && view.template.api_family != ApiFamily::AnthropicMessages
            })
            .expect("public built-in template");
        let template = shell
            .list_provider_templates()
            .expect("safe templates")
            .into_iter()
            .find(|template| template.id == template_view.template.id.0)
            .expect("safe matching template");
        assert_eq!(
            template.default_api_origin.as_deref(),
            template_view
                .template
                .default_manifest
                .default_api_origin
                .as_ref()
                .map(CanonicalOrigin::as_str)
        );
        assert_eq!(
            template.supports_model_listing,
            template_view
                .template
                .default_manifest
                .endpoints
                .models
                .is_some()
        );
        assert_eq!(
            template.parameters.len(),
            template_view.template.default_manifest.parameters.len()
        );
        let origin = template_view
            .template
            .default_manifest
            .default_api_origin
            .clone()
            .expect("origin")
            .to_string();
        let credential_required = !matches!(
            template_view.template.default_manifest.auth,
            AuthBinding::None
        );
        let connection = shell
            .create_provider_connection(CreateProviderConnectionInput {
                id: "shell-connection".to_owned(),
                template_id: template_view.template.id.0.clone(),
                template_version: template_view.template.manifest_version,
                display_name: "Shell connection".to_owned(),
                api_origin: origin.clone(),
                api_base_path: None,
                network_mode: ProviderNetworkModeInput::Public,
                local_network_approval: None,
                values: Vec::new(),
                approved_credential_origin: credential_required.then_some(origin),
                timeout_seconds: 30,
            })
            .expect("create connection");
        assert_eq!(connection.credential_binding_required, credential_required);
        let connection = shell
            .upsert_provider_connection(UpdateProviderConnectionInput {
                id: connection.id,
                display_name: "Renamed connection".to_owned(),
                timeout_seconds: 45,
            })
            .expect("update connection");
        assert_eq!(connection.display_name, "Renamed connection");
        assert_eq!(connection.timeout_seconds, 45);

        let route = shell
            .upsert_model_route(UpsertModelRouteInput::Create {
                id: "shell-route".to_owned(),
                connection_id: connection.id.clone(),
                api_family: api_family_input(template_view.template.api_family),
                model_id: "shell-model".to_owned(),
                display_name: Some("Shell model".to_owned()),
                route_config: ModelRouteConfigDto {
                    deployment_id: None,
                    region: None,
                    endpoint_path: None,
                    values: Vec::new(),
                },
                status: ModelAvailabilityInput::Available,
            })
            .expect("create route");
        let effective_parameter_specs = shell
            .effective_parameter_specs(&route.id)
            .expect("effective parameter specs");
        assert_eq!(
            effective_parameter_specs.len(),
            shell
                .core
                .effective_parameter_specs(&route.id.clone().into())
                .expect("Core effective parameter specs")
                .len()
        );
        let capability_override = shell
            .upsert_user_capability_override(UpsertCapabilityOverrideInput {
                id: "shell-streaming-override".to_owned(),
                model_route_id: route.id.clone(),
                key: CapabilityKeyInput::Streaming,
                value: CapabilityOverrideValueInput::Boolean(true),
                status: CapabilityOverrideStatusInput::Verified,
                expires_at: None,
            })
            .expect("upsert capability override");
        assert_eq!(capability_override.source, "user_override");
        assert!(capability_override.evidence_ref.is_none());
        assert!(
            shell
                .list_capability_observations(&route.id)
                .expect("capability observations")
                .iter()
                .any(|observation| observation.id == capability_override.id)
        );
        let effective_capability = shell
            .effective_capability(&route.id, CapabilityKeyInput::Streaming)
            .expect("effective capability")
            .expect("selected capability");
        assert_eq!(effective_capability.selected.id, capability_override.id);
        shell
            .delete_user_capability_override(&route.id, &capability_override.id)
            .expect("delete capability override");
        assert!(
            shell
                .list_capability_observations(&route.id)
                .expect("capability observations after delete")
                .iter()
                .all(|observation| observation.id != capability_override.id)
        );
        let preset_input = GenerationPresetInput {
            id: "shell-preset".to_owned(),
            model_route_id: route.id.clone(),
            display_name: "Shell preset".to_owned(),
            values: Vec::new(),
            reasoning: GenerationReasoningSettingsDto {
                mode: "provider_default".to_owned(),
                effort: None,
                budget_tokens: None,
                summary: "provider_default".to_owned(),
                preserve_opaque_state: false,
            },
            prompt_cache: PromptCacheSettingsDto {
                mode: "provider_default".to_owned(),
                ttl_kind: "provider_default".to_owned(),
                ttl_seconds: None,
                context_reference: None,
            },
        };
        shell
            .validate_generation_preset_candidate(preset_input.clone())
            .expect("validate preset");
        let preset = shell
            .upsert_generation_preset(preset_input)
            .expect("create preset");
        let settings = shell
            .select_generation_target(Some(GenerationTargetDto {
                model_route_id: route.id.clone(),
                generation_preset_id: preset.id.clone(),
            }))
            .expect("select target");
        assert_eq!(
            settings.selected_model_route_id.as_deref(),
            Some(route.id.as_str())
        );
        assert_eq!(
            settings.selected_generation_preset_id.as_deref(),
            Some(preset.id.as_str())
        );
        let updated_settings = shell
            .update_settings(AppSettingsDto {
                preserve_partial_generations: false,
                ..settings
            })
            .expect("update settings");
        assert!(!updated_settings.preserve_partial_generations);

        shell
            .delete_generation_preset(&preset.id)
            .expect("delete preset");
        shell.delete_model_route(&route.id).expect("delete route");
        if credential_required {
            let prepared = shell
                .prepare_provider_credential_operation(
                    &connection.id,
                    ProviderCredentialOperationKindInput::RemoveForArchive,
                    ProviderCredentialSlotStatusInput::Missing,
                )
                .expect("prepare credential-aware connection archive");
            shell
                .finish_provider_credential_archive(
                    &prepared.operation_id,
                    &prepared.plan_sha256,
                    ProviderCredentialSlotStatusInput::Missing,
                )
                .expect("finish credential-aware connection archive");
        } else {
            shell
                .delete_provider_connection(&connection.id)
                .expect("delete no-auth connection");
        }
    }

    const fn api_family_input(value: ApiFamily) -> ApiFamilyInput {
        match value {
            ApiFamily::OpenAiResponses => ApiFamilyInput::OpenAiResponses,
            ApiFamily::OpenAiChatCompletions => ApiFamilyInput::OpenAiChatCompletions,
            ApiFamily::AnthropicMessages => ApiFamilyInput::AnthropicMessages,
            ApiFamily::GeminiGenerateContent => ApiFamilyInput::GeminiGenerateContent,
            ApiFamily::OllamaNative => ApiFamilyInput::OllamaNative,
        }
    }

    fn synthetic_connection() -> lorepia_core::ProviderConnection {
        use chrono::Utc;
        use lorepia_core::{
            CanonicalOrigin, ConnectionConfig, ConnectionStatus, ProviderTemplateId,
        };

        lorepia_core::ProviderConnection {
            id: ProviderConnectionId::from("connection"),
            template_id: ProviderTemplateId::from("template"),
            template_version: 1,
            display_name: "Synthetic".into(),
            api_origin: CanonicalOrigin::parse("https://example.test").expect("origin"),
            config: ConnectionConfig::default(),
            credential_ref: None,
            credential_scope: None,
            timeout_seconds: 30,
            status: ConnectionStatus::Untested,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn create_shell_credential_connection(shell: &ShellApi) -> String {
        let template_view = shell
            .core
            .list_provider_template_views()
            .expect("template views")
            .into_iter()
            .find(|view| {
                view.default_network_mode == ProviderNetworkMode::Public
                    && view.template.default_manifest.default_api_origin.is_some()
                    && !matches!(view.template.default_manifest.auth, AuthBinding::None)
            })
            .expect("public credential-bound template");
        let origin = template_view
            .template
            .default_manifest
            .default_api_origin
            .expect("template origin")
            .to_string();
        shell
            .create_provider_connection(CreateProviderConnectionInput {
                id: "shell-generic-install".to_owned(),
                template_id: template_view.template.id.0,
                template_version: template_view.template.manifest_version,
                display_name: "Shell credential test".to_owned(),
                api_origin: origin.clone(),
                api_base_path: None,
                network_mode: ProviderNetworkModeInput::Public,
                local_network_approval: None,
                values: Vec::new(),
                approved_credential_origin: Some(origin),
                timeout_seconds: 30,
            })
            .expect("create credential-bound connection")
            .id
    }
}
