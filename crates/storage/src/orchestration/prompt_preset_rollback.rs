//! Durable prompt-preset rollback review, approval, and application.

use super::{
    CoreError, CoreResult, DateTime, DocumentTable, ObjectRevision, OptionalExtension,
    PlacementZone, PromptPreset, PromptPresetId, PromptPresetRevisionDiff,
    PromptPresetRollbackCommit, PromptPresetRollbackReview, RevisionEventKind, RevisionWrite,
    SourceKind, Storage, StoredRevision, Transaction, TransactionBehavior, Utc, Value,
    append_content_revision, built_in_prompt_presets, decode_document, encode_document,
    get_object_revision, i64_revision, not_found, params, parse_datetime,
    prompt_preset_diff_from_revisions, prompt_preset_rollback_approval_sha256, revision_conflict,
    sha256_hex, storage_corrupted, storage_db_error, u64_revision, validate_identifier,
    validate_prompt_preset_storage_shape, write_prompt_preset_projection,
};

fn prompt_preset_rollback_review_sha256(review: &PromptPresetRollbackReview) -> CoreResult<String> {
    serde_json::to_string(&serde_json::json!({
        "schema_version": 1,
        "preset_id": review.preset_id,
        "expected_current_state_revision": review.expected_current_state_revision,
        "expected_current_revision_id": review.expected_current_revision_id,
        "expected_current_sha256": review.expected_current_sha256,
        "target_revision_id": review.target_revision_id,
        "target_revision": review.target_revision,
        "target_sha256": review.target_sha256,
        "target_document_sha256": review.target_document_sha256,
        "target_dependency_sha256": review.target_dependency_sha256,
        "binding_snapshot_sha256": review.binding_snapshot_sha256,
        "diff": review.diff,
    }))
    .map(|json| sha256_hex(json.as_bytes()))
    .map_err(|error| {
        CoreError::internal(format!(
            "cannot encode prompt preset rollback review: {error}"
        ))
    })
}

struct CurrentPromptPresetRevision {
    state_revision: u64,
    revision_id: String,
    sha256: String,
    created_at: DateTime<Utc>,
    value: PromptPreset,
}

pub(super) fn review_prompt_preset_rollback(
    storage: &Storage,
    id: &PromptPresetId,
    expected_current_state_revision: u64,
    target_revision: u64,
    reviewed_at: DateTime<Utc>,
) -> CoreResult<PromptPresetRollbackReview> {
    validate_identifier("prompt preset", id.as_str())?;
    if expected_current_state_revision == 0 || target_revision == 0 {
        return Err(CoreError::invalid(
            "prompt preset rollback revisions must be positive",
        ));
    }
    let mut connection = storage.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_db_error)?;
    let current = current_prompt_preset_revision(&transaction, id)?;
    if current.state_revision != expected_current_state_revision {
        return Err(revision_conflict(
            "prompt_preset",
            id.as_str(),
            Some(expected_current_state_revision),
            Some(current.state_revision),
        ));
    }
    if current.value.metadata.provenance.source_kind == SourceKind::ApplicationBuiltIn {
        return Err(CoreError::invalid(
            "application-built-in prompt presets cannot be rolled back",
        ));
    }
    let target = get_object_revision::<PromptPreset>(
        &transaction,
        DocumentTable::PromptPresets,
        id.as_str(),
        target_revision,
    )?;
    if target.value.metadata.provenance.source_kind == SourceKind::ApplicationBuiltIn {
        return Err(CoreError::invalid(
            "application-built-in prompt preset revisions cannot be rollback targets",
        ));
    }
    validate_historical_prompt_rollback_target(&target.value)?;
    if target.revision_id == current.revision_id {
        return Err(CoreError::invalid(
            "prompt preset rollback target is already active",
        ));
    }
    let diff = build_prompt_preset_rollback_diff(id, &current, &target)?;
    let target_dependency_sha256 =
        prompt_preset_dependency_snapshot_sha256(&transaction, &target.revision_id)?;
    let binding_snapshot_sha256 = prompt_preset_binding_snapshot_sha256(&transaction, id.as_str())?;
    let mut review = PromptPresetRollbackReview {
        review_sha256: String::new(),
        preset_id: id.clone(),
        expected_current_state_revision,
        expected_current_revision_id: current.revision_id,
        expected_current_sha256: current.sha256,
        target_revision_id: target.revision_id,
        target_revision,
        target_sha256: target.sha256.clone(),
        target_document_sha256: target.sha256,
        target_dependency_sha256,
        binding_snapshot_sha256,
        diff,
        reviewed_at,
    };
    review.review_sha256 = prompt_preset_rollback_review_sha256(&review)?;
    let review = persist_prompt_preset_rollback_review(&transaction, &review)?;
    transaction.commit().map_err(storage_db_error)?;
    Ok(review)
}

fn build_prompt_preset_rollback_diff(
    id: &PromptPresetId,
    current: &CurrentPromptPresetRevision,
    target: &ObjectRevision<PromptPreset>,
) -> CoreResult<PromptPresetRevisionDiff> {
    let current_value = serde_json::to_value(&current.value).map_err(|error| {
        CoreError::internal(format!("cannot inspect current prompt preset: {error}"))
    })?;
    let target_value = serde_json::to_value(&target.value).map_err(|error| {
        CoreError::internal(format!("cannot inspect target prompt preset: {error}"))
    })?;
    prompt_preset_diff_from_revisions(
        id,
        ObjectRevision {
            revision_id: current.revision_id.clone(),
            object_kind: "prompt_preset".to_owned(),
            object_id: id.as_str().to_owned(),
            revision: current.state_revision,
            value: current_value,
            sha256: current.sha256.clone(),
            created_at: current.created_at,
        },
        ObjectRevision {
            revision_id: target.revision_id.clone(),
            object_kind: target.object_kind.clone(),
            object_id: target.object_id.clone(),
            revision: target.revision,
            value: target_value,
            sha256: target.sha256.clone(),
            created_at: target.created_at,
        },
    )
}

fn persist_prompt_preset_rollback_review(
    transaction: &Transaction<'_>,
    review: &PromptPresetRollbackReview,
) -> CoreResult<PromptPresetRollbackReview> {
    let review_json = serde_json::to_string(review).map_err(|error| {
        CoreError::internal(format!(
            "cannot encode prompt preset rollback review: {error}"
        ))
    })?;
    let existing = transaction
        .query_row(
            "SELECT review_json
             FROM prompt_preset_rollback_reviews
             WHERE review_sha256 = ?1",
            [review.review_sha256.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?;
    if let Some(existing) = existing {
        return decode_document("prompt preset rollback review", &existing);
    }
    transaction
        .execute(
            "INSERT INTO prompt_preset_rollback_reviews
             (review_sha256, prompt_preset_id, expected_state_revision,
              expected_current_revision_id, target_revision_id,
              target_dependency_sha256, binding_snapshot_sha256,
              diff_sha256, review_json, reviewed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                review.review_sha256,
                review.preset_id.as_str(),
                i64_revision(review.expected_current_state_revision)?,
                review.expected_current_revision_id,
                review.target_revision_id,
                review.target_dependency_sha256,
                review.binding_snapshot_sha256,
                review.diff.diff_sha256,
                review_json,
                review.reviewed_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(review.clone())
}

pub(super) fn apply_prompt_preset_rollback(
    storage: &Storage,
    commit: &PromptPresetRollbackCommit,
) -> CoreResult<StoredRevision<PromptPreset>> {
    validate_prompt_preset_rollback_commit(commit)?;
    let mut connection = storage.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_db_error)?;
    if let Some(stored) = recover_applied_prompt_preset_rollback(&transaction, commit)? {
        transaction.commit().map_err(storage_db_error)?;
        return Ok(stored);
    }
    let persisted_review =
        load_durable_prompt_preset_rollback_review(&transaction, &commit.review.review_sha256)?;
    if persisted_review != commit.review {
        return Err(CoreError::invalid(
            "prompt preset rollback review differs from durable review",
        ));
    }
    let validated = validate_prompt_preset_rollback_state(&transaction, commit)?;
    validate_canonical_prompt_rollback_target(
        &validated.current.value,
        &validated.target.value,
        &commit.canonical_target,
    )?;
    insert_prompt_preset_rollback_approval(&transaction, commit)?;
    let written = append_content_revision(
        &transaction,
        DocumentTable::PromptPresets,
        commit.review.preset_id.as_str(),
        commit.canonical_target.schema_version,
        &commit.canonical_target,
        &commit.canonical_target.metadata.provenance,
        Some(commit.review.expected_current_state_revision),
        RevisionEventKind::Rollback,
    )?;
    materialize_prompt_preset_rollback(&transaction, commit, &written)?;
    transaction.commit().map_err(storage_db_error)?;
    Ok(StoredRevision {
        value: commit.canonical_target.clone(),
        revision: written.state_version,
        revision_id: Some(written.revision_id),
        created_at: written.created_at,
        updated_at: written.updated_at,
        deleted_at: None,
    })
}

fn validate_prompt_preset_rollback_commit(commit: &PromptPresetRollbackCommit) -> CoreResult<()> {
    validate_identifier(
        "prompt preset rollback approval",
        &commit.approval.approval_id,
    )?;
    let expected_approval_sha256 = prompt_preset_rollback_approval_sha256(
        &commit.approval.approval_id,
        &commit.approval.expected_review_sha256,
    )?;
    if commit.approval.approval_sha256 != expected_approval_sha256
        || commit.approval.expected_review_sha256 != commit.review.review_sha256
        || prompt_preset_rollback_review_sha256(&commit.review)? != commit.review.review_sha256
        || commit.canonical_target.id != commit.review.preset_id
    {
        return Err(CoreError::invalid(
            "prompt preset rollback approval or review hash is invalid",
        ));
    }
    validate_prompt_preset_storage_shape(&commit.canonical_target)?;
    Ok(())
}

fn recover_applied_prompt_preset_rollback(
    transaction: &Transaction<'_>,
    commit: &PromptPresetRollbackCommit,
) -> CoreResult<Option<StoredRevision<PromptPreset>>> {
    if let Some(existing) =
        load_applied_prompt_preset_rollback(transaction, &commit.approval.approval_id)?
    {
        if existing.0 != commit.approval.approval_sha256
            || existing.1 != commit.review.review_sha256
        {
            return Err(CoreError::invalid(
                "prompt preset rollback approval id was reused",
            ));
        }
        let revision_id = existing.2.ok_or_else(|| {
            storage_corrupted("prompt preset rollback approval has no applied revision")
        })?;
        let stored = stored_prompt_preset_rollback_revision(
            transaction,
            &commit.review.preset_id,
            &revision_id,
            commit
                .review
                .expected_current_state_revision
                .checked_add(1)
                .ok_or_else(|| CoreError::internal("prompt preset revision overflow"))?,
            existing.3,
        )?;
        return Ok(Some(stored));
    }
    Ok(None)
}

fn load_durable_prompt_preset_rollback_review(
    transaction: &Transaction<'_>,
    review_sha256: &str,
) -> CoreResult<PromptPresetRollbackReview> {
    let persisted_review = transaction
        .query_row(
            "SELECT review_json
             FROM prompt_preset_rollback_reviews
             WHERE review_sha256 = ?1",
            [review_sha256],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("prompt preset rollback review"))?;
    decode_document("prompt preset rollback review", &persisted_review)
}

struct ValidatedPromptPresetRollback {
    current: CurrentPromptPresetRevision,
    target: ObjectRevision<PromptPreset>,
}

fn validate_prompt_preset_rollback_state(
    transaction: &Transaction<'_>,
    commit: &PromptPresetRollbackCommit,
) -> CoreResult<ValidatedPromptPresetRollback> {
    let current = current_prompt_preset_revision(transaction, &commit.review.preset_id)?;
    if current.state_revision != commit.review.expected_current_state_revision
        || current.revision_id != commit.review.expected_current_revision_id
        || current.sha256 != commit.review.expected_current_sha256
    {
        return Err(revision_conflict(
            "prompt_preset",
            commit.review.preset_id.as_str(),
            Some(commit.review.expected_current_state_revision),
            Some(current.state_revision),
        ));
    }
    if current.value.metadata.provenance.source_kind == SourceKind::ApplicationBuiltIn {
        return Err(CoreError::invalid(
            "application-built-in prompt presets cannot be rolled back",
        ));
    }
    let target = get_object_revision::<PromptPreset>(
        transaction,
        DocumentTable::PromptPresets,
        commit.review.preset_id.as_str(),
        commit.review.target_revision,
    )?;
    if target.value.metadata.provenance.source_kind == SourceKind::ApplicationBuiltIn {
        return Err(CoreError::invalid(
            "application-built-in prompt preset revisions cannot be rollback targets",
        ));
    }
    validate_historical_prompt_rollback_target(&target.value)?;
    if target.revision_id != commit.review.target_revision_id
        || target.sha256 != commit.review.target_sha256
        || target.sha256 != commit.review.target_document_sha256
        || prompt_preset_dependency_snapshot_sha256(transaction, &target.revision_id)?
            != commit.review.target_dependency_sha256
        || prompt_preset_binding_snapshot_sha256(transaction, commit.review.preset_id.as_str())?
            != commit.review.binding_snapshot_sha256
    {
        return Err(CoreError::invalid(
            "prompt preset rollback target, dependencies, or bindings changed",
        ));
    }
    Ok(ValidatedPromptPresetRollback { current, target })
}

fn insert_prompt_preset_rollback_approval(
    transaction: &Transaction<'_>,
    commit: &PromptPresetRollbackCommit,
) -> CoreResult<()> {
    transaction
        .execute(
            "INSERT INTO prompt_preset_rollback_approvals
             (approval_id, approval_sha256, review_sha256,
              applied_revision_id, approved_at, applied_at)
             VALUES (?1, ?2, ?3, NULL, ?4, NULL)",
            params![
                commit.approval.approval_id,
                commit.approval.approval_sha256,
                commit.review.review_sha256,
                commit.approval.approved_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn materialize_prompt_preset_rollback(
    transaction: &Transaction<'_>,
    commit: &PromptPresetRollbackCommit,
    written: &RevisionWrite,
) -> CoreResult<()> {
    let (document_json, _) =
        encode_document("prompt preset rollback target", &commit.canonical_target)?;
    write_prompt_preset_projection(
        transaction,
        &written.revision_id,
        &commit.canonical_target,
        &document_json,
        Some(commit.review.expected_current_state_revision),
    )?;
    copy_prompt_preset_dependency_snapshot(
        transaction,
        &commit.review.target_revision_id,
        &written.revision_id,
    )?;
    let applied_at = written.updated_at;
    let changed = transaction
        .execute(
            "UPDATE prompt_preset_rollback_approvals
             SET applied_revision_id = ?2, applied_at = ?3
             WHERE approval_id = ?1 AND applied_revision_id IS NULL",
            params![
                commit.approval.approval_id,
                written.revision_id,
                applied_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(storage_corrupted(
            "prompt preset rollback approval could not be finalized",
        ));
    }
    Ok(())
}

fn current_prompt_preset_revision(
    transaction: &Transaction<'_>,
    id: &PromptPresetId,
) -> CoreResult<CurrentPromptPresetRevision> {
    transaction
        .query_row(
            "SELECT state.state_version, revision.id,
                    revision.document_sha256, revision.created_at,
                    revision.document_json
             FROM content_objects AS object
             JOIN content_object_state AS state ON state.object_id = object.id
             JOIN content_revisions AS revision
               ON revision.object_id = object.id
              AND revision.id = state.active_revision_id
             WHERE object.id = ?1
               AND object.object_kind = 'prompt_preset'
               AND object.deleted_at IS NULL",
            [id.as_str()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("prompt preset"))
        .and_then(|row| {
            Ok(CurrentPromptPresetRevision {
                state_revision: u64_revision(row.0)?,
                revision_id: row.1,
                sha256: row.2,
                created_at: parse_datetime("prompt preset revision created_at", &row.3)?,
                value: decode_document("prompt preset", &row.4)?,
            })
        })
}

fn prompt_preset_dependency_snapshot_sha256(
    transaction: &Transaction<'_>,
    revision_id: &str,
) -> CoreResult<String> {
    let mut rows = Vec::<Value>::new();
    for (kind, sql) in [
        (
            "knowledge",
            "SELECT ordinal, '', knowledge_book_revision_id, '', enabled, config_json
             FROM prompt_preset_knowledge_books
             WHERE prompt_preset_revision_id = ?1 ORDER BY ordinal",
        ),
        (
            "transform",
            "SELECT ordinal, '', transform_set_revision_id, '', enabled, config_json
             FROM prompt_preset_transform_sets
             WHERE prompt_preset_revision_id = ?1 ORDER BY ordinal",
        ),
        (
            "memory",
            "SELECT 0, '', memory_profile_revision_id, '', enabled, config_json
             FROM prompt_preset_memory_profiles
             WHERE prompt_preset_revision_id = ?1",
        ),
        (
            "module",
            "SELECT ordinal, module_id, module_revision_id, source_sha256,
                    enabled, config_json
             FROM prompt_preset_modules
             WHERE prompt_preset_revision_id = ?1 ORDER BY ordinal",
        ),
    ] {
        let mut statement = transaction.prepare(sql).map_err(storage_db_error)?;
        let values = statement
            .query_map([revision_id], |row| {
                Ok(serde_json::json!({
                    "kind": kind,
                    "ordinal": row.get::<_, i64>(0)?,
                    "object_id": row.get::<_, String>(1)?,
                    "revision_id": row.get::<_, String>(2)?,
                    "source_sha256": row.get::<_, String>(3)?,
                    "enabled": row.get::<_, bool>(4)?,
                    "config_json": row.get::<_, String>(5)?,
                }))
            })
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?;
        rows.extend(values);
    }
    serde_json::to_string(&serde_json::json!({
        "schema_version": 1,
        "prompt_preset_revision_id": revision_id,
        "dependencies": rows,
    }))
    .map(|json| sha256_hex(json.as_bytes()))
    .map_err(|error| {
        CoreError::internal(format!(
            "cannot encode prompt preset dependency snapshot: {error}"
        ))
    })
}

fn prompt_preset_binding_snapshot_sha256(
    transaction: &Transaction<'_>,
    preset_id: &str,
) -> CoreResult<String> {
    let mut statement = transaction
        .prepare(
            "SELECT id, resolution_mode, pinned_revision_id, revision,
                    enabled, priority, updated_at
             FROM prompt_preset_bindings
             WHERE prompt_preset_id = ?1 AND deleted_at IS NULL
             ORDER BY id",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map([preset_id], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "resolution_mode": row.get::<_, String>(1)?,
                "pinned_revision_id": row.get::<_, Option<String>>(2)?,
                "revision": row.get::<_, i64>(3)?,
                "enabled": row.get::<_, bool>(4)?,
                "priority": row.get::<_, i64>(5)?,
                "updated_at": row.get::<_, String>(6)?,
            }))
        })
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    serde_json::to_string(&serde_json::json!({
        "schema_version": 1,
        "prompt_preset_id": preset_id,
        "bindings": rows,
    }))
    .map(|json| sha256_hex(json.as_bytes()))
    .map_err(|error| {
        CoreError::internal(format!(
            "cannot encode prompt preset binding snapshot: {error}"
        ))
    })
}

fn validate_canonical_prompt_rollback_target(
    _current: &PromptPreset,
    historical_target: &PromptPreset,
    canonical_target: &PromptPreset,
) -> CoreResult<()> {
    validate_historical_prompt_rollback_target(historical_target)?;
    let without_application = |preset: &PromptPreset| {
        let mut value = preset.clone();
        value.blocks.retain(|block| {
            block.authority != lorepia_domain::InstructionAuthority::Application
                && block.placement_zone != PlacementZone::ApplicationPolicy
        });
        value
    };
    let application_blocks = |preset: &PromptPreset| {
        preset
            .blocks
            .iter()
            .filter(|block| {
                block.authority == lorepia_domain::InstructionAuthority::Application
                    || block.placement_zone == PlacementZone::ApplicationPolicy
            })
            .cloned()
            .collect::<Vec<_>>()
    };
    let canonical_application_policy = built_in_prompt_presets()[0].clone();
    if without_application(historical_target) != without_application(canonical_target)
        || application_blocks(&canonical_application_policy) != application_blocks(canonical_target)
    {
        return Err(CoreError::invalid(
            "canonical rollback target changes reviewed content or application policy",
        ));
    }
    Ok(())
}

fn validate_historical_prompt_rollback_target(target: &PromptPreset) -> CoreResult<()> {
    if target.blocks.iter().any(|block| {
        block.provenance.source_kind == SourceKind::ApplicationBuiltIn
            && (block.authority != lorepia_domain::InstructionAuthority::Application
                || block.placement_zone != PlacementZone::ApplicationPolicy)
    }) {
        Err(CoreError::invalid(
            "historical rollback target contains a creator block with application-built-in provenance",
        ))
    } else {
        Ok(())
    }
}

fn copy_prompt_preset_dependency_snapshot(
    transaction: &Transaction<'_>,
    source_revision_id: &str,
    target_revision_id: &str,
) -> CoreResult<()> {
    for table in [
        "prompt_preset_knowledge_books",
        "prompt_preset_transform_sets",
        "prompt_preset_memory_profiles",
        "prompt_preset_modules",
    ] {
        transaction
            .execute(
                &format!("DELETE FROM {table} WHERE prompt_preset_revision_id = ?1"),
                [target_revision_id],
            )
            .map_err(storage_db_error)?;
    }
    transaction
        .execute(
            "INSERT INTO prompt_preset_knowledge_books
             SELECT ?2, ordinal, knowledge_book_revision_id, enabled, config_json
             FROM prompt_preset_knowledge_books
             WHERE prompt_preset_revision_id = ?1",
            params![source_revision_id, target_revision_id],
        )
        .map_err(storage_db_error)?;
    transaction
        .execute(
            "INSERT INTO prompt_preset_transform_sets
             SELECT ?2, ordinal, transform_set_revision_id, enabled, config_json
             FROM prompt_preset_transform_sets
             WHERE prompt_preset_revision_id = ?1",
            params![source_revision_id, target_revision_id],
        )
        .map_err(storage_db_error)?;
    transaction
        .execute(
            "INSERT INTO prompt_preset_memory_profiles
             SELECT ?2, memory_profile_revision_id, enabled, config_json
             FROM prompt_preset_memory_profiles
             WHERE prompt_preset_revision_id = ?1",
            params![source_revision_id, target_revision_id],
        )
        .map_err(storage_db_error)?;
    transaction
        .execute(
            "INSERT INTO prompt_preset_modules
             SELECT ?2, ordinal, module_id, module_revision_id,
                    source_sha256, enabled, config_json
             FROM prompt_preset_modules
             WHERE prompt_preset_revision_id = ?1",
            params![source_revision_id, target_revision_id],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

type AppliedPromptPresetRollbackRow = (String, String, Option<String>, Option<DateTime<Utc>>);

fn load_applied_prompt_preset_rollback(
    transaction: &Transaction<'_>,
    approval_id: &str,
) -> CoreResult<Option<AppliedPromptPresetRollbackRow>> {
    transaction
        .query_row(
            "SELECT approval_sha256, review_sha256,
                    applied_revision_id, applied_at
             FROM prompt_preset_rollback_approvals
             WHERE approval_id = ?1",
            [approval_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .map(|row| {
            Ok((
                row.0,
                row.1,
                row.2,
                row.3
                    .as_deref()
                    .map(|value| parse_datetime("rollback applied_at", value))
                    .transpose()?,
            ))
        })
        .transpose()
}

fn stored_prompt_preset_rollback_revision(
    transaction: &Transaction<'_>,
    id: &PromptPresetId,
    revision_id: &str,
    state_revision: u64,
    applied_at: Option<DateTime<Utc>>,
) -> CoreResult<StoredRevision<PromptPreset>> {
    let row = transaction
        .query_row(
            "SELECT revision.document_json, object.created_at
             FROM content_revisions AS revision
             JOIN content_objects AS object
               ON object.id = revision.object_id
             WHERE revision.id = ?1 AND revision.object_id = ?2
               AND revision.object_kind = 'prompt_preset'",
            params![revision_id, id.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("applied prompt preset rollback revision"))?;
    let created_at = parse_datetime("rollback revision created_at", &row.1)?;
    Ok(StoredRevision {
        value: decode_document("prompt preset rollback revision", &row.0)?,
        revision: state_revision,
        revision_id: Some(revision_id.to_owned()),
        created_at,
        updated_at: applied_at.unwrap_or(created_at),
        deleted_at: None,
    })
}
