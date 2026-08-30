//! Credential-free, deterministic provider discovery.
//!
//! This module is intentionally independent from the durable discovery
//! reducer. It accepts only compiled provider identities, policy-validated
//! document fetch plans, or the sanitized output of the bounded cURL parser.
//! Raw cURL, credentials, response bodies, and credential-origin approvals are
//! neither retained nor returned.

#[cfg(test)]
#[allow(unused_imports)]
use std::{collections::BTreeMap, error::Error, fmt};

#[cfg(test)]
#[allow(unused_imports)]
use lorepia_domain::{
    ApiFamily, AuthBinding, CanonicalOrigin, ConnectionFieldSpec, EndpointPath, HeaderName,
    HttpMethod, HttpUrl, ManifestSource, ManifestSourceKind, ProviderManifest, ProviderNetworkMode,
    ProviderTemplate, ProviderTemplateId, TemplateSource,
};
#[cfg(test)]
#[allow(unused_imports)]
use lorepia_providers::{
    AdapterRegistry, BuiltInTemplateId, CurlAuthHint, JsonShape, ParsedCurlEvidence,
    discovery::{
        BoundedDocumentFetcher, DiscoveryDocumentEvidence, DiscoveryEvidenceKind,
        DiscoveryFetchBudget, DiscoveryFetchError, DiscoveryFetchIssue, DiscoveryFetchIssueKind,
        DiscoveryFetchPlan,
    },
    url_policy::{UrlNetworkBoundary, UrlPolicy},
};
#[cfg(test)]
use lorepia_providers::{SecretCurlInput, parse_curl, url_policy::UrlPolicyMode};
#[cfg(test)]
#[allow(unused_imports)]
use serde::{Deserialize, Serialize};
#[cfg(test)]
#[allow(unused_imports)]
use serde_json::{Value, json};
#[cfg(test)]
#[allow(unused_imports)]
use sha2::{Digest, Sha256};

#[path = "provider_discovery/deterministic_execution.rs"]
mod execution;
#[path = "provider_discovery/deterministic_resolution.rs"]
mod resolution;
#[path = "provider_discovery/deterministic_source.rs"]
mod source;

pub(crate) use execution::DeterministicDiscoveryExecutor;
pub(crate) use resolution::embed_discovered_api_base_path;
pub(crate) use source::{
    DeterministicDiscoveryError, DeterministicDiscoveryErrorKind, DeterministicDiscoveryOutput,
    DeterministicDiscoverySource, DiscoveryCandidateConfidence, RedactedDiscoveryEvidenceRecord,
};

#[cfg(test)]
use execution::sanitized_curl_json;
#[cfg(test)]
use source::{
    DeterministicDiscoveryResult, DeterministicDiscoverySourceKind, SanitizedCurlDiscoveryEvidence,
    sanitize_curl_source,
};

#[cfg(test)]
#[path = "provider_discovery/deterministic_tests.rs"]
mod tests;
