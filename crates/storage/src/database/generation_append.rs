use super::{
    BoundedJson, ConversationBranchId, CoreError, CoreErrorCode, CoreResult, DateTime,
    GenerationPresetId, GenerationRecord, GenerationStatus, InteractionStateKey, Message,
    MessageId, MessageRole, MessageStatus, ModelRouteId, OptionalExtension, Sha256Digest, Storage,
    TransactionBehavior, Utc, api_family_to_str, ensure_generation_provider_credential_settled,
    generation_status_to_str, insert_message,
    materialize_generation_attempt_interaction_for_append, mode_to_str, params,
    serialize_opaque_reasoning_state, stale_branch_error, storage_db_error, u64_to_i64,
};

pub(super) type PromptPlanObservation<'a> = (
    &'a crate::orchestration::GenerationPromptPlanRecord,
    &'a [crate::orchestration::KnowledgeActivationLog],
);

struct GenerationAppendObservation<'a> {
    branch_id: &'a ConversationBranchId,
    expected_head: Option<&'a MessageId>,
    user: &'a Message,
    assistant: &'a Message,
    generation: &'a GenerationRecord,
    prompt_plan: Option<PromptPlanObservation<'a>>,
    require_attempt: bool,
}

struct PreparedGenerationAppendAttempt {
    attempt: crate::generation_attempt::StoredGenerationAttempt,
    target_key: InteractionStateKey,
}

impl Storage {
    pub fn append_generation(
        &self,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        user: &Message,
        assistant: &Message,
        generation: &GenerationRecord,
    ) -> CoreResult<()> {
        self.append_generation_observed(
            branch_id,
            expected_head,
            user,
            assistant,
            generation,
            None,
            false,
            None,
            false,
        )
    }

    /// Atomically appends the user/assistant messages, a sealed prompt plan,
    /// its credential-free provider request evidence, and the linked
    /// generation. Any validation, head-CAS, or persistence failure rolls the
    /// complete `SQLite` mutation back.
    #[allow(clippy::too_many_arguments)]
    pub fn append_generation_with_prompt_plan(
        &self,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        user: &Message,
        assistant: &Message,
        generation: &GenerationRecord,
        prompt_plan: &crate::orchestration::GenerationPromptPlanRecord,
        knowledge_logs: &[crate::orchestration::KnowledgeActivationLog],
    ) -> CoreResult<()> {
        self.append_generation_observed(
            branch_id,
            expected_head,
            user,
            assistant,
            generation,
            Some((prompt_plan, knowledge_logs)),
            false,
            None,
            false,
        )
    }

    /// Production append boundary for a generation whose exact
    /// `BeforeGeneration` processing has reached `dispatch_ready`.
    ///
    /// The attempt identity, source head, target branch, module composition,
    /// and prompt fingerprint are rechecked in the same transaction that
    /// makes the generation visible and transitions the attempt to `running`.
    #[allow(clippy::too_many_arguments)]
    pub fn append_generation_attempt_with_prompt_plan(
        &self,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        user: &Message,
        assistant: &Message,
        generation: &GenerationRecord,
        prompt_plan: &crate::orchestration::GenerationPromptPlanRecord,
        knowledge_logs: &[crate::orchestration::KnowledgeActivationLog],
        credential_authority: Option<&crate::ProviderCredentialAccessAuthority>,
        require_exact_credential_authority: bool,
    ) -> CoreResult<()> {
        self.append_generation_observed(
            branch_id,
            expected_head,
            user,
            assistant,
            generation,
            Some((prompt_plan, knowledge_logs)),
            true,
            credential_authority,
            require_exact_credential_authority,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn append_generation_observed(
        &self,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        user: &Message,
        assistant: &Message,
        generation: &GenerationRecord,
        prompt_plan: Option<(
            &crate::orchestration::GenerationPromptPlanRecord,
            &[crate::orchestration::KnowledgeActivationLog],
        )>,
        require_attempt: bool,
        credential_authority: Option<&crate::ProviderCredentialAccessAuthority>,
        require_exact_credential_authority: bool,
    ) -> CoreResult<()> {
        let observation = GenerationAppendObservation {
            branch_id,
            expected_head,
            user,
            assistant,
            generation,
            prompt_plan,
            require_attempt,
        };
        validate_generation_append(branch_id, expected_head, user, assistant, generation)?;
        validate_generation_prompt_plan_link(
            branch_id,
            expected_head,
            user,
            generation,
            prompt_plan.map(|value| value.0),
        )?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        ensure_generation_provider_credential_settled(
            &transaction,
            generation,
            credential_authority,
            require_exact_credential_authority,
        )?;
        require_generation_append_branch(&transaction, &observation)?;
        let dispatch_attempt = prepare_generation_append_attempt(&transaction, &observation)?;
        let occurred_at = Utc::now();
        if let Some(prepared) = dispatch_attempt.as_ref() {
            materialize_generation_append_attempt(
                self,
                &transaction,
                &observation,
                prepared,
                occurred_at,
            )?;
        }
        write_generation_append(
            &transaction,
            &observation,
            dispatch_attempt.as_ref(),
            occurred_at,
        )?;
        transaction.commit().map_err(storage_db_error)
    }
}

fn require_generation_append_branch(
    transaction: &rusqlite::Transaction<'_>,
    observation: &GenerationAppendObservation<'_>,
) -> CoreResult<()> {
    let stored = transaction
        .query_row(
            "SELECT conversation_id, head_message_id
             FROM conversation_branches
             WHERE id = ?1",
            [&observation.branch_id.0],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "conversation branch was not found",
                false,
            )
        })?;
    if stored.0 != observation.user.conversation_id.0
        || stored.1.as_deref()
            != observation
                .expected_head
                .map(|message_id| message_id.0.as_str())
    {
        return Err(stale_branch_error());
    }
    let Some(head_id) = observation.expected_head else {
        return Ok(());
    };
    let pending = transaction
        .query_row(
            "SELECT status = 'pending'
             FROM messages
             WHERE conversation_id = ?1 AND id = ?2",
            params![observation.user.conversation_id.0, head_id.0],
            |row| row.get::<_, bool>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "expected branch head was not found",
                false,
            )
        })?;
    if pending {
        Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            "cannot append while the branch head is still generating",
            true,
        ))
    } else {
        Ok(())
    }
}

fn prepare_generation_append_attempt(
    transaction: &rusqlite::Transaction<'_>,
    observation: &GenerationAppendObservation<'_>,
) -> CoreResult<Option<PreparedGenerationAppendAttempt>> {
    if !observation.require_attempt {
        return Ok(None);
    }
    let prompt_plan = observation
        .prompt_plan
        .map(|value| value.0)
        .ok_or_else(|| CoreError::invalid("generation attempt requires a prompt plan"))?;
    let module_plan_sha256 = generation_prompt_module_plan_sha256(prompt_plan)?;
    let prompt_plan_sha256 =
        Sha256Digest::parse(prompt_plan.plan_sha256.clone()).map_err(CoreError::invalid)?;
    let input_fingerprint_sha256 =
        Sha256Digest::parse(prompt_plan.input_fingerprint_sha256.clone())
            .map_err(CoreError::invalid)?;
    let attempt = crate::generation_attempt::require_dispatch_ready_attempt(
        transaction,
        &observation.generation.id,
        &observation.user.conversation_id,
        observation.branch_id,
        observation.branch_id,
        observation.expected_head,
        &module_plan_sha256,
        &prompt_plan_sha256,
        &input_fingerprint_sha256,
    )?;
    crate::interaction_repository::require_generation_attempt_prompt_context_authority_transaction(
        transaction,
        &attempt,
        prompt_plan,
    )?;
    let state_id = transaction
        .query_row(
            "SELECT id
             FROM interaction_state
             WHERE conversation_id = ?1 AND branch_id = ?2",
            params![
                observation.user.conversation_id.0.as_str(),
                observation.branch_id.0.as_str()
            ],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "generation append interaction state was not found",
                false,
            )
        })?;
    Ok(Some(PreparedGenerationAppendAttempt {
        attempt,
        target_key: InteractionStateKey {
            state_id,
            conversation_id: observation.user.conversation_id.clone(),
            branch_id: observation.branch_id.clone(),
        },
    }))
}

fn materialize_generation_append_attempt(
    storage: &Storage,
    transaction: &rusqlite::Transaction<'_>,
    observation: &GenerationAppendObservation<'_>,
    prepared: &PreparedGenerationAppendAttempt,
    occurred_at: DateTime<Utc>,
) -> CoreResult<()> {
    let final_prompt_plan = observation
        .prompt_plan
        .map(|value| value.0)
        .ok_or_else(|| {
            CoreError::invalid("generation interaction materialization requires a prompt plan")
        })?;
    materialize_and_validate_generation_attempt(
        storage,
        transaction,
        &prepared.attempt,
        &prepared.target_key,
        final_prompt_plan,
        occurred_at,
    )
}

pub(super) fn materialize_and_validate_generation_attempt(
    storage: &Storage,
    transaction: &rusqlite::Transaction<'_>,
    attempt: &crate::generation_attempt::StoredGenerationAttempt,
    target_key: &InteractionStateKey,
    prompt_plan: &crate::orchestration::GenerationPromptPlanRecord,
    occurred_at: DateTime<Utc>,
) -> CoreResult<()> {
    let receipt = materialize_generation_attempt_interaction_for_append(
        storage,
        transaction,
        attempt,
        target_key,
        prompt_plan,
        occurred_at,
    )?;
    let seal = attempt.dispatch_seal.as_ref().ok_or_else(|| {
        CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "dispatch-ready generation attempt is missing its seal",
            false,
        )
    })?;
    if receipt.final_state_revision == seal.final_interaction_state_revision
        && receipt.final_state_snapshot_sha256 == seal.final_interaction_state_sha256
    {
        Ok(())
    } else {
        Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "generation interaction materialization receipt differs from its dispatch seal",
            false,
        ))
    }
}

fn write_generation_append(
    transaction: &rusqlite::Transaction<'_>,
    observation: &GenerationAppendObservation<'_>,
    prepared: Option<&PreparedGenerationAppendAttempt>,
    occurred_at: DateTime<Utc>,
) -> CoreResult<()> {
    insert_message(transaction, observation.user)?;
    insert_message(transaction, observation.assistant)?;
    let prompt_plan_link = observation
        .prompt_plan
        .map(|(record, logs)| {
            crate::orchestration::write_generation_prompt_plan(transaction, record, logs)
        })
        .transpose()?;
    insert_generation(
        transaction,
        observation.generation,
        prompt_plan_link.as_ref(),
    )?;
    if let Some(prepared) = prepared {
        crate::generation_attempt::mark_attempt_running_in_transaction(
            transaction,
            &prepared.attempt,
            occurred_at,
        )?;
    }
    let now = occurred_at.to_rfc3339();
    let changed = transaction
        .execute(
            "UPDATE conversation_branches
             SET head_message_id = ?3, updated_at = ?4
             WHERE id = ?1
               AND conversation_id = ?2
               AND ((head_message_id IS NULL AND ?5 IS NULL) OR head_message_id = ?5)",
            params![
                observation.branch_id.0,
                observation.user.conversation_id.0,
                observation.assistant.id.0,
                now,
                observation
                    .expected_head
                    .map(|message_id| message_id.0.as_str())
            ],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(stale_branch_error());
    }
    transaction
        .execute(
            "UPDATE conversations SET updated_at = ?2 WHERE id = ?1",
            params![observation.user.conversation_id.0, now],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

pub(super) fn validate_generation_append(
    branch_id: &ConversationBranchId,
    expected_head: Option<&MessageId>,
    user: &Message,
    assistant: &Message,
    generation: &GenerationRecord,
) -> CoreResult<()> {
    if user.role != MessageRole::User
        || user.status != MessageStatus::Complete
        || user.generation_id.is_some()
        || user.parent_id.as_ref() != expected_head
    {
        return Err(CoreError::invalid(
            "branch append requires a complete user message parented to the expected head",
        ));
    }
    if assistant.role != MessageRole::Assistant
        || assistant.status != MessageStatus::Pending
        || assistant.parent_id.as_ref() != Some(&user.id)
        || assistant.conversation_id != user.conversation_id
    {
        return Err(CoreError::invalid(
            "branch append requires a pending assistant child of the user message",
        ));
    }
    if generation.status != GenerationStatus::Running
        || generation.finished_at.is_some()
        || !generation.opaque_reasoning_state.is_empty()
        || generation.id
            != assistant.generation_id.clone().ok_or_else(|| {
                CoreError::invalid("pending assistant message requires a generation id")
            })?
        || generation.conversation_id != user.conversation_id
        || &generation.branch_id != branch_id
        || generation.user_message_id != user.id
        || generation.assistant_message_id.as_ref() != Some(&assistant.id)
    {
        return Err(CoreError::invalid(
            "generation record does not own the appended user and assistant messages",
        ));
    }
    Ok(())
}

pub(super) fn validate_generation_prompt_plan_link(
    branch_id: &ConversationBranchId,
    expected_head: Option<&MessageId>,
    user: &Message,
    generation: &GenerationRecord,
    prompt_plan: Option<&crate::orchestration::GenerationPromptPlanRecord>,
) -> CoreResult<()> {
    let Some(prompt_plan) = prompt_plan else {
        return Ok(());
    };
    let provider_family_matches_route = if generation.model_route_id.is_some() {
        generation.provider_family == Some(prompt_plan.provider_request.api_family)
    } else {
        // Legacy credential-backed profiles intentionally have no catalog
        // route/family provenance on the generation row. The immutable prompt
        // request snapshot still records the concrete wire family used.
        generation.provider_family.is_none()
    };
    if prompt_plan.generation_id != generation.id
        || prompt_plan.conversation_id != generation.conversation_id
        || &prompt_plan.branch_id != branch_id
        || prompt_plan.head_message_id.as_ref() != expected_head
        || prompt_plan.latest_user_message_id != user.id
        || prompt_plan.model_route_id != generation.model_route_id
        || prompt_plan.generation_preset_id != generation.generation_preset_id
        || !provider_family_matches_route
    {
        return Err(CoreError::invalid(
            "generation prompt plan does not match the appended generation",
        ));
    }
    Ok(())
}

pub(super) fn generation_prompt_module_plan_sha256(
    prompt_plan: &crate::orchestration::GenerationPromptPlanRecord,
) -> CoreResult<Sha256Digest> {
    let value = prompt_plan
        .provider_request
        .mapping_diagnostics
        .value
        .get("module_plan_sha256");
    match value {
        None | Some(serde_json::Value::Null) => {
            Ok(lorepia_orchestration::no_applied_module_runtime_plan_sha256())
        }
        Some(serde_json::Value::String(value)) => {
            Sha256Digest::parse(value.to_owned()).map_err(CoreError::invalid)
        }
        Some(_) => Err(CoreError::invalid(
            "generation module plan diagnostic must be a SHA-256 string or null",
        )),
    }
}

pub(super) fn insert_generation(
    transaction: &rusqlite::Transaction<'_>,
    generation: &GenerationRecord,
    prompt_plan: Option<&crate::orchestration::GenerationPromptPlanLink>,
) -> CoreResult<()> {
    let opaque_reasoning_state =
        serialize_opaque_reasoning_state(&generation.opaque_reasoning_state)?;
    transaction
        .execute(
            "INSERT INTO generations
             (id, conversation_id, branch_id, user_message_id, assistant_message_id,
              mode, model, status, input_tokens, output_tokens, error_code,
              started_at, finished_at, model_route_id, generation_preset_id,
              provider_family, cached_read_tokens, cached_write_tokens,
              reasoning_tokens, tool_tokens, provider_raw_summary_json,
              opaque_reasoning_state_json, resolved_prompt_plan_id,
              prompt_plan_sha256, provider_request_snapshot_id)
             VALUES (
                 ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                 ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24,
                 ?25
             )",
            params![
                generation.id.0,
                generation.conversation_id.0,
                generation.branch_id.0,
                generation.user_message_id.0,
                generation
                    .assistant_message_id
                    .as_ref()
                    .map(|value| value.0.as_str()),
                mode_to_str(generation.mode),
                generation.model,
                generation_status_to_str(generation.status),
                generation.input_tokens.map(u64_to_i64).transpose()?,
                generation.output_tokens.map(u64_to_i64).transpose()?,
                generation.error_code,
                generation.started_at.to_rfc3339(),
                generation.finished_at.map(|value| value.to_rfc3339()),
                generation.model_route_id.as_ref().map(ModelRouteId::as_str),
                generation
                    .generation_preset_id
                    .as_ref()
                    .map(GenerationPresetId::as_str),
                generation.provider_family.map(api_family_to_str),
                generation.cached_read_tokens.map(u64_to_i64).transpose()?,
                generation.cached_write_tokens.map(u64_to_i64).transpose()?,
                generation.reasoning_tokens.map(u64_to_i64).transpose()?,
                generation.tool_tokens.map(u64_to_i64).transpose()?,
                generation
                    .provider_raw_summary
                    .as_ref()
                    .map(BoundedJson::as_str),
                opaque_reasoning_state,
                prompt_plan.map(|link| link.plan_id.as_str()),
                prompt_plan.map(|link| link.plan_sha256.as_str()),
                prompt_plan.map(|link| link.provider_request_snapshot_id.as_str()),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}
