use super::{
    CoreError, CoreErrorCode, CoreResult, DateTime, DiscoveryActionId, DiscoveryCommitAttemptId,
    DiscoveryCommitPlan, DiscoveryCompensationKind, DiscoveryCompensationStatus,
    DiscoveryCompensationStep, DiscoveryCompensationTarget, DiscoveryCompletedOperationWrite,
    DiscoveryFailure, DiscoveryJsonUpdate, DiscoveryOperationId, DiscoveryOperationKind,
    DiscoveryOperationStatus, DiscoveryRecoveryResult, DiscoverySessionId,
    DiscoverySessionSnapshot, DiscoveryState, DiscoveryTransitionWrite, DurableOperationOutcome,
    PreparedDiscoveryCompensationStep, ProviderDiscoveryAction, ProviderDiscoveryOrchestrator, Utc,
    deterministic_action_id, deterministic_id, hydrate_working_draft,
    provider_discovery_action_envelope, resumable_assistant_operation_ids, transition_error,
    working_draft_value,
};

impl ProviderDiscoveryOrchestrator<'_> {
    /// Marks unfinished work interrupted or outcome-unknown. It never executes
    /// a prepared operation and therefore never replays a request on startup.
    pub fn recover_startup(
        &self,
        recovered_at: DateTime<Utc>,
    ) -> CoreResult<Vec<DiscoveryRecoveryResult>> {
        let resumable = resumable_assistant_operation_ids(self.storage)?;
        self.storage
            .recover_unfinished_discovery_operations_except(recovered_at, &resumable)
    }

    pub fn unfinished_recovery_candidates(&self) -> CoreResult<Vec<DiscoverySessionSnapshot>> {
        self.storage
            .list_unfinished_discovery_sessions_for_recovery()
    }

    pub fn credential_recovery_candidates(&self) -> CoreResult<Vec<DiscoverySessionSnapshot>> {
        self.unfinished_recovery_candidates()?
            .into_iter()
            .filter(|snapshot| {
                snapshot.session.state == DiscoveryState::Committing
                    && snapshot.session.input.credential_ref.is_some()
            })
            .map(|snapshot| {
                let operation = self
                    .storage
                    .get_current_discovery_operation(&snapshot.session.id)?
                    .ok_or_else(|| {
                        CoreError::new(
                            CoreErrorCode::StorageCorrupted,
                            "credential recovery candidate has no active operation",
                            false,
                        )
                    })?;
                if operation.kind != DiscoveryOperationKind::AtomicCommit
                    || !matches!(
                        operation.status,
                        DiscoveryOperationStatus::Prepared | DiscoveryOperationStatus::Started
                    )
                    || snapshot.active_operation_id.as_ref() != Some(&operation.id)
                {
                    return Err(CoreError::new(
                        CoreErrorCode::StorageCorrupted,
                        "credential recovery candidate is detached from its active operation",
                        false,
                    ));
                }
                Ok(snapshot)
            })
            .collect()
    }

    pub fn compensation_steps(
        &self,
        attempt_id: &DiscoveryCommitAttemptId,
    ) -> CoreResult<Vec<lorepia_storage::DiscoveryCompensationRecord>> {
        self.storage.list_discovery_compensation_steps(attempt_id)
    }

    /// Starts the compensation operation and executes only Core-owned database
    /// steps. It stops before native credential deletion and never retries a
    /// failed or unknown step.
    #[allow(clippy::too_many_lines)]
    pub fn continue_compensation(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let snapshot = self.get(session_id)?;
        if snapshot.session.state != DiscoveryState::Compensating {
            return Err(CoreError::invalid("provider discovery is not compensating"));
        }
        if snapshot.session.failure.is_some() {
            return Err(CoreError::invalid(
                "failed compensation requires an explicit resume action",
            ));
        }
        let operation = self
            .storage
            .get_current_discovery_operation(session_id)?
            .ok_or_else(|| CoreError::invalid("compensation has no active operation"))?;
        if operation.kind != DiscoveryOperationKind::Compensation {
            return Err(CoreError::invalid(
                "active discovery operation is not compensation",
            ));
        }
        if operation.status == DiscoveryOperationStatus::Prepared
            && !self
                .storage
                .mark_discovery_operation_started(&operation.id, Utc::now())?
        {
            return Err(CoreError::invalid(
                "compensation operation changed concurrently",
            ));
        } else if !matches!(
            operation.status,
            DiscoveryOperationStatus::Prepared | DiscoveryOperationStatus::Started
        ) {
            return Err(CoreError::invalid("compensation operation is not active"));
        }
        let attempt_id = snapshot
            .session
            .commit_attempt_id
            .as_ref()
            .ok_or_else(|| CoreError::internal("compensation lost its commit attempt"))?
            .clone();
        loop {
            let steps = self
                .storage
                .list_discovery_compensation_steps(&attempt_id)?;
            let Some(step) = steps.iter().find(|step| {
                step.status != lorepia_storage::DiscoveryCompensationStatus::Completed
            }) else {
                let current = self.get(session_id)?;
                let mut draft = hydrate_working_draft(&current)?;
                let operation_id = current
                    .active_operation_id
                    .as_ref()
                    .ok_or_else(|| CoreError::invalid("compensation operation disappeared"))?;
                self.persist_operation_completion(
                    &current,
                    operation_id,
                    &mut draft,
                    ProviderDiscoveryAction::CompensationSucceeded,
                    DurableOperationOutcome::Succeeded,
                    Vec::new(),
                    Vec::new(),
                    DiscoveryJsonUpdate::Preserve,
                )?;
                return self.get(session_id);
            };
            match step.status {
                lorepia_storage::DiscoveryCompensationStatus::Failed => {
                    return Err(CoreError::invalid(
                        "failed compensation step requires an explicit resume",
                    ));
                }
                lorepia_storage::DiscoveryCompensationStatus::OutcomeUnknown => {
                    return Err(CoreError::invalid(
                        "unknown compensation outcome requires explicit reconciliation",
                    ));
                }
                lorepia_storage::DiscoveryCompensationStatus::Pending => {
                    if step.kind == DiscoveryCompensationKind::RemoveCredentialSlot {
                        return self.get(session_id);
                    }
                    self.storage.update_discovery_compensation_status(
                        &step.id,
                        lorepia_storage::DiscoveryCompensationStatus::Pending,
                        lorepia_storage::DiscoveryCompensationStatus::InProgress,
                        None,
                        Utc::now(),
                    )?;
                }
                lorepia_storage::DiscoveryCompensationStatus::InProgress => {
                    if step.kind == DiscoveryCompensationKind::RemoveCredentialSlot {
                        return self.get(session_id);
                    }
                }
                lorepia_storage::DiscoveryCompensationStatus::Completed => continue,
            }
            let result = match step.kind {
                DiscoveryCompensationKind::RemoveConnectionGraph => self
                    .storage
                    .compensate_discovered_provider_graph(&attempt_id, Utc::now()),
                DiscoveryCompensationKind::RestorePreviousSelection => self
                    .storage
                    .restore_discovery_previous_selection(&attempt_id, Utc::now()),
                DiscoveryCompensationKind::RemoveCredentialSlot => return self.get(session_id),
            };
            if result.is_err() {
                return self.persist_compensation_failure(
                    session_id,
                    &step.id,
                    DiscoveryFailure {
                        code: "compensation_database_step_failed".to_owned(),
                        message_key: "provider.discovery.compensation_database_step_failed"
                            .to_owned(),
                        recoverable: true,
                    },
                );
            }
        }
    }

    pub fn start_credential_compensation(
        &self,
        session_id: &DiscoverySessionId,
        step_id: &str,
    ) -> CoreResult<lorepia_storage::DiscoveryCompensationRecord> {
        self.continue_compensation(session_id)?;
        let step = self.require_credential_compensation_step(session_id, step_id)?;
        let expected = match step.status {
            lorepia_storage::DiscoveryCompensationStatus::Pending
            | lorepia_storage::DiscoveryCompensationStatus::Failed => step.status,
            _ => {
                return Err(CoreError::invalid(
                    "credential compensation step cannot be started",
                ));
            }
        };
        self.storage.update_discovery_compensation_status(
            step_id,
            expected,
            lorepia_storage::DiscoveryCompensationStatus::InProgress,
            None,
            Utc::now(),
        )?;
        self.require_credential_compensation_step(session_id, step_id)
    }

    pub fn complete_credential_compensation(
        &self,
        session_id: &DiscoverySessionId,
        step_id: &str,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.require_credential_compensation_step(session_id, step_id)?;
        self.storage.update_discovery_compensation_status(
            step_id,
            lorepia_storage::DiscoveryCompensationStatus::InProgress,
            lorepia_storage::DiscoveryCompensationStatus::Completed,
            None,
            Utc::now(),
        )?;
        self.continue_compensation(session_id)
    }

    pub fn fail_credential_compensation(
        &self,
        session_id: &DiscoverySessionId,
        step_id: &str,
        failure: DiscoveryFailure,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        failure.validate().map_err(|error| {
            CoreError::invalid(format!("invalid compensation failure: {error}"))
        })?;
        self.require_credential_compensation_step(session_id, step_id)?;
        self.persist_compensation_failure(session_id, step_id, failure)
    }

    fn persist_compensation_failure(
        &self,
        session_id: &DiscoverySessionId,
        step_id: &str,
        failure: DiscoveryFailure,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let snapshot = self.get(session_id)?;
        let draft = hydrate_working_draft(&snapshot)?;
        let operation_id = snapshot
            .active_operation_id
            .as_ref()
            .ok_or_else(|| CoreError::invalid("compensation operation disappeared"))?
            .clone();
        let envelope = provider_discovery_action_envelope(
            DiscoveryActionId::new(),
            snapshot.session.revision,
            ProviderDiscoveryAction::CompensationFailed { failure },
        )?;
        let transition = snapshot
            .session
            .apply(&envelope)
            .map_err(transition_error)?;
        self.storage
            .fail_discovery_compensation_and_persist_transition(
                step_id,
                &DiscoveryTransitionWrite {
                    transition,
                    draft: DiscoveryJsonUpdate::Replace(working_draft_value(&draft)?),
                    review: DiscoveryJsonUpdate::Preserve,
                    new_evidence: Vec::new(),
                    new_candidates: Vec::new(),
                    approval: None,
                    new_operation_id: None,
                    completed_operation: Some(DiscoveryCompletedOperationWrite {
                        id: operation_id,
                        outcome: DurableOperationOutcome::Failed,
                    }),
                    prepared_commit: None,
                    provider_graph: None,
                    occurred_at: Utc::now(),
                },
            )?;
        self.get(session_id)
    }

    pub fn mark_credential_compensation_unknown(
        &self,
        session_id: &DiscoverySessionId,
        step_id: &str,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.require_credential_compensation_step(session_id, step_id)?;
        let snapshot = self.get(session_id)?;
        let draft = hydrate_working_draft(&snapshot)?;
        let operation_id = snapshot
            .active_operation_id
            .as_ref()
            .ok_or_else(|| CoreError::invalid("compensation operation disappeared"))?
            .clone();
        let envelope = provider_discovery_action_envelope(
            DiscoveryActionId::new(),
            snapshot.session.revision,
            ProviderDiscoveryAction::ExternalOutcomeBecameUnknown,
        )?;
        let transition = snapshot
            .session
            .apply(&envelope)
            .map_err(transition_error)?;
        self.storage
            .mark_discovery_compensation_unknown_and_persist_transition(
                step_id,
                &DiscoveryTransitionWrite {
                    transition,
                    draft: DiscoveryJsonUpdate::Replace(working_draft_value(&draft)?),
                    review: DiscoveryJsonUpdate::Preserve,
                    new_evidence: Vec::new(),
                    new_candidates: Vec::new(),
                    approval: None,
                    new_operation_id: None,
                    completed_operation: Some(DiscoveryCompletedOperationWrite {
                        id: operation_id,
                        outcome: DurableOperationOutcome::OutcomeUnknown,
                    }),
                    prepared_commit: None,
                    provider_graph: None,
                    occurred_at: Utc::now(),
                },
            )?;
        self.get(session_id)
    }

    pub fn resume_compensation(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        let snapshot = self.get(session_id)?;
        let draft = hydrate_working_draft(&snapshot)?;
        let envelope = provider_discovery_action_envelope(
            DiscoveryActionId::new(),
            snapshot.session.revision,
            ProviderDiscoveryAction::ResumeCompensation,
        )?;
        let transition = snapshot
            .session
            .apply(&envelope)
            .map_err(transition_error)?;
        self.storage
            .persist_discovery_transition(&DiscoveryTransitionWrite {
                new_operation_id: Some(DiscoveryOperationId::new()),
                transition,
                draft: DiscoveryJsonUpdate::Replace(working_draft_value(&draft)?),
                review: DiscoveryJsonUpdate::Preserve,
                new_evidence: Vec::new(),
                new_candidates: Vec::new(),
                approval: None,
                completed_operation: None,
                prepared_commit: None,
                provider_graph: None,
                occurred_at: Utc::now(),
            })?;
        self.continue_compensation(session_id)
    }

    fn require_credential_compensation_step(
        &self,
        session_id: &DiscoverySessionId,
        step_id: &str,
    ) -> CoreResult<lorepia_storage::DiscoveryCompensationRecord> {
        let snapshot = self.get(session_id)?;
        let attempt_id = snapshot
            .session
            .commit_attempt_id
            .as_ref()
            .ok_or_else(|| CoreError::invalid("discovery has no commit attempt"))?;
        self.storage
            .list_discovery_compensation_steps(attempt_id)?
            .into_iter()
            .find(|step| {
                step.id == step_id && step.kind == DiscoveryCompensationKind::RemoveCredentialSlot
            })
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::NotFound,
                    "credential compensation step was not found",
                    false,
                )
            })
    }
}

pub(super) fn compensation_recipe(
    session_id: &DiscoverySessionId,
    revision: u64,
    plan: &DiscoveryCommitPlan,
) -> Vec<PreparedDiscoveryCompensationStep> {
    let mut steps = vec![PreparedDiscoveryCompensationStep {
        id: deterministic_id(session_id, revision, "compensation:restore-selection"),
        step: DiscoveryCompensationStep {
            action_id: deterministic_action_id(
                session_id,
                revision,
                "compensation:restore-selection",
            ),
            ordinal: 0,
            kind: DiscoveryCompensationKind::RestorePreviousSelection,
            target: DiscoveryCompensationTarget::RestorePreviousSelection {
                previous_selection: plan.previous_selection.clone(),
            },
            status: DiscoveryCompensationStatus::Pending,
        },
    }];
    steps.push(PreparedDiscoveryCompensationStep {
        id: deterministic_id(session_id, revision, "compensation:remove-graph"),
        step: DiscoveryCompensationStep {
            action_id: deterministic_action_id(session_id, revision, "compensation:remove-graph"),
            ordinal: 1,
            kind: DiscoveryCompensationKind::RemoveConnectionGraph,
            target: DiscoveryCompensationTarget::RemoveConnectionGraph {
                connection_id: plan.connection_id.clone(),
            },
            status: DiscoveryCompensationStatus::Pending,
        },
    });
    if let Some(credential_ref) = &plan.credential_ref {
        steps.push(PreparedDiscoveryCompensationStep {
            id: deterministic_id(session_id, revision, "compensation:remove-credential"),
            step: DiscoveryCompensationStep {
                action_id: deterministic_action_id(
                    session_id,
                    revision,
                    "compensation:remove-credential",
                ),
                ordinal: 2,
                kind: DiscoveryCompensationKind::RemoveCredentialSlot,
                target: DiscoveryCompensationTarget::RemoveCredentialSlot {
                    connection_id: plan.connection_id.clone(),
                    credential_ref: credential_ref.clone(),
                },
                status: DiscoveryCompensationStatus::Pending,
            },
        });
    }
    steps
}

impl crate::app::Core {
    /// Returns every session with an unfinished durable operation for backend
    /// startup recovery. Unlike the user-facing history query, this scan is
    /// complete and is not capped to the latest sessions.
    pub fn list_unfinished_provider_discovery_recovery_candidates(
        &self,
    ) -> CoreResult<Vec<DiscoverySessionSnapshot>> {
        self.provider_discovery().unfinished_recovery_candidates()
    }

    /// Returns every credential-bound commit that requires native vault
    /// reconciliation before generic startup recovery may classify its WAL.
    pub fn list_provider_discovery_credential_recovery_candidates(
        &self,
    ) -> CoreResult<Vec<DiscoverySessionSnapshot>> {
        self.provider_discovery().credential_recovery_candidates()
    }

    pub fn recover_provider_discovery(
        &self,
        recovered_at: DateTime<Utc>,
    ) -> CoreResult<Vec<DiscoveryRecoveryResult>> {
        for snapshot in self.provider_discovery().credential_recovery_candidates()? {
            let context = self
                .provider_discovery()
                .credential_install_recovery_context(&snapshot.session.id)?;
            if context.operation_status == DiscoveryOperationStatus::Prepared
                && context.native_execution_id.is_none()
                && let Some(physical_authority_id) =
                    context.native_execution_reservation_id.as_deref()
            {
                // Consume the process-local capability before recovery. If the
                // durable transition later fails, fail closed: this physical
                // reservation still cannot be retried or adopted.
                self.forget_discovery_credential_reservation(physical_authority_id)?;
            }
        }
        self.provider_discovery().recover_startup(recovered_at)
    }

    pub fn list_provider_discovery_compensation_steps(
        &self,
        attempt_id: &DiscoveryCommitAttemptId,
    ) -> CoreResult<Vec<lorepia_storage::DiscoveryCompensationRecord>> {
        self.provider_discovery().compensation_steps(attempt_id)
    }

    pub fn continue_provider_discovery_compensation(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery().continue_compensation(session_id)
    }

    pub fn start_provider_discovery_credential_compensation(
        &self,
        session_id: &DiscoverySessionId,
        step_id: &str,
    ) -> CoreResult<lorepia_storage::DiscoveryCompensationRecord> {
        self.provider_discovery()
            .start_credential_compensation(session_id, step_id)
    }

    pub fn complete_provider_discovery_credential_compensation(
        &self,
        session_id: &DiscoverySessionId,
        step_id: &str,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery()
            .complete_credential_compensation(session_id, step_id)
    }

    pub fn fail_provider_discovery_credential_compensation(
        &self,
        session_id: &DiscoverySessionId,
        step_id: &str,
        failure: DiscoveryFailure,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery()
            .fail_credential_compensation(session_id, step_id, failure)
    }

    pub fn mark_provider_discovery_credential_compensation_unknown(
        &self,
        session_id: &DiscoverySessionId,
        step_id: &str,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery()
            .mark_credential_compensation_unknown(session_id, step_id)
    }

    pub fn resume_provider_discovery_compensation(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery().resume_compensation(session_id)
    }
}
