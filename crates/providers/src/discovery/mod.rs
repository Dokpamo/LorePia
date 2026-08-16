//! Deterministic, credential-free provider document discovery.
//!
//! This module deliberately separates untrusted document fetching and
//! extraction from credential-bearing provider requests. The fetch API has no
//! credential or arbitrary-header argument.

mod evidence;
mod fetcher;
mod openapi;

pub use evidence::{
    DiscoveryDocumentEvidence, DiscoveryEvidenceKind, RedactedUrlEvidence, UntrustedDocumentText,
    UntrustedTextOrigin,
};
pub(crate) use evidence::{
    contains_credential_like_token, looks_like_known_credential, looks_like_opaque_secret,
};
pub use fetcher::{
    BoundedDocumentFetcher, DiscoveryFetchBudget, DiscoveryFetchError, DiscoveryFetchIssue,
    DiscoveryFetchIssueKind, DiscoveryFetchPlan, DiscoveryFetchReport,
};
pub use openapi::{
    ApiFamilyHint, OpenApiAuthSchemeEvidence, OpenApiDocumentFormat, OpenApiEvidence,
    OpenApiExtractionError, OpenApiExtractionLimits, OpenApiOperationEvidence,
    OpenApiServerCandidate, extract_openapi,
};
