use chrono::{DateTime, Utc};
use lorepia_domain::{
    ConversationBranchId, ConversationId, ConversationMode, CoreError, CoreErrorCode, CoreResult,
    GenerationId, GenerationReasoningEffort, GenerationTarget, MessageActionGeneration, MessageId,
    Sha256Digest,
};
use lorepia_storage::{
    GenerationProviderTargetAuthority, MessageGenerationAction, MessageGenerationActionContext,
    ProviderCredentialAccessAuthority, deterministic_proposed_branch_id,
};
use sha2::{Digest, Sha256};

use super::{
    GenerationActionSemanticSnapshot, GenerationActionTargetIdentity, GenerationOperationContext,
    MessageGenerationActionIdentityInput, PromptRouteWireContract, ValidatedGenerationTarget,
    generation_action_name, generation_attempt_module_authority,
    generation_attempt_prompt_authority, generation_target_provider_authority,
    new_generation_operation_id, require_generation_provider_target_authority,
    validate_generation_target_plan_with_reasoning_effort,
};
use crate::{
    app::{Core, canonical_value_sha256, validate_user_message_text},
    orchestration::GenerationPromptAuthorityCapture,
};

pub(in crate::app) struct PreparedMessageGenerationAction {
    pub(super) conversation_id: ConversationId,
    pub(super) source_branch_id: ConversationBranchId,
    pub(super) expected_source_head_message_id: Option<MessageId>,
    pub(super) target_message_id: MessageId,
    pub(super) action: MessageGenerationAction,
    pub(super) context: MessageGenerationActionContext,
    pub(super) text: String,
    pub(super) target: GenerationActionTargetIdentity,
    semantic_base_fingerprint_sha256: Sha256Digest,
    pub(in crate::app) operation_id: String,
    resume_generation_attempt_id: Option<GenerationId>,
    pub(super) proposed_branch_id: ConversationBranchId,
    pub(super) mode: ConversationMode,
}

#[derive(Clone, Copy)]
pub(super) struct MessageGenerationAttemptConfiguration<'a> {
    pub(super) generation_target: Option<&'a GenerationTarget>,
    pub(super) temperature: Option<f64>,
    pub(super) max_output_tokens: Option<u32>,
    pub(super) prompt_wire_contract: Option<&'a PromptRouteWireContract>,
    pub(super) provider_target_authority: &'a GenerationProviderTargetAuthority,
    pub(super) credential_authority: Option<&'a ProviderCredentialAccessAuthority>,
    pub(super) require_exact_credential_authority: bool,
}

pub(super) struct PreparedMessageActionAttempt {
    pub(super) attempt: lorepia_storage::StoredGenerationAttempt,
    pub(super) interaction_state: lorepia_storage::StoredInteractionState,
    pub(super) target_interaction_state_key: lorepia_storage::InteractionStateKey,
    pub(super) applied_module_plan: Option<lorepia_orchestration::AppliedModuleRuntimePlan>,
}

pub(super) enum MessageActionAttempt {
    Existing(MessageActionGeneration),
    Ready(Box<PreparedMessageActionAttempt>),
}

impl Core {
    pub(in crate::app) fn prepare_message_generation_action_identity(
        &self,
        input: MessageGenerationActionIdentityInput<'_>,
    ) -> CoreResult<PreparedMessageGenerationAction> {
        let MessageGenerationActionIdentityInput {
            conversation_id,
            source_branch_id,
            expected_source_head_message_id,
            target_message_id,
            action,
            replacement_text,
            operation_context,
            target,
        } = input;
        let replacement_text = validate_action_replacement(action, replacement_text)?;
        let context = self
            .inner
            .storage
            .load_message_generation_action_identity_context(
                conversation_id,
                source_branch_id,
                target_message_id,
                action,
            )?;
        let text = replacement_text.map_or_else(
            || validate_user_message_text(&context.user_text).map(str::to_owned),
            |text| Ok(text.to_owned()),
        )?;
        let mode = self
            .inner
            .storage
            .get_conversation_state(conversation_id)?
            .selected_mode;
        let replacement_text_sha256 = format!("{:x}", Sha256::digest(text.as_bytes()));
        let semantic_base_fingerprint_sha256 = Sha256Digest::parse(canonical_value_sha256(
            &GenerationActionSemanticSnapshot {
                schema_version: 1,
                action: generation_action_name(action),
                conversation_id,
                source_branch_id,
                expected_source_head_message_id,
                target_message_id,
                context_head_message_id: context.fork_message_id.as_ref(),
                replacement_text_sha256: &replacement_text_sha256,
                target: &target,
            },
            "generation action semantic request",
        )?)
        .map_err(CoreError::invalid)?;
        let (operation_id, resume_generation_attempt_id) = match operation_context {
            GenerationOperationContext::New { operation_nonce } => (
                new_generation_operation_id(
                    "generation-action-v5",
                    &semantic_base_fingerprint_sha256,
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
                (
                    attempt.input.operation_id,
                    Some(generation_attempt_id.clone()),
                )
            }
        };
        let proposed_branch_id = deterministic_proposed_branch_id(
            &operation_id,
            conversation_id,
            source_branch_id,
            context.fork_message_id.as_ref(),
        )?;
        let mode = match self
            .inner
            .storage
            .get_generation_attempt_by_operation_id(conversation_id, &operation_id)
        {
            Ok(attempt) => generation_attempt_prompt_authority(&attempt)?.mode,
            Err(error) if error.code == CoreErrorCode::NotFound => mode,
            Err(error) => return Err(error),
        };
        let prepared = PreparedMessageGenerationAction {
            conversation_id: conversation_id.clone(),
            source_branch_id: source_branch_id.clone(),
            expected_source_head_message_id: expected_source_head_message_id.cloned(),
            target_message_id: target_message_id.clone(),
            action,
            context,
            text,
            target,
            semantic_base_fingerprint_sha256,
            operation_id,
            resume_generation_attempt_id,
            proposed_branch_id,
            mode,
        };
        self.validate_message_generation_action_identity(&prepared)?;
        Ok(prepared)
    }

    fn validate_message_generation_action_identity(
        &self,
        prepared: &PreparedMessageGenerationAction,
    ) -> CoreResult<()> {
        match self.inner.storage.get_generation_attempt_by_operation_id(
            &prepared.conversation_id,
            &prepared.operation_id,
        ) {
            Ok(attempt) => {
                let mismatched = prepared
                    .resume_generation_attempt_id
                    .as_ref()
                    .is_some_and(|generation_id| generation_id != &attempt.generation_id)
                    || attempt.input.conversation_id != prepared.conversation_id
                    || attempt.input.source_branch_id != prepared.source_branch_id
                    || attempt.input.proposed_branch_id != prepared.proposed_branch_id
                    || attempt.input.expected_head_message_id
                        != prepared.expected_source_head_message_id
                    || attempt.input.context_head_message_id != prepared.context.fork_message_id
                    || attempt.input.base_request_fingerprint_sha256
                        != prepared.semantic_base_fingerprint_sha256
                    || attempt.input.prompt_selection_authority.is_none();
                if mismatched {
                    return if prepared.resume_generation_attempt_id.is_some() {
                        Err(CoreError::new(
                            CoreErrorCode::InvalidInput,
                            "generation resume attempt does not match the caller-owned action; start a new generation operation",
                            true,
                        ))
                    } else {
                        Err(CoreError::new(
                            CoreErrorCode::StorageCorrupted,
                            "stored generation action attempt differs from its immutable request",
                            false,
                        ))
                    };
                }
            }
            Err(error) if error.code == CoreErrorCode::NotFound => {
                if prepared.resume_generation_attempt_id.is_some() {
                    return Err(CoreError::new(
                        CoreErrorCode::InvalidInput,
                        "generation resume attempt does not belong to this action; start a new generation operation",
                        true,
                    ));
                }
            }
            Err(error) => return Err(error),
        }
        // A completed append moves the active branch away from the immutable
        // source branch. Resolve an exact durable operation before rechecking
        // that live branch snapshot so a response-loss retry can replay after
        // restart without relaunching its provider.
        if self.existing_message_action_generation(prepared)?.is_some() {
            return Ok(());
        }
        let validated_context = match self.inner.storage.prepare_message_generation_action(
            &prepared.conversation_id,
            &prepared.source_branch_id,
            prepared.expected_source_head_message_id.as_ref(),
            &prepared.target_message_id,
            prepared.action,
        ) {
            Ok(context) => context,
            Err(error) => {
                // Close the narrow race where another caller atomically
                // materialized this exact operation after the first lookup.
                if self.existing_message_action_generation(prepared)?.is_some() {
                    return Ok(());
                }
                return Err(error);
            }
        };
        if validated_context != prepared.context {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "message action identity changed during live snapshot validation",
                false,
            ));
        }
        Ok(())
    }

    pub(super) fn preflight_message_action_provider_authority(
        &self,
        action: &PreparedMessageGenerationAction,
        provider_target_authority: &GenerationProviderTargetAuthority,
    ) -> CoreResult<()> {
        match self
            .inner
            .storage
            .get_generation_attempt_by_operation_id(&action.conversation_id, &action.operation_id)
        {
            Ok(attempt) => {
                require_generation_provider_target_authority(&attempt, provider_target_authority)
            }
            Err(error)
                if error.code == CoreErrorCode::NotFound
                    && action.resume_generation_attempt_id.is_none() =>
            {
                Ok(())
            }
            Err(error) if error.code == CoreErrorCode::NotFound => Err(CoreError::new(
                CoreErrorCode::InvalidInput,
                "generation resume attempt is unavailable; start a new generation operation",
                true,
            )),
            Err(error) => Err(error),
        }
    }

    pub(super) fn validate_message_action_generation_target(
        &self,
        action: &PreparedMessageGenerationAction,
        target: &GenerationTarget,
        requested_reasoning_effort: Option<GenerationReasoningEffort>,
    ) -> CoreResult<ValidatedGenerationTarget> {
        let validated = validate_generation_target_plan_with_reasoning_effort(
            self,
            target,
            requested_reasoning_effort,
        )?;
        let provider_target_authority = generation_target_provider_authority(target, &validated)?;
        self.preflight_message_action_provider_authority(action, &provider_target_authority)?;
        Ok(validated)
    }

    fn prepare_message_generation_attempt(
        &self,
        action: &PreparedMessageGenerationAction,
        configuration: MessageGenerationAttemptConfiguration<'_>,
        module_runtime_review: &lorepia_orchestration::ModuleMergeReview,
        applied_module_plan: Option<&lorepia_orchestration::AppliedModuleRuntimePlan>,
        prepared_at: DateTime<Utc>,
    ) -> CoreResult<lorepia_storage::StoredGenerationAttempt> {
        let applied_module_plan_sha256 = applied_module_plan.map_or_else(
            lorepia_orchestration::no_applied_module_runtime_plan_sha256,
            |plan| plan.applied_plan_sha256.clone(),
        );
        let base_request_fingerprint_sha256 = action.semantic_base_fingerprint_sha256.clone();
        match self
            .inner
            .storage
            .get_generation_attempt_by_operation_id(&action.conversation_id, &action.operation_id)
        {
            Ok(existing) => {
                require_generation_provider_target_authority(
                    &existing,
                    configuration.provider_target_authority,
                )?;
                if existing.input.source_branch_id != action.source_branch_id
                    || existing.input.proposed_branch_id != action.proposed_branch_id
                    || existing.input.expected_head_message_id
                        != action.expected_source_head_message_id
                    || existing.input.context_head_message_id != action.context.fork_message_id
                    || existing.input.module_plan_sha256 != applied_module_plan_sha256
                    || existing.input.base_request_fingerprint_sha256
                        != base_request_fingerprint_sha256
                    || existing.input.prompt_selection_authority.is_none()
                    || existing.input.module_runtime_review_authority.as_ref()
                        != Some(module_runtime_review)
                    || existing.input.applied_runtime_plan_authority.as_ref() != applied_module_plan
                {
                    return Err(CoreError::new(
                        CoreErrorCode::StorageCorrupted,
                        "stored generation action attempt differs from its immutable request",
                        false,
                    ));
                }
                if configuration.require_exact_credential_authority {
                    return self
                        .inner
                        .storage
                        .prepare_generation_attempt_with_credential_authority(
                            &existing.input,
                            existing.created_at,
                            configuration.credential_authority,
                        );
                }
                return Ok(existing);
            }
            Err(error) if error.code == CoreErrorCode::NotFound => {}
            Err(error) => return Err(error),
        }
        let conversation = self
            .inner
            .storage
            .get_conversation(&action.conversation_id)?;
        let character = self
            .inner
            .storage
            .get_character(&conversation.character_id)?;
        let prompt_selection_authority =
            self.capture_generation_prompt_selection_authority(GenerationPromptAuthorityCapture {
                character: &character,
                conversation_id: &action.conversation_id,
                branch_id: &action.source_branch_id,
                mode: action.mode,
                explicit_preset_id: None,
                generation_target: configuration.generation_target,
                temperature: configuration.temperature,
                max_output_tokens: configuration.max_output_tokens,
                prompt_wire_contract: configuration.prompt_wire_contract,
                provider_target_authority: configuration.provider_target_authority.clone(),
            })?;
        let input = lorepia_storage::GenerationAttemptInput {
            operation_id: action.operation_id.clone(),
            conversation_id: action.conversation_id.clone(),
            source_branch_id: action.source_branch_id.clone(),
            proposed_branch_id: action.proposed_branch_id.clone(),
            expected_head_message_id: action.expected_source_head_message_id.clone(),
            context_head_message_id: action.context.fork_message_id.clone(),
            module_plan_sha256: applied_module_plan_sha256,
            base_request_fingerprint_sha256,
            prompt_selection_authority: Some(prompt_selection_authority),
            module_runtime_review_authority: Some(module_runtime_review.clone()),
            applied_runtime_plan_authority: applied_module_plan.cloned(),
        };
        if configuration.require_exact_credential_authority {
            self.inner
                .storage
                .prepare_generation_attempt_with_credential_authority(
                    &input,
                    prepared_at,
                    configuration.credential_authority,
                )
        } else {
            self.inner
                .storage
                .prepare_generation_attempt(&input, prepared_at)
        }
    }

    pub(super) fn existing_message_action_generation(
        &self,
        action: &PreparedMessageGenerationAction,
    ) -> CoreResult<Option<MessageActionGeneration>> {
        let attempt = match self
            .inner
            .storage
            .get_generation_attempt_by_operation_id(&action.conversation_id, &action.operation_id)
        {
            Ok(attempt) => attempt,
            Err(error) if error.code == CoreErrorCode::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if !matches!(
            attempt.status,
            lorepia_storage::GenerationAttemptStatus::Running
                | lorepia_storage::GenerationAttemptStatus::Completed
        ) {
            return Ok(None);
        }
        if attempt.input.source_branch_id != action.source_branch_id
            || attempt.input.proposed_branch_id != action.proposed_branch_id
            || attempt.input.context_head_message_id != action.context.fork_message_id
        {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "stored generation action identity differs from its canonical operation",
                false,
            ));
        }
        let branch = self
            .inner
            .storage
            .get_conversation_branch(&attempt.input.proposed_branch_id)?;
        if branch.conversation_id != action.conversation_id {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "stored generation action branch belongs to another conversation",
                false,
            ));
        }
        Ok(Some(MessageActionGeneration {
            branch,
            generation_id: attempt.generation_id,
        }))
    }

    pub(super) fn prepare_message_action_attempt(
        &self,
        action: &PreparedMessageGenerationAction,
        configuration: MessageGenerationAttemptConfiguration<'_>,
    ) -> CoreResult<MessageActionAttempt> {
        self.ensure_interaction_state_available(&action.conversation_id, &action.source_branch_id)?;
        let existing_attempt = match self
            .inner
            .storage
            .get_generation_attempt_by_operation_id(&action.conversation_id, &action.operation_id)
        {
            Ok(existing) => Some(existing),
            Err(error) if error.code == CoreErrorCode::NotFound => None,
            Err(error) => return Err(error),
        };
        let (module_runtime_review, mut applied_module_plan) =
            if let Some(existing) = existing_attempt.as_ref() {
                let (review, plan) = generation_attempt_module_authority(existing)?;
                (review.clone(), plan.cloned())
            } else {
                self.preview_module_runtime_authority_for_proposed_branch(
                    &action.conversation_id,
                    &action.proposed_branch_id,
                )?
            };
        let mut attempt = self.prepare_message_generation_attempt(
            action,
            configuration,
            &module_runtime_review,
            applied_module_plan.as_ref(),
            Utc::now(),
        )?;

        if attempt.status != lorepia_storage::GenerationAttemptStatus::Prepared {
            let before = self
                .inner
                .storage
                .get_generation_attempt_before_review(&attempt.generation_id)?
                .ok_or_else(|| {
                    CoreError::new(
                        CoreErrorCode::StorageCorrupted,
                        "generation action attempt is missing its immutable review",
                        false,
                    )
                })?;
            applied_module_plan = before.applied_runtime_plan;
        }

        if attempt.status == lorepia_storage::GenerationAttemptStatus::Prepared {
            let boundary = self
                .inner
                .storage
                .get_generation_attempt_interaction_boundary(&attempt.generation_id)?;
            let review = self.prepare_generation_attempt_before_review(
                &attempt,
                &boundary.state,
                &boundary.context_checkpoint_sha256,
                &module_runtime_review,
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

        self.finish_prepared_message_action_attempt(action, attempt, applied_module_plan)
    }

    fn finish_prepared_message_action_attempt(
        &self,
        action: &PreparedMessageGenerationAction,
        attempt: lorepia_storage::StoredGenerationAttempt,
        applied_module_plan: Option<lorepia_orchestration::AppliedModuleRuntimePlan>,
    ) -> CoreResult<MessageActionAttempt> {
        match attempt.status {
            lorepia_storage::GenerationAttemptStatus::BeforeGenerationApplied
            | lorepia_storage::GenerationAttemptStatus::DispatchReady => {}
            lorepia_storage::GenerationAttemptStatus::AwaitingApproval => {
                return Err(CoreError::new(
                    CoreErrorCode::PermissionDenied,
                    "generation is waiting for an interaction approval",
                    true,
                ));
            }
            lorepia_storage::GenerationAttemptStatus::FailedBeforeDispatch => {
                return Err(CoreError::new(
                    CoreErrorCode::PermissionDenied,
                    "generation attempt requires an explicit pre-dispatch retry",
                    true,
                ));
            }
            lorepia_storage::GenerationAttemptStatus::Prepared => {
                return Err(CoreError::new(
                    CoreErrorCode::StorageUnavailable,
                    "generation attempt remained unreviewed",
                    true,
                ));
            }
            lorepia_storage::GenerationAttemptStatus::Running
            | lorepia_storage::GenerationAttemptStatus::Completed => {
                return self
                    .existing_message_action_generation(action)?
                    .map(MessageActionAttempt::Existing)
                    .ok_or_else(|| {
                        CoreError::new(
                            CoreErrorCode::StorageCorrupted,
                            "generation action attempt is terminal without its durable branch",
                            false,
                        )
                    });
            }
        }

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
        let interaction_state = lorepia_storage::StoredInteractionState {
            key: boundary.state.key,
            state: aggregate.state,
            knowledge: aggregate.knowledge,
        };
        Ok(MessageActionAttempt::Ready(Box::new(
            PreparedMessageActionAttempt {
                attempt,
                interaction_state,
                target_interaction_state_key: crate::orchestration_runtime::interaction_state_key(
                    &action.conversation_id,
                    &action.proposed_branch_id,
                )?,
                applied_module_plan,
            },
        )))
    }

    pub(super) fn prompt_reasoning_effort_for_message_action(
        &self,
        action: &PreparedMessageGenerationAction,
    ) -> CoreResult<Option<GenerationReasoningEffort>> {
        match self
            .inner
            .storage
            .get_generation_attempt_by_operation_id(&action.conversation_id, &action.operation_id)
        {
            Ok(attempt) => {
                return Ok(generation_attempt_prompt_authority(&attempt)?
                    .quick_settings
                    .reasoning_effort);
            }
            Err(error) if error.code == CoreErrorCode::NotFound => {}
            Err(error) => return Err(error),
        }
        let state = self
            .inner
            .storage
            .get_conversation_state(&action.conversation_id)?;
        // Edit/regenerate creates a new branch. Resolve against an unbound
        // branch identity so the same conversation/character/user/app scope
        // precedence used by the eventual new branch determines the provider
        // overlay without inheriting a source-branch-only binding.
        self.prompt_reasoning_effort_for_context(
            &action.conversation_id,
            &action.proposed_branch_id,
            state.selected_mode,
            None,
        )
    }
}

fn validate_action_replacement(
    action: MessageGenerationAction,
    replacement_text: Option<&str>,
) -> CoreResult<Option<&str>> {
    match replacement_text {
        Some(text) => validate_user_message_text(text).map(Some),
        None if action == MessageGenerationAction::EditUser => {
            Err(CoreError::invalid("message text cannot be empty"))
        }
        None => Ok(None),
    }
}
