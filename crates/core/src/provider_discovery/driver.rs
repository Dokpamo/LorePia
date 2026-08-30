use super::{
    AuthBinding, CoreError, CoreErrorCode, CoreResult, DiscoveryActionId,
    DiscoveryCompletedOperationWrite, DiscoveryEffect, DiscoveryEvidenceRecord, DiscoveryFailure,
    DiscoveryInterruptionOutcome, DiscoveryJsonUpdate, DiscoveryOperationId,
    DiscoveryOperationKind, DiscoveryOperationStatus, DiscoveryReviewDiff, DiscoverySessionId,
    DiscoverySessionSnapshot, DiscoveryTransitionWrite, DiscoveryWorkingDraft,
    DurableOperationOutcome, MAX_AUTOMATIC_EFFECTS, ProviderDiscoveryAction,
    ProviderDiscoveryOrchestrator, StoredDiscoveryCandidate, Utc, Value,
    WORKING_DRAFT_SCHEMA_VERSION, cancel_assistant_snapshot, provider_discovery_action_envelope,
    watch,
};

impl ProviderDiscoveryOrchestrator<'_> {
    pub(super) fn drive_nonpersistent(
        &self,
        session_id: &DiscoverySessionId,
        credential: Option<&str>,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        for _ in 0..MAX_AUTOMATIC_EFFECTS {
            let snapshot = self.get(session_id)?;
            let Some(operation) = self.storage.get_current_discovery_operation(session_id)? else {
                return Ok(snapshot);
            };
            if matches!(
                operation.kind,
                DiscoveryOperationKind::AtomicCommit
                    | DiscoveryOperationKind::Compensation
                    | DiscoveryOperationKind::BuildAssistantManifestDraft
            ) {
                return Ok(snapshot);
            }
            let mut draft = hydrate_working_draft(&snapshot)?;
            let requires_credential = matches!(
                operation.kind,
                DiscoveryOperationKind::ListModels | DiscoveryOperationKind::ProbeCapabilities
            ) && draft
                .template
                .as_ref()
                .is_some_and(|template| template.default_manifest.auth != AuthBinding::None);
            if requires_credential && credential.is_none_or(str::is_empty) {
                self.persist_operation_completion(
                    &snapshot,
                    &operation.id,
                    &mut draft,
                    ProviderDiscoveryAction::Interrupt {
                        operation: operation.kind,
                        outcome: DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
                    },
                    DurableOperationOutcome::Interrupted,
                    Vec::new(),
                    Vec::new(),
                    DiscoveryJsonUpdate::Preserve,
                )?;
                return self.get(session_id);
            }
            if !self
                .storage
                .mark_discovery_operation_started(&operation.id, Utc::now())?
            {
                return self.get(session_id);
            }
            let completion = match self.execute_nonpersistent_effect(
                &snapshot,
                operation.kind,
                &mut draft,
                credential,
                cancelled.clone(),
            ) {
                Ok(completion) => completion,
                Err(error) => {
                    let (action, outcome) = nonpersistent_failure_action(operation.kind, &error);
                    let completion_snapshot =
                        self.inflight_completion_snapshot(&snapshot, &operation.id)?;
                    self.persist_operation_completion(
                        &completion_snapshot,
                        &operation.id,
                        &mut draft,
                        action,
                        outcome,
                        Vec::new(),
                        Vec::new(),
                        DiscoveryJsonUpdate::Preserve,
                    )?;
                    return self.get(session_id);
                }
            };
            let completion_snapshot =
                self.inflight_completion_snapshot(&snapshot, &operation.id)?;
            self.persist_operation_completion(
                &completion_snapshot,
                &operation.id,
                &mut draft,
                completion.action,
                completion.outcome,
                completion.evidence,
                completion.candidates,
                completion.review,
            )?;
        }
        Err(CoreError::internal(
            "provider discovery exceeded its automatic transition bound",
        ))
    }

    pub(super) fn inflight_completion_snapshot(
        &self,
        started_snapshot: &DiscoverySessionSnapshot,
        operation_id: &DiscoveryOperationId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let latest = self.get(&started_snapshot.session.id)?;
        if latest.session.revision == started_snapshot.session.revision {
            return Ok(latest);
        }
        let current_operation = self
            .storage
            .get_current_discovery_operation(&started_snapshot.session.id)?;
        if latest.session.cancellation_pending
            && latest.session.state == started_snapshot.session.state
            && current_operation.as_ref().is_some_and(|operation| {
                operation.id == *operation_id
                    && operation.status == DiscoveryOperationStatus::Started
            })
        {
            // RequestCancellation deliberately advances the durable revision
            // while the same operation remains active. Complete against that
            // exact cancellation snapshot so the domain transition settles to
            // Cancelled or UnknownOutcome instead of losing the cancellation
            // to a stale-revision write.
            return Ok(latest);
        }
        Err(CoreError::internal(
            "provider discovery changed while its operation was in flight",
        ))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn persist_operation_completion(
        &self,
        snapshot: &DiscoverySessionSnapshot,
        operation_id: &DiscoveryOperationId,
        draft: &mut DiscoveryWorkingDraft,
        action: ProviderDiscoveryAction,
        outcome: DurableOperationOutcome,
        evidence: Vec<DiscoveryEvidenceRecord>,
        candidates: Vec<StoredDiscoveryCandidate>,
        review: DiscoveryJsonUpdate<DiscoveryReviewDiff>,
    ) -> CoreResult<()> {
        let write = Self::operation_completion_write(
            snapshot,
            operation_id,
            draft,
            action,
            outcome,
            evidence,
            candidates,
            review,
        )?;
        self.storage.persist_discovery_transition(&write)?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn operation_completion_write(
        snapshot: &DiscoverySessionSnapshot,
        operation_id: &DiscoveryOperationId,
        draft: &mut DiscoveryWorkingDraft,
        action: ProviderDiscoveryAction,
        outcome: DurableOperationOutcome,
        evidence: Vec<DiscoveryEvidenceRecord>,
        candidates: Vec<StoredDiscoveryCandidate>,
        review: DiscoveryJsonUpdate<DiscoveryReviewDiff>,
    ) -> CoreResult<DiscoveryTransitionWrite> {
        let envelope = provider_discovery_action_envelope(
            DiscoveryActionId::new(),
            snapshot.session.revision,
            action,
        )?;
        let transition = snapshot
            .session
            .apply(&envelope)
            .map_err(transition_error)?;
        if transition.session.state.is_terminal() {
            cancel_assistant_snapshot(draft)?;
        }
        let new_operation_id =
            operation_for_effect(&transition.effect).map(|_| DiscoveryOperationId::new());
        Ok(DiscoveryTransitionWrite {
            transition,
            draft: DiscoveryJsonUpdate::Replace(working_draft_value(draft)?),
            review,
            new_evidence: evidence,
            new_candidates: candidates,
            approval: None,
            new_operation_id,
            completed_operation: Some(DiscoveryCompletedOperationWrite {
                id: operation_id.clone(),
                outcome,
            }),
            prepared_commit: None,
            provider_graph: None,
            occurred_at: Utc::now(),
        })
    }
}

pub(super) struct EffectCompletion {
    pub(super) action: ProviderDiscoveryAction,
    pub(super) evidence: Vec<DiscoveryEvidenceRecord>,
    pub(super) candidates: Vec<StoredDiscoveryCandidate>,
    pub(super) review: DiscoveryJsonUpdate<DiscoveryReviewDiff>,
    pub(super) outcome: DurableOperationOutcome,
}

impl EffectCompletion {
    pub(super) fn simple(action: ProviderDiscoveryAction) -> Self {
        Self {
            action,
            evidence: Vec::new(),
            candidates: Vec::new(),
            review: DiscoveryJsonUpdate::Preserve,
            outcome: DurableOperationOutcome::Succeeded,
        }
    }
}

fn nonpersistent_failure_action(
    operation: DiscoveryOperationKind,
    error: &CoreError,
) -> (ProviderDiscoveryAction, DurableOperationOutcome) {
    if error.recoverable
        || matches!(
            error.code,
            CoreErrorCode::ProviderAuthFailed
                | CoreErrorCode::ProviderRateLimited
                | CoreErrorCode::ProviderUnavailable
                | CoreErrorCode::NetworkUnavailable
                | CoreErrorCode::Cancelled
                | CoreErrorCode::StorageUnavailable
        )
    {
        (
            ProviderDiscoveryAction::Interrupt {
                operation,
                outcome: DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
            },
            DurableOperationOutcome::Interrupted,
        )
    } else {
        (
            ProviderDiscoveryAction::Fail {
                failure: DiscoveryFailure {
                    code: error.code.as_str().to_owned(),
                    message_key: "provider.discovery.operation_failed".to_owned(),
                    recoverable: false,
                },
            },
            DurableOperationOutcome::Failed,
        )
    }
}

pub(super) fn transition_error(error: impl std::fmt::Display) -> CoreError {
    CoreError::invalid(format!(
        "provider discovery transition was rejected: {error}"
    ))
}

pub(super) fn working_draft_value(draft: &DiscoveryWorkingDraft) -> CoreResult<Value> {
    serde_json::to_value(draft)
        .map_err(|_| CoreError::internal("provider discovery draft could not be serialized"))
}

pub(super) fn hydrate_working_draft(
    snapshot: &DiscoverySessionSnapshot,
) -> CoreResult<DiscoveryWorkingDraft> {
    let value = snapshot
        .draft_json
        .clone()
        .ok_or_else(|| CoreError::internal("provider discovery draft is missing"))?;
    let draft = serde_json::from_value::<DiscoveryWorkingDraft>(value).map_err(|_| {
        CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "provider discovery draft is invalid",
            false,
        )
    })?;
    if draft.schema_version != WORKING_DRAFT_SCHEMA_VERSION {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "provider discovery draft version is unsupported",
            false,
        ));
    }
    Ok(draft)
}

pub(super) fn operation_for_effect(effect: &DiscoveryEffect) -> Option<DiscoveryOperationKind> {
    match effect {
        DiscoveryEffect::ResolveKnownProvider => Some(DiscoveryOperationKind::ResolveKnownProvider),
        DiscoveryEffect::FetchDocuments => Some(DiscoveryOperationKind::FetchDocuments),
        DiscoveryEffect::ExtractEvidence => Some(DiscoveryOperationKind::ExtractEvidence),
        DiscoveryEffect::BuildDeterministicManifestDraft => {
            Some(DiscoveryOperationKind::BuildDeterministicManifestDraft)
        }
        DiscoveryEffect::BuildAssistantManifestDraft { .. } => {
            Some(DiscoveryOperationKind::BuildAssistantManifestDraft)
        }
        DiscoveryEffect::ValidateManifest => Some(DiscoveryOperationKind::ValidateManifest),
        DiscoveryEffect::ListModels => Some(DiscoveryOperationKind::ListModels),
        DiscoveryEffect::ProbeCapabilities { .. } => {
            Some(DiscoveryOperationKind::ProbeCapabilities)
        }
        DiscoveryEffect::CommitAtomically { .. } => Some(DiscoveryOperationKind::AtomicCommit),
        DiscoveryEffect::RunCompensation { .. } => Some(DiscoveryOperationKind::Compensation),
        DiscoveryEffect::None | DiscoveryEffect::RequestCancellation { .. } => None,
    }
}
