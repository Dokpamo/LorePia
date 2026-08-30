use chrono::{DateTime, Utc};
use lorepia_domain::{
    ConversationBranchId, ConversationId, CoreError, CoreResult, GenerationId,
    InteractionProposalStatus, InteractionState, MessageId,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::Storage;

use super::{
    ClonedInteractionCheckpoint, InteractionKnowledgeBinding, InteractionStateKey,
    MAX_STATE_JSON_BYTES, StoredGenerationAttemptInteractionBoundary, StoredInteractionState,
    StoredInteractionStateCheckpoint, decode_json, encode_json, i64_from_u64,
    interaction_state_snapshot_sha256, is_sha256, not_found, parse_datetime,
    read_generation_attempt_interaction_aggregate, read_knowledge_bindings, read_state_row,
    replace_normalized_state, require_state_for_key, revision_conflict, sha256_hex,
    storage_corrupted, storage_db_error, u64_from_i64, validate_key, validate_knowledge_bindings,
    validate_nonempty_id, validate_normalized_state, validate_state,
};

#[derive(Debug)]
pub(super) struct GenerationAttemptAuthority {
    pub(super) revision: u64,
    pub(super) status: String,
    pub(super) conversation_id: ConversationId,
    pub(super) source_branch_id: ConversationBranchId,
    pub(super) proposed_branch_id: ConversationBranchId,
    pub(super) context_head_message_id: Option<MessageId>,
    pub(super) module_plan_sha256: String,
}
impl Storage {
    /// Loads and verifies the immutable interaction snapshot at one exact
    /// committed-message boundary.
    pub fn get_interaction_state_checkpoint(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        message_id: &MessageId,
    ) -> CoreResult<StoredInteractionStateCheckpoint> {
        validate_nonempty_id("interaction checkpoint message id", &message_id.0)?;
        let connection = self.connection()?;
        read_interaction_state_checkpoint(&connection, conversation_id, branch_id, message_id)?
            .ok_or_else(|| not_found("interaction state checkpoint"))
    }

    /// Returns the exact initial interaction boundary only while the branch
    /// still has no message head.
    pub fn get_empty_branch_interaction_boundary(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
    ) -> CoreResult<StoredInteractionState> {
        let connection = self.connection()?;
        let head = connection
            .query_row(
                "SELECT head_message_id
                 FROM conversation_branches
                 WHERE conversation_id = ?1 AND id = ?2",
                params![conversation_id.0.as_str(), branch_id.0.as_str()],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("conversation branch"))?;
        if head.is_some() {
            return Err(revision_conflict(
                "interaction initial boundary is no longer the branch head",
            ));
        }
        let row = read_state_row(&connection, conversation_id, branch_id)?
            .ok_or_else(|| not_found("interaction state"))?;
        validate_normalized_state(&connection, &row)?;
        let knowledge = read_knowledge_bindings(&connection, &row.id)?;
        Ok(StoredInteractionState {
            key: InteractionStateKey {
                state_id: row.id,
                conversation_id: row.conversation_id,
                branch_id: row.branch_id,
            },
            state: row.state,
            knowledge,
        })
    }

    /// Resolves the exact review boundary named by one immutable generation
    /// attempt, including same-branch live state, a historical fork
    /// checkpoint, or the pre-first-message root snapshot.
    pub fn get_generation_attempt_interaction_boundary(
        &self,
        generation_id: &GenerationId,
    ) -> CoreResult<StoredGenerationAttemptInteractionBoundary> {
        validate_nonempty_id("generation attempt id", &generation_id.0)?;
        let connection = self.connection()?;
        let authority = read_generation_attempt_authority(&connection, generation_id)?;
        let (state, knowledge, context_checkpoint_sha256) =
            read_generation_attempt_review_boundary(&connection, &authority)?;
        let source_state_id = connection
            .query_row(
                "SELECT id
                 FROM interaction_state
                 WHERE conversation_id = ?1 AND branch_id = ?2",
                params![
                    authority.conversation_id.0.as_str(),
                    authority.source_branch_id.0.as_str(),
                ],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("generation attempt source interaction state"))?;
        Ok(StoredGenerationAttemptInteractionBoundary {
            state: StoredInteractionState {
                key: InteractionStateKey {
                    state_id: source_state_id,
                    conversation_id: authority.conversation_id,
                    branch_id: authority.source_branch_id,
                },
                state,
                knowledge,
            },
            context_checkpoint_sha256,
        })
    }
}

pub(super) fn write_interaction_state_checkpoint(
    transaction: &Transaction<'_>,
    key: &InteractionStateKey,
    message_id: &MessageId,
    state: &InteractionState,
    knowledge: &[InteractionKnowledgeBinding],
    created_at: DateTime<Utc>,
) -> CoreResult<()> {
    let terminal_head_exists = transaction
        .query_row(
            "SELECT EXISTS(
                 SELECT 1
                 FROM conversation_branches AS branch
                 JOIN messages AS message
                   ON message.conversation_id = branch.conversation_id
                  AND message.id = branch.head_message_id
                 WHERE branch.conversation_id = ?1
                   AND branch.id = ?2
                   AND branch.head_message_id = ?3
                   AND message.status != 'pending'
             )",
            params![
                key.conversation_id.0.as_str(),
                key.branch_id.0.as_str(),
                message_id.0.as_str(),
            ],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)?;
    if !terminal_head_exists {
        return Err(revision_conflict(
            "interaction checkpoint owner is not the exact terminal branch head",
        ));
    }
    validate_state(state)?;
    validate_knowledge_bindings(state, knowledge)?;
    let state_document_json =
        encode_json("interaction checkpoint state", state, MAX_STATE_JSON_BYTES)?;
    let mut ordered_knowledge = knowledge.to_vec();
    ordered_knowledge.sort();
    let knowledge_bindings_json = encode_json(
        "interaction checkpoint knowledge",
        &ordered_knowledge,
        MAX_STATE_JSON_BYTES,
    )?;
    let checkpoint_sha256 = interaction_state_snapshot_sha256(state, &ordered_knowledge)?;
    transaction
        .execute(
            "INSERT INTO interaction_state_checkpoints
             (conversation_id, branch_id, message_id,
              source_interaction_state_id, state_revision,
              state_document_json, knowledge_bindings_json,
              checkpoint_sha256, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                key.conversation_id.0.as_str(),
                key.branch_id.0.as_str(),
                message_id.0.as_str(),
                key.state_id,
                i64_from_u64("interaction checkpoint state revision", state.revision)?,
                state_document_json,
                knowledge_bindings_json,
                checkpoint_sha256,
                created_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn read_interaction_state_checkpoint(
    connection: &Connection,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    message_id: &MessageId,
) -> CoreResult<Option<StoredInteractionStateCheckpoint>> {
    let raw = connection
        .query_row(
            "SELECT source_interaction_state_id, state_revision,
                    state_document_json, knowledge_bindings_json,
                    checkpoint_sha256, created_at
             FROM interaction_state_checkpoints
             WHERE conversation_id = ?1 AND branch_id = ?2
               AND message_id = ?3",
            params![
                conversation_id.0.as_str(),
                branch_id.0.as_str(),
                message_id.0.as_str(),
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?;
    raw.map(
        |(
            source_interaction_state_id,
            state_revision,
            state_document_json,
            knowledge_bindings_json,
            checkpoint_sha256,
            created_at,
        )| {
            validate_nonempty_id(
                "checkpoint source interaction state id",
                &source_interaction_state_id,
            )
            .map_err(|error| {
                storage_corrupted(format!(
                    "stored checkpoint state identity is invalid: {error}"
                ))
            })?;
            let state: InteractionState = decode_json(
                "stored interaction checkpoint state",
                &state_document_json,
                MAX_STATE_JSON_BYTES,
            )?;
            let state_revision =
                u64_from_i64("interaction checkpoint state revision", state_revision)?;
            if state.revision != state_revision
                || encode_json(
                    "stored interaction checkpoint state",
                    &state,
                    MAX_STATE_JSON_BYTES,
                )? != state_document_json
            {
                return Err(storage_corrupted(
                    "stored interaction checkpoint state is non-canonical",
                ));
            }
            validate_state(&state).map_err(|error| {
                storage_corrupted(format!(
                    "stored interaction checkpoint state is invalid: {error}"
                ))
            })?;
            let knowledge: Vec<InteractionKnowledgeBinding> = decode_json(
                "stored interaction checkpoint knowledge",
                &knowledge_bindings_json,
                MAX_STATE_JSON_BYTES,
            )?;
            validate_knowledge_bindings(&state, &knowledge).map_err(|error| {
                storage_corrupted(format!(
                    "stored interaction checkpoint knowledge is invalid: {error}"
                ))
            })?;
            let mut ordered_knowledge = knowledge.clone();
            ordered_knowledge.sort();
            if ordered_knowledge != knowledge
                || encode_json(
                    "stored interaction checkpoint knowledge",
                    &knowledge,
                    MAX_STATE_JSON_BYTES,
                )? != knowledge_bindings_json
                || interaction_state_snapshot_sha256(&state, &knowledge)? != checkpoint_sha256
            {
                return Err(storage_corrupted(
                    "stored interaction checkpoint fingerprint does not match its payload",
                ));
            }
            Ok(StoredInteractionStateCheckpoint {
                conversation_id: conversation_id.clone(),
                branch_id: branch_id.clone(),
                message_id: message_id.clone(),
                source_interaction_state_id,
                state,
                knowledge,
                checkpoint_sha256,
                created_at: parse_datetime("interaction checkpoint created_at", &created_at)?,
            })
        },
    )
    .transpose()
}

fn read_generation_user_interaction_boundary(
    connection: &Connection,
    conversation_id: &ConversationId,
    source_branch_id: &ConversationBranchId,
    user_message_id: &MessageId,
) -> CoreResult<(InteractionState, Vec<InteractionKnowledgeBinding>, String)> {
    let (generation_id, matching_count) = connection
        .query_row(
            "SELECT MIN(generation.id), COUNT(*)
             FROM generations AS generation
             JOIN generation_attempt_interaction_aggregates AS aggregate
               ON aggregate.generation_id = generation.id
             WHERE generation.conversation_id = ?1
               AND generation.branch_id = ?2
               AND generation.user_message_id = ?3",
            params![
                conversation_id.0.as_str(),
                source_branch_id.0.as_str(),
                user_message_id.0.as_str(),
            ],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?)),
        )
        .map_err(storage_db_error)?;
    if matching_count != 1 {
        return Err(if matching_count == 0 {
            not_found("interaction state checkpoint")
        } else {
            storage_corrupted(
                "multiple generation attempts own the same branch user-message boundary",
            )
        });
    }
    let generation_id = GenerationId(generation_id.ok_or_else(|| {
        storage_corrupted("generation user-message boundary identity is missing")
    })?);
    let aggregate = read_generation_attempt_interaction_aggregate(connection, &generation_id)?;
    Ok((
        aggregate.state,
        aggregate.knowledge,
        aggregate.state_snapshot_sha256.into_inner(),
    ))
}

/// Clones the exact historical interaction boundary into a newly-created
/// action branch inside the caller's transaction.
///
/// Terminal proposal records are branch-local audit history and are not
/// inherited. A pending proposal blocks the clone so approval authority cannot
/// be bypassed by forking.
pub(crate) fn clone_interaction_checkpoint_for_branch_transaction(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    source_branch_id: &ConversationBranchId,
    context_head_message_id: Option<&MessageId>,
    target_key: &InteractionStateKey,
    updated_at: DateTime<Utc>,
) -> CoreResult<ClonedInteractionCheckpoint> {
    validate_key(target_key)?;
    if target_key.conversation_id != *conversation_id || target_key.branch_id == *source_branch_id {
        return Err(CoreError::invalid(
            "interaction checkpoint clone target must be a distinct branch in the same conversation",
        ));
    }
    let (source, generation_user_boundary, historical_root) =
        if let Some(message_id) = context_head_message_id {
            if let Some(checkpoint) = read_interaction_state_checkpoint(
                transaction,
                conversation_id,
                source_branch_id,
                message_id,
            )? {
                (Some(checkpoint), None, None)
            } else {
                (
                    None,
                    Some(read_generation_user_interaction_boundary(
                        transaction,
                        conversation_id,
                        source_branch_id,
                        message_id,
                    )?),
                    None,
                )
            }
        } else {
            let source_head = transaction
                .query_row(
                    "SELECT head_message_id
                 FROM conversation_branches
                 WHERE conversation_id = ?1 AND id = ?2",
                    params![conversation_id.0.as_str(), source_branch_id.0.as_str(),],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()
                .map_err(storage_db_error)?
                .ok_or_else(|| not_found("source conversation branch"))?;
            let historical_root = source_head
                .is_some()
                .then(|| {
                    read_pre_first_message_interaction_boundary(
                        transaction,
                        conversation_id,
                        source_branch_id,
                    )
                })
                .transpose()?;
            (None, None, historical_root)
        };
    let (mut cloned_state, knowledge, checkpoint_sha256) = if let Some(checkpoint) = &source {
        (
            checkpoint.state.clone(),
            checkpoint.knowledge.clone(),
            checkpoint.checkpoint_sha256.clone(),
        )
    } else if let Some((state, knowledge, checkpoint_sha256)) = historical_root {
        (state, knowledge, checkpoint_sha256)
    } else if let Some((state, knowledge, checkpoint_sha256)) = generation_user_boundary {
        (state, knowledge, checkpoint_sha256)
    } else {
        let current = read_state_row(transaction, conversation_id, source_branch_id)?
            .ok_or_else(|| not_found("source interaction state"))?;
        validate_normalized_state(transaction, &current)?;
        let knowledge = read_knowledge_bindings(transaction, &current.id)?;
        let checkpoint_sha256 = interaction_state_snapshot_sha256(&current.state, &knowledge)?;
        (current.state, knowledge, checkpoint_sha256)
    };
    if cloned_state
        .proposals
        .iter()
        .any(|proposal| proposal.status == InteractionProposalStatus::Pending)
    {
        return Err(revision_conflict(
            "cannot clone an interaction checkpoint with a pending proposal",
        ));
    }
    cloned_state.proposals.clear();
    validate_state(&cloned_state)?;
    validate_knowledge_bindings(&cloned_state, &knowledge)?;
    let target_fork_message_id = transaction
        .query_row(
            "SELECT fork_message_id
             FROM conversation_branches
             WHERE conversation_id = ?1 AND id = ?2",
            params![conversation_id.0.as_str(), target_key.branch_id.0.as_str(),],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("target conversation branch"))?;
    if target_fork_message_id.as_deref()
        != context_head_message_id.map(|message_id| message_id.0.as_str())
    {
        return Err(revision_conflict(
            "interaction checkpoint does not match the target branch fork boundary",
        ));
    }
    let state_document_json = encode_json(
        "cloned interaction state",
        &cloned_state,
        MAX_STATE_JSON_BYTES,
    )?;
    let cloned_state_document_sha256 = sha256_hex(state_document_json.as_bytes());
    let cloned_state_snapshot_sha256 =
        interaction_state_snapshot_sha256(&cloned_state, &knowledge)?;
    let inserted = transaction
        .execute(
            "INSERT OR IGNORE INTO interaction_state
             (id, conversation_id, branch_id, revision, document_json, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                target_key.state_id,
                conversation_id.0.as_str(),
                target_key.branch_id.0.as_str(),
                i64_from_u64("cloned interaction state revision", cloned_state.revision)?,
                state_document_json,
                updated_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    if inserted == 1 {
        replace_normalized_state(
            transaction,
            &target_key.state_id,
            &cloned_state,
            &knowledge,
            updated_at,
        )?;
    }
    let durable = require_state_for_key(transaction, target_key)?;
    if durable.state != cloned_state {
        return Err(revision_conflict(
            "target branch already has a different interaction state",
        ));
    }
    let durable_knowledge = read_knowledge_bindings(transaction, &durable.id)?;
    if durable_knowledge != knowledge {
        return Err(revision_conflict(
            "target branch already has different interaction knowledge",
        ));
    }
    Ok(ClonedInteractionCheckpoint {
        source,
        cloned: StoredInteractionState {
            key: target_key.clone(),
            state: durable.state,
            knowledge: durable_knowledge,
        },
        checkpoint_sha256,
        cloned_state_document_sha256,
        cloned_state_snapshot_sha256,
    })
}
pub(super) fn read_generation_attempt_authority(
    connection: &Connection,
    generation_id: &GenerationId,
) -> CoreResult<GenerationAttemptAuthority> {
    connection
        .query_row(
            "SELECT revision, status, conversation_id, source_branch_id,
                    proposed_branch_id, context_head_message_id,
                    module_plan_sha256
             FROM generation_attempt_intents
             WHERE generation_id = ?1",
            [generation_id.0.as_str()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .map(
            |(
                revision,
                status,
                conversation_id,
                source_branch_id,
                proposed_branch_id,
                context_head_message_id,
                module_plan_sha256,
            )| {
                Ok(GenerationAttemptAuthority {
                    revision: u64_from_i64("generation attempt revision", revision)?,
                    status,
                    conversation_id: ConversationId(conversation_id),
                    source_branch_id: ConversationBranchId(source_branch_id),
                    proposed_branch_id: ConversationBranchId(proposed_branch_id),
                    context_head_message_id: context_head_message_id.map(MessageId),
                    module_plan_sha256,
                })
            },
        )
        .transpose()?
        .ok_or_else(|| not_found("generation attempt"))
}

fn read_pre_first_message_interaction_boundary(
    connection: &Connection,
    conversation_id: &ConversationId,
    source_branch_id: &ConversationBranchId,
) -> CoreResult<(InteractionState, Vec<InteractionKnowledgeBinding>, String)> {
    let historical = connection
        .query_row(
            "SELECT snapshot.previous_state_json,
                    snapshot.previous_knowledge_json,
                    snapshot.previous_state_snapshot_sha256,
                    snapshot.context_checkpoint_sha256
             FROM generations AS generation
             JOIN messages AS user_message
               ON user_message.id = generation.user_message_id
              AND user_message.conversation_id = generation.conversation_id
             JOIN generation_attempt_before_event_snapshots AS snapshot
               ON snapshot.generation_id = generation.id
              AND snapshot.context_head_message_id IS NULL
             WHERE generation.conversation_id = ?1
               AND generation.branch_id = ?2
               AND user_message.parent_id IS NULL
             ORDER BY generation.started_at, generation.id
             LIMIT 1",
            params![conversation_id.0.as_str(), source_branch_id.0.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            revision_conflict(
                "pre-first-message interaction boundary has no generation attempt snapshot",
            )
        })?;
    let state: InteractionState = decode_json(
        "historical pre-first-message interaction state",
        &historical.0,
        MAX_STATE_JSON_BYTES,
    )?;
    let knowledge: Vec<InteractionKnowledgeBinding> = decode_json(
        "historical pre-first-message interaction knowledge",
        &historical.1,
        MAX_STATE_JSON_BYTES,
    )?;
    validate_state(&state)?;
    validate_knowledge_bindings(&state, &knowledge)?;
    if encode_json(
        "historical pre-first-message interaction state",
        &state,
        MAX_STATE_JSON_BYTES,
    )? != historical.0
        || encode_json(
            "historical pre-first-message interaction knowledge",
            &knowledge,
            MAX_STATE_JSON_BYTES,
        )? != historical.1
        || interaction_state_snapshot_sha256(&state, &knowledge)? != historical.2
        || !is_sha256(&historical.3)
    {
        return Err(storage_corrupted(
            "historical pre-first-message interaction snapshot is invalid",
        ));
    }
    Ok((state, knowledge, historical.3))
}

pub(super) fn read_generation_attempt_review_boundary(
    connection: &Connection,
    authority: &GenerationAttemptAuthority,
) -> CoreResult<(InteractionState, Vec<InteractionKnowledgeBinding>, String)> {
    if authority.proposed_branch_id == authority.source_branch_id {
        let branch_head = connection
            .query_row(
                "SELECT head_message_id
                 FROM conversation_branches
                 WHERE conversation_id = ?1 AND id = ?2",
                params![
                    authority.conversation_id.0.as_str(),
                    authority.source_branch_id.0.as_str(),
                ],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("generation attempt source branch"))?;
        if branch_head.as_deref()
            != authority
                .context_head_message_id
                .as_ref()
                .map(|message_id| message_id.0.as_str())
        {
            return Err(revision_conflict(
                "same-branch generation attempt head advanced before BeforeGeneration review",
            ));
        }
        let row = read_state_row(
            connection,
            &authority.conversation_id,
            &authority.source_branch_id,
        )?
        .ok_or_else(|| not_found("generation attempt interaction state"))?;
        validate_normalized_state(connection, &row)?;
        let knowledge = read_knowledge_bindings(connection, &row.id)?;
        let checkpoint_sha256 = interaction_state_snapshot_sha256(&row.state, &knowledge)?;
        return Ok((row.state, knowledge, checkpoint_sha256));
    }

    if let Some(context_head_message_id) = &authority.context_head_message_id {
        let checkpoint = read_interaction_state_checkpoint(
            connection,
            &authority.conversation_id,
            &authority.source_branch_id,
            context_head_message_id,
        )?
        .ok_or_else(|| not_found("generation attempt interaction checkpoint"))?;
        if checkpoint
            .state
            .proposals
            .iter()
            .any(|proposal| proposal.status == InteractionProposalStatus::Pending)
        {
            return Err(revision_conflict(
                "cannot stage generation from a checkpoint with a pending proposal",
            ));
        }
        let mut state = checkpoint.state;
        state.proposals.clear();
        validate_state(&state)?;
        validate_knowledge_bindings(&state, &checkpoint.knowledge)?;
        return Ok((state, checkpoint.knowledge, checkpoint.checkpoint_sha256));
    }

    let source_head = connection
        .query_row(
            "SELECT head_message_id
             FROM conversation_branches
             WHERE conversation_id = ?1 AND id = ?2",
            params![
                authority.conversation_id.0.as_str(),
                authority.source_branch_id.0.as_str(),
            ],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("generation attempt source branch"))?;
    if source_head.is_none() {
        let row = read_state_row(
            connection,
            &authority.conversation_id,
            &authority.source_branch_id,
        )?
        .ok_or_else(|| not_found("generation attempt interaction state"))?;
        validate_normalized_state(connection, &row)?;
        let knowledge = read_knowledge_bindings(connection, &row.id)?;
        let checkpoint_sha256 = interaction_state_snapshot_sha256(&row.state, &knowledge)?;
        return Ok((row.state, knowledge, checkpoint_sha256));
    }

    read_pre_first_message_interaction_boundary(
        connection,
        &authority.conversation_id,
        &authority.source_branch_id,
    )
}
