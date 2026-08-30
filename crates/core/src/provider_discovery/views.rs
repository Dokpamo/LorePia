use super::{
    CandidateView, CoreResult, DateTime, DiscoveryApprovalRecord, DiscoveryEventId,
    DiscoveryEvidenceRecord, DiscoveryOutboxEvent, DiscoveryReviewDiff, DiscoverySessionId,
    DiscoverySessionSnapshot, MAX_DISCOVERY_ROWS, ProviderDiscoveryOrchestrator, Utc,
};

impl ProviderDiscoveryOrchestrator<'_> {
    pub fn get(&self, session_id: &DiscoverySessionId) -> CoreResult<DiscoverySessionSnapshot> {
        self.storage.get_discovery_session(session_id)
    }

    pub fn candidates(&self, session_id: &DiscoverySessionId) -> CoreResult<Vec<CandidateView>> {
        self.storage
            .read_discovery_candidates(session_id, MAX_DISCOVERY_ROWS)
    }

    pub fn evidence(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<Vec<DiscoveryEvidenceRecord>> {
        self.storage
            .list_discovery_evidence(session_id, MAX_DISCOVERY_ROWS)
    }

    pub fn review(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<Option<DiscoveryReviewDiff>> {
        self.storage.get_discovery_review(session_id)
    }

    pub fn poll_outbox(
        &self,
        limit: u32,
        available_at: DateTime<Utc>,
    ) -> CoreResult<Vec<DiscoveryOutboxEvent>> {
        self.storage.poll_discovery_events(limit, available_at)
    }

    pub fn poll_outbox_for_session(
        &self,
        session_id: &DiscoverySessionId,
        limit: u32,
        available_at: DateTime<Utc>,
    ) -> CoreResult<Vec<DiscoveryOutboxEvent>> {
        self.storage
            .poll_discovery_events_for_session(session_id, limit, available_at)
    }

    pub fn ack_outbox(
        &self,
        event_id: &DiscoveryEventId,
        delivered_at: DateTime<Utc>,
    ) -> CoreResult<bool> {
        self.storage.ack_discovery_event(event_id, delivered_at)
    }

    pub fn approvals(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<Vec<DiscoveryApprovalRecord>> {
        self.storage
            .list_discovery_approvals(session_id, MAX_DISCOVERY_ROWS)
    }

    pub fn list(&self, limit: u32) -> CoreResult<Vec<DiscoverySessionSnapshot>> {
        self.storage.list_discovery_sessions(limit)
    }
}

impl crate::app::Core {
    pub(crate) fn provider_discovery(&self) -> ProviderDiscoveryOrchestrator<'_> {
        ProviderDiscoveryOrchestrator::new(
            self.storage(),
            self.runtime_handle(),
            self.discovery_recovery_owner(),
        )
    }

    pub fn list_provider_discoveries(
        &self,
        limit: u32,
    ) -> CoreResult<Vec<DiscoverySessionSnapshot>> {
        self.provider_discovery().list(limit)
    }

    pub fn get_provider_discovery(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery().get(session_id)
    }

    pub fn list_provider_discovery_evidence(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<Vec<DiscoveryEvidenceRecord>> {
        self.provider_discovery().evidence(session_id)
    }

    pub fn get_provider_discovery_review(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<Option<DiscoveryReviewDiff>> {
        self.provider_discovery().review(session_id)
    }

    pub fn poll_provider_discovery_events(
        &self,
        limit: u32,
        available_at: DateTime<Utc>,
    ) -> CoreResult<Vec<DiscoveryOutboxEvent>> {
        self.provider_discovery().poll_outbox(limit, available_at)
    }

    pub fn poll_provider_discovery_events_for_session(
        &self,
        session_id: &DiscoverySessionId,
        limit: u32,
        available_at: DateTime<Utc>,
    ) -> CoreResult<Vec<DiscoveryOutboxEvent>> {
        self.provider_discovery()
            .poll_outbox_for_session(session_id, limit, available_at)
    }

    pub fn ack_provider_discovery_event(
        &self,
        event_id: &DiscoveryEventId,
        delivered_at: DateTime<Utc>,
    ) -> CoreResult<bool> {
        self.provider_discovery().ack_outbox(event_id, delivered_at)
    }
}
