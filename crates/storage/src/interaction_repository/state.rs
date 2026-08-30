use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use lorepia_domain::{
    ConversationBranchId, ConversationId, CoreError, CoreErrorCode, CoreResult,
    InteractionProposalRecordId, InteractionState, KnowledgeEntryId, ValidateOrchestration,
    VariableRef, VariableScope, VariableValue,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::Storage;

use super::{
    InteractionKnowledgeBinding, InteractionProposalWrite, InteractionStateKey,
    MAX_AUDIT_JSON_BYTES, MAX_EVENT_JSON_BYTES, MAX_STATE_JSON_BYTES, StoredInteractionState,
    decode_json, encode_json, i64_from_u64, is_sha256, not_found, read_proposal, revision_conflict,
    sha256_hex, storage_corrupted, storage_db_error, u64_from_i64,
};

impl Storage {
    /// Returns the durable state for a conversation branch.
    pub fn get_interaction_state(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
    ) -> CoreResult<InteractionState> {
        self.get_interaction_state_snapshot(conversation_id, branch_id)
            .map(|snapshot| snapshot.state)
    }

    /// Returns state and its revision-pinned normalized knowledge projection
    /// from one consistent read.
    pub fn get_interaction_state_snapshot(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
    ) -> CoreResult<StoredInteractionState> {
        let connection = self.connection()?;
        let row = read_state_row(&connection, conversation_id, branch_id)?
            .ok_or_else(|| not_found("interaction state"))?;
        validate_normalized_state(&connection, &row)?;
        let state = decode_state_row(&row)?;
        let knowledge = read_knowledge_bindings(&connection, &row.id)?;
        Ok(StoredInteractionState {
            key: InteractionStateKey {
                state_id: row.id,
                conversation_id: row.conversation_id,
                branch_id: row.branch_id,
            },
            state,
            knowledge,
        })
    }
    /// Creates revision zero exactly once, or returns the already initialized
    /// state for the same key. A reused state ID or branch key is rejected.
    pub fn get_or_init_interaction_state(
        &self,
        key: &InteractionStateKey,
        initial_state: &InteractionState,
        knowledge: &[InteractionKnowledgeBinding],
        updated_at: DateTime<Utc>,
    ) -> CoreResult<InteractionState> {
        validate_key(key)?;
        validate_state(initial_state)?;
        if initial_state.revision != 0 {
            return Err(CoreError::invalid(
                "initial interaction state revision must be zero",
            ));
        }
        if !initial_state.proposals.is_empty() {
            return Err(CoreError::invalid(
                "initial interaction state must not contain proposals",
            ));
        }
        validate_knowledge_bindings(initial_state, knowledge)?;

        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;

        if let Some(existing) = read_state_row(&transaction, &key.conversation_id, &key.branch_id)?
        {
            if existing.id != key.state_id {
                return Err(revision_conflict(
                    "interaction state branch is already initialized under another state id",
                ));
            }
            validate_normalized_state(&transaction, &existing)?;
            let state = decode_state_row(&existing)?;
            transaction.commit().map_err(storage_db_error)?;
            return Ok(state);
        }

        let reused_key = transaction
            .query_row(
                "SELECT conversation_id, branch_id
                 FROM interaction_state
                 WHERE id = ?1",
                [&key.state_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(storage_db_error)?;
        if reused_key.is_some() {
            return Err(revision_conflict(
                "interaction state id is already bound to another branch",
            ));
        }

        let state_json = encode_json("interaction state", initial_state, MAX_STATE_JSON_BYTES)?;
        transaction
            .execute(
                "INSERT INTO interaction_state
                 (id, conversation_id, branch_id, revision, document_json, updated_at)
                 VALUES (?1, ?2, ?3, 0, ?4, ?5)",
                params![
                    key.state_id,
                    key.conversation_id.0.as_str(),
                    key.branch_id.0.as_str(),
                    state_json,
                    updated_at.to_rfc3339(),
                ],
            )
            .map_err(storage_db_error)?;
        replace_normalized_state(
            &transaction,
            &key.state_id,
            initial_state,
            knowledge,
            updated_at,
        )?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(initial_state.clone())
    }
}

pub(super) struct StateRow {
    pub(super) id: String,
    pub(super) conversation_id: ConversationId,
    pub(super) branch_id: ConversationBranchId,
    pub(super) revision: u64,
    pub(super) document_json: String,
    pub(super) state: InteractionState,
}

pub(super) fn read_state_row(
    connection: &Connection,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
) -> CoreResult<Option<StateRow>> {
    let raw = connection
        .query_row(
            "SELECT id, conversation_id, branch_id, revision, document_json
             FROM interaction_state
             WHERE conversation_id = ?1 AND branch_id = ?2",
            params![conversation_id.0.as_str(), branch_id.0.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?;
    raw.map(|(id, conversation, branch, revision, document)| {
        decode_raw_state_row(id, conversation, branch, revision, document)
    })
    .transpose()
}

pub(super) fn read_state_by_id(connection: &Connection, id: &str) -> CoreResult<Option<StateRow>> {
    let raw = connection
        .query_row(
            "SELECT id, conversation_id, branch_id, revision, document_json
             FROM interaction_state
             WHERE id = ?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?;
    raw.map(|(id, conversation, branch, revision, document)| {
        decode_raw_state_row(id, conversation, branch, revision, document)
    })
    .transpose()
}

fn decode_raw_state_row(
    id: String,
    conversation_id: String,
    branch_id: String,
    revision: i64,
    document_json: String,
) -> CoreResult<StateRow> {
    let revision = u64_from_i64("interaction state revision", revision)?;
    let state: InteractionState = decode_json(
        "stored interaction state",
        &document_json,
        MAX_STATE_JSON_BYTES,
    )?;
    if state.revision != revision {
        return Err(storage_corrupted(
            "interaction state document revision differs from its row revision",
        ));
    }
    validate_state(&state).map_err(|error| {
        storage_corrupted(format!("stored interaction state is invalid: {error}"))
    })?;
    Ok(StateRow {
        id,
        conversation_id: ConversationId(conversation_id),
        branch_id: ConversationBranchId(branch_id),
        revision,
        document_json,
        state,
    })
}

fn decode_state_row(row: &StateRow) -> CoreResult<InteractionState> {
    let decoded: InteractionState = decode_json(
        "stored interaction state",
        &row.document_json,
        MAX_STATE_JSON_BYTES,
    )?;
    if decoded.revision != row.revision {
        return Err(storage_corrupted(
            "interaction state document revision differs from its row revision",
        ));
    }
    Ok(decoded)
}

pub(super) fn validate_normalized_state(connection: &Connection, row: &StateRow) -> CoreResult<()> {
    let expected_variables = row
        .state
        .variables
        .values
        .iter()
        .map(|binding| {
            let (scope, namespace) = persistent_variable_scope(&binding.variable)?;
            let value_json = encode_json(
                "interaction variable value",
                &binding.value,
                MAX_AUDIT_JSON_BYTES,
            )?;
            Ok((
                (
                    scope.to_owned(),
                    namespace,
                    binding.variable.id.as_str().to_owned(),
                ),
                (variable_value_type(&binding.value).to_owned(), value_json),
            ))
        })
        .collect::<CoreResult<BTreeMap<_, _>>>()?;
    let stored_variables = {
        let mut statement = connection
            .prepare(
                "SELECT scope, namespace, variable_id, value_type,
                        value_json, state_revision
                 FROM interaction_state_variables
                 WHERE interaction_state_id = ?1
                 ORDER BY scope, namespace, variable_id",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map([&row.id], |sql_row| {
                Ok((
                    sql_row.get::<_, String>(0)?,
                    sql_row.get::<_, String>(1)?,
                    sql_row.get::<_, String>(2)?,
                    sql_row.get::<_, String>(3)?,
                    sql_row.get::<_, String>(4)?,
                    sql_row.get::<_, i64>(5)?,
                ))
            })
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?;
        let mut normalized = BTreeMap::new();
        for (scope, namespace, id, value_type, value_json, revision) in rows {
            if u64_from_i64("normalized interaction variable revision", revision)? != row.revision {
                return Err(storage_corrupted(
                    "normalized interaction variable has a stale state revision",
                ));
            }
            let _: VariableValue = decode_json(
                "normalized interaction variable value",
                &value_json,
                MAX_AUDIT_JSON_BYTES,
            )?;
            if normalized
                .insert((scope, namespace, id), (value_type, value_json))
                .is_some()
            {
                return Err(storage_corrupted(
                    "normalized interaction variables contain a duplicate key",
                ));
            }
        }
        normalized
    };
    if expected_variables != stored_variables {
        return Err(storage_corrupted(
            "normalized interaction variables differ from the state document",
        ));
    }

    let stored_knowledge = {
        let mut statement = connection
            .prepare(
                "SELECT book_revision_id, entry_id, enabled, state_revision
                 FROM interaction_state_knowledge
                 WHERE interaction_state_id = ?1
                 ORDER BY book_revision_id, entry_id",
            )
            .map_err(storage_db_error)?;
        statement
            .query_map([&row.id], |sql_row| {
                Ok((
                    sql_row.get::<_, String>(0)?,
                    sql_row.get::<_, String>(1)?,
                    sql_row.get::<_, bool>(2)?,
                    sql_row.get::<_, i64>(3)?,
                ))
            })
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?
    };
    let mut stored_entry_ids = BTreeSet::new();
    for (book_revision, entry_id, enabled, revision) in stored_knowledge {
        if book_revision.trim().is_empty()
            || !enabled
            || u64_from_i64("normalized interaction knowledge revision", revision)? != row.revision
            || !stored_entry_ids.insert(entry_id)
        {
            return Err(storage_corrupted(
                "normalized interaction knowledge is invalid or ambiguous",
            ));
        }
    }
    let expected_entry_ids = row
        .state
        .manually_active_knowledge
        .iter()
        .map(|entry| entry.as_str().to_owned())
        .collect::<BTreeSet<_>>();
    if stored_entry_ids != expected_entry_ids {
        return Err(storage_corrupted(
            "normalized interaction knowledge differs from the state document",
        ));
    }

    let proposal_ids = {
        let mut statement = connection
            .prepare(
                "SELECT id FROM interaction_proposals
                 WHERE interaction_state_id = ?1
                 ORDER BY id",
            )
            .map_err(storage_db_error)?;
        statement
            .query_map([&row.id], |sql_row| sql_row.get::<_, String>(0))
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?
    };
    let state_proposals = row
        .state
        .proposals
        .iter()
        .map(|proposal| (proposal.id.as_str(), proposal))
        .collect::<BTreeMap<_, _>>();
    if state_proposals.len() != row.state.proposals.len()
        || proposal_ids.len() != state_proposals.len()
    {
        return Err(storage_corrupted(
            "normalized interaction proposals differ from the state document",
        ));
    }
    for id in proposal_ids {
        let proposal =
            read_proposal(connection, &InteractionProposalRecordId::from(id.clone()))?
                .ok_or_else(|| storage_corrupted("normalized interaction proposal is missing"))?;
        if state_proposals.get(id.as_str()).copied() != Some(&proposal.record) {
            return Err(storage_corrupted(
                "normalized interaction proposal differs from the state document",
            ));
        }
    }
    Ok(())
}

pub(super) fn read_knowledge_bindings(
    connection: &Connection,
    state_id: &str,
) -> CoreResult<Vec<InteractionKnowledgeBinding>> {
    let mut statement = connection
        .prepare(
            "SELECT book_revision_id, entry_id
             FROM interaction_state_knowledge
             WHERE interaction_state_id = ?1 AND enabled = 1
             ORDER BY book_revision_id, entry_id",
        )
        .map_err(storage_db_error)?;
    statement
        .query_map([state_id], |row| {
            Ok(InteractionKnowledgeBinding {
                book_revision_id: row.get(0)?,
                entry_id: KnowledgeEntryId::from(row.get::<_, String>(1)?),
            })
        })
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)
}
pub(super) fn require_state_for_key(
    transaction: &Transaction<'_>,
    key: &InteractionStateKey,
) -> CoreResult<StateRow> {
    validate_key(key)?;
    let current = read_state_row(transaction, &key.conversation_id, &key.branch_id)?
        .ok_or_else(|| not_found("interaction state"))?;
    if current.id != key.state_id {
        return Err(revision_conflict(
            "interaction state key does not match its durable branch row",
        ));
    }
    validate_normalized_state(transaction, &current)?;
    Ok(current)
}

pub(super) fn require_state_revision(current: &StateRow, expected: u64) -> CoreResult<()> {
    if current.revision != expected {
        return Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            format!(
                "interaction state revision conflict: expected {expected}, current {}",
                current.revision
            ),
            true,
        ));
    }
    Ok(())
}
pub(super) fn write_state_document_only(
    transaction: &Transaction<'_>,
    state_id: &str,
    expected_revision: u64,
    next_state: &InteractionState,
    updated_at: DateTime<Utc>,
) -> CoreResult<()> {
    let next_revision = expected_revision
        .checked_add(1)
        .ok_or_else(|| CoreError::invalid("interaction state revision overflowed"))?;
    if next_state.revision != next_revision {
        return Err(CoreError::invalid(format!(
            "interaction state transition must advance to revision {next_revision}"
        )));
    }
    let document_json = encode_json("interaction state", next_state, MAX_STATE_JSON_BYTES)?;
    let changed = transaction
        .execute(
            "UPDATE interaction_state
             SET revision = ?1, document_json = ?2, updated_at = ?3
             WHERE id = ?4 AND revision = ?5",
            params![
                i64_from_u64("interaction state revision", next_state.revision)?,
                document_json,
                updated_at.to_rfc3339(),
                state_id,
                i64_from_u64("expected interaction state revision", expected_revision)?,
            ],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(revision_conflict(
            "interaction state compare-and-swap failed",
        ));
    }
    Ok(())
}

pub(super) fn replace_normalized_state(
    transaction: &Transaction<'_>,
    state_id: &str,
    state: &InteractionState,
    knowledge: &[InteractionKnowledgeBinding],
    updated_at: DateTime<Utc>,
) -> CoreResult<()> {
    transaction
        .execute(
            "DELETE FROM interaction_state_variables
             WHERE interaction_state_id = ?1",
            [state_id],
        )
        .map_err(storage_db_error)?;
    for binding in &state.variables.values {
        let (scope, namespace) = persistent_variable_scope(&binding.variable)?;
        let value_json = encode_json(
            "interaction variable value",
            &binding.value,
            MAX_AUDIT_JSON_BYTES,
        )?;
        transaction
            .execute(
                "INSERT INTO interaction_state_variables
                 (interaction_state_id, scope, namespace, variable_id,
                  value_type, value_json, state_revision, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    state_id,
                    scope,
                    namespace,
                    binding.variable.id.as_str(),
                    variable_value_type(&binding.value),
                    value_json,
                    i64_from_u64("interaction state revision", state.revision)?,
                    updated_at.to_rfc3339(),
                ],
            )
            .map_err(storage_db_error)?;
    }

    transaction
        .execute(
            "DELETE FROM interaction_state_knowledge
             WHERE interaction_state_id = ?1",
            [state_id],
        )
        .map_err(storage_db_error)?;
    for binding in knowledge {
        transaction
            .execute(
                "INSERT INTO interaction_state_knowledge
                 (interaction_state_id, book_revision_id, entry_id,
                  enabled, state_revision)
                 VALUES (?1, ?2, ?3, 1, ?4)",
                params![
                    state_id,
                    binding.book_revision_id,
                    binding.entry_id.as_str(),
                    i64_from_u64("interaction state revision", state.revision)?,
                ],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

pub(super) fn bump_normalized_state_revisions(
    transaction: &Transaction<'_>,
    state_id: &str,
    revision: u64,
) -> CoreResult<()> {
    let revision = i64_from_u64("interaction state revision", revision)?;
    transaction
        .execute(
            "UPDATE interaction_state_variables
             SET state_revision = ?1
             WHERE interaction_state_id = ?2",
            params![revision, state_id],
        )
        .map_err(storage_db_error)?;
    transaction
        .execute(
            "UPDATE interaction_state_knowledge
             SET state_revision = ?1
             WHERE interaction_state_id = ?2",
            params![revision, state_id],
        )
        .map_err(storage_db_error)?;
    Ok(())
}
pub(super) fn validate_state(state: &InteractionState) -> CoreResult<()> {
    state
        .validate()
        .map_err(|error| CoreError::invalid(error.to_string()))?;
    for binding in &state.variables.values {
        persistent_variable_scope(&binding.variable)?;
    }
    Ok(())
}

pub(super) fn validate_key(key: &InteractionStateKey) -> CoreResult<()> {
    validate_nonempty_id("interaction state id", &key.state_id)?;
    validate_nonempty_id("conversation id", key.conversation_id.0.as_str())?;
    validate_nonempty_id("conversation branch id", key.branch_id.0.as_str())
}

pub(super) fn validate_nonempty_id(label: &str, value: &str) -> CoreResult<()> {
    if value.trim().is_empty() || value.len() > 1_024 {
        return Err(CoreError::invalid(format!(
            "{label} must be non-empty and at most 1024 bytes"
        )));
    }
    Ok(())
}

pub(super) fn validate_knowledge_bindings(
    state: &InteractionState,
    bindings: &[InteractionKnowledgeBinding],
) -> CoreResult<()> {
    let state_entries = state
        .manually_active_knowledge
        .iter()
        .map(KnowledgeEntryId::as_str)
        .collect::<BTreeSet<_>>();
    let bound_entries = bindings
        .iter()
        .map(|binding| binding.entry_id.as_str())
        .collect::<BTreeSet<_>>();
    if state_entries.len() != state.manually_active_knowledge.len()
        || bound_entries.len() != bindings.len()
        || state_entries != bound_entries
    {
        return Err(CoreError::invalid(
            "interaction knowledge bindings must map every active entry exactly once",
        ));
    }
    for binding in bindings {
        validate_nonempty_id("knowledge book revision id", &binding.book_revision_id)?;
    }
    Ok(())
}

pub(super) fn validate_review_hash(proposal: &InteractionProposalWrite) -> CoreResult<()> {
    if !is_sha256(&proposal.review_payload_sha256) {
        return Err(CoreError::invalid(
            "interaction proposal review hash must be lowercase SHA-256",
        ));
    }
    let payload_json = encode_json(
        "interaction proposal",
        &proposal.record,
        MAX_EVENT_JSON_BYTES,
    )?;
    if sha256_hex(payload_json.as_bytes()) != proposal.review_payload_sha256 {
        return Err(CoreError::invalid(
            "interaction proposal review hash does not match its canonical record",
        ));
    }
    Ok(())
}

fn persistent_variable_scope(variable: &VariableRef) -> CoreResult<(&'static str, String)> {
    match (&variable.scope, &variable.namespace) {
        (VariableScope::App, None) => Ok(("app", String::new())),
        (VariableScope::User, None) => Ok(("user", String::new())),
        (VariableScope::Persona, None) => Ok(("persona", String::new())),
        (VariableScope::Character, None) => Ok(("character", String::new())),
        (VariableScope::Conversation, None) => Ok(("conversation", String::new())),
        (VariableScope::Branch, None) => Ok(("branch", String::new())),
        (VariableScope::Module, Some(namespace)) => {
            let prefix = format!("{}.", namespace.as_str());
            if !variable.id.as_str().starts_with(&prefix) {
                return Err(CoreError::invalid(
                    "module variable id must begin with its namespace",
                ));
            }
            Ok(("module", namespace.as_str().to_owned()))
        }
        (VariableScope::Module, None) => Err(CoreError::invalid(
            "module interaction variables require a namespace",
        )),
        (VariableScope::Session | VariableScope::Turn, _) => Err(CoreError::invalid(
            "session and turn variables cannot be persisted as interaction state",
        )),
        (_, Some(_)) => Err(CoreError::invalid(
            "only module interaction variables may have a namespace",
        )),
    }
}

fn variable_value_type(value: &VariableValue) -> &'static str {
    match value {
        VariableValue::Bool(_) => "bool",
        VariableValue::Integer(_) => "integer",
        VariableValue::Decimal(_) => "decimal",
        VariableValue::Text(_) => "text",
        VariableValue::Enum(_) => "enum",
        VariableValue::StringList(_) => "string_list",
    }
}
