use std::time::Duration;

use lorepia_domain::{AuthBinding, CoreError, CoreErrorCode, CoreResult};
use reqwest::{
    Client, RequestBuilder, Response,
    header::{AUTHORIZATION, HeaderName, HeaderValue},
};
use zeroize::Zeroizing;

use crate::url_policy::{CanonicalUrl, ResolvedNetworkTarget, UrlPolicy};

/// One exact HTTP target plus the network policy that must hold for every
/// request to it.
///
/// Resolution is intentionally lazy so the synchronous adapter registry can
/// construct providers without performing network I/O. Every request resolves,
/// pins, revalidates, and later verifies the connected peer.
#[derive(Debug, Clone)]
pub(crate) struct ProviderHttpTarget {
    canonical: CanonicalUrl,
    timeout: Duration,
}

impl ProviderHttpTarget {
    pub(crate) fn new(endpoint: &str, policy: &UrlPolicy, timeout: Duration) -> CoreResult<Self> {
        if timeout.is_zero() {
            return Err(CoreError::invalid(
                "provider timeout must be greater than zero",
            ));
        }
        let canonical = policy
            .canonicalize(endpoint)
            .map_err(|_| invalid_target_error())?;
        if canonical.stripped_sensitive_query_parameters() != 0 || canonical.stripped_fragment() {
            return Err(CoreError::invalid(
                "provider endpoint must not contain a query credential or fragment",
            ));
        }
        Ok(Self { canonical, timeout })
    }

    pub(crate) fn inferred(endpoint: &str, timeout: Duration) -> CoreResult<Self> {
        let parsed = url::Url::parse(endpoint)
            .map_err(|_| CoreError::invalid("invalid provider endpoint URL"))?;
        let policy = if parsed.host().is_some_and(is_loopback_host) {
            UrlPolicy::local_loopback()
        } else {
            UrlPolicy::public()
        };
        Self::new(endpoint, &policy, timeout)
    }

    pub(crate) fn url(&self) -> &url::Url {
        self.canonical.url()
    }

    pub(crate) fn origin(&self) -> &crate::url_policy::CanonicalOrigin {
        self.canonical.origin()
    }

    pub(crate) async fn prepare(&self) -> CoreResult<PreparedHttpTarget> {
        let lookup_timeout = self.timeout.min(Duration::from_secs(30));
        let resolved = ResolvedNetworkTarget::resolve(&self.canonical, lookup_timeout)
            .await
            .map_err(|_| resolution_error())?;
        let builder = Client::builder()
            .timeout(self.timeout)
            .connect_timeout(lookup_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .referer(false)
            .no_proxy();
        let client = resolved
            .pin_reqwest_builder(builder)
            .build()
            .map_err(|_| client_error())?;
        // Revalidate only after the pinned client exists, immediately before
        // the caller constructs and sends its request.
        resolved
            .revalidate_dns(lookup_timeout)
            .await
            .map_err(|_| resolution_error())?;
        Ok(PreparedHttpTarget { resolved, client })
    }
}

pub(crate) struct PreparedHttpTarget {
    resolved: ResolvedNetworkTarget,
    client: Client,
}

impl PreparedHttpTarget {
    pub(crate) fn client(&self) -> &Client {
        &self.client
    }

    pub(crate) fn validate_response_peer(&self, response: &Response) -> CoreResult<()> {
        let peer = response.remote_addr().ok_or_else(peer_error)?;
        self.resolved.validate_peer(peer).map_err(|_| peer_error())
    }
}

pub(crate) fn authorize_request(
    request: RequestBuilder,
    auth: &AuthBinding,
    credential: Option<&str>,
) -> CoreResult<RequestBuilder> {
    validate_credential_for_auth(auth, credential)?;
    match auth {
        AuthBinding::None => Ok(request),
        AuthBinding::BearerHeader => {
            let credential = required_credential(credential)?;
            let wire_credential = Zeroizing::new(format!("Bearer {credential}"));
            let mut value =
                HeaderValue::from_str(&wire_credential).map_err(|_| invalid_credential_error())?;
            value.set_sensitive(true);
            Ok(request.header(AUTHORIZATION, value))
        }
        AuthBinding::HeaderApiKey { header_name } => {
            let credential = required_credential(credential)?;
            let name = HeaderName::from_bytes(header_name.as_str().as_bytes()).map_err(|_| {
                CoreError::internal("validated provider auth header became invalid")
            })?;
            let mut value =
                HeaderValue::from_str(credential).map_err(|_| invalid_credential_error())?;
            value.set_sensitive(true);
            Ok(request.header(name, value))
        }
    }
}

pub(crate) fn validate_credential_for_auth(
    auth: &AuthBinding,
    credential: Option<&str>,
) -> CoreResult<()> {
    match auth {
        AuthBinding::None if credential.is_some_and(|value| !value.is_empty()) => Err(
            CoreError::invalid("this provider connection does not permit a credential"),
        ),
        AuthBinding::BearerHeader | AuthBinding::HeaderApiKey { .. } => {
            required_credential(credential).map(|_| ())
        }
        AuthBinding::None => Ok(()),
    }
}

fn is_loopback_host(host: url::Host<&str>) -> bool {
    match host {
        url::Host::Domain(domain) => {
            let domain = domain.trim_end_matches('.');
            domain.eq_ignore_ascii_case("localhost")
                || domain
                    .to_ascii_lowercase()
                    .strip_suffix(".localhost")
                    .is_some_and(|prefix| !prefix.is_empty())
        }
        url::Host::Ipv4(address) => address.is_loopback(),
        url::Host::Ipv6(address) => address.is_loopback(),
    }
}

fn invalid_target_error() -> CoreError {
    CoreError::new(
        CoreErrorCode::PermissionDenied,
        "provider endpoint is not allowed by the selected network policy",
        false,
    )
}

fn resolution_error() -> CoreError {
    CoreError::new(
        CoreErrorCode::NetworkUnavailable,
        "provider network target could not be resolved safely",
        true,
    )
}

fn client_error() -> CoreError {
    CoreError::new(
        CoreErrorCode::ProviderUnavailable,
        "cannot create provider HTTP client",
        true,
    )
}

fn peer_error() -> CoreError {
    CoreError::new(
        CoreErrorCode::PermissionDenied,
        "provider response peer did not match the approved network target",
        false,
    )
}

fn invalid_credential_error() -> CoreError {
    CoreError::new(
        CoreErrorCode::ProviderAuthFailed,
        "provider credential cannot be used as an HTTP header",
        false,
    )
}

fn required_credential(credential: Option<&str>) -> CoreResult<&str> {
    credential.filter(|value| !value.is_empty()).ok_or_else(|| {
        CoreError::new(
            CoreErrorCode::ProviderAuthFailed,
            "provider credential is required",
            false,
        )
    })
}
