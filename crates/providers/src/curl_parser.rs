//! Bounded, non-executing parser for a deliberately small cURL command subset.
//!
//! The parser never invokes a shell or `curl`. Raw input is accepted through
//! [`SecretCurlInput`], which intentionally implements neither `Debug`,
//! `Clone`, nor `Serialize`. Returned evidence contains no header values,
//! query values, or JSON scalar body values.

use std::{error::Error, fmt, mem};

use lorepia_domain::{ApiFamily, CanonicalOrigin, EndpointPath, HeaderName, HttpMethod};
use serde::Serialize;
use serde_json::Value;
use url::Url;
use zeroize::{Zeroize, Zeroizing};

use crate::discovery::contains_credential_like_token;

const MAX_INPUT_BYTES: usize = 64 * 1024;
const MAX_TOKEN_BYTES: usize = 32 * 1024;
const MAX_TOKENS: usize = 256;
const MAX_URL_BYTES: usize = 8 * 1024;
const MAX_HEADERS: usize = 64;
const MAX_HEADER_BYTES: usize = 8 * 1024;
const MAX_EXTRACTED_CREDENTIAL_BYTES: usize = 4 * 1024;
const MAX_QUERY_PARAMETERS: usize = 64;
const MAX_QUERY_NAME_BYTES: usize = 128;
const MAX_BODY_BYTES: usize = 32 * 1024;
const MAX_JSON_DEPTH: usize = 32;
const MAX_JSON_NODES: usize = 2_048;
const MAX_JSON_OBJECT_FIELDS: usize = 128;
const MAX_JSON_ARRAY_SHAPES: usize = 16;
const MAX_JSON_FIELD_NAME_BYTES: usize = 256;
const MAX_MODEL_HINT_BYTES: usize = 256;
const REDACTED_PATH_SEGMENT: &str = "__redacted__";
const REDACTED_FIELD_NAME: &str = "__redacted_field__";
const REDACTED_CREDENTIAL_PLACEHOLDER: &str = "{{credential}}";
const REDACTED_VALUE_PLACEHOLDER: &str = "{{redacted}}";

/// Raw pasted cURL input.
///
/// This type is intentionally opaque and does not implement `Debug`, `Clone`,
/// `Serialize`, or `Display`, so raw credentials cannot accidentally enter
/// routine diagnostics or persistence.
pub struct SecretCurlInput {
    value: String,
}

impl SecretCurlInput {
    pub fn new(value: String) -> Self {
        Self { value }
    }

    fn expose_to_parser(&self) -> &str {
        &self.value
    }
}

impl From<String> for SecretCurlInput {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for SecretCurlInput {
    fn from(value: &str) -> Self {
        Self::new(value.to_owned())
    }
}

impl Drop for SecretCurlInput {
    fn drop(&mut self) {
        self.value.zeroize();
    }
}

/// A credential location inferred without retaining its value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CurlAuthHint {
    BearerHeader,
    AuthorizationHeader,
    ApiKeyHeader { header_name: HeaderName },
    CookieHeader { header_name: HeaderName },
    ApiKeyQuery { parameter_name: String },
}

/// Scalar-free description of a JSON request body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JsonShape {
    Null,
    Boolean,
    Number,
    String,
    Array {
        items: Vec<JsonShape>,
        truncated: bool,
    },
    Object {
        fields: Vec<JsonFieldShape>,
    },
}

/// One JSON object field and the scalar-free shape of its value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JsonFieldShape {
    pub name: String,
    pub shape: JsonShape,
}

/// Sanitized evidence extracted from one cURL request.
///
/// Every field is safe to log or serialize. In particular, all header values,
/// all query values, and all JSON scalar body values have been discarded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ParsedCurlEvidence {
    pub method: HttpMethod,
    pub origin: CanonicalOrigin,
    pub path: EndpointPath,
    pub query_parameter_names: Vec<String>,
    pub header_names: Vec<HeaderName>,
    pub auth_hints: Vec<CurlAuthHint>,
    pub body_json_shape: Option<JsonShape>,
    pub model_hint: Option<String>,
    pub stream_hint: Option<bool>,
    pub api_family_candidates: Vec<ApiFamily>,
    pub redacted_curl: String,
}

/// Ephemeral credential bytes for immediate handoff to a platform vault.
///
/// This type intentionally implements neither `Debug`, `Clone`, `Serialize`,
/// nor `Display`. Its backing allocation is zeroized on drop.
pub struct SecretBytes {
    value: Vec<u8>,
}

impl SecretBytes {
    fn copy_from(value: &str) -> Self {
        Self {
            value: value.as_bytes().to_vec(),
        }
    }

    /// Exposes the bytes only for an immediate credential-vault handoff.
    pub fn expose_to_vault(&self) -> &[u8] {
        &self.value
    }
}

impl Drop for SecretBytes {
    fn drop(&mut self) {
        self.value.zeroize();
    }
}

/// Ephemeral result of inspecting one raw cURL command.
///
/// The raw command is consumed and never retained. The optional credential is
/// present only when inspection found exactly one distinct credential value in
/// a supported authorization header, API-key header, credential-like cookie,
/// or sensitive query parameter. This secret-bearing type intentionally
/// implements neither `Debug`, `Clone`, `Serialize`, nor `Display`.
pub struct CurlInspection {
    evidence: ParsedCurlEvidence,
    extracted_credential: Option<SecretBytes>,
}

impl CurlInspection {
    /// Return the safe, serializable evidence.
    pub fn evidence(&self) -> &ParsedCurlEvidence {
        &self.evidence
    }

    /// Borrow the extracted credential for immediate request-scoped handling.
    pub fn extracted_credential(&self) -> Option<&SecretBytes> {
        self.extracted_credential.as_ref()
    }

    /// Consume the inspection and return its safe evidence and ephemeral
    /// credential separately.
    pub fn into_parts(self) -> (ParsedCurlEvidence, Option<SecretBytes>) {
        (self.evidence, self.extracted_credential)
    }

    fn into_evidence(self) -> ParsedCurlEvidence {
        let Self {
            evidence,
            extracted_credential,
        } = self;
        drop(extracted_credential);
        evidence
    }
}

/// Stable, secret-free reason a pasted command was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurlParseErrorKind {
    InputTooLarge,
    ControlCharacter,
    ShellSyntax,
    UnterminatedQuote,
    InvalidEscape,
    TooManyTokens,
    TokenTooLarge,
    MissingCurlCommand,
    UnknownOption,
    MissingOptionValue,
    UnsupportedMethod,
    MissingUrl,
    MultipleUrls,
    InvalidUrl,
    EmbeddedUrlCredential,
    InvalidEndpointPath,
    InvalidQueryName,
    TooManyQueryParameters,
    InvalidHeader,
    TooManyHeaders,
    CredentialTooLarge,
    ConflictingCredentials,
    FileReference,
    MultipleBodies,
    BodyTooLarge,
    InvalidJsonBody,
    JsonTooDeep,
    JsonTooComplex,
}

/// A parse error that never carries raw input or a token derived from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurlParseError {
    kind: CurlParseErrorKind,
}

impl CurlParseError {
    fn new(kind: CurlParseErrorKind) -> Self {
        Self { kind }
    }

    pub fn kind(self) -> CurlParseErrorKind {
        self.kind
    }
}

impl fmt::Display for CurlParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.kind {
            CurlParseErrorKind::InputTooLarge => "cURL input exceeds the size limit",
            CurlParseErrorKind::ControlCharacter => {
                "cURL input contains a prohibited control character"
            }
            CurlParseErrorKind::ShellSyntax => "cURL input contains prohibited shell syntax",
            CurlParseErrorKind::UnterminatedQuote => "cURL input contains an unterminated quote",
            CurlParseErrorKind::InvalidEscape => "cURL input contains an invalid escape",
            CurlParseErrorKind::TooManyTokens => "cURL input contains too many arguments",
            CurlParseErrorKind::TokenTooLarge => "a cURL argument exceeds the size limit",
            CurlParseErrorKind::MissingCurlCommand => "input must begin with the curl command",
            CurlParseErrorKind::UnknownOption => "cURL input contains an unsupported option",
            CurlParseErrorKind::MissingOptionValue => "a cURL option is missing its value",
            CurlParseErrorKind::UnsupportedMethod => "only GET and POST requests are supported",
            CurlParseErrorKind::MissingUrl => "cURL input must contain exactly one URL",
            CurlParseErrorKind::MultipleUrls => "cURL input contains more than one URL",
            CurlParseErrorKind::InvalidUrl => "cURL input contains an invalid HTTP URL",
            CurlParseErrorKind::EmbeddedUrlCredential => {
                "URLs with embedded credentials are prohibited"
            }
            CurlParseErrorKind::InvalidEndpointPath => "cURL URL contains an unsafe endpoint path",
            CurlParseErrorKind::InvalidQueryName => {
                "cURL URL contains an invalid query parameter name"
            }
            CurlParseErrorKind::TooManyQueryParameters => {
                "cURL URL contains too many query parameters"
            }
            CurlParseErrorKind::InvalidHeader => "cURL input contains an invalid header",
            CurlParseErrorKind::TooManyHeaders => "cURL input contains too many headers",
            CurlParseErrorKind::CredentialTooLarge => {
                "cURL credential exceeds the one-shot handoff limit"
            }
            CurlParseErrorKind::ConflictingCredentials => {
                "cURL input contains conflicting credential values"
            }
            CurlParseErrorKind::FileReference => {
                "cURL file and standard-input references are prohibited"
            }
            CurlParseErrorKind::MultipleBodies => "cURL input contains more than one request body",
            CurlParseErrorKind::BodyTooLarge => "cURL request body exceeds the size limit",
            CurlParseErrorKind::InvalidJsonBody => "cURL request body must be one valid JSON value",
            CurlParseErrorKind::JsonTooDeep => "cURL JSON request body exceeds the nesting limit",
            CurlParseErrorKind::JsonTooComplex => {
                "cURL JSON request body exceeds the complexity limit"
            }
        })
    }
}

impl Error for CurlParseError {}

/// Parse one bounded cURL command and return only redacted evidence.
pub fn parse_curl(input: SecretCurlInput) -> Result<ParsedCurlEvidence, CurlParseError> {
    inspect_curl(input).map(CurlInspection::into_evidence)
}

/// Inspect one bounded cURL command without executing it or retaining the raw
/// command.
///
/// Tokenization and parsing happen exactly once. The caller may immediately
/// move an unambiguous extracted credential into platform credential storage,
/// while only [`ParsedCurlEvidence`] is safe to persist.
pub fn inspect_curl(input: SecretCurlInput) -> Result<CurlInspection, CurlParseError> {
    let tokens = tokenize(input.expose_to_parser())?;
    let CurlArguments {
        explicit_method,
        url_argument,
        body_argument,
        header_names,
        mut auth_hints,
        mut credential_values,
    } = parse_curl_arguments(tokens.as_slice())?;
    let sanitized_url = sanitize_url(url_argument, &mut auth_hints, &mut credential_values)?;
    let method = explicit_method.unwrap_or(if body_argument.is_some() {
        HttpMethod::Post
    } else {
        HttpMethod::Get
    });
    let sanitized_body = sanitize_body(body_argument)?;
    let api_family_candidates = infer_api_families(
        sanitized_url.path.as_str(),
        &header_names,
        sanitized_body
            .value
            .as_ref()
            .map(SecretJsonValue::expose_to_parser),
    );
    let redacted_curl = build_redacted_curl(
        method,
        &sanitized_url.redacted_url,
        &header_names,
        &auth_hints,
        sanitized_body.json_shape.as_ref(),
        sanitized_body.model_hint.as_deref(),
        sanitized_body.stream_hint,
    );

    Ok(CurlInspection {
        evidence: ParsedCurlEvidence {
            method,
            origin: sanitized_url.origin,
            path: sanitized_url.path,
            query_parameter_names: sanitized_url.query_parameter_names,
            header_names,
            auth_hints,
            body_json_shape: sanitized_body.json_shape,
            model_hint: sanitized_body.model_hint,
            stream_hint: sanitized_body.stream_hint,
            api_family_candidates,
            redacted_curl,
        },
        extracted_credential: exactly_one_distinct_credential(credential_values)?,
    })
}

struct CurlArguments<'a> {
    explicit_method: Option<HttpMethod>,
    url_argument: &'a str,
    body_argument: Option<&'a str>,
    header_names: Vec<HeaderName>,
    auth_hints: Vec<CurlAuthHint>,
    credential_values: Vec<SecretBytes>,
}

fn parse_curl_arguments(tokens: &[String]) -> Result<CurlArguments<'_>, CurlParseError> {
    if tokens.first().map(String::as_str) != Some("curl") {
        return Err(CurlParseError::new(CurlParseErrorKind::MissingCurlCommand));
    }

    let mut explicit_method = None;
    let mut url_argument = None;
    let mut body_argument = None;
    let mut header_names = Vec::new();
    let mut auth_hints = Vec::new();
    let mut credential_values = Vec::new();
    let mut index = 1;

    while index < tokens.len() {
        let token = tokens[index].as_str();

        if token == "-X" || token == "--request" {
            index += 1;
            let value = tokens
                .get(index)
                .ok_or_else(|| CurlParseError::new(CurlParseErrorKind::MissingOptionValue))?;
            set_method(&mut explicit_method, value)?;
        } else if let Some(value) = token.strip_prefix("--request=") {
            require_inline_value(value)?;
            set_method(&mut explicit_method, value)?;
        } else if let Some(value) = token.strip_prefix("-X").filter(|value| !value.is_empty()) {
            set_method(&mut explicit_method, value)?;
        } else if token == "--url" {
            index += 1;
            let value = tokens
                .get(index)
                .ok_or_else(|| CurlParseError::new(CurlParseErrorKind::MissingOptionValue))?;
            set_url(&mut url_argument, value)?;
        } else if let Some(value) = token.strip_prefix("--url=") {
            require_inline_value(value)?;
            set_url(&mut url_argument, value)?;
        } else if token == "-H" || token == "--header" {
            index += 1;
            let value = tokens
                .get(index)
                .ok_or_else(|| CurlParseError::new(CurlParseErrorKind::MissingOptionValue))?;
            parse_header(
                value,
                &mut header_names,
                &mut auth_hints,
                &mut credential_values,
            )?;
        } else if let Some(value) = token.strip_prefix("--header=") {
            require_inline_value(value)?;
            parse_header(
                value,
                &mut header_names,
                &mut auth_hints,
                &mut credential_values,
            )?;
        } else if let Some(value) = token.strip_prefix("-H").filter(|value| !value.is_empty()) {
            parse_header(
                value,
                &mut header_names,
                &mut auth_hints,
                &mut credential_values,
            )?;
        } else if is_data_option(token) {
            index += 1;
            let value = tokens
                .get(index)
                .ok_or_else(|| CurlParseError::new(CurlParseErrorKind::MissingOptionValue))?;
            set_body(&mut body_argument, value)?;
        } else if let Some(value) = inline_data_value(token) {
            require_inline_value(value)?;
            set_body(&mut body_argument, value)?;
        } else if let Some(value) = token.strip_prefix("-d").filter(|value| !value.is_empty()) {
            set_body(&mut body_argument, value)?;
        } else if token.starts_with('-') {
            return Err(CurlParseError::new(CurlParseErrorKind::UnknownOption));
        } else {
            set_url(&mut url_argument, token)?;
        }

        index += 1;
    }

    Ok(CurlArguments {
        explicit_method,
        url_argument: url_argument
            .ok_or_else(|| CurlParseError::new(CurlParseErrorKind::MissingUrl))?,
        body_argument,
        header_names,
        auth_hints,
        credential_values,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum QuoteState {
    Unquoted,
    Single,
    Double,
}

/// Owns every decoded token produced from the raw command.
///
/// Unlike a plain `Vec<String>`, this buffer wipes each token on every return
/// path, including parser errors after tokenization has completed.
struct SecretTokenBuffer {
    values: Vec<String>,
}

impl SecretTokenBuffer {
    fn new() -> Self {
        Self { values: Vec::new() }
    }

    fn as_slice(&self) -> &[String] {
        &self.values
    }

    fn len(&self) -> usize {
        self.values.len()
    }

    fn push(&mut self, value: String) {
        self.values.push(value);
    }

    fn zeroize(&mut self) {
        for value in &mut self.values {
            value.zeroize();
        }
        self.values.clear();
    }
}

impl Drop for SecretTokenBuffer {
    fn drop(&mut self) {
        self.zeroize();
    }
}

fn tokenize(input: &str) -> Result<SecretTokenBuffer, CurlParseError> {
    if input.len() > MAX_INPUT_BYTES {
        return Err(CurlParseError::new(CurlParseErrorKind::InputTooLarge));
    }
    if input
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(CurlParseError::new(CurlParseErrorKind::ControlCharacter));
    }

    let input = input.trim_matches(|character: char| character.is_ascii_whitespace());
    let mut characters = input.chars().peekable();
    let mut tokens = SecretTokenBuffer::new();
    // The in-progress token can contain the credential before it is moved
    // into `tokens`; wipe it on early tokenization errors too.
    let mut token = Zeroizing::new(String::new());
    let mut token_started = false;
    let mut state = QuoteState::Unquoted;

    while let Some(character) = characters.next() {
        match state {
            QuoteState::Unquoted => match character {
                ' ' | '\t' => finish_token(&mut tokens, &mut token, &mut token_started)?,
                '\'' => {
                    state = QuoteState::Single;
                    token_started = true;
                }
                '"' => {
                    state = QuoteState::Double;
                    token_started = true;
                }
                '\\' => {
                    let escaped = characters
                        .next()
                        .ok_or_else(|| CurlParseError::new(CurlParseErrorKind::InvalidEscape))?;
                    if escaped == '\n' {
                        continue;
                    }
                    if escaped == '\r' {
                        if characters.next() == Some('\n') {
                            continue;
                        }
                        return Err(CurlParseError::new(CurlParseErrorKind::InvalidEscape));
                    }
                    if is_shell_operator(escaped) || matches!(escaped, '$' | '`' | '#') {
                        return Err(CurlParseError::new(CurlParseErrorKind::ShellSyntax));
                    }
                    push_token_character(&mut token, escaped)?;
                    token_started = true;
                }
                '\n' | '\r' | '$' | '`' | '#' | '(' | ')' | ';' | '|' | '&' | '<' | '>' => {
                    return Err(CurlParseError::new(CurlParseErrorKind::ShellSyntax));
                }
                _ => {
                    push_token_character(&mut token, character)?;
                    token_started = true;
                }
            },
            QuoteState::Single => match character {
                '\'' => state = QuoteState::Unquoted,
                '\n' | '\r' | '\t' => {
                    return Err(CurlParseError::new(CurlParseErrorKind::ControlCharacter));
                }
                _ => push_token_character(&mut token, character)?,
            },
            QuoteState::Double => match character {
                '"' => state = QuoteState::Unquoted,
                '$' | '`' => {
                    return Err(CurlParseError::new(CurlParseErrorKind::ShellSyntax));
                }
                '\n' | '\r' | '\t' => {
                    return Err(CurlParseError::new(CurlParseErrorKind::ControlCharacter));
                }
                '\\' => {
                    let escaped = characters
                        .next()
                        .ok_or_else(|| CurlParseError::new(CurlParseErrorKind::InvalidEscape))?;
                    if escaped == '\n' {
                        continue;
                    }
                    if escaped == '\r' {
                        if characters.next() == Some('\n') {
                            continue;
                        }
                        return Err(CurlParseError::new(CurlParseErrorKind::InvalidEscape));
                    }
                    if matches!(escaped, '$' | '`') {
                        return Err(CurlParseError::new(CurlParseErrorKind::ShellSyntax));
                    }
                    if !matches!(escaped, '"' | '\\') {
                        push_token_character(&mut token, '\\')?;
                    }
                    push_token_character(&mut token, escaped)?;
                }
                _ => push_token_character(&mut token, character)?,
            },
        }
    }

    if state != QuoteState::Unquoted {
        return Err(CurlParseError::new(CurlParseErrorKind::UnterminatedQuote));
    }
    finish_token(&mut tokens, &mut token, &mut token_started)?;
    Ok(tokens)
}

fn finish_token(
    tokens: &mut SecretTokenBuffer,
    token: &mut Zeroizing<String>,
    token_started: &mut bool,
) -> Result<(), CurlParseError> {
    if !*token_started {
        return Ok(());
    }
    if tokens.len() >= MAX_TOKENS {
        return Err(CurlParseError::new(CurlParseErrorKind::TooManyTokens));
    }
    tokens.push(mem::take(&mut **token));
    *token_started = false;
    Ok(())
}

fn push_token_character(token: &mut String, character: char) -> Result<(), CurlParseError> {
    if token.len() + character.len_utf8() > MAX_TOKEN_BYTES {
        return Err(CurlParseError::new(CurlParseErrorKind::TokenTooLarge));
    }
    token.push(character);
    Ok(())
}

fn is_shell_operator(character: char) -> bool {
    matches!(character, ';' | '|' | '&' | '<' | '>' | '(' | ')')
}

fn require_inline_value(value: &str) -> Result<(), CurlParseError> {
    if value.is_empty() {
        return Err(CurlParseError::new(CurlParseErrorKind::MissingOptionValue));
    }
    Ok(())
}

fn set_method(destination: &mut Option<HttpMethod>, value: &str) -> Result<(), CurlParseError> {
    let method = if value.eq_ignore_ascii_case("GET") {
        HttpMethod::Get
    } else if value.eq_ignore_ascii_case("POST") {
        HttpMethod::Post
    } else {
        return Err(CurlParseError::new(CurlParseErrorKind::UnsupportedMethod));
    };

    if destination.replace(method).is_some() {
        return Err(CurlParseError::new(CurlParseErrorKind::UnknownOption));
    }
    Ok(())
}

fn set_url<'a>(destination: &mut Option<&'a str>, value: &'a str) -> Result<(), CurlParseError> {
    if destination.replace(value).is_some() {
        return Err(CurlParseError::new(CurlParseErrorKind::MultipleUrls));
    }
    Ok(())
}

fn is_data_option(value: &str) -> bool {
    matches!(value, "-d" | "--data" | "--data-raw" | "--data-binary")
}

fn inline_data_value(value: &str) -> Option<&str> {
    ["--data=", "--data-raw=", "--data-binary="]
        .into_iter()
        .find_map(|prefix| value.strip_prefix(prefix))
}

fn set_body<'a>(destination: &mut Option<&'a str>, value: &'a str) -> Result<(), CurlParseError> {
    if value.len() > MAX_BODY_BYTES {
        return Err(CurlParseError::new(CurlParseErrorKind::BodyTooLarge));
    }
    if value.trim_start().starts_with('@') {
        return Err(CurlParseError::new(CurlParseErrorKind::FileReference));
    }
    if destination.replace(value).is_some() {
        return Err(CurlParseError::new(CurlParseErrorKind::MultipleBodies));
    }
    Ok(())
}

fn parse_header(
    value: &str,
    header_names: &mut Vec<HeaderName>,
    auth_hints: &mut Vec<CurlAuthHint>,
    credential_values: &mut Vec<SecretBytes>,
) -> Result<(), CurlParseError> {
    if value.len() > MAX_HEADER_BYTES {
        return Err(CurlParseError::new(CurlParseErrorKind::InvalidHeader));
    }
    if value.trim_start().starts_with('@') {
        return Err(CurlParseError::new(CurlParseErrorKind::FileReference));
    }
    if header_names.len() >= MAX_HEADERS {
        return Err(CurlParseError::new(CurlParseErrorKind::TooManyHeaders));
    }

    let (raw_name, raw_value) = value
        .split_once(':')
        .ok_or_else(|| CurlParseError::new(CurlParseErrorKind::InvalidHeader))?;
    let name = HeaderName::parse(raw_name.trim())
        .map_err(|_| CurlParseError::new(CurlParseErrorKind::InvalidHeader))?;
    let lower_name = name.as_str();
    if contains_credential_like_token(lower_name) {
        return Err(CurlParseError::new(CurlParseErrorKind::InvalidHeader));
    }

    if lower_name == "authorization" || lower_name == "proxy-authorization" {
        let trimmed_value = raw_value.trim();
        let hint = if trimmed_value
            .get(..7)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("bearer "))
        {
            push_credential_value(
                credential_values,
                trimmed_value.get(7..).unwrap_or_default(),
            )?;
            CurlAuthHint::BearerHeader
        } else {
            push_credential_value(credential_values, trimmed_value)?;
            CurlAuthHint::AuthorizationHeader
        };
        push_unique(auth_hints, hint);
    } else if matches!(lower_name, "cookie" | "set-cookie") {
        extract_cookie_credential_values(raw_value, credential_values)?;
        push_unique(
            auth_hints,
            CurlAuthHint::CookieHeader {
                header_name: name.clone(),
            },
        );
    } else if is_api_key_name(lower_name) {
        push_credential_value(credential_values, raw_value)?;
        push_unique(
            auth_hints,
            CurlAuthHint::ApiKeyHeader {
                header_name: name.clone(),
            },
        );
    }

    header_names.push(name);
    Ok(())
}

fn push_credential_value(values: &mut Vec<SecretBytes>, value: &str) -> Result<(), CurlParseError> {
    let value = value.trim();
    if value.is_empty()
        || matches!(
            value,
            REDACTED_CREDENTIAL_PLACEHOLDER | REDACTED_VALUE_PLACEHOLDER
        )
    {
        return Ok(());
    }
    if value.len() > MAX_EXTRACTED_CREDENTIAL_BYTES {
        return Err(CurlParseError::new(CurlParseErrorKind::CredentialTooLarge));
    }
    values.push(SecretBytes::copy_from(value));
    Ok(())
}

fn extract_cookie_credential_values(
    header_value: &str,
    values: &mut Vec<SecretBytes>,
) -> Result<(), CurlParseError> {
    for pair in header_value.split(';') {
        let Some((name, value)) = pair.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if is_credential_cookie_name(name) {
            push_credential_value(values, strip_matching_quotes(value.trim()))?;
        }
    }
    Ok(())
}

fn is_credential_cookie_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    is_sensitive_query_name(&name)
        || is_api_key_name(&name)
        || matches!(
            name.as_str(),
            "auth" | "authorization" | "bearer" | "session" | "sessionid" | "session_id"
        )
        || name.ends_with("_token")
        || name.ends_with("-token")
}

fn strip_matching_quotes(value: &str) -> &str {
    if value.len() >= 2 {
        let first = value.as_bytes()[0];
        let last = value.as_bytes()[value.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &value[1..value.len() - 1];
        }
    }
    value
}

fn exactly_one_distinct_credential(
    values: Vec<SecretBytes>,
) -> Result<Option<SecretBytes>, CurlParseError> {
    let mut values = values.into_iter();
    let Some(first) = values.next() else {
        return Ok(None);
    };
    for value in values {
        if value.expose_to_vault() != first.expose_to_vault() {
            return Err(CurlParseError::new(
                CurlParseErrorKind::ConflictingCredentials,
            ));
        }
    }
    Ok(Some(first))
}

fn is_api_key_name(name: &str) -> bool {
    matches!(
        name,
        "api-key" | "apikey" | "x-api-key" | "x-apikey" | "x-goog-api-key" | "anthropic-api-key"
    ) || name.ends_with("-api-key")
}

fn is_sensitive_query_name(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "key"
            | "api_key"
            | "api-key"
            | "apikey"
            | "token"
            | "access_token"
            | "access-token"
            | "signature"
            | "sig"
            | "x-goog-api-key"
    )
}

fn push_unique<T: PartialEq>(values: &mut Vec<T>, value: T) {
    if !values.contains(&value) {
        values.push(value);
    }
}

struct SanitizedUrl {
    origin: CanonicalOrigin,
    path: EndpointPath,
    query_parameter_names: Vec<String>,
    redacted_url: String,
}

/// Parsed URL storage whose serialized backing string can contain query
/// credentials until sanitization finishes.
struct SecretParsedUrl {
    value: Option<Url>,
}

impl SecretParsedUrl {
    fn parse(value: &str) -> Result<Self, CurlParseError> {
        let value =
            Url::parse(value).map_err(|_| CurlParseError::new(CurlParseErrorKind::InvalidUrl))?;
        Ok(Self { value: Some(value) })
    }

    fn expose_to_parser(&self) -> &Url {
        self.value
            .as_ref()
            .expect("secret parsed URL remains available until drop")
    }

    fn zeroize(&mut self) {
        if let Some(value) = self.value.take() {
            let mut serialized = String::from(value);
            serialized.zeroize();
        }
    }
}

impl Drop for SecretParsedUrl {
    fn drop(&mut self) {
        self.zeroize();
    }
}

fn sanitize_url(
    value: &str,
    auth_hints: &mut Vec<CurlAuthHint>,
    credential_values: &mut Vec<SecretBytes>,
) -> Result<SanitizedUrl, CurlParseError> {
    if value.len() > MAX_URL_BYTES {
        return Err(CurlParseError::new(CurlParseErrorKind::InvalidUrl));
    }
    let url = SecretParsedUrl::parse(value)?;
    let url = url.expose_to_parser();
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(CurlParseError::new(CurlParseErrorKind::InvalidUrl));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(CurlParseError::new(
            CurlParseErrorKind::EmbeddedUrlCredential,
        ));
    }
    if url.host_str().is_some_and(|host| {
        host.parse::<std::net::IpAddr>().is_err()
            && host.split('.').any(contains_credential_like_token)
    }) {
        return Err(CurlParseError::new(CurlParseErrorKind::InvalidUrl));
    }

    let origin = CanonicalOrigin::parse(&url.origin().ascii_serialization())
        .map_err(|_| CurlParseError::new(CurlParseErrorKind::InvalidUrl))?;
    let path = sanitize_endpoint_path(url.path())?;
    let mut query_parameter_names = Vec::new();

    for (name, value) in url.query_pairs() {
        if query_parameter_names.len() >= MAX_QUERY_PARAMETERS {
            return Err(CurlParseError::new(
                CurlParseErrorKind::TooManyQueryParameters,
            ));
        }
        if !is_valid_query_name(&name) {
            return Err(CurlParseError::new(CurlParseErrorKind::InvalidQueryName));
        }
        let name = name.into_owned();
        // Percent-decoding may allocate a second copy of a raw query value.
        // Keep that copy under an explicit zeroizing owner even when the
        // parameter is not recognized as an authentication field.
        let value = Zeroizing::new(value.into_owned());
        if is_sensitive_query_name(&name) {
            push_credential_value(credential_values, &value)?;
            push_unique(
                auth_hints,
                CurlAuthHint::ApiKeyQuery {
                    parameter_name: name.clone(),
                },
            );
        }
        query_parameter_names.push(name);
    }

    let mut redacted_url = format!("{origin}{}", path.as_str());
    if !query_parameter_names.is_empty() {
        redacted_url.push('?');
        for (index, name) in query_parameter_names.iter().enumerate() {
            if index > 0 {
                redacted_url.push('&');
            }
            redacted_url.push_str(name);
            redacted_url.push_str("={{redacted}}");
        }
    }

    Ok(SanitizedUrl {
        origin,
        path,
        query_parameter_names,
        redacted_url,
    })
}

fn is_valid_query_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_QUERY_NAME_BYTES
        && !contains_credential_like_token(name)
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'[' | b']')
        })
}

fn sanitize_endpoint_path(value: &str) -> Result<EndpointPath, CurlParseError> {
    let mut sanitized = String::with_capacity(value.len());
    let mut redact_next = false;

    for (index, raw_segment) in value.split('/').enumerate() {
        if index > 0 {
            sanitized.push('/');
        }
        if raw_segment.is_empty() {
            continue;
        }

        let decoded = percent_decode_path_segment(raw_segment)?;
        let contains_credential =
            contains_credential_like_token(&decoded) || contains_secret_assignment(&decoded);
        if redact_next || contains_credential {
            sanitized.push_str(REDACTED_PATH_SEGMENT);
        } else {
            sanitized.push_str(raw_segment);
        }
        redact_next = is_sensitive_path_name(&decoded);
    }

    EndpointPath::parse(&sanitized)
        .map_err(|_| CurlParseError::new(CurlParseErrorKind::InvalidEndpointPath))
}

fn is_sensitive_path_name(value: &str) -> bool {
    matches!(
        normalize_name(value).as_str(),
        "authorization"
            | "credential"
            | "credentials"
            | "password"
            | "secret"
            | "signature"
            | "sig"
            | "token"
            | "accesstoken"
            | "apikey"
            | "xapikey"
            | "xgoogapikey"
    )
}

fn contains_secret_assignment(value: &str) -> bool {
    let Some(separator) = value.find(['=', ':']) else {
        return false;
    };
    let name = &value[..separator];
    let assigned = value[separator + 1..].trim();
    is_sensitive_path_name(name) || contains_credential_like_token(assigned)
}

fn percent_decode_path_segment(value: &str) -> Result<Zeroizing<String>, CurlParseError> {
    let mut decoded = Zeroizing::new(value.to_owned());
    for _ in 0..4 {
        let Some(next) = percent_decode_once(&decoded)? else {
            return Ok(decoded);
        };
        decoded = Zeroizing::new(next);
    }
    if percent_decode_once(&decoded)?.is_some() {
        return Err(CurlParseError::new(CurlParseErrorKind::InvalidEndpointPath));
    }
    Ok(decoded)
}

fn percent_decode_once(value: &str) -> Result<Option<String>, CurlParseError> {
    if !value.as_bytes().contains(&b'%') {
        return Ok(None);
    }

    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        if index + 2 >= bytes.len() {
            return Err(CurlParseError::new(CurlParseErrorKind::InvalidEndpointPath));
        }
        let high = hex_value(bytes[index + 1])
            .ok_or_else(|| CurlParseError::new(CurlParseErrorKind::InvalidEndpointPath))?;
        let low = hex_value(bytes[index + 2])
            .ok_or_else(|| CurlParseError::new(CurlParseErrorKind::InvalidEndpointPath))?;
        decoded.push((high << 4) | low);
        index += 3;
    }

    match String::from_utf8(decoded) {
        Ok(decoded) => Ok(Some(decoded)),
        Err(error) => {
            let mut decoded = error.into_bytes();
            decoded.zeroize();
            Err(CurlParseError::new(CurlParseErrorKind::InvalidEndpointPath))
        }
    }
}

const fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn normalize_name(value: &str) -> String {
    value
        .bytes()
        .filter(u8::is_ascii_alphanumeric)
        .map(|byte| byte.to_ascii_lowercase() as char)
        .collect()
}

struct SanitizedBody {
    json_shape: Option<JsonShape>,
    model_hint: Option<String>,
    stream_hint: Option<bool>,
    value: Option<SecretJsonValue>,
}

/// Owns the raw parsed JSON body until structural inference finishes.
///
/// String scalars and object keys may contain pasted credentials or private
/// prompt text. Recursively consume and zeroize those allocations before the
/// JSON tree is released.
struct SecretJsonValue {
    value: Value,
}

impl SecretJsonValue {
    fn new(value: Value) -> Self {
        Self { value }
    }

    fn expose_to_parser(&self) -> &Value {
        &self.value
    }

    fn zeroize(&mut self) {
        zeroize_json_value(mem::replace(&mut self.value, Value::Null));
    }
}

impl Drop for SecretJsonValue {
    fn drop(&mut self) {
        self.zeroize();
    }
}

fn zeroize_json_value(value: Value) {
    match value {
        Value::String(mut value) => value.zeroize(),
        Value::Array(values) => {
            for value in values {
                zeroize_json_value(value);
            }
        }
        Value::Object(values) => {
            for (mut name, value) in values {
                name.zeroize();
                zeroize_json_value(value);
            }
        }
        // Null, booleans, and the enabled serde_json number representation do
        // not own heap buffers. Replacing the root with Null above ensures the
        // raw scalar is no longer reachable through this owner.
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn sanitize_body(body: Option<&str>) -> Result<SanitizedBody, CurlParseError> {
    let Some(body) = body else {
        return Ok(SanitizedBody {
            json_shape: None,
            model_hint: None,
            stream_hint: None,
            value: None,
        });
    };
    if body.len() > MAX_BODY_BYTES {
        return Err(CurlParseError::new(CurlParseErrorKind::BodyTooLarge));
    }

    let value = SecretJsonValue::new(
        serde_json::from_str(body)
            .map_err(|_| CurlParseError::new(CurlParseErrorKind::InvalidJsonBody))?,
    );
    let model_hint = value
        .expose_to_parser()
        .as_object()
        .and_then(|object| object.get("model"))
        .and_then(Value::as_str)
        .and_then(safe_model_hint);
    let stream_hint = value
        .expose_to_parser()
        .as_object()
        .and_then(|object| object.get("stream"))
        .and_then(Value::as_bool);
    let mut node_count = 0;
    let shape = json_shape(value.expose_to_parser(), 0, &mut node_count)?;

    Ok(SanitizedBody {
        json_shape: Some(shape),
        model_hint,
        stream_hint,
        value: Some(value),
    })
}

fn safe_model_hint(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > MAX_MODEL_HINT_BYTES
        || contains_credential_like_token(value)
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/' | b'@')
        })
    {
        return None;
    }
    Some(value.to_owned())
}

fn json_shape(
    value: &Value,
    depth: usize,
    node_count: &mut usize,
) -> Result<JsonShape, CurlParseError> {
    if depth > MAX_JSON_DEPTH {
        return Err(CurlParseError::new(CurlParseErrorKind::JsonTooDeep));
    }
    *node_count += 1;
    if *node_count > MAX_JSON_NODES {
        return Err(CurlParseError::new(CurlParseErrorKind::JsonTooComplex));
    }

    match value {
        Value::Null => Ok(JsonShape::Null),
        Value::Bool(_) => Ok(JsonShape::Boolean),
        Value::Number(_) => Ok(JsonShape::Number),
        Value::String(_) => Ok(JsonShape::String),
        Value::Array(values) => {
            let mut items = Vec::with_capacity(values.len().min(MAX_JSON_ARRAY_SHAPES));
            for value in values {
                let shape = json_shape(value, depth + 1, node_count)?;
                if items.len() < MAX_JSON_ARRAY_SHAPES {
                    items.push(shape);
                }
            }
            Ok(JsonShape::Array {
                items,
                truncated: values.len() > MAX_JSON_ARRAY_SHAPES,
            })
        }
        Value::Object(object) => {
            if object.len() > MAX_JSON_OBJECT_FIELDS {
                return Err(CurlParseError::new(CurlParseErrorKind::JsonTooComplex));
            }
            let mut fields = Vec::with_capacity(object.len());
            for (name, value) in object {
                if name.len() > MAX_JSON_FIELD_NAME_BYTES || name.chars().any(char::is_control) {
                    return Err(CurlParseError::new(CurlParseErrorKind::JsonTooComplex));
                }
                let name = if contains_credential_like_token(name) {
                    REDACTED_FIELD_NAME.to_owned()
                } else {
                    name.clone()
                };
                fields.push(JsonFieldShape {
                    name,
                    shape: json_shape(value, depth + 1, node_count)?,
                });
            }
            Ok(JsonShape::Object { fields })
        }
    }
}

fn infer_api_families(
    path: &str,
    header_names: &[HeaderName],
    body: Option<&Value>,
) -> Vec<ApiFamily> {
    let path = path.to_ascii_lowercase();
    let mut candidates = Vec::new();

    if path.ends_with("/responses") {
        push_unique(&mut candidates, ApiFamily::OpenAiResponses);
    }
    if path.ends_with("/chat/completions") {
        push_unique(&mut candidates, ApiFamily::OpenAiChatCompletions);
    }
    if path.ends_with("/messages") {
        push_unique(&mut candidates, ApiFamily::AnthropicMessages);
    }
    if path.contains(":generatecontent") || path.contains(":streamgeneratecontent") {
        push_unique(&mut candidates, ApiFamily::GeminiGenerateContent);
    }
    if path.ends_with("/api/chat") || path.ends_with("/api/generate") {
        push_unique(&mut candidates, ApiFamily::OllamaNative);
    }

    if header_names
        .iter()
        .any(|name| matches!(name.as_str(), "anthropic-version" | "anthropic-beta"))
    {
        push_unique(&mut candidates, ApiFamily::AnthropicMessages);
    }
    if header_names
        .iter()
        .any(|name| name.as_str() == "x-goog-api-key")
    {
        push_unique(&mut candidates, ApiFamily::GeminiGenerateContent);
    }

    if let Some(object) = body.and_then(Value::as_object) {
        if object.contains_key("input") {
            push_unique(&mut candidates, ApiFamily::OpenAiResponses);
        }
        if object.contains_key("contents") {
            push_unique(&mut candidates, ApiFamily::GeminiGenerateContent);
        }
        if object.contains_key("messages") && candidates.is_empty() {
            push_unique(&mut candidates, ApiFamily::OpenAiChatCompletions);
            push_unique(&mut candidates, ApiFamily::AnthropicMessages);
            push_unique(&mut candidates, ApiFamily::OllamaNative);
        }
    }

    candidates
}

fn build_redacted_curl(
    method: HttpMethod,
    redacted_url: &str,
    header_names: &[HeaderName],
    auth_hints: &[CurlAuthHint],
    body_shape: Option<&JsonShape>,
    model_hint: Option<&str>,
    stream_hint: Option<bool>,
) -> String {
    let method = match method {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
    };
    let mut command = format!(
        "curl --request {} --url {}",
        json_quoted(method),
        json_quoted(redacted_url)
    );

    for name in header_names {
        let placeholder = redacted_header_value(name, auth_hints);
        command.push_str(" --header ");
        command.push_str(&json_quoted(&format!("{}: {placeholder}", name.as_str())));
    }
    if let Some(body_shape) = body_shape {
        let body = redacted_json_value(body_shape, None, model_hint, stream_hint);
        command.push_str(" --data-raw ");
        command.push_str(&json_quoted(&body.to_string()));
    }
    command
}

fn redacted_header_value(name: &HeaderName, auth_hints: &[CurlAuthHint]) -> &'static str {
    if matches!(name.as_str(), "authorization" | "proxy-authorization")
        && auth_hints.contains(&CurlAuthHint::BearerHeader)
    {
        "Bearer {{credential}}"
    } else if header_is_credential(name, auth_hints) {
        REDACTED_CREDENTIAL_PLACEHOLDER
    } else {
        REDACTED_VALUE_PLACEHOLDER
    }
}

/// Reconstructs one valid, scalar-free JSON value from the sanitized shape.
///
/// The generated cURL is intentionally safe to parse again after a native
/// credential handoff. Safe model and stream hints are retained because they
/// are already part of [`ParsedCurlEvidence`]; every other scalar is replaced
/// with a fixed non-secret value. A truncated array gets one extra sentinel so
/// reparsing preserves its `truncated` bit and first sixteen item shapes.
fn redacted_json_value(
    shape: &JsonShape,
    field_name: Option<&str>,
    model_hint: Option<&str>,
    stream_hint: Option<bool>,
) -> Value {
    match shape {
        JsonShape::Null => Value::Null,
        JsonShape::Boolean => Value::Bool(
            field_name
                .filter(|name| *name == "stream")
                .and(stream_hint)
                .unwrap_or(false),
        ),
        JsonShape::Number => Value::Number(0.into()),
        JsonShape::String => Value::String(
            field_name
                .filter(|name| *name == "model")
                .and(model_hint)
                .unwrap_or_default()
                .to_owned(),
        ),
        JsonShape::Array { items, truncated } => {
            let mut values = items
                .iter()
                .map(|item| redacted_json_value(item, None, model_hint, stream_hint))
                .collect::<Vec<_>>();
            if *truncated {
                values.push(Value::Null);
            }
            Value::Array(values)
        }
        JsonShape::Object { fields } => {
            let mut object = serde_json::Map::new();
            for field in fields {
                object.insert(
                    field.name.clone(),
                    redacted_json_value(&field.shape, Some(&field.name), model_hint, stream_hint),
                );
            }
            Value::Object(object)
        }
    }
}

fn header_is_credential(name: &HeaderName, auth_hints: &[CurlAuthHint]) -> bool {
    auth_hints.iter().any(|hint| match hint {
        CurlAuthHint::BearerHeader | CurlAuthHint::AuthorizationHeader => {
            matches!(name.as_str(), "authorization" | "proxy-authorization")
        }
        CurlAuthHint::ApiKeyHeader { header_name } | CurlAuthHint::CookieHeader { header_name } => {
            header_name == name
        }
        CurlAuthHint::ApiKeyQuery { .. } => false,
    })
}

fn json_quoted(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"{{redacted}}\"".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "curl-parser-secret-canary-9f7b";

    fn parse(command: String) -> ParsedCurlEvidence {
        parse_curl(SecretCurlInput::new(command)).expect("command should parse")
    }

    #[test]
    fn parses_multiline_openai_request_and_discards_scalar_values() {
        let evidence = parse(format!(
            "curl 'https://api.example.test/v1/chat/completions?api_key={SECRET}&api-version=2026-01-01' \\\n             -H 'Authorization: Bearer {SECRET}' \\\n             -H 'Content-Type: application/json' \\\n             --data-raw '{{\"model\":\"gpt-safe-1\",\"stream\":true,\"messages\":[{{\"role\":\"user\",\"content\":\"{SECRET}\"}}]}}'"
        ));

        assert_eq!(evidence.method, HttpMethod::Post);
        assert_eq!(evidence.origin.as_str(), "https://api.example.test");
        assert_eq!(evidence.path.as_str(), "/v1/chat/completions");
        assert_eq!(evidence.query_parameter_names, ["api_key", "api-version"]);
        assert_eq!(evidence.model_hint.as_deref(), Some("gpt-safe-1"));
        assert_eq!(evidence.stream_hint, Some(true));
        assert_eq!(
            evidence.api_family_candidates.first(),
            Some(&ApiFamily::OpenAiChatCompletions)
        );
        assert!(!format!("{evidence:?}").contains(SECRET));
        assert!(!serde_json::to_string(&evidence).unwrap().contains(SECRET));
        assert!(!evidence.redacted_curl.contains(SECRET));
        assert!(evidence.redacted_curl.contains("{{credential}}"));
        assert!(evidence.redacted_curl.contains("{{redacted}}"));
        assert!(evidence.redacted_curl.contains("--data-raw"));
        assert!(evidence.redacted_curl.contains("\\\"messages\\\""));
    }

    #[test]
    fn generated_redacted_curl_is_secret_free_and_reparseable() {
        let first = inspect_curl(SecretCurlInput::new(format!(
            "curl 'https://api.example.test/v1/chat/completions?api_key={SECRET}&api-version=2026-01-01' \
             -H 'Authorization: Bearer {SECRET}' \
             -H 'Content-Type: application/json' \
             --data-raw '{{\"model\":\"gpt-safe-1\",\"stream\":true,\"messages\":[{{\"role\":\"user\",\"content\":\"{SECRET}\"}}],\"metadata\":{{\"attempt\":3,\"enabled\":true,\"optional\":null}}}}'"
        )))
        .expect("raw command should inspect");
        let redacted = first.evidence().redacted_curl.clone();
        assert!(!redacted.contains(SECRET));

        let second =
            inspect_curl(SecretCurlInput::new(redacted)).expect("redacted command should reparse");
        assert!(second.extracted_credential().is_none());
        assert_eq!(second.evidence().method, first.evidence().method);
        assert_eq!(second.evidence().origin, first.evidence().origin);
        assert_eq!(second.evidence().path, first.evidence().path);
        assert_eq!(
            second.evidence().query_parameter_names,
            first.evidence().query_parameter_names
        );
        assert_eq!(
            second.evidence().header_names,
            first.evidence().header_names
        );
        assert_eq!(second.evidence().auth_hints, first.evidence().auth_hints);
        assert_eq!(
            second.evidence().body_json_shape,
            first.evidence().body_json_shape
        );
        assert_eq!(second.evidence().model_hint, first.evidence().model_hint);
        assert_eq!(second.evidence().stream_hint, first.evidence().stream_hint);
        assert_eq!(
            second.evidence().api_family_candidates,
            first.evidence().api_family_candidates
        );
        assert!(!format!("{:?}", second.evidence()).contains(SECRET));
    }

    #[test]
    fn raw_intermediate_owners_clear_secret_bearing_allocations() {
        let command = format!(
            "curl 'https://example.test/v1/models?api_key={SECRET}' \
             --data-raw '{{\"private\":\"{SECRET}\",\"nested\":[\"{SECRET}\"]}}'"
        );
        let mut tokens = tokenize(&command).expect("tokenize");
        assert!(tokens.values.iter().any(|value| {
            value
                .as_bytes()
                .windows(SECRET.len())
                .any(|part| part == SECRET.as_bytes())
        }));
        tokens.zeroize();
        assert!(tokens.values.is_empty());

        let mut url =
            SecretParsedUrl::parse(&format!("https://example.test/v1/models?api_key={SECRET}"))
                .expect("URL");
        assert!(
            url.expose_to_parser()
                .as_str()
                .as_bytes()
                .windows(SECRET.len())
                .any(|part| part == SECRET.as_bytes())
        );
        url.zeroize();
        assert!(url.value.is_none());

        let mut body = SecretJsonValue::new(serde_json::json!({
            "private": SECRET,
            "nested": [SECRET],
        }));
        assert!(body.expose_to_parser().to_string().contains(SECRET));
        body.zeroize();
        assert_eq!(body.expose_to_parser(), &Value::Null);
    }

    #[test]
    fn one_shot_inspection_returns_bearer_without_leaking_it() {
        let inspection = inspect_curl(SecretCurlInput::new(format!(
            "curl https://api.example.test/v1/models \
             -H 'Authorization: Bearer {SECRET}'"
        )))
        .expect("command should inspect");

        assert_eq!(
            inspection
                .extracted_credential()
                .map(SecretBytes::expose_to_vault),
            Some(SECRET.as_bytes())
        );
        assert!(!format!("{:?}", inspection.evidence()).contains(SECRET));
        assert!(
            !serde_json::to_string(inspection.evidence())
                .expect("safe evidence should serialize")
                .contains(SECRET)
        );

        let (evidence, extracted_credential) = inspection.into_parts();
        assert_eq!(
            extracted_credential
                .as_ref()
                .map(SecretBytes::expose_to_vault),
            Some(SECRET.as_bytes())
        );
        assert!(!evidence.redacted_curl.contains(SECRET));
    }

    #[test]
    fn one_shot_inspection_extracts_supported_credential_locations() {
        let commands = [
            format!("curl https://example.test/v1/models -H 'x-api-key: {SECRET}'"),
            format!("curl 'https://example.test/v1/models?api_key={SECRET}'"),
            format!(
                "curl https://example.test/v1/models -H 'Cookie: theme=dark; access_token={SECRET}'"
            ),
        ];

        for command in commands {
            let inspection =
                inspect_curl(SecretCurlInput::new(command)).expect("command should inspect");
            assert_eq!(
                inspection
                    .extracted_credential()
                    .map(SecretBytes::expose_to_vault),
                Some(SECRET.as_bytes())
            );
            assert!(!format!("{:?}", inspection.evidence()).contains(SECRET));
            assert!(
                !serde_json::to_string(inspection.evidence())
                    .expect("safe evidence should serialize")
                    .contains(SECRET)
            );
        }
    }

    #[test]
    fn one_shot_inspection_rejects_conflicting_credential_values() {
        let second_secret = "curl-parser-second-secret-canary-2c6d";
        let result = inspect_curl(SecretCurlInput::new(format!(
            "curl 'https://example.test/v1/models?api_key={SECRET}' \
             -H 'Authorization: Bearer {second_secret}'"
        )));
        let Err(error) = result else {
            panic!("conflicting credentials must be rejected");
        };

        assert_eq!(error.kind(), CurlParseErrorKind::ConflictingCredentials);
        let debug = format!("{error:?}");
        let display = error.to_string();
        for secret in [SECRET, second_secret] {
            assert!(!debug.contains(secret));
            assert!(!display.contains(secret));
        }
    }

    #[test]
    fn one_shot_inspection_accepts_one_repeated_credential() {
        let inspection = inspect_curl(SecretCurlInput::new(format!(
            "curl 'https://example.test/v1/models?api_key={SECRET}' \
             -H 'Authorization: Bearer {SECRET}'"
        )))
        .expect("one repeated credential should inspect");

        assert_eq!(
            inspection
                .extracted_credential()
                .map(SecretBytes::expose_to_vault),
            Some(SECRET.as_bytes())
        );
        let serialized =
            serde_json::to_string(inspection.evidence()).expect("safe evidence should serialize");
        assert!(!serialized.contains(SECRET));
    }

    #[test]
    fn one_shot_inspection_errors_are_stable_and_secret_free() {
        let result = inspect_curl(SecretCurlInput::new(format!(
            "curl --unsupported={SECRET} https://example.test/v1/models"
        )));
        let Err(error) = result else {
            panic!("unsupported option must fail");
        };

        assert_eq!(error.kind(), CurlParseErrorKind::UnknownOption);
        assert_eq!(
            error.to_string(),
            "cURL input contains an unsupported option"
        );
        assert!(!error.to_string().contains(SECRET));
        assert!(!format!("{error:?}").contains(SECRET));

        let oversized = "s".repeat(MAX_EXTRACTED_CREDENTIAL_BYTES + 1);
        let result = inspect_curl(SecretCurlInput::new(format!(
            "curl https://example.test/v1/models -H 'x-api-key: {oversized}'"
        )));
        let Err(error) = result else {
            panic!("oversized credential must fail");
        };
        assert_eq!(error.kind(), CurlParseErrorKind::CredentialTooLarge);
        assert!(!error.to_string().contains(&oversized));
        assert!(!format!("{error:?}").contains(&oversized));
    }

    #[test]
    fn opaque_secret_in_model_field_is_not_retained_as_a_hint() {
        let opaque = "A7b9C2d4E6f8G1h3J5k7L9m2N4p6Q8r0";
        let evidence = parse(format!(
            "curl https://example.test/v1/chat/completions --data '{{\"model\":\"{opaque}\",\"messages\":[]}}'"
        ));

        assert_eq!(evidence.model_hint, None);
        assert!(!format!("{evidence:?}").contains(opaque));
        assert!(!serde_json::to_string(&evidence).unwrap().contains(opaque));
    }

    #[test]
    fn credential_literals_in_structural_names_are_redacted_or_rejected() {
        let credential = "sk-short-private-value";
        let evidence = parse(format!(
            "curl https://api.example.test/token/{credential}/v1/models \
             --data '{{\"model\":\"{credential}\",\"{credential}\":true}}'"
        ));
        let serialized = serde_json::to_string(&evidence).unwrap();

        assert_eq!(evidence.path.as_str(), "/token/__redacted__/v1/models");
        assert_eq!(evidence.model_hint, None);
        assert!(serialized.contains(REDACTED_FIELD_NAME));
        assert!(!serialized.contains(credential));
        assert!(!format!("{evidence:?}").contains(credential));

        let host_error = parse_curl(SecretCurlInput::from(
            "curl https://sk-short-private-value.example.test/v1/models",
        ))
        .unwrap_err();
        assert_eq!(host_error.kind(), CurlParseErrorKind::InvalidUrl);

        for command in [
            "curl https://example.test/v1/models?sk-short-private-value=ignored",
            "curl https://example.test/v1/models -H 'sk-short-private-value: ignored'",
        ] {
            let error = parse_curl(SecretCurlInput::from(command)).unwrap_err();
            assert!(!format!("{error:?}").contains(credential));
        }
    }

    #[test]
    fn infers_supported_families_from_paths_and_shapes() {
        let cases = [
            (
                "https://example.test/v1/responses",
                "{\"input\":\"private\"}",
                ApiFamily::OpenAiResponses,
            ),
            (
                "https://example.test/v1/messages",
                "{\"messages\":[]}",
                ApiFamily::AnthropicMessages,
            ),
            (
                "https://example.test/v1/models/gemini:generateContent",
                "{\"contents\":[]}",
                ApiFamily::GeminiGenerateContent,
            ),
            (
                "http://127.0.0.1:11434/api/chat",
                "{\"messages\":[]}",
                ApiFamily::OllamaNative,
            ),
        ];

        for (url, body, expected) in cases {
            let evidence = parse(format!("curl --url '{url}' --data '{body}'"));
            assert!(evidence.api_family_candidates.contains(&expected));
        }
    }

    #[test]
    fn get_without_body_is_supported() {
        let evidence = parse(
            "curl --request GET --url 'https://example.test/v1/models?key=private'".to_owned(),
        );

        assert_eq!(evidence.method, HttpMethod::Get);
        assert!(evidence.body_json_shape.is_none());
        assert_eq!(
            evidence.auth_hints,
            [CurlAuthHint::ApiKeyQuery {
                parameter_name: "key".to_owned()
            }]
        );
        assert!(!evidence.redacted_curl.contains("private"));
    }

    #[test]
    fn raw_secrets_never_appear_in_errors() {
        let commands = [
            format!("curl --output {SECRET} https://example.test/v1/models"),
            format!("curl https://user:{SECRET}@example.test/v1/models"),
            format!("curl https://example.test/v1/models -H '{SECRET}'"),
            format!("curl https://example.test/v1/models --data '{{{SECRET}'"),
            format!("curl https://example.test/v1/models --data '@{SECRET}'"),
        ];

        for command in commands {
            let error = parse_curl(SecretCurlInput::new(command)).unwrap_err();
            assert!(!error.to_string().contains(SECRET));
            assert!(!format!("{error:?}").contains(SECRET));
        }
    }

    #[test]
    fn rejects_shell_syntax_without_executing_it() {
        for command in [
            "curl https://example.test; touch /tmp/nope",
            "curl $(printf https://example.test)",
            "curl `printf https://example.test`",
            "curl https://example.test > output",
            "curl https://example.test | sh",
            "curl \"$API_URL\"",
            "curl https://example.test\ncurl https://second.test",
        ] {
            let error =
                parse_curl(SecretCurlInput::from(command)).expect_err("must reject shell syntax");
            assert_eq!(error.kind(), CurlParseErrorKind::ShellSyntax);
        }
    }

    #[test]
    fn rejects_unknown_options_file_references_and_multiple_urls() {
        let cases = [
            (
                "curl --config config https://example.test",
                CurlParseErrorKind::UnknownOption,
            ),
            (
                "curl https://example.test --data-binary @payload.json",
                CurlParseErrorKind::FileReference,
            ),
            (
                "curl https://example.test -H @headers.txt",
                CurlParseErrorKind::FileReference,
            ),
            (
                "curl https://first.test https://second.test",
                CurlParseErrorKind::MultipleUrls,
            ),
        ];

        for (command, expected) in cases {
            let error = parse_curl(SecretCurlInput::from(command)).unwrap_err();
            assert_eq!(error.kind(), expected);
        }
    }

    #[test]
    fn rejects_controls_and_unbounded_inputs() {
        let nul = "curl https://example.test\0";
        assert_eq!(
            parse_curl(SecretCurlInput::from(nul)).unwrap_err().kind(),
            CurlParseErrorKind::ControlCharacter
        );

        let oversized = format!("curl https://example.test/{}", "a".repeat(MAX_INPUT_BYTES));
        assert_eq!(
            parse_curl(SecretCurlInput::new(oversized))
                .unwrap_err()
                .kind(),
            CurlParseErrorKind::InputTooLarge
        );
    }
}
