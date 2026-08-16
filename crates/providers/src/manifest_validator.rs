use std::{
    collections::{HashMap, HashSet},
    net::IpAddr,
};

use lorepia_domain::{
    ApiFamily, AuthBinding, ConnectionFieldSpec, ConnectionFieldType, CoreError, CoreResult,
    DecoderId, EndpointSpec, HttpMethod, ManifestSource, ParameterLiteral, ParameterSpec,
    ParameterType, ProviderManifest, ProviderParameterTarget,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use url::{Host, Url};

use crate::discovery::{
    contains_credential_like_token, looks_like_known_credential, looks_like_opaque_secret,
};

const SUPPORTED_SCHEMA_VERSION: u32 = 1;
const MAX_MANIFEST_BYTES: usize = 256 * 1024;
const MAX_SOURCE_COUNT: usize = 32;
const MAX_SOURCE_URL_BYTES: usize = 2_048;
const MAX_SOURCE_URL_BYTES_TOTAL: usize = 16 * 1024;
const MAX_ENDPOINT_PATH_BYTES: usize = 1_024;
const MAX_CONNECTION_FIELD_COUNT: usize = 32;
const MAX_PARAMETER_COUNT: usize = 128;
const MAX_PARAMETER_ID_BYTES: usize = 96;
const MAX_METADATA_KEY_BYTES: usize = 256;
const MAX_PARAMETER_CHOICES: usize = 128;
const MAX_LITERAL_STRING_BYTES: usize = 16 * 1024;
const MAX_LITERAL_LIST_ITEMS: usize = 128;
const MAX_JSON_SCHEMA_NODES: usize = 2_048;
const MAX_JSON_SCHEMA_DEPTH: usize = 32;

/// A manifest which passed the closed adapter, network, and request-mapping policy.
///
/// Its fields remain private so callers cannot mutate a manifest after validation
/// without obtaining a fresh validation result and hash.
#[derive(Debug, Clone)]
pub struct ValidatedProviderManifest {
    manifest: ProviderManifest,
    sha256: String,
}

impl ValidatedProviderManifest {
    pub fn manifest(&self) -> &ProviderManifest {
        &self.manifest
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}

/// Validates a provider manifest and returns its deterministic canonical JSON hash.
pub fn validate_manifest(manifest: &ProviderManifest) -> CoreResult<ValidatedProviderManifest> {
    if manifest.schema_version != SUPPORTED_SCHEMA_VERSION {
        return Err(invalid(
            "schema_version",
            format!(
                "expected {SUPPORTED_SCHEMA_VERSION}, got {}",
                manifest.schema_version
            ),
        ));
    }

    validate_family_and_decoders(manifest)?;
    validate_sources(&manifest.sources)?;

    if let Some(origin) = &manifest.default_api_origin {
        validate_secure_url("default_api_origin", origin.as_str(), false)?;
    }

    validate_auth(&manifest.auth)?;
    validate_endpoint("endpoints.generate", &manifest.endpoints.generate)?;
    if manifest.endpoints.generate.method != HttpMethod::Post {
        return Err(invalid(
            "endpoints.generate.method",
            "generation endpoints must use POST",
        ));
    }
    if let Some(models) = &manifest.endpoints.models {
        validate_endpoint("endpoints.models", models)?;
        if models.method != HttpMethod::Get {
            return Err(invalid(
                "endpoints.models.method",
                "model-list endpoints must use GET",
            ));
        }
    }
    if let Some(embeddings) = &manifest.endpoints.embeddings {
        validate_endpoint("endpoints.embeddings", embeddings)?;
        if embeddings.method != HttpMethod::Post {
            return Err(invalid(
                "endpoints.embeddings.method",
                "embedding endpoints must use POST",
            ));
        }
    }

    validate_parameters(&manifest.parameters)?;

    let canonical_json = canonical_manifest_json(manifest)?;
    if canonical_json.len() > MAX_MANIFEST_BYTES {
        return Err(invalid(
            "manifest",
            format!(
                "canonical manifest is {} bytes; limit is {MAX_MANIFEST_BYTES}",
                canonical_json.len()
            ),
        ));
    }

    let digest = Sha256::digest(&canonical_json);
    let sha256 = format!("{digest:x}");
    Ok(ValidatedProviderManifest {
        manifest: manifest.clone(),
        sha256,
    })
}

/// Validates the connection-field portion of a provider template.
///
/// Connection fields are outside `ProviderManifest` in the domain model, so
/// template registration must call this alongside [`validate_manifest`].
pub fn validate_connection_fields(fields: &[ConnectionFieldSpec]) -> CoreResult<()> {
    if fields.len() > MAX_CONNECTION_FIELD_COUNT {
        return Err(invalid(
            "connection_fields",
            format!(
                "contains {} fields; limit is {MAX_CONNECTION_FIELD_COUNT}",
                fields.len()
            ),
        ));
    }

    let mut keys = HashSet::with_capacity(fields.len());
    let mut credential_field_count = 0_usize;
    for (index, field) in fields.iter().enumerate() {
        let location = format!("connection_fields[{index}]");
        validate_identifier(
            &format!("{location}.key"),
            &field.key,
            MAX_PARAMETER_ID_BYTES,
        )?;
        validate_metadata_key(&format!("{location}.label_key"), &field.label_key)?;
        if let Some(description_key) = &field.description_key {
            validate_metadata_key(&format!("{location}.description_key"), description_key)?;
        }

        let normalized_key = field.key.to_ascii_lowercase();
        if !keys.insert(normalized_key) {
            return Err(invalid(
                format!("{location}.key"),
                "connection field keys must be unique ignoring ASCII case",
            ));
        }

        if field.value_type == ConnectionFieldType::Credential {
            credential_field_count += 1;
        } else if is_credential_name(&field.key) {
            return Err(invalid(
                format!("{location}.key"),
                "a credential-named field must use the credential field type",
            ));
        }
    }

    if credential_field_count > 1 {
        return Err(invalid(
            "connection_fields",
            "at most one credential field is supported",
        ));
    }

    Ok(())
}

fn validate_family_and_decoders(manifest: &ProviderManifest) -> CoreResult<()> {
    let (response, streaming) = match manifest.api_family {
        ApiFamily::OpenAiResponses | ApiFamily::OpenAiChatCompletions => {
            (DecoderId::OpenAiJsonV1, DecoderId::OpenAiSseV1)
        }
        ApiFamily::AnthropicMessages => (DecoderId::AnthropicJsonV1, DecoderId::AnthropicSseV1),
        ApiFamily::GeminiGenerateContent => (DecoderId::GeminiJsonV1, DecoderId::GeminiSseV1),
        ApiFamily::OllamaNative => (DecoderId::OllamaJsonV1, DecoderId::OllamaJsonlV1),
    };

    if manifest.decoders.response != response {
        return Err(invalid(
            "decoders.response",
            format!(
                "{:?} requires the {response:?} response decoder",
                manifest.api_family
            ),
        ));
    }
    if manifest
        .decoders
        .streaming
        .is_some_and(|decoder| decoder != streaming)
    {
        return Err(invalid(
            "decoders.streaming",
            format!(
                "{:?} requires the {streaming:?} streaming decoder",
                manifest.api_family
            ),
        ));
    }

    Ok(())
}

fn validate_sources(sources: &[ManifestSource]) -> CoreResult<()> {
    if sources.len() > MAX_SOURCE_COUNT {
        return Err(invalid(
            "sources",
            format!(
                "contains {} source URLs; limit is {MAX_SOURCE_COUNT}",
                sources.len()
            ),
        ));
    }

    let mut total_url_bytes = 0_usize;
    let mut seen_urls = HashSet::with_capacity(sources.len());
    for (index, source) in sources.iter().enumerate() {
        let location = format!("sources[{index}]");
        let raw_url = source.url.as_str();
        if raw_url.len() > MAX_SOURCE_URL_BYTES {
            return Err(invalid(
                format!("{location}.url"),
                format!("URL exceeds the {MAX_SOURCE_URL_BYTES}-byte limit"),
            ));
        }
        total_url_bytes = total_url_bytes
            .checked_add(raw_url.len())
            .ok_or_else(|| invalid("sources", "combined source URL size overflowed"))?;
        if total_url_bytes > MAX_SOURCE_URL_BYTES_TOTAL {
            return Err(invalid(
                "sources",
                format!("combined source URLs exceed the {MAX_SOURCE_URL_BYTES_TOTAL}-byte limit"),
            ));
        }

        let url = validate_secure_url(&format!("{location}.url"), raw_url, true)?;
        validate_source_url_secrets(&format!("{location}.url"), &url)?;
        if !seen_urls.insert(url.as_str().to_owned()) {
            return Err(invalid(
                format!("{location}.url"),
                "source URLs must be unique",
            ));
        }

        if let Some(hash) = &source.content_sha256
            && (hash.len() != 64
                || !hash
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
        {
            return Err(invalid(
                format!("{location}.content_sha256"),
                "content hash must be 64 lowercase hexadecimal characters",
            ));
        }
    }

    Ok(())
}

fn validate_secure_url(location: &str, value: &str, allow_resource_path: bool) -> CoreResult<Url> {
    let url = Url::parse(value)
        .map_err(|error| invalid(location, format!("URL could not be parsed: {error}")))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(invalid(location, "embedded URL credentials are forbidden"));
    }
    if url.host_str().is_some_and(|host| {
        host.parse::<IpAddr>().is_err() && host.split('.').any(contains_credential_like_token)
    }) {
        return Err(invalid(
            location,
            "URL host contains a credential-like opaque label",
        ));
    }
    if !allow_resource_path
        && (url.path() != "/" || url.query().is_some() || url.fragment().is_some())
    {
        return Err(invalid(
            location,
            "API origin must not include a path, query, or fragment",
        ));
    }

    match url.scheme() {
        "https" => {}
        "http" if is_loopback_host(&url) => {}
        "http" => {
            return Err(invalid(
                location,
                "plain HTTP is allowed only for an explicit loopback host",
            ));
        }
        _ => {
            return Err(invalid(
                location,
                "only HTTPS and loopback HTTP URLs are allowed",
            ));
        }
    }
    Ok(url)
}

fn is_loopback_host(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(domain)) => {
            let domain = domain.trim_end_matches('.');
            domain.eq_ignore_ascii_case("localhost")
                || domain
                    .to_ascii_lowercase()
                    .strip_suffix(".localhost")
                    .is_some_and(|prefix| !prefix.is_empty())
        }
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => IpAddr::V6(address).is_loopback(),
        None => false,
    }
}

fn validate_source_url_secrets(location: &str, url: &Url) -> CoreResult<()> {
    // Durable source provenance never needs a query or fragment. Reject both
    // wholesale instead of trying to predict every vendor's future credential
    // parameter name.
    if url.query().is_some() || url.fragment().is_some() {
        return Err(invalid(
            location,
            "source URLs must omit query parameters and fragments",
        ));
    }

    if url.host_str().is_some_and(|host| {
        host.parse::<IpAddr>().is_err()
            && host
                .split('.')
                .any(|label| looks_like_known_credential(label) || looks_like_opaque_secret(label))
    }) {
        return Err(invalid(
            location,
            "source URL host contains a credential-like opaque label",
        ));
    }

    for segment in url.path_segments().into_iter().flatten() {
        // `Url::path_segments` retains percent escapes. Inspect bounded decode
        // layers as well as the serialized form so `sk%2D...`,
        // `token%3D...`, or a double-encoded equivalent cannot enter durable
        // provenance. More deeply nested escapes fail closed.
        let mut decoded = segment.to_owned();
        for _ in 0..=2 {
            validate_source_path_component(location, &decoded)?;
            let Some(next) = percent_decode_once(&decoded)? else {
                break;
            };
            decoded = next;
        }
        validate_source_path_component(location, &decoded)?;
        if contains_percent_escape(&decoded) {
            return Err(invalid(
                location,
                "source URL path contains excessive nested encoding",
            ));
        }
    }

    Ok(())
}

fn validate_source_path_component(location: &str, component: &str) -> CoreResult<()> {
    if looks_like_credential_literal(component) || looks_like_opaque_secret(component) {
        return Err(invalid(
            location,
            "source URL path contains a credential-like opaque segment",
        ));
    }

    // A path component can also spell out a credential assignment without
    // looking high-entropy (for example `token=short-value`).
    for separator in ['=', ':'] {
        if let Some((key, value)) = component.split_once(separator)
            && is_credential_name(key)
            && !value.is_empty()
        {
            return Err(invalid(
                location,
                "source URL path contains a credential-bearing component",
            ));
        }
    }
    Ok(())
}

fn percent_decode_once(value: &str) -> CoreResult<Option<String>> {
    if !contains_percent_escape(value) {
        return Ok(None);
    }
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0_usize;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = bytes
                .get(index + 1)
                .and_then(|byte| hex_value(*byte))
                .ok_or_else(|| {
                    invalid("sources", "source URL path has invalid percent encoding")
                })?;
            let low = bytes
                .get(index + 2)
                .and_then(|byte| hex_value(*byte))
                .ok_or_else(|| {
                    invalid("sources", "source URL path has invalid percent encoding")
                })?;
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).map(Some).map_err(|_| {
        invalid(
            "sources",
            "source URL path percent encoding must decode to UTF-8",
        )
    })
}

fn contains_percent_escape(value: &str) -> bool {
    value.as_bytes().windows(3).any(|window| {
        window[0] == b'%' && hex_value(window[1]).is_some() && hex_value(window[2]).is_some()
    })
}

const fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn validate_auth(auth: &AuthBinding) -> CoreResult<()> {
    match auth {
        AuthBinding::None | AuthBinding::BearerHeader => Ok(()),
        AuthBinding::HeaderApiKey { header_name } => {
            validate_auth_header("auth.header_name", header_name.as_str())
        }
    }
}

fn validate_auth_header(location: &str, header_name: &str) -> CoreResult<()> {
    let normalized = header_name.to_ascii_lowercase();
    let forbidden = matches!(
        normalized.as_str(),
        "authorization"
            | "proxy-authorization"
            | "proxy-authenticate"
            | "www-authenticate"
            | "host"
            | "connection"
            | "keep-alive"
            | "transfer-encoding"
            | "te"
            | "trailer"
            | "upgrade"
            | "content-length"
            | "content-type"
            | "cookie"
            | "set-cookie"
            | "origin"
            | "referer"
            | "forwarded"
            | "via"
            | "expect"
    ) || normalized.starts_with("proxy-")
        || normalized.starts_with("sec-")
        || normalized.starts_with("x-forwarded-");

    if forbidden {
        return Err(invalid(
            location,
            format!("the {header_name:?} header is reserved or security-sensitive"),
        ));
    }
    Ok(())
}

fn validate_endpoint(location: &str, endpoint: &EndpointSpec) -> CoreResult<()> {
    let path = endpoint.path.as_str();
    if path.len() > MAX_ENDPOINT_PATH_BYTES {
        return Err(invalid(
            format!("{location}.path"),
            format!("endpoint path exceeds the {MAX_ENDPOINT_PATH_BYTES}-byte limit"),
        ));
    }
    if path.is_empty()
        || !path.starts_with('/')
        || path.starts_with("//")
        || path.contains("//")
        || path.contains('\\')
        || path.contains('?')
        || path.contains('#')
        || path.bytes().any(|byte| byte.is_ascii_control())
        || path.split('/').any(|segment| matches!(segment, "." | ".."))
    {
        return Err(invalid(
            format!("{location}.path"),
            "endpoint must be a normalized absolute path",
        ));
    }

    let lower = path.to_ascii_lowercase();
    if lower.contains("%2e")
        || lower.contains("%2f")
        || lower.contains("%5c")
        || lower.contains("%00")
    {
        return Err(invalid(
            format!("{location}.path"),
            "endpoint contains an encoded path delimiter, dot segment, or NUL",
        ));
    }
    validate_percent_encoding(&format!("{location}.path"), path)?;
    for segment in path.split('/').filter(|segment| !segment.is_empty()) {
        let mut decoded = segment.to_owned();
        for _ in 0..=2 {
            validate_source_path_component(location, &decoded)?;
            let Some(next) = percent_decode_once(&decoded)? else {
                break;
            };
            decoded = next;
        }
        validate_source_path_component(location, &decoded)?;
        if contains_percent_escape(&decoded) {
            return Err(invalid(
                format!("{location}.path"),
                "endpoint contains excessive nested encoding",
            ));
        }
    }

    let joined = Url::parse(&format!("https://manifest.invalid{path}"))
        .map_err(|error| invalid(format!("{location}.path"), error.to_string()))?;
    if joined.path() != path || joined.query().is_some() || joined.fragment().is_some() {
        return Err(invalid(
            format!("{location}.path"),
            "endpoint changes when URL-normalized",
        ));
    }
    Ok(())
}

fn validate_percent_encoding(location: &str, value: &str) -> CoreResult<()> {
    let bytes = value.as_bytes();
    let mut index = 0_usize;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit()
            {
                return Err(invalid(
                    location,
                    "endpoint contains an invalid percent escape",
                ));
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    Ok(())
}

fn validate_parameters(parameters: &[ParameterSpec]) -> CoreResult<()> {
    if parameters.len() > MAX_PARAMETER_COUNT {
        return Err(invalid(
            "parameters",
            format!(
                "contains {} parameters; limit is {MAX_PARAMETER_COUNT}",
                parameters.len()
            ),
        ));
    }

    let mut parameter_indexes = HashMap::with_capacity(parameters.len());
    let mut mappings = HashSet::with_capacity(parameters.len());
    for (index, parameter) in parameters.iter().enumerate() {
        let location = format!("parameters[{index}]");
        validate_parameter_shape(&location, parameter)?;

        let normalized_id = parameter.id.as_str().to_ascii_lowercase();
        if parameter_indexes.insert(normalized_id, index).is_some() {
            return Err(invalid(
                format!("{location}.id"),
                "parameter IDs must be unique ignoring ASCII case",
            ));
        }

        let normalized_mapping = parameter.provider_mapping.field_name.to_ascii_lowercase();
        if !mappings.insert((parameter.provider_mapping.target, normalized_mapping)) {
            return Err(invalid(
                format!("{location}.provider_mapping"),
                "provider mappings must be unique",
            ));
        }
    }

    for (index, parameter) in parameters.iter().enumerate() {
        let location = format!("parameters[{index}]");
        if let Some(condition) = &parameter.visibility {
            let referenced_id = condition.parameter_id.as_str().to_ascii_lowercase();
            let referenced_index = parameter_indexes.get(&referenced_id).ok_or_else(|| {
                invalid(
                    format!("{location}.visibility.parameter_id"),
                    "visibility references an unknown parameter",
                )
            })?;
            if *referenced_index == index {
                return Err(invalid(
                    format!("{location}.visibility.parameter_id"),
                    "a parameter cannot control its own visibility",
                ));
            }
            validate_literal_for_parameter(
                &format!("{location}.visibility.value"),
                &condition.value,
                &parameters[*referenced_index],
            )?;
        }

        let mut conflict_targets = HashSet::with_capacity(parameter.conflicts.len());
        for (conflict_index, conflict) in parameter.conflicts.iter().enumerate() {
            let conflict_location = format!("{location}.conflicts[{conflict_index}]");
            validate_metadata_key(
                &format!("{conflict_location}.message_key"),
                &conflict.message_key,
            )?;
            let referenced_id = conflict.parameter_id.as_str().to_ascii_lowercase();
            let referenced_index = parameter_indexes.get(&referenced_id).ok_or_else(|| {
                invalid(
                    format!("{conflict_location}.parameter_id"),
                    "conflict references an unknown parameter",
                )
            })?;
            if *referenced_index == index {
                return Err(invalid(
                    format!("{conflict_location}.parameter_id"),
                    "a parameter cannot conflict with itself",
                ));
            }
            if !conflict_targets.insert((referenced_id, conflict.kind)) {
                return Err(invalid(conflict_location, "duplicate parameter conflict"));
            }
        }
    }

    Ok(())
}

fn validate_parameter_shape(location: &str, parameter: &ParameterSpec) -> CoreResult<()> {
    validate_identifier(
        &format!("{location}.id"),
        parameter.id.as_str(),
        MAX_PARAMETER_ID_BYTES,
    )?;
    validate_metadata_key(&format!("{location}.label_key"), &parameter.label_key)?;
    if let Some(description_key) = &parameter.description_key {
        validate_metadata_key(&format!("{location}.description_key"), description_key)?;
    }
    validate_parameter_mapping(
        &format!("{location}.provider_mapping"),
        parameter.provider_mapping.target,
        &parameter.provider_mapping.field_name,
    )?;

    if parameter.allowed_values.len() > MAX_PARAMETER_CHOICES {
        return Err(invalid(
            format!("{location}.allowed_values"),
            format!(
                "contains {} choices; limit is {MAX_PARAMETER_CHOICES}",
                parameter.allowed_values.len()
            ),
        ));
    }
    if parameter.value_type == ParameterType::Enum && parameter.allowed_values.is_empty() {
        return Err(invalid(
            format!("{location}.allowed_values"),
            "enum parameters require at least one allowed value",
        ));
    }

    validate_numeric_contract(location, parameter)?;

    let mut choices = HashSet::with_capacity(parameter.allowed_values.len());
    for (choice_index, choice) in parameter.allowed_values.iter().enumerate() {
        let choice_location = format!("{location}.allowed_values[{choice_index}]");
        validate_metadata_key(&format!("{choice_location}.label_key"), &choice.label_key)?;
        validate_literal_for_parameter(
            &format!("{choice_location}.value"),
            &choice.value,
            parameter,
        )?;
        let encoded = serde_json::to_string(&choice.value).map_err(|error| {
            invalid(
                format!("{choice_location}.value"),
                format!("could not serialize choice: {error}"),
            )
        })?;
        if !choices.insert(encoded) {
            return Err(invalid(choice_location, "duplicate allowed value"));
        }
    }

    Ok(())
}

fn validate_numeric_contract(location: &str, parameter: &ParameterSpec) -> CoreResult<()> {
    let numeric = matches!(
        parameter.value_type,
        ParameterType::Integer | ParameterType::Number
    );
    if !numeric
        && (parameter.minimum.is_some() || parameter.maximum.is_some() || parameter.step.is_some())
    {
        return Err(invalid(
            location,
            "minimum, maximum, and step are valid only for numeric parameters",
        ));
    }

    for (name, value) in [
        ("minimum", parameter.minimum),
        ("maximum", parameter.maximum),
        ("step", parameter.step),
    ] {
        if value.is_some_and(|number| !number.is_finite()) {
            return Err(invalid(
                format!("{location}.{name}"),
                "numeric constraints must be finite",
            ));
        }
    }
    if parameter
        .minimum
        .zip(parameter.maximum)
        .is_some_and(|(minimum, maximum)| minimum > maximum)
    {
        return Err(invalid(location, "minimum must not exceed maximum"));
    }
    if parameter.step.is_some_and(|step| step <= 0.0) {
        return Err(invalid(
            format!("{location}.step"),
            "numeric step must be greater than zero",
        ));
    }
    if parameter.value_type == ParameterType::Integer
        && [parameter.minimum, parameter.maximum, parameter.step]
            .into_iter()
            .flatten()
            .any(|number| number.fract() != 0.0)
    {
        return Err(invalid(
            location,
            "integer parameter bounds and step must be whole numbers",
        ));
    }
    Ok(())
}

fn validate_literal_for_parameter(
    location: &str,
    literal: &ParameterLiteral,
    parameter: &ParameterSpec,
) -> CoreResult<()> {
    let type_matches = matches!(
        (parameter.value_type, literal),
        (ParameterType::Boolean, ParameterLiteral::Boolean(_))
            | (ParameterType::Integer, ParameterLiteral::Integer(_))
            | (ParameterType::Number, ParameterLiteral::Number(_))
            | (ParameterType::String, ParameterLiteral::String(_))
            | (ParameterType::Enum, ParameterLiteral::Enum(_))
            | (ParameterType::StringList, ParameterLiteral::StringList(_))
            | (ParameterType::JsonSchema, ParameterLiteral::JsonSchema(_))
            | (
                ParameterType::StopSequenceList,
                ParameterLiteral::StopSequenceList(_)
            )
            | (ParameterType::ToolPolicy, ParameterLiteral::ToolPolicy(_))
    );
    if !type_matches {
        return Err(invalid(
            location,
            "literal type does not match the parameter type",
        ));
    }

    match literal {
        ParameterLiteral::Number(number) => {
            if !number.is_finite() {
                return Err(invalid(location, "numeric literals must be finite"));
            }
            validate_numeric_value(location, *number, parameter)?;
        }
        ParameterLiteral::Integer(number) => {
            validate_numeric_value(location, integer_as_f64(*number), parameter)?;
        }
        ParameterLiteral::String(value) | ParameterLiteral::Enum(value) => {
            validate_literal_string(location, value)?;
        }
        ParameterLiteral::StringList(values) | ParameterLiteral::StopSequenceList(values) => {
            if values.len() > MAX_LITERAL_LIST_ITEMS {
                return Err(invalid(
                    location,
                    format!(
                        "literal list contains {} entries; limit is {MAX_LITERAL_LIST_ITEMS}",
                        values.len()
                    ),
                ));
            }
            for (index, value) in values.iter().enumerate() {
                validate_literal_string(&format!("{location}[{index}]"), value)?;
            }
        }
        ParameterLiteral::JsonSchema(schema) => {
            validate_literal_string(location, schema)?;
            let value: Value = serde_json::from_str(schema)
                .map_err(|error| invalid(location, format!("invalid JSON schema: {error}")))?;
            let mut node_count = 0_usize;
            validate_json_shape(location, &value, 0, &mut node_count)?;
        }
        ParameterLiteral::Boolean(_) | ParameterLiteral::ToolPolicy(_) => {}
    }
    Ok(())
}

fn validate_numeric_value(location: &str, value: f64, parameter: &ParameterSpec) -> CoreResult<()> {
    if parameter.minimum.is_some_and(|minimum| value < minimum) {
        return Err(invalid(location, "numeric literal is below the minimum"));
    }
    if parameter.maximum.is_some_and(|maximum| value > maximum) {
        return Err(invalid(location, "numeric literal exceeds the maximum"));
    }
    if let Some(step) = parameter.step {
        let base = parameter.minimum.unwrap_or(0.0);
        let quotient = (value - base) / step;
        let distance = (quotient - quotient.round()).abs();
        let tolerance = quotient.abs().max(1.0) * f64::EPSILON * 16.0;
        if distance > tolerance {
            return Err(invalid(
                location,
                "numeric literal does not align with the declared step",
            ));
        }
    }
    Ok(())
}

#[allow(clippy::cast_precision_loss)]
fn integer_as_f64(value: i64) -> f64 {
    value as f64
}

fn validate_literal_string(location: &str, value: &str) -> CoreResult<()> {
    if value.len() > MAX_LITERAL_STRING_BYTES {
        return Err(invalid(
            location,
            format!("literal exceeds the {MAX_LITERAL_STRING_BYTES}-byte limit"),
        ));
    }
    if value.chars().any(|character| character == '\0') {
        return Err(invalid(location, "literal contains a NUL character"));
    }
    if looks_like_credential_literal(value) {
        return Err(invalid(
            location,
            "credential literals and credential placeholders are forbidden",
        ));
    }
    Ok(())
}

fn validate_json_shape(
    location: &str,
    value: &Value,
    depth: usize,
    node_count: &mut usize,
) -> CoreResult<()> {
    if depth > MAX_JSON_SCHEMA_DEPTH {
        return Err(invalid(
            location,
            format!("JSON schema exceeds depth {MAX_JSON_SCHEMA_DEPTH}"),
        ));
    }
    *node_count = node_count
        .checked_add(1)
        .ok_or_else(|| invalid(location, "JSON schema node count overflowed"))?;
    if *node_count > MAX_JSON_SCHEMA_NODES {
        return Err(invalid(
            location,
            format!("JSON schema exceeds {MAX_JSON_SCHEMA_NODES} nodes"),
        ));
    }
    match value {
        Value::Array(values) => {
            for child in values {
                validate_json_shape(location, child, depth + 1, node_count)?;
            }
        }
        Value::Object(values) => {
            for (key, child) in values {
                if looks_like_credential_literal(key) {
                    return Err(invalid(
                        location,
                        "JSON schema contains a credential literal",
                    ));
                }
                validate_json_shape(location, child, depth + 1, node_count)?;
            }
        }
        Value::String(value) if looks_like_credential_literal(value) => {
            return Err(invalid(
                location,
                "JSON schema contains a credential literal",
            ));
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

fn validate_parameter_mapping(
    location: &str,
    target: ProviderParameterTarget,
    field_name: &str,
) -> CoreResult<()> {
    validate_identifier(
        &format!("{location}.field_name"),
        field_name,
        MAX_PARAMETER_ID_BYTES,
    )?;
    if is_reserved_mapping(field_name) {
        return Err(invalid(
            format!("{location}.field_name"),
            "mapping targets adapter-owned request state",
        ));
    }
    if target == ProviderParameterTarget::RequestHeader {
        validate_auth_header(&format!("{location}.field_name"), field_name)?;
    }
    Ok(())
}

fn is_reserved_mapping(field_name: &str) -> bool {
    let normalized = field_name.to_ascii_lowercase();
    normalized
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|component| !component.is_empty())
        .any(|component| {
            matches!(
                component,
                "model"
                    | "models"
                    | "message"
                    | "messages"
                    | "input"
                    | "stream"
                    | "streaming"
                    | "auth"
                    | "authentication"
                    | "authorization"
                    | "credential"
                    | "credentials"
                    | "secret"
                    | "token"
                    | "apikey"
                    | "url"
                    | "uri"
                    | "endpoint"
                    | "timeout"
                    | "headers"
            )
        })
        || matches!(
            normalized.as_str(),
            "api_key"
                | "x_api_key"
                | "base_url"
                | "api_url"
                | "request_url"
                | "timeout_seconds"
                | "timeout_ms"
        )
}

fn validate_identifier(location: &str, value: &str, maximum_bytes: usize) -> CoreResult<()> {
    if value.is_empty() || value.len() > maximum_bytes {
        return Err(invalid(
            location,
            format!("identifier must contain 1 to {maximum_bytes} bytes"),
        ));
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
    {
        return Err(invalid(
            location,
            "identifier contains characters outside the safe identifier set",
        ));
    }
    Ok(())
}

fn validate_metadata_key(location: &str, value: &str) -> CoreResult<()> {
    validate_identifier(location, value, MAX_METADATA_KEY_BYTES)
}

fn is_credential_name(value: &str) -> bool {
    let normalized = value
        .trim()
        .to_ascii_lowercase()
        .replace(['-', '.', ' '], "_");
    matches!(
        normalized.as_str(),
        "api_key"
            | "apikey"
            | "access_key"
            | "access_token"
            | "auth"
            | "authorization"
            | "bearer"
            | "credential"
            | "credentials"
            | "password"
            | "private_key"
            | "secret"
            | "secret_key"
            | "signature"
            | "token"
            | "x_api_key"
    ) || normalized.ends_with("_api_key")
        || normalized.ends_with("_access_token")
        || normalized.ends_with("_secret")
        || normalized.ends_with("_token")
}

fn looks_like_credential_literal(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    if [
        "{{credential}}",
        "${credential}",
        "<credential>",
        "{{api_key}}",
        "${api_key}",
        "-----begin private key-----",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
    {
        return true;
    }
    if [
        "bearer ",
        "basic ",
        "api_key=",
        "apikey=",
        "token=",
        "secret=",
        "password=",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix))
    {
        return true;
    }
    if (lower.starts_with("sk-") || lower.starts_with("sk_")) && trimmed.len() >= 16 {
        return true;
    }
    if trimmed.starts_with("AIza") && trimmed.len() >= 24 {
        return true;
    }
    if (trimmed.starts_with("AKIA") || trimmed.starts_with("ASIA"))
        && trimmed.len() == 20
        && trimmed.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        return true;
    }
    looks_like_jwt(trimmed) || contains_credential_like_token(trimmed)
}

fn looks_like_jwt(value: &str) -> bool {
    let mut segments = value.split('.');
    let Some(header) = segments.next() else {
        return false;
    };
    let Some(payload) = segments.next() else {
        return false;
    };
    let Some(signature) = segments.next() else {
        return false;
    };
    segments.next().is_none()
        && header.len() >= 8
        && payload.len() >= 8
        && signature.len() >= 8
        && [header, payload, signature].into_iter().all(|segment| {
            segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
}

fn canonical_manifest_json(manifest: &ProviderManifest) -> CoreResult<Vec<u8>> {
    let value = serde_json::to_value(manifest).map_err(|error| {
        invalid(
            "manifest",
            format!("could not serialize manifest for hashing: {error}"),
        )
    })?;
    let mut output = Vec::new();
    write_canonical_json(&value, &mut output)?;
    Ok(output)
}

fn write_canonical_json(value: &Value, output: &mut Vec<u8>) -> CoreResult<()> {
    match value {
        Value::Null => output.extend_from_slice(b"null"),
        Value::Bool(true) => output.extend_from_slice(b"true"),
        Value::Bool(false) => output.extend_from_slice(b"false"),
        Value::Number(number) => {
            if number.as_f64().is_some_and(|value| value == 0.0) {
                output.push(b'0');
            } else {
                output.extend_from_slice(number.to_string().as_bytes());
            }
        }
        Value::String(string) => {
            serde_json::to_writer(&mut *output, string).map_err(|error| {
                invalid(
                    "manifest",
                    format!("could not canonicalize JSON string: {error}"),
                )
            })?;
        }
        Value::Array(values) => {
            output.push(b'[');
            for (index, child) in values.iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                write_canonical_json(child, output)?;
            }
            output.push(b']');
        }
        Value::Object(values) => {
            output.push(b'{');
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_unstable_by_key(|(key, _)| *key);
            for (index, (key, child)) in entries.into_iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                serde_json::to_writer(&mut *output, key).map_err(|error| {
                    invalid(
                        "manifest",
                        format!("could not canonicalize JSON key: {error}"),
                    )
                })?;
                output.push(b':');
                write_canonical_json(child, output)?;
            }
            output.push(b'}');
        }
    }
    Ok(())
}

fn invalid(location: impl AsRef<str>, reason: impl AsRef<str>) -> CoreError {
    CoreError::invalid(format!(
        "invalid provider manifest {}: {}",
        location.as_ref(),
        reason.as_ref()
    ))
}

#[cfg(test)]
mod tests {
    use lorepia_domain::{
        ApiFamily, AuthBinding, CanonicalOrigin, ConnectionFieldSpec, ConnectionFieldType,
        DecoderId, EndpointPath, EndpointSpec, HeaderName, HttpMethod, HttpUrl, ManifestDecoders,
        ManifestEndpoints, ManifestSource, ManifestSourceKind, ParameterChoice,
        ParameterDefaultMode, ParameterId, ParameterLiteral, ParameterSpec, ParameterType,
        ProviderManifest, ProviderParameterMapping, ProviderParameterTarget, UiParameterLevel,
    };
    use serde_json::Value;

    use super::{
        MAX_CONNECTION_FIELD_COUNT, MAX_SOURCE_COUNT, validate_connection_fields, validate_manifest,
    };

    fn manifest_for(family: ApiFamily) -> ProviderManifest {
        let (response, streaming) = match family {
            ApiFamily::OpenAiResponses | ApiFamily::OpenAiChatCompletions => {
                (DecoderId::OpenAiJsonV1, DecoderId::OpenAiSseV1)
            }
            ApiFamily::AnthropicMessages => (DecoderId::AnthropicJsonV1, DecoderId::AnthropicSseV1),
            ApiFamily::GeminiGenerateContent => (DecoderId::GeminiJsonV1, DecoderId::GeminiSseV1),
            ApiFamily::OllamaNative => (DecoderId::OllamaJsonV1, DecoderId::OllamaJsonlV1),
        };
        ProviderManifest {
            schema_version: 1,
            api_family: family,
            sources: vec![ManifestSource {
                kind: ManifestSourceKind::OfficialDocumentation,
                url: HttpUrl::parse("https://docs.example.test/api").expect("valid source URL"),
                content_sha256: Some("ab".repeat(32)),
            }],
            default_api_origin: Some(
                CanonicalOrigin::parse("https://api.example.test").expect("valid API origin"),
            ),
            auth: AuthBinding::BearerHeader,
            endpoints: ManifestEndpoints {
                models: Some(EndpointSpec {
                    method: HttpMethod::Get,
                    path: EndpointPath::parse("/v1/models").expect("valid models path"),
                }),
                generate: EndpointSpec {
                    method: HttpMethod::Post,
                    path: EndpointPath::parse("/v1/generate").expect("valid generate path"),
                },
                embeddings: Some(EndpointSpec {
                    method: HttpMethod::Post,
                    path: EndpointPath::parse("/v1/embeddings").expect("valid embeddings path"),
                }),
            },
            decoders: ManifestDecoders {
                response,
                streaming: Some(streaming),
            },
            parameters: vec![number_parameter("temperature", "temperature")],
        }
    }

    fn number_parameter(id: &str, field_name: &str) -> ParameterSpec {
        ParameterSpec {
            id: ParameterId::from(id),
            label_key: format!("provider.parameter.{id}"),
            description_key: Some(format!("provider.parameter.{id}.description")),
            value_type: ParameterType::Number,
            allowed_values: vec![ParameterChoice {
                value: ParameterLiteral::Number(0.5),
                label_key: "provider.parameter.choice.half".to_owned(),
            }],
            minimum: Some(0.0),
            maximum: Some(2.0),
            step: Some(0.1),
            default_mode: ParameterDefaultMode::ProviderDefault,
            visibility: None,
            conflicts: Vec::new(),
            provider_mapping: ProviderParameterMapping {
                target: ProviderParameterTarget::RequestBody,
                field_name: field_name.to_owned(),
            },
            level: UiParameterLevel::Basic,
        }
    }

    #[test]
    fn every_compiled_family_accepts_only_its_decoder_pair() {
        for family in [
            ApiFamily::OpenAiResponses,
            ApiFamily::OpenAiChatCompletions,
            ApiFamily::AnthropicMessages,
            ApiFamily::GeminiGenerateContent,
            ApiFamily::OllamaNative,
        ] {
            let manifest = manifest_for(family);
            assert!(
                validate_manifest(&manifest).is_ok(),
                "{family:?} should be supported"
            );

            let mut wrong_response = manifest.clone();
            wrong_response.decoders.response = DecoderId::OllamaJsonV1;
            if family != ApiFamily::OllamaNative {
                assert!(validate_manifest(&wrong_response).is_err());
            }

            let mut wrong_stream = manifest;
            wrong_stream.decoders.streaming = Some(DecoderId::GeminiSseV1);
            if family != ApiFamily::GeminiGenerateContent {
                assert!(validate_manifest(&wrong_stream).is_err());
            }
        }
    }

    #[test]
    fn unknown_adapter_and_decoder_are_rejected_during_typed_decode() {
        let mut manifest = serde_json::to_value(manifest_for(ApiFamily::OpenAiResponses))
            .expect("serialize manifest");
        manifest["api_family"] = Value::String("generic_json".to_owned());
        assert!(serde_json::from_value::<ProviderManifest>(manifest).is_err());

        let mut manifest = serde_json::to_value(manifest_for(ApiFamily::OpenAiResponses))
            .expect("serialize manifest");
        manifest["decoders"]["response"] = Value::String("generated_decoder".to_owned());
        assert!(serde_json::from_value::<ProviderManifest>(manifest).is_err());
    }

    #[test]
    fn schema_version_must_be_exactly_one() {
        for version in [0, 2, u32::MAX] {
            let mut manifest = manifest_for(ApiFamily::OpenAiChatCompletions);
            manifest.schema_version = version;
            assert!(validate_manifest(&manifest).is_err());
        }
    }

    #[test]
    fn remote_http_is_rejected_but_loopback_http_is_allowed() {
        let mut manifest = manifest_for(ApiFamily::OpenAiChatCompletions);
        manifest.default_api_origin =
            Some(CanonicalOrigin::parse("http://api.example.test").expect("typed origin"));
        assert!(validate_manifest(&manifest).is_err());

        for origin in [
            "http://localhost:11434",
            "http://provider.localhost:11434",
            "http://127.0.0.1:11434",
            "http://[::1]:11434",
        ] {
            manifest.default_api_origin =
                Some(CanonicalOrigin::parse(origin).expect("loopback origin"));
            assert!(
                validate_manifest(&manifest).is_ok(),
                "{origin} should be accepted"
            );
        }

        manifest.default_api_origin = Some(
            CanonicalOrigin::parse("https://sk-short-private-value.example.test")
                .expect("typed origin"),
        );
        assert!(validate_manifest(&manifest).is_err());
    }

    #[test]
    fn source_urls_are_bounded_unique_and_secret_free() {
        let mut manifest = manifest_for(ApiFamily::OpenAiChatCompletions);
        manifest.sources[0].url =
            HttpUrl::parse("https://docs.example.test/?api_key=sk-secret-value")
                .expect("typed URL");
        assert!(validate_manifest(&manifest).is_err());

        for unsafe_source in [
            "https://docs.example.test/api?lang=en",
            "https://docs.example.test/api#authentication",
            "https://docs.example.test/signed/A7b9C2d4E6f8G1h3J5k7L9m2N4p6Q8r0",
            "https://docs.example.test/token=short-value",
            "https://docs.example.test/token%3Dshort-value",
            "https://docs.example.test/sk%2Dabcdefghijklmnopqrstuv",
            "https://docs.example.test/token%253Dshort-value",
            "https://sk-A7b9C2d4E6f8G1h3J5k7L9m2N4p6Q8r0.example.test/api",
        ] {
            let mut manifest = manifest_for(ApiFamily::OpenAiChatCompletions);
            manifest.sources[0].url = HttpUrl::parse(unsafe_source).expect("typed URL");
            assert!(
                validate_manifest(&manifest).is_err(),
                "{unsafe_source} must not enter durable source provenance"
            );
        }

        let mut manifest = manifest_for(ApiFamily::OpenAiChatCompletions);
        manifest.sources.push(manifest.sources[0].clone());
        assert!(validate_manifest(&manifest).is_err());

        let mut manifest = manifest_for(ApiFamily::OpenAiChatCompletions);
        manifest.sources = (0..=MAX_SOURCE_COUNT)
            .map(|index| ManifestSource {
                kind: ManifestSourceKind::OfficialDocumentation,
                url: HttpUrl::parse(&format!("https://docs{index}.example.test/"))
                    .expect("typed URL"),
                content_sha256: None,
            })
            .collect();
        assert!(validate_manifest(&manifest).is_err());

        let mut manifest = manifest_for(ApiFamily::OpenAiChatCompletions);
        manifest.sources[0].content_sha256 = Some("AB".repeat(32));
        assert!(validate_manifest(&manifest).is_err());
    }

    #[test]
    fn endpoint_methods_and_normalization_are_fail_closed() {
        let mut manifest = manifest_for(ApiFamily::OpenAiChatCompletions);
        manifest.endpoints.generate.method = HttpMethod::Get;
        assert!(validate_manifest(&manifest).is_err());

        let mut manifest = manifest_for(ApiFamily::OpenAiChatCompletions);
        manifest
            .endpoints
            .models
            .as_mut()
            .expect("models endpoint")
            .method = HttpMethod::Post;
        assert!(validate_manifest(&manifest).is_err());

        let mut manifest = manifest_for(ApiFamily::OpenAiChatCompletions);
        manifest
            .endpoints
            .embeddings
            .as_mut()
            .expect("embeddings endpoint")
            .method = HttpMethod::Get;
        assert!(validate_manifest(&manifest).is_err());

        let mut encoded = serde_json::to_value(manifest_for(ApiFamily::OpenAiChatCompletions))
            .expect("serialize manifest");
        encoded["endpoints"]["generate"]["path"] = Value::String("/v1/%2e%2e/admin".to_owned());
        assert!(serde_json::from_value::<ProviderManifest>(encoded).is_err());

        for unsafe_path in [
            "/v1/sk-short-private-value/generate",
            "/v1/sk%252Dshort-private-value/generate",
            "/v1/token=short-private-value/generate",
        ] {
            let mut manifest = manifest_for(ApiFamily::OpenAiChatCompletions);
            manifest.endpoints.generate.path =
                EndpointPath::parse(unsafe_path).expect("typed endpoint path");
            assert!(
                validate_manifest(&manifest).is_err(),
                "{unsafe_path} must not enter a manifest"
            );
        }
    }

    #[test]
    fn custom_auth_header_uses_a_denylist() {
        let mut manifest = manifest_for(ApiFamily::AnthropicMessages);
        for header in [
            "Authorization",
            "Host",
            "Cookie",
            "Content-Length",
            "Proxy-Authorization",
            "X-Forwarded-Host",
            "Sec-Fetch-Site",
        ] {
            manifest.auth = AuthBinding::HeaderApiKey {
                header_name: HeaderName::parse(header).expect("syntactically valid header"),
            };
            assert!(
                validate_manifest(&manifest).is_err(),
                "{header} must be forbidden"
            );
        }

        manifest.auth = AuthBinding::HeaderApiKey {
            header_name: HeaderName::parse("x-api-key").expect("safe header"),
        };
        assert!(validate_manifest(&manifest).is_ok());
    }

    #[test]
    fn connection_fields_are_unique_bounded_and_credential_typed() {
        let field = ConnectionFieldSpec {
            key: "region".to_owned(),
            label_key: "provider.connection.region".to_owned(),
            description_key: None,
            value_type: ConnectionFieldType::Text,
            required: false,
        };
        assert!(validate_connection_fields(std::slice::from_ref(&field)).is_ok());

        let mut duplicate = field.clone();
        duplicate.key = "REGION".to_owned();
        assert!(validate_connection_fields(&[field.clone(), duplicate]).is_err());

        let mut plaintext_secret = field.clone();
        plaintext_secret.key = "api_key".to_owned();
        assert!(validate_connection_fields(&[plaintext_secret]).is_err());

        let credential = ConnectionFieldSpec {
            key: "api_key".to_owned(),
            label_key: "provider.connection.api_key".to_owned(),
            description_key: None,
            value_type: ConnectionFieldType::Credential,
            required: true,
        };
        assert!(validate_connection_fields(std::slice::from_ref(&credential)).is_ok());
        assert!(validate_connection_fields(&[credential.clone(), credential]).is_err());

        let too_many = vec![field; MAX_CONNECTION_FIELD_COUNT + 1];
        assert!(validate_connection_fields(&too_many).is_err());
    }

    #[test]
    fn duplicate_parameters_and_mappings_are_rejected() {
        let mut manifest = manifest_for(ApiFamily::OpenAiChatCompletions);
        let mut duplicate = manifest.parameters[0].clone();
        duplicate.id = ParameterId::from("TEMPERATURE");
        duplicate.provider_mapping.field_name = "top_p".to_owned();
        manifest.parameters.push(duplicate);
        assert!(validate_manifest(&manifest).is_err());

        let mut manifest = manifest_for(ApiFamily::OpenAiChatCompletions);
        let mut duplicate_mapping = number_parameter("top_p", "TEMPERATURE");
        duplicate_mapping.allowed_values[0].value = ParameterLiteral::Number(0.4);
        manifest.parameters.push(duplicate_mapping);
        assert!(validate_manifest(&manifest).is_err());
    }

    #[test]
    fn reserved_adapter_owned_mappings_are_rejected() {
        for field in [
            "model",
            "messages",
            "stream",
            "authorization",
            "credential",
            "api_key",
            "request_url",
            "base-url",
            "timeout_ms",
        ] {
            let mut manifest = manifest_for(ApiFamily::OpenAiChatCompletions);
            manifest.parameters[0].provider_mapping.field_name = field.to_owned();
            assert!(
                validate_manifest(&manifest).is_err(),
                "{field} must remain adapter-owned"
            );
        }
    }

    #[test]
    fn numeric_contract_rejects_nan_inversion_zero_step_and_misalignment() {
        let mut manifest = manifest_for(ApiFamily::OpenAiChatCompletions);
        manifest.parameters[0].minimum = Some(f64::NAN);
        assert!(validate_manifest(&manifest).is_err());

        let mut manifest = manifest_for(ApiFamily::OpenAiChatCompletions);
        manifest.parameters[0].minimum = Some(3.0);
        assert!(validate_manifest(&manifest).is_err());

        let mut manifest = manifest_for(ApiFamily::OpenAiChatCompletions);
        manifest.parameters[0].step = Some(0.0);
        assert!(validate_manifest(&manifest).is_err());

        let mut manifest = manifest_for(ApiFamily::OpenAiChatCompletions);
        manifest.parameters[0].allowed_values[0].value = ParameterLiteral::Number(f64::INFINITY);
        assert!(validate_manifest(&manifest).is_err());

        let mut manifest = manifest_for(ApiFamily::OpenAiChatCompletions);
        manifest.parameters[0].allowed_values[0].value = ParameterLiteral::Number(0.55);
        assert!(validate_manifest(&manifest).is_err());
    }

    #[test]
    fn parameter_literal_type_and_credential_literals_are_rejected() {
        let mut manifest = manifest_for(ApiFamily::OpenAiChatCompletions);
        manifest.parameters[0].allowed_values[0].value =
            ParameterLiteral::String("not a number".to_owned());
        assert!(validate_manifest(&manifest).is_err());

        let mut manifest = manifest_for(ApiFamily::OpenAiChatCompletions);
        manifest.parameters[0].value_type = ParameterType::String;
        manifest.parameters[0].minimum = None;
        manifest.parameters[0].maximum = None;
        manifest.parameters[0].step = None;
        manifest.parameters[0].allowed_values[0].value =
            ParameterLiteral::String("Bearer secret-value".to_owned());
        assert!(validate_manifest(&manifest).is_err());

        manifest.parameters[0].allowed_values[0].value =
            ParameterLiteral::String("A7b9C2d4E6f8G1h3J5k7L9m2N4p6Q8r0".to_owned());
        assert!(validate_manifest(&manifest).is_err());
    }

    #[test]
    fn canonical_hash_is_deterministic_and_read_only() {
        let manifest = manifest_for(ApiFamily::GeminiGenerateContent);
        let compact = serde_json::to_string(&manifest).expect("compact JSON");
        let pretty = serde_json::to_string_pretty(&manifest).expect("pretty JSON");
        let compact_manifest: ProviderManifest =
            serde_json::from_str(&compact).expect("decode compact manifest");
        let pretty_manifest: ProviderManifest =
            serde_json::from_str(&pretty).expect("decode pretty manifest");

        let first = validate_manifest(&compact_manifest).expect("valid manifest");
        let second = validate_manifest(&pretty_manifest).expect("valid manifest");
        assert_eq!(first.sha256(), second.sha256());
        assert_eq!(first.sha256().len(), 64);
        assert_eq!(first.manifest(), &manifest);

        let mut changed = manifest;
        changed.parameters[0].maximum = Some(1.0);
        let changed = validate_manifest(&changed).expect("valid changed manifest");
        assert_ne!(first.sha256(), changed.sha256());
    }
}
