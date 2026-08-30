//! Atomic discovery transition, begin-session, and operation-start persistence.

mod artifacts;

use super::{
    BTreeSet, CompletedDiscoveryOperation, CoreError, CoreErrorCode, CoreResult, DateTime,
    DiscoveryApprovalBinding, DiscoveryApprovalGrant, DiscoveryCommitAttemptId,
    DiscoveryCompensationKind, DiscoveryEvidenceRecord, DiscoveryJsonUpdate, DiscoveryOperationId,
    DiscoveryOperationKind, DiscoveryReviewDiff, DiscoveryState, DiscoveryTransitionWrite,
    DomainCompensationStatus, DurableDiscoveryEffect, DurableDiscoveryTransition,
    DurableOperationOutcome, NewDiscoveryApproval, NewDiscoveryCommitAttempt,
    NewDiscoveryCompensationStep, NewDiscoveryOperation, OptionalExtension,
    PersistDiscoveryTransition, PreparedDiscoveryCommit, ProviderCredentialAccessAuthority,
    ProviderDiscoverySession, ProviderNetworkMode, SanitizedDiscoveryInput, Storage,
    StoredDiscoveryCandidate, Transaction, TransactionBehavior, Utc, Value, append_audit,
    apply_provider_graph_in_transaction, approval_kind, candidate_kind, canonical_json_result,
    complete_commit_attempt_for_ready_transition, contract_error, corrupted, database_error,
    discovery, discovery_error, encode_approval_grant, encode_commit_plan_json, encode_json_result,
    encode_redacted_json, ensure_provider_credential_operation_settled_for_discovery,
    enum_wire_result, finalize_commit_failed_before_apply, load_discovery_previous_selection,
    load_operation_by_id, load_session_snapshot, params, parse_timestamp,
    project_reconciled_discovery_credential_ownership, reconcile_discovery_saga_ledger,
    require_session, sha256_hex, validate_approval_references, validate_atomic_discovery_begin,
    validate_candidate_evidence_references, validate_discovery_evidence, validate_identifier,
    validate_provider_credential_access_authority_in_transaction,
    validate_review_evidence_references, validate_sanitized_input,
    validate_terminal_compensation_transition, validate_transition_write,
};
use artifacts::{insert_candidate_in_transaction, insert_evidence_in_transaction};

impl Storage {
    pub fn mark_discovery_operation_started(
        &self,
        operation_id: &DiscoveryOperationId,
        started_at: DateTime<Utc>,
    ) -> CoreResult<bool> {
        let mut connection = self.connection()?;
        let operation = load_operation_by_id(&connection, operation_id)?;
        if started_at < operation.created_at {
            return Err(CoreError::invalid(
                "discovery operation cannot start before it was created",
            ));
        }
        discovery::mark_discovery_operation_started(
            &mut connection,
            operation_id.as_str(),
            &started_at.to_rfc3339(),
        )
        .map_err(discovery_error)
    }

    pub fn persist_discovery_transition(
        &self,
        write: &DiscoveryTransitionWrite,
    ) -> CoreResult<PersistDiscoveryTransition> {
        if write.completed_operation.as_ref().is_some_and(|completed| {
            completed.outcome == DurableOperationOutcome::AttestedNoExternalEffect
        }) {
            return Err(CoreError::invalid(
                "native no-effect completion requires its atomic attestation API",
            ));
        }
        if write
            .provider_graph
            .as_ref()
            .is_some_and(|graph| graph.plan.credential_ref.is_some())
        {
            return Err(CoreError::invalid(
                "credentialed provider graphs require native credential confirmation",
            ));
        }
        validate_transition_write(write)?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let authority_observed_at = Utc::now();
        let result =
            persist_transition_in_transaction(&transaction, write, Some(authority_observed_at))?;
        transaction.commit().map_err(database_error)?;
        Ok(result)
    }

    /// Creates the draft row and applies `Begin` in one `SQLite` transaction.
    ///
    /// This prevents a process crash from leaving an invisible draft without
    /// its first operation, action receipt, and outbox event.
    pub fn begin_discovery_session(
        &self,
        initial_session: &ProviderDiscoverySession,
        write: &DiscoveryTransitionWrite,
    ) -> CoreResult<PersistDiscoveryTransition> {
        self.begin_discovery_session_observed(initial_session, write, None, false)
    }

    /// Production admission boundary after a native credential read. If the
    /// intended identifier already names an active credential-bound
    /// connection, the exact durable read authority is compared in the same
    /// immediate transaction which creates the discovery session and its
    /// first operation/outbox event.
    pub fn begin_discovery_session_with_credential_authority(
        &self,
        initial_session: &ProviderDiscoverySession,
        write: &DiscoveryTransitionWrite,
        credential_authority: Option<&ProviderCredentialAccessAuthority>,
    ) -> CoreResult<PersistDiscoveryTransition> {
        self.begin_discovery_session_observed(initial_session, write, credential_authority, true)
    }

    fn begin_discovery_session_observed(
        &self,
        initial_session: &ProviderDiscoverySession,
        write: &DiscoveryTransitionWrite,
        credential_authority: Option<&ProviderCredentialAccessAuthority>,
        require_exact_credential_authority: bool,
    ) -> CoreResult<PersistDiscoveryTransition> {
        validate_atomic_discovery_begin(initial_session, write)?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let session_exists = transaction
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM provider_discovery_sessions WHERE id = ?1
                 )",
                [initial_session.id.as_str()],
                |row| row.get::<_, bool>(0),
            )
            .map_err(database_error)?;
        if session_exists {
            let stored = load_session_snapshot(&transaction, initial_session.id.as_str())?
                .ok_or_else(|| corrupted("existing discovery session disappeared"))?;
            if stored.session.input != initial_session.input
                || stored.created_at != write.occurred_at
                || (stored.session.revision == 0 && stored.session != *initial_session)
            {
                return Err(CoreError::invalid(
                    "existing discovery session does not match the atomic Begin request",
                ));
            }
        } else {
            ensure_provider_credential_operation_settled_for_discovery(
                &transaction,
                &initial_session.input.connection_id,
            )?;
            if require_exact_credential_authority {
                let active_connection_exists = transaction
                    .query_row(
                        "SELECT EXISTS(
                           SELECT 1
                           FROM provider_connections
                           WHERE id = ?1 AND archived_at IS NULL
                         )",
                        [initial_session.input.connection_id.as_str()],
                        |row| row.get::<_, bool>(0),
                    )
                    .map_err(database_error)?;
                if active_connection_exists {
                    validate_provider_credential_access_authority_in_transaction(
                        &transaction,
                        &initial_session.input.connection_id,
                        credential_authority,
                    )?;
                } else if credential_authority.is_some() {
                    return Err(CoreError::invalid(
                        "credential authority was supplied for a new provider discovery",
                    ));
                }
            }
            insert_session_in_transaction(&transaction, initial_session, write.occurred_at)?;
        }
        let result = persist_transition_in_transaction(&transaction, write, None)?;
        transaction.commit().map_err(database_error)?;
        Ok(result)
    }
}

#[allow(clippy::too_many_lines)]
pub(super) fn persist_transition_in_transaction(
    transaction: &Transaction<'_>,
    write: &DiscoveryTransitionWrite,
    publication_authority_observed_at: Option<DateTime<Utc>>,
) -> CoreResult<PersistDiscoveryTransition> {
    let transition = &write.transition;
    let session_id = transition.session.id.as_str();
    let (stored_draft, stored_review) = transaction
        .query_row(
            "SELECT draft_json, review_diff_json
             FROM provider_discovery_sessions
             WHERE id = ?1",
            [session_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "provider discovery session was not found",
                false,
            )
        })?;

    let draft_json = resolve_draft_update(&write.draft, stored_draft)?;
    let review_json = resolve_review_update(&write.review, stored_review)?;
    let error_json = transition
        .session
        .failure
        .as_ref()
        .map(|failure| encode_json_result(serde_json::to_value(failure), "discovery failure"))
        .transpose()?;
    let recovery_json = transition
        .session
        .recovery
        .as_ref()
        .map(|recovery| {
            encode_json_result(
                serde_json::to_value(recovery),
                "discovery recovery checkpoint",
            )
        })
        .transpose()?;
    let state = enum_wire_result(
        serde_json::to_value(transition.session.state),
        "discovery state",
    )?;
    let unknown_operation = transition
        .session
        .unknown_operation
        .map(|operation| {
            enum_wire_result(
                serde_json::to_value(operation),
                "discovery unknown operation",
            )
        })
        .transpose()?;
    let event_json =
        encode_json_result(serde_json::to_value(&transition.event), "discovery event")?;
    let response_json = encode_json_result(
        serde_json::to_value(transition),
        "discovery action response",
    )?;
    let receipt_outcome = enum_wire_result(
        serde_json::to_value(transition.receipt.outcome),
        "discovery receipt outcome",
    )?;
    let occurred_at = write.occurred_at.to_rfc3339();

    let approval_json = write
        .approval
        .as_ref()
        .map(|approval| encode_approval_grant(&approval.grant))
        .transpose()?;
    let approval_kind = write
        .approval
        .as_ref()
        .map(|approval| approval_kind(&approval.grant));
    let approval_decision = write
        .approval
        .as_ref()
        .map(|approval| {
            enum_wire_result(
                serde_json::to_value(approval.decision),
                "discovery approval decision",
            )
        })
        .transpose()?;
    let approval_candidate_id =
        write
            .approval
            .as_ref()
            .and_then(|approval| match &approval.grant {
                DiscoveryApprovalGrant::TemplateSelection { candidate_id } => {
                    Some(candidate_id.as_str())
                }
                _ => None,
            });
    let approval = write.approval.as_ref().map(|record| NewDiscoveryApproval {
        id: record.id.as_str(),
        approval_kind: approval_kind.expect("approval kind exists with record"),
        candidate_id: approval_candidate_id,
        decision: approval_decision
            .as_deref()
            .expect("approval decision exists with record"),
        grant_json: approval_json
            .as_deref()
            .expect("approval JSON exists with record"),
    });

    let (durable_effect, operation_kind, operation_approval) =
        map_discovery_effect(&transition.effect);
    let operation_kind_wire = operation_kind
        .map(|kind| enum_wire_result(serde_json::to_value(kind), "discovery operation kind"))
        .transpose()?;
    let side_effect_wire = operation_kind
        .map(|kind| {
            enum_wire_result(
                serde_json::to_value(kind.side_effect_class()),
                "discovery side-effect class",
            )
        })
        .transpose()?;
    let operation = write
        .new_operation_id
        .as_ref()
        .map(|operation_id| NewDiscoveryOperation {
            id: operation_id.as_str(),
            operation_kind: operation_kind_wire
                .as_deref()
                .expect("operation kind exists with operation id"),
            side_effect_class: side_effect_wire
                .as_deref()
                .expect("side-effect class exists with operation id"),
            approval_id: operation_approval.map(|binding| binding.approval_id.as_str()),
            approval_grant_sha256: operation_approval.map(|binding| binding.grant_sha256.as_str()),
        });
    let completed_operation =
        write
            .completed_operation
            .as_ref()
            .map(|completed| CompletedDiscoveryOperation {
                id: completed.id.as_str(),
                outcome: completed.outcome,
            });

    let prepared_plan_json = write
        .prepared_commit
        .as_ref()
        .map(|commit| encode_commit_plan_json(&commit.plan))
        .transpose()?;
    let prepared_steps_json = write
        .prepared_commit
        .as_ref()
        .map(|commit| {
            commit
                .compensation_steps
                .iter()
                .map(|step| {
                    encode_json_result(
                        serde_json::to_value(&step.step),
                        "discovery compensation step",
                    )
                })
                .collect::<CoreResult<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
    let prepared_step_kinds = write
        .prepared_commit
        .as_ref()
        .map(|commit| {
            commit
                .compensation_steps
                .iter()
                .map(|step| {
                    enum_wire_result(
                        serde_json::to_value(step.step.kind),
                        "discovery compensation kind",
                    )
                })
                .collect::<CoreResult<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
    let prepared_steps = write
        .prepared_commit
        .as_ref()
        .map(|commit| {
            commit
                .compensation_steps
                .iter()
                .zip(prepared_steps_json.iter())
                .zip(prepared_step_kinds.iter())
                .map(
                    |((step, step_json), step_kind)| NewDiscoveryCompensationStep {
                        id: &step.id,
                        ordinal: step.step.ordinal,
                        action_id: step.step.action_id.as_str(),
                        step_kind,
                        step_json,
                    },
                )
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let prepared_commit = write
        .prepared_commit
        .as_ref()
        .map(|commit| NewDiscoveryCommitAttempt {
            id: commit.plan.attempt_id.as_str(),
            attempt_number: commit.attempt_number,
            plan_sha256: &commit.plan_sha256,
            plan_json: prepared_plan_json
                .as_deref()
                .expect("commit JSON exists with commit"),
            reuse_existing: commit.reuse_existing,
            compensation_steps: &prepared_steps,
        });

    let durable = DurableDiscoveryTransition {
        session_id,
        expected_revision: transition.previous_revision,
        resulting_revision: transition.session.revision,
        event_sequence: transition.event.sequence,
        next_event_sequence: transition.session.next_event_sequence,
        state: &state,
        draft_json: draft_json.as_deref(),
        review_diff_json: review_json.as_deref(),
        error_json: error_json.as_deref(),
        recovery_json: recovery_json.as_deref(),
        unknown_operation: unknown_operation.as_deref(),
        manifest_sha256: transition.session.manifest_sha256.as_deref(),
        commit_plan_sha256: transition.session.commit_plan_sha256.as_deref(),
        commit_attempt_id: transition
            .session
            .commit_attempt_id
            .as_ref()
            .map(DiscoveryCommitAttemptId::as_str),
        committed_connection_id: transition
            .session
            .committed_connection_id
            .as_ref()
            .map(lorepia_domain::ProviderConnectionId::as_str),
        cancellation_pending: transition.session.cancellation_pending,
        event_id: transition.event.id.as_str(),
        event_version: transition.event.version,
        event_json: &event_json,
        effect: durable_effect,
        action_id: transition.receipt.action_id.as_str(),
        action_kind: &transition.receipt.action_kind,
        action_approval_id: write.approval.as_ref().map(|record| record.id.as_str()),
        request_sha256: &transition.receipt.request_sha256,
        response_json: &response_json,
        receipt_outcome: &receipt_outcome,
        audit_kind: audit_kind_for_action(&transition.receipt.action_kind),
        audit_summary_key: "discovery.audit.transition_applied",
        occurred_at: &occurred_at,
        operation,
        completed_operation,
        approval,
        commit: prepared_commit,
    };
    let receipt_exists = transaction
        .query_row(
            "SELECT EXISTS(
                 SELECT 1
                 FROM provider_discovery_action_receipts
                 WHERE action_id = ?1
             )",
            [transition.receipt.action_id.as_str()],
            |row| row.get::<_, bool>(0),
        )
        .map_err(database_error)?;
    if receipt_exists {
        return discovery::persist_discovery_transition_in_transaction(transaction, &durable)
            .map_err(discovery_error);
    }
    validate_completed_operation_binding(transaction, write)?;
    for evidence in &write.new_evidence {
        insert_evidence_in_transaction(transaction, evidence)?;
    }
    for candidate in &write.new_candidates {
        insert_candidate_in_transaction(transaction, candidate, transition.previous_revision)?;
    }
    if let DiscoveryJsonUpdate::Replace(review) = &write.review {
        validate_review_evidence_references(transaction, &transition.session.id, review)?;
    }
    if let Some(approval) = &write.approval {
        validate_approval_references(transaction, approval)?;
    }
    if let Some(commit) = &write.prepared_commit {
        validate_prepared_commit_session_binding(transaction, commit)?;
    }
    if let Some(graph) = &write.provider_graph {
        let authority_observed_at = publication_authority_observed_at.ok_or_else(|| {
            CoreError::internal(
                "provider graph publication has no transaction-scoped authority observation",
            )
        })?;
        apply_provider_graph_in_transaction(
            transaction,
            graph,
            transition.previous_revision,
            write.occurred_at,
            authority_observed_at,
        )?;
    }
    finalize_commit_failed_before_apply(transaction, write)?;
    reconcile_discovery_saga_ledger(transaction, write)?;
    validate_terminal_compensation_transition(transaction, write)?;
    let result = discovery::persist_discovery_transition_in_transaction(transaction, &durable)
        .map_err(discovery_error)?;
    if transition.session.state == DiscoveryState::Ready {
        complete_commit_attempt_for_ready_transition(transaction, transition, write.occurred_at)?;
        project_reconciled_discovery_credential_ownership(
            transaction,
            transition,
            write.occurred_at,
        )?;
    }
    Ok(result)
}

pub(super) fn validate_completed_operation_binding(
    transaction: &Transaction<'_>,
    write: &DiscoveryTransitionWrite,
) -> CoreResult<()> {
    let active_operation_id = transaction
        .query_row(
            "SELECT active_operation_id
             FROM provider_discovery_sessions
             WHERE id = ?1",
            [write.transition.session.id.as_str()],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "provider discovery session was not found",
                false,
            )
        })?;
    let action_kind = write.transition.receipt.action_kind.as_str();
    let expected_outcome = match action_kind {
        "known_provider_candidates_resolved"
        | "documents_fetched"
        | "evidence_extracted"
        | "manifest_draft_built"
        | "assistant_requested_more_evidence"
        | "assistant_resumed_with_evidence"
        | "manifest_validated"
        | "models_listed"
        | "probes_completed"
        | "commit_succeeded"
        | "compensation_succeeded" => Some(DurableOperationOutcome::Succeeded),
        "fail" | "commit_failed_before_apply" | "compensation_required" | "compensation_failed" => {
            active_operation_id
                .as_ref()
                .map(|_| DurableOperationOutcome::Failed)
        }
        "external_outcome_became_unknown" => Some(DurableOperationOutcome::OutcomeUnknown),
        "interrupt" => Some(
            if write.transition.session.state == DiscoveryState::UnknownOutcome {
                DurableOperationOutcome::OutcomeUnknown
            } else {
                DurableOperationOutcome::Interrupted
            },
        ),
        _ => None,
    };
    let completed = match (
        active_operation_id.as_deref(),
        expected_outcome,
        write.completed_operation.as_ref(),
    ) {
        (None | Some(_), None, None) => return Ok(()),
        (Some(active_id), Some(expected), Some(completed))
            if completed.id.as_str() == active_id
                && (completed.outcome == expected
                    || (expected == DurableOperationOutcome::Interrupted
                        && completed.outcome
                            == DurableOperationOutcome::AttestedNoExternalEffect)) =>
        {
            completed
        }
        _ => {
            return Err(CoreError::invalid(
                "completed discovery operation does not match the domain action outcome",
            ));
        }
    };
    let (created_at, started_at) = transaction
        .query_row(
            "SELECT created_at, started_at
             FROM provider_discovery_operations
             WHERE id = ?1",
            [completed.id.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| corrupted("completed discovery operation is missing"))?;
    let created_at = parse_timestamp(&created_at, "discovery operation created_at")?;
    let started_at = started_at
        .as_deref()
        .map(|value| parse_timestamp(value, "discovery operation started_at"))
        .transpose()?;
    if write.occurred_at < created_at || started_at.is_some_and(|time| write.occurred_at < time) {
        return Err(CoreError::invalid(
            "discovery operation cannot finish before it was created or started",
        ));
    }
    Ok(())
}

fn resolve_draft_update(
    update: &DiscoveryJsonUpdate<Value>,
    stored: Option<String>,
) -> CoreResult<Option<String>> {
    match update {
        DiscoveryJsonUpdate::Preserve => Ok(stored),
        DiscoveryJsonUpdate::Clear => Ok(None),
        DiscoveryJsonUpdate::Replace(value) => {
            encode_redacted_json(value, "discovery draft").map(Some)
        }
    }
}

fn resolve_review_update(
    update: &DiscoveryJsonUpdate<DiscoveryReviewDiff>,
    stored: Option<String>,
) -> CoreResult<Option<String>> {
    match update {
        DiscoveryJsonUpdate::Preserve => Ok(stored),
        DiscoveryJsonUpdate::Clear => Ok(None),
        DiscoveryJsonUpdate::Replace(review) => {
            review.validate().map_err(contract_error)?;
            encode_json_result(serde_json::to_value(review), "discovery review").map(Some)
        }
    }
}

pub(super) fn map_discovery_effect(
    effect: &lorepia_domain::discovery::DiscoveryEffect,
) -> (
    DurableDiscoveryEffect,
    Option<DiscoveryOperationKind>,
    Option<&DiscoveryApprovalBinding>,
) {
    use lorepia_domain::discovery::DiscoveryEffect;
    match effect {
        DiscoveryEffect::None => (DurableDiscoveryEffect::None, None, None),
        DiscoveryEffect::ResolveKnownProvider => (
            DurableDiscoveryEffect::ResolveKnownProvider,
            Some(DiscoveryOperationKind::ResolveKnownProvider),
            None,
        ),
        DiscoveryEffect::FetchDocuments => (
            DurableDiscoveryEffect::FetchDocuments,
            Some(DiscoveryOperationKind::FetchDocuments),
            None,
        ),
        DiscoveryEffect::ExtractEvidence => (
            DurableDiscoveryEffect::ExtractEvidence,
            Some(DiscoveryOperationKind::ExtractEvidence),
            None,
        ),
        DiscoveryEffect::BuildDeterministicManifestDraft => (
            DurableDiscoveryEffect::BuildDeterministicManifestDraft,
            Some(DiscoveryOperationKind::BuildDeterministicManifestDraft),
            None,
        ),
        DiscoveryEffect::BuildAssistantManifestDraft { approval } => (
            DurableDiscoveryEffect::BuildAssistantManifestDraft,
            Some(DiscoveryOperationKind::BuildAssistantManifestDraft),
            Some(approval),
        ),
        DiscoveryEffect::ValidateManifest => (
            DurableDiscoveryEffect::ValidateManifest,
            Some(DiscoveryOperationKind::ValidateManifest),
            None,
        ),
        DiscoveryEffect::ListModels => (
            DurableDiscoveryEffect::ListModels,
            Some(DiscoveryOperationKind::ListModels),
            None,
        ),
        DiscoveryEffect::ProbeCapabilities { approval } => (
            DurableDiscoveryEffect::ProbeCapabilities,
            Some(DiscoveryOperationKind::ProbeCapabilities),
            Some(approval),
        ),
        DiscoveryEffect::RequestCancellation { .. } => {
            (DurableDiscoveryEffect::RequestCancellation, None, None)
        }
        DiscoveryEffect::CommitAtomically { .. } => (
            DurableDiscoveryEffect::CommitAtomically,
            Some(DiscoveryOperationKind::AtomicCommit),
            None,
        ),
        DiscoveryEffect::RunCompensation { .. } => (
            DurableDiscoveryEffect::RunCompensation,
            Some(DiscoveryOperationKind::Compensation),
            None,
        ),
    }
}

pub(super) fn audit_kind_for_action(action_kind: &str) -> &'static str {
    match action_kind {
        "resolve_unknown_outcome" => "unknown_outcome_reconciled",
        "compensation_required" => "compensation_started",
        _ => "transition_applied",
    }
}

pub(super) fn validate_prepared_commit(write: &DiscoveryTransitionWrite) -> CoreResult<()> {
    let Some(commit) = &write.prepared_commit else {
        return Ok(());
    };
    commit.plan.validate().map_err(contract_error)?;
    if commit.attempt_number == 0
        || commit.plan.session_id != write.transition.session.id
        || (!commit.reuse_existing
            && commit.plan.expected_revision != write.transition.previous_revision)
        || (commit.reuse_existing
            && commit.plan.expected_revision > write.transition.previous_revision)
        || write.transition.session.commit_attempt_id.as_ref() != Some(&commit.plan.attempt_id)
        || write.transition.session.commit_plan_sha256.as_deref()
            != Some(commit.plan_sha256.as_str())
    {
        return Err(CoreError::invalid(
            "prepared discovery commit does not match its transition",
        ));
    }
    let plan_json = encode_commit_plan_json(&commit.plan)?;
    if sha256_hex(plan_json.as_bytes()) != commit.plan_sha256 {
        return Err(CoreError::invalid(
            "discovery commit plan hash does not match its canonical plan",
        ));
    }
    if commit.reuse_existing {
        if !commit.compensation_steps.is_empty() {
            return Err(CoreError::invalid(
                "reused discovery commits must reuse their stored compensation recipe",
            ));
        }
        return Ok(());
    }
    let mut ids = BTreeSet::new();
    let mut ordinals = BTreeSet::new();
    let mut action_ids = BTreeSet::new();
    let mut credential_steps = 0_usize;
    let mut graph_steps = 0_usize;
    let mut selection_steps = 0_usize;
    for step in &commit.compensation_steps {
        validate_identifier("compensation step id", &step.id, 128)?;
        step.step
            .validate_against(&commit.plan)
            .map_err(contract_error)?;
        if step.step.status != DomainCompensationStatus::Pending
            || !ids.insert(step.id.as_str())
            || !ordinals.insert(step.step.ordinal)
            || !action_ids.insert(step.step.action_id.as_str())
        {
            return Err(CoreError::invalid(
                "prepared compensation steps must be unique pending steps",
            ));
        }
        match step.step.kind {
            DiscoveryCompensationKind::RemoveCredentialSlot => credential_steps += 1,
            DiscoveryCompensationKind::RemoveConnectionGraph => graph_steps += 1,
            DiscoveryCompensationKind::RestorePreviousSelection => selection_steps += 1,
        }
    }
    let expected_ordinals = (0..u32::try_from(commit.compensation_steps.len())
        .map_err(|_| CoreError::invalid("discovery compensation recipe is too large"))?)
        .collect::<BTreeSet<_>>();
    if ordinals != expected_ordinals
        || graph_steps != 1
        || credential_steps != usize::from(commit.plan.credential_ref.is_some())
        || selection_steps != 1
    {
        return Err(CoreError::invalid(
            "fresh discovery commit requires a complete contiguous compensation recipe",
        ));
    }
    Ok(())
}

fn validate_prepared_commit_session_binding(
    transaction: &Transaction<'_>,
    commit: &PreparedDiscoveryCommit,
) -> CoreResult<()> {
    let (input_json, created_at) = transaction
        .query_row(
            "SELECT sanitized_input_json, created_at
             FROM provider_discovery_sessions
             WHERE id = ?1",
            [commit.plan.session_id.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| corrupted("prepared commit discovery session is missing"))?;
    let input = serde_json::from_str::<SanitizedDiscoveryInput>(&input_json)
        .map_err(|_| corrupted("stored discovery input is invalid"))?;
    input
        .validate()
        .map_err(|_| corrupted("stored discovery input violates its contract"))?;
    validate_sanitized_input(&input)
        .map_err(|_| corrupted("stored discovery input contains forbidden data"))?;
    let created_at = parse_timestamp(&created_at, "discovery session creation time")?;
    validate_discovery_local_network_approval_binding(&input, created_at, Utc::now())?;
    if commit.plan.connection_id != input.connection_id
        || commit.plan.credential_ref != input.credential_ref
    {
        return Err(CoreError::invalid(
            "commit plan connection identity differs from its sanitized input",
        ));
    }
    let current_selection = load_discovery_previous_selection(transaction)?;
    if commit.plan.previous_selection != current_selection {
        return Err(CoreError::invalid(
            "commit plan previous selection is not the current atomic snapshot",
        ));
    }
    Ok(())
}

fn insert_session_in_transaction(
    transaction: &Transaction<'_>,
    session: &ProviderDiscoverySession,
    created_at: DateTime<Utc>,
) -> CoreResult<()> {
    validate_discovery_local_network_approval_binding(&session.input, created_at, created_at)?;
    let input_json = canonical_json_result(
        serde_json::to_value(&session.input),
        "sanitized discovery input",
    )?;
    transaction
        .execute(
            "INSERT INTO provider_discovery_sessions (
                 id, state, revision, next_event_sequence, sanitized_input_json,
                 cancellation_pending, redaction_version, created_at, updated_at
             ) VALUES (?1, 'draft', 0, 1, ?2, 0, 1, ?3, ?3)",
            params![session.id.as_str(), input_json, created_at.to_rfc3339()],
        )
        .map_err(database_error)?;
    append_audit(
        transaction,
        session.id.as_str(),
        0,
        "session_created",
        None,
        Some(session.id.as_str()),
        "discovery.audit.session_created",
        created_at,
    )
}

pub(super) fn validate_discovery_local_network_approval_binding(
    input: &SanitizedDiscoveryInput,
    session_created_at: DateTime<Utc>,
    observed_at: DateTime<Utc>,
) -> CoreResult<()> {
    input
        .connection_options
        .require_active_local_network_approval_at(observed_at)
        .map_err(|error| {
            CoreError::new(
                CoreErrorCode::InvalidInput,
                format!(
                    "provider discovery LAN approval is inactive; restart provider discovery: {error}"
                ),
                true,
            )
        })?;
    if input.connection_options.network_mode == ProviderNetworkMode::ApprovedLocalNetwork
        && input.connection_options.local_network_approved_at != Some(session_created_at)
    {
        return Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            "provider discovery LAN approval is not bound to its immutable session creation time; restart provider discovery",
            true,
        ));
    }
    Ok(())
}
