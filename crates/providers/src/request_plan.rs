use std::{error::Error, fmt};

use lorepia_domain::{
    ApiFamily, CanonicalOrigin, CoreError, CoreResult, EndpointPath, HeaderName, HttpMethod,
};
use serde::Serialize;
use serde_json::{Map, Value, json};
use url::Url;

use crate::discovery::contains_credential_like_token;
use crate::parameter_mapping::{
    AnthropicCacheTtl, PromptCacheDirective, PromptCacheTtl, ProviderRequestPlan, RequestBodyPatch,
};
use crate::setup_assistant::ASSISTANT_OUTPUT_SCHEMA_NAME;

const MAX_PREVIEW_ORIGIN_BYTES: usize = 512;
const MAX_PREVIEW_PATH_BYTES: usize = 2_048;
const MAX_PREVIEW_PATH_SEGMENT_BYTES: usize = 256;
const MAX_PREVIEW_QUERY_BYTES: usize = 16 * 1024;
const MAX_PREVIEW_QUERY_PARAMETERS: usize = 64;
const MAX_PREVIEW_QUERY_NAME_BYTES: usize = 128;
const MAX_PREVIEW_ENCODED_QUERY_NAME_BYTES: usize = 512;
const MAX_PREVIEW_HEADER_NAMES: usize = 64;
const MAX_PREVIEW_HEADER_NAME_BYTES: usize = 128;
const MAX_PREVIEW_BODY_DEPTH: usize = 16;
const MAX_PREVIEW_BODY_NODES: usize = 256;
const MAX_PREVIEW_OBJECT_FIELDS: usize = 32;
const MAX_PREVIEW_ARRAY_ITEMS: usize = 8;
const MAX_PREVIEW_BODY_FIELD_NAME_BYTES: usize = 128;
const REDACTED_PATH_SEGMENT: &str = "__redacted__";
const REDACTED_BODY_FIELD_NAME: &str = "__redacted_field__";
pub(crate) const CAPABILITY_PROBE_TOOL_NAME: &str = "lorepia_probe";

/// The two provider features whose success requires an adapter-owned wire
/// contract rather than a prompt-only inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CapabilityProbePlanKind {
    StructuredOutput,
    ToolCalling,
}

/// Builds the fixed, family-specific request patch used by one capability
/// probe.
///
/// This constructor is crate-private and accepts no provider-controlled field
/// name or value. It intentionally bypasses the user-parameter mapper only for
/// these two adapter-owned fields, both of which are otherwise reserved.
#[allow(clippy::too_many_lines)]
pub(crate) fn capability_probe_request_plan(
    family: ApiFamily,
    kind: CapabilityProbePlanKind,
) -> ProviderRequestPlan {
    let schema = capability_probe_schema();
    let body_patches = match (family, kind) {
        (ApiFamily::OpenAiResponses, CapabilityProbePlanKind::StructuredOutput) => vec![patch(
            "text.format",
            json!({
                "type": "json_schema",
                "name": "lorepia_probe",
                "schema": schema,
                "strict": true,
            }),
        )],
        (ApiFamily::OpenAiChatCompletions, CapabilityProbePlanKind::StructuredOutput) => {
            vec![patch(
                "response_format",
                json!({
                    "type": "json_schema",
                    "json_schema": {
                        "name": "lorepia_probe",
                        "schema": schema,
                        "strict": true,
                    },
                }),
            )]
        }
        (ApiFamily::AnthropicMessages, CapabilityProbePlanKind::StructuredOutput) => vec![patch(
            "output_config.format",
            json!({
                "type": "json_schema",
                "schema": schema,
            }),
        )],
        (ApiFamily::GeminiGenerateContent, CapabilityProbePlanKind::StructuredOutput) => vec![
            patch(
                "generationConfig.responseMimeType",
                json!("application/json"),
            ),
            patch("generationConfig.responseJsonSchema", schema),
        ],
        (ApiFamily::OllamaNative, CapabilityProbePlanKind::StructuredOutput) => {
            vec![patch("format", schema)]
        }
        (ApiFamily::OpenAiResponses, CapabilityProbePlanKind::ToolCalling) => vec![
            patch(
                "tools",
                json!([{
                    "type": "function",
                    "name": CAPABILITY_PROBE_TOOL_NAME,
                    "description": "Return the fixed capability probe argument.",
                    "parameters": schema,
                    "strict": true,
                }]),
            ),
            patch(
                "tool_choice",
                json!({
                    "type": "function",
                    "name": CAPABILITY_PROBE_TOOL_NAME,
                }),
            ),
            patch("parallel_tool_calls", json!(false)),
        ],
        (ApiFamily::OpenAiChatCompletions, CapabilityProbePlanKind::ToolCalling) => vec![
            patch(
                "tools",
                json!([{
                    "type": "function",
                    "function": {
                        "name": CAPABILITY_PROBE_TOOL_NAME,
                        "description": "Return the fixed capability probe argument.",
                        "parameters": schema,
                        "strict": true,
                    },
                }]),
            ),
            patch(
                "tool_choice",
                json!({
                    "type": "function",
                    "function": {
                        "name": CAPABILITY_PROBE_TOOL_NAME,
                    },
                }),
            ),
            patch("parallel_tool_calls", json!(false)),
        ],
        (ApiFamily::AnthropicMessages, CapabilityProbePlanKind::ToolCalling) => vec![
            patch(
                "tools",
                json!([{
                    "name": CAPABILITY_PROBE_TOOL_NAME,
                    "description": "Return the fixed capability probe argument.",
                    "input_schema": schema,
                }]),
            ),
            patch(
                "tool_choice",
                json!({
                    "type": "tool",
                    "name": CAPABILITY_PROBE_TOOL_NAME,
                    "disable_parallel_tool_use": true,
                }),
            ),
        ],
        (ApiFamily::GeminiGenerateContent, CapabilityProbePlanKind::ToolCalling) => vec![
            patch(
                "tools",
                json!([{
                    "functionDeclarations": [{
                        "name": CAPABILITY_PROBE_TOOL_NAME,
                        "description": "Return the fixed capability probe argument.",
                        "parametersJsonSchema": schema,
                    }],
                }]),
            ),
            patch(
                "toolConfig.functionCallingConfig",
                json!({
                    "mode": "ANY",
                    "allowedFunctionNames": [CAPABILITY_PROBE_TOOL_NAME],
                }),
            ),
        ],
        (ApiFamily::OllamaNative, CapabilityProbePlanKind::ToolCalling) => vec![patch(
            "tools",
            json!([{
                "type": "function",
                "function": {
                    "name": CAPABILITY_PROBE_TOOL_NAME,
                    "description": "Return the fixed capability probe argument.",
                    "parameters": schema,
                },
            }]),
        )],
    };
    ProviderRequestPlan {
        family,
        body_patches,
        prompt_cache: PromptCacheDirective::None,
        cache_dialect: crate::parameter_mapping::PromptCacheWireDialect::Unsupported,
        preserve_opaque_reasoning_state: false,
    }
}

/// Builds the fixed structured-output contract for one setup-assistant call.
///
/// `family` is the selected assistant route's transport dialect. The schema's
/// API-family enum is built separately from the target provider allowlist, so
/// the assistant transport can never silently broaden the manifest families
/// the session is permitted to draft.
#[allow(clippy::too_many_lines)]
pub(crate) fn setup_assistant_request_plan(
    family: ApiFamily,
    schema: Value,
) -> ProviderRequestPlan {
    let body_patches = match family {
        ApiFamily::OpenAiResponses => vec![patch(
            "text.format",
            json!({
                "type": "json_schema",
                "name": ASSISTANT_OUTPUT_SCHEMA_NAME,
                "schema": schema,
                "strict": true,
            }),
        )],
        ApiFamily::OpenAiChatCompletions => vec![patch(
            "response_format",
            json!({
                "type": "json_schema",
                "json_schema": {
                    "name": ASSISTANT_OUTPUT_SCHEMA_NAME,
                    "schema": schema,
                    "strict": true,
                },
            }),
        )],
        ApiFamily::AnthropicMessages => vec![patch(
            "output_config.format",
            json!({
                "type": "json_schema",
                "schema": schema,
            }),
        )],
        ApiFamily::GeminiGenerateContent => vec![
            patch(
                "generationConfig.responseMimeType",
                json!("application/json"),
            ),
            patch("generationConfig.responseJsonSchema", schema),
        ],
        ApiFamily::OllamaNative => vec![patch("format", schema)],
    };
    ProviderRequestPlan {
        family,
        body_patches,
        prompt_cache: PromptCacheDirective::None,
        cache_dialect: crate::parameter_mapping::PromptCacheWireDialect::Unsupported,
        preserve_opaque_reasoning_state: false,
    }
}

fn capability_probe_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "probe": {
                "type": "string",
                "enum": ["ok"],
            },
        },
        "required": ["probe"],
        "additionalProperties": false,
    })
}

fn patch(path: &'static str, value: Value) -> RequestBodyPatch {
    RequestBodyPatch {
        path: path.to_owned(),
        value,
    }
}

/// Scalar-free shape of one provider request body.
///
/// Scalar variants describe only their JSON type. No string, number, boolean,
/// private prompt, credential, or opaque provider-state value has a
/// representation in this type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RequestBodyShape {
    Null,
    Boolean,
    Number,
    String,
    Array {
        items: Vec<Self>,
        truncated: bool,
    },
    Object {
        fields: Vec<RequestBodyField>,
        truncated: bool,
    },
    /// The field is structurally present, but even its nested shape is private.
    Redacted,
    /// A structural bound was reached before this subtree could be described.
    Truncated,
}

/// One safe field name and its scalar-free request-body shape.
///
/// Fields are private so arbitrary strings cannot be inserted into a value
/// advertised as safe to serialize.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RequestBodyField {
    name: String,
    shape: RequestBodyShape,
}

impl RequestBodyField {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn shape(&self) -> &RequestBodyShape {
        &self.shape
    }
}

/// A bounded request preview that is safe to render, debug, or serialize.
///
/// It contains header names, but their values cannot be represented. The
/// destination is split into a canonical origin and a credential-free path.
/// Only bounded, decoded query parameter names are represented; query values
/// and the fragment have no representation. Body values are reduced to
/// [`RequestBodyShape`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RequestPreview {
    method: HttpMethod,
    origin: CanonicalOrigin,
    path: EndpointPath,
    query_parameter_names: Vec<String>,
    header_names: Vec<HeaderName>,
    body: Option<RequestBodyShape>,
    body_truncated: bool,
}

impl RequestPreview {
    pub fn method(&self) -> HttpMethod {
        self.method
    }

    pub fn origin(&self) -> &CanonicalOrigin {
        &self.origin
    }

    pub fn path(&self) -> &EndpointPath {
        &self.path
    }

    pub fn query_parameter_names(&self) -> &[String] {
        &self.query_parameter_names
    }

    pub fn header_names(&self) -> &[HeaderName] {
        &self.header_names
    }

    pub fn body(&self) -> Option<&RequestBodyShape> {
        self.body.as_ref()
    }

    pub const fn body_truncated(&self) -> bool {
        self.body_truncated
    }
}

/// Stable, secret-free reason a request preview could not be constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestPreviewErrorKind {
    InvalidDestination,
    EmbeddedCredential,
    DestinationTooLarge,
    InvalidPath,
    QueryTooLarge,
    TooManyQueryParameters,
    InvalidQueryName,
    TooManyHeaderNames,
    InvalidHeaderName,
}

/// A request-preview error which never retains destination, header, query, or
/// body input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestPreviewError {
    kind: RequestPreviewErrorKind,
}

impl RequestPreviewError {
    const fn new(kind: RequestPreviewErrorKind) -> Self {
        Self { kind }
    }

    pub const fn kind(self) -> RequestPreviewErrorKind {
        self.kind
    }
}

impl fmt::Display for RequestPreviewError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.kind {
            RequestPreviewErrorKind::InvalidDestination => {
                "request preview destination must be an HTTP URL with a host"
            }
            RequestPreviewErrorKind::EmbeddedCredential => {
                "request preview destination must not contain embedded credentials"
            }
            RequestPreviewErrorKind::DestinationTooLarge => {
                "request preview destination exceeds a structural size limit"
            }
            RequestPreviewErrorKind::InvalidPath => {
                "request preview destination contains an unsafe path"
            }
            RequestPreviewErrorKind::QueryTooLarge => {
                "request preview query exceeds a structural size limit"
            }
            RequestPreviewErrorKind::TooManyQueryParameters => {
                "request preview contains too many query parameters"
            }
            RequestPreviewErrorKind::InvalidQueryName => {
                "request preview contains an unsafe query parameter name"
            }
            RequestPreviewErrorKind::TooManyHeaderNames => {
                "request preview contains too many header names"
            }
            RequestPreviewErrorKind::InvalidHeaderName => {
                "request preview contains an unsafe header name"
            }
        })
    }
}

impl Error for RequestPreviewError {}

/// Builds a structured preview without accepting any header value.
///
/// Query values and the URL fragment are ignored, JSON scalar values are
/// discarded, sensitive prompt/state subtrees are collapsed, and opaque
/// credential-like path segments are replaced. The returned error contains
/// only a stable category and never retains any input.
pub fn build_request_preview(
    method: HttpMethod,
    destination: &Url,
    header_names: &[HeaderName],
    body: Option<&Value>,
) -> Result<RequestPreview, RequestPreviewError> {
    let origin = preview_origin(destination)?;
    let path = preview_path(destination)?;
    let query_parameter_names = preview_query_parameter_names(destination)?;
    let header_names = preview_header_names(header_names)?;
    let mut budget = BodyShapeBudget::new();
    let body = body.map(|value| request_body_shape(value, None, 0, &mut budget));
    let body_truncated = budget.truncated();

    Ok(RequestPreview {
        method,
        origin,
        path,
        query_parameter_names,
        header_names,
        body,
        body_truncated,
    })
}

fn preview_origin(destination: &Url) -> Result<CanonicalOrigin, RequestPreviewError> {
    if !matches!(destination.scheme(), "http" | "https") || destination.host_str().is_none() {
        return Err(RequestPreviewError::new(
            RequestPreviewErrorKind::InvalidDestination,
        ));
    }
    if !destination.username().is_empty() || destination.password().is_some() {
        return Err(RequestPreviewError::new(
            RequestPreviewErrorKind::EmbeddedCredential,
        ));
    }
    if destination.host_str().is_some_and(unsafe_origin_host) {
        return Err(RequestPreviewError::new(
            RequestPreviewErrorKind::InvalidDestination,
        ));
    }
    let value = destination.origin().ascii_serialization();
    if value.len() > MAX_PREVIEW_ORIGIN_BYTES {
        return Err(RequestPreviewError::new(
            RequestPreviewErrorKind::DestinationTooLarge,
        ));
    }
    CanonicalOrigin::parse(&value)
        .map_err(|_| RequestPreviewError::new(RequestPreviewErrorKind::InvalidDestination))
}

fn preview_path(destination: &Url) -> Result<EndpointPath, RequestPreviewError> {
    let raw_path = destination.path();
    if raw_path.len() > MAX_PREVIEW_PATH_BYTES {
        return Err(RequestPreviewError::new(
            RequestPreviewErrorKind::DestinationTooLarge,
        ));
    }

    let mut path = String::with_capacity(raw_path.len());
    let mut redact_next = false;
    for (index, raw_segment) in raw_path.split('/').enumerate() {
        if index > 0 {
            path.push('/');
        }
        if raw_segment.is_empty() {
            continue;
        }
        let decoded = percent_decode(raw_segment)
            .ok_or_else(|| RequestPreviewError::new(RequestPreviewErrorKind::InvalidPath))?;
        let sensitive_location = is_sensitive_location_name(&decoded);
        if redact_next || unsafe_path_segment(raw_segment, &decoded) {
            path.push_str(REDACTED_PATH_SEGMENT);
        } else {
            path.push_str(raw_segment);
        }
        redact_next = sensitive_location;
    }

    EndpointPath::parse(&path)
        .map_err(|_| RequestPreviewError::new(RequestPreviewErrorKind::InvalidPath))
}

fn unsafe_path_segment(raw: &str, decoded: &str) -> bool {
    raw.len() > MAX_PREVIEW_PATH_SEGMENT_BYTES
        || decoded.len() > MAX_PREVIEW_PATH_SEGMENT_BYTES
        || decoded.contains('@')
        || decoded.chars().any(char::is_control)
        || contains_inline_secret_location(decoded)
        || contains_credential_like_token(decoded)
}

fn unsafe_origin_host(value: &str) -> bool {
    let value = value.trim_matches(['[', ']']);
    if value.parse::<std::net::IpAddr>().is_ok() {
        return false;
    }
    contains_credential_like_token(value) || value.split('.').any(contains_credential_like_token)
}

fn contains_inline_secret_location(value: &str) -> bool {
    ['=', ':'].into_iter().any(|separator| {
        value
            .split_once(separator)
            .is_some_and(|(name, _)| is_sensitive_location_name(name))
    })
}

fn preview_query_parameter_names(destination: &Url) -> Result<Vec<String>, RequestPreviewError> {
    let Some(query) = destination.query() else {
        return Ok(Vec::new());
    };
    if query.len() > MAX_PREVIEW_QUERY_BYTES {
        return Err(RequestPreviewError::new(
            RequestPreviewErrorKind::QueryTooLarge,
        ));
    }
    if query.is_empty() {
        return Ok(Vec::new());
    }

    let mut names = Vec::new();
    for parameter in query.split('&') {
        if names.len() >= MAX_PREVIEW_QUERY_PARAMETERS {
            return Err(RequestPreviewError::new(
                RequestPreviewErrorKind::TooManyQueryParameters,
            ));
        }
        let raw_name = parameter
            .split_once('=')
            .map_or(parameter, |(name, _)| name);
        if raw_name.is_empty() || raw_name.len() > MAX_PREVIEW_ENCODED_QUERY_NAME_BYTES {
            return Err(RequestPreviewError::new(
                RequestPreviewErrorKind::InvalidQueryName,
            ));
        }
        let form_decoded = raw_name.replace('+', " ");
        let name = percent_decode(&form_decoded)
            .ok_or_else(|| RequestPreviewError::new(RequestPreviewErrorKind::InvalidQueryName))?;
        if name.is_empty()
            || name.len() > MAX_PREVIEW_QUERY_NAME_BYTES
            || name.chars().any(|character| {
                character.is_control()
                    || character.is_whitespace()
                    || matches!(character, '&' | '=' | '?' | '#')
            })
            || contains_credential_like_token(&name)
        {
            return Err(RequestPreviewError::new(
                RequestPreviewErrorKind::InvalidQueryName,
            ));
        }
        names.push(name);
    }
    names.sort_unstable();
    names.dedup();
    Ok(names)
}

fn preview_header_names(
    header_names: &[HeaderName],
) -> Result<Vec<HeaderName>, RequestPreviewError> {
    if header_names.len() > MAX_PREVIEW_HEADER_NAMES {
        return Err(RequestPreviewError::new(
            RequestPreviewErrorKind::TooManyHeaderNames,
        ));
    }
    let mut names = header_names.to_vec();
    if names.iter().any(|name| {
        name.as_str().len() > MAX_PREVIEW_HEADER_NAME_BYTES
            || contains_credential_like_token(name.as_str())
    }) {
        return Err(RequestPreviewError::new(
            RequestPreviewErrorKind::InvalidHeaderName,
        ));
    }
    names.sort_unstable_by(|left, right| left.as_str().cmp(right.as_str()));
    names.dedup_by(|left, right| left.as_str() == right.as_str());
    Ok(names)
}

fn percent_decode(value: &str) -> Option<String> {
    let mut decoded = value.to_owned();
    for _ in 0..4 {
        if !decoded.as_bytes().contains(&b'%') {
            return Some(decoded);
        }
        decoded = percent_decode_once(&decoded)?;
    }
    if decoded.as_bytes().contains(&b'%') {
        return None;
    }
    Some(decoded)
}

fn percent_decode_once(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                let high = bytes.get(index + 1).and_then(|byte| hex_value(*byte))?;
                let low = bytes.get(index + 2).and_then(|byte| hex_value(*byte))?;
                decoded.push((high << 4) | low);
                index += 3;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(decoded).ok()
}

const fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

struct BodyShapeBudget {
    remaining_nodes: usize,
    truncated: bool,
}

impl BodyShapeBudget {
    const fn new() -> Self {
        Self {
            remaining_nodes: MAX_PREVIEW_BODY_NODES,
            truncated: false,
        }
    }

    fn consume(&mut self) -> bool {
        let Some(remaining) = self.remaining_nodes.checked_sub(1) else {
            self.truncated = true;
            return false;
        };
        self.remaining_nodes = remaining;
        true
    }

    fn mark_truncated(&mut self) {
        self.truncated = true;
    }

    const fn truncated(&self) -> bool {
        self.truncated
    }
}

fn request_body_shape(
    value: &Value,
    field_name: Option<&str>,
    depth: usize,
    budget: &mut BodyShapeBudget,
) -> RequestBodyShape {
    if !budget.consume() {
        return RequestBodyShape::Truncated;
    }
    if field_name.is_some_and(body_field_requires_redaction) {
        return RequestBodyShape::Redacted;
    }
    if depth >= MAX_PREVIEW_BODY_DEPTH {
        budget.mark_truncated();
        return RequestBodyShape::Truncated;
    }

    match value {
        Value::Null => RequestBodyShape::Null,
        Value::Bool(_) => RequestBodyShape::Boolean,
        Value::Number(_) => RequestBodyShape::Number,
        Value::String(_) => RequestBodyShape::String,
        Value::Array(values) => {
            let mut items = Vec::with_capacity(values.len().min(MAX_PREVIEW_ARRAY_ITEMS));
            for value in values.iter().take(MAX_PREVIEW_ARRAY_ITEMS) {
                if budget.remaining_nodes == 0 {
                    budget.mark_truncated();
                    break;
                }
                items.push(request_body_shape(value, None, depth + 1, budget));
            }
            let truncated = items.len() < values.len();
            if truncated {
                budget.mark_truncated();
            }
            RequestBodyShape::Array { items, truncated }
        }
        Value::Object(values) => {
            let mut fields = Vec::with_capacity(values.len().min(MAX_PREVIEW_OBJECT_FIELDS));
            for (name, value) in values.iter().take(MAX_PREVIEW_OBJECT_FIELDS) {
                if budget.remaining_nodes == 0 {
                    budget.mark_truncated();
                    break;
                }
                fields.push(RequestBodyField {
                    name: safe_body_field_name(name),
                    shape: request_body_shape(value, Some(name), depth + 1, budget),
                });
            }
            let truncated = fields.len() < values.len();
            if truncated {
                budget.mark_truncated();
            }
            RequestBodyShape::Object { fields, truncated }
        }
    }
}

fn safe_body_field_name(value: &str) -> String {
    let safe = !value.is_empty()
        && value.len() <= MAX_PREVIEW_BODY_FIELD_NAME_BYTES
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'_' | b'-' | b'.' | b':' | b'$' | b'[' | b']')
        })
        && !contains_credential_like_token(value);
    if safe {
        value.to_owned()
    } else {
        REDACTED_BODY_FIELD_NAME.to_owned()
    }
}

fn body_field_requires_redaction(value: &str) -> bool {
    matches!(
        normalize_name(value).as_str(),
        "authorization"
            | "proxyauthorization"
            | "apikey"
            | "xapikey"
            | "xgoogapikey"
            | "accesstoken"
            | "refreshtoken"
            | "idtoken"
            | "credential"
            | "credentials"
            | "secret"
            | "password"
            | "cookie"
            | "setcookie"
            | "messages"
            | "message"
            | "content"
            | "contents"
            | "text"
            | "prompt"
            | "input"
            | "instructions"
            | "system"
            | "systeminstruction"
            | "signature"
            | "thoughtsignature"
            | "encryptedcontent"
            | "reasoningsignature"
            | "reasoningstate"
            | "opaquereasoningstate"
            | "previousresponseid"
            | "continuationtoken"
            | "sessionid"
            | "conversationid"
    )
}

fn is_sensitive_location_name(value: &str) -> bool {
    matches!(
        normalize_name(value).as_str(),
        "key"
            | "apikey"
            | "token"
            | "accesstoken"
            | "refreshtoken"
            | "idtoken"
            | "signature"
            | "sig"
            | "signed"
            | "credential"
            | "secret"
            | "password"
            | "auth"
            | "authorization"
            | "cookie"
            | "session"
            | "sessionid"
    )
}

fn normalize_name(value: &str) -> String {
    value
        .bytes()
        .filter(u8::is_ascii_alphanumeric)
        .map(|byte| byte.to_ascii_lowercase())
        .map(char::from)
        .collect()
}

pub(crate) fn planned_json_payload<T: Serialize>(
    payload: &T,
    expected_family: ApiFamily,
    plan: Option<&ProviderRequestPlan>,
) -> CoreResult<Value> {
    let mut value = serde_json::to_value(payload)
        .map_err(|_| CoreError::internal("cannot serialize provider request"))?;
    let Some(plan) = plan else {
        return Ok(value);
    };
    if plan.family != expected_family {
        return Err(CoreError::invalid(
            "provider request plan does not match the adapter API family",
        ));
    }
    plan.apply_to_body(&mut value)
        .map_err(|error| CoreError::invalid(format!("provider parameters are invalid: {error}")))?;
    apply_prompt_cache_directive(&mut value, expected_family, &plan.prompt_cache)?;
    validate_prompt_cache_payload(&value, expected_family)?;
    Ok(value)
}

fn validate_prompt_cache_payload(body: &Value, family: ApiFamily) -> CoreResult<()> {
    if family != ApiFamily::GeminiGenerateContent {
        return Ok(());
    }
    let Some(object) = body.as_object() else {
        return Err(CoreError::internal(
            "provider request body must be an object",
        ));
    };
    if !object.contains_key("cachedContent") {
        return Ok(());
    }
    if ["systemInstruction", "tools", "toolConfig"]
        .iter()
        .any(|field| object.contains_key(*field))
    {
        return Err(CoreError::invalid(
            "Gemini cachedContent cannot be combined with systemInstruction, tools, or toolConfig",
        ));
    }
    Ok(())
}

fn apply_prompt_cache_directive(
    body: &mut Value,
    family: ApiFamily,
    directive: &PromptCacheDirective,
) -> CoreResult<()> {
    match directive {
        PromptCacheDirective::None | PromptCacheDirective::ProviderManagedAutomatic => Ok(()),
        PromptCacheDirective::AnthropicAutomatic { ttl } => {
            require_family(family, ApiFamily::AnthropicMessages)?;
            insert_top_level(body, "cache_control", anthropic_cache_control(*ttl))
        }
        PromptCacheDirective::AnthropicExplicitBreakpoints { ttl } => {
            require_family(family, ApiFamily::AnthropicMessages)?;
            apply_anthropic_system_breakpoint(body, *ttl)
        }
        PromptCacheDirective::AnthropicDisabled => {
            require_family(family, ApiFamily::AnthropicMessages)
        }
        PromptCacheDirective::GeminiExplicitContext {
            resource_name,
            requested_ttl,
        } => {
            require_family(family, ApiFamily::GeminiGenerateContent)?;
            if *requested_ttl != PromptCacheTtl::ProviderDefault {
                return Err(CoreError::invalid(
                    "Gemini cached-content TTL cannot be applied by generateContent",
                ));
            }
            insert_top_level(body, "cachedContent", json!(resource_name))
        }
    }
}

fn require_family(actual: ApiFamily, expected: ApiFamily) -> CoreResult<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(CoreError::invalid(
            "prompt-cache directive does not match the adapter API family",
        ))
    }
}

fn insert_top_level(body: &mut Value, field: &str, value: Value) -> CoreResult<()> {
    let object = body
        .as_object_mut()
        .ok_or_else(|| CoreError::internal("provider request body must be an object"))?;
    if object.insert(field.to_owned(), value).is_some() {
        return Err(CoreError::invalid(
            "provider request plan conflicts with an adapter-owned field",
        ));
    }
    Ok(())
}

fn anthropic_cache_control(ttl: AnthropicCacheTtl) -> Value {
    let mut cache_control = Map::from_iter([("type".to_owned(), json!("ephemeral"))]);
    if ttl == AnthropicCacheTtl::OneHour {
        cache_control.insert("ttl".to_owned(), json!("1h"));
    }
    Value::Object(cache_control)
}

fn apply_anthropic_system_breakpoint(body: &mut Value, ttl: AnthropicCacheTtl) -> CoreResult<()> {
    let object = body
        .as_object_mut()
        .ok_or_else(|| CoreError::internal("provider request body must be an object"))?;
    let system = object
        .get_mut("system")
        .ok_or_else(|| CoreError::invalid("explicit prompt caching requires a system prompt"))?;
    let text = system
        .as_str()
        .ok_or_else(|| CoreError::invalid("Anthropic system prompt has an unexpected shape"))?
        .to_owned();
    *system = json!([{
        "type": "text",
        "text": text,
        "cache_control": anthropic_cache_control(ttl),
    }]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use lorepia_domain::{ApiFamily, HeaderName, HttpMethod};
    use serde_json::json;
    use url::Url;

    use super::{
        CAPABILITY_PROBE_TOOL_NAME, CapabilityProbePlanKind, MAX_PREVIEW_BODY_NODES,
        MAX_PREVIEW_QUERY_BYTES, MAX_PREVIEW_QUERY_NAME_BYTES, MAX_PREVIEW_QUERY_PARAMETERS,
        REDACTED_PATH_SEGMENT, RequestBodyShape, RequestPreviewErrorKind, build_request_preview,
        capability_probe_request_plan, planned_json_payload, setup_assistant_request_plan,
    };
    use crate::parameter_mapping::{AnthropicCacheTtl, PromptCacheDirective, ProviderRequestPlan};
    use crate::setup_assistant::ASSISTANT_OUTPUT_SCHEMA_NAME;

    #[test]
    fn no_plan_preserves_an_omitted_provider_default() {
        let body = json!({"model": "fixture"});
        let planned =
            planned_json_payload(&body, ApiFamily::OpenAiResponses, None).expect("payload");
        assert_eq!(planned, body);
        assert!(planned.get("temperature").is_none());
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn capability_probe_plans_use_exact_closed_family_dialects() {
        let schema = json!({
            "type": "object",
            "properties": {
                "probe": {
                    "type": "string",
                    "enum": ["ok"],
                },
            },
            "required": ["probe"],
            "additionalProperties": false,
        });
        for family in [
            ApiFamily::OpenAiResponses,
            ApiFamily::OpenAiChatCompletions,
            ApiFamily::AnthropicMessages,
            ApiFamily::GeminiGenerateContent,
            ApiFamily::OllamaNative,
        ] {
            let structured =
                capability_probe_request_plan(family, CapabilityProbePlanKind::StructuredOutput);
            let structured =
                planned_json_payload(&json!({"model": "fixture"}), family, Some(&structured))
                    .expect("structured probe payload");
            match family {
                ApiFamily::OpenAiResponses => {
                    assert_eq!(structured["text"]["format"]["type"], "json_schema");
                    assert_eq!(structured["text"]["format"]["name"], "lorepia_probe");
                    assert_eq!(structured["text"]["format"]["strict"], true);
                    assert_eq!(structured["text"]["format"]["schema"], schema);
                }
                ApiFamily::OpenAiChatCompletions => {
                    let format = &structured["response_format"];
                    assert_eq!(format["type"], "json_schema");
                    assert_eq!(format["json_schema"]["name"], "lorepia_probe");
                    assert_eq!(format["json_schema"]["strict"], true);
                    assert_eq!(format["json_schema"]["schema"], schema);
                }
                ApiFamily::AnthropicMessages => {
                    assert_eq!(
                        structured["output_config"]["format"],
                        json!({"type": "json_schema", "schema": schema.clone()})
                    );
                }
                ApiFamily::GeminiGenerateContent => {
                    assert_eq!(
                        structured["generationConfig"]["responseMimeType"],
                        "application/json"
                    );
                    assert_eq!(structured["generationConfig"]["responseJsonSchema"], schema);
                }
                ApiFamily::OllamaNative => assert_eq!(structured["format"], schema),
            }

            let tool = capability_probe_request_plan(family, CapabilityProbePlanKind::ToolCalling);
            let tool = planned_json_payload(&json!({"model": "fixture"}), family, Some(&tool))
                .expect("tool probe payload");
            match family {
                ApiFamily::OpenAiResponses => {
                    assert_eq!(tool["tools"][0]["type"], "function");
                    assert_eq!(tool["tools"][0]["name"], CAPABILITY_PROBE_TOOL_NAME);
                    assert_eq!(tool["tools"][0]["parameters"], schema);
                    assert_eq!(
                        tool["tool_choice"],
                        json!({"type": "function", "name": CAPABILITY_PROBE_TOOL_NAME})
                    );
                    assert_eq!(tool["parallel_tool_calls"], false);
                }
                ApiFamily::OpenAiChatCompletions => {
                    assert_eq!(tool["tools"][0]["type"], "function");
                    assert_eq!(
                        tool["tools"][0]["function"]["name"],
                        CAPABILITY_PROBE_TOOL_NAME
                    );
                    assert_eq!(tool["tools"][0]["function"]["parameters"], schema);
                    assert_eq!(
                        tool["tool_choice"],
                        json!({
                            "type": "function",
                            "function": {"name": CAPABILITY_PROBE_TOOL_NAME},
                        })
                    );
                    assert_eq!(tool["parallel_tool_calls"], false);
                }
                ApiFamily::AnthropicMessages => {
                    assert_eq!(tool["tools"][0]["name"], CAPABILITY_PROBE_TOOL_NAME);
                    assert_eq!(tool["tools"][0]["input_schema"], schema);
                    assert_eq!(
                        tool["tool_choice"],
                        json!({
                            "type": "tool",
                            "name": CAPABILITY_PROBE_TOOL_NAME,
                            "disable_parallel_tool_use": true,
                        })
                    );
                }
                ApiFamily::GeminiGenerateContent => {
                    let declaration = &tool["tools"][0]["functionDeclarations"][0];
                    assert_eq!(declaration["name"], CAPABILITY_PROBE_TOOL_NAME);
                    assert_eq!(declaration["parametersJsonSchema"], schema);
                    assert_eq!(
                        tool["toolConfig"]["functionCallingConfig"],
                        json!({
                            "mode": "ANY",
                            "allowedFunctionNames": [CAPABILITY_PROBE_TOOL_NAME],
                        })
                    );
                }
                ApiFamily::OllamaNative => {
                    assert_eq!(tool["tools"][0]["type"], "function");
                    assert_eq!(
                        tool["tools"][0]["function"]["name"],
                        CAPABILITY_PROBE_TOOL_NAME
                    );
                    assert_eq!(tool["tools"][0]["function"]["parameters"], schema);
                    assert!(
                        tool.get("tool_choice").is_none(),
                        "native Ollama does not define a force-tool field"
                    );
                }
            }
        }
    }

    #[test]
    fn setup_assistant_plans_use_exact_closed_family_dialects() {
        let schema = json!({
            "type": "object",
            "properties": {
                "turn": {
                    "type": "object",
                    "properties": {
                        "type": {"type": "string", "enum": ["need_more_evidence"]},
                        "questions": {"type": "array", "items": {"type": "string"}},
                    },
                    "required": ["type", "questions"],
                    "additionalProperties": false,
                },
            },
            "required": ["turn"],
            "additionalProperties": false,
        });
        for family in [
            ApiFamily::OpenAiResponses,
            ApiFamily::OpenAiChatCompletions,
            ApiFamily::AnthropicMessages,
            ApiFamily::GeminiGenerateContent,
            ApiFamily::OllamaNative,
        ] {
            let plan = setup_assistant_request_plan(family, schema.clone());
            let planned = planned_json_payload(&json!({"model": "assistant"}), family, Some(&plan))
                .expect("setup-assistant payload");
            match family {
                ApiFamily::OpenAiResponses => {
                    assert_eq!(
                        planned["text"]["format"],
                        json!({
                            "type": "json_schema",
                            "name": ASSISTANT_OUTPUT_SCHEMA_NAME,
                            "schema": schema.clone(),
                            "strict": true,
                        })
                    );
                }
                ApiFamily::OpenAiChatCompletions => {
                    assert_eq!(
                        planned["response_format"],
                        json!({
                            "type": "json_schema",
                            "json_schema": {
                                "name": ASSISTANT_OUTPUT_SCHEMA_NAME,
                                "schema": schema.clone(),
                                "strict": true,
                            },
                        })
                    );
                }
                ApiFamily::AnthropicMessages => {
                    assert_eq!(
                        planned["output_config"]["format"],
                        json!({
                            "type": "json_schema",
                            "schema": schema.clone(),
                        })
                    );
                }
                ApiFamily::GeminiGenerateContent => {
                    assert_eq!(
                        planned["generationConfig"]["responseMimeType"],
                        "application/json"
                    );
                    assert_eq!(planned["generationConfig"]["responseJsonSchema"], schema);
                }
                ApiFamily::OllamaNative => {
                    assert_eq!(planned["format"], schema);
                }
            }

            let wrong_family = match family {
                ApiFamily::OpenAiResponses => ApiFamily::OpenAiChatCompletions,
                _ => ApiFamily::OpenAiResponses,
            };
            assert!(
                planned_json_payload(&json!({"model": "assistant"}), wrong_family, Some(&plan),)
                    .is_err(),
                "a {family:?} setup plan must fail closed on {wrong_family:?}"
            );
        }
    }

    #[test]
    fn explicit_anthropic_cache_breakpoint_is_structural_and_bounded() {
        let body = json!({
            "model": "fixture",
            "system": "stable character prompt",
            "messages": [{"role": "user", "content": "hello"}],
        });
        let plan = ProviderRequestPlan {
            family: ApiFamily::AnthropicMessages,
            body_patches: Vec::new(),
            prompt_cache: PromptCacheDirective::AnthropicExplicitBreakpoints {
                ttl: AnthropicCacheTtl::OneHour,
            },
            cache_dialect: crate::parameter_mapping::PromptCacheWireDialect::Anthropic {
                supports_automatic: false,
                supports_explicit_breakpoints: true,
                supports_one_hour_ttl: true,
            },
            preserve_opaque_reasoning_state: false,
        };
        let planned = planned_json_payload(&body, ApiFamily::AnthropicMessages, Some(&plan))
            .expect("planned payload");
        assert_eq!(planned["system"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(planned["system"][0]["cache_control"]["ttl"], "1h");
    }

    #[test]
    fn plan_for_another_family_is_rejected_before_networking() {
        let plan = ProviderRequestPlan {
            family: ApiFamily::GeminiGenerateContent,
            body_patches: Vec::new(),
            prompt_cache: PromptCacheDirective::None,
            cache_dialect: crate::parameter_mapping::PromptCacheWireDialect::Gemini {
                supports_implicit: false,
                supports_explicit_context: false,
            },
            preserve_opaque_reasoning_state: false,
        };
        assert!(planned_json_payload(&json!({}), ApiFamily::OpenAiResponses, Some(&plan)).is_err());
    }

    #[test]
    fn gemini_generate_content_rejects_an_unapplied_cache_ttl() {
        let plan = ProviderRequestPlan {
            family: ApiFamily::GeminiGenerateContent,
            body_patches: Vec::new(),
            prompt_cache: PromptCacheDirective::GeminiExplicitContext {
                resource_name: "cachedContents/cache_1".to_owned(),
                requested_ttl: crate::parameter_mapping::PromptCacheTtl::CustomSeconds(60),
            },
            cache_dialect: crate::parameter_mapping::PromptCacheWireDialect::Gemini {
                supports_implicit: false,
                supports_explicit_context: true,
            },
            preserve_opaque_reasoning_state: false,
        };
        assert!(
            planned_json_payload(&json!({}), ApiFamily::GeminiGenerateContent, Some(&plan))
                .is_err()
        );
    }

    #[test]
    fn preview_retains_only_names_and_scalar_free_structure() {
        let private_message = "private-user-message-sentinel";
        let raw_credential = "sk-raw-credential-sentinel";
        let opaque_state = "opaque-reasoning-state-sentinel";
        let destination = Url::parse(&format!(
            "https://Example.TEST:443/v1/messages?api_key={raw_credential}&safe=private-query-value#private-fragment"
        ))
        .expect("URL");
        let headers = [
            HeaderName::parse("X-Trace").expect("header"),
            HeaderName::parse("Authorization").expect("header"),
            HeaderName::parse("X-Trace").expect("duplicate header"),
        ];
        let body = json!({
            "model": "private-model-value",
            "temperature": 0.73,
            "enabled": true,
            "messages": [{
                "role": "user",
                "content": private_message,
            }],
            "authorization": raw_credential,
            "opaque_reasoning_state": {
                "state": opaque_state,
            },
            "metadata": {
                "safe_field": "private-metadata-value",
            },
        });

        let preview = build_request_preview(HttpMethod::Post, &destination, &headers, Some(&body))
            .expect("safe preview");
        assert_eq!(preview.method(), HttpMethod::Post);
        assert_eq!(preview.origin().as_str(), "https://example.test");
        assert_eq!(preview.path().as_str(), "/v1/messages");
        assert_eq!(preview.query_parameter_names(), ["api_key", "safe"]);
        assert!(!preview.body_truncated());
        assert_eq!(
            preview
                .header_names()
                .iter()
                .map(HeaderName::as_str)
                .collect::<Vec<_>>(),
            ["authorization", "x-trace"]
        );

        let serialized = serde_json::to_string(&preview).expect("serialize preview");
        let debug = format!("{preview:?}");
        assert!(serialized.contains("api_key"));
        for private in [
            private_message,
            raw_credential,
            opaque_state,
            "private-query-value",
            "private-fragment",
            "private-model-value",
            "private-metadata-value",
            "0.73",
        ] {
            assert!(!serialized.contains(private), "{private}");
            assert!(!debug.contains(private), "{private}");
        }
        assert!(serialized.contains("\"model\""));
        assert!(serialized.contains("\"number\""));
        assert!(serialized.contains("\"redacted\""));
    }

    #[test]
    fn preview_query_names_are_decoded_bounded_sorted_and_value_free() {
        let query_secret = "query-value-secret-canary";
        let destination = Url::parse(&format!(
            "https://api.example.test/v1/generate?z={query_secret}&api%2Dversion=2026&a=first&z=second"
        ))
        .expect("URL");
        let preview = build_request_preview(HttpMethod::Post, &destination, &[], None)
            .expect("bounded query names");

        assert_eq!(preview.query_parameter_names(), ["a", "api-version", "z"]);
        assert!(!preview.body_truncated());
        let serialized = serde_json::to_string(&preview).expect("serialize");
        let debug = format!("{preview:?}");
        for value in [query_secret, "2026", "first", "second"] {
            assert!(!serialized.contains(value), "{value}");
            assert!(!debug.contains(value), "{value}");
        }

        let too_many = (0..=MAX_PREVIEW_QUERY_PARAMETERS)
            .map(|index| format!("p{index}=v"))
            .collect::<Vec<_>>()
            .join("&");
        let destination =
            Url::parse(&format!("https://api.example.test/v1?{too_many}")).expect("URL");
        let error = build_request_preview(HttpMethod::Get, &destination, &[], None)
            .expect_err("too many query parameters must fail");
        assert_eq!(
            error.kind(),
            RequestPreviewErrorKind::TooManyQueryParameters
        );

        let long_name = "n".repeat(MAX_PREVIEW_QUERY_NAME_BYTES + 1);
        let destination =
            Url::parse(&format!("https://api.example.test/v1?{long_name}=v")).expect("URL");
        let error = build_request_preview(HttpMethod::Get, &destination, &[], None)
            .expect_err("long query name must fail");
        assert_eq!(error.kind(), RequestPreviewErrorKind::InvalidQueryName);

        let long_value = "v".repeat(MAX_PREVIEW_QUERY_BYTES);
        let destination =
            Url::parse(&format!("https://api.example.test/v1?safe={long_value}")).expect("URL");
        let error = build_request_preview(HttpMethod::Get, &destination, &[], None)
            .expect_err("oversized query must fail");
        assert_eq!(error.kind(), RequestPreviewErrorKind::QueryTooLarge);
        assert!(!format!("{error:?} {error}").contains(&long_value));
    }

    #[test]
    fn preview_redacts_signed_and_opaque_path_segments() {
        let signed_token = concat!(
            "eyJhbGciOiJIUzI1NiJ9.",
            "eyJzdWIiOiIxMjM0NTY3ODkwIn0.",
            "SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c"
        );
        let destination = Url::parse(&format!(
            "https://api.example.test/v1/signature/{signed_token}/models"
        ))
        .expect("URL");

        let preview = build_request_preview(HttpMethod::Get, &destination, &[], None)
            .expect("redacted preview");
        assert_eq!(
            preview.path().as_str(),
            format!("/v1/signature/{REDACTED_PATH_SEGMENT}/models")
        );
        let serialized = serde_json::to_string(&preview).expect("serialize");
        assert!(!serialized.contains(signed_token));

        for inline_secret in [
            "token=short-secret",
            "api_key%3Dshort-secret",
            "token%253Dshort-secret",
            "sk%252Dshort-private-value",
        ] {
            let destination = Url::parse(&format!(
                "https://api.example.test/v1/{inline_secret}/models"
            ))
            .expect("URL");
            let preview = build_request_preview(HttpMethod::Get, &destination, &[], None)
                .expect("redacted preview");
            assert_eq!(
                preview.path().as_str(),
                format!("/v1/{REDACTED_PATH_SEGMENT}/models")
            );
            let serialized = serde_json::to_string(&preview).expect("serialize");
            assert!(!serialized.contains(inline_secret));
            assert!(!serialized.contains("short-secret"));
            assert!(!serialized.contains("short-private-value"));
        }
    }

    #[test]
    fn preview_collapses_private_and_opaque_subtrees() {
        let body = json!({
            "messages": [{
                "private-key-used-as-json-key": {
                    "more": "private",
                },
            }],
            "signature": {
                "private-key-used-as-json-key": "opaque",
            },
            "response_format": {
                "type": "json_schema",
            },
        });
        let destination = Url::parse("https://api.example.test/v1/generate").expect("URL");
        let preview = build_request_preview(HttpMethod::Post, &destination, &[], Some(&body))
            .expect("preview");
        let RequestBodyShape::Object { fields, .. } = preview.body().expect("body shape") else {
            panic!("expected object shape");
        };

        for name in ["messages", "signature"] {
            let field = fields
                .iter()
                .find(|field| field.name() == name)
                .expect("redacted field");
            assert_eq!(field.shape(), &RequestBodyShape::Redacted);
        }
        let serialized = serde_json::to_string(&preview).expect("serialize");
        assert!(!serialized.contains("private-key-used-as-json-key"));
        assert!(!serialized.contains("\"more\""));
    }

    #[test]
    fn preview_body_shape_is_bounded_by_depth_nodes_and_array_width() {
        let mut deep = json!("private-deep-value");
        for index in 0..(MAX_PREVIEW_BODY_NODES + 8) {
            let mut object = serde_json::Map::new();
            object.insert(
                format!("field_{index}"),
                json!([deep, index, "private-array-value"]),
            );
            deep = serde_json::Value::Object(object);
        }
        let destination = Url::parse("https://api.example.test/v1/generate").expect("URL");
        let preview = build_request_preview(HttpMethod::Post, &destination, &[], Some(&deep))
            .expect("bounded preview");

        assert!(preview.body_truncated());
        let serialized = serde_json::to_string(&preview).expect("serialize");
        assert!(serialized.len() < 64 * 1024);
        assert!(
            serialized.contains("\"truncated\":true")
                || serialized.contains("\"kind\":\"truncated\"")
        );
        assert!(!serialized.contains("private-deep-value"));
        assert!(!serialized.contains("private-array-value"));
    }

    #[test]
    fn preview_errors_never_retain_embedded_credentials_or_unsafe_names() {
        let raw_credential = "raw-userinfo-credential-sentinel";
        let destination =
            Url::parse(&format!("https://user:{raw_credential}@example.test/v1")).expect("URL");
        let error = build_request_preview(HttpMethod::Post, &destination, &[], None)
            .expect_err("userinfo must be rejected");
        assert_eq!(error.kind(), RequestPreviewErrorKind::EmbeddedCredential);
        let diagnostic = format!("{error:?} {error}");
        assert!(!diagnostic.contains(raw_credential));

        let credential_host = "sk-super-secret-host-name";
        let destination =
            Url::parse(&format!("https://{credential_host}.example.test/v1")).expect("URL");
        let error = build_request_preview(HttpMethod::Get, &destination, &[], None)
            .expect_err("credential-like host label must be rejected");
        assert_eq!(error.kind(), RequestPreviewErrorKind::InvalidDestination);
        let diagnostic = format!("{error:?} {error}");
        assert!(!diagnostic.contains(credential_host));
    }
}
