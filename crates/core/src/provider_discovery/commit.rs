use super::{
    CoreError, CoreErrorCode, CoreResult, DiscoveredProviderGraph, DiscoveryActionId,
    DiscoveryCommitAttemptId, DiscoveryCommitPhase, DiscoveryCommitPlan,
    DiscoveryCompletedOperationWrite, DiscoveryJsonUpdate, DiscoveryOperationId,
    DiscoveryOperationKind, DiscoveryOperationStatus, DiscoveryReviewDiff, DiscoverySessionId,
    DiscoverySessionSnapshot, DiscoveryState, DiscoveryTransitionWrite, DiscoveryWorkingDraft,
    DurableOperationOutcome, ProviderConnection, ProviderDiscoveryAction,
    ProviderDiscoveryCredentialCommitConfirmation, ProviderDiscoveryOrchestrator, Storage, Utc,
    hydrate_working_draft, provider_discovery_action_envelope,
    require_active_discovery_network_authority, revalidate_discovery_catalog_authority,
    revalidate_prepared_discovery_catalog_authority, sanitized_graph_sha256, transition_error,
    validate_manifest, working_draft_value,
};

impl ProviderDiscoveryOrchestrator<'_> {
    /// Executes the already-approved atomic graph publication. For a graph
    /// carrying an opaque native credential reference, the caller must confirm
    /// that the reference exists in the native vault; the raw credential is
    /// never accepted here.
    pub fn commit(
        &self,
        session_id: &DiscoverySessionId,
        credential_confirmation: Option<&ProviderDiscoveryCredentialCommitConfirmation>,
    ) -> CoreResult<ProviderConnection> {
        let snapshot = self.get(session_id)?;
        require_active_discovery_commit_authority(&snapshot)?;
        let operation_id = snapshot
            .active_operation_id
            .clone()
            .ok_or_else(|| CoreError::internal("committing discovery has no active operation"))?;
        let mut draft = hydrate_working_draft(&snapshot)?;
        let attempt_id =
            snapshot.session.commit_attempt_id.as_ref().ok_or_else(|| {
                CoreError::internal("committing discovery lost its commit attempt")
            })?;
        let attempt = self.storage.get_discovery_commit_attempt(attempt_id)?;
        revalidate_prepared_discovery_catalog_authority(self.storage, &draft, attempt.phase)?;
        let graph = graph_from_plan(&draft, attempt.plan, attempt.plan_sha256)?;
        let credential_bound = graph.connection.credential_ref.is_some();
        if !credential_bound && credential_confirmation.is_some() {
            return Err(CoreError::invalid(
                "credentialless discovery cannot accept a native credential confirmation",
            ));
        }
        if credential_bound {
            self.require_commit_operation_started(&snapshot, &operation_id)?;
        } else {
            self.ensure_commit_operation_started(&snapshot, &operation_id)?;
        }
        if snapshot.session.cancellation_pending {
            self.settle_started_commit_cancellation(&snapshot, &operation_id)?;
            return Err(cancelled_commit_error());
        }
        if credential_bound {
            self.require_exact_credential_commit_confirmation(session_id, credential_confirmation)?;
        }

        let current = self.get(session_id)?;
        if current.session.state != DiscoveryState::Committing {
            return Err(CoreError::invalid(
                "provider discovery changed while the atomic commit was starting",
            ));
        }
        if current.session.cancellation_pending {
            self.settle_started_commit_cancellation(&current, &operation_id)?;
            return Err(cancelled_commit_error());
        }
        let envelope = provider_discovery_action_envelope(
            DiscoveryActionId::new(),
            current.session.revision,
            ProviderDiscoveryAction::CommitSucceeded {
                connection_id: graph.connection.id.clone(),
            },
        )?;
        let transition = current.session.apply(&envelope).map_err(transition_error)?;
        let write = DiscoveryTransitionWrite {
            transition,
            draft: DiscoveryJsonUpdate::Replace(working_draft_value(&draft)?),
            review: DiscoveryJsonUpdate::Preserve,
            new_evidence: Vec::new(),
            new_candidates: Vec::new(),
            approval: None,
            new_operation_id: None,
            completed_operation: Some(DiscoveryCompletedOperationWrite {
                id: operation_id.clone(),
                outcome: DurableOperationOutcome::Succeeded,
            }),
            prepared_commit: None,
            provider_graph: Some(graph.clone()),
            occurred_at: Utc::now(),
        };
        let persisted = if graph.connection.credential_ref.is_none() {
            self.storage.persist_discovery_transition(&write)
        } else {
            self.storage
                .persist_credential_confirmed_discovery_commit(&write)
        };
        if let Err(error) = persisted {
            let latest = self.get(session_id)?;
            if latest.session.state == DiscoveryState::Committing
                && latest.session.cancellation_pending
            {
                self.settle_started_commit_cancellation(&latest, &operation_id)?;
                return Err(cancelled_commit_error());
            }
            return Err(error);
        }
        let ready = self.get(session_id)?;
        if !matches!(
            ready.session.state,
            DiscoveryState::Ready | DiscoveryState::Compensating
        ) {
            return Err(CoreError::internal(
                "provider discovery commit reached neither ready nor compensation",
            ));
        }
        draft
            .connection
            .take()
            .ok_or_else(|| CoreError::internal("committed discovery lost its provider connection"))
    }

    fn require_exact_credential_commit_confirmation(
        &self,
        session_id: &DiscoverySessionId,
        confirmation: Option<&ProviderDiscoveryCredentialCommitConfirmation>,
    ) -> CoreResult<()> {
        let confirmation = confirmation.ok_or_else(|| {
            CoreError::invalid("native credential reference confirmation is required")
        })?;
        let current_context = self.credential_install_context(session_id)?;
        let expected_confirmation =
            ProviderDiscoveryCredentialCommitConfirmation::try_from(&current_context)?;
        if confirmation != &expected_confirmation
            || current_context.operation_status != DiscoveryOperationStatus::Started
            || current_context.commit_phase != DiscoveryCommitPhase::Prepared
        {
            return Err(CoreError::invalid(
                "native credential confirmation does not match the active commit operation",
            ));
        }
        Ok(())
    }

    fn ensure_commit_operation_started(
        &self,
        snapshot: &DiscoverySessionSnapshot,
        operation_id: &DiscoveryOperationId,
    ) -> CoreResult<()> {
        if self
            .storage
            .mark_discovery_operation_started(operation_id, Utc::now())?
        {
            return Ok(());
        }
        let operation = self
            .storage
            .get_current_discovery_operation(&snapshot.session.id)?
            .ok_or_else(|| CoreError::invalid("atomic discovery commit operation disappeared"))?;
        if operation.id == *operation_id && operation.status == DiscoveryOperationStatus::Started {
            Ok(())
        } else {
            Err(CoreError::invalid(
                "atomic discovery commit already completed or changed",
            ))
        }
    }

    fn require_commit_operation_started(
        &self,
        snapshot: &DiscoverySessionSnapshot,
        operation_id: &DiscoveryOperationId,
    ) -> CoreResult<()> {
        let operation = self
            .storage
            .get_current_discovery_operation(&snapshot.session.id)?
            .ok_or_else(|| CoreError::invalid("atomic discovery commit operation disappeared"))?;
        if snapshot.active_operation_id.as_ref() == Some(operation_id)
            && operation.id == *operation_id
            && operation.kind == DiscoveryOperationKind::AtomicCommit
            && operation.status == DiscoveryOperationStatus::Started
        {
            Ok(())
        } else {
            Err(CoreError::invalid(
                "credential-bound discovery commit was not explicitly started",
            ))
        }
    }

    fn settle_started_commit_cancellation(
        &self,
        snapshot: &DiscoverySessionSnapshot,
        operation_id: &DiscoveryOperationId,
    ) -> CoreResult<()> {
        if snapshot.session.state != DiscoveryState::Committing
            || !snapshot.session.cancellation_pending
        {
            return Err(CoreError::invalid(
                "atomic discovery commit has no pending cancellation",
            ));
        }
        let mut draft = hydrate_working_draft(snapshot)?;
        self.persist_operation_completion(
            snapshot,
            operation_id,
            &mut draft,
            ProviderDiscoveryAction::CompensationRequired,
            DurableOperationOutcome::Failed,
            Vec::new(),
            Vec::new(),
            DiscoveryJsonUpdate::Preserve,
        )?;
        self.continue_compensation(&snapshot.session.id)?;
        Ok(())
    }
}

pub(super) fn commit_plan_for(
    storage: &Storage,
    snapshot: &DiscoverySessionSnapshot,
    draft: &DiscoveryWorkingDraft,
    attempt_id: DiscoveryCommitAttemptId,
    review: &DiscoveryReviewDiff,
) -> CoreResult<DiscoveryCommitPlan> {
    revalidate_discovery_catalog_authority(storage, draft, Utc::now())?;
    let template = draft
        .template
        .as_ref()
        .ok_or_else(|| CoreError::internal("commit plan has no template"))?;
    let connection = draft
        .connection
        .as_ref()
        .ok_or_else(|| CoreError::internal("commit plan has no connection"))?;
    let manifest_sha256 = validate_manifest(&template.default_manifest)?
        .sha256()
        .to_owned();
    let graph_sha256 = sanitized_graph_sha256(draft)?;
    if review.graph_sha256 != graph_sha256 {
        return Err(CoreError::invalid(
            "persisted review does not match the sanitized provider graph",
        ));
    }
    let plan = DiscoveryCommitPlan {
        attempt_id,
        session_id: snapshot.session.id.clone(),
        expected_revision: snapshot.session.revision,
        manifest_sha256,
        graph_sha256,
        template_id: template.id.clone(),
        template_version: template.manifest_version,
        connection_id: connection.id.clone(),
        model_route_ids: draft.routes.iter().map(|route| route.id.clone()).collect(),
        credential_ref: connection.credential_ref.clone(),
        credential_approval_id: draft.credential_approval_id.clone(),
        review_sha256: review.sha256.clone(),
        catalog_authority: draft.catalog_authority.clone(),
        previous_selection: storage.current_discovery_previous_selection()?,
    };
    plan.validate()
        .map_err(|error| CoreError::invalid(format!("invalid discovery commit plan: {error}")))?;
    Ok(plan)
}

fn graph_from_plan(
    draft: &DiscoveryWorkingDraft,
    plan: DiscoveryCommitPlan,
    plan_sha256: String,
) -> CoreResult<DiscoveredProviderGraph> {
    let graph = DiscoveredProviderGraph {
        plan,
        plan_sha256,
        template: draft
            .template
            .clone()
            .ok_or_else(|| CoreError::internal("commit graph has no template"))?,
        connection: draft
            .connection
            .clone()
            .ok_or_else(|| CoreError::internal("commit graph has no connection"))?,
        routes: draft.routes.clone(),
        observations: draft.observations.clone(),
        presets: draft.presets.clone(),
    };
    if graph.ownership_sha256()? != graph.plan.graph_sha256 {
        return Err(CoreError::invalid(
            "provider graph changed after review approval",
        ));
    }
    Ok(graph)
}

impl crate::app::Core {
    pub fn commit_provider_discovery(
        &self,
        session_id: &DiscoverySessionId,
        credential_confirmation: Option<&ProviderDiscoveryCredentialCommitConfirmation>,
    ) -> CoreResult<ProviderConnection> {
        self.provider_discovery()
            .commit(session_id, credential_confirmation)
    }
}

fn cancelled_commit_error() -> CoreError {
    CoreError::new(
        CoreErrorCode::Cancelled,
        "provider discovery commit was cancelled before graph publication",
        false,
    )
}

fn require_active_discovery_commit_authority(
    snapshot: &DiscoverySessionSnapshot,
) -> CoreResult<()> {
    if snapshot.session.state != DiscoveryState::Committing {
        return Err(CoreError::invalid(
            "provider discovery is not awaiting an atomic commit",
        ));
    }
    require_active_discovery_network_authority(
        &snapshot.session.input.connection_options,
        Utc::now(),
    )
}
