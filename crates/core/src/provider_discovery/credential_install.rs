use super::{
    CoreError, CoreErrorCode, CoreResult, DiscoveryCommitAttemptId, DiscoveryCommitPhase,
    DiscoveryEvidenceRecord, DiscoveryInterruptionOutcome, DiscoveryJsonUpdate,
    DiscoveryNativeCredentialExecutionReservation, DiscoveryNativeCredentialStoreAttemptStart,
    DiscoveryNativeNoEffectAttestationWrite, DiscoveryOperationId, DiscoveryOperationKind,
    DiscoveryOperationStatus, DiscoveryRecoveryOwner, DiscoveryReviewDiff, DiscoverySessionId,
    DiscoverySessionSnapshot, DiscoveryState, DiscoveryWorkingDraft, DurableOperationOutcome,
    ProviderConnectionId, ProviderDiscoveryAction, ProviderDiscoveryCredentialInstallContext,
    ProviderDiscoveryOrchestrator, StoredDiscoveryCandidate, Utc, hydrate_working_draft,
};

impl ProviderDiscoveryOrchestrator<'_> {
    /// Reserves a fresh physical slot incarnation while the semantic operation
    /// remains Prepared. Native fallible preconditions run against this exact
    /// reservation before any durable store-attempt intent is recorded.
    pub fn reserve_credential_install(
        &self,
        session_id: &DiscoverySessionId,
        expected_revision: u64,
        expected_operation_id: &DiscoveryOperationId,
        expected_attempt_id: &DiscoveryCommitAttemptId,
        expected_plan_sha256: &str,
    ) -> CoreResult<ProviderDiscoveryCredentialInstallContext> {
        let context = self.credential_install_context(session_id)?;
        if context.session_revision != expected_revision
            || &context.operation_id != expected_operation_id
            || &context.commit_attempt_id != expected_attempt_id
            || context.commit_plan_sha256 != expected_plan_sha256
            || context.commit_phase != DiscoveryCommitPhase::Prepared
            || context.operation_status != DiscoveryOperationStatus::Prepared
            || context.native_execution_id.is_some()
        {
            return Err(CoreError::invalid(
                "credential installation context changed before native reservation",
            ));
        }
        if context.native_execution_reservation_id.is_some() {
            return Err(CoreError::invalid(
                "prepared credential reservation already exists and requires recovery",
            ));
        }
        let reservation = DiscoveryNativeCredentialExecutionReservation {
            operation_id: context.operation_id.clone(),
            session_id: context.session_id.clone(),
            commit_attempt_id: context.commit_attempt_id.clone(),
            commit_plan_sha256: context.commit_plan_sha256.clone(),
            connection_id: context.connection_id.clone(),
            connection_binding_sha256: context.connection_binding_sha256.clone(),
            reserved_at: Utc::now(),
        };
        let execution = self
            .storage
            .reserve_discovery_credential_install_execution(&reservation)?;
        let reserved = self.credential_install_context(session_id)?;
        if reserved.operation_id != context.operation_id
            || reserved.operation_status != DiscoveryOperationStatus::Prepared
            || reserved.commit_phase != DiscoveryCommitPhase::Prepared
            || reserved.native_execution_id.is_some()
            || reserved.native_execution_reservation_id.as_deref()
                != Some(execution.physical_authority_id.as_str())
        {
            return Err(CoreError::internal(
                "credential installation reservation was not durably bound",
            ));
        }
        Ok(reserved)
    }

    /// Durably records that the exact reserved physical slot is the next
    /// external action, and atomically moves the semantic operation to Started.
    pub fn start_credential_install(
        &self,
        session_id: &DiscoverySessionId,
        expected_revision: u64,
        expected_operation_id: &DiscoveryOperationId,
        expected_attempt_id: &DiscoveryCommitAttemptId,
        expected_plan_sha256: &str,
        expected_native_execution_reservation_id: &str,
    ) -> CoreResult<ProviderDiscoveryCredentialInstallContext> {
        let context = self.credential_install_context(session_id)?;
        if context.session_revision != expected_revision
            || &context.operation_id != expected_operation_id
            || &context.commit_attempt_id != expected_attempt_id
            || context.commit_plan_sha256 != expected_plan_sha256
            || context.commit_phase != DiscoveryCommitPhase::Prepared
            || context.operation_status != DiscoveryOperationStatus::Prepared
            || context.native_execution_id.is_some()
            || context.native_execution_reservation_id.as_deref()
                != Some(expected_native_execution_reservation_id)
        {
            return Err(CoreError::invalid(
                "credential installation reservation changed before native store",
            ));
        }
        let execution_start = DiscoveryNativeCredentialStoreAttemptStart {
            operation_id: context.operation_id.clone(),
            physical_authority_id: expected_native_execution_reservation_id.to_owned(),
            started_at: Utc::now(),
        };
        let execution = self
            .storage
            .start_reserved_discovery_credential_install_execution(&execution_start)?;
        let started = self.credential_install_context(session_id)?;
        if started.operation_id != context.operation_id
            || started.operation_status != DiscoveryOperationStatus::Started
            || started.commit_phase != DiscoveryCommitPhase::Prepared
            || started.native_execution_reservation_id.as_deref()
                != Some(execution.physical_authority_id.as_str())
            || started.native_execution_id.as_deref()
                != Some(execution.physical_authority_id.as_str())
        {
            return Err(CoreError::internal(
                "credential installation start was not durably bound",
            ));
        }
        Ok(started)
    }

    /// Records a platform-attested missing vault slot after an installation
    /// attempt, without guessing or retrying an external effect.
    pub fn attest_credential_install_no_effect(
        &self,
        session_id: &DiscoverySessionId,
        expected_operation_id: &DiscoveryOperationId,
        expected_attempt_id: &DiscoveryCommitAttemptId,
        expected_plan_sha256: &str,
        expected_native_execution_id: &str,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        if self.recovery_owner != DiscoveryRecoveryOwner::NativePlatform {
            return Err(CoreError::new(
                CoreErrorCode::PermissionDenied,
                "native credential no-effect attestation requires the native recovery owner",
                false,
            ));
        }
        let context = self.credential_install_context_inner(session_id, true)?;
        if &context.operation_id != expected_operation_id
            || &context.commit_attempt_id != expected_attempt_id
            || context.commit_plan_sha256 != expected_plan_sha256
            || context.commit_phase != DiscoveryCommitPhase::Prepared
            || context.operation_status != DiscoveryOperationStatus::Started
            || context.native_execution_id.as_deref() != Some(expected_native_execution_id)
        {
            return Err(CoreError::invalid(
                "credential no-effect attestation does not match the active commit",
            ));
        }
        let snapshot = self.get(session_id)?;
        let mut draft = hydrate_working_draft(&snapshot)?;
        self.persist_native_no_effect_completion(
            &snapshot,
            &context.operation_id,
            &mut draft,
            ProviderDiscoveryAction::Interrupt {
                operation: DiscoveryOperationKind::AtomicCommit,
                outcome: DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
            },
            DurableOperationOutcome::AttestedNoExternalEffect,
            Vec::new(),
            Vec::new(),
            DiscoveryJsonUpdate::Preserve,
            &context,
        )?;
        self.get(session_id)
    }

    /// Durably records that the exact native credential store attempt reported
    /// an explicit durability failure after it may have mutated its slot.
    ///
    /// Immediate vault visibility is deliberately not accepted here: a native
    /// platform can expose the new bytes while failing the directory/fsync
    /// boundary needed to survive a crash. The complete immutable execution
    /// authority is compared before the active atomic-commit operation is
    /// closed as outcome-unknown in the same discovery transaction.
    #[allow(clippy::too_many_arguments)]
    pub fn mark_credential_install_durability_unknown(
        &self,
        session_id: &DiscoverySessionId,
        expected_revision: u64,
        expected_operation_id: &DiscoveryOperationId,
        expected_attempt_id: &DiscoveryCommitAttemptId,
        expected_plan_sha256: &str,
        expected_native_execution_id: &str,
        expected_connection_id: &ProviderConnectionId,
        expected_connection_binding_sha256: &str,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        if self.recovery_owner != DiscoveryRecoveryOwner::NativePlatform {
            return Err(CoreError::new(
                CoreErrorCode::PermissionDenied,
                "native credential durability attestation requires the native recovery owner",
                false,
            ));
        }
        let context = self.credential_install_context_inner(session_id, true)?;
        if context.session_revision != expected_revision
            || &context.operation_id != expected_operation_id
            || &context.commit_attempt_id != expected_attempt_id
            || context.commit_plan_sha256 != expected_plan_sha256
            || context.commit_phase != DiscoveryCommitPhase::Prepared
            || context.operation_status != DiscoveryOperationStatus::Started
            || context.native_execution_reservation_id.as_deref()
                != Some(expected_native_execution_id)
            || context.native_execution_id.as_deref() != Some(expected_native_execution_id)
            || &context.connection_id != expected_connection_id
            || context.connection_binding_sha256 != expected_connection_binding_sha256
        {
            return Err(CoreError::invalid(
                "credential durability failure does not match the active native commit",
            ));
        }
        let snapshot = self.get(session_id)?;
        if snapshot.session.revision != expected_revision
            || snapshot.active_operation_id.as_ref() != Some(expected_operation_id)
        {
            return Err(CoreError::invalid(
                "credential durability failure changed before settlement",
            ));
        }
        let mut draft = hydrate_working_draft(&snapshot)?;
        self.persist_operation_completion(
            &snapshot,
            expected_operation_id,
            &mut draft,
            ProviderDiscoveryAction::ExternalOutcomeBecameUnknown,
            DurableOperationOutcome::OutcomeUnknown,
            Vec::new(),
            Vec::new(),
            DiscoveryJsonUpdate::Preserve,
        )?;
        self.get(session_id)
    }

    #[allow(clippy::too_many_arguments)]
    fn persist_native_no_effect_completion(
        &self,
        snapshot: &DiscoverySessionSnapshot,
        operation_id: &DiscoveryOperationId,
        draft: &mut DiscoveryWorkingDraft,
        action: ProviderDiscoveryAction,
        outcome: DurableOperationOutcome,
        evidence: Vec<DiscoveryEvidenceRecord>,
        candidates: Vec<StoredDiscoveryCandidate>,
        review: DiscoveryJsonUpdate<DiscoveryReviewDiff>,
        context: &ProviderDiscoveryCredentialInstallContext,
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
        let physical_authority_id = context.native_execution_id.clone().ok_or_else(|| {
            CoreError::invalid("native no-effect attestation has no started physical authority")
        })?;
        let attestation = DiscoveryNativeNoEffectAttestationWrite::credential_slot_missing(
            context.operation_id.clone(),
            physical_authority_id,
            context.session_id.clone(),
            context.commit_attempt_id.clone(),
            context.commit_plan_sha256.clone(),
            context.connection_id.clone(),
        )?;
        self.storage
            .persist_native_no_effect_discovery_transition(&write, &attestation)?;
        Ok(())
    }
}

impl crate::app::Core {
    pub(super) fn prepared_discovery_credential_reservation_id(
        &self,
        session_id: &DiscoverySessionId,
        expected_revision: u64,
    ) -> CoreResult<Option<String>> {
        let snapshot = self.provider_discovery().get(session_id)?;
        if snapshot.session.revision != expected_revision
            || snapshot.session.state != DiscoveryState::Committing
            || snapshot.session.input.credential_ref.is_none()
        {
            return Ok(None);
        }
        let context = self
            .provider_discovery()
            .credential_install_recovery_context(session_id)?;
        Ok(
            (context.operation_status == DiscoveryOperationStatus::Prepared
                && context.native_execution_id.is_none())
            .then_some(context.native_execution_reservation_id)
            .flatten(),
        )
    }

    pub fn reserve_provider_discovery_credential_install(
        &self,
        session_id: &DiscoverySessionId,
        expected_revision: u64,
        expected_operation_id: &DiscoveryOperationId,
        expected_attempt_id: &DiscoveryCommitAttemptId,
        expected_plan_sha256: &str,
    ) -> CoreResult<ProviderDiscoveryCredentialInstallContext> {
        let reserved = self.provider_discovery().reserve_credential_install(
            session_id,
            expected_revision,
            expected_operation_id,
            expected_attempt_id,
            expected_plan_sha256,
        )?;
        let physical_authority_id = reserved
            .native_execution_reservation_id
            .as_deref()
            .ok_or_else(|| CoreError::internal("credential reservation has no physical id"))?;
        self.remember_discovery_credential_reservation(physical_authority_id)?;
        Ok(reserved)
    }

    pub fn start_provider_discovery_credential_install(
        &self,
        session_id: &DiscoverySessionId,
        expected_revision: u64,
        expected_operation_id: &DiscoveryOperationId,
        expected_attempt_id: &DiscoveryCommitAttemptId,
        expected_plan_sha256: &str,
        expected_native_execution_reservation_id: &str,
    ) -> CoreResult<ProviderDiscoveryCredentialInstallContext> {
        let preflight = self
            .provider_discovery()
            .credential_install_context(session_id)?;
        if preflight.session_revision != expected_revision
            || &preflight.operation_id != expected_operation_id
            || &preflight.commit_attempt_id != expected_attempt_id
            || preflight.commit_plan_sha256 != expected_plan_sha256
            || preflight.commit_phase != DiscoveryCommitPhase::Prepared
            || preflight.operation_status != DiscoveryOperationStatus::Prepared
            || preflight.native_execution_id.is_some()
            || preflight.native_execution_reservation_id.as_deref()
                != Some(expected_native_execution_reservation_id)
        {
            return Err(CoreError::invalid(
                "credential installation reservation changed before process-local start",
            ));
        }
        self.consume_discovery_credential_reservation(expected_native_execution_reservation_id)?;
        self.provider_discovery().start_credential_install(
            session_id,
            expected_revision,
            expected_operation_id,
            expected_attempt_id,
            expected_plan_sha256,
            expected_native_execution_reservation_id,
        )
    }

    pub fn attest_provider_discovery_credential_install_no_effect(
        &self,
        session_id: &DiscoverySessionId,
        expected_operation_id: &DiscoveryOperationId,
        expected_attempt_id: &DiscoveryCommitAttemptId,
        expected_plan_sha256: &str,
        expected_native_execution_id: &str,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery()
            .attest_credential_install_no_effect(
                session_id,
                expected_operation_id,
                expected_attempt_id,
                expected_plan_sha256,
                expected_native_execution_id,
            )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn mark_provider_discovery_credential_install_durability_unknown(
        &self,
        session_id: &DiscoverySessionId,
        expected_revision: u64,
        expected_operation_id: &DiscoveryOperationId,
        expected_attempt_id: &DiscoveryCommitAttemptId,
        expected_plan_sha256: &str,
        expected_native_execution_id: &str,
        expected_connection_id: &ProviderConnectionId,
        expected_connection_binding_sha256: &str,
    ) -> CoreResult<DiscoverySessionSnapshot> {
        self.provider_discovery()
            .mark_credential_install_durability_unknown(
                session_id,
                expected_revision,
                expected_operation_id,
                expected_attempt_id,
                expected_plan_sha256,
                expected_native_execution_id,
                expected_connection_id,
                expected_connection_binding_sha256,
            )
    }
}
