use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use reqwest::ClientBuilder;
use tokio::net::lookup_host;
use tokio::time::timeout;
use url::{Host, Url};

/// Maximum accepted input URL size, measured in UTF-8 bytes.
pub const MAX_URL_BYTES: usize = 8 * 1024;
/// Maximum canonical DNS host size.
pub const MAX_HOST_BYTES: usize = 253;
/// Maximum canonical URL path size.
pub const MAX_PATH_BYTES: usize = 2 * 1024;
/// Maximum size of one URL path segment.
pub const MAX_PATH_SEGMENT_BYTES: usize = 255;
/// Maximum number of URL path segments.
pub const MAX_PATH_SEGMENTS: usize = 128;
/// Maximum canonical query size after sensitive parameters are removed.
pub const MAX_QUERY_BYTES: usize = 2 * 1024;
/// Maximum number of DNS answers accepted for one resolution.
pub const MAX_DNS_ANSWERS: usize = 16;
/// Maximum number of redirects accepted for one discovery request.
pub const MAX_REDIRECTS: usize = 5;

/// Network boundary applied while canonicalizing and resolving a discovery URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UrlPolicyMode {
    /// Remote discovery. Only `https` URLs whose addresses are globally routable
    /// are allowed.
    Public,
    /// An explicitly selected local-provider flow. Only loopback names and
    /// addresses are allowed; plain `http` is permitted in this mode.
    LocalLoopback,
}

/// Reader-facing name for the effective network boundary.
///
/// `ApprovedLocalNetwork` is deliberately not represented by
/// [`UrlPolicyMode`]. Callers cannot opt into it with a loose enum flag: they
/// must construct an [`ApprovedLocalNetworkOrigin`] that pins one exact origin
/// to a finite set of exact private addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UrlNetworkBoundary {
    Public,
    LocalLoopback,
    ApprovedLocalNetwork,
}

/// A canonical origin scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum OriginScheme {
    Http,
    Https,
}

impl OriginScheme {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
        }
    }

    pub const fn default_port(self) -> u16 {
        match self {
            Self::Http => 80,
            Self::Https => 443,
        }
    }
}

impl fmt::Display for OriginScheme {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Exact, canonical credential and redirect origin.
///
/// The host is lower-case ASCII (including IDNA conversion), a terminal DNS
/// root dot is removed, and `port` is always the effective port.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CanonicalOrigin {
    scheme: OriginScheme,
    host: String,
    port: u16,
}

impl CanonicalOrigin {
    pub const fn scheme(&self) -> OriginScheme {
        self.scheme
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub const fn port(&self) -> u16 {
        self.port
    }

    pub const fn is_default_port(&self) -> bool {
        self.port == self.scheme.default_port()
    }

    /// Returns the exact serialized origin without a trailing slash.
    pub fn as_string(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        if self.is_default_port() {
            format!("{}://{host}", self.scheme)
        } else {
            format!("{}://{host}:{}", self.scheme, self.port)
        }
    }

    fn from_validated_url(url: &Url) -> Result<Self, UrlPolicyError> {
        let scheme = match url.scheme() {
            "http" => OriginScheme::Http,
            "https" => OriginScheme::Https,
            _ => return Err(UrlPolicyError::UnsupportedScheme),
        };
        let host = url
            .host_str()
            .ok_or(UrlPolicyError::MissingHost)?
            .to_owned();
        let port = url
            .port_or_known_default()
            .ok_or(UrlPolicyError::InvalidPort)?;
        Ok(Self { scheme, host, port })
    }
}

impl fmt::Display for CanonicalOrigin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.as_string())
    }
}

/// Explicit approval for one local-network origin.
///
/// The approval is intentionally an exact-origin plus exact-address list. It
/// cannot express a subnet, wildcard hostname, or "all private addresses".
/// This keeps a LAN exception reviewable and prevents one approval from
/// silently granting access to unrelated devices.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ApprovedLocalNetworkOrigin {
    origin: CanonicalOrigin,
    addresses: Vec<IpAddr>,
}

impl ApprovedLocalNetworkOrigin {
    /// Creates an approval for one origin and between one and
    /// [`MAX_DNS_ANSWERS`] exact RFC1918/ULA addresses.
    ///
    /// `origin` must contain only a scheme, host, and optional port. Paths,
    /// queries, fragments, user information, loopback, link-local, metadata,
    /// CGNAT, and public addresses are rejected.
    pub fn new(origin: &str, addresses: &[IpAddr]) -> Result<Self, UrlPolicyError> {
        let origin = canonicalize_approved_local_origin(origin)?;
        let addresses = validate_approved_local_addresses(addresses)?;
        if let Some(literal) = origin_host_ip(&origin) {
            let literal = normalize_ip(literal);
            if addresses.as_slice() != [literal] {
                return Err(UrlPolicyError::LanOriginAddressNotApproved);
            }
        }
        Ok(Self { origin, addresses })
    }

    pub fn origin(&self) -> &CanonicalOrigin {
        &self.origin
    }

    pub fn addresses(&self) -> &[IpAddr] {
        &self.addresses
    }

    pub fn permits_address(&self, address: IpAddr) -> bool {
        self.addresses.binary_search(&normalize_ip(address)).is_ok()
    }
}

/// A URL accepted and normalized by the discovery policy.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct CanonicalUrl {
    url: Url,
    origin: CanonicalOrigin,
    policy: UrlPolicy,
    stripped_sensitive_query_parameters: usize,
    stripped_fragment: bool,
}

impl fmt::Debug for CanonicalUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CanonicalUrl")
            .field("scheme", &self.origin.scheme())
            .field("port", &self.origin.port())
            .field("network_boundary", &self.policy.network_boundary())
            .field("path_is_root", &(self.url.path() == "/"))
            .field("has_query", &self.url.query().is_some())
            .field(
                "stripped_sensitive_query_parameters",
                &self.stripped_sensitive_query_parameters,
            )
            .field("stripped_fragment", &self.stripped_fragment)
            .finish()
    }
}

impl CanonicalUrl {
    pub fn parse(input: &str, mode: UrlPolicyMode) -> Result<Self, UrlPolicyError> {
        canonicalize_url(input, mode)
    }

    /// Parses a URL while preserving an explicit typed network policy.
    pub fn parse_with_policy(input: &str, policy: &UrlPolicy) -> Result<Self, UrlPolicyError> {
        canonicalize_url_with_policy(input, policy)
    }

    pub fn url(&self) -> &Url {
        &self.url
    }

    pub fn as_str(&self) -> &str {
        self.url.as_str()
    }

    pub fn into_url(self) -> Url {
        self.url
    }

    pub fn origin(&self) -> &CanonicalOrigin {
        &self.origin
    }

    pub const fn mode(&self) -> UrlPolicyMode {
        self.policy.mode()
    }

    pub const fn network_boundary(&self) -> UrlNetworkBoundary {
        self.policy.network_boundary()
    }

    pub fn policy(&self) -> &UrlPolicy {
        &self.policy
    }

    pub const fn stripped_sensitive_query_parameters(&self) -> usize {
        self.stripped_sensitive_query_parameters
    }

    pub const fn stripped_fragment(&self) -> bool {
        self.stripped_fragment
    }

    pub fn host_ip(&self) -> Option<IpAddr> {
        match self.url.host()? {
            Host::Ipv4(address) => Some(IpAddr::V4(address)),
            Host::Ipv6(address) => Some(IpAddr::V6(address)),
            Host::Domain(_) => None,
        }
    }

    /// Resolves a relative reference and reapplies this URL's complete policy.
    pub fn join(&self, reference: &str) -> Result<Self, UrlPolicyError> {
        let joined = self
            .url
            .join(reference)
            .map_err(|error| UrlPolicyError::InvalidUrl(error.to_string()))?;
        Self::parse_with_policy(joined.as_str(), &self.policy)
    }
}

impl fmt::Display for CanonicalUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum UrlPolicyKind {
    Public,
    LocalLoopback,
    ApprovedLocalNetwork(ApprovedLocalNetworkOrigin),
}

/// A typed network policy carried by every canonical URL.
///
/// Construction is closed over the three supported safety boundaries. The LAN
/// constructor requires an exact, validated approval object and therefore
/// cannot accidentally become an arbitrary private-network allow switch.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UrlPolicy {
    kind: UrlPolicyKind,
}

impl UrlPolicy {
    pub const fn new(mode: UrlPolicyMode) -> Self {
        let kind = match mode {
            UrlPolicyMode::Public => UrlPolicyKind::Public,
            UrlPolicyMode::LocalLoopback => UrlPolicyKind::LocalLoopback,
        };
        Self { kind }
    }

    pub const fn public() -> Self {
        Self::new(UrlPolicyMode::Public)
    }

    pub const fn local_loopback() -> Self {
        Self::new(UrlPolicyMode::LocalLoopback)
    }

    pub fn approved_local_network(approval: ApprovedLocalNetworkOrigin) -> Self {
        Self {
            kind: UrlPolicyKind::ApprovedLocalNetwork(approval),
        }
    }

    /// Compatibility projection used by document-format logic. LAN approvals
    /// share loopback's explicit `http` allowance, but all host and DNS checks
    /// still use the complete policy rather than this projection.
    pub const fn mode(&self) -> UrlPolicyMode {
        match self.kind {
            UrlPolicyKind::Public => UrlPolicyMode::Public,
            UrlPolicyKind::LocalLoopback | UrlPolicyKind::ApprovedLocalNetwork(_) => {
                UrlPolicyMode::LocalLoopback
            }
        }
    }

    pub const fn network_boundary(&self) -> UrlNetworkBoundary {
        match self.kind {
            UrlPolicyKind::Public => UrlNetworkBoundary::Public,
            UrlPolicyKind::LocalLoopback => UrlNetworkBoundary::LocalLoopback,
            UrlPolicyKind::ApprovedLocalNetwork(_) => UrlNetworkBoundary::ApprovedLocalNetwork,
        }
    }

    pub const fn is_public(&self) -> bool {
        matches!(self.kind, UrlPolicyKind::Public)
    }

    pub fn canonicalize(&self, input: &str) -> Result<CanonicalUrl, UrlPolicyError> {
        canonicalize_url_with_policy(input, self)
    }

    pub fn validate_dns(&self, answers: &[IpAddr]) -> Result<ValidatedDnsAnswers, UrlPolicyError> {
        validate_dns_answers_with_policy(self, answers)
    }

    fn approved_local_origin(&self) -> Option<&ApprovedLocalNetworkOrigin> {
        match &self.kind {
            UrlPolicyKind::ApprovedLocalNetwork(approval) => Some(approval),
            UrlPolicyKind::Public | UrlPolicyKind::LocalLoopback => None,
        }
    }
}

/// Stable IP safety category used by URL, DNS, redirect, and peer checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IpAddressClass {
    Public,
    Unspecified,
    Loopback,
    Private,
    LinkLocal,
    CarrierGradeNat,
    Multicast,
    Documentation,
    Benchmark,
    Metadata,
    Reserved,
}

impl fmt::Display for IpAddressClass {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Public => "public",
            Self::Unspecified => "unspecified",
            Self::Loopback => "loopback",
            Self::Private => "private",
            Self::LinkLocal => "link-local",
            Self::CarrierGradeNat => "carrier-grade NAT",
            Self::Multicast => "multicast",
            Self::Documentation => "documentation",
            Self::Benchmark => "benchmark",
            Self::Metadata => "metadata",
            Self::Reserved => "reserved",
        };
        formatter.write_str(value)
    }
}

/// Deterministic URL-policy failure. Error messages never include the input URL
/// or query values, so they are safe to surface without leaking credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlPolicyError {
    EmptyUrl,
    UrlTooLong,
    InvalidUrl(String),
    EmbeddedControlCharacter,
    UnsupportedScheme,
    UserInfoNotAllowed,
    MissingHost,
    InvalidHost,
    HostTooLong,
    ReservedHost,
    InvalidPort,
    PathTooLong,
    PathSegmentTooLong,
    TooManyPathSegments,
    QueryTooLong,
    DisallowedIpAddress {
        address: IpAddr,
        class: IpAddressClass,
    },
    EmptyDnsAnswers,
    TooManyDnsAnswers,
    MixedDnsAddressScopes,
    DnsAnswersChanged,
    DnsAnswerDoesNotMatchLiteral,
    PeerAddressNotValidated,
    PeerPortNotValidated,
    LanApprovalMustBeOrigin,
    LanApprovalRequiresPrivateAddresses,
    LanOriginNotApproved,
    LanOriginAddressNotApproved,
    TooManyRedirects,
}

impl fmt::Display for UrlPolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyUrl => formatter.write_str("URL is empty"),
            Self::UrlTooLong => formatter.write_str("URL exceeds the size limit"),
            Self::InvalidUrl(error) => write!(formatter, "invalid URL: {error}"),
            Self::EmbeddedControlCharacter => {
                formatter.write_str("URL contains a control character")
            }
            Self::UnsupportedScheme => {
                formatter.write_str("URL scheme is not allowed by this policy")
            }
            Self::UserInfoNotAllowed => formatter.write_str("URL user information is not allowed"),
            Self::MissingHost => formatter.write_str("URL has no host"),
            Self::InvalidHost => formatter.write_str("URL host is invalid"),
            Self::HostTooLong => formatter.write_str("URL host exceeds the size limit"),
            Self::ReservedHost => {
                formatter.write_str("URL host is local, reserved, or not publicly qualified")
            }
            Self::InvalidPort => formatter.write_str("URL port is invalid"),
            Self::PathTooLong => formatter.write_str("URL path exceeds the size limit"),
            Self::PathSegmentTooLong => {
                formatter.write_str("URL path segment exceeds the size limit")
            }
            Self::TooManyPathSegments => formatter.write_str("URL has too many path segments"),
            Self::QueryTooLong => formatter.write_str("URL query exceeds the size limit"),
            Self::DisallowedIpAddress { address, class } => {
                write!(formatter, "{class} IP address {address} is not allowed")
            }
            Self::EmptyDnsAnswers => formatter.write_str("DNS returned no addresses"),
            Self::TooManyDnsAnswers => formatter.write_str("DNS returned too many addresses"),
            Self::MixedDnsAddressScopes => {
                formatter.write_str("DNS mixed allowed and disallowed address scopes")
            }
            Self::DnsAnswersChanged => {
                formatter.write_str("DNS answers changed after policy validation")
            }
            Self::DnsAnswerDoesNotMatchLiteral => {
                formatter.write_str("address does not match the URL IP literal")
            }
            Self::PeerAddressNotValidated => {
                formatter.write_str("connected peer was not in the validated DNS answer set")
            }
            Self::PeerPortNotValidated => {
                formatter.write_str("connected peer port did not match the validated origin")
            }
            Self::LanApprovalMustBeOrigin => {
                formatter.write_str("local-network approval must be an exact origin")
            }
            Self::LanApprovalRequiresPrivateAddresses => {
                formatter.write_str("local-network approval requires exact private addresses")
            }
            Self::LanOriginNotApproved => {
                formatter.write_str("URL origin does not match the local-network approval")
            }
            Self::LanOriginAddressNotApproved => {
                formatter.write_str("address is not included in the local-network approval")
            }
            Self::TooManyRedirects => formatter.write_str("redirect limit was exceeded"),
        }
    }
}

impl std::error::Error for UrlPolicyError {}

/// Canonicalizes a provider-discovery URL and applies its pre-resolution policy.
///
/// Domain names still require [`validate_dns_answers_for_url`] after resolution,
/// followed by [`ValidatedDnsAnswers::validate_peer`] for the connected socket.
pub fn canonicalize_url(input: &str, mode: UrlPolicyMode) -> Result<CanonicalUrl, UrlPolicyError> {
    canonicalize_url_with_policy(input, &UrlPolicy::new(mode))
}

/// Canonicalizes a URL while retaining the complete typed network policy.
pub fn canonicalize_url_with_policy(
    input: &str,
    policy: &UrlPolicy,
) -> Result<CanonicalUrl, UrlPolicyError> {
    if input.is_empty() {
        return Err(UrlPolicyError::EmptyUrl);
    }
    if input.len() > MAX_URL_BYTES {
        return Err(UrlPolicyError::UrlTooLong);
    }
    if input.trim() != input || input.bytes().any(is_ascii_control) {
        return Err(UrlPolicyError::EmbeddedControlCharacter);
    }

    let mut url =
        Url::parse(input).map_err(|error| UrlPolicyError::InvalidUrl(error.to_string()))?;
    if url.cannot_be_a_base() {
        return Err(UrlPolicyError::UnsupportedScheme);
    }
    if authority_has_userinfo(input) || !url.username().is_empty() || url.password().is_some() {
        return Err(UrlPolicyError::UserInfoNotAllowed);
    }

    match (&policy.kind, url.scheme()) {
        (UrlPolicyKind::Public, "https")
        | (
            UrlPolicyKind::LocalLoopback | UrlPolicyKind::ApprovedLocalNetwork(_),
            "http" | "https",
        ) => {}
        _ => return Err(UrlPolicyError::UnsupportedScheme),
    }

    canonicalize_host(&mut url)?;
    if url.port() == Some(0) || url.port_or_known_default().is_none() {
        return Err(UrlPolicyError::InvalidPort);
    }
    validate_url_host_with_policy(&url, policy)?;

    let stripped_fragment = url.fragment().is_some();
    url.set_fragment(None);
    let stripped_sensitive_query_parameters = strip_sensitive_query_parameters(&mut url);
    validate_path_and_query(&url)?;

    let origin = CanonicalOrigin::from_validated_url(&url)?;
    Ok(CanonicalUrl {
        url,
        origin,
        policy: policy.clone(),
        stripped_sensitive_query_parameters,
        stripped_fragment,
    })
}

fn authority_has_userinfo(input: &str) -> bool {
    let Some(authority_start) = input.find("://").map(|index| index + 3) else {
        return false;
    };
    let authority_end = input[authority_start..]
        .find(['/', '?', '#'])
        .map_or(input.len(), |index| authority_start + index);
    input[authority_start..authority_end].contains('@')
}

pub fn canonicalize_public_url(input: &str) -> Result<CanonicalUrl, UrlPolicyError> {
    canonicalize_url(input, UrlPolicyMode::Public)
}

pub fn canonicalize_local_loopback_url(input: &str) -> Result<CanonicalUrl, UrlPolicyError> {
    canonicalize_url(input, UrlPolicyMode::LocalLoopback)
}

/// Returns whether a query key is treated as credential-bearing.
///
/// Matching is case-insensitive and ignores ASCII punctuation so common forms
/// such as `api_key`, `api-key`, and `ApiKey` are equivalent.
pub fn is_sensitive_query_key(key: &str) -> bool {
    let normalized = key
        .bytes()
        .filter(u8::is_ascii_alphanumeric)
        .map(|byte| byte.to_ascii_lowercase())
        .collect::<Vec<_>>();
    matches!(
        normalized.as_slice(),
        b"apikey"
            | b"xapikey"
            | b"key"
            | b"token"
            | b"accesstoken"
            | b"refreshtoken"
            | b"idtoken"
            | b"securitytoken"
            | b"xamzcredential"
            | b"xamzsignature"
            | b"xamzsecuritytoken"
            | b"xgoogcredential"
            | b"xgoogsignature"
            | b"auth"
            | b"authorization"
            | b"credential"
            | b"credentials"
            | b"secret"
            | b"clientsecret"
            | b"password"
            | b"passwd"
            | b"signature"
            | b"sig"
            | b"code"
            | b"jwt"
            | b"session"
            | b"sessionid"
            | b"ticket"
    ) || [
        b"apikey".as_slice(),
        b"token".as_slice(),
        b"credential".as_slice(),
        b"secret".as_slice(),
        b"password".as_slice(),
        b"signature".as_slice(),
        b"session".as_slice(),
        b"ticket".as_slice(),
    ]
    .iter()
    .any(|suffix| normalized.ends_with(suffix))
}

fn strip_sensitive_query_parameters(url: &mut Url) -> usize {
    let pairs = url.query_pairs().into_owned().collect::<Vec<_>>();
    let stripped = pairs
        .iter()
        .filter(|(key, _)| is_sensitive_query_key(key))
        .count();
    if stripped == 0 {
        return 0;
    }

    let retained = pairs
        .into_iter()
        .filter(|(key, _)| !is_sensitive_query_key(key))
        .collect::<Vec<_>>();
    url.set_query(None);
    if !retained.is_empty() {
        url.query_pairs_mut().extend_pairs(retained);
    }
    stripped
}

fn canonicalize_host(url: &mut Url) -> Result<(), UrlPolicyError> {
    let domain = match url.host().ok_or(UrlPolicyError::MissingHost)? {
        Host::Domain(domain) => Some(domain.to_owned()),
        Host::Ipv4(_) | Host::Ipv6(_) => None,
    };
    if let Some(domain) = domain {
        if domain.ends_with("..") {
            return Err(UrlPolicyError::InvalidHost);
        }
        if let Some(without_root_dot) = domain.strip_suffix('.') {
            if without_root_dot.is_empty() {
                return Err(UrlPolicyError::InvalidHost);
            }
            url.set_host(Some(without_root_dot))
                .map_err(|_| UrlPolicyError::InvalidHost)?;
        }
    }
    Ok(())
}

fn canonicalize_approved_local_origin(input: &str) -> Result<CanonicalOrigin, UrlPolicyError> {
    if input.is_empty() {
        return Err(UrlPolicyError::EmptyUrl);
    }
    if input.len() > MAX_URL_BYTES {
        return Err(UrlPolicyError::UrlTooLong);
    }
    if input.trim() != input || input.bytes().any(is_ascii_control) {
        return Err(UrlPolicyError::EmbeddedControlCharacter);
    }
    let mut url =
        Url::parse(input).map_err(|error| UrlPolicyError::InvalidUrl(error.to_string()))?;
    if url.cannot_be_a_base() {
        return Err(UrlPolicyError::UnsupportedScheme);
    }
    if authority_has_userinfo(input) || !url.username().is_empty() || url.password().is_some() {
        return Err(UrlPolicyError::UserInfoNotAllowed);
    }
    if !matches!(url.scheme(), "http" | "https") {
        return Err(UrlPolicyError::UnsupportedScheme);
    }
    canonicalize_host(&mut url)?;
    if url.port() == Some(0) || url.port_or_known_default().is_none() {
        return Err(UrlPolicyError::InvalidPort);
    }
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        return Err(UrlPolicyError::LanApprovalMustBeOrigin);
    }
    match url.host().ok_or(UrlPolicyError::MissingHost)? {
        Host::Domain(host) => validate_domain_syntax(host)?,
        Host::Ipv4(address) => {
            if classify_ip_address(IpAddr::V4(address)) != IpAddressClass::Private {
                return Err(UrlPolicyError::LanApprovalRequiresPrivateAddresses);
            }
        }
        Host::Ipv6(address) => {
            if classify_ip_address(IpAddr::V6(address)) != IpAddressClass::Private {
                return Err(UrlPolicyError::LanApprovalRequiresPrivateAddresses);
            }
        }
    }
    CanonicalOrigin::from_validated_url(&url)
}

fn validate_approved_local_addresses(addresses: &[IpAddr]) -> Result<Vec<IpAddr>, UrlPolicyError> {
    if addresses.is_empty() || addresses.len() > MAX_DNS_ANSWERS {
        return Err(UrlPolicyError::LanApprovalRequiresPrivateAddresses);
    }
    let mut normalized = addresses
        .iter()
        .copied()
        .map(normalize_ip)
        .collect::<Vec<_>>();
    if normalized
        .iter()
        .any(|address| classify_ip_address(*address) != IpAddressClass::Private)
    {
        return Err(UrlPolicyError::LanApprovalRequiresPrivateAddresses);
    }
    normalized.sort_unstable();
    normalized.dedup();
    Ok(normalized)
}

fn origin_host_ip(origin: &CanonicalOrigin) -> Option<IpAddr> {
    origin.host().parse().ok()
}

fn validate_url_host_with_policy(url: &Url, policy: &UrlPolicy) -> Result<(), UrlPolicyError> {
    if let Some(approval) = policy.approved_local_origin() {
        match url.host().ok_or(UrlPolicyError::MissingHost)? {
            Host::Domain(host) => validate_domain_syntax(host)?,
            Host::Ipv4(address) => {
                validate_approved_local_literal(IpAddr::V4(address), approval)?;
            }
            Host::Ipv6(address) => {
                validate_approved_local_literal(IpAddr::V6(address), approval)?;
            }
        }
        let origin = CanonicalOrigin::from_validated_url(url)?;
        return if &origin == approval.origin() {
            Ok(())
        } else {
            Err(UrlPolicyError::LanOriginNotApproved)
        };
    }

    match url.host().ok_or(UrlPolicyError::MissingHost)? {
        Host::Domain(host) => validate_domain(host, policy.mode()),
        Host::Ipv4(address) => validate_literal_ip(IpAddr::V4(address), policy.mode()),
        Host::Ipv6(address) => validate_literal_ip(IpAddr::V6(address), policy.mode()),
    }
}

fn validate_domain(host: &str, mode: UrlPolicyMode) -> Result<(), UrlPolicyError> {
    validate_domain_syntax(host)?;

    match mode {
        UrlPolicyMode::Public => {
            if !host.contains('.') || is_reserved_domain(host) {
                return Err(UrlPolicyError::ReservedHost);
            }
        }
        UrlPolicyMode::LocalLoopback => {
            if !is_loopback_domain(host) {
                return Err(UrlPolicyError::ReservedHost);
            }
        }
    }
    Ok(())
}

fn validate_domain_syntax(host: &str) -> Result<(), UrlPolicyError> {
    if host.len() > MAX_HOST_BYTES {
        return Err(UrlPolicyError::HostTooLong);
    }
    if host.is_empty()
        || !host.is_ascii()
        || host.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        return Err(UrlPolicyError::InvalidHost);
    }
    Ok(())
}

fn validate_approved_local_literal(
    address: IpAddr,
    approval: &ApprovedLocalNetworkOrigin,
) -> Result<(), UrlPolicyError> {
    let address = normalize_ip(address);
    let class = classify_ip_address(address);
    if class != IpAddressClass::Private {
        return Err(UrlPolicyError::DisallowedIpAddress { address, class });
    }
    if approval.permits_address(address) {
        Ok(())
    } else {
        Err(UrlPolicyError::LanOriginAddressNotApproved)
    }
}

fn is_loopback_domain(host: &str) -> bool {
    host == "localhost" || host.ends_with(".localhost")
}

fn is_reserved_domain(host: &str) -> bool {
    const RESERVED_SUFFIXES: [&str; 11] = [
        "localhost",
        "local",
        "localdomain",
        "internal",
        "home",
        "lan",
        "test",
        "invalid",
        "example",
        "onion",
        "arpa",
    ];
    RESERVED_SUFFIXES
        .iter()
        .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
}

fn validate_literal_ip(address: IpAddr, mode: UrlPolicyMode) -> Result<(), UrlPolicyError> {
    let class = classify_ip_address(address);
    let allowed = match mode {
        UrlPolicyMode::Public => class == IpAddressClass::Public,
        UrlPolicyMode::LocalLoopback => class == IpAddressClass::Loopback,
    };
    if allowed {
        Ok(())
    } else {
        Err(UrlPolicyError::DisallowedIpAddress { address, class })
    }
}

fn validate_path_and_query(url: &Url) -> Result<(), UrlPolicyError> {
    let path = url.path();
    if path.len() > MAX_PATH_BYTES {
        return Err(UrlPolicyError::PathTooLong);
    }
    validate_percent_encoded_controls(path)?;
    let segments = path.split('/').collect::<Vec<_>>();
    if segments.len() > MAX_PATH_SEGMENTS {
        return Err(UrlPolicyError::TooManyPathSegments);
    }
    if segments
        .iter()
        .any(|segment| segment.len() > MAX_PATH_SEGMENT_BYTES)
    {
        return Err(UrlPolicyError::PathSegmentTooLong);
    }

    if let Some(query) = url.query() {
        if query.len() > MAX_QUERY_BYTES {
            return Err(UrlPolicyError::QueryTooLong);
        }
        validate_percent_encoded_controls(query)?;
    }
    Ok(())
}

fn validate_percent_encoded_controls(value: &str) -> Result<(), UrlPolicyError> {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if is_ascii_control(bytes[index]) {
            return Err(UrlPolicyError::EmbeddedControlCharacter);
        }
        if bytes[index] == b'%' {
            let Some(high) = bytes.get(index + 1).and_then(|byte| hex_value(*byte)) else {
                return Err(UrlPolicyError::InvalidUrl(
                    "invalid percent encoding".to_owned(),
                ));
            };
            let Some(low) = bytes.get(index + 2).and_then(|byte| hex_value(*byte)) else {
                return Err(UrlPolicyError::InvalidUrl(
                    "invalid percent encoding".to_owned(),
                ));
            };
            if is_ascii_control((high << 4) | low) {
                return Err(UrlPolicyError::EmbeddedControlCharacter);
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    Ok(())
}

const fn is_ascii_control(byte: u8) -> bool {
    byte < 0x20 || byte == 0x7f
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Classifies an address without performing any network access.
///
/// IPv4-mapped IPv6 addresses are classified as their embedded IPv4 address.
/// The well-known NAT64 prefix is also checked against the embedded address so
/// it cannot be used to disguise a blocked IPv4 destination.
pub fn classify_ip_address(address: IpAddr) -> IpAddressClass {
    match address {
        IpAddr::V4(address) => classify_ipv4(address),
        IpAddr::V6(address) => classify_ipv6(address),
    }
}

pub fn is_public_ip_address(address: IpAddr) -> bool {
    classify_ip_address(address) == IpAddressClass::Public
}

fn classify_ipv4(address: Ipv4Addr) -> IpAddressClass {
    let value = u32::from(address);

    if matches!(
        address.octets(),
        [169, 254, 169, 254]
            | [169, 254, 170, 2]
            | [100, 100, 100, 200]
            | [168, 63, 129, 16]
            | [192, 0, 0, 192]
    ) {
        return IpAddressClass::Metadata;
    }
    if ipv4_in_prefix(value, 0x0000_0000, 8) {
        return IpAddressClass::Unspecified;
    }
    if ipv4_in_prefix(value, 0x7f00_0000, 8) {
        return IpAddressClass::Loopback;
    }
    if ipv4_in_prefix(value, 0x0a00_0000, 8)
        || ipv4_in_prefix(value, 0xac10_0000, 12)
        || ipv4_in_prefix(value, 0xc0a8_0000, 16)
    {
        return IpAddressClass::Private;
    }
    if ipv4_in_prefix(value, 0xa9fe_0000, 16) {
        return IpAddressClass::LinkLocal;
    }
    if ipv4_in_prefix(value, 0x6440_0000, 10) {
        return IpAddressClass::CarrierGradeNat;
    }
    if ipv4_in_prefix(value, 0xe000_0000, 4) {
        return IpAddressClass::Multicast;
    }
    if ipv4_in_prefix(value, 0xc000_0200, 24)
        || ipv4_in_prefix(value, 0xc633_6400, 24)
        || ipv4_in_prefix(value, 0xcb00_7100, 24)
    {
        return IpAddressClass::Documentation;
    }
    if ipv4_in_prefix(value, 0xc612_0000, 15) {
        return IpAddressClass::Benchmark;
    }
    if ipv4_in_prefix(value, 0xc000_0000, 24)
        || ipv4_in_prefix(value, 0xc058_6300, 24)
        || ipv4_in_prefix(value, 0xf000_0000, 4)
    {
        return IpAddressClass::Reserved;
    }
    IpAddressClass::Public
}

fn classify_ipv6(address: Ipv6Addr) -> IpAddressClass {
    if let Some(mapped) = ipv4_mapped(address) {
        return classify_ipv4(mapped);
    }
    if address.is_unspecified() {
        return IpAddressClass::Unspecified;
    }
    if address.is_loopback() {
        return IpAddressClass::Loopback;
    }

    let value = u128::from(address);
    if ipv6_in_prefix(value, ipv6_value([0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0]), 64)
        && address.segments()[7] == 0x0254
    {
        return IpAddressClass::Metadata;
    }
    if ipv6_in_prefix(value, ipv6_value([0xfc00, 0, 0, 0, 0, 0, 0, 0]), 7) {
        return IpAddressClass::Private;
    }
    if ipv6_in_prefix(value, ipv6_value([0xfe80, 0, 0, 0, 0, 0, 0, 0]), 10) {
        return IpAddressClass::LinkLocal;
    }
    if ipv6_in_prefix(value, ipv6_value([0xff00, 0, 0, 0, 0, 0, 0, 0]), 8) {
        return IpAddressClass::Multicast;
    }
    if ipv6_in_prefix(value, ipv6_value([0x2001, 0x0db8, 0, 0, 0, 0, 0, 0]), 32)
        || ipv6_in_prefix(value, ipv6_value([0x3fff, 0, 0, 0, 0, 0, 0, 0]), 20)
    {
        return IpAddressClass::Documentation;
    }
    if ipv6_in_prefix(value, ipv6_value([0x2001, 0x0002, 0, 0, 0, 0, 0, 0]), 48) {
        return IpAddressClass::Benchmark;
    }

    if let Some(translated) = well_known_nat64_ipv4(address) {
        return classify_ipv4(translated);
    }

    if ipv6_in_prefix(value, ipv6_value([0x2001, 0, 0, 0, 0, 0, 0, 0]), 23)
        && !is_globally_reachable_ietf_assignment(value)
    {
        return IpAddressClass::Reserved;
    }

    if ipv6_in_prefix(value, ipv6_value([0x0100, 0, 0, 0, 0, 0, 0, 0]), 64)
        || ipv6_in_prefix(value, ipv6_value([0x0064, 0xff9b, 1, 0, 0, 0, 0, 0]), 48)
        || ipv6_in_prefix(value, ipv6_value([0x2002, 0, 0, 0, 0, 0, 0, 0]), 16)
        || ipv6_in_prefix(value, ipv6_value([0x5f00, 0, 0, 0, 0, 0, 0, 0]), 16)
        || ipv6_in_prefix(value, ipv6_value([0xfec0, 0, 0, 0, 0, 0, 0, 0]), 10)
        || !ipv6_in_prefix(value, ipv6_value([0x2000, 0, 0, 0, 0, 0, 0, 0]), 3)
    {
        return IpAddressClass::Reserved;
    }
    IpAddressClass::Public
}

fn is_globally_reachable_ietf_assignment(address: u128) -> bool {
    [
        ipv6_value([0x2001, 1, 0, 0, 0, 0, 0, 1]),
        ipv6_value([0x2001, 1, 0, 0, 0, 0, 0, 2]),
        ipv6_value([0x2001, 1, 0, 0, 0, 0, 0, 3]),
    ]
    .contains(&address)
        || ipv6_in_prefix(address, ipv6_value([0x2001, 3, 0, 0, 0, 0, 0, 0]), 32)
        || ipv6_in_prefix(address, ipv6_value([0x2001, 4, 0x0112, 0, 0, 0, 0, 0]), 48)
        || ipv6_in_prefix(address, ipv6_value([0x2001, 0x0020, 0, 0, 0, 0, 0, 0]), 28)
        || ipv6_in_prefix(address, ipv6_value([0x2001, 0x0030, 0, 0, 0, 0, 0, 0]), 28)
}

const fn ipv4_in_prefix(address: u32, network: u32, prefix: u32) -> bool {
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    address & mask == network & mask
}

const fn ipv6_in_prefix(address: u128, network: u128, prefix: u32) -> bool {
    let mask = if prefix == 0 {
        0
    } else {
        u128::MAX << (128 - prefix)
    };
    address & mask == network & mask
}

const fn ipv6_value(segments: [u16; 8]) -> u128 {
    ((segments[0] as u128) << 112)
        | ((segments[1] as u128) << 96)
        | ((segments[2] as u128) << 80)
        | ((segments[3] as u128) << 64)
        | ((segments[4] as u128) << 48)
        | ((segments[5] as u128) << 32)
        | ((segments[6] as u128) << 16)
        | segments[7] as u128
}

fn ipv4_mapped(address: Ipv6Addr) -> Option<Ipv4Addr> {
    let segments = address.segments();
    if segments[..5] == [0; 5] && segments[5] == 0xffff {
        Some(ipv4_from_tail(segments[6], segments[7]))
    } else {
        None
    }
}

fn well_known_nat64_ipv4(address: Ipv6Addr) -> Option<Ipv4Addr> {
    let segments = address.segments();
    if segments[..6] == [0x0064, 0xff9b, 0, 0, 0, 0] {
        Some(ipv4_from_tail(segments[6], segments[7]))
    } else {
        None
    }
}

fn ipv4_from_tail(high: u16, low: u16) -> Ipv4Addr {
    let [first, second] = high.to_be_bytes();
    let [third, fourth] = low.to_be_bytes();
    Ipv4Addr::new(first, second, third, fourth)
}

fn normalize_ip(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(address) => ipv4_mapped(address).map_or(IpAddr::V6(address), IpAddr::V4),
        IpAddr::V4(address) => IpAddr::V4(address),
    }
}

/// A bounded, canonical DNS answer set that can be pinned to a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedDnsAnswers {
    policy: UrlPolicy,
    addresses: Vec<IpAddr>,
}

impl ValidatedDnsAnswers {
    pub const fn mode(&self) -> UrlPolicyMode {
        self.policy.mode()
    }

    pub fn policy(&self) -> &UrlPolicy {
        &self.policy
    }

    pub fn addresses(&self) -> &[IpAddr] {
        &self.addresses
    }

    pub fn contains(&self, address: IpAddr) -> bool {
        self.addresses.binary_search(&normalize_ip(address)).is_ok()
    }

    /// Revalidates a fresh lookup and rejects any answer-set change.
    pub fn revalidate(&self, answers: &[IpAddr]) -> Result<(), UrlPolicyError> {
        let current = validate_dns_answers_with_policy(&self.policy, answers)?;
        if current.addresses == self.addresses {
            Ok(())
        } else {
            Err(UrlPolicyError::DnsAnswersChanged)
        }
    }

    /// Checks the actual connected socket peer against the pinned answer set.
    pub fn validate_peer(&self, peer: IpAddr) -> Result<(), UrlPolicyError> {
        validate_address_with_policy(peer, &self.policy)?;
        if self.contains(peer) {
            Ok(())
        } else {
            Err(UrlPolicyError::PeerAddressNotValidated)
        }
    }
}

/// Safe, bounded DNS resolution failure. Hostnames and resolver details are
/// intentionally omitted from display text so the error can be logged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkResolutionError {
    InvalidTimeout,
    LookupTimedOut,
    LookupFailed,
    UrlPolicy(UrlPolicyError),
}

impl fmt::Display for NetworkResolutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTimeout => formatter.write_str("network resolution timeout is invalid"),
            Self::LookupTimedOut => formatter.write_str("DNS lookup timed out"),
            Self::LookupFailed => formatter.write_str("DNS lookup failed"),
            Self::UrlPolicy(error) => write!(formatter, "network policy rejected target: {error}"),
        }
    }
}

impl std::error::Error for NetworkResolutionError {}

impl From<UrlPolicyError> for NetworkResolutionError {
    fn from(error: UrlPolicyError) -> Self {
        Self::UrlPolicy(error)
    }
}

/// A URL whose DNS answers have been policy-validated and can be pinned into a
/// fresh reqwest client.
///
/// Safe request order:
///
/// 1. [`Self::resolve`] the canonical URL.
/// 2. Apply [`Self::pin_reqwest_builder`] to a fresh client with proxies and
///    automatic redirects disabled.
/// 3. Call [`Self::revalidate_dns`] immediately before sending.
/// 4. After headers arrive, call [`Self::validate_peer`] with
///    `Response::remote_addr`.
///
/// Pinning means a resolver change cannot redirect the actual connection;
/// revalidation and peer validation make the failure explicit and auditable.
#[derive(Debug, Clone)]
pub struct ResolvedNetworkTarget {
    url: CanonicalUrl,
    socket_addresses: Vec<SocketAddr>,
    validated_dns: ValidatedDnsAnswers,
}

impl ResolvedNetworkTarget {
    pub async fn resolve(
        url: &CanonicalUrl,
        lookup_timeout: Duration,
    ) -> Result<Self, NetworkResolutionError> {
        if lookup_timeout.is_zero() {
            return Err(NetworkResolutionError::InvalidTimeout);
        }
        let socket_addresses = resolve_socket_addresses(url, lookup_timeout).await?;
        let answers = socket_addresses
            .iter()
            .map(SocketAddr::ip)
            .collect::<Vec<_>>();
        let validated_dns = validate_dns_answers_for_url(url, &answers)?;
        Ok(Self {
            url: url.clone(),
            socket_addresses,
            validated_dns,
        })
    }

    pub fn url(&self) -> &CanonicalUrl {
        &self.url
    }

    pub fn socket_addresses(&self) -> &[SocketAddr] {
        &self.socket_addresses
    }

    pub fn validated_dns(&self) -> &ValidatedDnsAnswers {
        &self.validated_dns
    }

    /// Pins the validated address set for this exact hostname. IP-literal URLs
    /// need no resolver override and return the builder unchanged.
    pub fn pin_reqwest_builder(&self, builder: ClientBuilder) -> ClientBuilder {
        match self.url.url().host() {
            Some(Host::Domain(host)) => builder.resolve_to_addrs(host, &self.socket_addresses),
            Some(Host::Ipv4(_) | Host::Ipv6(_)) | None => builder,
        }
    }

    /// Performs a fresh OS lookup and requires the complete answer set to be
    /// unchanged from the pinned set.
    pub async fn revalidate_dns(
        &self,
        lookup_timeout: Duration,
    ) -> Result<(), NetworkResolutionError> {
        if lookup_timeout.is_zero() {
            return Err(NetworkResolutionError::InvalidTimeout);
        }
        let current = resolve_socket_addresses(&self.url, lookup_timeout).await?;
        let answers = current.iter().map(SocketAddr::ip).collect::<Vec<_>>();
        self.validated_dns.revalidate(&answers)?;
        Ok(())
    }

    /// Validates both the actual peer IP and destination port.
    pub fn validate_peer(&self, peer: SocketAddr) -> Result<(), UrlPolicyError> {
        if peer.port() != self.url.origin().port() {
            return Err(UrlPolicyError::PeerPortNotValidated);
        }
        self.validated_dns.validate_peer(peer.ip())
    }
}

async fn resolve_socket_addresses(
    url: &CanonicalUrl,
    lookup_timeout: Duration,
) -> Result<Vec<SocketAddr>, NetworkResolutionError> {
    let port = url
        .url()
        .port_or_known_default()
        .ok_or(UrlPolicyError::InvalidPort)?;
    match url.url().host().ok_or(UrlPolicyError::MissingHost)? {
        Host::Domain(host) => {
            let resolved = timeout(lookup_timeout, lookup_host((host, port)))
                .await
                .map_err(|_| NetworkResolutionError::LookupTimedOut)?
                .map_err(|_| NetworkResolutionError::LookupFailed)?;
            let mut addresses = resolved.take(MAX_DNS_ANSWERS + 1).collect::<Vec<_>>();
            addresses.sort_unstable();
            addresses.dedup();
            if addresses.len() > MAX_DNS_ANSWERS {
                return Err(NetworkResolutionError::UrlPolicy(
                    UrlPolicyError::TooManyDnsAnswers,
                ));
            }
            Ok(addresses)
        }
        Host::Ipv4(address) => Ok(vec![SocketAddr::new(IpAddr::V4(address), port)]),
        Host::Ipv6(address) => Ok(vec![SocketAddr::new(IpAddr::V6(address), port)]),
    }
}

/// Validates one DNS lookup. Mixed A/AAAA families are allowed; mixing allowed
/// and disallowed network scopes is rejected.
pub fn validate_dns_answers(
    mode: UrlPolicyMode,
    answers: &[IpAddr],
) -> Result<ValidatedDnsAnswers, UrlPolicyError> {
    validate_dns_answers_with_policy(&UrlPolicy::new(mode), answers)
}

/// Validates one DNS answer set against the complete typed policy.
pub fn validate_dns_answers_with_policy(
    policy: &UrlPolicy,
    answers: &[IpAddr],
) -> Result<ValidatedDnsAnswers, UrlPolicyError> {
    if answers.is_empty() {
        return Err(UrlPolicyError::EmptyDnsAnswers);
    }
    if answers.len() > MAX_DNS_ANSWERS {
        return Err(UrlPolicyError::TooManyDnsAnswers);
    }

    let mut normalized = Vec::with_capacity(answers.len());
    let mut first_disallowed = None;
    let mut has_allowed = false;
    let mut has_disallowed = false;
    for original in answers {
        let address = normalize_ip(*original);
        match validate_address_with_policy(address, policy) {
            Ok(()) => has_allowed = true,
            Err(error) => {
                has_disallowed = true;
                first_disallowed.get_or_insert(error);
            }
        }
        normalized.push(address);
    }
    if has_allowed && has_disallowed {
        return Err(UrlPolicyError::MixedDnsAddressScopes);
    }
    if let Some(error) = first_disallowed {
        return Err(error);
    }

    normalized.sort_unstable();
    normalized.dedup();
    Ok(ValidatedDnsAnswers {
        policy: policy.clone(),
        addresses: normalized,
    })
}

fn validate_address_with_policy(address: IpAddr, policy: &UrlPolicy) -> Result<(), UrlPolicyError> {
    let address = normalize_ip(address);
    let class = classify_ip_address(address);
    match &policy.kind {
        UrlPolicyKind::Public if class == IpAddressClass::Public => Ok(()),
        UrlPolicyKind::LocalLoopback if class == IpAddressClass::Loopback => Ok(()),
        UrlPolicyKind::ApprovedLocalNetwork(_) if class != IpAddressClass::Private => {
            Err(UrlPolicyError::DisallowedIpAddress { address, class })
        }
        UrlPolicyKind::ApprovedLocalNetwork(approval) if approval.permits_address(address) => {
            Ok(())
        }
        UrlPolicyKind::ApprovedLocalNetwork(_) => Err(UrlPolicyError::LanOriginAddressNotApproved),
        UrlPolicyKind::Public | UrlPolicyKind::LocalLoopback => {
            Err(UrlPolicyError::DisallowedIpAddress { address, class })
        }
    }
}

/// Validates a lookup and, for an IP-literal URL, requires an exact literal
/// match. Callers normally skip DNS entirely for an IP literal, but this
/// prevents accidental resolver substitution when a common path is used.
pub fn validate_dns_answers_for_url(
    url: &CanonicalUrl,
    answers: &[IpAddr],
) -> Result<ValidatedDnsAnswers, UrlPolicyError> {
    let validated = validate_dns_answers_with_policy(&url.policy, answers)?;
    if let Some(literal) = url.host_ip() {
        let literal = normalize_ip(literal);
        if validated.addresses != [literal] {
            return Err(UrlPolicyError::DnsAnswerDoesNotMatchLiteral);
        }
    }
    Ok(validated)
}

pub fn revalidate_dns_answers(
    previous: &ValidatedDnsAnswers,
    current: &[IpAddr],
) -> Result<(), UrlPolicyError> {
    previous.revalidate(current)
}

/// Result of resolving and policy-checking a redirect location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedRedirect {
    target: CanonicalUrl,
    origin_changed: bool,
}

impl ValidatedRedirect {
    pub fn target(&self) -> &CanonicalUrl {
        &self.target
    }

    pub const fn origin_changed(&self) -> bool {
        self.origin_changed
    }

    /// Credentials are always stripped from an automatically followed redirect,
    /// including a same-origin redirect. A new request may independently ask the
    /// credential broker for an exact-origin credential.
    pub const fn must_strip_credentials(&self) -> bool {
        true
    }

    pub const fn requires_origin_approval(&self) -> bool {
        self.origin_changed
    }
}

/// Resolves a relative or absolute redirect, reapplies the complete URL policy,
/// and never grants credential forwarding.
pub fn validate_redirect(
    source: &CanonicalUrl,
    location: &str,
    redirects_followed: usize,
) -> Result<ValidatedRedirect, UrlPolicyError> {
    if redirects_followed >= MAX_REDIRECTS {
        return Err(UrlPolicyError::TooManyRedirects);
    }
    if location.is_empty() {
        return Err(UrlPolicyError::InvalidUrl(
            "empty redirect location".to_owned(),
        ));
    }
    if location.len() > MAX_URL_BYTES
        || location.trim() != location
        || location.bytes().any(is_ascii_control)
    {
        return Err(UrlPolicyError::InvalidUrl(
            "invalid redirect location".to_owned(),
        ));
    }

    let target = source.join(location)?;
    let origin_changed = source.origin != target.origin;
    Ok(ValidatedRedirect {
        target,
        origin_changed,
    })
}

/// Applies URL policy and DNS policy to a redirect target in one step.
pub fn validate_redirect_with_dns(
    source: &CanonicalUrl,
    location: &str,
    redirects_followed: usize,
    answers: &[IpAddr],
) -> Result<(ValidatedRedirect, ValidatedDnsAnswers), UrlPolicyError> {
    let redirect = validate_redirect(source, location, redirects_followed)?;
    let dns = validate_dns_answers_for_url(redirect.target(), answers)?;
    Ok((redirect, dns))
}

/// Credential permission bound to one exact canonical origin.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CredentialOriginScope {
    origin: CanonicalOrigin,
}

impl CredentialOriginScope {
    pub fn new(origin: CanonicalOrigin) -> Self {
        Self { origin }
    }

    pub fn from_url(url: &CanonicalUrl) -> Self {
        Self::new(url.origin.clone())
    }

    pub fn origin(&self) -> &CanonicalOrigin {
        &self.origin
    }

    pub fn permits(&self, target: &CanonicalUrl) -> bool {
        self.origin == target.origin
    }
}

pub fn credential_scope_matches(approved: &CanonicalOrigin, target: &CanonicalOrigin) -> bool {
    approved == target
}

pub fn same_canonical_origin(left: &CanonicalUrl, right: &CanonicalUrl) -> bool {
    left.origin == right.origin
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(value: &str) -> IpAddr {
        value.parse().expect("test address should parse")
    }

    #[test]
    fn canonicalizes_idna_root_dot_default_port_and_strips_secrets() {
        let url = canonicalize_public_url(
            "https://BÜCHER.Example.COM.:443/docs?lang=ko&API_KEY=secret&x=1#part",
        )
        .unwrap();

        assert_eq!(
            url.as_str(),
            "https://xn--bcher-kva.example.com/docs?lang=ko&x=1"
        );
        assert_eq!(
            url.origin().as_string(),
            "https://xn--bcher-kva.example.com"
        );
        assert_eq!(url.stripped_sensitive_query_parameters(), 1);
        assert!(url.stripped_fragment());
    }

    #[test]
    fn canonical_url_debug_never_contains_path_or_query_material() {
        let path_secret = "signed-path-private-sentinel";
        let query_secret = "private-query-sentinel";
        let url = canonicalize_public_url(&format!(
            "https://api.example.com/{path_secret}?lang={query_secret}"
        ))
        .unwrap();
        let debug = format!("{url:?}");
        assert!(!debug.contains(path_secret));
        assert!(!debug.contains(query_secret));
        assert!(debug.contains("path_is_root"));
        assert!(debug.contains("has_query"));
    }

    #[test]
    fn sensitive_query_matching_handles_case_encoding_and_punctuation() {
        for key in [
            "api_key",
            "Api-Key",
            "API%5FKEY",
            "access_token",
            "X-Amz-Credential",
            "X-Amz-Signature",
            "password",
            "authorization",
            "session_id",
        ] {
            let input = format!("https://api.example.com/v1?keep=yes&{key}=do-not-keep");
            let url = canonicalize_public_url(&input).unwrap();
            assert_eq!(url.as_str(), "https://api.example.com/v1?keep=yes");
        }
        assert!(!is_sensitive_query_key("monkey"));
        assert!(!is_sensitive_query_key("model"));
    }

    #[test]
    fn rejects_userinfo_and_non_https_public_urls() {
        for input in [
            "https://user:secret@api.example.com/v1",
            "https://@api.example.com/v1",
            "https://user@api.example.com/v1",
        ] {
            assert!(
                matches!(
                    canonicalize_public_url(input),
                    Err(UrlPolicyError::UserInfoNotAllowed)
                ),
                "{input}"
            );
        }
        assert!(matches!(
            canonicalize_public_url("http://api.example.com/v1"),
            Err(UrlPolicyError::UnsupportedScheme)
        ));
        assert!(matches!(
            canonicalize_public_url("file:///etc/passwd"),
            Err(UrlPolicyError::UnsupportedScheme)
        ));
    }

    #[test]
    fn rejects_control_characters_in_raw_and_encoded_urls() {
        assert!(matches!(
            canonicalize_public_url("https://api.example.com/\nheader"),
            Err(UrlPolicyError::EmbeddedControlCharacter)
        ));
        assert!(matches!(
            canonicalize_public_url("https://api.example.com/%0d%0aheader"),
            Err(UrlPolicyError::EmbeddedControlCharacter)
        ));
        assert!(matches!(
            canonicalize_public_url(" https://api.example.com/"),
            Err(UrlPolicyError::EmbeddedControlCharacter)
        ));
    }

    #[test]
    fn canonical_origin_uses_exact_effective_port() {
        let implicit = canonicalize_public_url("https://api.example.com/v1").unwrap();
        let explicit_default =
            canonicalize_public_url("https://api.example.com:443/elsewhere").unwrap();
        let alternate = canonicalize_public_url("https://api.example.com:8443/v1").unwrap();

        assert!(same_canonical_origin(&implicit, &explicit_default));
        assert!(!same_canonical_origin(&implicit, &alternate));
        assert_eq!(
            alternate.origin().as_string(),
            "https://api.example.com:8443"
        );
    }

    #[test]
    fn rejects_reserved_or_nonqualified_public_names() {
        for input in [
            "https://localhost/",
            "https://service.localhost/",
            "https://model.internal/",
            "https://host.local/",
            "https://example.test/",
            "https://singlelabel/",
            "https://home.arpa/",
        ] {
            assert!(
                matches!(
                    canonicalize_public_url(input),
                    Err(UrlPolicyError::ReservedHost)
                ),
                "{input}"
            );
        }
    }

    #[test]
    fn local_mode_is_explicit_and_loopback_only() {
        for input in [
            "http://localhost:11434/api",
            "http://model.localhost/api",
            "http://127.0.0.1:8080/v1",
            "https://[::1]:8443/v1",
        ] {
            canonicalize_local_loopback_url(input).unwrap();
        }

        for input in [
            "http://192.168.1.10:8080/",
            "http://10.0.0.1/",
            "http://api.example.com/",
        ] {
            assert!(canonicalize_local_loopback_url(input).is_err(), "{input}");
        }
        assert!(canonicalize_public_url("https://127.0.0.1/").is_err());
    }

    #[test]
    fn approved_lan_is_exact_origin_and_exact_private_addresses_only() {
        let approval = ApprovedLocalNetworkOrigin::new(
            "http://models.lan:11434",
            &[ip("192.168.10.24"), ip("fd12:3456::24")],
        )
        .unwrap();
        let policy = UrlPolicy::approved_local_network(approval);
        let url = policy
            .canonicalize("http://models.lan:11434/api/tags")
            .unwrap();
        assert_eq!(
            url.network_boundary(),
            UrlNetworkBoundary::ApprovedLocalNetwork
        );
        policy
            .validate_dns(&[ip("192.168.10.24"), ip("fd12:3456::24")])
            .unwrap();
        assert!(matches!(
            policy.validate_dns(&[ip("192.168.10.25")]),
            Err(UrlPolicyError::LanOriginAddressNotApproved)
        ));
        assert!(matches!(
            policy.canonicalize("http://other.lan:11434/api/tags"),
            Err(UrlPolicyError::LanOriginNotApproved)
        ));
        assert!(matches!(
            policy.canonicalize("http://models.lan:8080/api/tags"),
            Err(UrlPolicyError::LanOriginNotApproved)
        ));
        assert!(matches!(
            policy.canonicalize("https://models.lan:11434/api/tags"),
            Err(UrlPolicyError::LanOriginNotApproved)
        ));
        assert!(validate_redirect(&url, "http://192.168.10.24:11434/api/tags", 0).is_err());
    }

    #[test]
    fn lan_approval_cannot_express_broad_or_non_private_access() {
        for addresses in [
            Vec::new(),
            vec![ip("127.0.0.1")],
            vec![ip("169.254.169.254")],
            vec![ip("100.64.0.1")],
            vec![ip("1.1.1.1")],
        ] {
            assert!(matches!(
                ApprovedLocalNetworkOrigin::new("http://models.lan:11434", &addresses),
                Err(UrlPolicyError::LanApprovalRequiresPrivateAddresses)
            ));
        }
        assert!(matches!(
            ApprovedLocalNetworkOrigin::new("http://models.lan:11434/path", &[ip("192.168.10.24")]),
            Err(UrlPolicyError::LanApprovalMustBeOrigin)
        ));
    }

    #[test]
    fn approved_lan_literal_dns_revalidation_and_peer_checks_fail_closed() {
        assert!(matches!(
            ApprovedLocalNetworkOrigin::new("http://192.168.10.24:11434", &[ip("192.168.10.25")]),
            Err(UrlPolicyError::LanOriginAddressNotApproved)
        ));
        assert!(matches!(
            ApprovedLocalNetworkOrigin::new(
                "http://192.168.10.24:11434",
                &[ip("192.168.10.24"), ip("192.168.10.25")]
            ),
            Err(UrlPolicyError::LanOriginAddressNotApproved)
        ));

        let approval = ApprovedLocalNetworkOrigin::new(
            "https://models.lan:8443",
            &[ip("10.0.0.9"), ip("192.168.10.24")],
        )
        .unwrap();
        let policy = UrlPolicy::approved_local_network(approval);
        let target = policy.canonicalize("https://models.lan:8443/v1").unwrap();
        let pinned = validate_dns_answers_for_url(
            &target,
            &[ip("192.168.10.24"), ip("10.0.0.9"), ip("10.0.0.9")],
        )
        .unwrap();

        pinned
            .revalidate(&[ip("10.0.0.9"), ip("192.168.10.24")])
            .unwrap();
        assert!(matches!(
            pinned.revalidate(&[ip("10.0.0.9")]),
            Err(UrlPolicyError::DnsAnswersChanged)
        ));
        assert!(matches!(
            policy.validate_dns(&[ip("10.0.0.10")]),
            Err(UrlPolicyError::LanOriginAddressNotApproved)
        ));
        assert!(matches!(
            policy.validate_dns(&[ip("1.1.1.1")]),
            Err(UrlPolicyError::DisallowedIpAddress {
                class: IpAddressClass::Public,
                ..
            })
        ));
        assert!(matches!(
            policy.validate_dns(&[ip("10.0.0.9"), ip("10.0.0.10")]),
            Err(UrlPolicyError::MixedDnsAddressScopes)
        ));
        assert!(matches!(
            pinned.validate_peer(ip("192.168.10.25")),
            Err(UrlPolicyError::LanOriginAddressNotApproved)
        ));

        let one_address = policy.validate_dns(&[ip("10.0.0.9")]).unwrap();
        assert!(matches!(
            one_address.validate_peer(ip("192.168.10.24")),
            Err(UrlPolicyError::PeerAddressNotValidated)
        ));
    }

    #[tokio::test]
    async fn resolved_target_revalidates_literal_and_checks_peer_port() {
        let url = canonicalize_local_loopback_url("http://127.0.0.1:11434/api/tags").unwrap();
        let target = ResolvedNetworkTarget::resolve(&url, Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(
            target.socket_addresses(),
            &["127.0.0.1:11434".parse::<SocketAddr>().unwrap()]
        );
        target.revalidate_dns(Duration::from_secs(1)).await.unwrap();
        target
            .validate_peer("127.0.0.1:11434".parse().unwrap())
            .unwrap();
        assert!(matches!(
            target.validate_peer("127.0.0.1:11435".parse().unwrap()),
            Err(UrlPolicyError::PeerPortNotValidated)
        ));
        assert!(matches!(
            target.validate_peer("127.0.0.2:11434".parse().unwrap()),
            Err(UrlPolicyError::PeerAddressNotValidated)
        ));
        target
            .pin_reqwest_builder(
                reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .no_proxy(),
            )
            .build()
            .unwrap();
    }

    #[test]
    fn classifies_ipv4_ssrf_corpus() {
        let cases = [
            ("0.0.0.0", IpAddressClass::Unspecified),
            ("10.0.0.1", IpAddressClass::Private),
            ("100.64.0.1", IpAddressClass::CarrierGradeNat),
            ("100.100.100.200", IpAddressClass::Metadata),
            ("127.255.255.254", IpAddressClass::Loopback),
            ("169.254.1.1", IpAddressClass::LinkLocal),
            ("169.254.169.254", IpAddressClass::Metadata),
            ("172.16.0.1", IpAddressClass::Private),
            ("192.168.0.1", IpAddressClass::Private),
            ("192.0.2.1", IpAddressClass::Documentation),
            ("198.18.0.1", IpAddressClass::Benchmark),
            ("198.51.100.1", IpAddressClass::Documentation),
            ("203.0.113.1", IpAddressClass::Documentation),
            ("224.0.0.1", IpAddressClass::Multicast),
            ("255.255.255.255", IpAddressClass::Reserved),
            ("168.63.129.16", IpAddressClass::Metadata),
            ("1.1.1.1", IpAddressClass::Public),
        ];
        for (address, expected) in cases {
            assert_eq!(classify_ip_address(ip(address)), expected, "{address}");
        }
    }

    #[test]
    fn classifies_ipv6_and_embedded_ipv4_ssrf_corpus() {
        let cases = [
            ("::", IpAddressClass::Unspecified),
            ("::1", IpAddressClass::Loopback),
            ("fc00::1", IpAddressClass::Private),
            ("fd00:ec2::254", IpAddressClass::Metadata),
            ("fe80::1", IpAddressClass::LinkLocal),
            ("ff02::1", IpAddressClass::Multicast),
            ("2001:db8::1", IpAddressClass::Documentation),
            ("3fff::1", IpAddressClass::Documentation),
            ("2001:2::1", IpAddressClass::Benchmark),
            ("2001:5::1", IpAddressClass::Reserved),
            ("2001:4:112::1", IpAddressClass::Public),
            ("2002:7f00:1::", IpAddressClass::Reserved),
            ("::ffff:127.0.0.1", IpAddressClass::Loopback),
            ("::ffff:10.0.0.1", IpAddressClass::Private),
            ("::ffff:192.0.2.1", IpAddressClass::Documentation),
            ("::ffff:8.8.8.8", IpAddressClass::Public),
            ("64:ff9b::127.0.0.1", IpAddressClass::Loopback),
            ("64:ff9b::8.8.8.8", IpAddressClass::Public),
            ("2606:4700:4700::1111", IpAddressClass::Public),
        ];
        for (address, expected) in cases {
            assert_eq!(classify_ip_address(ip(address)), expected, "{address}");
        }
    }

    #[test]
    fn url_parser_numeric_aliases_cannot_bypass_ip_policy() {
        for input in [
            "https://127.1/",
            "https://2130706433/",
            "https://0x7f000001/",
            "https://0177.0.0.1/",
            "https://[::ffff:127.0.0.1]/",
        ] {
            assert!(canonicalize_public_url(input).is_err(), "{input}");
        }
    }

    #[test]
    fn dns_validation_is_bounded_and_rejects_mixed_scope() {
        assert!(matches!(
            validate_dns_answers(UrlPolicyMode::Public, &[]),
            Err(UrlPolicyError::EmptyDnsAnswers)
        ));
        let too_many = vec![ip("1.1.1.1"); MAX_DNS_ANSWERS + 1];
        assert!(matches!(
            validate_dns_answers(UrlPolicyMode::Public, &too_many),
            Err(UrlPolicyError::TooManyDnsAnswers)
        ));
        assert!(matches!(
            validate_dns_answers(UrlPolicyMode::Public, &[ip("1.1.1.1"), ip("127.0.0.1")]),
            Err(UrlPolicyError::MixedDnsAddressScopes)
        ));
        assert!(matches!(
            validate_dns_answers(UrlPolicyMode::Public, &[ip("10.0.0.1")]),
            Err(UrlPolicyError::DisallowedIpAddress {
                class: IpAddressClass::Private,
                ..
            })
        ));
    }

    #[test]
    fn dns_validation_allows_mixed_public_families_and_pins_peer() {
        let answers = validate_dns_answers(
            UrlPolicyMode::Public,
            &[ip("2606:4700:4700::1111"), ip("1.1.1.1"), ip("1.1.1.1")],
        )
        .unwrap();
        assert_eq!(
            answers.addresses(),
            &[ip("1.1.1.1"), ip("2606:4700:4700::1111")]
        );
        answers.validate_peer(ip("1.1.1.1")).unwrap();
        assert!(matches!(
            answers.validate_peer(ip("8.8.8.8")),
            Err(UrlPolicyError::PeerAddressNotValidated)
        ));
        assert!(matches!(
            answers.revalidate(&[ip("8.8.8.8")]),
            Err(UrlPolicyError::DnsAnswersChanged)
        ));
    }

    #[test]
    fn local_dns_requires_only_loopback_answers() {
        validate_dns_answers(UrlPolicyMode::LocalLoopback, &[ip("127.0.0.1"), ip("::1")]).unwrap();
        assert!(matches!(
            validate_dns_answers(
                UrlPolicyMode::LocalLoopback,
                &[ip("127.0.0.1"), ip("192.168.1.2")]
            ),
            Err(UrlPolicyError::MixedDnsAddressScopes)
        ));
    }

    #[test]
    fn literal_url_dns_must_match_exactly() {
        let url = canonicalize_public_url("https://1.1.1.1/v1").unwrap();
        validate_dns_answers_for_url(&url, &[ip("1.1.1.1")]).unwrap();
        assert!(matches!(
            validate_dns_answers_for_url(&url, &[ip("8.8.8.8")]),
            Err(UrlPolicyError::DnsAnswerDoesNotMatchLiteral)
        ));
    }

    #[test]
    fn redirect_reapplies_policy_and_never_forwards_credentials() {
        let source = canonicalize_public_url("https://api.example.com/start").unwrap();
        let same = validate_redirect(&source, "/next?token=secret&part=1#x", 0).unwrap();
        assert_eq!(
            same.target().as_str(),
            "https://api.example.com/next?part=1"
        );
        assert!(!same.origin_changed());
        assert!(same.must_strip_credentials());

        let cross = validate_redirect(&source, "https://other.example.com/next", 1).unwrap();
        assert!(cross.origin_changed());
        assert!(cross.requires_origin_approval());
        assert!(cross.must_strip_credentials());

        assert!(validate_redirect(&source, "http://127.0.0.1/admin", 1).is_err());
        assert!(matches!(
            validate_redirect(&source, "/next", MAX_REDIRECTS),
            Err(UrlPolicyError::TooManyRedirects)
        ));
    }

    #[test]
    fn credential_scope_is_exact_origin_only() {
        let approved = canonicalize_public_url("https://api.example.com/v1").unwrap();
        let same = canonicalize_public_url("https://api.example.com/models").unwrap();
        let different_port = canonicalize_public_url("https://api.example.com:8443/v1").unwrap();
        let subdomain = canonicalize_public_url("https://sub.api.example.com/v1").unwrap();
        let scope = CredentialOriginScope::from_url(&approved);

        assert!(scope.permits(&same));
        assert!(!scope.permits(&different_port));
        assert!(!scope.permits(&subdomain));
        assert!(credential_scope_matches(approved.origin(), same.origin()));
    }

    #[test]
    fn enforces_path_query_and_segment_bounds() {
        let long_segment = "a".repeat(MAX_PATH_SEGMENT_BYTES + 1);
        let input = format!("https://api.example.com/{long_segment}");
        assert!(matches!(
            canonicalize_public_url(&input),
            Err(UrlPolicyError::PathSegmentTooLong)
        ));

        let long_query = "a".repeat(MAX_QUERY_BYTES + 1);
        let input = format!("https://api.example.com/?q={long_query}");
        assert!(matches!(
            canonicalize_public_url(&input),
            Err(UrlPolicyError::QueryTooLong)
        ));

        let path = (0..MAX_PATH_SEGMENTS)
            .map(|_| "x")
            .collect::<Vec<_>>()
            .join("/");
        let input = format!("https://api.example.com/{path}");
        assert!(matches!(
            canonicalize_public_url(&input),
            Err(UrlPolicyError::TooManyPathSegments)
        ));
    }
}
