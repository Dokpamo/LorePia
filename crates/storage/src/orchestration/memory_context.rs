//! Exact-at-head memory and sealed prompt-context authority.

use super::{
    ConversationBranchId, ConversationId, CoreError, CoreResult, GenerationPromptPlanRecord,
    MAX_MEMORY_RECORDS, MemoryRecordAtHeadEvidence, MemoryRecordsAtHeadSelection,
    MemoryRecordsAtHeadSnapshot, MessageId, OptionalExtension, PromptContextBindingEvidence,
    PromptContextPersonaEvidence, PromptContextSnapshotV1, ResolvedPromptPlan, Serialize, Storage,
    Transaction, ValidateOrchestration, memory_records_at_head_in_connection, params,
    prompt_context_changed, prompt_context_snapshot_sha256, sha256_hex, storage_corrupted,
    storage_db_error,
};

#[cfg(test)]
use super::{
    HashMap, LocalUserId, MemoryRecord, MemoryRecordId, PromptPresetBinding, PromptPresetId,
    PromptSummarySourceEvidence, StoredRevision, not_found, prompt_binding_targets_for_test,
    prompt_local_user_id_sha256, u64_revision, validate_prompt_binding_context_for_test,
};

impl Storage {
    pub fn list_memory_records_at_head(
        &self,
        conversation_id: &ConversationId,
        source_branch_id: &ConversationBranchId,
        context_head_message_id: Option<&MessageId>,
        include_invalidated: bool,
    ) -> CoreResult<MemoryRecordsAtHeadSelection> {
        let connection = self.connection()?;
        memory_records_at_head_in_connection(
            &connection,
            conversation_id,
            source_branch_id,
            context_head_message_id,
            include_invalidated,
        )
    }
}

#[derive(Serialize)]
struct MemoryRecordsAtHeadSnapshotDigest<'a> {
    schema_version: u32,
    conversation_id: &'a ConversationId,
    source_branch_id: &'a ConversationBranchId,
    context_head_message_id: Option<&'a MessageId>,
    include_invalidated: bool,
    records: &'a [MemoryRecordAtHeadEvidence],
}

pub fn memory_records_at_head_snapshot_sha256(
    snapshot: &MemoryRecordsAtHeadSnapshot,
) -> CoreResult<String> {
    if snapshot.schema_version != 1 || snapshot.records.len() > MAX_MEMORY_RECORDS {
        return Err(CoreError::invalid(
            "memory head snapshot schema or record count is invalid",
        ));
    }
    let json = serde_json::to_string(&MemoryRecordsAtHeadSnapshotDigest {
        schema_version: snapshot.schema_version,
        conversation_id: &snapshot.conversation_id,
        source_branch_id: &snapshot.source_branch_id,
        context_head_message_id: snapshot.context_head_message_id.as_ref(),
        include_invalidated: snapshot.include_invalidated,
        records: &snapshot.records,
    })
    .map_err(|error| CoreError::internal(format!("cannot encode memory head snapshot: {error}")))?;
    if json.len() > 8 * 1_024 * 1_024 {
        return Err(CoreError::invalid(
            "memory head snapshot exceeds its byte limit",
        ));
    }
    Ok(sha256_hex(json.as_bytes()))
}

pub(crate) fn require_memory_records_at_head_snapshot_transaction(
    transaction: &Transaction<'_>,
    expected: &MemoryRecordsAtHeadSnapshot,
) -> CoreResult<()> {
    if memory_records_at_head_snapshot_sha256(expected)? != expected.snapshot_sha256 {
        return Err(CoreError::invalid(
            "memory head snapshot fingerprint is invalid",
        ));
    }
    let current = memory_records_at_head_in_connection(
        transaction,
        &expected.conversation_id,
        &expected.source_branch_id,
        expected.context_head_message_id.as_ref(),
        expected.include_invalidated,
    )?;
    if current.snapshot != *expected {
        return Err(CoreError::invalid(
            "memory records changed after generation preparation",
        ));
    }
    Ok(())
}

/// Rechecks every mutable prompt-context authority in the same transaction
/// that makes an attempt-bound generation visible.
///
/// Prompt text remains sealed only in the immutable resolved plan. This gate
/// compares the content-free source identities captured by Core so a room
/// binding, persona selection, summary revision, or local identity cannot
/// drift between preparation and dispatch.
#[cfg(test)]
pub(crate) fn require_generation_prompt_context_snapshot_transaction(
    transaction: &Transaction<'_>,
    record: &GenerationPromptPlanRecord,
    expected_source_branch_id: &ConversationBranchId,
    expected_context_head_message_id: Option<&MessageId>,
    local_user_id: &LocalUserId,
) -> CoreResult<()> {
    let resolved: ResolvedPromptPlan = serde_json::from_value(record.plan.value.clone())
        .map_err(|error| CoreError::invalid(format!("resolved prompt plan is invalid: {error}")))?;
    resolved
        .validate()
        .map_err(|error| CoreError::invalid(format!("resolved prompt plan is invalid: {error}")))?;
    let expected = resolved.trace.context_snapshot.as_ref().ok_or_else(|| {
        CoreError::invalid("attempt-bound prompt plan is missing its context snapshot")
    })?;
    require_prompt_context_snapshot_identity(
        expected,
        record,
        expected_source_branch_id,
        expected_context_head_message_id,
        local_user_id,
    )?;
    let persona_id = require_prompt_context_persona(transaction, expected)?;
    require_prompt_context_binding(transaction, record, expected, persona_id.as_deref())?;
    require_prompt_context_summaries(transaction, expected)
}

/// Validates an attempt-bound prompt against the immutable authority captured
/// before its approval pause. Mutable binding, persona-selection, settings,
/// and memory heads are deliberately not re-read here; their exact identities
/// are carried by the attempt and its `BeforeGeneration` snapshot.
pub(crate) struct SealedGenerationPromptContext<'a> {
    pub(crate) conversation_id: &'a ConversationId,
    pub(crate) target_branch_id: &'a ConversationBranchId,
    pub(crate) source_branch_id: &'a ConversationBranchId,
    pub(crate) context_head_message_id: Option<&'a MessageId>,
    pub(crate) authority: &'a crate::generation_attempt::GenerationPromptSelectionAuthority,
    pub(crate) memory_snapshot: &'a MemoryRecordsAtHeadSnapshot,
}

pub(crate) fn require_sealed_generation_prompt_context_snapshot_transaction(
    transaction: &Transaction<'_>,
    record: &GenerationPromptPlanRecord,
    context: SealedGenerationPromptContext<'_>,
) -> CoreResult<()> {
    let expected = sealed_prompt_context_snapshot(record)?;
    require_sealed_prompt_context_identity(record, &expected, &context)?;
    let sealed_binding = sealed_prompt_binding(context.authority)?;
    if expected.binding != sealed_binding {
        return Err(prompt_context_changed(
            "prompt binding differs from its sealed generation authority",
        ));
    }
    let sealed_persona = sealed_prompt_persona(transaction, context.authority)?;
    if expected.persona != sealed_persona {
        return Err(prompt_context_changed(
            "prompt persona differs from its sealed generation authority",
        ));
    }
    require_sealed_prompt_memory_context(&expected, &context)
}

fn sealed_prompt_context_snapshot(
    record: &GenerationPromptPlanRecord,
) -> CoreResult<PromptContextSnapshotV1> {
    let resolved: ResolvedPromptPlan = serde_json::from_value(record.plan.value.clone())
        .map_err(|error| CoreError::invalid(format!("resolved prompt plan is invalid: {error}")))?;
    resolved
        .validate()
        .map_err(|error| CoreError::invalid(format!("resolved prompt plan is invalid: {error}")))?;
    resolved.trace.context_snapshot.ok_or_else(|| {
        CoreError::invalid("attempt-bound prompt plan is missing its context snapshot")
    })
}

fn require_sealed_prompt_context_identity(
    record: &GenerationPromptPlanRecord,
    expected: &PromptContextSnapshotV1,
    context: &SealedGenerationPromptContext<'_>,
) -> CoreResult<()> {
    if record.conversation_id != *context.conversation_id
        || record.branch_id != *context.target_branch_id
        || record.head_message_id.as_ref() != context.context_head_message_id
        || record.prompt_preset_id != context.authority.preset.id
        || record.prompt_preset_revision_id != context.authority.preset_revision_id
        || expected.schema_version != 1
        || expected.conversation_id != *context.conversation_id
        || expected.source_branch_id != *context.source_branch_id
        || expected.context_head_message_id.as_ref() != context.context_head_message_id
        || expected.local_user_id_sha256 != context.authority.local_user_id_sha256
        || prompt_context_snapshot_sha256(expected).map_err(|error| {
            CoreError::invalid(format!("prompt context snapshot is invalid: {error}"))
        })? != expected.snapshot_sha256
    {
        return Err(prompt_context_changed(
            "sealed prompt context identity differs from its generation attempt",
        ));
    }
    Ok(())
}

fn sealed_prompt_binding(
    authority: &crate::generation_attempt::GenerationPromptSelectionAuthority,
) -> CoreResult<Option<PromptContextBindingEvidence>> {
    authority
        .binding
        .as_ref()
        .map(|binding| {
            Ok(PromptContextBindingEvidence {
                binding_id: binding.value.id.clone(),
                binding_revision: binding.revision,
                document_sha256: binding.value.canonical_document_sha256()?,
            })
        })
        .transpose()
}

fn sealed_prompt_persona(
    transaction: &Transaction<'_>,
    authority: &crate::generation_attempt::GenerationPromptSelectionAuthority,
) -> CoreResult<Option<PromptContextPersonaEvidence>> {
    authority
        .persona_selection
        .as_ref()
        .map(|selection| {
            let revision_id = selection.revision_id.as_deref().ok_or_else(|| {
                storage_corrupted("sealed prompt persona selection has no revision identity")
            })?;
            let document_sha256 = transaction
                .query_row(
                    "SELECT document_sha256
                     FROM content_revisions
                     WHERE object_id = ?1 AND id = ?2 AND object_kind = 'persona'",
                    params![selection.value.persona_id.as_str(), revision_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(storage_db_error)?
                .ok_or_else(|| storage_corrupted("sealed prompt persona revision is missing"))?;
            Ok(PromptContextPersonaEvidence {
                selection_revision: selection.revision,
                persona_id: selection.value.persona_id.clone(),
                persona_revision_id: revision_id.to_owned(),
                persona_sha256: document_sha256,
            })
        })
        .transpose()
}

fn require_sealed_prompt_memory_context(
    expected: &PromptContextSnapshotV1,
    context: &SealedGenerationPromptContext<'_>,
) -> CoreResult<()> {
    let memory_snapshot = context.memory_snapshot;
    if memory_snapshot.conversation_id != *context.conversation_id
        || memory_snapshot.source_branch_id != *context.source_branch_id
        || memory_snapshot.context_head_message_id.as_ref() != context.context_head_message_id
        || memory_snapshot.include_invalidated
        || memory_records_at_head_snapshot_sha256(memory_snapshot)?
            != memory_snapshot.snapshot_sha256
    {
        return Err(storage_corrupted(
            "generation memory snapshot differs from its sealed prompt boundary",
        ));
    }
    for summary in &expected.summaries {
        let matches = memory_snapshot.records.iter().any(|record| {
            summary.summary_id == record.record_id
                && summary.record_branch_id == record.record_branch_id
                && summary.source_start_message_id == record.source_start_message_id
                && summary.source_end_message_id == record.source_end_message_id
                && summary.state_revision == record.state_revision
                && summary.active_revision_id == record.active_revision_id
                && summary.active_revision_sha256 == record.active_revision_sha256
        });
        if !matches {
            return Err(prompt_context_changed(
                "prompt summary differs from its sealed memory snapshot",
            ));
        }
    }
    if expected.conversation_summary_id.as_ref().is_some_and(|id| {
        !expected
            .summaries
            .iter()
            .any(|summary| &summary.summary_id == id)
    }) {
        return Err(storage_corrupted(
            "sealed prompt conversation summary has no exact revision evidence",
        ));
    }
    Ok(())
}

#[cfg(test)]
fn require_prompt_context_snapshot_identity(
    expected: &PromptContextSnapshotV1,
    record: &GenerationPromptPlanRecord,
    expected_source_branch_id: &ConversationBranchId,
    expected_context_head_message_id: Option<&MessageId>,
    local_user_id: &LocalUserId,
) -> CoreResult<()> {
    if expected.schema_version != 1
        || expected.conversation_id != record.conversation_id
        || expected.source_branch_id != *expected_source_branch_id
        || expected.context_head_message_id.as_ref() != expected_context_head_message_id
        || record.head_message_id.as_ref() != expected_context_head_message_id
    {
        return Err(prompt_context_changed(
            "prompt context boundary changed after generation preparation",
        ));
    }
    let fingerprint = prompt_context_snapshot_sha256(expected).map_err(|error| {
        CoreError::invalid(format!("prompt context snapshot is invalid: {error}"))
    })?;
    if fingerprint != expected.snapshot_sha256
        || expected.local_user_id_sha256 != prompt_local_user_id_sha256(local_user_id)
    {
        return Err(prompt_context_changed(
            "prompt context identity changed after generation preparation",
        ));
    }
    Ok(())
}

#[cfg(test)]
fn require_prompt_context_persona(
    transaction: &Transaction<'_>,
    expected: &PromptContextSnapshotV1,
) -> CoreResult<Option<String>> {
    let current = transaction
        .query_row(
            "SELECT selection.persona_id, selection.persona_revision_id,
                    selection.revision, revision.document_sha256
             FROM conversation_persona_selections AS selection
             JOIN content_objects AS object
               ON object.id = selection.persona_id
              AND object.object_kind = 'persona'
              AND object.deleted_at IS NULL
             JOIN content_revisions AS revision
               ON revision.object_id = selection.persona_id
              AND revision.id = selection.persona_revision_id
              AND revision.object_kind = 'persona'
             WHERE selection.conversation_id = ?1
               AND selection.deleted_at IS NULL",
            [&expected.conversation_id.0],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?;
    match (&expected.persona, current) {
        (None, None) => Ok(None),
        (Some(expected), Some((persona_id, revision_id, revision, sha256)))
            if expected.persona_id.as_str() == persona_id
                && expected.persona_revision_id == revision_id
                && expected.selection_revision == u64_revision(revision)?
                && expected.persona_sha256 == sha256 =>
        {
            Ok(Some(persona_id))
        }
        _ => Err(prompt_context_changed(
            "prompt persona selection changed after generation preparation",
        )),
    }
}

#[cfg(test)]
#[derive(Debug)]
struct CurrentPromptBinding {
    evidence: PromptContextBindingEvidence,
    prompt_preset_id: PromptPresetId,
}

#[cfg(test)]
fn require_prompt_context_binding(
    transaction: &Transaction<'_>,
    record: &GenerationPromptPlanRecord,
    expected: &PromptContextSnapshotV1,
    persona_id: Option<&str>,
) -> CoreResult<()> {
    let character_id = transaction
        .query_row(
            "SELECT character_id FROM conversations WHERE id = ?1",
            [&record.conversation_id.0],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("prompt context conversation"))?;
    let scopes = [
        ("branch", Some(record.branch_id.0.as_str())),
        ("conversation", Some(record.conversation_id.0.as_str())),
        ("character", Some(character_id.as_str())),
        ("persona", persona_id),
        ("user", None),
        ("app", None),
    ];
    let mut current = None;
    for (scope_kind, target_id) in scopes {
        if scope_kind == "persona" && target_id.is_none() {
            continue;
        }
        if let Some(binding) = prompt_context_binding_at_scope(
            transaction,
            scope_kind,
            target_id,
            &record.conversation_id,
        )? {
            current = (binding.prompt_preset_id == record.prompt_preset_id).then_some(binding);
            break;
        }
    }
    if current.as_ref().map(|binding| &binding.evidence) != expected.binding.as_ref() {
        return Err(prompt_context_changed(
            "prompt binding changed after generation preparation",
        ));
    }
    Ok(())
}

#[cfg(test)]
fn prompt_context_binding_at_scope(
    transaction: &Transaction<'_>,
    scope_kind: &str,
    target_id: Option<&str>,
    conversation_id: &ConversationId,
) -> CoreResult<Option<CurrentPromptBinding>> {
    let target_clause = match scope_kind {
        "branch" => "branch_id = ?2",
        "conversation" => "conversation_id = ?2",
        "character" => "character_id = ?2",
        "persona" => "persona_id = ?2",
        "user" | "app" => "1 = 1",
        _ => return Err(CoreError::internal("unsupported prompt binding scope")),
    };
    let sql = format!(
        "SELECT id, revision, prompt_preset_id, document_json
         FROM prompt_preset_bindings
         WHERE scope_kind = ?1 AND {target_clause}
           AND enabled = 1 AND deleted_at IS NULL
         ORDER BY priority DESC, id"
    );
    let mut statement = transaction.prepare(&sql).map_err(storage_db_error)?;
    let rows = if let Some(target_id) = target_id {
        statement
            .query_map(params![scope_kind, target_id], prompt_context_binding_row)
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?
    } else {
        statement
            .query_map([scope_kind], prompt_context_binding_row)
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?
    };
    if rows.len() > 1 {
        return Err(prompt_context_changed(
            "multiple enabled prompt bindings now apply at one scope",
        ));
    }
    rows.into_iter()
        .next()
        .map(|row| decode_current_prompt_binding(row, scope_kind, target_id, conversation_id))
        .transpose()
}

#[cfg(test)]
type CurrentPromptBindingRow = (String, i64, String, String);

#[cfg(test)]
fn prompt_context_binding_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<CurrentPromptBindingRow> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
}

#[cfg(test)]
fn decode_current_prompt_binding(
    row: CurrentPromptBindingRow,
    scope_kind: &str,
    target_id: Option<&str>,
    conversation_id: &ConversationId,
) -> CoreResult<CurrentPromptBinding> {
    let value: PromptPresetBinding = serde_json::from_str(&row.3)
        .map_err(|error| storage_corrupted(format!("stored prompt binding is invalid: {error}")))?;
    validate_prompt_binding_context_for_test(&value).map_err(|error| {
        storage_corrupted(format!("stored prompt binding context is invalid: {error}"))
    })?;
    let targets = prompt_binding_targets_for_test(&value)?;
    let document_target = match scope_kind {
        "branch" => targets.branch_id,
        "conversation" => targets.conversation_id,
        "character" => targets.character_id,
        "persona" => targets.persona_id,
        _ => None,
    };
    if value.id != row.0
        || value.prompt_preset_id.as_str() != row.2
        || targets.scope_kind != scope_kind
        || document_target != target_id
        || (scope_kind == "branch" && targets.conversation_id != Some(conversation_id.0.as_str()))
    {
        return Err(storage_corrupted(
            "stored prompt binding document differs from its projection",
        ));
    }
    Ok(CurrentPromptBinding {
        evidence: PromptContextBindingEvidence {
            binding_id: row.0,
            binding_revision: u64_revision(row.1)?,
            document_sha256: sha256_hex(row.3.as_bytes()),
        },
        prompt_preset_id: value.prompt_preset_id,
    })
}

#[cfg(test)]
fn require_prompt_context_summaries(
    transaction: &Transaction<'_>,
    expected: &PromptContextSnapshotV1,
) -> CoreResult<()> {
    if expected.summaries.is_empty() {
        if expected.conversation_summary_id.is_some() {
            return Err(CoreError::invalid(
                "prompt context conversation summary is missing its evidence",
            ));
        }
        return Ok(());
    }
    let current = memory_records_at_head_in_connection(
        transaction,
        &expected.conversation_id,
        &expected.source_branch_id,
        expected.context_head_message_id.as_ref(),
        false,
    )?;
    if current.records.len() != current.snapshot.records.len() {
        return Err(storage_corrupted(
            "memory records differ from their exact-head evidence",
        ));
    }
    let visible_summaries = current
        .records
        .into_iter()
        .zip(current.snapshot.records)
        .filter(|(record, _)| {
            record.value.kind == lorepia_domain::MemoryKind::ConversationSummary
                && record.value.invalidated_at.is_none()
                && !record.value.excluded_from_conversation
                && !record.value.excluded_from_character
                && record.deleted_at.is_none()
        })
        .collect::<Vec<_>>();
    for expected_summary in &expected.summaries {
        let unchanged = visible_summaries.iter().any(|(record, evidence)| {
            prompt_summary_evidence_matches(expected_summary, record, evidence)
        });
        if !unchanged {
            return Err(prompt_context_changed(
                "prompt summary changed after generation preparation",
            ));
        }
    }
    if let Some(expected_summary_id) = &expected.conversation_summary_id {
        let Some(context_head) = expected.context_head_message_id.as_ref() else {
            return Err(CoreError::invalid(
                "prompt context summary cannot exist before the first message",
            ));
        };
        let latest = latest_visible_prompt_summary_id(
            transaction,
            expected,
            context_head,
            &visible_summaries,
        )?;
        if latest.as_ref() != Some(expected_summary_id) {
            return Err(prompt_context_changed(
                "latest conversation summary changed after generation preparation",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
fn prompt_summary_evidence_matches(
    expected: &PromptSummarySourceEvidence,
    record: &StoredRevision<MemoryRecord>,
    evidence: &MemoryRecordAtHeadEvidence,
) -> bool {
    expected.summary_id == record.value.id
        && expected.summary_id == evidence.record_id
        && expected.record_branch_id == record.value.branch_id
        && expected.record_branch_id == evidence.record_branch_id
        && expected.source_start_message_id == record.value.source_start_message_id
        && expected.source_start_message_id == evidence.source_start_message_id
        && expected.source_end_message_id == record.value.source_end_message_id
        && expected.source_end_message_id == evidence.source_end_message_id
        && expected.state_revision == record.revision
        && expected.state_revision == evidence.state_revision
        && record.revision_id.as_deref() == Some(expected.active_revision_id.as_str())
        && expected.active_revision_id == evidence.active_revision_id
        && expected.active_revision_sha256 == evidence.active_revision_sha256
}

#[cfg(test)]
fn latest_visible_prompt_summary_id(
    transaction: &Transaction<'_>,
    expected: &PromptContextSnapshotV1,
    context_head: &MessageId,
    summaries: &[(StoredRevision<MemoryRecord>, MemoryRecordAtHeadEvidence)],
) -> CoreResult<Option<MemoryRecordId>> {
    let mut endpoints = summaries
        .iter()
        .map(|(record, _)| record.value.source_end_message_id.clone())
        .collect::<Vec<_>>();
    endpoints.sort_by(|left, right| left.0.cmp(&right.0));
    endpoints.dedup_by(|left, right| left.0 == right.0);
    let depths = prompt_context_lineage_depths(
        transaction,
        &expected.conversation_id,
        &expected.source_branch_id,
        context_head,
        &endpoints,
    )?;
    summaries
        .iter()
        .map(|(record, _)| {
            depths
                .get(&record.value.source_end_message_id)
                .copied()
                .map(|depth| (depth, record.value.id.clone()))
                .ok_or_else(|| storage_corrupted("visible summary has no lineage depth"))
        })
        .collect::<CoreResult<Vec<_>>>()
        .map(|mut ordered| {
            ordered.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
            ordered.pop().map(|(_, id)| id)
        })
}

#[cfg(test)]
fn prompt_context_lineage_depths(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    source_branch_id: &ConversationBranchId,
    context_head: &MessageId,
    message_ids: &[MessageId],
) -> CoreResult<HashMap<MessageId, u64>> {
    let requested = message_ids
        .iter()
        .map(|id| id.0.as_str())
        .collect::<Vec<_>>();
    let requested_json = serde_json::to_string(&requested).map_err(|error| {
        CoreError::internal(format!(
            "cannot encode prompt context lineage request: {error}"
        ))
    })?;
    let mut statement = transaction
        .prepare(
            "WITH RECURSIVE source_lineage(id, parent_id, depth) AS (
                 SELECT message.id, message.parent_id, 0
                 FROM conversation_branches AS branch
                 JOIN messages AS message
                   ON message.conversation_id = branch.conversation_id
                  AND message.id = branch.head_message_id
                 WHERE branch.conversation_id = ?1 AND branch.id = ?2
                 UNION ALL
                 SELECT parent.id, parent.parent_id, child.depth + 1
                 FROM messages AS parent
                 JOIN source_lineage AS child ON child.parent_id = parent.id
                 WHERE parent.conversation_id = ?1 AND child.depth < 100000
             ),
             context(id, parent_id) AS (
                 SELECT message.id, message.parent_id
                 FROM messages AS message
                 JOIN source_lineage ON source_lineage.id = message.id
                 WHERE message.conversation_id = ?1 AND message.id = ?3
             ),
             lineage(id, parent_id, depth) AS (
                 SELECT id, parent_id, 0 FROM context
                 UNION ALL
                 SELECT parent.id, parent.parent_id, child.depth + 1
                 FROM messages AS parent
                 JOIN lineage AS child ON child.parent_id = parent.id
                 WHERE parent.conversation_id = ?1 AND child.depth < 100000
             )
             SELECT lineage.id, lineage.depth
             FROM json_each(?4) AS requested
             JOIN lineage ON lineage.id = requested.value",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map(
            params![
                conversation_id.0,
                source_branch_id.0,
                context_head.0,
                requested_json
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    rows.into_iter()
        .map(|(id, depth)| Ok((MessageId(id), u64_revision(depth)?)))
        .collect()
}
