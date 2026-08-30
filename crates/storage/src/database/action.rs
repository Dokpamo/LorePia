use super::generation_append::{
    PromptPlanObservation, generation_prompt_module_plan_sha256, insert_generation,
    materialize_and_validate_generation_attempt, validate_generation_append,
    validate_generation_prompt_plan_link,
};
use super::{
    ConversationBranch, ConversationBranchId, CoreError, CoreResult, DateTime, GenerationRecord,
    InteractionStateKey, Message, MessageGenerationAction, MessageGenerationActionContext,
    MessageId, Sha256Digest, Storage, TransactionBehavior, Utc,
    ensure_generation_provider_credential_settled, insert_message,
    load_message_generation_action_context, params, stale_branch_error, storage_db_error,
};

struct MessageActionAppendObservation<'a> {
    source_branch_id: &'a ConversationBranchId,
    expected_source_head: Option<&'a MessageId>,
    target_message_id: &'a MessageId,
    action: MessageGenerationAction,
    branch: &'a ConversationBranch,
    target_interaction_state_key: Option<&'a InteractionStateKey>,
    user: &'a Message,
    assistant: &'a Message,
    generation: &'a GenerationRecord,
    prompt_plan: Option<PromptPlanObservation<'a>>,
    require_attempt: bool,
}

impl Storage {
    #[allow(clippy::too_many_arguments)]
    pub fn append_message_generation_action(
        &self,
        source_branch_id: &ConversationBranchId,
        expected_source_head: Option<&MessageId>,
        target_message_id: &MessageId,
        action: MessageGenerationAction,
        branch: &ConversationBranch,
        user: &Message,
        assistant: &Message,
        generation: &GenerationRecord,
    ) -> CoreResult<()> {
        self.append_message_generation_action_observed(
            source_branch_id,
            expected_source_head,
            target_message_id,
            action,
            branch,
            None,
            user,
            assistant,
            generation,
            None,
            false,
            None,
            false,
        )
    }

    /// Atomic message-action variant that seals and attaches exact prompt
    /// provenance before the new generation becomes visible.
    #[allow(clippy::too_many_arguments)]
    pub fn append_message_generation_action_with_prompt_plan(
        &self,
        source_branch_id: &ConversationBranchId,
        expected_source_head: Option<&MessageId>,
        target_message_id: &MessageId,
        action: MessageGenerationAction,
        branch: &ConversationBranch,
        user: &Message,
        assistant: &Message,
        generation: &GenerationRecord,
        prompt_plan: &crate::orchestration::GenerationPromptPlanRecord,
        knowledge_logs: &[crate::orchestration::KnowledgeActivationLog],
    ) -> CoreResult<()> {
        self.append_message_generation_action_observed(
            source_branch_id,
            expected_source_head,
            target_message_id,
            action,
            branch,
            None,
            user,
            assistant,
            generation,
            Some((prompt_plan, knowledge_logs)),
            false,
            None,
            false,
        )
    }

    /// Attempt-bound action append. The new branch and its complete generation
    /// remain invisible unless the exact source snapshot and dispatch-ready
    /// attempt both validate in this transaction.
    #[allow(clippy::too_many_arguments)]
    pub fn append_message_generation_action_attempt_with_prompt_plan(
        &self,
        source_branch_id: &ConversationBranchId,
        expected_source_head: Option<&MessageId>,
        target_message_id: &MessageId,
        action: MessageGenerationAction,
        branch: &ConversationBranch,
        target_interaction_state_key: &InteractionStateKey,
        user: &Message,
        assistant: &Message,
        generation: &GenerationRecord,
        prompt_plan: &crate::orchestration::GenerationPromptPlanRecord,
        knowledge_logs: &[crate::orchestration::KnowledgeActivationLog],
        credential_authority: Option<&crate::ProviderCredentialAccessAuthority>,
        require_exact_credential_authority: bool,
    ) -> CoreResult<()> {
        self.append_message_generation_action_observed(
            source_branch_id,
            expected_source_head,
            target_message_id,
            action,
            branch,
            Some(target_interaction_state_key),
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
    fn append_message_generation_action_observed(
        &self,
        source_branch_id: &ConversationBranchId,
        expected_source_head: Option<&MessageId>,
        target_message_id: &MessageId,
        action: MessageGenerationAction,
        branch: &ConversationBranch,
        target_interaction_state_key: Option<&InteractionStateKey>,
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
        let observation = MessageActionAppendObservation {
            source_branch_id,
            expected_source_head,
            target_message_id,
            action,
            branch,
            target_interaction_state_key,
            user,
            assistant,
            generation,
            prompt_plan,
            require_attempt,
        };
        validate_message_action_append(&observation)?;

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
        require_message_action_context(&transaction, &observation)?;
        let dispatch_attempt = prepare_message_action_attempt(&transaction, &observation)?;
        let occurred_at = Utc::now();
        write_message_action_append(
            self,
            &transaction,
            &observation,
            dispatch_attempt.as_ref(),
            occurred_at,
        )?;
        transaction.commit().map_err(storage_db_error)
    }
}

fn validate_message_action_append(
    observation: &MessageActionAppendObservation<'_>,
) -> CoreResult<()> {
    validate_generation_append(
        &observation.branch.id,
        observation.branch.fork_message_id.as_ref(),
        observation.user,
        observation.assistant,
        observation.generation,
    )?;
    validate_generation_prompt_plan_link(
        &observation.branch.id,
        observation.branch.fork_message_id.as_ref(),
        observation.user,
        observation.generation,
        observation.prompt_plan.map(|value| value.0),
    )?;
    if observation.branch.conversation_id != observation.user.conversation_id
        || observation.branch.head_message_id.as_ref() != Some(&observation.assistant.id)
        || observation.branch.fork_message_id != observation.user.parent_id
    {
        return Err(CoreError::invalid(
            "message action branch does not own the appended generation",
        ));
    }
    match (
        observation.require_attempt,
        observation.target_interaction_state_key,
    ) {
        (true, Some(key))
            if key.conversation_id == observation.branch.conversation_id
                && key.branch_id == observation.branch.id
                && !key.state_id.trim().is_empty() =>
        {
            Ok(())
        }
        (true, _) => Err(CoreError::invalid(
            "attempt-bound message action requires its exact target interaction state key",
        )),
        (false, None) => Ok(()),
        (false, Some(_)) => Err(CoreError::invalid(
            "legacy message action cannot materialize a generation interaction attempt",
        )),
    }
}

fn require_message_action_context(
    transaction: &rusqlite::Transaction<'_>,
    observation: &MessageActionAppendObservation<'_>,
) -> CoreResult<MessageGenerationActionContext> {
    let context = load_message_generation_action_context(
        transaction,
        &observation.user.conversation_id,
        observation.source_branch_id,
        observation.expected_source_head,
        observation.target_message_id,
        observation.action,
    )?;
    if context.fork_message_id != observation.branch.fork_message_id
        || (observation.action == MessageGenerationAction::RegenerateAssistant
            && context.user_text != observation.user.content)
    {
        Err(stale_branch_error())
    } else {
        Ok(context)
    }
}

fn prepare_message_action_attempt(
    transaction: &rusqlite::Transaction<'_>,
    observation: &MessageActionAppendObservation<'_>,
) -> CoreResult<Option<crate::generation_attempt::StoredGenerationAttempt>> {
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
        observation.source_branch_id,
        &observation.branch.id,
        observation.expected_source_head,
        &module_plan_sha256,
        &prompt_plan_sha256,
        &input_fingerprint_sha256,
    )?;
    crate::interaction_repository::require_generation_attempt_prompt_context_authority_transaction(
        transaction,
        &attempt,
        prompt_plan,
    )?;
    Ok(Some(attempt))
}

fn write_message_action_append(
    storage: &Storage,
    transaction: &rusqlite::Transaction<'_>,
    observation: &MessageActionAppendObservation<'_>,
    dispatch_attempt: Option<&crate::generation_attempt::StoredGenerationAttempt>,
    occurred_at: DateTime<Utc>,
) -> CoreResult<()> {
    insert_message(transaction, observation.user)?;
    insert_message(transaction, observation.assistant)?;
    insert_message_action_branch(transaction, observation.branch)?;
    if let (Some(attempt), Some(target_key)) =
        (dispatch_attempt, observation.target_interaction_state_key)
    {
        let prompt_plan = observation
            .prompt_plan
            .map(|value| value.0)
            .ok_or_else(|| {
                CoreError::invalid("generation interaction materialization requires a prompt plan")
            })?;
        materialize_and_validate_generation_attempt(
            storage,
            transaction,
            attempt,
            target_key,
            prompt_plan,
            occurred_at,
        )?;
    }
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
    if let Some(attempt) = dispatch_attempt {
        crate::generation_attempt::mark_attempt_running_in_transaction(
            transaction,
            attempt,
            occurred_at,
        )?;
    }
    activate_message_action_branch(transaction, observation, occurred_at)
}

fn insert_message_action_branch(
    transaction: &rusqlite::Transaction<'_>,
    branch: &ConversationBranch,
) -> CoreResult<()> {
    transaction
        .execute(
            "INSERT INTO conversation_branches
             (id, conversation_id, title, fork_message_id, head_message_id,
              created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                branch.id.0,
                branch.conversation_id.0,
                branch.title,
                branch
                    .fork_message_id
                    .as_ref()
                    .map(|message_id| message_id.0.as_str()),
                branch
                    .head_message_id
                    .as_ref()
                    .map(|message_id| message_id.0.as_str()),
                branch.created_at.to_rfc3339(),
                branch.updated_at.to_rfc3339()
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn activate_message_action_branch(
    transaction: &rusqlite::Transaction<'_>,
    observation: &MessageActionAppendObservation<'_>,
    occurred_at: DateTime<Utc>,
) -> CoreResult<()> {
    let now = occurred_at.to_rfc3339();
    let changed = transaction
        .execute(
            "UPDATE conversation_state
             SET active_branch_id = ?3, updated_at = ?4
             WHERE conversation_id = ?1 AND active_branch_id = ?2",
            params![
                observation.user.conversation_id.0,
                observation.source_branch_id.0,
                observation.branch.id.0,
                now
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
