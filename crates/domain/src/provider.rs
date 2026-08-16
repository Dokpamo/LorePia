use std::{fmt, net::IpAddr, str::FromStr};

use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use url::Url;
use uuid::Uuid;
use zeroize::Zeroize;

use crate::{ConversationId, GenerationId, Message, ResolvedPromptPlan};

/// Maximum serialized size accepted for provider-specific usage metadata.
pub const MAX_BOUNDED_JSON_BYTES: usize = 4 * 1024;
/// Maximum Unicode scalar count accepted for provider-specific usage metadata.
pub const MAX_BOUNDED_JSON_CHARS: usize = 2 * 1024;
/// Maximum UTF-8 size of one provider tool-call identifier.
pub const MAX_TOOL_CALL_ID_BYTES: usize = 512;
/// Maximum Unicode scalar count of one provider tool-call identifier.
pub const MAX_TOOL_CALL_ID_CHARS: usize = 256;
/// Maximum UTF-8 size of one provider tool name.
pub const MAX_TOOL_NAME_BYTES: usize = 256;
/// Maximum Unicode scalar count of one provider tool name.
pub const MAX_TOOL_NAME_CHARS: usize = 128;
/// Maximum UTF-8 size of one streamed tool-arguments delta.
pub const MAX_TOOL_ARGUMENT_DELTA_BYTES: usize = 64 * 1024;
/// Maximum Unicode scalar count of one streamed tool-arguments delta.
pub const MAX_TOOL_ARGUMENT_DELTA_CHARS: usize = 32 * 1024;
/// Maximum opaque reasoning items retained for one generation.
pub const MAX_OPAQUE_REASONING_STATE_COUNT: usize = 32;
/// Maximum UTF-8 size of one encrypted reasoning payload or signature.
pub const MAX_OPAQUE_REASONING_ITEM_BYTES: usize = 64 * 1024;
/// Maximum cumulative UTF-8 size of opaque reasoning state for one generation.
pub const MAX_OPAQUE_REASONING_TOTAL_BYTES: usize = 256 * 1024;
/// Maximum JSON-encoded size of the complete opaque reasoning state vector.
///
/// This is the durable schema envelope introduced with generation protocol
/// state. Keeping the encoded bound in the domain contract ensures chat
/// rejects escape-expanded payloads before a terminal storage write.
pub const MAX_OPAQUE_REASONING_SERIALIZED_BYTES: usize = 264 * 1024;
/// Maximum summary or content parts retained in one `OpenAI` reasoning item.
pub const MAX_OPENAI_REASONING_PARTS: usize = 128;
/// Maximum number of ordered content blocks retained from one Anthropic turn.
pub const MAX_ANTHROPIC_CONTENT_BLOCKS: usize = 128;
/// Maximum Unicode scalar count retained in one Anthropic text-like block.
pub const MAX_ANTHROPIC_BLOCK_TEXT_CHARS: usize = 64 * 1024;
/// Maximum JSON nesting accepted for retained Anthropic tool input.
pub const MAX_ANTHROPIC_TOOL_INPUT_DEPTH: usize = 32;
/// Maximum JSON nodes accepted for retained Anthropic tool input.
pub const MAX_ANTHROPIC_TOOL_INPUT_NODES: usize = 4 * 1024;

macro_rules! stable_id {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4().to_string())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_inner(self) -> String {
                self.0
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

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }
    };
}

stable_id!(ProviderTemplateId);
stable_id!(ProviderConnectionId);
stable_id!(ModelRouteId);
stable_id!(GenerationPresetId);
stable_id!(ObservationId);
stable_id!(ParameterId);
stable_id!(EvidenceId);
stable_id!(DiscoverySessionId);
stable_id!(ModelSyncJobId);

/// Atomic selection used for a generation.
///
/// A preset is meaningful only for the model route it belongs to, so public
/// APIs carry both identifiers together and validate the relationship before
/// any messages or generation rows are persisted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationTarget {
    pub model_route_id: ModelRouteId,
    pub generation_preset_id: GenerationPresetId,
}

/// Provider identity bound to one normalized generation request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationProviderProvenance {
    pub api_family: ApiFamily,
    pub model_route_id: ModelRouteId,
    pub generation_preset_id: GenerationPresetId,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HttpUrl(String);

impl HttpUrl {
    pub fn parse(value: &str) -> Result<Self, String> {
        let url = Url::parse(value).map_err(|error| error.to_string())?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err("only http and https URLs are supported".to_owned());
        }
        if url.host_str().is_none() {
            return Err("URL must contain a host".to_owned());
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err("URL must not contain embedded credentials".to_owned());
        }
        Ok(Self(url.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for HttpUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for HttpUrl {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl Serialize for HttpUrl {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for HttpUrl {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CanonicalOrigin(String);

impl CanonicalOrigin {
    pub fn parse(value: &str) -> Result<Self, String> {
        let url = Url::parse(value).map_err(|error| error.to_string())?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err("only http and https origins are supported".to_owned());
        }
        if url.host_str().is_none() {
            return Err("origin must contain a host".to_owned());
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err("origin must not contain embedded credentials".to_owned());
        }
        if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
            return Err("origin must not contain a path, query, or fragment".to_owned());
        }
        Ok(Self(url.origin().ascii_serialization()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CanonicalOrigin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for CanonicalOrigin {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl Serialize for CanonicalOrigin {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for CanonicalOrigin {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CredentialRef(pub String);

impl CredentialRef {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HeaderName(String);

impl HeaderName {
    pub fn parse(value: &str) -> Result<Self, String> {
        if value.is_empty()
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
        {
            return Err("header name must be a non-empty RFC 9110 token".to_owned());
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Serialize for HeaderName {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for HeaderName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EndpointPath(String);

impl EndpointPath {
    pub fn parse(value: &str) -> Result<Self, String> {
        let lower = value.to_ascii_lowercase();
        if !value.starts_with('/')
            || value.starts_with("//")
            || value.contains('\\')
            || value.contains('?')
            || value.contains('#')
            || value.split('/').any(|segment| segment == "..")
            || lower.contains("%2e")
            || lower.contains("%2f")
            || lower.contains("%5c")
        {
            return Err("endpoint must be a normalized absolute path".to_owned());
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Serialize for EndpointPath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for EndpointPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ApiFamily {
    #[serde(rename = "openai_responses")]
    OpenAiResponses,
    #[serde(rename = "openai_chat_completions")]
    OpenAiChatCompletions,
    #[serde(rename = "anthropic_messages")]
    AnthropicMessages,
    #[serde(rename = "gemini_generate_content")]
    GeminiGenerateContent,
    #[serde(rename = "ollama_native")]
    OllamaNative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateSource {
    BuiltIn,
    SignedCatalog,
    UserDiscovered,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AuthBinding {
    None,
    BearerHeader,
    HeaderApiKey { header_name: HeaderName },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionFieldType {
    Text,
    Integer,
    Boolean,
    Credential,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionFieldSpec {
    pub key: String,
    pub label_key: String,
    pub description_key: Option<String>,
    pub value_type: ConnectionFieldType,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ConnectionConfigValue {
    Text(String),
    Integer(i64),
    Boolean(bool),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionConfigEntry {
    pub key: String,
    pub value: ConnectionConfigValue,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionConfig {
    /// Optional path prefix between the origin and manifest endpoint paths.
    ///
    /// This preserves OpenAI-compatible legacy URLs such as
    /// `https://example.test/v1` without weakening the origin boundary.
    #[serde(default)]
    pub api_base_path: Option<EndpointPath>,
    #[serde(default)]
    pub network_mode: ProviderNetworkMode,
    /// Exact origin/address grant required for the explicit LAN mode.
    ///
    /// The provider boundary validates and normalizes this grant before it is
    /// persisted. Public and loopback connections must leave it unset.
    #[serde(default)]
    pub local_network_approval: Option<ProviderLocalNetworkApproval>,
    #[serde(default)]
    pub values: Vec<ConnectionConfigEntry>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderNetworkMode {
    #[default]
    Public,
    LocalLoopback,
    ApprovedLocalNetwork,
}

/// Secret-free approval for one exact LAN origin and finite address set.
///
/// Addresses are stored as typed IP values rather than subnets or hostname
/// patterns. Core normalizes them into sorted order and accepts only one to
/// sixteen RFC1918 IPv4 or ULA IPv6 addresses.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderLocalNetworkApproval {
    pub origin: CanonicalOrigin,
    pub addresses: Vec<IpAddr>,
}

/// Maximum lifetime of one immutable exact-origin LAN approval.
///
/// The approval mirror records `ProviderConnection::created_at` as its issue
/// time. Reusing that existing immutable timestamp keeps old serialized
/// connections compatible while ensuring DHCP/address reuse eventually fails
/// closed and requires the user to create a fresh approval.
pub const PROVIDER_LOCAL_NETWORK_APPROVAL_TTL_SECONDS: i64 = 24 * 60 * 60;

/// Returns whether an immutable LAN approval is active at `observed_at`.
///
/// Future-dated issue times fail closed. The expiry instant itself is no
/// longer active.
pub fn provider_local_network_approval_is_active_at(
    approved_at: DateTime<Utc>,
    observed_at: DateTime<Utc>,
) -> bool {
    approved_at <= observed_at
        && approved_at
            .checked_add_signed(TimeDelta::seconds(
                PROVIDER_LOCAL_NETWORK_APPROVAL_TTL_SECONDS,
            ))
            .is_some_and(|expires_at| observed_at < expires_at)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialRedirectPolicy {
    Deny,
    FollowWithoutCredential,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialScope {
    pub allowed_origins: Vec<CanonicalOrigin>,
    pub auth_binding: AuthBinding,
    pub redirect_policy: CredentialRedirectPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionStatus {
    Untested,
    Connected,
    AuthFailed,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestSourceKind {
    OfficialSite,
    OfficialDocumentation,
    SignedCatalog,
    UserSupplied,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestSource {
    pub kind: ManifestSourceKind,
    pub url: HttpUrl,
    pub content_sha256: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Post,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndpointSpec {
    pub method: HttpMethod,
    pub path: EndpointPath,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestEndpoints {
    pub models: Option<EndpointSpec>,
    pub generate: EndpointSpec,
    /// Optional provider-native embedding endpoint.
    ///
    /// Absence is an explicit capability boundary: callers must fail before
    /// dispatch instead of reusing a generation endpoint or guessing a route.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embeddings: Option<EndpointSpec>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecoderId {
    OpenAiJsonV1,
    OpenAiSseV1,
    AnthropicJsonV1,
    AnthropicSseV1,
    GeminiJsonV1,
    GeminiSseV1,
    OllamaJsonV1,
    OllamaJsonlV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestDecoders {
    pub response: DecoderId,
    pub streaming: Option<DecoderId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParameterType {
    Boolean,
    Integer,
    Number,
    String,
    Enum,
    StringList,
    JsonSchema,
    StopSequenceList,
    ToolPolicy,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ParameterLiteral {
    Boolean(bool),
    Integer(i64),
    Number(f64),
    String(String),
    Enum(String),
    StringList(Vec<String>),
    JsonSchema(String),
    StopSequenceList(Vec<String>),
    ToolPolicy(ToolPolicy),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolPolicy {
    None,
    Auto,
    Required,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub enum ParameterValueState {
    InheritProviderDefault,
    Explicit(ParameterLiteral),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterValue {
    pub parameter_id: ParameterId,
    pub state: ParameterValueState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterChoice {
    pub value: ParameterLiteral,
    pub label_key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParameterDefaultMode {
    ProviderDefault,
    ExplicitRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParameterConditionOperator {
    Equals,
    NotEquals,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterCondition {
    pub parameter_id: ParameterId,
    pub operator: ParameterConditionOperator,
    pub value: ParameterLiteral,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParameterConflictKind {
    MutuallyExclusive,
    Requires,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterConflict {
    pub parameter_id: ParameterId,
    pub kind: ParameterConflictKind,
    pub message_key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderParameterTarget {
    RequestBody,
    RequestHeader,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderParameterMapping {
    pub target: ProviderParameterTarget,
    pub field_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiParameterLevel {
    Basic,
    Advanced,
    Expert,
    HiddenInternal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterSpec {
    pub id: ParameterId,
    pub label_key: String,
    pub description_key: Option<String>,
    pub value_type: ParameterType,
    pub allowed_values: Vec<ParameterChoice>,
    pub minimum: Option<f64>,
    pub maximum: Option<f64>,
    pub step: Option<f64>,
    pub default_mode: ParameterDefaultMode,
    pub visibility: Option<ParameterCondition>,
    pub conflicts: Vec<ParameterConflict>,
    pub provider_mapping: ProviderParameterMapping,
    pub level: UiParameterLevel,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderManifest {
    pub schema_version: u32,
    pub api_family: ApiFamily,
    pub sources: Vec<ManifestSource>,
    pub default_api_origin: Option<CanonicalOrigin>,
    pub auth: AuthBinding,
    pub endpoints: ManifestEndpoints,
    pub decoders: ManifestDecoders,
    pub parameters: Vec<ParameterSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderTemplate {
    pub id: ProviderTemplateId,
    pub display_name: String,
    pub manifest_version: u32,
    pub source: TemplateSource,
    pub api_family: ApiFamily,
    pub connection_fields: Vec<ConnectionFieldSpec>,
    pub default_manifest: ProviderManifest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderConnection {
    pub id: ProviderConnectionId,
    pub template_id: ProviderTemplateId,
    pub template_version: u32,
    pub display_name: String,
    pub api_origin: CanonicalOrigin,
    pub config: ConnectionConfig,
    pub credential_ref: Option<CredentialRef>,
    pub credential_scope: Option<CredentialScope>,
    pub timeout_seconds: u32,
    pub status: ConnectionStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ProviderConnection {
    /// Checks the lifetime of this connection's immutable LAN approval.
    ///
    /// Callers should invoke this only when `network_mode` is
    /// `ApprovedLocalNetwork`; the connection creation timestamp is also the
    /// issue time stored by the immutable approval mirror.
    pub fn local_network_approval_is_active_at(&self, observed_at: DateTime<Utc>) -> bool {
        provider_local_network_approval_is_active_at(self.created_at, observed_at)
    }
}

/// Secret-free input used to create or update a provider connection.
///
/// Native code stores the credential in the platform credential vault under
/// the connection ID. The approved origin is carried explicitly so Core can
/// bind that opaque credential reference to exactly one origin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConnectionDraft {
    pub id: ProviderConnectionId,
    pub template_id: ProviderTemplateId,
    pub template_version: u32,
    pub display_name: String,
    pub api_origin: CanonicalOrigin,
    pub api_base_path: Option<EndpointPath>,
    pub network_mode: ProviderNetworkMode,
    #[serde(default)]
    pub local_network_approval: Option<ProviderLocalNetworkApproval>,
    #[serde(default)]
    pub values: Vec<ConnectionConfigEntry>,
    pub approved_credential_origin: Option<CanonicalOrigin>,
    pub timeout_seconds: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRouteConfig {
    pub deployment_id: Option<String>,
    pub region: Option<String>,
    pub endpoint_path: Option<EndpointPath>,
    #[serde(default)]
    pub values: Vec<ConnectionConfigEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelAvailability {
    Available,
    MissingTemporarily,
    DocumentedOnly,
    AccessDenied,
    Deprecated,
    Retired,
    Unknown,
}

/// Provenance category for bounded route metadata.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelMetadataSource {
    #[default]
    Legacy,
    ProviderApi,
    OfficialDocumentation,
    SignedCatalog,
    CapabilityProbe,
    UserOverride,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRoute {
    pub id: ModelRouteId,
    pub connection_id: ProviderConnectionId,
    pub api_family: ApiFamily,
    pub model_id: String,
    pub display_name: Option<String>,
    pub route_config: ModelRouteConfig,
    pub status: ModelAvailability,
    /// Number of consecutive successful listings which omitted this stable
    /// route identity. A later listing resets the value to zero.
    #[serde(default)]
    pub miss_count: u32,
    /// Bounded normalized metadata, never an arbitrary provider response.
    #[serde(default)]
    pub raw_metadata: Option<BoundedJson>,
    #[serde(default)]
    pub metadata_source: ModelMetadataSource,
    #[serde(default)]
    pub metadata_observed_at: Option<DateTime<Utc>>,
    /// Last durable synchronization which checked this route, whether seen or
    /// omitted.
    #[serde(default)]
    pub last_reconciled_sync_job_id: Option<ModelSyncJobId>,
    /// Synchronization which supplied the currently stored positive metadata.
    /// Omission never overwrites this provenance.
    #[serde(default)]
    pub metadata_sync_job_id: Option<ModelSyncJobId>,
    pub first_seen_at: DateTime<Utc>,
    pub last_seen_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GenerationPreset {
    pub id: GenerationPresetId,
    pub model_route_id: ModelRouteId,
    pub display_name: String,
    pub values: Vec<ParameterValue>,
    #[serde(default)]
    pub reasoning: GenerationReasoningSettings,
    #[serde(default)]
    pub prompt_cache: GenerationPromptCacheSettings,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationReasoningMode {
    #[default]
    ProviderDefault,
    Disabled,
    Automatic,
    Enabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationReasoningEffort {
    Minimal,
    Low,
    Medium,
    High,
    ExtraHigh,
    Maximum,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationReasoningSummary {
    #[default]
    ProviderDefault,
    Disabled,
    Automatic,
    Concise,
    Detailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationReasoningSettings {
    pub mode: GenerationReasoningMode,
    pub effort: Option<GenerationReasoningEffort>,
    pub budget_tokens: Option<u32>,
    pub summary: GenerationReasoningSummary,
    pub preserve_opaque_state: bool,
}

impl Default for GenerationReasoningSettings {
    fn default() -> Self {
        Self {
            mode: GenerationReasoningMode::ProviderDefault,
            effort: None,
            budget_tokens: None,
            summary: GenerationReasoningSummary::ProviderDefault,
            preserve_opaque_state: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationPromptCacheMode {
    #[default]
    ProviderDefault,
    Automatic,
    ExplicitBreakpoints,
    ExplicitContext,
    DisabledIfSupported,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "seconds", rename_all = "snake_case")]
pub enum GenerationPromptCacheTtl {
    #[default]
    ProviderDefault,
    Short,
    Long,
    CustomSeconds(u32),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationPromptCacheSettings {
    pub mode: GenerationPromptCacheMode,
    pub ttl: GenerationPromptCacheTtl,
    pub context_reference: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    pub streaming: bool,
    pub reasoning: bool,
    pub max_context_tokens: Option<u32>,
}

/// Legacy OpenAI-compatible provider configuration retained during migration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderProfile {
    pub id: String,
    pub display_name: String,
    pub base_url: String,
    pub model: String,
    pub timeout_seconds: u32,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct GenerationRequest {
    pub generation_id: GenerationId,
    pub conversation_id: ConversationId,
    pub model: String,
    pub messages: Vec<Message>,
    /// Exact provider-neutral prompt resolution for the current request.
    ///
    /// Adapters consume this plan before the legacy `messages` fallback. It is
    /// never generically serialized or deserialized, so prompt traces and
    /// provider compilation metadata cannot be copied wholesale onto the wire.
    #[serde(default, skip_serializing, skip_deserializing)]
    pub resolved_prompt_plan: Option<ResolvedPromptPlan>,
    /// Hash of the exact provider wire-role and cache-disposition plan shown
    /// in the redacted preview.
    ///
    /// This remains separate from the provider-neutral resolved-plan hash and
    /// is never copied through generic serialization. The selected adapter
    /// recomputes it from its exact route contract immediately before
    /// materializing the network request.
    #[serde(default, skip_serializing, skip_deserializing)]
    pub provider_execution_plan_hash: Option<String>,
    /// `None` preserves the provider/model default by omitting the wire field.
    pub temperature: Option<f64>,
    pub max_output_tokens: Option<u32>,
    /// Exact catalog target for opaque provider-state isolation.
    #[serde(default)]
    pub provider_provenance: Option<GenerationProviderProvenance>,
    /// Whether provider-native opaque reasoning continuity may be retained.
    #[serde(default)]
    pub preserve_opaque_reasoning_state: bool,
    /// Previously persisted state, already bound to its original assistant
    /// message and provider target. Adapters must validate the binding again.
    /// Generic request serialization omits it; only native adapter payload
    /// builders may place the recognized opaque form on the wire.
    #[serde(default, skip_serializing, skip_deserializing)]
    pub opaque_reasoning_context: Vec<OpaqueReasoningContext>,
}

impl fmt::Debug for GenerationRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GenerationRequest")
            .field("generation_id", &self.generation_id)
            .field("conversation_id", &self.conversation_id)
            // A provider-controlled model identifier can reflect a borrowed
            // credential. Keep it out of diagnostic output even though Core
            // validates ordinary route identities before dispatch.
            .field("model_bytes", &self.model.len())
            .field("message_count", &self.messages.len())
            .field(
                "has_resolved_prompt_plan",
                &self.resolved_prompt_plan.is_some(),
            )
            // This field is verified as a digest by the adapter, but the
            // domain value can be constructed before that verification. Do
            // not log an arbitrary caller-supplied scalar.
            .field(
                "has_provider_execution_plan_hash",
                &self.provider_execution_plan_hash.is_some(),
            )
            .field("temperature", &self.temperature)
            .field("max_output_tokens", &self.max_output_tokens)
            .field(
                "provider_api_family",
                &self
                    .provider_provenance
                    .as_ref()
                    .map(|provenance| provenance.api_family),
            )
            .field(
                "preserve_opaque_reasoning_state",
                &self.preserve_opaque_reasoning_state,
            )
            .field(
                "opaque_reasoning_context_count",
                &self.opaque_reasoning_context.len(),
            )
            .finish()
    }
}

/// A bounded provider item identifier used only to reconstruct an opaque
/// reasoning input item.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct OpaqueReasoningItemId(String);

impl OpaqueReasoningItemId {
    pub fn parse(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        validate_bounded_protocol_text(
            "opaque reasoning item id",
            &value,
            MAX_TOOL_CALL_ID_BYTES,
            MAX_TOOL_CALL_ID_CHARS,
            false,
        )?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for OpaqueReasoningItemId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OpaqueReasoningItemId([REDACTED])")
    }
}

impl<'de> Deserialize<'de> for OpaqueReasoningItemId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

/// Encrypted or signed provider state. Debug output is always redacted.
#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct OpaqueReasoningData(String);

impl OpaqueReasoningData {
    pub fn parse(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        validate_bounded_protocol_text(
            "opaque reasoning data",
            &value,
            MAX_OPAQUE_REASONING_ITEM_BYTES,
            MAX_OPAQUE_REASONING_ITEM_BYTES,
            false,
        )?;
        Ok(Self(value))
    }

    pub fn expose_to_provider(&self) -> &str {
        &self.0
    }

    pub fn byte_len(&self) -> usize {
        self.0.len()
    }
}

impl fmt::Debug for OpaqueReasoningData {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OpaqueReasoningData([REDACTED])")
    }
}

impl<'de> Deserialize<'de> for OpaqueReasoningData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

/// Bounded text from one Anthropic content block.
///
/// The value can contain normal message whitespace, so unlike protocol
/// identifiers it is constrained only by byte and Unicode-scalar budgets.
/// Debug output is always redacted because thinking summaries and assistant
/// text are private conversation content.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct AnthropicBlockText(String);

impl AnthropicBlockText {
    pub fn parse(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        if value.len() > MAX_OPAQUE_REASONING_TOTAL_BYTES
            || value.chars().count() > MAX_ANTHROPIC_BLOCK_TEXT_CHARS
        {
            return Err(format!(
                "Anthropic block text exceeds the {MAX_OPAQUE_REASONING_TOTAL_BYTES}-byte or \
                 {MAX_ANTHROPIC_BLOCK_TEXT_CHARS}-character limit"
            ));
        }
        Ok(Self(value))
    }

    pub fn expose_to_provider(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AnthropicBlockText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AnthropicBlockText([REDACTED])")
    }
}

impl<'de> Deserialize<'de> for AnthropicBlockText {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

/// Canonical, bounded JSON object from one Anthropic `tool_use` block.
///
/// `LorePia` retains this only to replay the exact provider protocol topology.
/// It is inert data and never authorizes or executes the tool call.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct AnthropicToolInput(serde_json::Value);

impl AnthropicToolInput {
    pub fn from_value(value: &serde_json::Value) -> Result<Self, String> {
        if !value.is_object() {
            return Err("Anthropic tool input must be a JSON object".to_owned());
        }
        validate_anthropic_tool_input_shape(value)?;
        let encoded = serde_json::to_vec(value)
            .map_err(|_| "Anthropic tool input must be valid JSON".to_owned())?;
        if encoded.len() > MAX_OPAQUE_REASONING_ITEM_BYTES {
            return Err(format!(
                "Anthropic tool input exceeds the {MAX_OPAQUE_REASONING_ITEM_BYTES}-byte limit"
            ));
        }
        Ok(Self(value.clone()))
    }

    pub fn expose_to_provider(&self) -> &serde_json::Value {
        &self.0
    }
}

impl fmt::Debug for AnthropicToolInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AnthropicToolInput([REDACTED])")
    }
}

impl<'de> Deserialize<'de> for AnthropicToolInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        Self::from_value(&value).map_err(D::Error::custom)
    }
}

fn validate_anthropic_tool_input_shape(value: &serde_json::Value) -> Result<(), String> {
    let mut stack = vec![(value, 0_usize)];
    let mut nodes = 0_usize;
    while let Some((current, depth)) = stack.pop() {
        nodes = nodes
            .checked_add(1)
            .ok_or_else(|| "Anthropic tool input node count overflowed".to_owned())?;
        if nodes > MAX_ANTHROPIC_TOOL_INPUT_NODES {
            return Err(format!(
                "Anthropic tool input exceeds the {MAX_ANTHROPIC_TOOL_INPUT_NODES}-node limit"
            ));
        }
        if depth > MAX_ANTHROPIC_TOOL_INPUT_DEPTH {
            return Err(format!(
                "Anthropic tool input exceeds depth {MAX_ANTHROPIC_TOOL_INPUT_DEPTH}"
            ));
        }
        match current {
            serde_json::Value::Array(values) => {
                stack.extend(values.iter().map(|value| (value, depth.saturating_add(1))));
            }
            serde_json::Value::Object(values) => {
                stack.extend(
                    values
                        .values()
                        .map(|value| (value, depth.saturating_add(1))),
                );
            }
            serde_json::Value::Null
            | serde_json::Value::Bool(_)
            | serde_json::Value::Number(_)
            | serde_json::Value::String(_) => {}
        }
    }
    Ok(())
}

/// One exact, ordered block from an Anthropic assistant response.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AnthropicContentBlock {
    Text {
        text: AnthropicBlockText,
    },
    Thinking {
        thinking: AnthropicBlockText,
        signature: OpaqueReasoningData,
    },
    RedactedThinking {
        data: OpaqueReasoningData,
    },
    ToolUse {
        id: ToolCallId,
        name: ToolName,
        input: AnthropicToolInput,
    },
}

impl fmt::Debug for AnthropicContentBlock {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Text { .. } => "AnthropicContentBlock::Text([REDACTED])",
            Self::Thinking { .. } => "AnthropicContentBlock::Thinking([REDACTED])",
            Self::RedactedThinking { .. } => "AnthropicContentBlock::RedactedThinking([REDACTED])",
            Self::ToolUse { .. } => "AnthropicContentBlock::ToolUse([REDACTED])",
        })
    }
}

/// Complete bounded Anthropic assistant content topology.
///
/// A topology is retained only when it contains a thinking or
/// `redacted_thinking` block. Text and tool-use blocks are included as well so
/// a later request can replay the original assistant content array without
/// flattening, dropping, or reordering any block.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct AnthropicContentBlockTopology(Vec<AnthropicContentBlock>);

impl AnthropicContentBlockTopology {
    pub fn new(blocks: Vec<AnthropicContentBlock>) -> Result<Self, String> {
        if blocks.is_empty() || blocks.len() > MAX_ANTHROPIC_CONTENT_BLOCKS {
            return Err(format!(
                "Anthropic content topology must contain between 1 and \
                 {MAX_ANTHROPIC_CONTENT_BLOCKS} blocks"
            ));
        }
        if !blocks.iter().any(|block| {
            matches!(
                block,
                AnthropicContentBlock::Thinking { .. }
                    | AnthropicContentBlock::RedactedThinking { .. }
            )
        }) {
            return Err("Anthropic opaque topology must contain a thinking block".to_owned());
        }
        let mut visible_content_seen = false;
        for block in &blocks {
            match block {
                AnthropicContentBlock::Thinking { .. }
                | AnthropicContentBlock::RedactedThinking { .. }
                    if visible_content_seen =>
                {
                    return Err("Anthropic thinking blocks must precede text blocks".to_owned());
                }
                AnthropicContentBlock::Thinking { .. }
                | AnthropicContentBlock::RedactedThinking { .. }
                | AnthropicContentBlock::ToolUse { .. } => {}
                AnthropicContentBlock::Text { .. } => visible_content_seen = true,
            }
        }
        let topology = Self(blocks);
        if topology.serialized_bytes() > MAX_OPAQUE_REASONING_TOTAL_BYTES {
            return Err(format!(
                "Anthropic content topology exceeds the \
                 {MAX_OPAQUE_REASONING_TOTAL_BYTES}-byte limit"
            ));
        }
        Ok(topology)
    }

    pub fn blocks(&self) -> &[AnthropicContentBlock] {
        &self.0
    }

    pub fn serialized_bytes(&self) -> usize {
        serde_json::to_vec(self).map_or(usize::MAX, |encoded| encoded.len())
    }

    pub fn flattened_text(&self) -> String {
        self.0
            .iter()
            .filter_map(|block| match block {
                AnthropicContentBlock::Text { text } => Some(text.expose_to_provider()),
                AnthropicContentBlock::Thinking { .. }
                | AnthropicContentBlock::RedactedThinking { .. }
                | AnthropicContentBlock::ToolUse { .. } => None,
            })
            .collect()
    }
}

impl fmt::Debug for AnthropicContentBlockTopology {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AnthropicContentBlockTopology([REDACTED])")
    }
}

impl<'de> Deserialize<'de> for AnthropicContentBlockTopology {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(Vec::<AnthropicContentBlock>::deserialize(deserializer)?)
            .map_err(D::Error::custom)
    }
}

/// One complete input-safe `OpenAI` Responses reasoning item.
///
/// The item retains only fields accepted when manually replaying reasoning
/// input. Unknown fields are rejected, private text is redacted from Debug,
/// and construction enforces one bounded item before it can be persisted.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OpenAiResponsesReasoningItem {
    id: OpaqueReasoningItemId,
    #[serde(rename = "type")]
    kind: OpenAiResponsesReasoningItemKind,
    summary: Vec<OpenAiResponsesReasoningSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<Vec<OpenAiResponsesReasoningContent>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    encrypted_content: Option<OpaqueReasoningData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<OpenAiResponsesReasoningStatus>,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum OpenAiResponsesReasoningItemKind {
    #[serde(rename = "reasoning")]
    Reasoning,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenAiResponsesReasoningSummary {
    #[serde(rename = "type")]
    kind: OpenAiResponsesReasoningSummaryKind,
    text: OpenAiResponsesReasoningText,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum OpenAiResponsesReasoningSummaryKind {
    #[serde(rename = "summary_text")]
    SummaryText,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenAiResponsesReasoningContent {
    #[serde(rename = "type")]
    kind: OpenAiResponsesReasoningContentKind,
    text: OpenAiResponsesReasoningText,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum OpenAiResponsesReasoningContentKind {
    #[serde(rename = "reasoning_text")]
    ReasoningText,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OpenAiResponsesReasoningStatus {
    InProgress,
    Completed,
    Incomplete,
}

impl OpenAiResponsesReasoningStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Incomplete => "incomplete",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
struct OpenAiResponsesReasoningText(String);

impl OpenAiResponsesReasoningText {
    fn parse(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        if value.len() > MAX_OPAQUE_REASONING_ITEM_BYTES
            || value.chars().count() > MAX_OPAQUE_REASONING_ITEM_BYTES
        {
            return Err(format!(
                "OpenAI reasoning text exceeds the \
                 {MAX_OPAQUE_REASONING_ITEM_BYTES}-byte or character limit"
            ));
        }
        Ok(Self(value))
    }
}

impl<'de> Deserialize<'de> for OpenAiResponsesReasoningText {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

impl OpenAiResponsesReasoningItem {
    pub fn from_value(value: &serde_json::Value) -> Result<Self, String> {
        serde_json::from_value(value.clone())
            .map_err(|_| "OpenAI reasoning item has an invalid input-safe shape".to_owned())
    }

    pub fn item_id(&self) -> &str {
        self.id.as_str()
    }

    pub fn to_provider_value(&self) -> Result<serde_json::Value, String> {
        serde_json::to_value(self)
            .map_err(|_| "OpenAI reasoning item could not be reconstructed".to_owned())
    }

    pub fn payload_bytes(&self) -> usize {
        sensitive_serialized_len(self)
    }

    /// Exact borrowed scan used only by the credential reflection guard.
    pub fn contains_exact_for_reflection_guard(&self, candidate: &str) -> bool {
        self.id.as_str().contains(candidate)
            || self
                .summary
                .iter()
                .any(|part| part.text.0.contains(candidate))
            || self
                .content
                .as_ref()
                .is_some_and(|parts| parts.iter().any(|part| part.text.0.contains(candidate)))
            || self
                .encrypted_content
                .as_ref()
                .is_some_and(|data| data.expose_to_provider().contains(candidate))
            || self
                .status
                .is_some_and(|status| status.as_str().contains(candidate))
    }

    fn validate(&self) -> Result<(), String> {
        if self.summary.len() > MAX_OPENAI_REASONING_PARTS
            || self
                .content
                .as_ref()
                .is_some_and(|parts| parts.len() > MAX_OPENAI_REASONING_PARTS)
        {
            return Err(format!(
                "OpenAI reasoning item exceeds the {MAX_OPENAI_REASONING_PARTS}-part limit"
            ));
        }
        if self.payload_bytes() > MAX_OPAQUE_REASONING_ITEM_BYTES {
            return Err(format!(
                "OpenAI reasoning item exceeds the \
                 {MAX_OPAQUE_REASONING_ITEM_BYTES}-byte limit"
            ));
        }
        Ok(())
    }

    fn zeroize_sensitive_payloads(&mut self) {
        self.id.0.zeroize();
        for part in &mut self.summary {
            part.text.0.zeroize();
        }
        if let Some(content) = &mut self.content {
            for part in content {
                part.text.0.zeroize();
            }
        }
        if let Some(encrypted_content) = &mut self.encrypted_content {
            encrypted_content.0.zeroize();
        }
    }
}

impl fmt::Debug for OpenAiResponsesReasoningItem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OpenAiResponsesReasoningItem([REDACTED])")
    }
}

impl<'de> Deserialize<'de> for OpenAiResponsesReasoningItem {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawItem {
            id: OpaqueReasoningItemId,
            #[serde(rename = "type")]
            kind: OpenAiResponsesReasoningItemKind,
            summary: Vec<OpenAiResponsesReasoningSummary>,
            #[serde(default)]
            content: Option<Vec<OpenAiResponsesReasoningContent>>,
            #[serde(default)]
            encrypted_content: Option<OpaqueReasoningData>,
            #[serde(default)]
            status: Option<OpenAiResponsesReasoningStatus>,
        }

        let raw = RawItem::deserialize(deserializer)?;
        let item = Self {
            id: raw.id,
            kind: raw.kind,
            summary: raw.summary,
            content: raw.content,
            encrypted_content: raw.encrypted_content,
            status: raw.status,
        };
        item.validate().map_err(D::Error::custom)?;
        Ok(item)
    }
}

/// One canonical, bounded `OpenRouter` `reasoning_details` object.
///
/// The object is retained only as provider-native continuity state. Its Debug
/// representation is redacted and generic request/DTO serialization never
/// exposes it.
#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct OpenRouterReasoningDetail(String);

impl OpenRouterReasoningDetail {
    pub fn parse(value: impl Into<String>) -> Result<Self, String> {
        let mut value = value.into();
        let parsed = serde_json::from_str(&value);
        let Ok(mut parsed) = parsed else {
            value.zeroize();
            return Err("OpenRouter reasoning detail must be valid JSON".to_owned());
        };
        let detail = Self::from_value(&parsed);
        zeroize_json_strings(&mut parsed);
        value.zeroize();
        detail
    }

    pub fn from_value(value: &serde_json::Value) -> Result<Self, String> {
        validate_openrouter_reasoning_detail(value)?;
        let mut canonical = serde_json::to_string(value)
            .map_err(|_| "OpenRouter reasoning detail must be valid JSON".to_owned())?;
        if let Err(error) = validate_bounded_protocol_text(
            "OpenRouter reasoning detail",
            &canonical,
            MAX_OPAQUE_REASONING_ITEM_BYTES,
            MAX_OPAQUE_REASONING_ITEM_BYTES,
            false,
        ) {
            canonical.zeroize();
            return Err(error);
        }
        Ok(Self(canonical))
    }

    pub fn expose_to_provider(&self) -> &str {
        &self.0
    }

    pub fn byte_len(&self) -> usize {
        self.0.len()
    }
}

impl fmt::Debug for OpenRouterReasoningDetail {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OpenRouterReasoningDetail([REDACTED])")
    }
}

impl Drop for OpenRouterReasoningDetail {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl<'de> Deserialize<'de> for OpenRouterReasoningDetail {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

fn validate_openrouter_reasoning_detail(value: &serde_json::Value) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or_else(|| "OpenRouter reasoning detail must be a JSON object".to_owned())?;
    let kind = object
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "OpenRouter reasoning detail must contain a string type".to_owned())?;
    let required_payload = match kind {
        "reasoning.summary" => Some("summary"),
        "reasoning.encrypted" => Some("data"),
        "reasoning.text" => None,
        _ => return Err("OpenRouter reasoning detail type is unsupported".to_owned()),
    };
    if let Some(required_payload) = required_payload
        && !object
            .get(required_payload)
            .is_some_and(serde_json::Value::is_string)
    {
        return Err(format!(
            "OpenRouter {kind} detail must contain a string {required_payload}"
        ));
    }
    if object
        .get("text")
        .is_some_and(|text| !text.is_null() && !text.is_string())
    {
        return Err("OpenRouter reasoning text must be a string or null".to_owned());
    }
    if object
        .get("id")
        .is_some_and(|id| !id.is_null() && !id.is_string())
    {
        return Err("OpenRouter reasoning detail id must be a string or null".to_owned());
    }
    if let Some(format) = object.get("format").filter(|format| !format.is_null()) {
        let Some(format) = format.as_str() else {
            return Err("OpenRouter reasoning detail format must be a string or null".to_owned());
        };
        if format.is_empty()
            || format.len() > MAX_TOOL_NAME_BYTES
            || format.chars().count() > MAX_TOOL_NAME_CHARS
            || format.chars().any(char::is_control)
        {
            return Err(
                "OpenRouter reasoning detail format must be a bounded identifier".to_owned(),
            );
        }
    }
    if object
        .get("index")
        .is_some_and(|index| !index.is_null() && index.as_u64().is_none())
    {
        return Err("OpenRouter reasoning detail index must be a non-negative integer".to_owned());
    }
    if kind == "reasoning.text"
        && object
            .get("signature")
            .is_some_and(|signature| !signature.is_null() && !signature.is_string())
    {
        return Err("OpenRouter reasoning text signature must be a string or null".to_owned());
    }
    Ok(())
}

/// Complete message-level `OpenRouter` reasoning continuity.
///
/// `Some(Vec::new())` deliberately differs from `None`: some routed models
/// require an observed empty `reasoning_details` array to be replayed.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OpenRouterReasoningTopology {
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<OpenRouterReasoningText>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_details: Option<Vec<OpenRouterReasoningDetail>>,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
struct OpenRouterReasoningText(String);

impl OpenRouterReasoningText {
    fn parse(value: impl Into<String>) -> Result<Self, String> {
        let mut value = value.into();
        if value.len() > MAX_OPAQUE_REASONING_TOTAL_BYTES
            || value.chars().count() > MAX_OPAQUE_REASONING_TOTAL_BYTES
        {
            value.zeroize();
            return Err(format!(
                "OpenRouter reasoning text exceeds the \
                 {MAX_OPAQUE_REASONING_TOTAL_BYTES}-byte or character limit"
            ));
        }
        Ok(Self(value))
    }
}

impl<'de> Deserialize<'de> for OpenRouterReasoningText {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

impl Drop for OpenRouterReasoningText {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl OpenRouterReasoningTopology {
    pub fn new(
        reasoning: Option<String>,
        reasoning_details: Option<Vec<OpenRouterReasoningDetail>>,
    ) -> Result<Self, String> {
        if reasoning.is_none() && reasoning_details.is_none() {
            return Err("OpenRouter reasoning topology must retain an observed field".to_owned());
        }
        let topology = Self {
            reasoning: reasoning.map(OpenRouterReasoningText::parse).transpose()?,
            reasoning_details,
        };
        topology.validate()?;
        Ok(topology)
    }

    pub fn reasoning(&self) -> Option<&str> {
        self.reasoning.as_ref().map(|value| value.0.as_str())
    }

    pub fn reasoning_details(&self) -> Option<&[OpenRouterReasoningDetail]> {
        self.reasoning_details.as_deref()
    }

    pub fn payload_bytes(&self) -> usize {
        sensitive_serialized_len(self)
    }

    pub fn contains_exact_for_reflection_guard(&self, candidate: &str) -> bool {
        self.reasoning()
            .is_some_and(|reasoning| reasoning.contains(candidate))
            || self.reasoning_details.as_ref().is_some_and(|details| {
                details.iter().any(|detail| {
                    let Ok(mut value) = serde_json::from_str(detail.expose_to_provider()) else {
                        return false;
                    };
                    let contains = json_value_contains_exact(&value, candidate);
                    zeroize_json_strings(&mut value);
                    contains
                })
            })
    }

    fn validate(&self) -> Result<(), String> {
        if self
            .reasoning_details
            .as_ref()
            .is_some_and(|details| details.len() > MAX_OPAQUE_REASONING_STATE_COUNT)
        {
            return Err(format!(
                "OpenRouter reasoning topology exceeds the \
                 {MAX_OPAQUE_REASONING_STATE_COUNT}-detail limit"
            ));
        }
        if let Some(details) = &self.reasoning_details {
            for (expected, detail) in details.iter().enumerate() {
                let mut value: serde_json::Value =
                    serde_json::from_str(detail.expose_to_provider()).map_err(|_| {
                        "OpenRouter reasoning topology contains malformed details".to_owned()
                    })?;
                let index = value.get("index").and_then(serde_json::Value::as_u64);
                zeroize_json_strings(&mut value);
                if let Some(index) = index {
                    let expected = u64::try_from(expected)
                        .map_err(|_| "OpenRouter reasoning detail index overflow".to_owned())?;
                    if index != expected {
                        return Err(
                            "OpenRouter reasoning detail indexes must follow logical order"
                                .to_owned(),
                        );
                    }
                }
            }
        }
        if self.payload_bytes() > MAX_OPAQUE_REASONING_TOTAL_BYTES {
            return Err(format!(
                "OpenRouter reasoning topology exceeds the \
                 {MAX_OPAQUE_REASONING_TOTAL_BYTES}-byte limit"
            ));
        }
        Ok(())
    }

    fn zeroize_sensitive_payloads(&mut self) {
        if let Some(reasoning) = &mut self.reasoning {
            reasoning.0.zeroize();
        }
        if let Some(details) = &mut self.reasoning_details {
            for detail in details {
                detail.0.zeroize();
            }
        }
    }
}

impl fmt::Debug for OpenRouterReasoningTopology {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OpenRouterReasoningTopology([REDACTED])")
    }
}

impl Drop for OpenRouterReasoningTopology {
    fn drop(&mut self) {
        self.zeroize_sensitive_payloads();
    }
}

impl<'de> Deserialize<'de> for OpenRouterReasoningTopology {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawTopology {
            #[serde(default)]
            reasoning: Option<OpenRouterReasoningText>,
            #[serde(default)]
            reasoning_details: Option<Vec<OpenRouterReasoningDetail>>,
        }

        let raw = RawTopology::deserialize(deserializer)?;
        let topology = Self {
            reasoning: raw.reasoning,
            reasoning_details: raw.reasoning_details,
        };
        if topology.reasoning.is_none() && topology.reasoning_details.is_none() {
            return Err(D::Error::custom(
                "OpenRouter reasoning topology must retain an observed field",
            ));
        }
        topology.validate().map_err(D::Error::custom)?;
        Ok(topology)
    }
}

fn json_value_contains_exact(value: &serde_json::Value, candidate: &str) -> bool {
    match value {
        serde_json::Value::String(value) => value.contains(candidate),
        serde_json::Value::Array(values) => values
            .iter()
            .any(|value| json_value_contains_exact(value, candidate)),
        serde_json::Value::Object(object) => object.iter().any(|(key, value)| {
            key.contains(candidate) || json_value_contains_exact(value, candidate)
        }),
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {
            false
        }
    }
}

/// Closed provider-native reasoning continuity formats.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OpaqueReasoningState {
    OpenAiResponses {
        item: OpenAiResponsesReasoningItem,
    },
    GeminiThoughtSignature {
        part_index: u32,
        signature: OpaqueReasoningData,
    },
    OpenRouterReasoning {
        topology: OpenRouterReasoningTopology,
    },
    AnthropicMessages {
        content_blocks: AnthropicContentBlockTopology,
    },
}

impl OpaqueReasoningState {
    pub fn payload_bytes(&self) -> usize {
        match self {
            Self::OpenAiResponses { item } => item.payload_bytes(),
            Self::GeminiThoughtSignature { signature, .. } => signature.byte_len(),
            Self::OpenRouterReasoning { topology } => topology.payload_bytes(),
            Self::AnthropicMessages { content_blocks } => content_blocks.serialized_bytes(),
        }
    }

    /// Wipes every owned text payload before a rejected state is discarded.
    ///
    /// This deliberately leaves the value structurally invalid and is only for
    /// fail-closed disposal at a credential boundary.
    pub fn zeroize_sensitive_payloads(&mut self) {
        match self {
            Self::OpenAiResponses { item } => item.zeroize_sensitive_payloads(),
            Self::GeminiThoughtSignature { signature, .. } => signature.0.zeroize(),
            Self::OpenRouterReasoning { topology } => topology.zeroize_sensitive_payloads(),
            Self::AnthropicMessages { content_blocks } => {
                for block in &mut content_blocks.0 {
                    match block {
                        AnthropicContentBlock::Text { text } => text.0.zeroize(),
                        AnthropicContentBlock::Thinking {
                            thinking,
                            signature,
                        } => {
                            thinking.0.zeroize();
                            signature.0.zeroize();
                        }
                        AnthropicContentBlock::RedactedThinking { data } => data.0.zeroize(),
                        AnthropicContentBlock::ToolUse { id, name, input } => {
                            id.0.zeroize();
                            name.0.zeroize();
                            zeroize_json_strings(&mut input.0);
                        }
                    }
                }
            }
        }
    }
}

fn zeroize_json_strings(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(value) => value.zeroize(),
        serde_json::Value::Array(values) => {
            for value in values {
                zeroize_json_strings(value);
            }
        }
        serde_json::Value::Object(values) => {
            for (mut key, mut value) in std::mem::take(values) {
                key.zeroize();
                zeroize_json_strings(&mut value);
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}

fn sensitive_serialized_len<T: Serialize>(value: &T) -> usize {
    let Ok(mut encoded) = serde_json::to_vec(value) else {
        return usize::MAX;
    };
    let len = encoded.len();
    encoded.zeroize();
    len
}

/// One prior state item bound to the assistant message and exact provider
/// target that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpaqueReasoningContext {
    pub source_message_id: crate::MessageId,
    pub api_family: ApiFamily,
    pub model: String,
    pub model_route_id: ModelRouteId,
    pub generation_preset_id: GenerationPresetId,
    pub state: OpaqueReasoningState,
}

pub fn validate_opaque_reasoning_states(states: &[OpaqueReasoningState]) -> Result<(), String> {
    if states.len() > MAX_OPAQUE_REASONING_STATE_COUNT {
        return Err(format!(
            "opaque reasoning state exceeds the {MAX_OPAQUE_REASONING_STATE_COUNT}-item limit"
        ));
    }
    let mut total = 0_usize;
    for state in states {
        total = total
            .checked_add(state.payload_bytes())
            .ok_or_else(|| "opaque reasoning state size overflow".to_owned())?;
        if total > MAX_OPAQUE_REASONING_TOTAL_BYTES {
            return Err(format!(
                "opaque reasoning state exceeds the {MAX_OPAQUE_REASONING_TOTAL_BYTES}-byte limit"
            ));
        }
    }
    let mut serialized = serde_json::to_vec(states)
        .map_err(|_| "opaque reasoning state could not be encoded".to_owned())?;
    let serialized_len = serialized.len();
    serialized.zeroize();
    if serialized_len > MAX_OPAQUE_REASONING_SERIALIZED_BYTES {
        return Err(format!(
            "opaque reasoning state exceeds the \
             {MAX_OPAQUE_REASONING_SERIALIZED_BYTES}-byte serialized JSON limit"
        ));
    }
    Ok(())
}

/// A provider-supplied identifier for one tool call in a generation stream.
///
/// The value is opaque to `LorePia`. It is never interpreted as a command and
/// is bounded before it can enter a public event.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ToolCallId(String);

impl ToolCallId {
    pub fn parse(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        validate_bounded_protocol_text(
            "tool call id",
            &value,
            MAX_TOOL_CALL_ID_BYTES,
            MAX_TOOL_CALL_ID_CHARS,
            false,
        )?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl<'de> Deserialize<'de> for ToolCallId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

/// A provider-supplied function/tool name.
///
/// Provider adapters may impose a narrower provider-specific alphabet. The
/// shared contract only rejects blank, control-containing, or oversized names.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ToolName(String);

impl ToolName {
    pub fn parse(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        validate_bounded_protocol_text(
            "tool name",
            &value,
            MAX_TOOL_NAME_BYTES,
            MAX_TOOL_NAME_CHARS,
            true,
        )?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl<'de> Deserialize<'de> for ToolName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

/// One opaque streamed fragment of tool-call arguments.
///
/// The fragment is not required to be valid JSON by itself because providers
/// may split a JSON value at any byte-safe UTF-8 boundary. `LorePia` forwards it
/// as inert data and does not execute it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ToolCallArgumentsDelta(String);

impl ToolCallArgumentsDelta {
    pub fn parse(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        validate_bounded_protocol_text(
            "tool call arguments delta",
            &value,
            MAX_TOOL_ARGUMENT_DELTA_BYTES,
            MAX_TOOL_ARGUMENT_DELTA_CHARS,
            false,
        )?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl<'de> Deserialize<'de> for ToolCallArgumentsDelta {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

fn validate_bounded_protocol_text(
    label: &str,
    value: &str,
    max_bytes: usize,
    max_chars: usize,
    reject_surrounding_whitespace: bool,
) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    if reject_surrounding_whitespace && value.trim() != value {
        return Err(format!("{label} must not contain surrounding whitespace"));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("{label} must not contain control characters"));
    }
    if value.len() > max_bytes || value.chars().count() > max_chars {
        return Err(format!(
            "{label} exceeds the {max_bytes}-byte or {max_chars}-character limit"
        ));
    }
    Ok(())
}

/// A validated, size-bounded JSON object.
///
/// Provider adapters use this only for typed usage metadata that has no
/// provider-neutral field. They must reconstruct the object from recognized
/// usage counters rather than retaining an arbitrary provider response body.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct BoundedJson(String);

impl BoundedJson {
    pub fn parse(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        validate_bounded_json_size(&value)?;
        let parsed: serde_json::Value =
            serde_json::from_str(&value).map_err(|_| "value must be valid JSON".to_owned())?;
        if !parsed.is_object() {
            return Err("value must be a JSON object".to_owned());
        }
        let canonical =
            serde_json::to_string(&parsed).map_err(|_| "value must be valid JSON".to_owned())?;
        validate_bounded_json_size(&canonical)?;
        Ok(Self(canonical))
    }

    pub fn from_value(value: &serde_json::Value) -> Result<Self, String> {
        if !value.is_object() {
            return Err("value must be a JSON object".to_owned());
        }
        let canonical =
            serde_json::to_string(value).map_err(|_| "value must be valid JSON".to_owned())?;
        validate_bounded_json_size(&canonical)?;
        Ok(Self(canonical))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl<'de> Deserialize<'de> for BoundedJson {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(D::Error::custom)
    }
}

fn validate_bounded_json_size(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err("value must not be empty".to_owned());
    }
    if value.len() > MAX_BOUNDED_JSON_BYTES || value.chars().count() > MAX_BOUNDED_JSON_CHARS {
        return Err(format!(
            "value exceeds the {MAX_BOUNDED_JSON_BYTES}-byte or \
             {MAX_BOUNDED_JSON_CHARS}-character limit"
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationUsage {
    pub input_tokens: Option<u64>,
    #[serde(default)]
    pub cached_read_tokens: Option<u64>,
    #[serde(default)]
    pub cached_write_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    #[serde(default)]
    pub reasoning_tokens: Option<u64>,
    #[serde(default)]
    pub tool_tokens: Option<u64>,
    #[serde(default)]
    pub provider_raw_summary: Option<BoundedJson>,
}

#[cfg(test)]
mod tests {
    use chrono::{TimeDelta, Utc};

    use super::{
        ApiFamily, BoundedJson, CanonicalOrigin, EndpointPath, GenerationPresetId,
        GenerationProviderProvenance, GenerationReasoningSettings, GenerationRequest, HeaderName,
        HttpUrl, MAX_BOUNDED_JSON_BYTES, MAX_BOUNDED_JSON_CHARS, MAX_OPAQUE_REASONING_ITEM_BYTES,
        MAX_OPAQUE_REASONING_SERIALIZED_BYTES, MAX_OPAQUE_REASONING_STATE_COUNT,
        MAX_OPAQUE_REASONING_TOTAL_BYTES, MAX_TOOL_ARGUMENT_DELTA_BYTES, MAX_TOOL_CALL_ID_BYTES,
        MAX_TOOL_NAME_BYTES, ModelRouteId, OpaqueReasoningContext, OpaqueReasoningData,
        OpaqueReasoningState, OpenAiResponsesReasoningItem, OpenRouterReasoningDetail,
        OpenRouterReasoningTopology, PROVIDER_LOCAL_NETWORK_APPROVAL_TTL_SECONDS, ParameterId,
        ParameterLiteral, ParameterValue, ParameterValueState, ToolCallArgumentsDelta, ToolCallId,
        ToolName, provider_local_network_approval_is_active_at, validate_opaque_reasoning_states,
    };

    #[test]
    fn local_network_approval_lifetime_fails_closed_at_expiry_and_for_future_issue_time() {
        let approved_at = Utc::now();
        assert!(provider_local_network_approval_is_active_at(
            approved_at,
            approved_at
        ));
        assert!(provider_local_network_approval_is_active_at(
            approved_at,
            approved_at + TimeDelta::seconds(PROVIDER_LOCAL_NETWORK_APPROVAL_TTL_SECONDS - 1)
        ));
        assert!(!provider_local_network_approval_is_active_at(
            approved_at,
            approved_at + TimeDelta::seconds(PROVIDER_LOCAL_NETWORK_APPROVAL_TTL_SECONDS)
        ));
        assert!(!provider_local_network_approval_is_active_at(
            approved_at,
            approved_at - TimeDelta::seconds(1)
        ));
    }

    #[test]
    fn stable_ids_serialize_as_plain_strings() {
        let id = ModelRouteId::from("route-1");

        assert_eq!(serde_json::to_string(&id).unwrap(), "\"route-1\"");
        assert_eq!(
            serde_json::from_str::<ModelRouteId>("\"route-1\"").unwrap(),
            id
        );
    }

    #[test]
    fn api_family_uses_wire_stable_names() {
        assert_eq!(
            serde_json::to_string(&ApiFamily::OpenAiChatCompletions).unwrap(),
            "\"openai_chat_completions\""
        );
    }

    #[test]
    fn canonical_origin_rejects_paths_and_credentials() {
        assert_eq!(
            CanonicalOrigin::parse("HTTPS://Example.COM:443")
                .unwrap()
                .as_str(),
            "https://example.com"
        );
        assert!(CanonicalOrigin::parse("https://example.com/v1").is_err());
        assert!(CanonicalOrigin::parse("https://user@example.com").is_err());
    }

    #[test]
    fn source_url_and_endpoint_path_reject_unsafe_forms() {
        assert!(HttpUrl::parse("file:///tmp/catalog.json").is_err());
        assert!(HttpUrl::parse("https://user@example.com/docs").is_err());
        assert!(EndpointPath::parse("/v1/models").is_ok());
        assert!(EndpointPath::parse("//example.com/models").is_err());
        assert!(EndpointPath::parse("/v1/../admin").is_err());
        assert!(EndpointPath::parse("/v1/%2e%2e/admin").is_err());
    }

    #[test]
    fn header_name_is_validated_and_normalized() {
        assert_eq!(
            HeaderName::parse("X-API-Key").unwrap().as_str(),
            "x-api-key"
        );
        assert!(HeaderName::parse("bad header").is_err());
        assert!(HeaderName::parse("").is_err());
    }

    #[test]
    fn parameter_value_preserves_provider_default_vs_explicit() {
        let inherited = ParameterValue {
            parameter_id: ParameterId::from("temperature"),
            state: ParameterValueState::InheritProviderDefault,
        };
        let explicit = ParameterValue {
            parameter_id: ParameterId::from("temperature"),
            state: ParameterValueState::Explicit(ParameterLiteral::Number(0.7)),
        };

        assert_ne!(
            serde_json::to_value(inherited).unwrap(),
            serde_json::to_value(explicit).unwrap()
        );
    }

    #[test]
    fn reasoning_defaults_do_not_opt_into_opaque_provider_state() {
        assert!(!GenerationReasoningSettings::default().preserve_opaque_state);
    }

    #[test]
    fn generation_request_debug_redacts_provider_controlled_scalars() {
        let model_canary = "sk-reflected-model-canary-never-log";
        let execution_hash_canary = "sk-unverified-execution-hash-never-log";
        let route_canary = "sk-reflected-route-canary-never-log";
        let preset_canary = "sk-reflected-preset-canary-never-log";
        let request = GenerationRequest {
            generation_id: crate::GenerationId::new(),
            conversation_id: crate::ConversationId::new(),
            model: model_canary.to_owned(),
            messages: Vec::new(),
            resolved_prompt_plan: None,
            provider_execution_plan_hash: Some(execution_hash_canary.to_owned()),
            temperature: None,
            max_output_tokens: None,
            provider_provenance: Some(GenerationProviderProvenance {
                api_family: ApiFamily::OpenAiResponses,
                model_route_id: ModelRouteId::from(route_canary),
                generation_preset_id: GenerationPresetId::from(preset_canary),
            }),
            preserve_opaque_reasoning_state: false,
            opaque_reasoning_context: Vec::new(),
        };

        let debug = format!("{request:?}");
        assert!(!debug.contains(model_canary));
        assert!(!debug.contains(execution_hash_canary));
        assert!(!debug.contains(route_canary));
        assert!(!debug.contains(preset_canary));
        assert!(debug.contains("model_bytes"));
        assert!(debug.contains("has_provider_execution_plan_hash: true"));
        assert!(debug.contains("provider_api_family: Some(OpenAiResponses)"));
    }

    #[test]
    fn bounded_json_canonicalizes_objects_and_rejects_invalid_shapes() {
        let summary = BoundedJson::parse(r#"{ "total_tokens": 7 }"#).unwrap();
        assert_eq!(summary.as_str(), r#"{"total_tokens":7}"#);
        assert!(BoundedJson::parse("[]").is_err());
        assert!(BoundedJson::parse("{not-json}").is_err());
        assert!(BoundedJson::parse("").is_err());
    }

    #[test]
    fn bounded_json_enforces_limits_during_construction_and_deserialization() {
        let oversized_bytes = format!(r#"{{"value":"{}"}}"#, "a".repeat(MAX_BOUNDED_JSON_BYTES));
        assert!(BoundedJson::parse(oversized_bytes).is_err());

        let oversized_chars = format!(r#"{{"value":"{}"}}"#, "😀".repeat(MAX_BOUNDED_JSON_CHARS));
        assert!(BoundedJson::parse(oversized_chars).is_err());

        let encoded = serde_json::to_string(&format!(
            r#"{{"value":"{}"}}"#,
            "a".repeat(MAX_BOUNDED_JSON_BYTES)
        ))
        .unwrap();
        assert!(serde_json::from_str::<BoundedJson>(&encoded).is_err());
    }

    #[test]
    fn tool_protocol_values_are_bounded_during_construction_and_deserialization() {
        let id = ToolCallId::parse("call-1").expect("valid id");
        let name = ToolName::parse("lookup_weather").expect("valid name");
        let delta = ToolCallArgumentsDelta::parse(r#"{"city":"Seoul"}"#).expect("valid delta");

        assert_eq!(id.as_str(), "call-1");
        assert_eq!(name.as_str(), "lookup_weather");
        assert_eq!(delta.as_str(), r#"{"city":"Seoul"}"#);
        assert!(ToolCallId::parse("").is_err());
        assert!(ToolCallId::parse("a".repeat(MAX_TOOL_CALL_ID_BYTES + 1)).is_err());
        assert!(ToolName::parse(" padded ").is_err());
        assert!(ToolName::parse("a".repeat(MAX_TOOL_NAME_BYTES + 1)).is_err());
        assert!(
            ToolCallArgumentsDelta::parse("a".repeat(MAX_TOOL_ARGUMENT_DELTA_BYTES + 1)).is_err()
        );
        assert!(serde_json::from_str::<ToolCallId>("\"\\n\"").is_err());
        assert!(serde_json::from_str::<ToolName>("\" tool\"").is_err());
    }

    #[test]
    fn opaque_reasoning_debug_is_redacted_and_json_shape_is_closed() {
        let canary = "opaque-secret-canary";
        let item_id_canary = "opaque-item-id-canary";
        let state = OpaqueReasoningState::OpenAiResponses {
            item: OpenAiResponsesReasoningItem::from_value(&serde_json::json!({
                "id": item_id_canary,
                "type": "reasoning",
                "summary": [{"type": "summary_text", "text": "private summary"}],
                "content": [{"type": "reasoning_text", "text": "private content"}],
                "encrypted_content": canary,
                "status": "completed"
            }))
            .expect("reasoning item"),
        };

        let debug = format!("{state:?}");
        assert!(!debug.contains(canary));
        assert!(!debug.contains(item_id_canary));
        assert!(debug.contains("[REDACTED]"));
        let encoded = serde_json::to_string(&state).expect("encode state");
        assert_eq!(
            serde_json::from_str::<OpaqueReasoningState>(&encoded).expect("decode state"),
            state
        );
        assert!(
            serde_json::from_str::<OpaqueReasoningState>(
                r#"{"kind":"gemini_thought_signature","part_index":0,"signature":{"nested":"value"}}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<OpaqueReasoningState>(
                r#"{"kind":"gemini_thought_signature","part_index":0,"signature":"value","unknown":true}"#
            )
            .is_err()
        );

        let request = GenerationRequest {
            generation_id: crate::GenerationId::new(),
            conversation_id: crate::ConversationId::new(),
            model: "model".to_owned(),
            messages: Vec::new(),
            resolved_prompt_plan: None,
            provider_execution_plan_hash: None,
            temperature: None,
            max_output_tokens: None,
            provider_provenance: None,
            preserve_opaque_reasoning_state: true,
            opaque_reasoning_context: vec![OpaqueReasoningContext {
                source_message_id: crate::MessageId::new(),
                api_family: ApiFamily::OpenAiResponses,
                model: "model".to_owned(),
                model_route_id: ModelRouteId::from("route"),
                generation_preset_id: GenerationPresetId::from("preset"),
                state,
            }],
        };
        let encoded_request = serde_json::to_string(&request).expect("encode request");
        assert!(!encoded_request.contains(canary));
        assert!(!encoded_request.contains("opaque_reasoning_context"));
    }

    #[test]
    fn openai_reasoning_items_require_closed_exact_shapes() {
        let openai = serde_json::json!({
            "id": "reasoning-item-1",
            "type": "reasoning",
            "summary": [{"type": "summary_text", "text": "private-summary-canary"}],
            "content": [{"type": "reasoning_text", "text": "private-content-canary"}],
            "encrypted_content": "private-encrypted-canary",
            "status": "completed"
        });
        let item = OpenAiResponsesReasoningItem::from_value(&openai).expect("OpenAI item");
        assert_eq!(item.to_provider_value().expect("provider value"), openai);
        assert!(!format!("{item:?}").contains("private-summary-canary"));
        for invalid in [
            serde_json::json!({
                "id": "reasoning-item-1",
                "type": "reasoning",
                "summary": [],
                "unknown": true
            }),
            serde_json::json!({
                "id": "reasoning-item-1",
                "type": "reasoning",
                "summary": [{"type": "wrong", "text": "summary"}]
            }),
            serde_json::json!({
                "id": "reasoning-item-1",
                "type": "reasoning",
                "summary": [],
                "status": "unknown"
            }),
        ] {
            assert!(OpenAiResponsesReasoningItem::from_value(&invalid).is_err());
        }
    }

    #[test]
    fn openrouter_reasoning_details_preserve_nullish_exact_shapes() {
        let openrouter = serde_json::json!({
            "type": "reasoning.encrypted",
            "data": "opaque",
            "id": null,
            "format": "anthropic-claude-v1"
        });
        assert!(OpenRouterReasoningDetail::from_value(&openrouter).is_ok());
        for exact in [
            serde_json::json!({
                "type": "reasoning.text",
                "signature": "signature-only"
            }),
            serde_json::json!({
                "type": "reasoning.text",
                "text": null,
                "signature": "signature-only",
                "id": null,
                "format": null,
                "index": null
            }),
        ] {
            let detail =
                OpenRouterReasoningDetail::from_value(&exact).expect("valid nullish text detail");
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(detail.expose_to_provider())
                    .expect("canonical detail"),
                exact
            );
            let encoded = serde_json::to_string(&detail).expect("encode detail");
            assert_eq!(
                serde_json::from_str::<OpenRouterReasoningDetail>(&encoded).expect("decode detail"),
                detail
            );
        }
        let missing_text = OpenRouterReasoningDetail::from_value(&serde_json::json!({
            "type": "reasoning.text",
            "signature": "missing-text"
        }))
        .expect("missing text is exact");
        let null_text = OpenRouterReasoningDetail::from_value(&serde_json::json!({
            "type": "reasoning.text",
            "text": null,
            "signature": "null-text",
            "id": null,
            "format": null
        }))
        .expect("null text is exact");
        let topology = OpenRouterReasoningTopology::new(None, Some(vec![missing_text, null_text]))
            .expect("nullish topology");
        let encoded = serde_json::to_string(&topology).expect("encode topology");
        assert_eq!(
            serde_json::from_str::<OpenRouterReasoningTopology>(&encoded).expect("decode topology"),
            topology
        );

        for invalid in [
            serde_json::json!({"type": "reasoning.summary", "summary": null}),
            serde_json::json!({"type": "reasoning.encrypted", "data": null}),
            serde_json::json!({"type": "reasoning.text", "text": 1}),
            serde_json::json!({"type": "reasoning.text", "id": false}),
            serde_json::json!({"type": "reasoning.text", "format": false}),
            serde_json::json!({"type": "reasoning.text", "index": -1}),
        ] {
            assert!(OpenRouterReasoningDetail::from_value(&invalid).is_err());
        }
        let oversized_canary = "private-oversized-openrouter-canary";
        let oversized = serde_json::json!({
            "type": "reasoning.text",
            "text": format!(
                "{oversized_canary}{}",
                "x".repeat(MAX_OPAQUE_REASONING_ITEM_BYTES)
            )
        });
        let error = OpenRouterReasoningDetail::from_value(&oversized)
            .expect_err("oversized canonical detail");
        assert!(!error.contains(oversized_canary));
    }

    #[test]
    fn opaque_reasoning_collection_limits_fail_closed() {
        let state = OpaqueReasoningState::GeminiThoughtSignature {
            part_index: 0,
            signature: OpaqueReasoningData::parse("signature").expect("signature"),
        };
        assert!(
            validate_opaque_reasoning_states(&vec![
                state.clone();
                MAX_OPAQUE_REASONING_STATE_COUNT + 1
            ])
            .is_err()
        );

        let large_state = OpaqueReasoningState::GeminiThoughtSignature {
            part_index: 0,
            signature: OpaqueReasoningData::parse("s".repeat(60 * 1024))
                .expect("individually bounded signature"),
        };
        assert!(validate_opaque_reasoning_states(&vec![large_state; 5]).is_err());
    }

    fn gemini_states_with_serialized_len(target: usize) -> Vec<OpaqueReasoningState> {
        fn build(counts: [usize; 4], append_plain_byte: bool) -> Vec<OpaqueReasoningState> {
            counts
                .into_iter()
                .enumerate()
                .map(|(part_index, count)| {
                    let mut signature = "\\".repeat(count);
                    if append_plain_byte && part_index == 0 {
                        signature.push('a');
                    }
                    OpaqueReasoningState::GeminiThoughtSignature {
                        part_index: u32::try_from(part_index).expect("bounded part index"),
                        signature: OpaqueReasoningData::parse(signature)
                            .expect("bounded backslash-heavy signature"),
                    }
                })
                .collect()
        }

        let mut counts = [1_usize; 4];
        let baseline = serde_json::to_vec(&build(counts, false))
            .expect("serialize baseline opaque state")
            .len();
        let mut remaining = target
            .checked_sub(baseline)
            .expect("target must fit the fixed state envelope");
        let append_plain_byte = remaining % 2 == 1;
        remaining -= usize::from(append_plain_byte);
        let mut extra_backslashes = remaining / 2;
        for (index, count) in counts.iter_mut().enumerate() {
            let suffix_bytes = usize::from(append_plain_byte && index == 0);
            let capacity = MAX_OPAQUE_REASONING_ITEM_BYTES - *count - suffix_bytes;
            let added = capacity.min(extra_backslashes);
            *count += added;
            extra_backslashes -= added;
        }
        assert_eq!(extra_backslashes, 0, "target exceeds domain item bounds");
        let states = build(counts, append_plain_byte);
        assert_eq!(
            serde_json::to_vec(&states)
                .expect("serialize exact opaque state")
                .len(),
            target
        );
        assert!(
            states
                .iter()
                .map(OpaqueReasoningState::payload_bytes)
                .sum::<usize>()
                <= MAX_OPAQUE_REASONING_TOTAL_BYTES
        );
        states
    }

    #[test]
    fn opaque_reasoning_serialized_json_limit_is_exact_and_shared() {
        let at_limit = gemini_states_with_serialized_len(MAX_OPAQUE_REASONING_SERIALIZED_BYTES);
        validate_opaque_reasoning_states(&at_limit)
            .expect("the exact durable JSON envelope must remain valid");

        let escape_expanded =
            gemini_states_with_serialized_len(MAX_OPAQUE_REASONING_SERIALIZED_BYTES + 2);
        let error = validate_opaque_reasoning_states(&escape_expanded)
            .expect_err("escape expansion past the durable JSON envelope must fail");
        assert!(error.contains("serialized JSON limit"));
    }
}
