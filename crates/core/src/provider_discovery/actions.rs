use super::{
    AssistantError, AssistantState, BTreeSet, CoreError, CoreResult, DateTime,
    DeterministicDiscoveryExecutor, DeterministicDiscoverySource, Digest, DiscoveryActionEnvelope,
    DiscoveryActionId, DiscoveryApprovalBinding, DiscoveryApprovalDecision, DiscoveryApprovalGrant,
    DiscoveryApprovalRecord, DiscoveryFetchBudget, DiscoveryFreshEvidenceSource,
    DiscoveryInterruptionOutcome, DiscoveryJsonUpdate, DiscoveryOperationId,
    DiscoveryOperationKind, DiscoveryOperationStatus, DiscoveryReviewDiff, DiscoverySessionId,
    DiscoverySessionSnapshot, DiscoveryState, DiscoveryTransitionWrite, DiscoveryWorkingDraft,
    DurableOperationOutcome, MAX_DISCOVERY_ROWS, PreparedDiscoveryCommit, ProviderDiscoveryAction,
    ProviderDiscoveryAdditionalEvidence, ProviderDiscoveryOrchestrator, Serialize, Sha256, Utc,
    Value, additional_curl_url_policy, additional_document_url_policy,
    apply_credential_origin_scope, approval_proposal_for, approval_record, assistant_error,
    assistant_proposal, build_review, cancel_assistant_snapshot, canonical_serde_sha256,
    commit_plan_for, compensation_recipe, corrupted_assistant_resume_boundary,
    credential_bearing_curl_requires_handoff, credential_origin_proposal, deterministic_artifacts,
    deterministic_commit_attempt_id, deterministic_error, grant_assistant_snapshot,
    hydrate_working_draft, initialize_assistant, inspect_curl, operation_for_effect,
    origin_from_http_url, probe_proposal, record_deterministic_assistant_claims,
    redacted_assistant_evidence, require_approval_binding, require_approval_id, restored_assistant,
    sanitized_graph_sha256, select_candidate, synchronize_assistant_snapshot, transition_error,
    watch, working_draft_value,
};

impl ProviderDiscoveryOrchestrator<'_> {
    /// Applies one user action with revision/idempotency and exact approval
    /// binding, then executes any resulting non-persistent effect.
    pub fn continue_discovery(
        &self,
        session_id: &DiscoverySessionId,
        envelope: DiscoveryActionEnvelope,
        credential: Option<&str>,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let (_cancel, cancelled) = watch::channel(false);
        self.continue_discovery_with_cancellation(session_id, envelope, credential, cancelled)
    }

    pub fn continue_discovery_with_cancellation(
        &self,
        session_id: &DiscoverySessionId,
        envelope: DiscoveryActionEnvelope,
        credential: Option<&str>,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        Self::validate_envelope(&envelope)?;
        if self
            .storage
            .find_discovery_action_replay(
                session_id,
                &envelope.id,
                &envelope.request_sha256,
                envelope.action.kind(),
            )?
            .is_some()
        {
            return self.get(session_id);
        }
        if !is_public_discovery_action(&envelope.action) {
            return Err(CoreError::invalid(
                "internal discovery completion actions are not accepted at the public boundary",
            ));
        }
        let snapshot = self.get(session_id)?;
        if snapshot.session.id != *session_id {
            return Err(CoreError::invalid("discovery session identifier mismatch"));
        }
        let mut draft = hydrate_working_draft(&snapshot)?;
        let is_cancel = matches!(&envelope.action, ProviderDiscoveryAction::Cancel);
        let occurred_at = Utc::now();
        let (approval, review_update, prepared_commit) =
            self.prepare_user_action(&snapshot, &envelope, &mut draft, occurred_at)?;
        let transition = snapshot
            .session
            .apply(&envelope)
            .map_err(transition_error)?;
        if transition.session.state.is_terminal() {
            cancel_assistant_snapshot(&mut draft)?;
        }
        let new_operation_id =
            operation_for_effect(&transition.effect).map(|_| DiscoveryOperationId::new());
        let write = DiscoveryTransitionWrite {
            transition,
            draft: DiscoveryJsonUpdate::Replace(working_draft_value(&draft)?),
            review: review_update,
            new_evidence: Vec::new(),
            new_candidates: Vec::new(),
            approval,
            new_operation_id,
            completed_operation: None,
            prepared_commit,
            provider_graph: None,
            occurred_at,
        };
        self.storage.persist_discovery_transition(&write)?;
        if is_cancel {
            self.settle_prepared_cancellation(session_id)?;
            // A Started operation owns its real cancellation outcome. Do not
            // re-enter the dispatcher without its credential and falsely
            // attest ConfirmedNoExternalEffect while another worker is still
            // in flight. The worker's shared watch token will settle it.
            return self.get(session_id);
        }
        self.drive_nonpersistent(session_id, credential, cancelled)
    }

    /// Collects one new document or one-shot cURL source under the existing
    /// discovery origin and persists only redacted deterministic evidence.
    ///
    /// Collection is bounded. A failed or empty collection leaves the durable
    /// session in `awaiting_more_evidence`. The raw cURL and any extracted
    /// credential are dropped before the action, draft, evidence, or outbox
    /// record is constructed.
    #[allow(clippy::too_many_lines)]
    pub fn supply_additional_evidence(
        &self,
        session_id: &DiscoverySessionId,
        expected_revision: u64,
        source: ProviderDiscoveryAdditionalEvidence,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let snapshot = self.get(session_id)?;
        if snapshot.session.state != DiscoveryState::AwaitingMoreEvidence {
            return Err(CoreError::invalid(
                "provider discovery is not awaiting more evidence",
            ));
        }
        if snapshot.session.revision != expected_revision {
            return Err(CoreError::invalid(
                "provider discovery revision changed before evidence collection",
            ));
        }
        let (deterministic_source, durable_source) = match source {
            ProviderDiscoveryAdditionalEvidence::DocumentUrl(url) => {
                let origin = origin_from_http_url(&url)?;
                let policy = additional_document_url_policy(&snapshot.session.input, &origin)?;
                let source = DeterministicDiscoverySource::site_with_policy(
                    url.as_str(),
                    policy,
                    DiscoveryFetchBudget::default(),
                )
                .map_err(deterministic_error)?;
                (source, DiscoveryFreshEvidenceSource::DocumentUrl { origin })
            }
            ProviderDiscoveryAdditionalEvidence::Curl(input) => {
                let inspection = inspect_curl(input)
                    .map_err(|_| CoreError::invalid("pasted cURL input was rejected"))?;
                let (evidence, extracted_credential) = inspection.into_parts();
                if extracted_credential.is_some() {
                    drop(extracted_credential);
                    return Err(credential_bearing_curl_requires_handoff());
                }
                let origin = evidence.origin.clone();
                let policy = additional_curl_url_policy(&snapshot.session.input, &origin)?;
                let source =
                    DeterministicDiscoverySource::sanitized_curl_with_policy(evidence, policy)
                        .map_err(deterministic_error)?;
                (
                    source,
                    DiscoveryFreshEvidenceSource::SanitizedCurl { origin },
                )
            }
        };
        let output = self
            .runtime
            .block_on(DeterministicDiscoveryExecutor::new().execute(deterministic_source))
            .map_err(deterministic_error)?;
        let (mut evidence, _) = deterministic_artifacts(&snapshot, &output)?;
        if evidence.is_empty() {
            return Err(CoreError::invalid(
                "additional evidence collection produced no safe evidence",
            ));
        }
        let existing_ids = self
            .storage
            .list_discovery_evidence(session_id, MAX_DISCOVERY_ROWS)?
            .into_iter()
            .map(|record| record.id)
            .collect::<BTreeSet<_>>();
        evidence.retain(|record| !existing_ids.contains(&record.id));
        if evidence.is_empty() {
            return Err(CoreError::invalid(
                "additional evidence collection produced no new safe evidence",
            ));
        }
        let evidence_ids = evidence
            .iter()
            .map(|record| record.id.clone())
            .collect::<Vec<_>>();

        let mut draft = hydrate_working_draft(&snapshot)?;
        record_deterministic_assistant_claims(&snapshot, &output, &mut draft)?;
        if draft.assistant.is_some() {
            let mut engine = restored_assistant(&draft)?;
            if engine.state() != AssistantState::AwaitingMoreEvidence {
                return Err(corrupted_assistant_resume_boundary());
            }
            let mut requires_fresh_consent = false;
            for record in &evidence {
                let claims = draft
                    .assistant_evidence_claims
                    .get(&record.id)
                    .cloned()
                    .unwrap_or_default();
                match engine
                    .add_redacted_evidence(redacted_assistant_evidence(record.clone(), claims)?)
                {
                    Ok(()) => {}
                    Err(AssistantError::UnapprovedEvidenceOrigin) => {
                        requires_fresh_consent = true;
                        break;
                    }
                    Err(error) => return Err(assistant_error(error)),
                }
            }
            if requires_fresh_consent {
                // A newly supplied origin is never added to the old egress
                // grant. Rebuild an unconsented assistant from the complete
                // persisted evidence set in the extraction operation below.
                draft.assistant = None;
                draft.assistant_approval_binding = None;
            } else {
                engine
                    .continue_after_more_evidence()
                    .map_err(assistant_error)?;
                synchronize_assistant_snapshot(&mut draft, &engine);
            }
        }
        draft.deterministic = Some(output);
        draft.evidence_ids.extend(evidence_ids.clone());
        draft.evidence_ids.sort();
        draft.evidence_ids.dedup();
        draft.extra_evidence_ids.extend(evidence_ids.clone());
        draft.extra_evidence_ids.sort();
        draft.extra_evidence_ids.dedup();
        let envelope = provider_discovery_action_envelope(
            DiscoveryActionId::new(),
            expected_revision,
            ProviderDiscoveryAction::SupplyFreshEvidence {
                evidence_ids,
                source: durable_source,
            },
        )?;
        let transition = snapshot
            .session
            .apply(&envelope)
            .map_err(transition_error)?;
        let new_operation_id =
            operation_for_effect(&transition.effect).map(|_| DiscoveryOperationId::new());
        self.storage
            .persist_discovery_transition(&DiscoveryTransitionWrite {
                transition,
                draft: DiscoveryJsonUpdate::Replace(working_draft_value(&draft)?),
                review: DiscoveryJsonUpdate::Preserve,
                new_evidence: evidence,
                new_candidates: Vec::new(),
                approval: None,
                new_operation_id,
                completed_operation: None,
                prepared_commit: None,
                provider_graph: None,
                occurred_at: Utc::now(),
            })?;
        let (_cancel, cancelled) = watch::channel(false);
        self.drive_nonpersistent(session_id, None, cancelled)
    }

    fn settle_prepared_cancellation(&self, session_id: &DiscoverySessionId) -> CoreResult<()> {
        let snapshot = self.get(session_id)?;
        if !snapshot.session.cancellation_pending {
            return Ok(());
        }
        let Some(operation) = self.storage.get_current_discovery_operation(session_id)? else {
            return Ok(());
        };
        if operation.status != DiscoveryOperationStatus::Prepared
            || operation.kind == DiscoveryOperationKind::Compensation
        {
            return Ok(());
        }
        let mut draft = hydrate_working_draft(&snapshot)?;
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
        )
    }

    fn validate_envelope(envelope: &DiscoveryActionEnvelope) -> CoreResult<()> {
        envelope
            .validate()
            .map_err(|error| CoreError::invalid(format!("invalid discovery action: {error}")))?;
        let expected = canonical_sha256(&envelope.action, "provider discovery action")?;
        if expected != envelope.request_sha256 {
            return Err(CoreError::invalid(
                "provider discovery action hash does not match its canonical payload",
            ));
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    pub(super) fn prepare_user_action(
        &self,
        snapshot: &DiscoverySessionSnapshot,
        envelope: &DiscoveryActionEnvelope,
        draft: &mut DiscoveryWorkingDraft,
        occurred_at: DateTime<Utc>,
    ) -> CoreResult<(
        Option<DiscoveryApprovalRecord>,
        DiscoveryJsonUpdate<DiscoveryReviewDiff>,
        Option<PreparedDiscoveryCommit>,
    )> {
        let mut review_update = DiscoveryJsonUpdate::Preserve;
        let mut prepared_commit = None;
        let approval = match &envelope.action {
            ProviderDiscoveryAction::SelectTemplate { candidate_id } => {
                select_candidate(self.storage, snapshot, draft, candidate_id, occurred_at)?;
                Some(approval_record(
                    snapshot,
                    approval_proposal_for(
                        &snapshot.session.id,
                        snapshot.session.revision,
                        DiscoveryApprovalGrant::TemplateSelection {
                            candidate_id: candidate_id.clone(),
                        },
                    )?,
                    DiscoveryApprovalDecision::Approved,
                    occurred_at,
                ))
            }
            ProviderDiscoveryAction::ApproveCredentialOrigin { approval_id } => {
                let proposal = credential_origin_proposal(snapshot, draft)?;
                require_approval_id(approval_id, &proposal)?;
                let connection = draft.connection.as_mut().ok_or_else(|| {
                    CoreError::internal("credential approval has no connection draft")
                })?;
                let template = draft.template.as_ref().ok_or_else(|| {
                    CoreError::internal("credential approval has no template draft")
                })?;
                apply_credential_origin_scope(template, connection);
                draft.credential_approval_id = Some(proposal.id.clone());
                Some(approval_record(
                    snapshot,
                    proposal,
                    DiscoveryApprovalDecision::Approved,
                    occurred_at,
                ))
            }
            ProviderDiscoveryAction::ApproveProbes {
                approval_id,
                approval_grant_sha256,
            } => {
                let proposal = probe_proposal(snapshot, draft)?;
                require_approval_binding(approval_id, approval_grant_sha256, &proposal)?;
                Some(approval_record(
                    snapshot,
                    proposal,
                    DiscoveryApprovalDecision::Approved,
                    occurred_at,
                ))
            }
            ProviderDiscoveryAction::SkipProbes => {
                let proposal = probe_proposal(snapshot, draft)?;
                let review = build_review(draft)?;
                review_update = DiscoveryJsonUpdate::Replace(review);
                Some(approval_record(
                    snapshot,
                    proposal,
                    DiscoveryApprovalDecision::Rejected,
                    occurred_at,
                ))
            }
            ProviderDiscoveryAction::ApproveAssistant {
                approval_id,
                approval_grant_sha256,
            } => {
                let proposal = assistant_proposal(snapshot, draft)?;
                require_approval_binding(approval_id, approval_grant_sha256, &proposal)?;
                grant_assistant_snapshot(snapshot, draft, &proposal.grant)?;
                draft.assistant_approval_binding = Some(DiscoveryApprovalBinding {
                    approval_id: proposal.id.clone(),
                    grant_sha256: proposal.grant_sha256.clone(),
                });
                Some(approval_record(
                    snapshot,
                    proposal,
                    DiscoveryApprovalDecision::Approved,
                    occurred_at,
                ))
            }
            ProviderDiscoveryAction::DeclineAssistant => {
                let proposal = assistant_proposal(snapshot, draft)?;
                draft.assistant_approval_binding = None;
                cancel_assistant_snapshot(draft)?;
                draft.assistant = None;
                Some(approval_record(
                    snapshot,
                    proposal,
                    DiscoveryApprovalDecision::Rejected,
                    occurred_at,
                ))
            }
            ProviderDiscoveryAction::RequestAssistant => {
                initialize_assistant(self.storage, snapshot, draft)?;
                draft.assistant_approval_binding = None;
                None
            }
            ProviderDiscoveryAction::ApproveReview {
                approval_id,
                commit_attempt_id,
                commit_plan_sha256,
                graph_sha256,
            } => {
                let review = snapshot
                    .review
                    .as_ref()
                    .ok_or_else(|| CoreError::internal("review approval has no durable review"))?;
                let current_graph_sha256 = sanitized_graph_sha256(draft)?;
                if review.graph_sha256 != current_graph_sha256
                    || graph_sha256 != &current_graph_sha256
                {
                    return Err(CoreError::invalid(
                        "review approval does not match the current sanitized provider graph",
                    ));
                }
                let expected_attempt = deterministic_commit_attempt_id(
                    &snapshot.session.id,
                    snapshot.session.revision,
                );
                if commit_attempt_id != &expected_attempt {
                    return Err(CoreError::invalid(
                        "review approval commit attempt identifier does not match",
                    ));
                }
                let plan =
                    commit_plan_for(self.storage, snapshot, draft, expected_attempt, review)?;
                let expected_plan_sha256 = canonical_serde_sha256(&plan, "discovery commit plan")?;
                if commit_plan_sha256 != &expected_plan_sha256 {
                    return Err(CoreError::invalid(
                        "review approval commit plan hash does not match",
                    ));
                }
                let proposal = approval_proposal_for(
                    &snapshot.session.id,
                    snapshot.session.revision,
                    DiscoveryApprovalGrant::Review {
                        review_sha256: review.sha256.clone(),
                        graph_sha256: current_graph_sha256,
                    },
                )?;
                require_approval_id(approval_id, &proposal)?;
                let compensation_steps =
                    compensation_recipe(&snapshot.session.id, snapshot.session.revision, &plan);
                prepared_commit = Some(PreparedDiscoveryCommit {
                    plan,
                    plan_sha256: expected_plan_sha256,
                    attempt_number: 1,
                    reuse_existing: false,
                    compensation_steps,
                });
                Some(approval_record(
                    snapshot,
                    proposal,
                    DiscoveryApprovalDecision::Approved,
                    occurred_at,
                ))
            }
            ProviderDiscoveryAction::ResolveUnknownOutcome {
                approval_id,
                resolution,
            } => {
                let operation = snapshot.session.unknown_operation.ok_or_else(|| {
                    CoreError::invalid("discovery has no unknown operation to resolve")
                })?;
                let proposal = approval_proposal_for(
                    &snapshot.session.id,
                    snapshot.session.revision,
                    DiscoveryApprovalGrant::UnknownOutcomeResolution {
                        operation,
                        resolution: resolution.clone(),
                    },
                )?;
                require_approval_id(approval_id, &proposal)?;
                Some(approval_record(
                    snapshot,
                    proposal,
                    DiscoveryApprovalDecision::Approved,
                    occurred_at,
                ))
            }
            ProviderDiscoveryAction::SupplyMoreEvidence { evidence_ids } => {
                let existing = self
                    .storage
                    .list_discovery_evidence(&snapshot.session.id, MAX_DISCOVERY_ROWS)?;
                if evidence_ids
                    .iter()
                    .any(|id| !existing.iter().any(|record| &record.id == id))
                {
                    return Err(CoreError::invalid(
                        "additional evidence must already belong to this discovery session",
                    ));
                }
                draft.extra_evidence_ids.clone_from(evidence_ids);
                draft.assistant = None;
                None
            }
            ProviderDiscoveryAction::RestartInterrupted => {
                if snapshot
                    .session
                    .recovery
                    .as_ref()
                    .is_some_and(|checkpoint| {
                        checkpoint.operation == DiscoveryOperationKind::AtomicCommit
                    })
                {
                    let attempt_id =
                        snapshot.session.commit_attempt_id.as_ref().ok_or_else(|| {
                            CoreError::internal("interrupted commit lost its attempt")
                        })?;
                    let attempt = self.storage.get_discovery_commit_attempt(attempt_id)?;
                    prepared_commit = Some(PreparedDiscoveryCommit {
                        plan: attempt.plan,
                        plan_sha256: attempt.plan_sha256,
                        attempt_number: attempt.attempt_number,
                        reuse_existing: true,
                        compensation_steps: Vec::new(),
                    });
                }
                None
            }
            _ => None,
        };
        Ok((approval, review_update, prepared_commit))
    }
}

fn is_public_discovery_action(action: &ProviderDiscoveryAction) -> bool {
    matches!(
        action,
        ProviderDiscoveryAction::SelectTemplate { .. }
            | ProviderDiscoveryAction::ContinueWithoutTemplate
            | ProviderDiscoveryAction::SupplyMoreEvidence { .. }
            | ProviderDiscoveryAction::RequestAssistant
            | ProviderDiscoveryAction::ApproveAssistant { .. }
            | ProviderDiscoveryAction::DeclineAssistant
            | ProviderDiscoveryAction::ApproveCredentialOrigin { .. }
            | ProviderDiscoveryAction::ApproveProbes { .. }
            | ProviderDiscoveryAction::SkipProbes
            | ProviderDiscoveryAction::ApproveReview { .. }
            | ProviderDiscoveryAction::RestartInterrupted
            | ProviderDiscoveryAction::ResumeCompensation
            | ProviderDiscoveryAction::ResolveUnknownOutcome { .. }
            | ProviderDiscoveryAction::Cancel
    )
}

/// Builds a redacted action envelope and hashes only the typed action payload.
pub fn provider_discovery_action_envelope(
    id: DiscoveryActionId,
    expected_revision: u64,
    action: ProviderDiscoveryAction,
) -> CoreResult<DiscoveryActionEnvelope> {
    let request_sha256 = canonical_sha256(&action, "provider discovery action")?;
    Ok(DiscoveryActionEnvelope {
        id,
        expected_revision,
        request_sha256,
        action,
    })
}

impl crate::app::Core {
    pub fn continue_provider_discovery(
        &self,
        session_id: &DiscoverySessionId,
        envelope: DiscoveryActionEnvelope,
        credential: Option<&str>,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        if matches!(&envelope.action, ProviderDiscoveryAction::Cancel)
            && let Some(physical_authority_id) = self.prepared_discovery_credential_reservation_id(
                session_id,
                envelope.expected_revision,
            )?
        {
            self.forget_discovery_credential_reservation(&physical_authority_id)?;
        }
        self.provider_discovery()
            .continue_discovery(session_id, envelope, credential)
    }

    pub fn continue_provider_discovery_with_cancellation(
        &self,
        session_id: &DiscoverySessionId,
        envelope: DiscoveryActionEnvelope,
        credential: Option<&str>,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        if matches!(&envelope.action, ProviderDiscoveryAction::Cancel)
            && let Some(physical_authority_id) = self.prepared_discovery_credential_reservation_id(
                session_id,
                envelope.expected_revision,
            )?
        {
            // Cancellation abandons a Prepared reservation. Consume the
            // process-local capability first so any later transition failure
            // remains fail-closed and cannot make that slot reusable.
            self.forget_discovery_credential_reservation(&physical_authority_id)?;
        }
        self.provider_discovery()
            .continue_discovery_with_cancellation(session_id, envelope, credential, cancelled)
    }

    pub fn supply_provider_discovery_evidence(
        &self,
        session_id: &DiscoverySessionId,
        expected_revision: u64,
        source: ProviderDiscoveryAdditionalEvidence,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery()
            .supply_additional_evidence(session_id, expected_revision, source)
    }

    pub fn cancel_provider_discovery(
        &self,
        session_id: &DiscoverySessionId,
        expected_revision: u64,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let envelope = provider_discovery_action_envelope(
            DiscoveryActionId::new(),
            expected_revision,
            ProviderDiscoveryAction::Cancel,
        )?;
        self.continue_provider_discovery(session_id, envelope, None)
    }
}

pub(super) fn canonical_sha256<T: Serialize>(value: &T, label: &str) -> CoreResult<String> {
    let value = serde_json::to_value(value)
        .map_err(|_| CoreError::internal(format!("{label} could not be serialized")))?;
    let mut canonical = String::new();
    write_canonical_json(&value, &mut canonical)?;
    Ok(sha256_hex(canonical.as_bytes()))
}

pub(super) fn write_canonical_json(value: &Value, output: &mut String) -> CoreResult<()> {
    match value {
        Value::Null => output.push_str("null"),
        Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        Value::Number(value) => output.push_str(&value.to_string()),
        Value::String(value) => output.push_str(
            &serde_json::to_string(value)
                .map_err(|_| CoreError::internal("JSON string could not be serialized"))?,
        ),
        Value::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_canonical_json(value, output)?;
            }
            output.push(']');
        }
        Value::Object(values) => {
            output.push('{');
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                output.push_str(
                    &serde_json::to_string(key)
                        .map_err(|_| CoreError::internal("JSON key could not be serialized"))?,
                );
                output.push(':');
                write_canonical_json(
                    values
                        .get(key)
                        .ok_or_else(|| CoreError::internal("canonical JSON key disappeared"))?,
                    output,
                )?;
            }
            output.push('}');
        }
    }
    Ok(())
}

pub(super) fn sha256_hex(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}
