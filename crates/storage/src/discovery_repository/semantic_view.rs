use lorepia_domain::{CoreResult, DiscoverySessionId, discovery::DiscoveryCandidate};

use crate::Storage;

/// Meaning-based discovery candidate projection for repository consumers.
///
/// Unlike [`StoredDiscoveryCandidate`], this type is a read contract rather
/// than an input to the durable discovery transition transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryCandidateSnapshot {
    pub candidate: DiscoveryCandidate,
    pub proposed_revision: u64,
}

/// Compatibility input used by the existing durable transition API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredDiscoveryCandidate {
    pub candidate: DiscoveryCandidate,
    pub proposed_revision: u64,
}

impl From<StoredDiscoveryCandidate> for DiscoveryCandidateSnapshot {
    fn from(value: StoredDiscoveryCandidate) -> Self {
        Self {
            candidate: value.candidate,
            proposed_revision: value.proposed_revision,
        }
    }
}

impl Storage {
    /// Reads candidate meaning without exposing the durable transition input.
    pub fn read_discovery_candidates(
        &self,
        session_id: &DiscoverySessionId,
        limit: u32,
    ) -> CoreResult<Vec<DiscoveryCandidateSnapshot>> {
        self.list_discovery_candidates(session_id, limit)
            .map(|candidates| candidates.into_iter().map(Into::into).collect())
    }
}
