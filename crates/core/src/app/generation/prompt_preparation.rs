use lorepia_domain::{
    ConversationMode, CoreError, CoreErrorCode, CoreResult, GenerationId, Message, Sha256Digest,
};
use lorepia_storage::ProviderCredentialAccessAuthority;
use sha2::{Digest, Sha256};
use tokio::sync::watch;

use super::credential::{
    ConnectionBoundCredential, GenerationCredential, validate_connection_credential_binding,
};
use super::protocol_request::{
    configure_generation_protocol_request, reject_sensitive_provider_preview_fields,
};
use super::target_resolution::{
    ResolvedGenerationTarget, build_resolved_generation_target,
    validate_generation_target_for_attempt,
};
use super::{
    GenerationActionTargetIdentity, GenerationOperationContext, SameBranchGenerationAttemptIdentity,
};
use crate::app::{
    Core, MAX_TASK_PROMPT_BYTES, MAX_TASK_PROMPT_CHARS, PreparedSameBranchGenerationAttempt,
    SameBranchGenerationAttempt, SameBranchGenerationTargetInput,
    generation_attempt_prompt_authority, validate_user_message_text,
};
use crate::orchestration::deterministic_prompt_user_message_id;

/// One provider-neutral message supplied by an imported character runtime.
///
/// Runtime scripts can ask the native host for a secondary generation, but
/// they cannot supply provider request JSON, credentials, URLs, or headers.
/// Core rebuilds the request through the same provider adapters used by the
/// Bounded, Core-owned input for an auxiliary provider task.
///
/// The task runner constructs this value from a trusted instruction and
/// already-inspected source text. It is never exposed as an arbitrary provider
/// body or native DTO.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BoundedTaskPrompt {
    pub(in crate::app) system_instruction: String,
    pub(in crate::app) input: String,
}

impl BoundedTaskPrompt {
    pub(crate) fn new(
        system_instruction: impl Into<String>,
        input: impl Into<String>,
    ) -> CoreResult<Self> {
        let prompt = Self {
            system_instruction: system_instruction.into(),
            input: input.into(),
        };
        let total_bytes = prompt
            .system_instruction
            .len()
            .checked_add(prompt.input.len())
            .ok_or_else(|| CoreError::invalid("auxiliary task prompt size overflowed"))?;
        let total_chars = prompt
            .system_instruction
            .chars()
            .count()
            .checked_add(prompt.input.chars().count())
            .ok_or_else(|| CoreError::invalid("auxiliary task prompt size overflowed"))?;
        if prompt.system_instruction.trim().is_empty()
            || prompt.input.trim().is_empty()
            || prompt.system_instruction.contains('\0')
            || prompt.input.contains('\0')
            || total_bytes > MAX_TASK_PROMPT_BYTES
            || total_chars > MAX_TASK_PROMPT_CHARS
        {
            return Err(CoreError::invalid(
                "auxiliary task prompt is empty, unsafe, or exceeds its size limit",
            ));
        }
        Ok(prompt)
    }
}

pub(in crate::app) struct ReviewedPromptSendContext {
    pub(in crate::app) mode: ConversationMode,
    pub(in crate::app) resolved: ResolvedGenerationTarget,
    pub(in crate::app) credential: GenerationCredential,
    pub(in crate::app) credential_authority: Option<ProviderCredentialAccessAuthority>,
    pub(in crate::app) user_message: Message,
    pub(in crate::app) attempt: PreparedSameBranchGenerationAttempt,
}

enum ReviewedPromptSendPreparation {
    Existing(GenerationId),
    Ready(Box<ReviewedPromptSendContext>),
}

impl Core {
    /// Resolves the explicit expert preview through the same provider and
    /// prompt preparation path used by a reviewed send.
    ///
    /// This is the only Core read surface that returns prompt bodies to a Rust
    /// caller. Shell and Tauri must replace it with a content-free allowlist
    /// projection before any `WebView` serialization. The provider snapshot is
    /// credential-free by contract and is rejected if it contains
    /// endpoint/header/credential/opaque-state fields or if the complete
    /// preview exceeds 2 MiB. Preparation may persist an isolated
    /// generation-attempt review and approval records so preview and send can
    /// share one temporal snapshot; it never applies those records to live
    /// branch interaction state, effects, messages, or generations.
    pub fn resolve_prompt_preview(
        &self,
        plan_request: &crate::PromptPlanRequest,
        operation_context: GenerationOperationContext<'_>,
    ) -> CoreResult<crate::ExpertPromptPreview> {
        let state = self
            .inner
            .storage
            .get_conversation_state(&plan_request.conversation_id)?;
        let prepared_target =
            self.prepare_same_branch_generation_target(SameBranchGenerationTargetInput {
                conversation_id: &plan_request.conversation_id,
                branch_id: &plan_request.branch_id,
                expected_head: plan_request.expected_head.as_ref(),
                live_mode: state.selected_mode,
                text: &plan_request.user_text,
                operation_context,
                target: &plan_request.generation_target,
                prompt_preset_id: plan_request.prompt_preset_id.as_ref(),
                variable_overrides: &plan_request.variable_overrides,
            })?;
        let initial_resolved = build_resolved_generation_target(prepared_target.validated)?;
        let attempt = match self.prepare_reviewed_prompt_generation_attempt(
            plan_request,
            operation_context,
            prepared_target.mode,
            &initial_resolved,
        )? {
            SameBranchGenerationAttempt::Ready(attempt) => *attempt,
            SameBranchGenerationAttempt::Existing(_) => {
                return Err(CoreError::invalid(
                    "reviewed generation attempt has already been dispatched",
                ));
            }
        };
        let validated = validate_generation_target_for_attempt(
            self,
            &plan_request.generation_target,
            &attempt.attempt,
        )?;
        let applied_parameters = validated
            .request_plan
            .body_patches()
            .iter()
            .map(|patch| {
                (
                    patch.path().to_owned(),
                    crate::PromptAppliedParameterPreview {
                        field: patch.path().to_owned(),
                        value: patch.value().clone(),
                    },
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        let resolved = build_resolved_generation_target(validated)?;
        let prepared = self.prepare_prompt_plan_request_with_wire_contract(
            plan_request,
            crate::orchestration::PromptPlanPreparation {
                prompt_wire_contract: Some(&resolved.prompt_wire_contract),
                interaction_state_override: Some(&attempt.interaction_state),
                applied_module_plan_override: attempt.applied_module_plan.as_ref(),
                prompt_selection_authority: attempt
                    .attempt
                    .input
                    .prompt_selection_authority
                    .as_ref(),
                generation_attempt_id: Some(&attempt.attempt.generation_id),
                resolution_time: attempt.attempt.created_at,
                session_seed: reviewed_prompt_session_seed(
                    &attempt.attempt.input.base_request_fingerprint_sha256,
                ),
            },
        )?;
        self.finish_expert_prompt_preview(
            attempt.attempt.generation_id,
            plan_request,
            resolved,
            prepared,
            applied_parameters,
        )
    }

    /// Prepares the redacted preview/explanation path from the same isolated
    /// attempt and exact provider wire contract used by expert preview and
    /// reviewed send.
    pub(crate) fn prepare_reviewed_prompt_plan_for_core(
        &self,
        plan_request: &crate::PromptPlanRequest,
        operation_context: GenerationOperationContext<'_>,
    ) -> CoreResult<crate::orchestration::PreparedGenerationPlan> {
        let state = self
            .inner
            .storage
            .get_conversation_state(&plan_request.conversation_id)?;
        let prepared_target =
            self.prepare_same_branch_generation_target(SameBranchGenerationTargetInput {
                conversation_id: &plan_request.conversation_id,
                branch_id: &plan_request.branch_id,
                expected_head: plan_request.expected_head.as_ref(),
                live_mode: state.selected_mode,
                text: &plan_request.user_text,
                operation_context,
                target: &plan_request.generation_target,
                prompt_preset_id: plan_request.prompt_preset_id.as_ref(),
                variable_overrides: &plan_request.variable_overrides,
            })?;
        let initial_resolved = build_resolved_generation_target(prepared_target.validated)?;
        let attempt = match self.prepare_reviewed_prompt_generation_attempt(
            plan_request,
            operation_context,
            prepared_target.mode,
            &initial_resolved,
        )? {
            SameBranchGenerationAttempt::Ready(attempt) => *attempt,
            SameBranchGenerationAttempt::Existing(_) => {
                return Err(CoreError::invalid(
                    "reviewed generation attempt has already been dispatched",
                ));
            }
        };
        let resolved = build_resolved_generation_target(validate_generation_target_for_attempt(
            self,
            &plan_request.generation_target,
            &attempt.attempt,
        )?)?;
        self.prepare_prompt_plan_request_with_wire_contract(
            plan_request,
            crate::orchestration::PromptPlanPreparation {
                prompt_wire_contract: Some(&resolved.prompt_wire_contract),
                interaction_state_override: Some(&attempt.interaction_state),
                applied_module_plan_override: attempt.applied_module_plan.as_ref(),
                prompt_selection_authority: attempt
                    .attempt
                    .input
                    .prompt_selection_authority
                    .as_ref(),
                generation_attempt_id: Some(&attempt.attempt.generation_id),
                resolution_time: attempt.attempt.created_at,
                session_seed: reviewed_prompt_session_seed(
                    &attempt.attempt.input.base_request_fingerprint_sha256,
                ),
            },
        )
    }

    /// Async expert preview path used by native hosts.
    ///
    /// Provider-backed memory retrieval is admitted only through the durable
    /// query-embedding state machine owned by `prepare_generation_plan_async`.
    /// The selected generation credential is neither required nor reused for
    /// that auxiliary task; the broker resolves the exact task connection.
    pub async fn resolve_prompt_preview_async(
        &self,
        plan_request: &crate::PromptPlanRequest,
        operation_context: GenerationOperationContext<'_>,
        task_credential_broker: &dyn crate::TaskCredentialBroker,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<crate::ExpertPromptPreview> {
        let state = self
            .inner
            .storage
            .get_conversation_state(&plan_request.conversation_id)?;
        let prepared_target =
            self.prepare_same_branch_generation_target(SameBranchGenerationTargetInput {
                conversation_id: &plan_request.conversation_id,
                branch_id: &plan_request.branch_id,
                expected_head: plan_request.expected_head.as_ref(),
                live_mode: state.selected_mode,
                text: &plan_request.user_text,
                operation_context,
                target: &plan_request.generation_target,
                prompt_preset_id: plan_request.prompt_preset_id.as_ref(),
                variable_overrides: &plan_request.variable_overrides,
            })?;
        let initial_resolved = build_resolved_generation_target(prepared_target.validated)?;
        let attempt = match self.prepare_reviewed_prompt_generation_attempt(
            plan_request,
            operation_context,
            prepared_target.mode,
            &initial_resolved,
        )? {
            SameBranchGenerationAttempt::Ready(attempt) => *attempt,
            SameBranchGenerationAttempt::Existing(_) => {
                return Err(CoreError::invalid(
                    "reviewed generation attempt has already been dispatched",
                ));
            }
        };
        let validated = validate_generation_target_for_attempt(
            self,
            &plan_request.generation_target,
            &attempt.attempt,
        )?;
        let applied_parameters = validated
            .request_plan
            .body_patches()
            .iter()
            .map(|patch| {
                (
                    patch.path().to_owned(),
                    crate::PromptAppliedParameterPreview {
                        field: patch.path().to_owned(),
                        value: patch.value().clone(),
                    },
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        let resolved = build_resolved_generation_target(validated)?;
        let prepared = self
            .prepare_prompt_plan_request_with_wire_contract_async(
                plan_request,
                crate::orchestration::AsyncPromptPlanPreparation {
                    prompt_wire_contract: Some(&resolved.prompt_wire_contract),
                    interaction_state_override: Some(&attempt.interaction_state),
                    applied_module_plan_override: attempt.applied_module_plan.as_ref(),
                    prompt_selection_authority: attempt
                        .attempt
                        .input
                        .prompt_selection_authority
                        .as_ref(),
                    generation_attempt_id: Some(&attempt.attempt.generation_id),
                    resolution_time: attempt.attempt.created_at,
                    session_seed: reviewed_prompt_session_seed(
                        &attempt.attempt.input.base_request_fingerprint_sha256,
                    ),
                    credential_broker: task_credential_broker,
                    cancelled,
                },
            )
            .await?;
        self.finish_expert_prompt_preview(
            attempt.attempt.generation_id,
            plan_request,
            resolved,
            prepared,
            applied_parameters,
        )
    }

    fn finish_expert_prompt_preview(
        &self,
        generation_attempt_id: GenerationId,
        plan_request: &crate::PromptPlanRequest,
        resolved: ResolvedGenerationTarget,
        prepared: crate::orchestration::PreparedGenerationPlan,
        mut applied_parameters: std::collections::BTreeMap<
            String,
            crate::PromptAppliedParameterPreview,
        >,
    ) -> CoreResult<crate::ExpertPromptPreview> {
        const MAX_EXPERT_PREVIEW_BYTES: usize = 2 * 1024 * 1024;

        let mut request = prepared.materialized.request.clone();
        // Opaque continuity is intentionally excluded from this expert
        // snapshot. It is never plaintext preview material.
        configure_generation_protocol_request(
            &self.inner.storage,
            &mut request,
            Some(&plan_request.generation_target),
            Some(resolved.api_family),
            false,
        )?;
        if let Some(temperature) = request.temperature {
            applied_parameters.insert(
                "temperature".to_owned(),
                crate::PromptAppliedParameterPreview {
                    field: "temperature".to_owned(),
                    value: serde_json::Value::from(temperature),
                },
            );
        }
        if let Some(tokens) = request.max_output_tokens {
            applied_parameters.insert(
                "max_output_tokens".to_owned(),
                crate::PromptAppliedParameterPreview {
                    field: "max_output_tokens".to_owned(),
                    value: serde_json::Value::from(tokens),
                },
            );
        }
        let provider_request = resolved.provider.snapshot_request(&request)?;
        reject_sensitive_provider_preview_fields(&provider_request)?;
        let resolved_plan = request.resolved_prompt_plan.as_ref().ok_or_else(|| {
            CoreError::internal("expert preview is missing its resolved prompt plan")
        })?;
        let effective_messages = resolved_plan
            .effective_messages
            .iter()
            .map(|message| crate::PromptEffectiveMessageContentPreview {
                sequence: message.sequence,
                block_id: message.block_id.clone(),
                block_kind: message.block_kind,
                requested_role: message.requested_role,
                effective_role: message.effective_role,
                estimated_tokens: message.estimated_tokens,
                source_message_ids: message.source_message_ids.clone(),
                content: message.content.clone(),
            })
            .collect::<Vec<_>>();
        let prompt_diff = resolved_plan
            .effective_messages
            .iter()
            .filter_map(|message| {
                let provider_message = prepared
                    .preview
                    .provider_messages
                    .iter()
                    .find(|candidate| candidate.sequence == message.sequence)?;
                let mut changes = vec![format!(
                    "requested role {:?} resolved to {:?}",
                    message.requested_role, message.effective_role
                )];
                changes.push(format!(
                    "effective role {:?} maps to provider role {:?} at {:?}",
                    message.effective_role, provider_message.wire_role, provider_message.placement
                ));
                Some(crate::PromptDiffEntry {
                    sequence: message.sequence,
                    block_id: message.block_id.clone(),
                    changes,
                })
            })
            .collect();
        let expert = crate::ExpertPromptPreview {
            generation_attempt_id,
            plan: prepared.preview,
            effective_messages,
            provider_request,
            applied_parameters: applied_parameters.into_values().collect(),
            prompt_diff,
        };
        let encoded = serde_json::to_vec(&expert).map_err(|error| {
            CoreError::internal(format!("cannot encode expert prompt preview: {error}"))
        })?;
        if encoded.len() > MAX_EXPERT_PREVIEW_BYTES {
            return Err(CoreError::new(
                CoreErrorCode::UnsupportedContent,
                "expert prompt preview exceeds the 2 MiB response limit",
                false,
            ));
        }
        Ok(expert)
    }

    /// Sends exactly the prompt plan previously reviewed by
    /// [`Core::resolve_prompt_preview`]. The active branch head and the
    /// resolver-owned plan hash are both checked again before any message or
    /// generation row is committed.
    pub fn send_message_with_prompt_plan(
        &self,
        plan_request: &crate::PromptPlanRequest,
        expected_generation_attempt_id: &GenerationId,
        credential: ConnectionBoundCredential,
    ) -> CoreResult<GenerationId> {
        let context = match self.prepare_reviewed_prompt_send(
            plan_request,
            expected_generation_attempt_id,
            credential,
        )? {
            ReviewedPromptSendPreparation::Existing(generation_id) => return Ok(generation_id),
            ReviewedPromptSendPreparation::Ready(context) => *context,
        };
        let mut prepared = self.prepare_prompt_plan_request_with_wire_contract(
            plan_request,
            crate::orchestration::PromptPlanPreparation {
                prompt_wire_contract: Some(&context.resolved.prompt_wire_contract),
                interaction_state_override: Some(&context.attempt.interaction_state),
                applied_module_plan_override: context.attempt.applied_module_plan.as_ref(),
                prompt_selection_authority: context
                    .attempt
                    .attempt
                    .input
                    .prompt_selection_authority
                    .as_ref(),
                generation_attempt_id: Some(&context.attempt.attempt.generation_id),
                resolution_time: context.attempt.attempt.created_at,
                session_seed: reviewed_prompt_session_seed(
                    &context
                        .attempt
                        .attempt
                        .input
                        .base_request_fingerprint_sha256,
                ),
            },
        )?;
        prepared.materialized.request.generation_id = context.attempt.attempt.generation_id.clone();
        self.launch_reviewed_prompt_send(plan_request, context, prepared)
    }

    /// Async reviewed-send path used when prompt resolution may require a
    /// provider-backed semantic query. The exact reviewed hash, generation
    /// attempt, and append contract are identical to the synchronous path.
    pub async fn send_message_with_prompt_plan_async(
        &self,
        plan_request: &crate::PromptPlanRequest,
        expected_generation_attempt_id: &GenerationId,
        credential: ConnectionBoundCredential,
        task_credential_broker: &dyn crate::TaskCredentialBroker,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<GenerationId> {
        let context = match self.prepare_reviewed_prompt_send(
            plan_request,
            expected_generation_attempt_id,
            credential,
        )? {
            ReviewedPromptSendPreparation::Existing(generation_id) => return Ok(generation_id),
            ReviewedPromptSendPreparation::Ready(context) => *context,
        };
        let mut prepared = self
            .prepare_prompt_plan_request_with_wire_contract_async(
                plan_request,
                crate::orchestration::AsyncPromptPlanPreparation {
                    prompt_wire_contract: Some(&context.resolved.prompt_wire_contract),
                    interaction_state_override: Some(&context.attempt.interaction_state),
                    applied_module_plan_override: context.attempt.applied_module_plan.as_ref(),
                    prompt_selection_authority: context
                        .attempt
                        .attempt
                        .input
                        .prompt_selection_authority
                        .as_ref(),
                    generation_attempt_id: Some(&context.attempt.attempt.generation_id),
                    resolution_time: context.attempt.attempt.created_at,
                    session_seed: reviewed_prompt_session_seed(
                        &context
                            .attempt
                            .attempt
                            .input
                            .base_request_fingerprint_sha256,
                    ),
                    credential_broker: task_credential_broker,
                    cancelled,
                },
            )
            .await?;
        prepared.materialized.request.generation_id = context.attempt.attempt.generation_id.clone();
        self.launch_reviewed_prompt_send(plan_request, context, prepared)
    }

    fn prepare_reviewed_prompt_send(
        &self,
        plan_request: &crate::PromptPlanRequest,
        expected_generation_attempt_id: &GenerationId,
        credential: ConnectionBoundCredential,
    ) -> CoreResult<ReviewedPromptSendPreparation> {
        let expected_plan_hash = plan_request
            .expected_plan_hash
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                CoreError::invalid("sending a reviewed prompt requires expected_plan_hash")
            })?;
        let sealed_attempt = match self
            .inner
            .storage
            .get_generation_attempt(expected_generation_attempt_id)
        {
            Ok(attempt) => attempt,
            Err(error) if error.code == CoreErrorCode::NotFound => {
                return Err(CoreError::new(
                    CoreErrorCode::InvalidInput,
                    "generation resume attempt is unavailable; start a new generation operation",
                    true,
                ));
            }
            Err(error) => return Err(error),
        };
        let text = validate_user_message_text(&plan_request.user_text)?;
        let operation_target = GenerationActionTargetIdentity::GenerationTarget {
            model_route_id: plan_request.generation_target.model_route_id.clone(),
            generation_preset_id: plan_request.generation_target.generation_preset_id.clone(),
        };
        self.resolve_same_branch_generation_operation_identity(
            SameBranchGenerationAttemptIdentity {
                conversation_id: &plan_request.conversation_id,
                branch_id: &plan_request.branch_id,
                expected_head: plan_request.expected_head.as_ref(),
                text,
                operation_context: GenerationOperationContext::Resume {
                    generation_attempt_id: expected_generation_attempt_id,
                },
                target: &operation_target,
                temperature: None,
                max_output_tokens: None,
                prompt_preset_id: plan_request.prompt_preset_id.as_ref(),
                variable_overrides: &plan_request.variable_overrides,
            },
        )?;
        let sealed_prompt_authority = generation_attempt_prompt_authority(&sealed_attempt)?;
        let mode = sealed_prompt_authority.mode;
        let validated = validate_generation_target_for_attempt(
            self,
            &plan_request.generation_target,
            &sealed_attempt,
        )?;
        validate_connection_credential_binding(&validated.connection, &credential)?;
        let resolved = build_resolved_generation_target(validated)?;
        let credential_authority = credential.access_authority().cloned();
        let credential: GenerationCredential = credential.into();
        let mut user_message = Message::user_after(
            plan_request.conversation_id.clone(),
            plan_request.expected_head.clone(),
            text,
        );
        user_message.id = deterministic_prompt_user_message_id(
            &plan_request.conversation_id,
            &plan_request.branch_id,
            plan_request.expected_head.as_ref(),
            text,
        );
        let attempt = match self.prepare_reviewed_prompt_generation_attempt(
            plan_request,
            GenerationOperationContext::Resume {
                generation_attempt_id: expected_generation_attempt_id,
            },
            mode,
            &resolved,
        )? {
            SameBranchGenerationAttempt::Existing(generation_id) => {
                let existing = self.validate_existing_reviewed_generation(
                    generation_id,
                    expected_generation_attempt_id,
                    expected_plan_hash,
                )?;
                return Ok(ReviewedPromptSendPreparation::Existing(existing));
            }
            SameBranchGenerationAttempt::Ready(attempt) => *attempt,
        };
        validate_reviewed_generation_attempt_id(
            expected_generation_attempt_id,
            &attempt.attempt.generation_id,
        )?;
        Ok(ReviewedPromptSendPreparation::Ready(Box::new(
            ReviewedPromptSendContext {
                mode,
                resolved,
                credential,
                credential_authority,
                user_message,
                attempt,
            },
        )))
    }
}

pub(in crate::app) fn reviewed_prompt_session_seed(
    base_request_fingerprint_sha256: &Sha256Digest,
) -> u64 {
    const SQLITE_SIGNED_INTEGER_MAX: u64 = 0x7fff_ffff_ffff_ffff;
    let digest = Sha256::digest(
        format!(
            "reviewed-prompt-session-seed-v2:{}",
            base_request_fingerprint_sha256.as_str()
        )
        .as_bytes(),
    );
    let raw_seed = u64::from_be_bytes(
        digest[..8]
            .try_into()
            .expect("SHA-256 always contains eight seed bytes"),
    );
    raw_seed & SQLITE_SIGNED_INTEGER_MAX
}

pub(in crate::app) fn validate_reviewed_generation_attempt_id(
    expected: &GenerationId,
    actual: &GenerationId,
) -> CoreResult<()> {
    if expected != actual {
        return Err(CoreError::invalid(
            "reviewed generation attempt changed; resolve a new preview before sending",
        ));
    }
    Ok(())
}
