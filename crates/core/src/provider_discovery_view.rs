use lorepia_domain::{CoreResult, DiscoverySessionId, discovery::DiscoveryCandidate};
use lorepia_storage::DiscoveryCandidateSnapshot;

use crate::app::Core;

/// Storage-independent projection returned by the provider-discovery list API.
///
/// Keeping this view in Core prevents shell bindings from depending on the
/// persistence row used to store discovery candidates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDiscoveryCandidateView {
    pub candidate: DiscoveryCandidate,
    pub proposed_revision: u64,
}

fn project_candidate(value: DiscoveryCandidateSnapshot) -> ProviderDiscoveryCandidateView {
    ProviderDiscoveryCandidateView {
        candidate: value.candidate,
        proposed_revision: value.proposed_revision,
    }
}

impl Core {
    pub fn list_provider_discovery_candidates(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<Vec<ProviderDiscoveryCandidateView>> {
        self.provider_discovery()
            .candidates(session_id)
            .map(|candidates| candidates.into_iter().map(project_candidate).collect())
    }
}
