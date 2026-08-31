use super::{
    AuthBinding, CanonicalOrigin, CoreError, CoreResult, CredentialRedirectPolicy, CredentialRef,
    CredentialScope, DateTime, DiscoveryApprovalDecision, DiscoveryApprovalId,
    DiscoveryApprovalRecord, DiscoveryCommitAttemptId, DiscoveryCommitPhase,
    DiscoveryNativeCredentialExecutionRecord, DiscoveryOperationId, DiscoveryOperationKind,
    DiscoveryOperationStatus, DiscoverySessionId, DiscoverySessionSnapshot, DiscoveryState,
    DiscoveryWorkingDraft, MAX_DISCOVERY_ROWS, ProviderConnection, ProviderConnectionId,
    ProviderDiscoveryOrchestrator, ProviderDiscoverySession, ProviderTemplate, Storage, Utc,
    approval_proposal_for, canonical_serde_sha256, credential_origin_grant,
    credential_origin_proposal, hydrate_working_draft, install_graph_seed,
};

/// Exact durable context authorizing one native credential installation.
///
/// This Rust-only value contains opaque identifiers and hashes, never
/// credential material. It binds the vault slot to one approved discovery
/// commit attempt and its currently active operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDiscoveryCredentialInstallContext {
    pub session_id: DiscoverySessionId,
    pub session_revision: u64,
    pub operation_id: DiscoveryOperationId,
    pub operation_status: DiscoveryOperationStatus,
    /// Durable pre-store reservation. This may be present while the semantic
    /// operation remains `Prepared`, but it is not yet authority to write or
    /// confirm the native slot.
    pub native_execution_reservation_id: Option<String>,
    /// Physical native-slot authority after the exact reservation is sealed to
    /// the durable `Started` transition. A prepared operation always has no
    /// value here, including after reservation.
    pub native_execution_id: Option<String>,
    pub commit_attempt_id: DiscoveryCommitAttemptId,
    pub commit_plan_sha256: String,
    pub commit_phase: DiscoveryCommitPhase,
    pub connection_id: ProviderConnectionId,
    pub connection_binding_sha256: String,
}

/// Exact native proof that the current operation's authority-scoped slot was
/// observed after its durable start marker.
///
/// This Rust-only value contains no credential material. Keeping the operation
/// and commit-attempt identities separate prevents a prior retry's physical
/// slot from being adopted by a later operation which reuses the same
/// immutable commit plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDiscoveryCredentialCommitConfirmation {
    pub operation_id: DiscoveryOperationId,
    pub native_execution_id: String,
    pub commit_attempt_id: DiscoveryCommitAttemptId,
    pub commit_plan_sha256: String,
    pub connection_id: ProviderConnectionId,
    pub connection_binding_sha256: String,
}

impl TryFrom<&ProviderDiscoveryCredentialInstallContext>
    for ProviderDiscoveryCredentialCommitConfirmation
{
    type Error = CoreError;

    fn try_from(value: &ProviderDiscoveryCredentialInstallContext) -> CoreResult<Self> {
        if value.operation_status != DiscoveryOperationStatus::Started
            || value.commit_phase != DiscoveryCommitPhase::Prepared
        {
            return Err(CoreError::invalid(
                "native credential confirmation requires a started commit operation",
            ));
        }
        let native_execution_id = value.native_execution_id.clone().ok_or_else(|| {
            CoreError::invalid(
                "native credential confirmation requires a started execution incarnation",
            )
        })?;
        if value.native_execution_reservation_id.as_deref() != Some(native_execution_id.as_str()) {
            return Err(CoreError::invalid(
                "native credential confirmation differs from its reserved execution incarnation",
            ));
        }
        Ok(Self {
            operation_id: value.operation_id.clone(),
            native_execution_id,
            commit_attempt_id: value.commit_attempt_id.clone(),
            commit_plan_sha256: value.commit_plan_sha256.clone(),
            connection_id: value.connection_id.clone(),
            connection_binding_sha256: value.connection_binding_sha256.clone(),
        })
    }
}

pub(super) fn native_credential_execution_context_ids(
    operation_status: DiscoveryOperationStatus,
    operation_started_at: Option<&DateTime<Utc>>,
    native_execution: Option<DiscoveryNativeCredentialExecutionRecord>,
    recovery_context: bool,
) -> CoreResult<(Option<String>, Option<String>)> {
    match (operation_status, native_execution) {
        (DiscoveryOperationStatus::Prepared, None) if operation_started_at.is_none() => {
            Ok((None, None))
        }
        (DiscoveryOperationStatus::Prepared, Some(execution))
            if operation_started_at.is_none() && execution.store_started_at.is_none() =>
        {
            Ok((Some(execution.physical_authority_id), None))
        }
        (DiscoveryOperationStatus::Started, Some(execution))
            if execution.store_started_at.is_some()
                && operation_started_at == execution.store_started_at.as_ref() =>
        {
            let physical_authority_id = execution.physical_authority_id;
            Ok((
                Some(physical_authority_id.clone()),
                Some(physical_authority_id),
            ))
        }
        (DiscoveryOperationStatus::Started, None)
            if recovery_context && operation_started_at.is_some() =>
        {
            // Storage returns no execution only for the immutable schema-37
            // cutoff snapshot of an already-Started legacy lineage. It has no
            // physical authority and is exposed solely so startup can classify
            // the semantic operation as outcome-unknown instead of
            // synthesizing or adopting a B.
            Ok((None, None))
        }
        (DiscoveryOperationStatus::Started, None) => Err(CoreError::invalid(
            "started credential installation has no native execution authority",
        )),
        _ => Err(CoreError::invalid(
            "native credential reservation and store attempt are inconsistent",
        )),
    }
}

/// Stable pre-commit authority for one discovery-scoped native credential.
///
/// This Rust-only value never contains credential material. The approval ID
/// and grant hash name the exact credential-origin approval, while the
/// connection hash binds the eventual provider credential scope before the
/// provider graph is published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDiscoveryCredentialLeaseContext {
    pub session_id: DiscoverySessionId,
    pub connection_id: ProviderConnectionId,
    pub credential_api_origin: CanonicalOrigin,
    pub credential_origin_approval_id: DiscoveryApprovalId,
    pub credential_origin_grant_sha256: String,
    pub connection_binding_sha256: String,
}

/// Exact secure-item authority for a compensating discovery removal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDiscoveryCredentialAuthority {
    pub operation_id: DiscoveryOperationId,
    pub native_execution_id: String,
    pub commit_attempt_id: DiscoveryCommitAttemptId,
    pub connection_id: ProviderConnectionId,
    pub credential_api_origin: CanonicalOrigin,
    pub credential_origin_approval_id: DiscoveryApprovalId,
    pub credential_origin_grant_sha256: String,
    pub connection_binding_sha256: String,
}

impl ProviderDiscoveryOrchestrator<'_> {
    pub fn credential_install_context(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<ProviderDiscoveryCredentialInstallContext> {
        self.credential_install_context_inner(session_id, false)
    }

    /// Returns the exact authority for a temporary discovery credential.
    ///
    /// The authority is available only before the provider graph is committed.
    /// Before origin approval it projects the exact scope that the approval
    /// action will apply. Afterwards it requires the immutable approved record
    /// and the current connection draft to still describe that same scope.
    pub fn credential_lease_context(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<ProviderDiscoveryCredentialLeaseContext> {
        let snapshot = self.get(session_id)?;
        if snapshot.session.cancellation_pending
            || !discovery_state_accepts_credential_lease(&snapshot.session)
        {
            return Err(CoreError::invalid(
                "provider discovery is not accepting a pre-commit credential lease",
            ));
        }
        let draft = hydrate_working_draft(&snapshot)?;
        let template = draft
            .template
            .as_ref()
            .ok_or_else(|| CoreError::internal("credential lease has no template draft"))?;
        if template.default_manifest.auth == AuthBinding::None {
            return Err(CoreError::invalid(
                "provider discovery does not require a credential lease",
            ));
        }
        let current_connection = draft
            .connection
            .as_ref()
            .ok_or_else(|| CoreError::internal("credential lease has no connection draft"))?;
        require_discovery_credential_reference(&snapshot, current_connection)?;

        let (approval_id, grant_sha256, connection_binding_sha256) =
            if snapshot.session.state == DiscoveryState::AwaitingCredentialOriginApproval {
                if draft.credential_approval_id.is_some()
                    || current_connection.credential_scope.is_some()
                {
                    return Err(CoreError::invalid(
                        "credential lease was scoped before origin approval",
                    ));
                }
                let proposal = credential_origin_proposal(&snapshot, &draft)?;
                (
                    proposal.id,
                    proposal.grant_sha256,
                    canonical_discovery_credential_binding_sha256(&snapshot, &draft)?,
                )
            } else {
                let approval_id = draft.credential_approval_id.as_ref().ok_or_else(|| {
                    CoreError::invalid("credential lease has no durable origin approval")
                })?;
                let approval = self
                    .storage
                    .list_discovery_approvals(&snapshot.session.id, MAX_DISCOVERY_ROWS)?
                    .into_iter()
                    .find(|approval| &approval.id == approval_id)
                    .ok_or_else(|| {
                        CoreError::invalid("credential lease origin approval record is missing")
                    })?;
                validate_credential_origin_approval(&snapshot, &draft, &approval)?;
                let current_binding_sha256 = validated_discovery_credential_binding_sha256(
                    &snapshot,
                    &draft,
                    current_connection,
                )?;
                (
                    approval.id,
                    canonical_serde_sha256(&approval.grant, "credential-origin approval grant")?,
                    current_binding_sha256,
                )
            };

        Ok(ProviderDiscoveryCredentialLeaseContext {
            session_id: snapshot.session.id,
            connection_id: current_connection.id.clone(),
            credential_api_origin: current_connection.api_origin.clone(),
            credential_origin_approval_id: approval_id,
            credential_origin_grant_sha256: grant_sha256,
            connection_binding_sha256,
        })
    }

    /// Returns the exact install binding during startup cancellation recovery.
    ///
    /// This does not authorize a new vault write. It exists only so the native
    /// host can compare a physically-bound WAL operation with the current vault
    /// status before Core performs its conservative generic recovery. A sealed
    /// pre-schema-37 Started lineage is returned without physical authority so
    /// the host must defer it to outcome-unknown recovery.
    pub fn credential_install_recovery_context(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<ProviderDiscoveryCredentialInstallContext> {
        self.credential_install_context_inner(session_id, true)
    }

    fn validate_credential_install_context_authority(
        &self,
        recovery_context: bool,
        session_id: &DiscoverySessionId,
        attempt_id: &DiscoveryCommitAttemptId,
        plan_sha256: &str,
        operation_id: &DiscoveryOperationId,
    ) -> CoreResult<()> {
        if recovery_context {
            self.storage
                .validate_discovery_credential_install_recovery_authority(
                    session_id,
                    attempt_id,
                    plan_sha256,
                    operation_id,
                )
        } else {
            self.storage
                .validate_discovery_credential_install_operation_authority(
                    session_id,
                    attempt_id,
                    plan_sha256,
                    operation_id,
                )
        }
    }

    pub(super) fn credential_install_context_inner(
        &self,
        session_id: &DiscoverySessionId,
        recovery_context: bool,
    ) -> CoreResult<ProviderDiscoveryCredentialInstallContext> {
        let snapshot = self.get(session_id)?;
        if snapshot.session.state != DiscoveryState::Committing
            || (!recovery_context && snapshot.session.cancellation_pending)
        {
            return Err(CoreError::invalid(
                "provider discovery is not accepting a credential installation",
            ));
        }
        let attempt_id = snapshot
            .session
            .commit_attempt_id
            .as_ref()
            .ok_or_else(|| CoreError::internal("credential commit has no attempt"))?;
        let plan_sha256 = snapshot
            .session
            .commit_plan_sha256
            .as_ref()
            .ok_or_else(|| CoreError::internal("credential commit has no plan hash"))?;
        let attempt = self.storage.get_discovery_commit_attempt(attempt_id)?;
        let operation = self
            .storage
            .get_current_discovery_operation(session_id)?
            .ok_or_else(|| CoreError::internal("credential commit has no active operation"))?;
        let credential_reference =
            attempt.plan.credential_ref.as_ref().ok_or_else(|| {
                CoreError::invalid("discovery commit does not require a credential")
            })?;
        if snapshot.active_operation_id.as_ref() != Some(&operation.id)
            || operation.kind != DiscoveryOperationKind::AtomicCommit
            || !matches!(
                operation.status,
                DiscoveryOperationStatus::Prepared | DiscoveryOperationStatus::Started
            )
            || attempt.session_id != snapshot.session.id
            || attempt.plan.session_id != snapshot.session.id
            || !(operation.expected_revision == snapshot.session.revision
                || (recovery_context && snapshot.session.cancellation_pending))
            || attempt.plan_sha256 != *plan_sha256
            || attempt.plan.attempt_id != *attempt_id
            || attempt.plan.connection_id != snapshot.session.input.connection_id
            || credential_reference.as_str() != attempt.plan.connection_id.as_str()
        {
            return Err(CoreError::invalid(
                "credential installation is detached from its approved commit attempt",
            ));
        }
        self.validate_credential_install_context_authority(
            recovery_context,
            &snapshot.session.id,
            &attempt.id,
            &attempt.plan_sha256,
            &operation.id,
        )?;
        let draft = hydrate_working_draft(&snapshot)?;
        let working_connection = draft
            .connection
            .as_ref()
            .ok_or_else(|| CoreError::internal("credential commit has no connection draft"))?;
        if working_connection.id != attempt.plan.connection_id {
            return Err(CoreError::invalid(
                "credential installation connection differs from its approved commit",
            ));
        }
        let connection_binding_sha256 =
            validated_discovery_credential_binding_sha256(&snapshot, &draft, working_connection)?;
        let native_execution = self
            .storage
            .get_discovery_native_credential_execution(&operation.id)?;
        if native_execution.as_ref().is_some_and(|execution| {
            execution.operation_id != operation.id
                || execution.session_id != snapshot.session.id
                || execution.commit_attempt_id != attempt.id
                || execution.commit_plan_sha256 != attempt.plan_sha256
                || execution.connection_id != attempt.plan.connection_id
                || execution.connection_binding_sha256 != connection_binding_sha256
        }) {
            return Err(CoreError::invalid(
                "native credential execution differs from its approved commit",
            ));
        }
        let (native_execution_reservation_id, native_execution_id) =
            native_credential_execution_context_ids(
                operation.status,
                operation.started_at.as_ref(),
                native_execution,
                recovery_context,
            )?;
        Ok(ProviderDiscoveryCredentialInstallContext {
            session_id: snapshot.session.id,
            session_revision: snapshot.session.revision,
            operation_id: operation.id,
            operation_status: operation.status,
            native_execution_reservation_id,
            native_execution_id,
            commit_attempt_id: attempt.id,
            commit_plan_sha256: attempt.plan_sha256,
            commit_phase: attempt.phase,
            connection_id: attempt.plan.connection_id,
            connection_binding_sha256,
        })
    }

    pub fn credential_compensation_authority(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<ProviderDiscoveryCredentialAuthority> {
        let snapshot = self.get(session_id)?;
        if snapshot.session.state != DiscoveryState::Compensating {
            return Err(CoreError::invalid(
                "provider discovery is not compensating a credential installation",
            ));
        }
        let attempt_id = snapshot
            .session
            .commit_attempt_id
            .as_ref()
            .ok_or_else(|| CoreError::internal("credential compensation has no attempt"))?;
        let attempt = self.storage.get_discovery_commit_attempt(attempt_id)?;
        let draft = hydrate_working_draft(&snapshot)?;
        let connection = draft.connection.as_ref().ok_or_else(|| {
            CoreError::internal("credential compensation has no connection draft")
        })?;
        if attempt.session_id != snapshot.session.id
            || attempt.plan.attempt_id != *attempt_id
            || attempt.plan.connection_id != connection.id
            || attempt
                .plan
                .credential_ref
                .as_ref()
                .map(CredentialRef::as_str)
                != Some(connection.id.as_str())
        {
            return Err(CoreError::invalid(
                "credential compensation differs from its immutable commit attempt",
            ));
        }
        let operation_id = self
            .storage
            .get_discovery_credential_compensation_operation_id(
                &snapshot.session.id,
                &attempt.id,
                &attempt.plan_sha256,
            )?;
        let connection_binding_sha256 =
            validated_discovery_credential_binding_sha256(&snapshot, &draft, connection)?;
        let (credential_origin_approval_id, credential_origin_grant_sha256) =
            approved_discovery_credential_origin_authority(self.storage, &snapshot, &draft)?;
        if attempt.plan.credential_approval_id.as_ref() != Some(&credential_origin_approval_id) {
            return Err(CoreError::invalid(
                "credential compensation origin approval differs from its immutable commit",
            ));
        }
        let native_execution = self
            .storage
            .get_discovery_native_credential_execution(&operation_id)?
            .ok_or_else(|| {
                CoreError::invalid(
                    "credential compensation has no producing native execution authority",
                )
            })?;
        if native_execution.operation_id != operation_id
            || native_execution.session_id != snapshot.session.id
            || native_execution.commit_attempt_id != attempt.id
            || native_execution.commit_plan_sha256 != attempt.plan_sha256
            || native_execution.connection_id != connection.id
            || native_execution.connection_binding_sha256 != connection_binding_sha256
            || native_execution.store_started_at.is_none()
        {
            return Err(CoreError::invalid(
                "credential compensation native execution differs from its immutable commit",
            ));
        }
        Ok(ProviderDiscoveryCredentialAuthority {
            operation_id,
            native_execution_id: native_execution.physical_authority_id,
            commit_attempt_id: attempt.id,
            connection_id: connection.id.clone(),
            credential_api_origin: connection.api_origin.clone(),
            credential_origin_approval_id,
            credential_origin_grant_sha256,
            connection_binding_sha256,
        })
    }
}

pub(super) fn apply_credential_origin_scope(
    template: &ProviderTemplate,
    connection: &mut ProviderConnection,
) {
    connection.credential_scope = Some(CredentialScope {
        allowed_origins: vec![connection.api_origin.clone()],
        auth_binding: template.default_manifest.auth.clone(),
        redirect_policy: CredentialRedirectPolicy::Deny,
    });
}

fn canonical_discovery_credential_connection(
    snapshot: &DiscoverySessionSnapshot,
    draft: &DiscoveryWorkingDraft,
) -> CoreResult<ProviderConnection> {
    let template = draft
        .template
        .clone()
        .ok_or_else(|| CoreError::internal("credential lease has no template draft"))?;
    let mut canonical = DiscoveryWorkingDraft::new(draft.source.clone());
    canonical.deterministic.clone_from(&draft.deterministic);
    install_graph_seed(snapshot, &mut canonical, template, snapshot.created_at)?;
    canonical
        .connection
        .ok_or_else(|| CoreError::internal("credential lease connection could not be rebuilt"))
}

fn canonical_discovery_credential_binding_sha256(
    snapshot: &DiscoverySessionSnapshot,
    draft: &DiscoveryWorkingDraft,
) -> CoreResult<String> {
    let template = draft
        .template
        .as_ref()
        .ok_or_else(|| CoreError::internal("credential lease has no template draft"))?;
    let mut connection = canonical_discovery_credential_connection(snapshot, draft)?;
    apply_credential_origin_scope(template, &mut connection);
    lorepia_storage::provider_credential_binding_sha256_for_connection(&connection)
}

pub(super) fn validated_discovery_credential_binding_sha256(
    snapshot: &DiscoverySessionSnapshot,
    draft: &DiscoveryWorkingDraft,
    connection: &ProviderConnection,
) -> CoreResult<String> {
    let current = lorepia_storage::provider_credential_binding_sha256_for_connection(connection)?;
    if current != canonical_discovery_credential_binding_sha256(snapshot, draft)? {
        return Err(CoreError::invalid(
            "provider credential binding changed after origin approval",
        ));
    }
    Ok(current)
}

fn require_discovery_credential_reference(
    snapshot: &DiscoverySessionSnapshot,
    connection: &ProviderConnection,
) -> CoreResult<()> {
    let input_reference = snapshot
        .session
        .input
        .credential_ref
        .as_ref()
        .ok_or_else(|| CoreError::invalid("credential lease has no opaque credential reference"))?;
    if input_reference.as_str() != snapshot.session.input.connection_id.as_str()
        || connection.id != snapshot.session.input.connection_id
        || connection.credential_ref.as_ref() != Some(input_reference)
    {
        return Err(CoreError::invalid(
            "credential lease reference is detached from its discovery connection",
        ));
    }
    Ok(())
}

fn discovery_state_accepts_credential_lease(session: &ProviderDiscoverySession) -> bool {
    match session.state {
        DiscoveryState::AwaitingCredentialOriginApproval
        | DiscoveryState::ListingModels
        | DiscoveryState::AwaitingProbeConsent
        | DiscoveryState::ProbingCapabilities
        | DiscoveryState::AwaitingReview
        | DiscoveryState::Committing => true,
        DiscoveryState::Interrupted => session.recovery.as_ref().is_some_and(|checkpoint| {
            matches!(
                checkpoint.operation,
                DiscoveryOperationKind::ListModels | DiscoveryOperationKind::ProbeCapabilities
            )
        }),
        _ => false,
    }
}

fn approved_discovery_credential_origin_authority(
    storage: &Storage,
    snapshot: &DiscoverySessionSnapshot,
    draft: &DiscoveryWorkingDraft,
) -> CoreResult<(DiscoveryApprovalId, String)> {
    let approval_id = draft
        .credential_approval_id
        .as_ref()
        .ok_or_else(|| CoreError::invalid("credential lease has no durable origin approval"))?;
    let approval = storage
        .list_discovery_approvals(&snapshot.session.id, MAX_DISCOVERY_ROWS)?
        .into_iter()
        .find(|approval| &approval.id == approval_id)
        .ok_or_else(|| CoreError::invalid("credential lease origin approval record is missing"))?;
    validate_credential_origin_approval(snapshot, draft, &approval)?;
    Ok((
        approval.id,
        canonical_serde_sha256(&approval.grant, "credential-origin approval grant")?,
    ))
}

pub(super) fn validate_credential_origin_approval(
    snapshot: &DiscoverySessionSnapshot,
    draft: &DiscoveryWorkingDraft,
    approval: &DiscoveryApprovalRecord,
) -> CoreResult<()> {
    if approval.session_id != snapshot.session.id
        || approval.decision != DiscoveryApprovalDecision::Approved
        || Some(&approval.id) != draft.credential_approval_id.as_ref()
    {
        return Err(CoreError::invalid(
            "credential lease origin approval is not valid for this session",
        ));
    }
    let expected_grant = credential_origin_grant(snapshot, draft)?;
    if approval.grant != expected_grant {
        return Err(CoreError::invalid(
            "credential lease differs from its approved origin or authentication binding",
        ));
    }
    let proposal = approval_proposal_for(
        &snapshot.session.id,
        approval.session_revision,
        approval.grant.clone(),
    )?;
    if proposal.id != approval.id {
        return Err(CoreError::invalid(
            "credential lease origin approval identifier is not canonical",
        ));
    }
    let template = draft
        .template
        .as_ref()
        .ok_or_else(|| CoreError::internal("credential lease has no template draft"))?;
    let connection = draft
        .connection
        .as_ref()
        .ok_or_else(|| CoreError::internal("credential lease has no connection draft"))?;
    let mut expected_connection = connection.clone();
    apply_credential_origin_scope(template, &mut expected_connection);
    if connection.credential_scope != expected_connection.credential_scope {
        return Err(CoreError::invalid(
            "provider credential scope changed after origin approval",
        ));
    }
    Ok(())
}

impl crate::app::Core {
    pub fn get_provider_discovery_credential_install_context(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<ProviderDiscoveryCredentialInstallContext> {
        self.provider_discovery()
            .credential_install_context(session_id)
    }

    pub fn get_provider_discovery_credential_lease_context(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<ProviderDiscoveryCredentialLeaseContext> {
        self.provider_discovery()
            .credential_lease_context(session_id)
    }

    pub fn get_provider_discovery_credential_install_recovery_context(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<ProviderDiscoveryCredentialInstallContext> {
        self.provider_discovery()
            .credential_install_recovery_context(session_id)
    }

    pub fn get_provider_discovery_credential_compensation_authority(
        &self,
        session_id: &DiscoverySessionId,
    ) -> CoreResult<ProviderDiscoveryCredentialAuthority> {
        self.provider_discovery()
            .credential_compensation_authority(session_id)
    }
}
