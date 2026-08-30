use chrono::Utc;
use lorepia_domain::{
    Character, ConversationBranchId, ConversationId, ConversationMode, CoreError, CoreErrorCode,
    CoreResult, GenerationId, GenerationTarget, MessageId, Sha256Digest, VariableMap,
    prompt_local_user_id_sha256,
};
use lorepia_storage::{GenerationProviderTargetAuthority, ProviderCredentialAccessAuthority};

use super::{
    GenerationActionTargetIdentity, GenerationOperationContext, PromptRouteWireContract,
    ResolvedGenerationOperationIdentity, ResolvedGenerationTarget,
    SameBranchGenerationAttemptIdentity, generation_target_provider_authority,
    new_generation_operation_id, require_generation_provider_target_authority,
    same_branch_generation_semantic_fingerprint,
    validate_generation_target_plan_with_reasoning_effort, validate_reviewed_generation_attempt_id,
    validate_same_branch_attempt_semantic_identity,
};
use crate::app::{Core, validate_user_message_text};
use crate::orchestration::GenerationPromptAuthorityCapture;

struct ExistingSameBranchAttemptRequest<'a> {
    conversation_id: &'a ConversationId,
    branch_id: &'a ConversationBranchId,
    expected_head: Option<&'a MessageId>,
    operation_id: &'a str,
    base_request_fingerprint_sha256: &'a Sha256Digest,
    provider_target_authority: &'a GenerationProviderTargetAuthority,
    resume_generation_attempt_id: Option<&'a GenerationId>,
}

enum ExistingSameBranchAttempt {
    Missing,
    Prepared(Box<lorepia_storage::StoredGenerationAttempt>),
    Resolved(SameBranchGenerationAttempt),
}

pub(in crate::app) enum SameBranchGenerationAttempt {
    Existing(GenerationId),
    Ready(Box<PreparedSameBranchGenerationAttempt>),
}

pub(in crate::app) struct PreparedSameBranchGenerationAttempt {
    pub(in crate::app) attempt: lorepia_storage::StoredGenerationAttempt,
    pub(in crate::app) interaction_state: lorepia_storage::StoredInteractionState,
    pub(in crate::app) applied_module_plan: Option<lorepia_orchestration::AppliedModuleRuntimePlan>,
}

impl Core {
    pub(in crate::app) fn ensure_interaction_state_available(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
    ) -> CoreResult<()> {
        self.drain_available_core_lifecycle_occurrences()?;
        self.inner
            .storage
            .get_interaction_state_snapshot(conversation_id, branch_id)
            .map(|_| ())
            .map_err(|error| {
                if error.code == CoreErrorCode::NotFound {
                    CoreError::new(
                        CoreErrorCode::StorageUnavailable,
                        "interaction lifecycle initialization is backlogged; retry the request",
                        true,
                    )
                } else {
                    error
                }
            })
    }

    pub(in crate::app) fn resolve_same_branch_generation_operation_identity(
        &self,
        input: SameBranchGenerationAttemptIdentity<'_>,
    ) -> CoreResult<ResolvedGenerationOperationIdentity> {
        let base_request_fingerprint_sha256 = same_branch_generation_semantic_fingerprint(&input)?;
        let (operation_id, resume_generation_attempt_id) = match input.operation_context {
            GenerationOperationContext::New { operation_nonce } => (
                new_generation_operation_id(
                    "generation-send-v5",
                    &base_request_fingerprint_sha256,
                    operation_nonce,
                )?,
                None,
            ),
            GenerationOperationContext::Resume {
                generation_attempt_id,
            } => {
                let attempt = self
                    .inner
                    .storage
                    .get_generation_attempt(generation_attempt_id)?;
                validate_same_branch_attempt_semantic_identity(
                    &attempt,
                    input.conversation_id,
                    input.branch_id,
                    input.expected_head,
                    &base_request_fingerprint_sha256,
                    Some(generation_attempt_id),
                )?;
                (
                    attempt.input.operation_id,
                    Some(generation_attempt_id.clone()),
                )
            }
        };
        Ok(ResolvedGenerationOperationIdentity {
            operation_id,
            base_request_fingerprint_sha256,
            resume_generation_attempt_id,
        })
    }

    pub(in crate::app) fn preflight_same_branch_provider_authority(
        &self,
        input: SameBranchGenerationAttemptIdentity<'_>,
        provider_target_authority: &GenerationProviderTargetAuthority,
    ) -> CoreResult<()> {
        let conversation_id = input.conversation_id;
        let branch_id = input.branch_id;
        let expected_head = input.expected_head;
        let is_resume = matches!(
            input.operation_context,
            GenerationOperationContext::Resume { .. }
        );
        let operation = self.resolve_same_branch_generation_operation_identity(input)?;
        match self
            .inner
            .storage
            .get_generation_attempt_by_operation_id(conversation_id, &operation.operation_id)
        {
            Ok(attempt) => {
                validate_same_branch_attempt_semantic_identity(
                    &attempt,
                    conversation_id,
                    branch_id,
                    expected_head,
                    &operation.base_request_fingerprint_sha256,
                    operation.resume_generation_attempt_id.as_ref(),
                )?;
                require_generation_provider_target_authority(&attempt, provider_target_authority)
            }
            Err(error) if error.code == CoreErrorCode::NotFound && !is_resume => Ok(()),
            Err(error) if error.code == CoreErrorCode::NotFound => Err(CoreError::new(
                CoreErrorCode::InvalidInput,
                "generation resume attempt is unavailable; start a new generation operation",
                true,
            )),
            Err(error) => Err(error),
        }
    }

    /// Prepares or resumes the isolated attempt shared by expert preview and
    /// reviewed send. `expected_plan_hash` is intentionally absent from the
    /// operation identity; it is validated later against the resolved plan.
    pub(in crate::app) fn prepare_reviewed_prompt_generation_attempt(
        &self,
        plan_request: &crate::PromptPlanRequest,
        operation_context: GenerationOperationContext<'_>,
        mode: ConversationMode,
        resolved: &ResolvedGenerationTarget,
    ) -> CoreResult<SameBranchGenerationAttempt> {
        let text = validate_user_message_text(&plan_request.user_text)?;
        let conversation = self
            .inner
            .storage
            .get_conversation(&plan_request.conversation_id)?;
        let character = self
            .inner
            .storage
            .get_character(&conversation.character_id)?;
        let validated = validate_generation_target_plan_with_reasoning_effort(
            self,
            &plan_request.generation_target,
            resolved.prompt_wire_contract.reasoning_effort_applied,
        )?;
        let provider_target_authority =
            generation_target_provider_authority(&plan_request.generation_target, &validated)?;
        let operation_target = GenerationActionTargetIdentity::GenerationTarget {
            model_route_id: plan_request.generation_target.model_route_id.clone(),
            generation_preset_id: plan_request.generation_target.generation_preset_id.clone(),
        };
        self.prepare_same_branch_generation_attempt(
            &character,
            &plan_request.conversation_id,
            &plan_request.branch_id,
            plan_request.expected_head.as_ref(),
            mode,
            text,
            operation_context,
            Some(&plan_request.generation_target),
            None,
            None,
            plan_request.prompt_preset_id.as_ref(),
            &plan_request.variable_overrides,
            Some(&resolved.prompt_wire_contract),
            &operation_target,
            &provider_target_authority,
            None,
            false,
        )
    }

    pub(in crate::app) fn validate_existing_reviewed_generation(
        &self,
        generation_id: GenerationId,
        expected_generation_attempt_id: &GenerationId,
        expected_plan_hash: &str,
    ) -> CoreResult<GenerationId> {
        validate_reviewed_generation_attempt_id(expected_generation_attempt_id, &generation_id)?;
        let stored_plan = self.get_generation_prompt_plan(&generation_id)?;
        if stored_plan.id != expected_plan_hash {
            return Err(CoreError::invalid(
                "prompt plan changed after preview; resolve a new preview before sending",
            ));
        }
        Ok(generation_id)
    }

    fn existing_same_branch_generation_attempt(
        &self,
        request: ExistingSameBranchAttemptRequest<'_>,
    ) -> CoreResult<ExistingSameBranchAttempt> {
        let existing = match self
            .inner
            .storage
            .get_generation_attempt_by_operation_id(request.conversation_id, request.operation_id)
        {
            Ok(existing) => existing,
            Err(error) if error.code == CoreErrorCode::NotFound => {
                return Ok(ExistingSameBranchAttempt::Missing);
            }
            Err(error) => return Err(error),
        };
        validate_same_branch_attempt_semantic_identity(
            &existing,
            request.conversation_id,
            request.branch_id,
            request.expected_head,
            request.base_request_fingerprint_sha256,
            request.resume_generation_attempt_id,
        )?;
        require_generation_provider_target_authority(&existing, request.provider_target_authority)?;
        if matches!(
            existing.status,
            lorepia_storage::GenerationAttemptStatus::Running
                | lorepia_storage::GenerationAttemptStatus::Completed
        ) {
            return Ok(ExistingSameBranchAttempt::Resolved(
                SameBranchGenerationAttempt::Existing(existing.generation_id),
            ));
        }
        if existing.status == lorepia_storage::GenerationAttemptStatus::Prepared {
            return Ok(ExistingSameBranchAttempt::Prepared(Box::new(existing)));
        }
        let before = self
            .inner
            .storage
            .get_generation_attempt_before_review(&existing.generation_id)?
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "generation attempt is missing its immutable review",
                    false,
                )
            })?;
        self.advance_same_branch_generation_attempt(
            existing,
            request.conversation_id,
            request.branch_id,
            None,
            before.applied_runtime_plan,
        )
        .map(ExistingSameBranchAttempt::Resolved)
    }

    fn validate_new_same_branch_module_authority(
        &self,
        character: &Character,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        mode: ConversationMode,
        prompt_preset_id: Option<&lorepia_domain::PromptPresetId>,
        module_plan_sha256: &Sha256Digest,
    ) -> CoreResult<()> {
        let prompt_module_plan_sha256 = self.resolve_generation_module_plan_sha256(
            character,
            conversation_id,
            branch_id,
            mode,
            prompt_preset_id,
        )?;
        if prompt_module_plan_sha256 != *module_plan_sha256 {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "prompt and interaction module runtime authorities diverged",
                false,
            ));
        }
        Ok(())
    }

    fn revalidate_prepared_same_branch_credential_authority(
        &self,
        existing: Option<&lorepia_storage::StoredGenerationAttempt>,
        provider_target_authority: &GenerationProviderTargetAuthority,
        credential_authority: Option<&ProviderCredentialAccessAuthority>,
        require_exact_credential_authority: bool,
    ) -> CoreResult<()> {
        let Some(existing) = existing else {
            return Ok(());
        };
        require_generation_provider_target_authority(existing, provider_target_authority)?;
        if require_exact_credential_authority {
            self.inner
                .storage
                .prepare_generation_attempt_with_credential_authority(
                    &existing.input,
                    existing.created_at,
                    credential_authority,
                )?;
        }
        Ok(())
    }

    fn resolve_same_branch_module_authority(
        &self,
        existing: Option<&lorepia_storage::StoredGenerationAttempt>,
        character: &Character,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        mode: ConversationMode,
        prompt_preset_id: Option<&lorepia_domain::PromptPresetId>,
    ) -> CoreResult<(
        lorepia_orchestration::ModuleMergeReview,
        Option<lorepia_orchestration::AppliedModuleRuntimePlan>,
        Sha256Digest,
    )> {
        let (module_runtime_review, applied_module_plan) = if let Some(existing) = existing {
            let (review, plan) = generation_attempt_module_authority(existing)?;
            (review.clone(), plan.cloned())
        } else {
            self.preview_module_runtime_authority_for_proposed_branch(conversation_id, branch_id)?
        };
        let module_plan_sha256 = applied_module_plan.as_ref().map_or_else(
            lorepia_orchestration::no_applied_module_runtime_plan_sha256,
            |plan| plan.applied_plan_sha256.clone(),
        );
        if existing.is_none() {
            self.validate_new_same_branch_module_authority(
                character,
                conversation_id,
                branch_id,
                mode,
                prompt_preset_id,
                &module_plan_sha256,
            )?;
        }
        Ok((
            module_runtime_review,
            applied_module_plan,
            module_plan_sha256,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::app) fn prepare_same_branch_generation_attempt(
        &self,
        character: &Character,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        mode: ConversationMode,
        text: &str,
        operation_context: GenerationOperationContext<'_>,
        generation_target: Option<&GenerationTarget>,
        temperature: Option<f64>,
        max_output_tokens: Option<u32>,
        prompt_preset_id: Option<&lorepia_domain::PromptPresetId>,
        variable_overrides: &VariableMap,
        prompt_wire_contract: Option<&PromptRouteWireContract>,
        operation_target: &GenerationActionTargetIdentity,
        provider_target_authority: &GenerationProviderTargetAuthority,
        credential_authority: Option<&ProviderCredentialAccessAuthority>,
        require_exact_credential_authority: bool,
    ) -> CoreResult<SameBranchGenerationAttempt> {
        self.ensure_interaction_state_available(conversation_id, branch_id)?;
        let operation = self.resolve_same_branch_generation_operation_identity(
            SameBranchGenerationAttemptIdentity {
                conversation_id,
                branch_id,
                expected_head,
                text,
                operation_context,
                target: operation_target,
                temperature,
                max_output_tokens,
                prompt_preset_id,
                variable_overrides,
            },
        )?;
        let existing_attempt = match self.existing_same_branch_generation_attempt(
            ExistingSameBranchAttemptRequest {
                conversation_id,
                branch_id,
                expected_head,
                operation_id: &operation.operation_id,
                base_request_fingerprint_sha256: &operation.base_request_fingerprint_sha256,
                provider_target_authority,
                resume_generation_attempt_id: operation.resume_generation_attempt_id.as_ref(),
            },
        )? {
            ExistingSameBranchAttempt::Missing => None,
            ExistingSameBranchAttempt::Prepared(existing) => Some(*existing),
            ExistingSameBranchAttempt::Resolved(result) => return Ok(result),
        };
        self.revalidate_prepared_same_branch_credential_authority(
            existing_attempt.as_ref(),
            provider_target_authority,
            credential_authority,
            require_exact_credential_authority,
        )?;
        let (module_runtime_review, applied_module_plan, module_plan_sha256) = self
            .resolve_same_branch_module_authority(
                existing_attempt.as_ref(),
                character,
                conversation_id,
                branch_id,
                mode,
                prompt_preset_id,
            )?;
        let attempt = if let Some(existing) = existing_attempt {
            require_generation_attempt_module_plan(&existing, &module_plan_sha256)?;
            existing
        } else {
            let prompt_selection_authority = self.capture_generation_prompt_selection_authority(
                GenerationPromptAuthorityCapture {
                    character,
                    conversation_id,
                    branch_id,
                    mode,
                    explicit_preset_id: prompt_preset_id,
                    generation_target,
                    temperature,
                    max_output_tokens,
                    prompt_wire_contract,
                    provider_target_authority: provider_target_authority.clone(),
                },
            )?;
            let input = lorepia_storage::GenerationAttemptInput {
                operation_id: operation.operation_id,
                conversation_id: conversation_id.clone(),
                source_branch_id: branch_id.clone(),
                proposed_branch_id: branch_id.clone(),
                expected_head_message_id: expected_head.cloned(),
                context_head_message_id: expected_head.cloned(),
                module_plan_sha256,
                base_request_fingerprint_sha256: operation.base_request_fingerprint_sha256,
                prompt_selection_authority: Some(prompt_selection_authority),
                module_runtime_review_authority: Some(module_runtime_review.clone()),
                applied_runtime_plan_authority: applied_module_plan.clone(),
            };
            if require_exact_credential_authority {
                self.inner
                    .storage
                    .prepare_generation_attempt_with_credential_authority(
                        &input,
                        Utc::now(),
                        credential_authority,
                    )?
            } else {
                self.inner
                    .storage
                    .prepare_generation_attempt(&input, Utc::now())?
            }
        };
        self.advance_same_branch_generation_attempt(
            attempt,
            conversation_id,
            branch_id,
            Some(&module_runtime_review),
            applied_module_plan,
        )
    }

    fn advance_same_branch_generation_attempt(
        &self,
        mut attempt: lorepia_storage::StoredGenerationAttempt,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        module_runtime_review: Option<&lorepia_orchestration::ModuleMergeReview>,
        applied_module_plan: Option<lorepia_orchestration::AppliedModuleRuntimePlan>,
    ) -> CoreResult<SameBranchGenerationAttempt> {
        if attempt.status == lorepia_storage::GenerationAttemptStatus::Prepared {
            if !self
                .inner
                .storage
                .list_interaction_proposals(
                    conversation_id,
                    branch_id,
                    lorepia_domain::InteractionProposalStatus::Pending,
                    1,
                )?
                .is_empty()
            {
                return Err(CoreError::new(
                    CoreErrorCode::PermissionDenied,
                    "generation is blocked by an existing interaction approval",
                    true,
                ));
            }
            let boundary = self
                .inner
                .storage
                .get_generation_attempt_interaction_boundary(&attempt.generation_id)?;
            let review = self.prepare_generation_attempt_before_review(
                &attempt,
                &boundary.state,
                &boundary.context_checkpoint_sha256,
                module_runtime_review.ok_or_else(|| {
                    CoreError::new(
                        CoreErrorCode::StorageCorrupted,
                        "prepared generation attempt is missing its module review",
                        false,
                    )
                })?,
                applied_module_plan.as_ref(),
                attempt.created_at,
            )?;
            self.inner
                .storage
                .commit_generation_attempt_before_review(&review)?;
            attempt = self
                .inner
                .storage
                .get_generation_attempt(&attempt.generation_id)?;
        }
        match attempt.status {
            lorepia_storage::GenerationAttemptStatus::BeforeGenerationApplied
            | lorepia_storage::GenerationAttemptStatus::DispatchReady => {
                let boundary = self
                    .inner
                    .storage
                    .get_generation_attempt_interaction_boundary(&attempt.generation_id)?;
                let aggregate = self
                    .inner
                    .storage
                    .get_generation_attempt_interaction_aggregate(&attempt.generation_id)?;
                if aggregate.pending_proposal_count != 0 {
                    return Err(CoreError::new(
                        CoreErrorCode::PermissionDenied,
                        "generation is waiting for an interaction approval",
                        true,
                    ));
                }
                Ok(SameBranchGenerationAttempt::Ready(Box::new(
                    PreparedSameBranchGenerationAttempt {
                        attempt,
                        interaction_state: lorepia_storage::StoredInteractionState {
                            key: boundary.state.key,
                            state: aggregate.state,
                            knowledge: aggregate.knowledge,
                        },
                        applied_module_plan,
                    },
                )))
            }
            lorepia_storage::GenerationAttemptStatus::Running
            | lorepia_storage::GenerationAttemptStatus::Completed => {
                Ok(SameBranchGenerationAttempt::Existing(attempt.generation_id))
            }
            lorepia_storage::GenerationAttemptStatus::Prepared => Err(CoreError::new(
                CoreErrorCode::StorageUnavailable,
                "generation attempt remained unreviewed",
                true,
            )),
            lorepia_storage::GenerationAttemptStatus::AwaitingApproval => Err(CoreError::new(
                CoreErrorCode::PermissionDenied,
                "generation is waiting for an interaction approval",
                true,
            )),
            lorepia_storage::GenerationAttemptStatus::FailedBeforeDispatch => Err(CoreError::new(
                CoreErrorCode::PermissionDenied,
                "generation attempt requires an explicit pre-dispatch retry",
                true,
            )),
        }
    }

    pub(in crate::app) fn seal_same_branch_generation_attempt(
        &self,
        attempt: lorepia_storage::StoredGenerationAttempt,
        prepared: &crate::orchestration::PreparedGenerationPlan,
        prompt_plan: &lorepia_storage::GenerationPromptPlanRecord,
    ) -> CoreResult<lorepia_storage::StoredGenerationAttempt> {
        if attempt.status == lorepia_storage::GenerationAttemptStatus::DispatchReady {
            return Ok(attempt);
        }
        if attempt.status != lorepia_storage::GenerationAttemptStatus::BeforeGenerationApplied {
            return Err(CoreError::invalid(
                "generation attempt is not ready for prompt sealing",
            ));
        }
        let applied_module_plan_sha256 = match prepared.module_plan_sha256.as_ref() {
            Some(value) => Sha256Digest::parse(value.clone()).map_err(CoreError::invalid)?,
            None => lorepia_orchestration::no_applied_module_runtime_plan_sha256(),
        };
        if applied_module_plan_sha256 != attempt.input.module_plan_sha256 {
            return Err(CoreError::invalid(
                "applied module plan changed after BeforeGeneration",
            ));
        }
        let interaction_aggregate = self
            .inner
            .storage
            .get_generation_attempt_interaction_aggregate(&attempt.generation_id)?;
        let before = attempt.before_generation_evidence.as_ref().ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "generation attempt is missing BeforeGeneration evidence",
                false,
            )
        })?;
        let before_generation_evidence_sha256 = attempt
            .before_generation_evidence_sha256
            .clone()
            .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "generation attempt is missing its BeforeGeneration evidence hash",
                false,
            )
        })?;
        let (final_interaction_state_revision, final_interaction_state_sha256) =
            attempt.approval_evidence.as_ref().map_or_else(
                || {
                    (
                        before.context_state_revision,
                        before.context_state_sha256.clone(),
                    )
                },
                |approval| {
                    (
                        approval.resulting_state_revision,
                        approval.resulting_state_sha256.clone(),
                    )
                },
            );
        self.inner.storage.seal_generation_attempt_dispatch_ready(
            &attempt.generation_id,
            attempt.revision,
            &lorepia_storage::GenerationDispatchSeal {
                final_prompt_plan_sha256: Sha256Digest::parse(prompt_plan.plan_sha256.clone())
                    .map_err(CoreError::invalid)?,
                final_prompt_input_fingerprint_sha256: Sha256Digest::parse(
                    prompt_plan.input_fingerprint_sha256.clone(),
                )
                .map_err(CoreError::invalid)?,
                final_interaction_state_revision,
                final_interaction_state_sha256,
                applied_module_plan_sha256,
                before_generation_evidence_sha256,
                approval_evidence_sha256: attempt.approval_evidence_sha256.clone(),
                derived_chain_sha256: Some(interaction_aggregate.derived_chain_sha256),
                derived_event_count: Some(interaction_aggregate.derived_event_count),
                derived_guard_count: Some(interaction_aggregate.derived_guard_count),
            },
            Utc::now(),
        )
    }
}

pub(in crate::app) fn generation_attempt_prompt_authority(
    attempt: &lorepia_storage::StoredGenerationAttempt,
) -> CoreResult<&lorepia_storage::GenerationPromptSelectionAuthority> {
    attempt
        .input
        .prompt_selection_authority
        .as_ref()
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "generation attempt is missing its sealed prompt authority",
                false,
            )
        })
}

fn require_generation_attempt_module_plan(
    attempt: &lorepia_storage::StoredGenerationAttempt,
    expected: &Sha256Digest,
) -> CoreResult<()> {
    if attempt.input.module_plan_sha256 != *expected {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "stored generation module plan differs from its immutable request",
            false,
        ));
    }
    Ok(())
}

pub(crate) fn generation_attempt_module_authority(
    attempt: &lorepia_storage::StoredGenerationAttempt,
) -> CoreResult<(
    &lorepia_orchestration::ModuleMergeReview,
    Option<&lorepia_orchestration::AppliedModuleRuntimePlan>,
)> {
    let review = attempt
        .input
        .module_runtime_review_authority
        .as_ref()
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "generation attempt is missing its sealed module review authority",
                false,
            )
        })?;
    let prompt_authority = generation_attempt_prompt_authority(attempt)?;
    review.verify().map_err(|_| {
        CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "generation attempt module review authority is invalid",
            false,
        )
    })?;
    if review.context.conversation_id.as_deref() != Some(attempt.input.conversation_id.0.as_str())
        || review.context.branch_id.as_deref() != Some(attempt.input.proposed_branch_id.0.as_str())
        || review.context.character_id.as_deref() != Some(prompt_authority.character.id.as_str())
        || review.context.persona_id.as_ref()
            != prompt_authority
                .persona_selection
                .as_ref()
                .map(|selection| &selection.value.persona_id)
        || prompt_local_user_id_sha256(&review.context.local_user_id)
            != prompt_authority.local_user_id_sha256
    {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "generation attempt module review authority differs from its target lineage",
            false,
        ));
    }
    if let Some(plan) = attempt.input.applied_runtime_plan_authority.as_ref() {
        plan.verify().map_err(|_| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "generation attempt applied module authority is invalid",
                false,
            )
        })?;
        if plan.review != *review
            || plan.applied_plan_sha256 != attempt.input.module_plan_sha256
            || review.ordered_bindings.is_empty()
        {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "generation attempt applied module authority differs from its review",
                false,
            ));
        }
        return Ok((review, Some(plan)));
    }
    if !review.ordered_bindings.is_empty()
        || attempt.input.module_plan_sha256
            != lorepia_orchestration::no_applied_module_runtime_plan_sha256()
    {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "generation attempt no-module authority differs from its review",
            false,
        ));
    }
    Ok((review, None))
}
