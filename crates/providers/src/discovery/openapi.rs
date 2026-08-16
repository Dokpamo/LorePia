use std::collections::BTreeSet;
use std::fmt;

use lorepia_domain::EndpointPath;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::evidence::redact_and_bound;
use crate::url_policy::{CanonicalUrl, UrlPolicyError, UrlPolicyMode, is_sensitive_query_key};

const MAX_HARD_SPEC_BYTES: usize = 4 * 1024 * 1024;
const MAX_HARD_DEPTH: usize = 64;
const MAX_HARD_NODES: usize = 100_000;
const MAX_HARD_PATHS: usize = 2_048;
const MAX_HARD_OPERATIONS: usize = 4_096;
const MAX_HARD_SERVERS: usize = 64;
const MAX_HARD_PARAMETERS: usize = 2_048;
const MAX_HARD_STRING_BYTES: usize = 16 * 1024;
const MAX_SECURITY_SCHEMES: usize = 128;
const MAX_SECURITY_REFERENCES_PER_OPERATION: usize = 32;
const MAX_TOTAL_OPERATION_SECURITY_REFERENCES: usize = 2_048;
const MAX_LOCAL_REFERENCE_DEPTH: usize = 8;
const MAX_SCHEMA_TRAVERSAL_STEPS: usize = 4_096;

/// Serialization used by a possible `OpenAPI` document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenApiDocumentFormat {
    Json,
    Yaml,
}

/// Finite structural limits for parsing an untrusted API specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenApiExtractionLimits {
    pub max_bytes: usize,
    pub max_depth: usize,
    pub max_nodes: usize,
    pub max_paths: usize,
    pub max_operations: usize,
    pub max_servers: usize,
    pub max_parameters: usize,
    pub max_string_bytes: usize,
}

impl Default for OpenApiExtractionLimits {
    fn default() -> Self {
        Self {
            max_bytes: 1024 * 1024,
            max_depth: 32,
            max_nodes: 20_000,
            max_paths: 512,
            max_operations: 1024,
            max_servers: 16,
            max_parameters: 512,
            max_string_bytes: 4 * 1024,
        }
    }
}

impl OpenApiExtractionLimits {
    pub(super) fn validate(self) -> Result<Self, OpenApiExtractionError> {
        let invalid = self.max_bytes == 0
            || self.max_bytes > MAX_HARD_SPEC_BYTES
            || self.max_depth == 0
            || self.max_depth > MAX_HARD_DEPTH
            || self.max_nodes == 0
            || self.max_nodes > MAX_HARD_NODES
            || self.max_paths == 0
            || self.max_paths > MAX_HARD_PATHS
            || self.max_operations == 0
            || self.max_operations > MAX_HARD_OPERATIONS
            || self.max_servers == 0
            || self.max_servers > MAX_HARD_SERVERS
            || self.max_parameters == 0
            || self.max_parameters > MAX_HARD_PARAMETERS
            || self.max_string_bytes == 0
            || self.max_string_bytes > MAX_HARD_STRING_BYTES;
        if invalid {
            Err(OpenApiExtractionError::InvalidLimits)
        } else {
            Ok(self)
        }
    }
}

/// Closed family hints based only on machine-readable path and request shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiFamilyHint {
    OpenAiResponses,
    OpenAiChatCompletions,
    AnthropicMessages,
    GeminiGenerateContent,
    OllamaNative,
}

/// Sanitized security-scheme metadata. Secret examples and values are excluded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OpenApiAuthSchemeEvidence {
    name: String,
    kind: Option<String>,
    location: Option<String>,
    parameter_name: Option<String>,
    scheme: Option<String>,
}

impl OpenApiAuthSchemeEvidence {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn kind(&self) -> Option<&str> {
        self.kind.as_deref()
    }

    pub fn location(&self) -> Option<&str> {
        self.location.as_deref()
    }

    pub fn parameter_name(&self) -> Option<&str> {
        self.parameter_name.as_deref()
    }

    pub fn scheme(&self) -> Option<&str> {
        self.scheme.as_deref()
    }
}

/// Sanitized operation shape extracted from an API specification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OpenApiOperationEvidence {
    method: String,
    path: String,
    operation_id: Option<String>,
    summary: Option<String>,
    parameter_names: Vec<String>,
    request_field_names: Vec<String>,
    content_types: Vec<String>,
    security_schemes: Vec<String>,
}

impl OpenApiOperationEvidence {
    pub fn method(&self) -> &str {
        &self.method
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn operation_id(&self) -> Option<&str> {
        self.operation_id.as_deref()
    }

    pub fn summary(&self) -> Option<&str> {
        self.summary.as_deref()
    }

    pub fn parameter_names(&self) -> &[String] {
        &self.parameter_names
    }

    pub fn request_field_names(&self) -> &[String] {
        &self.request_field_names
    }

    pub fn content_types(&self) -> &[String] {
        &self.content_types
    }

    pub fn security_schemes(&self) -> &[String] {
        &self.security_schemes
    }
}

/// A policy-validated API origin and normalized base path recovered from a
/// server declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OpenApiServerCandidate {
    origin: String,
    base_path: EndpointPath,
}

impl OpenApiServerCandidate {
    pub fn origin(&self) -> &str {
        &self.origin
    }

    pub fn base_path(&self) -> &EndpointPath {
        &self.base_path
    }
}

/// A bounded OpenAPI/Swagger summary suitable for durable discovery evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OpenApiEvidence {
    specification_version: String,
    server_candidates: Vec<OpenApiServerCandidate>,
    operations: Vec<OpenApiOperationEvidence>,
    auth_schemes: Vec<OpenApiAuthSchemeEvidence>,
    model_list_paths: Vec<String>,
    generation_paths: Vec<String>,
    streaming_media_types: Vec<String>,
    api_family_hints: Vec<ApiFamilyHint>,
}

impl OpenApiEvidence {
    pub fn specification_version(&self) -> &str {
        &self.specification_version
    }

    pub fn server_candidates(&self) -> &[OpenApiServerCandidate] {
        &self.server_candidates
    }

    pub fn operations(&self) -> &[OpenApiOperationEvidence] {
        &self.operations
    }

    pub fn auth_schemes(&self) -> &[OpenApiAuthSchemeEvidence] {
        &self.auth_schemes
    }

    pub fn model_list_paths(&self) -> &[String] {
        &self.model_list_paths
    }

    pub fn generation_paths(&self) -> &[String] {
        &self.generation_paths
    }

    pub fn streaming_media_types(&self) -> &[String] {
        &self.streaming_media_types
    }

    pub fn api_family_hints(&self) -> &[ApiFamilyHint] {
        &self.api_family_hints
    }
}

/// Deterministic `OpenAPI` extraction failure. It never carries source text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenApiExtractionError {
    InvalidLimits,
    DocumentTooLarge,
    InvalidUtf8,
    InvalidJson,
    StructureTooDeep,
    TooManyNodes,
    TooManyPaths,
    TooManyOperations,
    TooManyServers,
    TooManyParameters,
    TooManySecurityReferences,
    TooManySchemaTraversalSteps,
    StringTooLong,
    InvalidYaml,
    UnsafeServerUrl(UrlPolicyError),
    UnsafeServerBasePath,
    PotentialCredentialInServerPath,
}

impl fmt::Display for OpenApiExtractionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLimits => formatter.write_str("OpenAPI extraction limits are invalid"),
            Self::DocumentTooLarge => formatter.write_str("API specification is too large"),
            Self::InvalidUtf8 => formatter.write_str("API specification is not valid UTF-8"),
            Self::InvalidJson => formatter.write_str("API specification contains invalid JSON"),
            Self::StructureTooDeep => formatter.write_str("API specification nesting is too deep"),
            Self::TooManyNodes => formatter.write_str("API specification has too many nodes"),
            Self::TooManyPaths => formatter.write_str("API specification has too many paths"),
            Self::TooManyOperations => {
                formatter.write_str("API specification has too many operations")
            }
            Self::TooManyServers => formatter.write_str("API specification has too many servers"),
            Self::TooManyParameters => {
                formatter.write_str("API specification has too many parameters")
            }
            Self::TooManySecurityReferences => {
                formatter.write_str("API specification has too many security references")
            }
            Self::TooManySchemaTraversalSteps => {
                formatter.write_str("API specification schema traversal exceeded its work budget")
            }
            Self::StringTooLong => formatter.write_str("API specification contains a long string"),
            Self::InvalidYaml => {
                formatter.write_str("API specification contains unsupported or invalid YAML")
            }
            Self::UnsafeServerUrl(error) => write!(formatter, "unsafe server URL: {error}"),
            Self::UnsafeServerBasePath => {
                formatter.write_str("API specification contains an unsafe server base path")
            }
            Self::PotentialCredentialInServerPath => formatter.write_str(
                "API specification server path looks credential-bearing and was rejected",
            ),
        }
    }
}

impl std::error::Error for OpenApiExtractionError {}

/// Extracts a bounded, typed summary when `document` is `OpenAPI` or Swagger.
///
/// `source_url` supplies the explicit public/local policy for relative server
/// URLs. Extracted server candidates are evidence only; this function does not
/// approve them for credentials or issue any request.
pub fn extract_openapi(
    source_url: &CanonicalUrl,
    format: OpenApiDocumentFormat,
    document: &[u8],
    limits: OpenApiExtractionLimits,
) -> Result<Option<OpenApiEvidence>, OpenApiExtractionError> {
    let limits = limits.validate()?;
    if document.len() > limits.max_bytes {
        return Err(OpenApiExtractionError::DocumentTooLarge);
    }
    match format {
        OpenApiDocumentFormat::Json => extract_json_openapi(source_url, document, limits),
        OpenApiDocumentFormat::Yaml => extract_yaml_openapi(source_url, document, limits),
    }
}

fn extract_json_openapi(
    source_url: &CanonicalUrl,
    document: &[u8],
    limits: OpenApiExtractionLimits,
) -> Result<Option<OpenApiEvidence>, OpenApiExtractionError> {
    preflight_json_bounds(document, limits)?;
    let root: Value =
        serde_json::from_slice(document).map_err(|_| OpenApiExtractionError::InvalidJson)?;
    validate_json_bounds(&root, limits)?;
    let Some(object) = root.as_object() else {
        return Ok(None);
    };
    let Some(version) = specification_version(object) else {
        return Ok(None);
    };

    let mut server_candidates = extract_json_servers(source_url, object, &version, limits)?;
    sort_server_candidates(&mut server_candidates);
    let auth_schemes = extract_json_auth_schemes(object, limits);
    let root_security = security_names(
        object.get("security"),
        MAX_SECURITY_REFERENCES_PER_OPERATION,
    );
    let operations = extract_json_operations(&root, object, &root_security, limits)?;
    Ok(Some(finalize_evidence(
        version,
        server_candidates,
        operations,
        auth_schemes,
    )))
}

fn preflight_json_bounds(
    document: &[u8],
    limits: OpenApiExtractionLimits,
) -> Result<(), OpenApiExtractionError> {
    let mut index = 0;
    let mut depth = 0_usize;
    let mut nodes = 0_usize;
    while index < document.len() {
        match document[index] {
            b'{' | b'[' => {
                nodes = nodes
                    .checked_add(1)
                    .ok_or(OpenApiExtractionError::TooManyNodes)?;
                depth = depth
                    .checked_add(1)
                    .ok_or(OpenApiExtractionError::StructureTooDeep)?;
                if nodes > limits.max_nodes {
                    return Err(OpenApiExtractionError::TooManyNodes);
                }
                if depth > limits.max_depth {
                    return Err(OpenApiExtractionError::StructureTooDeep);
                }
                index += 1;
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                index += 1;
            }
            b'"' => {
                nodes = nodes
                    .checked_add(1)
                    .ok_or(OpenApiExtractionError::TooManyNodes)?;
                if nodes > limits.max_nodes {
                    return Err(OpenApiExtractionError::TooManyNodes);
                }
                let start = index + 1;
                index += 1;
                let mut escaped = false;
                while index < document.len() {
                    if index.saturating_sub(start) > limits.max_string_bytes {
                        return Err(OpenApiExtractionError::StringTooLong);
                    }
                    let byte = document[index];
                    index += 1;
                    if escaped {
                        escaped = false;
                    } else if byte == b'\\' {
                        escaped = true;
                    } else if byte == b'"' {
                        break;
                    }
                }
            }
            b'-' | b'0'..=b'9' | b't' | b'f' | b'n' => {
                nodes = nodes
                    .checked_add(1)
                    .ok_or(OpenApiExtractionError::TooManyNodes)?;
                if nodes > limits.max_nodes {
                    return Err(OpenApiExtractionError::TooManyNodes);
                }
                let start = index;
                index += 1;
                while index < document.len()
                    && !document[index].is_ascii_whitespace()
                    && !matches!(document[index], b',' | b']' | b'}' | b':')
                {
                    index += 1;
                }
                if index.saturating_sub(start) > limits.max_string_bytes {
                    return Err(OpenApiExtractionError::StringTooLong);
                }
            }
            _ => index += 1,
        }
    }
    Ok(())
}

fn specification_version(object: &serde_json::Map<String, Value>) -> Option<String> {
    object
        .get("openapi")
        .and_then(Value::as_str)
        .filter(|version| version.starts_with("3."))
        .or_else(|| {
            object
                .get("swagger")
                .and_then(Value::as_str)
                .filter(|version| *version == "2.0")
        })
        .map(ToOwned::to_owned)
}

fn validate_json_bounds(
    root: &Value,
    limits: OpenApiExtractionLimits,
) -> Result<(), OpenApiExtractionError> {
    let mut stack = vec![(root, 1_usize)];
    let mut nodes = 0_usize;
    while let Some((value, depth)) = stack.pop() {
        nodes = nodes
            .checked_add(1)
            .ok_or(OpenApiExtractionError::TooManyNodes)?;
        if nodes > limits.max_nodes {
            return Err(OpenApiExtractionError::TooManyNodes);
        }
        if depth > limits.max_depth {
            return Err(OpenApiExtractionError::StructureTooDeep);
        }
        match value {
            Value::String(value) => {
                if value.len() > limits.max_string_bytes {
                    return Err(OpenApiExtractionError::StringTooLong);
                }
            }
            Value::Array(values) => {
                stack.extend(values.iter().map(|value| (value, depth + 1)));
            }
            Value::Object(object) => {
                for (key, value) in object {
                    if key.len() > limits.max_string_bytes {
                        return Err(OpenApiExtractionError::StringTooLong);
                    }
                    stack.push((value, depth + 1));
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
        }
    }
    Ok(())
}

fn extract_json_servers(
    source_url: &CanonicalUrl,
    object: &serde_json::Map<String, Value>,
    version: &str,
    limits: OpenApiExtractionLimits,
) -> Result<Vec<OpenApiServerCandidate>, OpenApiExtractionError> {
    let mut servers = Vec::new();
    if version.starts_with("3.") {
        if let Some(values) = object.get("servers").and_then(Value::as_array) {
            for raw in values
                .iter()
                .filter_map(|entry| entry.get("url").and_then(Value::as_str))
            {
                push_server(source_url, raw, &mut servers, limits)?;
            }
        }
    } else if let Some(host) = object.get("host").and_then(Value::as_str) {
        let base_path = object
            .get("basePath")
            .and_then(Value::as_str)
            .unwrap_or("/");
        let schemes = object
            .get("schemes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        let scheme = if schemes.is_empty() {
            Some(source_url.url().scheme())
        } else {
            select_swagger_scheme(source_url.mode(), &schemes)
        };
        if let Some(scheme) = scheme {
            let raw = format!("{scheme}://{host}{base_path}");
            push_server(source_url, &raw, &mut servers, limits)?;
        }
    }
    Ok(servers)
}

fn select_swagger_scheme<'a>(mode: UrlPolicyMode, schemes: &'a [&'a str]) -> Option<&'a str> {
    match mode {
        UrlPolicyMode::Public => schemes.iter().copied().find(|scheme| *scheme == "https"),
        UrlPolicyMode::LocalLoopback => schemes
            .iter()
            .copied()
            .find(|scheme| matches!(*scheme, "http" | "https")),
    }
}

fn push_server(
    source_url: &CanonicalUrl,
    raw: &str,
    servers: &mut Vec<OpenApiServerCandidate>,
    limits: OpenApiExtractionLimits,
) -> Result<(), OpenApiExtractionError> {
    if raw.contains(['{', '}']) {
        return Ok(());
    }
    if servers.len() >= limits.max_servers {
        return Err(OpenApiExtractionError::TooManyServers);
    }
    let canonical = source_url
        .join(raw)
        .map_err(OpenApiExtractionError::UnsafeServerUrl)?;
    if canonical.url().query().is_some() {
        return Err(OpenApiExtractionError::UnsafeServerBasePath);
    }
    let path = canonical.url().path();
    if path_looks_credential_bearing(path) {
        return Err(OpenApiExtractionError::PotentialCredentialInServerPath);
    }
    if !is_safe_server_base_path(path) {
        return Err(OpenApiExtractionError::UnsafeServerBasePath);
    }
    let base_path =
        EndpointPath::parse(path).map_err(|_| OpenApiExtractionError::UnsafeServerBasePath)?;
    servers.push(OpenApiServerCandidate {
        origin: canonical.origin().as_string(),
        base_path,
    });
    Ok(())
}

fn path_looks_credential_bearing(path: &str) -> bool {
    if path.contains('%') {
        return true;
    }

    let segments = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    for (index, segment) in segments.iter().enumerate() {
        let lower = segment.to_ascii_lowercase();
        if [
            "sk-",
            "sk_",
            "pk-",
            "pk_",
            "ghp_",
            "github_pat_",
            "xoxb-",
            "xoxp-",
            "xoxa-",
            "ya29.",
            "hf_",
            "aiza",
            "eyj",
        ]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
        {
            return true;
        }

        if let Some((key, value)) = segment.split_once(['=', ':'])
            && is_sensitive_query_key(key)
            && !value.is_empty()
        {
            return true;
        }
        if is_sensitive_query_key(segment)
            && segments
                .get(index + 1)
                .is_some_and(|value| !value.is_empty())
        {
            return true;
        }

        let token_characters = segment
            .bytes()
            .filter(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_'))
            .count();
        let has_letter = segment.bytes().any(|byte| byte.is_ascii_alphabetic());
        let has_digit = segment.bytes().any(|byte| byte.is_ascii_digit());
        let has_lower = segment.bytes().any(|byte| byte.is_ascii_lowercase());
        let has_upper = segment.bytes().any(|byte| byte.is_ascii_uppercase());
        if (segment.len() >= 12 && has_lower && has_upper && has_digit)
            || (segment.len() >= 16
                && token_characters == segment.len()
                && has_letter
                && has_digit
                && !is_version_segment(segment))
        {
            return true;
        }
    }
    false
}

fn is_safe_server_base_path(path: &str) -> bool {
    if path == "/" {
        return true;
    }
    let segments = path
        .strip_prefix('/')
        .into_iter()
        .flat_map(|path| path.split('/'))
        .collect::<Vec<_>>();
    !segments.is_empty()
        && segments.len() <= 8
        && segments.iter().all(|segment| {
            is_plain_base_segment(segment)
                || is_version_segment(segment)
                || is_date_version_segment(segment)
        })
}

fn is_plain_base_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment.len() <= 32
        && segment
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        && segment
            .bytes()
            .last()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        && segment
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || matches!(byte, b'-' | b'_'))
}

fn is_version_segment(segment: &str) -> bool {
    let Some(remainder) = segment.strip_prefix('v') else {
        return false;
    };
    let digit_count = remainder.bytes().take_while(u8::is_ascii_digit).count();
    if digit_count == 0 || digit_count > 4 || segment.len() > 16 {
        return false;
    }
    let suffix = &remainder[digit_count..];
    suffix.is_empty()
        || ["alpha", "beta", "preview", "rc"].iter().any(|marker| {
            suffix
                .strip_prefix(marker)
                .is_some_and(|tail| tail.bytes().all(|byte| byte.is_ascii_digit()))
        })
}

fn is_date_version_segment(segment: &str) -> bool {
    let mut pieces = segment.split('-');
    matches!(
        (pieces.next(), pieces.next(), pieces.next(), pieces.next()),
        (Some(year), Some(month), Some(day), None)
            if year.len() == 4
                && month.len() == 2
                && day.len() == 2
                && year.bytes().all(|byte| byte.is_ascii_digit())
                && month.bytes().all(|byte| byte.is_ascii_digit())
                && day.bytes().all(|byte| byte.is_ascii_digit())
    )
}

fn sort_server_candidates(candidates: &mut Vec<OpenApiServerCandidate>) {
    candidates.sort_by(|left, right| {
        (&left.origin, left.base_path.as_str()).cmp(&(&right.origin, right.base_path.as_str()))
    });
    candidates
        .dedup_by(|left, right| left.origin == right.origin && left.base_path == right.base_path);
}

fn extract_json_auth_schemes(
    object: &serde_json::Map<String, Value>,
    limits: OpenApiExtractionLimits,
) -> Vec<OpenApiAuthSchemeEvidence> {
    let schemes = object
        .get("components")
        .and_then(|components| components.get("securitySchemes"))
        .or_else(|| object.get("securityDefinitions"))
        .and_then(Value::as_object);
    let mut names = schemes
        .into_iter()
        .flat_map(|schemes| schemes.iter())
        .collect::<Vec<_>>();
    names.sort_by_key(|(left, _)| *left);
    names
        .into_iter()
        .take(MAX_SECURITY_SCHEMES.min(limits.max_parameters))
        .map(|(name, scheme)| OpenApiAuthSchemeEvidence {
            name: bounded_redacted(name, limits.max_string_bytes),
            kind: sanitized_field(scheme, "type", limits.max_string_bytes),
            location: sanitized_field(scheme, "in", limits.max_string_bytes),
            parameter_name: sanitized_field(scheme, "name", limits.max_string_bytes),
            scheme: sanitized_field(scheme, "scheme", limits.max_string_bytes),
        })
        .collect()
}

fn sanitized_field(value: &Value, field: &str, max_bytes: usize) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(|value| bounded_redacted(value, max_bytes))
}

#[allow(clippy::too_many_lines)]
fn extract_json_operations(
    root: &Value,
    object: &serde_json::Map<String, Value>,
    root_security: &[String],
    limits: OpenApiExtractionLimits,
) -> Result<Vec<OpenApiOperationEvidence>, OpenApiExtractionError> {
    let Some(paths) = object.get("paths").and_then(Value::as_object) else {
        return Ok(Vec::new());
    };
    if paths.len() > limits.max_paths {
        return Err(OpenApiExtractionError::TooManyPaths);
    }

    let mut ordered_paths = paths.iter().collect::<Vec<_>>();
    ordered_paths.sort_by_key(|(left, _)| *left);
    let mut operations = Vec::new();
    let mut parameter_total = 0_usize;
    let mut security_reference_total = 0_usize;
    let mut schema_traversal_steps = 0_usize;
    let schema_traversal_limit = limits
        .max_parameters
        .saturating_mul(8)
        .min(MAX_SCHEMA_TRAVERSAL_STEPS);
    for (path, path_item) in ordered_paths {
        if !valid_api_path(path) {
            continue;
        }
        let Some(path_item) = resolve_local_component_chain(
            root,
            path_item,
            &["#/components/pathItems/"],
            &mut schema_traversal_steps,
            schema_traversal_limit,
        )?
        else {
            continue;
        };
        let Some(path_object) = path_item.as_object() else {
            continue;
        };
        for method in ["delete", "get", "head", "options", "patch", "post", "put"] {
            let Some(operation) = path_object.get(method).and_then(Value::as_object) else {
                continue;
            };
            if operations.len() >= limits.max_operations {
                return Err(OpenApiExtractionError::TooManyOperations);
            }

            let mut parameter_names = extract_parameter_names(
                root,
                path_object.get("parameters"),
                limits.max_parameters,
                &mut schema_traversal_steps,
                schema_traversal_limit,
            )?;
            parameter_names.extend(extract_parameter_names(
                root,
                operation.get("parameters"),
                limits.max_parameters,
                &mut schema_traversal_steps,
                schema_traversal_limit,
            )?);
            sort_deduplicate(&mut parameter_names);

            let mut request_field_names = request_field_names(
                root,
                operation,
                limits.max_parameters,
                &mut schema_traversal_steps,
                schema_traversal_limit,
            )?;
            sort_deduplicate(&mut request_field_names);
            parameter_total = parameter_total
                .checked_add(parameter_names.len())
                .and_then(|total| total.checked_add(request_field_names.len()))
                .ok_or(OpenApiExtractionError::TooManyParameters)?;
            if parameter_total > limits.max_parameters {
                return Err(OpenApiExtractionError::TooManyParameters);
            }

            let mut content_types = operation_content_types(
                root,
                operation,
                &mut schema_traversal_steps,
                schema_traversal_limit,
            )?;
            sort_deduplicate(&mut content_types);
            let mut security_schemes = if operation.contains_key("security") {
                security_names(
                    operation.get("security"),
                    MAX_SECURITY_REFERENCES_PER_OPERATION,
                )
            } else {
                root_security.to_vec()
            };
            sort_deduplicate(&mut security_schemes);
            security_reference_total = security_reference_total
                .checked_add(security_schemes.len())
                .ok_or(OpenApiExtractionError::TooManySecurityReferences)?;
            let security_reference_limit = limits
                .max_parameters
                .saturating_mul(4)
                .min(MAX_TOTAL_OPERATION_SECURITY_REFERENCES);
            if security_reference_total > security_reference_limit {
                return Err(OpenApiExtractionError::TooManySecurityReferences);
            }

            operations.push(OpenApiOperationEvidence {
                method: method.to_ascii_uppercase(),
                path: bounded_redacted(path, 2_048),
                operation_id: operation
                    .get("operationId")
                    .and_then(Value::as_str)
                    .map(|value| bounded_redacted(value, limits.max_string_bytes)),
                summary: operation
                    .get("summary")
                    .and_then(Value::as_str)
                    .map(|value| bounded_redacted(value, limits.max_string_bytes)),
                parameter_names,
                request_field_names,
                content_types,
                security_schemes,
            });
        }
    }
    Ok(operations)
}

fn extract_parameter_names(
    root: &Value,
    value: Option<&Value>,
    limit: usize,
    traversal_steps: &mut usize,
    traversal_limit: usize,
) -> Result<Vec<String>, OpenApiExtractionError> {
    let mut names = Vec::new();
    for parameter in value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(limit)
    {
        let Some(parameter) = resolve_local_component_chain(
            root,
            parameter,
            &["#/components/parameters/", "#/parameters/"],
            traversal_steps,
            traversal_limit,
        )?
        else {
            continue;
        };
        if let Some(name) = parameter.get("name").and_then(Value::as_str) {
            names.push(bounded_redacted(name, 512));
        }
    }
    Ok(names)
}

fn request_field_names(
    root: &Value,
    operation: &serde_json::Map<String, Value>,
    limit: usize,
    traversal_steps: &mut usize,
    traversal_limit: usize,
) -> Result<Vec<String>, OpenApiExtractionError> {
    let mut names = Vec::new();
    let mut seen_references = BTreeSet::new();
    if let Some(request_body) = operation.get("requestBody")
        && let Some(request_body) = resolve_local_component_chain(
            root,
            request_body,
            &["#/components/requestBodies/"],
            traversal_steps,
            traversal_limit,
        )?
        && let Some(content) = request_body.get("content").and_then(Value::as_object)
    {
        for media in content.values() {
            if let Some(schema) = media.get("schema") {
                collect_schema_property_names(
                    root,
                    schema,
                    0,
                    limit,
                    &mut names,
                    &mut seen_references,
                    traversal_steps,
                    traversal_limit,
                )?;
            }
        }
    }
    if let Some(parameters) = operation.get("parameters").and_then(Value::as_array) {
        for parameter in parameters {
            let Some(parameter) = resolve_local_component_chain(
                root,
                parameter,
                &["#/components/parameters/", "#/parameters/"],
                traversal_steps,
                traversal_limit,
            )?
            else {
                continue;
            };
            if let Some(schema) = parameter.get("schema") {
                collect_schema_property_names(
                    root,
                    schema,
                    0,
                    limit,
                    &mut names,
                    &mut seen_references,
                    traversal_steps,
                    traversal_limit,
                )?;
            }
        }
    }
    Ok(names)
}

#[allow(clippy::too_many_arguments)]
fn collect_schema_property_names(
    root: &Value,
    schema: &Value,
    reference_depth: usize,
    limit: usize,
    names: &mut Vec<String>,
    seen_references: &mut BTreeSet<String>,
    traversal_steps: &mut usize,
    traversal_limit: usize,
) -> Result<(), OpenApiExtractionError> {
    consume_schema_traversal_step(traversal_steps, traversal_limit)?;
    if names.len() >= limit || reference_depth > MAX_LOCAL_REFERENCE_DEPTH {
        return Ok(());
    }
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        if is_allowed_local_reference(reference, &["#/components/schemas/", "#/definitions/"])
            && seen_references.insert(reference.to_owned())
            && let Some(resolved) = resolve_local_reference(root, reference)
        {
            collect_schema_property_names(
                root,
                resolved,
                reference_depth + 1,
                limit,
                names,
                seen_references,
                traversal_steps,
                traversal_limit,
            )?;
        }
        return Ok(());
    }
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        let mut keys = properties.keys().collect::<Vec<_>>();
        keys.sort();
        for key in keys {
            if names.len() >= limit {
                return Ok(());
            }
            names.push(bounded_redacted(key, 512));
        }
    }
    for combinator in ["allOf", "anyOf", "oneOf"] {
        if let Some(parts) = schema.get(combinator).and_then(Value::as_array) {
            for part in parts {
                collect_schema_property_names(
                    root,
                    part,
                    reference_depth + 1,
                    limit,
                    names,
                    seen_references,
                    traversal_steps,
                    traversal_limit,
                )?;
            }
        }
    }
    Ok(())
}

fn resolve_local_reference<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    let pointer = reference.strip_prefix('#')?;
    root.pointer(pointer)
}

fn resolve_local_component_chain<'a>(
    root: &'a Value,
    value: &'a Value,
    allowed_prefixes: &[&str],
    traversal_steps: &mut usize,
    traversal_limit: usize,
) -> Result<Option<&'a Value>, OpenApiExtractionError> {
    let mut current = value;
    let mut depth = 0;
    let mut seen = BTreeSet::new();
    loop {
        let Some(reference) = current.get("$ref").and_then(Value::as_str) else {
            return Ok(Some(current));
        };
        if !is_allowed_local_reference(reference, allowed_prefixes)
            || !seen.insert(reference.to_owned())
        {
            return Ok(None);
        }
        if depth >= MAX_LOCAL_REFERENCE_DEPTH {
            return Err(OpenApiExtractionError::TooManySchemaTraversalSteps);
        }
        consume_schema_traversal_step(traversal_steps, traversal_limit)?;
        let Some(resolved) = resolve_local_reference(root, reference) else {
            return Ok(None);
        };
        current = resolved;
        depth += 1;
    }
}

fn is_allowed_local_reference(reference: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|prefix| reference.starts_with(prefix))
}

fn consume_schema_traversal_step(
    traversal_steps: &mut usize,
    traversal_limit: usize,
) -> Result<(), OpenApiExtractionError> {
    if *traversal_steps >= traversal_limit {
        return Err(OpenApiExtractionError::TooManySchemaTraversalSteps);
    }
    *traversal_steps = (*traversal_steps)
        .checked_add(1)
        .ok_or(OpenApiExtractionError::TooManySchemaTraversalSteps)?;
    Ok(())
}

fn operation_content_types(
    root: &Value,
    operation: &serde_json::Map<String, Value>,
    traversal_steps: &mut usize,
    traversal_limit: usize,
) -> Result<Vec<String>, OpenApiExtractionError> {
    let mut content_types = Vec::new();
    if let Some(request_body) = operation.get("requestBody")
        && let Some(request_body) = resolve_local_component_chain(
            root,
            request_body,
            &["#/components/requestBodies/"],
            traversal_steps,
            traversal_limit,
        )?
        && let Some(content) = request_body.get("content").and_then(Value::as_object)
    {
        content_types.extend(content.keys().map(|value| value.to_ascii_lowercase()));
    }
    if let Some(responses) = operation.get("responses").and_then(Value::as_object) {
        for response in responses.values() {
            let Some(response) = resolve_local_component_chain(
                root,
                response,
                &["#/components/responses/"],
                traversal_steps,
                traversal_limit,
            )?
            else {
                continue;
            };
            if let Some(content) = response.get("content").and_then(Value::as_object) {
                content_types.extend(content.keys().map(|value| value.to_ascii_lowercase()));
            }
        }
    }
    Ok(content_types)
}

fn security_names(value: Option<&Value>, limit: usize) -> Vec<String> {
    let mut names = value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .flat_map(|requirement| requirement.keys())
        .take(limit)
        .map(|name| bounded_redacted(name, 512))
        .collect::<Vec<_>>();
    sort_deduplicate(&mut names);
    names
}

#[derive(Debug)]
struct YamlLine {
    indent: usize,
    text: String,
}

fn extract_yaml_openapi(
    source_url: &CanonicalUrl,
    document: &[u8],
    limits: OpenApiExtractionLimits,
) -> Result<Option<OpenApiEvidence>, OpenApiExtractionError> {
    let text = std::str::from_utf8(document).map_err(|_| OpenApiExtractionError::InvalidUtf8)?;
    let lines = parse_yaml_lines(text, limits)?;
    let Some(version) = yaml_root_scalar(&lines, "openapi")
        .filter(|version| version.starts_with("3."))
        .or_else(|| yaml_root_scalar(&lines, "swagger").filter(|version| version == "2.0"))
    else {
        return Ok(None);
    };

    let mut server_candidates = yaml_servers(source_url, &lines, &version, limits)?;
    sort_server_candidates(&mut server_candidates);
    let auth_schemes = yaml_auth_schemes(&lines, limits);
    let root_security = yaml_root_security_names(&lines, MAX_SECURITY_REFERENCES_PER_OPERATION);
    let operations = yaml_operations(&lines, &root_security, limits)?;
    Ok(Some(finalize_evidence(
        version,
        server_candidates,
        operations,
        auth_schemes,
    )))
}

fn parse_yaml_lines(
    text: &str,
    limits: OpenApiExtractionLimits,
) -> Result<Vec<YamlLine>, OpenApiExtractionError> {
    let mut lines = Vec::new();
    let mut indentation_stack = Vec::new();
    let mut block_scalar_indent = None;
    for raw in text.lines() {
        if raw.trim().is_empty() {
            continue;
        }
        let raw_indent = raw.len() - raw.trim_start().len();
        if let Some(parent_indent) = block_scalar_indent {
            if raw_indent > parent_indent {
                continue;
            }
            block_scalar_indent = None;
        }
        if raw.contains('\t') {
            return Err(OpenApiExtractionError::InvalidYaml);
        }
        let without_comment = strip_yaml_comment(raw);
        if without_comment.trim().is_empty() || without_comment.trim() == "---" {
            continue;
        }
        let indent = without_comment.len() - without_comment.trim_start().len();
        while indentation_stack
            .last()
            .is_some_and(|previous| *previous > indent)
        {
            indentation_stack.pop();
        }
        if indentation_stack
            .last()
            .is_none_or(|previous| *previous < indent)
        {
            indentation_stack.push(indent);
        }
        if indentation_stack.len() > limits.max_depth {
            return Err(OpenApiExtractionError::StructureTooDeep);
        }
        if lines.len() >= limits.max_nodes {
            return Err(OpenApiExtractionError::TooManyNodes);
        }
        let content = without_comment.trim().to_owned();
        if content.len() > limits.max_string_bytes {
            return Err(OpenApiExtractionError::StringTooLong);
        }
        if yaml_uses_unsupported_reference(&content) || yaml_uses_unsupported_flow_value(&content) {
            return Err(OpenApiExtractionError::InvalidYaml);
        }
        let declares_block_scalar = yaml_declares_block_scalar(&content);
        lines.push(YamlLine {
            indent,
            text: content,
        });
        if declares_block_scalar {
            block_scalar_indent = Some(indent);
        }
    }
    Ok(lines)
}

fn yaml_declares_block_scalar(value: &str) -> bool {
    let Some((_, scalar)) = value.split_once(':') else {
        return false;
    };
    let scalar = scalar.trim();
    scalar.strip_prefix(['|', '>']).is_some_and(|suffix| {
        suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'+' | b'-'))
    })
}

fn yaml_uses_unsupported_reference(value: &str) -> bool {
    let trimmed = value.strip_prefix("- ").unwrap_or(value).trim();
    if trimmed.starts_with("<<:") {
        return true;
    }
    trimmed.split_once(':').is_some_and(|(_, scalar)| {
        let scalar = scalar.trim();
        scalar.starts_with('&') || scalar.starts_with('*')
    }) || trimmed.starts_with('&')
        || trimmed.starts_with('*')
}

fn yaml_uses_unsupported_flow_value(value: &str) -> bool {
    let trimmed = value.strip_prefix("- ").unwrap_or(value).trim();
    let scalar = trimmed
        .split_once(':')
        .map_or(trimmed, |(_, scalar)| scalar.trim());
    (scalar.starts_with('[') && scalar != "[]") || (scalar.starts_with('{') && scalar != "{}")
}

fn strip_yaml_comment(value: &str) -> &str {
    let mut single_quoted = false;
    let mut double_quoted = false;
    let mut escaped = false;
    for (index, character) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' if double_quoted => escaped = true,
            '\'' if !double_quoted => single_quoted = !single_quoted,
            '"' if !single_quoted => double_quoted = !double_quoted,
            '#' if !single_quoted
                && !double_quoted
                && (index == 0 || value.as_bytes()[index - 1].is_ascii_whitespace()) =>
            {
                return &value[..index];
            }
            _ => {}
        }
    }
    value
}

fn yaml_root_scalar(lines: &[YamlLine], key: &str) -> Option<String> {
    lines
        .iter()
        .filter(|line| line.indent == 0)
        .find_map(|line| yaml_scalar_for_key(&line.text, key))
}

fn yaml_scalar_for_key(value: &str, key: &str) -> Option<String> {
    let (candidate, raw) = value.split_once(':')?;
    if unquote_yaml_scalar(candidate.trim()) != key {
        return None;
    }
    let scalar = unquote_yaml_scalar(raw.trim());
    (!scalar.is_empty() && !yaml_declares_block_scalar(value)).then_some(scalar.to_owned())
}

fn unquote_yaml_scalar(value: &str) -> &str {
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

fn yaml_servers(
    source_url: &CanonicalUrl,
    lines: &[YamlLine],
    version: &str,
    limits: OpenApiExtractionLimits,
) -> Result<Vec<OpenApiServerCandidate>, OpenApiExtractionError> {
    let mut servers = Vec::new();
    if version.starts_with("3.") {
        if let Some((start, section_indent)) = yaml_root_section(lines, "servers") {
            for line in section_lines(lines, start, section_indent) {
                let text = line.text.strip_prefix("- ").unwrap_or(&line.text);
                if let Some(raw) = yaml_scalar_for_key(text, "url") {
                    push_server(source_url, &raw, &mut servers, limits)?;
                }
            }
        }
    } else if let Some(host) = yaml_root_scalar(lines, "host") {
        let base_path = yaml_root_scalar(lines, "basePath").unwrap_or_else(|| "/".to_owned());
        let schemes = yaml_sequence_values(lines, "schemes");
        let borrowed = schemes.iter().map(String::as_str).collect::<Vec<_>>();
        let scheme = if borrowed.is_empty() {
            Some(source_url.url().scheme())
        } else {
            select_swagger_scheme(source_url.mode(), &borrowed)
        };
        if let Some(scheme) = scheme {
            push_server(
                source_url,
                &format!("{scheme}://{host}{base_path}"),
                &mut servers,
                limits,
            )?;
        }
    }
    Ok(servers)
}

fn yaml_root_section(lines: &[YamlLine], key: &str) -> Option<(usize, usize)> {
    lines.iter().enumerate().find_map(|(index, line)| {
        if line.indent != 0 {
            return None;
        }
        let candidate = line.text.strip_suffix(':')?;
        (unquote_yaml_scalar(candidate.trim()) == key).then_some((index, line.indent))
    })
}

fn yaml_nested_section(
    lines: &[YamlLine],
    root_key: &str,
    child_key: &str,
) -> Option<(usize, usize)> {
    let (root_start, root_indent) = yaml_root_section(lines, root_key)?;
    let children = section_lines(lines, root_start, root_indent).collect::<Vec<_>>();
    let child_indent = children.iter().map(|line| line.indent).min()?;
    lines
        .iter()
        .enumerate()
        .skip(root_start + 1)
        .take_while(|(_, line)| line.indent > root_indent)
        .find_map(|(index, line)| {
            if line.indent != child_indent {
                return None;
            }
            let candidate = line.text.strip_suffix(':')?;
            (unquote_yaml_scalar(candidate.trim()) == child_key).then_some((index, line.indent))
        })
}

fn section_lines(
    lines: &[YamlLine],
    start: usize,
    section_indent: usize,
) -> impl Iterator<Item = &YamlLine> {
    lines[start + 1..]
        .iter()
        .take_while(move |line| line.indent > section_indent)
}

fn yaml_sequence_values(lines: &[YamlLine], key: &str) -> Vec<String> {
    let Some((start, indent)) = yaml_root_section(lines, key) else {
        return Vec::new();
    };
    section_lines(lines, start, indent)
        .filter_map(|line| line.text.strip_prefix("- "))
        .map(unquote_yaml_scalar)
        .map(ToOwned::to_owned)
        .collect()
}

fn yaml_auth_schemes(
    lines: &[YamlLine],
    limits: OpenApiExtractionLimits,
) -> Vec<OpenApiAuthSchemeEvidence> {
    let Some((start, indent)) = yaml_nested_section(lines, "components", "securitySchemes")
        .or_else(|| yaml_root_section(lines, "securityDefinitions"))
    else {
        return Vec::new();
    };
    let section = section_lines(lines, start, indent).collect::<Vec<_>>();
    let Some(scheme_indent) = section.iter().map(|line| line.indent).min() else {
        return Vec::new();
    };
    let mut schemes = Vec::new();
    let mut index = 0;
    while index < section.len() && schemes.len() < MAX_SECURITY_SCHEMES {
        let line = section[index];
        if line.indent != scheme_indent || !line.text.ends_with(':') {
            index += 1;
            continue;
        }
        let name = unquote_yaml_scalar(line.text[..line.text.len() - 1].trim());
        let end = section[index + 1..]
            .iter()
            .position(|candidate| candidate.indent <= scheme_indent)
            .map_or(section.len(), |relative| index + 1 + relative);
        let block = &section[index + 1..end];
        schemes.push(OpenApiAuthSchemeEvidence {
            name: bounded_redacted(name, limits.max_string_bytes),
            kind: yaml_block_scalar(block, "type", limits.max_string_bytes),
            location: yaml_block_scalar(block, "in", limits.max_string_bytes),
            parameter_name: yaml_block_scalar(block, "name", limits.max_string_bytes),
            scheme: yaml_block_scalar(block, "scheme", limits.max_string_bytes),
        });
        index = end;
    }
    schemes
}

fn yaml_block_scalar(lines: &[&YamlLine], key: &str, max_bytes: usize) -> Option<String> {
    lines.iter().find_map(|line| {
        yaml_scalar_for_key(&line.text, key).map(|value| bounded_redacted(&value, max_bytes))
    })
}

#[allow(clippy::too_many_lines)]
fn yaml_operations(
    lines: &[YamlLine],
    root_security: &[String],
    limits: OpenApiExtractionLimits,
) -> Result<Vec<OpenApiOperationEvidence>, OpenApiExtractionError> {
    let Some((start, paths_indent)) = yaml_root_section(lines, "paths") else {
        return Ok(Vec::new());
    };
    let section = section_lines(lines, start, paths_indent).collect::<Vec<_>>();
    let Some(path_indent) = section
        .iter()
        .filter(|line| yaml_path_key(&line.text).is_some())
        .map(|line| line.indent)
        .min()
    else {
        return Ok(Vec::new());
    };
    let path_count = section
        .iter()
        .filter(|line| line.indent == path_indent && yaml_path_key(&line.text).is_some())
        .count();
    if path_count > limits.max_paths {
        return Err(OpenApiExtractionError::TooManyPaths);
    }

    let mut operations = Vec::new();
    let mut total_parameters = 0_usize;
    let mut security_reference_total = 0_usize;
    let mut path_index = 0;
    while path_index < section.len() {
        let path_line = section[path_index];
        if path_line.indent != path_indent {
            path_index += 1;
            continue;
        }
        let Some(path) = yaml_path_key(&path_line.text) else {
            path_index += 1;
            continue;
        };
        let path_end = section[path_index + 1..]
            .iter()
            .position(|line| line.indent <= path_indent)
            .map_or(section.len(), |relative| path_index + 1 + relative);
        let path_block = &section[path_index + 1..path_end];
        let Some(method_indent) = path_block
            .iter()
            .filter(|line| yaml_http_method(&line.text).is_some())
            .map(|line| line.indent)
            .min()
        else {
            path_index = path_end;
            continue;
        };
        let path_parameter_lines =
            yaml_direct_subsection_lines(path_block, "parameters").unwrap_or_default();

        let mut method_index = 0;
        while method_index < path_block.len() {
            let method_line = path_block[method_index];
            let Some(method) = (method_line.indent == method_indent)
                .then(|| yaml_http_method(&method_line.text))
                .flatten()
            else {
                method_index += 1;
                continue;
            };
            if operations.len() >= limits.max_operations {
                return Err(OpenApiExtractionError::TooManyOperations);
            }
            let method_end = path_block[method_index + 1..]
                .iter()
                .position(|line| line.indent <= method_indent)
                .map_or(path_block.len(), |relative| method_index + 1 + relative);
            let block = &path_block[method_index + 1..method_end];
            let operation_parameter_lines =
                yaml_direct_subsection_lines(block, "parameters").unwrap_or_default();
            let request_body_lines =
                yaml_direct_subsection_lines(block, "requestBody").unwrap_or_default();
            let mut parameter_names =
                yaml_parameter_names(lines, &path_parameter_lines, limits.max_parameters);
            parameter_names.extend(yaml_parameter_names(
                lines,
                &operation_parameter_lines,
                limits.max_parameters.saturating_sub(parameter_names.len()),
            ));
            sort_deduplicate(&mut parameter_names);
            let mut request_lines = Vec::new();
            request_lines.extend(path_parameter_lines.iter().copied());
            request_lines.extend(operation_parameter_lines.iter().copied());
            request_lines.extend(request_body_lines);
            let mut request_field_names = yaml_request_field_names(
                lines,
                &request_lines,
                limits.max_parameters.saturating_sub(parameter_names.len()),
            );
            sort_deduplicate(&mut request_field_names);
            total_parameters = total_parameters
                .checked_add(parameter_names.len())
                .and_then(|total| total.checked_add(request_field_names.len()))
                .ok_or(OpenApiExtractionError::TooManyParameters)?;
            if total_parameters > limits.max_parameters {
                return Err(OpenApiExtractionError::TooManyParameters);
            }
            let mut content_types = yaml_content_types(block);
            sort_deduplicate(&mut content_types);
            let mut security_schemes = yaml_operation_security_names(block, root_security);
            sort_deduplicate(&mut security_schemes);
            security_reference_total = security_reference_total
                .checked_add(security_schemes.len())
                .ok_or(OpenApiExtractionError::TooManySecurityReferences)?;
            let security_reference_limit = limits
                .max_parameters
                .saturating_mul(4)
                .min(MAX_TOTAL_OPERATION_SECURITY_REFERENCES);
            if security_reference_total > security_reference_limit {
                return Err(OpenApiExtractionError::TooManySecurityReferences);
            }
            operations.push(OpenApiOperationEvidence {
                method: method.to_owned(),
                path: bounded_redacted(path, 2_048),
                operation_id: yaml_block_scalar(block, "operationId", limits.max_string_bytes),
                summary: yaml_block_scalar(block, "summary", limits.max_string_bytes),
                parameter_names,
                request_field_names,
                content_types,
                security_schemes,
            });
            method_index = method_end;
        }
        path_index = path_end;
    }
    Ok(operations)
}

fn yaml_path_key(value: &str) -> Option<&str> {
    let candidate = value.strip_suffix(':')?.trim();
    let path = unquote_yaml_scalar(candidate);
    valid_api_path(path).then_some(path)
}

fn yaml_http_method(value: &str) -> Option<&'static str> {
    match value.to_ascii_lowercase().as_str() {
        "delete:" => Some("DELETE"),
        "get:" => Some("GET"),
        "head:" => Some("HEAD"),
        "options:" => Some("OPTIONS"),
        "patch:" => Some("PATCH"),
        "post:" => Some("POST"),
        "put:" => Some("PUT"),
        _ => None,
    }
}

fn yaml_parameter_names(all_lines: &[YamlLine], lines: &[&YamlLine], limit: usize) -> Vec<String> {
    let Some(parameter_indent) = lines.iter().map(|line| line.indent).min() else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for line in lines.iter().filter(|line| line.indent == parameter_indent) {
        let text = line.text.strip_prefix("- ").unwrap_or(&line.text);
        if let Some(value) = yaml_scalar_for_key(text, "name") {
            names.push(bounded_redacted(&value, 512));
        } else if let Some(reference) = yaml_scalar_for_key(text, "$ref")
            && let Some(block) = yaml_schema_block(all_lines, &reference)
            && let Some(value) = yaml_direct_scalar(&block, "name", 512)
        {
            names.push(value);
        }
        if names.len() >= limit {
            break;
        }
    }
    names
}

fn yaml_direct_scalar(lines: &[&YamlLine], key: &str, max_bytes: usize) -> Option<String> {
    let direct_indent = lines.iter().map(|line| line.indent).min()?;
    lines
        .iter()
        .filter(|line| line.indent == direct_indent)
        .find_map(|line| {
            yaml_scalar_for_key(&line.text, key).map(|value| bounded_redacted(&value, max_bytes))
        })
}

fn yaml_property_names(lines: &[&YamlLine], limit: usize) -> Vec<String> {
    let mut properties = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if line.text != "properties:" {
            continue;
        }
        let indent = line.indent;
        let Some(property_indent) = lines[index + 1..]
            .iter()
            .take_while(|candidate| candidate.indent > indent)
            .map(|candidate| candidate.indent)
            .min()
        else {
            continue;
        };
        for candidate in lines[index + 1..]
            .iter()
            .take_while(|candidate| candidate.indent > indent)
            .filter(|candidate| candidate.indent == property_indent)
        {
            if properties.len() >= limit {
                return properties;
            }
            if let Some(name) = candidate.text.strip_suffix(':') {
                properties.push(bounded_redacted(unquote_yaml_scalar(name.trim()), 512));
            }
        }
    }
    properties
}

fn yaml_request_field_names(
    all_lines: &[YamlLine],
    request_lines: &[&YamlLine],
    limit: usize,
) -> Vec<String> {
    if limit == 0 {
        return Vec::new();
    }
    let mut names = yaml_property_names(request_lines, limit);
    let mut references = request_lines
        .iter()
        .filter_map(|line| yaml_scalar_for_key(&line.text, "$ref"))
        .collect::<Vec<_>>();
    references.sort();
    references.dedup();

    let mut seen = BTreeSet::new();
    for reference in references {
        collect_yaml_schema_property_names(all_lines, &reference, 0, limit, &mut names, &mut seen);
        if names.len() >= limit {
            break;
        }
    }
    names
}

fn yaml_direct_subsection_lines<'a>(
    lines: &[&'a YamlLine],
    key: &str,
) -> Option<Vec<&'a YamlLine>> {
    let direct_indent = lines.iter().map(|line| line.indent).min()?;
    let expected = format!("{key}:");
    let (index, section) = lines
        .iter()
        .enumerate()
        .find(|(_, line)| line.indent == direct_indent && line.text == expected)?;
    Some(
        lines[index + 1..]
            .iter()
            .copied()
            .take_while(|line| line.indent > section.indent)
            .collect(),
    )
}

fn collect_yaml_schema_property_names(
    all_lines: &[YamlLine],
    reference: &str,
    depth: usize,
    limit: usize,
    names: &mut Vec<String>,
    seen: &mut BTreeSet<String>,
) {
    if depth > MAX_LOCAL_REFERENCE_DEPTH
        || names.len() >= limit
        || !seen.insert(reference.to_owned())
    {
        return;
    }
    let Some(block) = yaml_schema_block(all_lines, reference) else {
        return;
    };
    names.extend(yaml_property_names(
        &block,
        limit.saturating_sub(names.len()),
    ));
    for nested_reference in block
        .iter()
        .filter_map(|line| yaml_scalar_for_key(&line.text, "$ref"))
    {
        collect_yaml_schema_property_names(
            all_lines,
            &nested_reference,
            depth + 1,
            limit,
            names,
            seen,
        );
        if names.len() >= limit {
            return;
        }
    }
}

fn yaml_schema_block<'a>(lines: &'a [YamlLine], reference: &str) -> Option<Vec<&'a YamlLine>> {
    let (section_start, section_indent, encoded_name) =
        if let Some(name) = reference.strip_prefix("#/components/schemas/") {
            let (start, indent) = yaml_nested_section(lines, "components", "schemas")?;
            (start, indent, name)
        } else if let Some(name) = reference.strip_prefix("#/components/requestBodies/") {
            let (start, indent) = yaml_nested_section(lines, "components", "requestBodies")?;
            (start, indent, name)
        } else if let Some(name) = reference.strip_prefix("#/components/parameters/") {
            let (start, indent) = yaml_nested_section(lines, "components", "parameters")?;
            (start, indent, name)
        } else if let Some(name) = reference.strip_prefix("#/definitions/") {
            let (start, indent) = yaml_root_section(lines, "definitions")?;
            (start, indent, name)
        } else {
            return None;
        };
    let schema_name = decode_json_pointer_segment(encoded_name);
    let section = section_lines(lines, section_start, section_indent).collect::<Vec<_>>();
    let entry_indent = section.iter().map(|line| line.indent).min()?;
    let entry_index = section.iter().position(|line| {
        line.indent == entry_indent
            && line
                .text
                .strip_suffix(':')
                .is_some_and(|candidate| unquote_yaml_scalar(candidate.trim()) == schema_name)
    })?;
    let entry = section[entry_index];
    Some(
        section[entry_index + 1..]
            .iter()
            .copied()
            .take_while(|line| line.indent > entry.indent)
            .collect(),
    )
}

fn decode_json_pointer_segment(value: &str) -> String {
    value.replace("~1", "/").replace("~0", "~")
}

fn yaml_content_types(lines: &[&YamlLine]) -> Vec<String> {
    lines
        .iter()
        .filter_map(|line| line.text.strip_suffix(':'))
        .map(unquote_yaml_scalar)
        .filter(|candidate| {
            let lower = candidate.to_ascii_lowercase();
            lower.contains('/')
                && matches!(
                    lower.as_str(),
                    "application/json"
                        | "application/x-ndjson"
                        | "application/jsonl"
                        | "text/event-stream"
                        | "application/problem+json"
                )
        })
        .map(str::to_ascii_lowercase)
        .collect()
}

fn yaml_root_security_names(lines: &[YamlLine], limit: usize) -> Vec<String> {
    let Some((start, section_indent)) = yaml_root_section(lines, "security") else {
        return Vec::new();
    };
    let block = section_lines(lines, start, section_indent).collect::<Vec<_>>();
    yaml_security_requirement_names(&block, limit)
}

fn yaml_operation_security_names(lines: &[&YamlLine], root_security: &[String]) -> Vec<String> {
    let Some(direct_indent) = lines.iter().map(|line| line.indent).min() else {
        return root_security.to_vec();
    };
    let Some((index, security)) = lines.iter().enumerate().find(|(_, line)| {
        line.indent == direct_indent
            && (line.text == "security:" || line.text.starts_with("security: "))
    }) else {
        return root_security.to_vec();
    };
    if security.text != "security:" {
        return Vec::new();
    }
    let block = lines[index + 1..]
        .iter()
        .copied()
        .take_while(|line| line.indent > security.indent)
        .collect::<Vec<_>>();
    yaml_security_requirement_names(&block, MAX_SECURITY_REFERENCES_PER_OPERATION)
}

fn yaml_security_requirement_names(lines: &[&YamlLine], limit: usize) -> Vec<String> {
    let mut names = Vec::new();
    for line in lines {
        let text = line.text.strip_prefix("- ").unwrap_or(&line.text);
        let Some((name, _)) = text.split_once(':') else {
            continue;
        };
        let name = unquote_yaml_scalar(name.trim());
        if name.is_empty() {
            continue;
        }
        names.push(bounded_redacted(name, 512));
        if names.len() >= limit {
            break;
        }
    }
    sort_deduplicate(&mut names);
    names
}

fn finalize_evidence(
    specification_version: String,
    server_candidates: Vec<OpenApiServerCandidate>,
    operations: Vec<OpenApiOperationEvidence>,
    auth_schemes: Vec<OpenApiAuthSchemeEvidence>,
) -> OpenApiEvidence {
    let mut model_list_paths = Vec::new();
    let mut generation_paths = Vec::new();
    let mut streaming_media_types = Vec::new();
    let mut api_family_hints = BTreeSet::new();

    for operation in &operations {
        let path = operation.path.to_ascii_lowercase();
        let operation_id = operation
            .operation_id
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if operation.method == "GET"
            && (path.ends_with("/models")
                || operation_id.contains("listmodels")
                || operation_id.contains("getmodels"))
        {
            model_list_paths.push(operation.path.clone());
        }
        if operation.method == "POST" && likely_generation_operation(&path, &operation_id) {
            generation_paths.push(operation.path.clone());
        }
        for content_type in &operation.content_types {
            if matches!(
                content_type.as_str(),
                "text/event-stream" | "application/x-ndjson" | "application/jsonl"
            ) {
                streaming_media_types.push(content_type.clone());
            }
        }

        if path.ends_with("/responses") {
            api_family_hints.insert(ApiFamilyHint::OpenAiResponses);
        }
        if path.contains("/chat/completions") {
            api_family_hints.insert(ApiFamilyHint::OpenAiChatCompletions);
        }
        if path.ends_with("/messages")
            && operation
                .request_field_names
                .iter()
                .any(|name| name == "max_tokens")
        {
            api_family_hints.insert(ApiFamilyHint::AnthropicMessages);
        }
        if path.contains("generatecontent") {
            api_family_hints.insert(ApiFamilyHint::GeminiGenerateContent);
        }
        if path.ends_with("/api/chat") {
            api_family_hints.insert(ApiFamilyHint::OllamaNative);
        }
    }

    sort_deduplicate(&mut model_list_paths);
    sort_deduplicate(&mut generation_paths);
    sort_deduplicate(&mut streaming_media_types);
    OpenApiEvidence {
        specification_version: bounded_redacted(&specification_version, 64),
        server_candidates,
        operations,
        auth_schemes,
        model_list_paths,
        generation_paths,
        streaming_media_types,
        api_family_hints: api_family_hints.into_iter().collect(),
    }
}

fn likely_generation_operation(path: &str, operation_id: &str) -> bool {
    [
        "/responses",
        "/chat/completions",
        "/completions",
        "/messages",
        "generatecontent",
        "/api/chat",
        "/generate",
    ]
    .iter()
    .any(|marker| path.contains(marker))
        || ["generate", "completion", "create_message", "createchat"]
            .iter()
            .any(|marker| operation_id.contains(marker))
}

fn valid_api_path(path: &str) -> bool {
    path.starts_with('/')
        && !path.starts_with("//")
        && path.len() <= 2_048
        && !path.contains(['?', '#', '\\'])
        && !path.bytes().any(|byte| byte.is_ascii_control())
        && !path.split('/').any(|segment| segment == "..")
        && !path_looks_credential_bearing(path)
}

fn bounded_redacted(value: &str, max_bytes: usize) -> String {
    redact_and_bound(value, max_bytes)
}

fn sort_deduplicate<T: Ord>(values: &mut Vec<T>) {
    values.sort();
    values.dedup();
}

pub(crate) fn summarize_json(
    document: &[u8],
    limits: OpenApiExtractionLimits,
) -> Result<(Vec<String>, bool), OpenApiExtractionError> {
    let limits = limits.validate()?;
    if document.len() > limits.max_bytes {
        return Err(OpenApiExtractionError::DocumentTooLarge);
    }
    preflight_json_bounds(document, limits)?;
    let root: Value =
        serde_json::from_slice(document).map_err(|_| OpenApiExtractionError::InvalidJson)?;
    validate_json_bounds(&root, limits)?;
    let Some(object) = root.as_object() else {
        return Ok((Vec::new(), false));
    };
    let mut keys = object.keys().take(256).cloned().collect::<Vec<_>>();
    sort_deduplicate(&mut keys);
    let looks_like_json_schema = object.contains_key("$schema")
        || (object.contains_key("type")
            && (object.contains_key("properties") || object.contains_key("$defs")));
    Ok((keys, looks_like_json_schema))
}

pub(crate) fn summarize_yaml(
    document: &[u8],
    limits: OpenApiExtractionLimits,
) -> Result<Vec<String>, OpenApiExtractionError> {
    let limits = limits.validate()?;
    if document.len() > limits.max_bytes {
        return Err(OpenApiExtractionError::DocumentTooLarge);
    }
    let text = std::str::from_utf8(document).map_err(|_| OpenApiExtractionError::InvalidUtf8)?;
    let lines = parse_yaml_lines(text, limits)?;
    let mut keys = lines
        .iter()
        .filter(|line| line.indent == 0)
        .filter_map(|line| line.text.split_once(':').map(|(key, _)| key))
        .map(unquote_yaml_scalar)
        .take(256)
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    sort_deduplicate(&mut keys);
    Ok(keys)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn local_source() -> CanonicalUrl {
        CanonicalUrl::parse(
            "http://127.0.0.1:11434/openapi.json",
            UrlPolicyMode::LocalLoopback,
        )
        .unwrap()
    }

    #[test]
    fn extracts_json_openapi_without_examples_or_descriptions() {
        let document = br##"{
          "openapi": "3.1.0",
          "servers": [{"url": "/v1?api_key=must-not-survive"}],
          "components": {
            "securitySchemes": {
              "ApiKey": {"type": "apiKey", "in": "header", "name": "x-api-key"}
            },
            "schemas": {
              "Request": {
                "type": "object",
                "properties": {
                  "model": {"type": "string"},
                  "input": {"type": "string"},
                  "stream": {"type": "boolean"},
                  "api_key": {"example": "raw-secret-example"}
                }
              }
            },
            "requestBodies": {
              "ResponseRequest": {
                "content": {
                  "application/json": {
                    "schema": {"$ref": "#/components/schemas/Request"}
                  }
                }
              }
            }
          },
          "paths": {
            "/models": {
              "get": {"operationId": "listModels", "responses": {"200": {"description": "ok"}}}
            },
            "/responses": {
              "post": {
                "operationId": "createResponse",
                "summary": "Bearer document-secret",
                "security": [{"ApiKey": []}],
                "requestBody": {"$ref": "#/components/requestBodies/ResponseRequest"},
                "responses": {
                  "200": {
                    "description": "contains raw-secret-example",
                    "content": {"text/event-stream": {"schema": {"type": "string"}}}
                  }
                }
              }
            }
          }
        }"##;
        let evidence = extract_openapi(
            &local_source(),
            OpenApiDocumentFormat::Json,
            document,
            OpenApiExtractionLimits::default(),
        )
        .unwrap()
        .unwrap();

        assert_eq!(evidence.specification_version, "3.1.0");
        assert_eq!(
            evidence.server_candidates[0].origin(),
            "http://127.0.0.1:11434"
        );
        assert_eq!(evidence.server_candidates[0].base_path().as_str(), "/v1");
        assert_eq!(evidence.model_list_paths, ["/models"]);
        assert_eq!(evidence.generation_paths, ["/responses"]);
        assert_eq!(evidence.streaming_media_types, ["text/event-stream"]);
        assert_eq!(evidence.api_family_hints, [ApiFamilyHint::OpenAiResponses]);
        let operation = evidence
            .operations
            .iter()
            .find(|operation| operation.path == "/responses")
            .unwrap();
        assert_eq!(
            operation.request_field_names,
            ["api_key", "input", "model", "stream"]
        );
        assert!(
            !operation
                .summary
                .as_deref()
                .unwrap()
                .contains("document-secret")
        );
        let serialized = serde_json::to_string(&evidence).unwrap();
        assert!(!serialized.contains("raw-secret-example"));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn extracts_bounded_yaml_openapi() {
        let document = br##"
openapi: 3.0.3
components:
  schemas:
    Request:
      type: object
      properties:
        servers:
          type: string
        model:
          type: string
        messages:
          type: array
        stream:
          type: boolean
  requestBodies:
    ChatRequest:
      content:
        application/json:
          schema:
            $ref: "#/components/schemas/Request"
  parameters:
    TraceParameter:
      name: trace
      in: header
      schema:
        type: string
  securitySchemes:
    BearerAuth:
      type: http
      scheme: bearer
servers:
  - url: http://127.0.0.1:11434/v1
    description: |
      url: http://10.0.0.1/private
      security:
        - Injected: []
security:
  - BearerAuth: []
paths:
  /models:
    get:
      security: []
      operationId: listModels
      responses:
        "200":
          content:
            application/json:
              schema:
                type: object
  /chat/completions:
    parameters:
      - name: tenant
        in: header
    post:
      operationId: createChatCompletion
      summary: "create a response"
      parameters:
        - $ref: "#/components/parameters/TraceParameter"
      requestBody:
        $ref: "#/components/requestBodies/ChatRequest"
      responses:
        "200":
          content:
            text/event-stream:
              schema:
                type: string
  /messages:
    post:
      responses:
        "200":
          content:
            application/json:
              schema:
                type: object
                properties:
                  max_tokens:
                    type: integer
                  name:
                    type: string
"##;
        let evidence = extract_openapi(
            &local_source(),
            OpenApiDocumentFormat::Yaml,
            document,
            OpenApiExtractionLimits::default(),
        )
        .unwrap()
        .unwrap();

        assert_eq!(evidence.model_list_paths, ["/models"]);
        assert_eq!(
            evidence.generation_paths,
            ["/chat/completions", "/messages"]
        );
        assert_eq!(
            evidence.api_family_hints,
            [ApiFamilyHint::OpenAiChatCompletions]
        );
        assert_eq!(evidence.server_candidates[0].base_path().as_str(), "/v1");
        assert_eq!(evidence.auth_schemes[0].name, "BearerAuth");
        assert_eq!(evidence.auth_schemes[0].scheme.as_deref(), Some("bearer"));
        let chat = evidence
            .operations
            .iter()
            .find(|operation| operation.path == "/chat/completions")
            .unwrap();
        assert_eq!(
            chat.request_field_names,
            ["messages", "model", "servers", "stream"]
        );
        assert_eq!(chat.parameter_names, ["tenant", "trace"]);
        assert_eq!(chat.security_schemes, ["BearerAuth"]);
        let models = evidence
            .operations
            .iter()
            .find(|operation| operation.path == "/models")
            .unwrap();
        assert!(models.security_schemes.is_empty());
        let messages = evidence
            .operations
            .iter()
            .find(|operation| operation.path == "/messages")
            .unwrap();
        assert!(messages.request_field_names.is_empty());
        assert!(messages.parameter_names.is_empty());
        assert!(
            !evidence
                .api_family_hints
                .contains(&ApiFamilyHint::AnthropicMessages)
        );
    }

    #[test]
    fn rejects_unsafe_cross_mode_server_candidates() {
        let document =
            br#"{"openapi":"3.0.0","servers":[{"url":"http://10.0.0.1/v1"}],"paths":{}}"#;
        assert!(matches!(
            extract_openapi(
                &local_source(),
                OpenApiDocumentFormat::Json,
                document,
                OpenApiExtractionLimits::default()
            ),
            Err(OpenApiExtractionError::UnsafeServerUrl(_))
        ));
    }

    #[test]
    fn rejects_server_paths_that_look_credential_bearing() {
        let document = br#"{
          "openapi": "3.0.0",
          "servers": [{"url": "/webhook/A7b9C2d4E6f8G1h3"}],
          "paths": {}
        }"#;
        assert!(matches!(
            extract_openapi(
                &local_source(),
                OpenApiDocumentFormat::Json,
                document,
                OpenApiExtractionLimits::default()
            ),
            Err(OpenApiExtractionError::PotentialCredentialInServerPath)
        ));
    }

    #[test]
    fn omits_operation_paths_that_look_credential_bearing() {
        let document = br#"{
          "openapi": "3.0.0",
          "paths": {
            "/webhook/A7b9C2d4E6f8G1h3": {
              "post": {"responses": {"200": {"description": "ok"}}}
            },
            "/v1/responses": {
              "post": {"responses": {"200": {"description": "ok"}}}
            }
          }
        }"#;
        let evidence = extract_openapi(
            &local_source(),
            OpenApiDocumentFormat::Json,
            document,
            OpenApiExtractionLimits::default(),
        )
        .unwrap()
        .unwrap();

        assert_eq!(evidence.operations.len(), 1);
        assert_eq!(evidence.operations[0].path(), "/v1/responses");
        assert!(!serde_json::to_string(&evidence).unwrap().contains("A7b9"));
    }

    #[test]
    fn root_security_requirements_are_bounded_before_operation_copying() {
        let root_security = (0..256)
            .map(|index| {
                let mut requirement = serde_json::Map::new();
                requirement.insert(format!("Scheme{index:03}"), Value::Array(Vec::new()));
                Value::Object(requirement)
            })
            .collect::<Vec<_>>();
        let mut document = json!({
            "openapi": "3.0.0",
            "paths": {
                "/responses": {
                    "post": {
                        "responses": {"200": {"description": "ok"}}
                    }
                }
            }
        });
        document["security"] = Value::Array(root_security);
        let bytes = serde_json::to_vec(&document).unwrap();
        let evidence = extract_openapi(
            &local_source(),
            OpenApiDocumentFormat::Json,
            &bytes,
            OpenApiExtractionLimits::default(),
        )
        .unwrap()
        .unwrap();

        assert_eq!(
            evidence.operations[0].security_schemes().len(),
            MAX_SECURITY_REFERENCES_PER_OPERATION
        );
    }

    #[test]
    fn cyclic_json_references_are_visited_once() {
        let repeated = (0..64)
            .map(|_| json!({"$ref": "#/components/schemas/Loop"}))
            .collect::<Vec<_>>();
        let document = json!({
            "openapi": "3.0.0",
            "components": {
                "schemas": {
                    "Loop": {"allOf": repeated}
                }
            },
            "paths": {
                "/responses": {
                    "post": {
                        "requestBody": {
                            "content": {
                                "application/json": {
                                    "schema": {"$ref": "#/components/schemas/Loop"}
                                }
                            }
                        },
                        "responses": {"200": {"description": "ok"}}
                    }
                }
            }
        });
        let bytes = serde_json::to_vec(&document).unwrap();
        let evidence = extract_openapi(
            &local_source(),
            OpenApiDocumentFormat::Json,
            &bytes,
            OpenApiExtractionLimits::default(),
        )
        .unwrap()
        .unwrap();

        assert!(evidence.operations[0].request_field_names().is_empty());
    }

    #[test]
    fn json_schema_traversal_budget_fails_closed() {
        let parts = (0..16)
            .map(|_| json!({"type": "string"}))
            .collect::<Vec<_>>();
        let document = json!({
            "openapi": "3.0.0",
            "paths": {
                "/responses": {
                    "post": {
                        "requestBody": {
                            "content": {
                                "application/json": {
                                    "schema": {"allOf": parts}
                                }
                            }
                        },
                        "responses": {"200": {"description": "ok"}}
                    }
                }
            }
        });
        let bytes = serde_json::to_vec(&document).unwrap();
        let limits = OpenApiExtractionLimits {
            max_parameters: 1,
            ..OpenApiExtractionLimits::default()
        };

        assert!(matches!(
            extract_openapi(&local_source(), OpenApiDocumentFormat::Json, &bytes, limits,),
            Err(OpenApiExtractionError::TooManySchemaTraversalSteps)
        ));
    }

    #[test]
    fn non_openapi_json_is_not_misclassified() {
        assert_eq!(
            extract_openapi(
                &local_source(),
                OpenApiDocumentFormat::Json,
                br#"{"hello":"world"}"#,
                OpenApiExtractionLimits::default()
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn json_depth_and_node_limits_fail_closed() {
        let limits = OpenApiExtractionLimits {
            max_depth: 2,
            ..OpenApiExtractionLimits::default()
        };
        assert!(matches!(
            extract_openapi(
                &local_source(),
                OpenApiDocumentFormat::Json,
                br#"{"openapi":"3.0.0","paths":{"x":{"y":{}}}}"#,
                limits
            ),
            Err(OpenApiExtractionError::StructureTooDeep)
        ));
    }

    #[test]
    fn unsupported_yaml_aliases_and_nonempty_flow_values_fail_closed() {
        for document in [
            "openapi: 3.0.0\npaths: *shared\n",
            "openapi: 3.0.0\npaths: {/responses: {}}\n",
        ] {
            assert!(matches!(
                extract_openapi(
                    &local_source(),
                    OpenApiDocumentFormat::Yaml,
                    document.as_bytes(),
                    OpenApiExtractionLimits::default(),
                ),
                Err(OpenApiExtractionError::InvalidYaml)
            ));
        }
    }
}
