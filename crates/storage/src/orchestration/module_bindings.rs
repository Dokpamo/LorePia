//! Module binding persistence, staleness, and context matching.

use super::{
    BTreeSet, ContentModuleId, CoreError, CoreResult, DateTime, ModuleBinding, ModuleBindingId,
    ModuleRevisionId, ModuleScope, OptionalExtension, RawStoredDocument, Storage, StoredRevision,
    Transaction, TransactionBehavior, Utc, ValidateOrchestration, Value, decode_document,
    decode_stored_document, encode_document, enum_wire, i64_revision, not_found, params,
    parse_datetime, revision_conflict, storage_db_error, u64_revision, validate_json_bounds,
};

impl Storage {
    pub fn save_module_binding(
        &self,
        binding: &ModuleBinding,
        expected_revision: Option<u64>,
    ) -> CoreResult<StoredRevision<ModuleBinding>> {
        save_module_binding(self, binding, expected_revision)
    }

    pub fn list_module_bindings(
        &self,
        module_id: &ContentModuleId,
    ) -> CoreResult<Vec<StoredRevision<ModuleBinding>>> {
        list_module_bindings(self, module_id)
    }

    pub fn soft_delete_module_binding(
        &self,
        id: &ModuleBindingId,
        expected_revision: u64,
    ) -> CoreResult<StoredRevision<ModuleBinding>> {
        soft_delete_module_binding(self, id, expected_revision)
    }

    pub fn list_all_module_bindings(&self) -> CoreResult<Vec<StoredRevision<ModuleBinding>>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT document_json, revision, created_at, updated_at, deleted_at
                 FROM content_module_bindings
                 WHERE deleted_at IS NULL
                 ORDER BY scope_kind, priority DESC, id",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map([], module_binding_row)
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?;
        rows.into_iter()
            .map(|row| decode_stored_document("module binding", row))
            .collect()
    }
}

pub(super) struct ModuleBindingTargets<'a> {
    pub(super) scope_kind: &'static str,
    persona_id: Option<&'a str>,
    character_id: Option<&'a str>,
    conversation_id: Option<&'a str>,
    branch_id: Option<&'a str>,
}

pub(super) fn module_binding_targets(
    binding: &ModuleBinding,
) -> CoreResult<ModuleBindingTargets<'_>> {
    let target = binding.target_id.as_deref();
    match binding.scope {
        ModuleScope::App if target.is_none() && binding.conversation_id.is_none() => {
            Ok(ModuleBindingTargets {
                scope_kind: "app",
                persona_id: None,
                character_id: None,
                conversation_id: None,
                branch_id: None,
            })
        }
        ModuleScope::User if target.is_none() && binding.conversation_id.is_none() => {
            Ok(ModuleBindingTargets {
                scope_kind: "user",
                persona_id: None,
                character_id: None,
                conversation_id: None,
                branch_id: None,
            })
        }
        ModuleScope::Persona if target.is_some() && binding.conversation_id.is_none() => {
            Ok(ModuleBindingTargets {
                scope_kind: "persona",
                persona_id: target,
                character_id: None,
                conversation_id: None,
                branch_id: None,
            })
        }
        ModuleScope::Character if target.is_some() && binding.conversation_id.is_none() => {
            Ok(ModuleBindingTargets {
                scope_kind: "character",
                persona_id: None,
                character_id: target,
                conversation_id: None,
                branch_id: None,
            })
        }
        ModuleScope::Conversation if target.is_some() && binding.conversation_id.is_none() => {
            Ok(ModuleBindingTargets {
                scope_kind: "conversation",
                persona_id: None,
                character_id: None,
                conversation_id: target,
                branch_id: None,
            })
        }
        ModuleScope::Branch if target.is_some() && binding.conversation_id.is_some() => {
            Ok(ModuleBindingTargets {
                scope_kind: "branch",
                persona_id: None,
                character_id: None,
                conversation_id: binding.conversation_id.as_ref().map(|id| id.0.as_str()),
                branch_id: target,
            })
        }
        _ => Err(CoreError::invalid(
            "module binding scope and target are inconsistent",
        )),
    }
}

fn validate_module_binding_revision(
    transaction: &Transaction<'_>,
    binding: &ModuleBinding,
) -> CoreResult<()> {
    let exists = transaction
        .query_row(
            "SELECT 1 FROM content_module_revisions
             WHERE module_id = ?1 AND revision_id = ?2",
            params![binding.module_id.as_str(), binding.revision_id.as_str()],
            |_| Ok(()),
        )
        .optional()
        .map_err(storage_db_error)?
        .is_some();
    if !exists {
        return Err(not_found("module binding revision"));
    }
    match binding.resolution_mode {
        lorepia_domain::ModuleRevisionResolutionMode::Pinned => {
            if binding.pinned_revision_id.as_ref() != Some(&binding.revision_id) {
                return Err(CoreError::invalid(
                    "pinned module binding revision is inconsistent",
                ));
            }
        }
        lorepia_domain::ModuleRevisionResolutionMode::Active => {
            let active = transaction
                .query_row(
                    "SELECT active_revision_id
                     FROM content_object_state WHERE object_id = ?1",
                    [binding.module_id.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .map_err(storage_db_error)?;
            if active != binding.revision_id.as_str() {
                return Err(CoreError::invalid(
                    "active module binding does not resolve to the current revision",
                ));
            }
        }
    }
    Ok(())
}

fn save_module_binding(
    storage: &Storage,
    binding: &ModuleBinding,
    expected_revision: Option<u64>,
) -> CoreResult<StoredRevision<ModuleBinding>> {
    let mut connection = storage.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_db_error)?;
    let stored = write_module_binding_transaction(&transaction, binding, expected_revision)?;
    transaction.commit().map_err(storage_db_error)?;
    Ok(stored)
}

type RawModuleBindingState = (i64, String, Option<String>);

fn read_module_binding_state(
    transaction: &Transaction<'_>,
    binding_id: &ModuleBindingId,
) -> CoreResult<Option<RawModuleBindingState>> {
    transaction
        .query_row(
            "SELECT revision, created_at, deleted_at
             FROM content_module_bindings WHERE id = ?1",
            [binding_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)
}

fn resolve_module_binding_write_revision(
    binding: &ModuleBinding,
    expected_revision: Option<u64>,
    current: Option<RawModuleBindingState>,
) -> CoreResult<(u64, DateTime<Utc>)> {
    match (expected_revision, current) {
        (None, None) => Ok((1, binding.created_at)),
        (None, Some((actual, _, _))) => Err(revision_conflict(
            "module binding",
            binding.id.as_str(),
            None,
            Some(u64_revision(actual)?),
        )),
        (Some(expected), Some((actual, created_at, deleted_at))) => {
            let actual = u64_revision(actual)?;
            if actual != expected || deleted_at.is_some() {
                return Err(revision_conflict(
                    "module binding",
                    binding.id.as_str(),
                    Some(expected),
                    Some(actual),
                ));
            }
            Ok((
                expected
                    .checked_add(1)
                    .ok_or_else(|| CoreError::internal("module binding revision overflow"))?,
                parse_datetime("module binding created_at", &created_at)?,
            ))
        }
        (Some(expected), None) => Err(revision_conflict(
            "module binding",
            binding.id.as_str(),
            Some(expected),
            None,
        )),
    }
}

struct ModuleBindingWriteMetadata<'a> {
    next_revision: u64,
    expected_revision: Option<u64>,
    variable_overrides_json: &'a str,
    document_json: &'a str,
    created_at: &'a DateTime<Utc>,
    now: &'a DateTime<Utc>,
}

fn execute_module_binding_upsert(
    transaction: &Transaction<'_>,
    value: &ModuleBinding,
    targets: &ModuleBindingTargets<'_>,
    metadata: &ModuleBindingWriteMetadata<'_>,
) -> CoreResult<()> {
    let changed = transaction
        .execute(
            "INSERT INTO content_module_bindings
             (id, revision, module_id, resolution_mode, pinned_revision_id,
              scope_kind, persona_id, character_id, conversation_id, branch_id,
              priority, enabled, approved, activation_approval_id,
              activation_review_sha256, activation_plan_sha256,
              package_import_approval_id, variable_overrides_json,
              document_json, created_at, updated_at, deleted_at)
             VALUES (
                 ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                 ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, NULL
             )
             ON CONFLICT(id) DO UPDATE SET
                 revision = excluded.revision,
                 module_id = excluded.module_id,
                 resolution_mode = excluded.resolution_mode,
                 pinned_revision_id = excluded.pinned_revision_id,
                 scope_kind = excluded.scope_kind,
                 persona_id = excluded.persona_id,
                 character_id = excluded.character_id,
                 conversation_id = excluded.conversation_id,
                 branch_id = excluded.branch_id,
                 priority = excluded.priority,
                 enabled = excluded.enabled,
                 approved = excluded.approved,
                 activation_approval_id = excluded.activation_approval_id,
                 activation_review_sha256 = excluded.activation_review_sha256,
                 activation_plan_sha256 = excluded.activation_plan_sha256,
                 package_import_approval_id =
                     excluded.package_import_approval_id,
                 variable_overrides_json = excluded.variable_overrides_json,
                 document_json = excluded.document_json,
                 updated_at = excluded.updated_at
             WHERE content_module_bindings.revision = ?22
               AND content_module_bindings.deleted_at IS NULL",
            params![
                value.id.as_str(),
                i64_revision(metadata.next_revision)?,
                value.module_id.as_str(),
                enum_wire(&value.resolution_mode)?,
                value
                    .pinned_revision_id
                    .as_ref()
                    .map(ModuleRevisionId::as_str),
                targets.scope_kind,
                targets.persona_id,
                targets.character_id,
                targets.conversation_id,
                targets.branch_id,
                value.priority,
                value.enabled,
                value.approved,
                value.activation_approval_id,
                value
                    .activation_review_sha256
                    .as_ref()
                    .map(lorepia_domain::Sha256Digest::as_str),
                value
                    .activation_plan_sha256
                    .as_ref()
                    .map(lorepia_domain::Sha256Digest::as_str),
                value.package_import_approval_id,
                metadata.variable_overrides_json,
                metadata.document_json,
                metadata.created_at.to_rfc3339(),
                metadata.now.to_rfc3339(),
                metadata
                    .expected_revision
                    .map(i64_revision)
                    .transpose()?
                    .unwrap_or(0),
            ],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(revision_conflict(
            "module binding",
            value.id.as_str(),
            metadata.expected_revision,
            None,
        ));
    }
    Ok(())
}

pub(super) fn write_module_binding_transaction(
    transaction: &Transaction<'_>,
    binding: &ModuleBinding,
    expected_revision: Option<u64>,
) -> CoreResult<StoredRevision<ModuleBinding>> {
    binding
        .validate()
        .map_err(|error| CoreError::invalid(error.to_string()))?;
    if binding.enabled && !binding.approved {
        return Err(CoreError::invalid(
            "enabled module binding requires explicit approval",
        ));
    }
    let targets = module_binding_targets(binding)?;
    validate_module_binding_revision(transaction, binding)?;
    let current = read_module_binding_state(transaction, &binding.id)?;
    let now = Utc::now();
    let (next_revision, created_at) =
        resolve_module_binding_write_revision(binding, expected_revision, current)?;
    let mut value = binding.clone();
    value.created_at = created_at;
    let (document_json, _) = encode_document("module binding", &value)?;
    let variable_overrides_json =
        serde_json::to_string(&value.variable_overrides).map_err(|error| {
            CoreError::invalid(format!("cannot encode module binding variables: {error}"))
        })?;
    let metadata = ModuleBindingWriteMetadata {
        next_revision,
        expected_revision,
        variable_overrides_json: &variable_overrides_json,
        document_json: &document_json,
        created_at: &created_at,
        now: &now,
    };
    execute_module_binding_upsert(transaction, &value, &targets, &metadata)?;
    Ok(StoredRevision {
        value,
        revision: next_revision,
        revision_id: None,
        created_at,
        updated_at: now,
        deleted_at: None,
    })
}

pub(super) fn module_binding_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawStoredDocument> {
    Ok((
        row.get::<_, String>(0)?,
        row.get::<_, i64>(1)?,
        None,
        row.get::<_, String>(2)?,
        row.get::<_, String>(3)?,
        row.get::<_, Option<String>>(4)?,
    ))
}

fn list_module_bindings(
    storage: &Storage,
    module_id: &ContentModuleId,
) -> CoreResult<Vec<StoredRevision<ModuleBinding>>> {
    let connection = storage.connection()?;
    let mut statement = connection
        .prepare(
            "SELECT document_json, revision, created_at, updated_at, deleted_at
             FROM content_module_bindings
             WHERE module_id = ?1 AND deleted_at IS NULL
             ORDER BY priority DESC, id",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map([module_id.as_str()], module_binding_row)
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    rows.into_iter()
        .map(|row| decode_stored_document("module binding", row))
        .collect()
}

pub(super) fn list_all_module_bindings_transaction(
    transaction: &Transaction<'_>,
) -> CoreResult<Vec<StoredRevision<ModuleBinding>>> {
    let mut statement = transaction
        .prepare(
            "SELECT document_json, revision, created_at, updated_at, deleted_at
             FROM content_module_bindings
             WHERE deleted_at IS NULL
             ORDER BY scope_kind, priority DESC, id",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map([], module_binding_row)
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    rows.into_iter()
        .map(|row| decode_stored_document("module binding", row))
        .collect()
}

pub(super) fn resolve_module_binding_revision(
    transaction: &Transaction<'_>,
    binding: &ModuleBinding,
) -> CoreResult<ModuleBinding> {
    let mut resolved = binding.clone();
    resolved.revision_id = match binding.resolution_mode {
        lorepia_domain::ModuleRevisionResolutionMode::Active => ModuleRevisionId::from(
            transaction
                .query_row(
                    "SELECT state.active_revision_id
                         FROM content_objects AS object
                         JOIN content_object_state AS state
                           ON state.object_id = object.id
                         WHERE object.id = ?1
                           AND object.object_kind = 'content_module'
                           AND object.deleted_at IS NULL",
                    [binding.module_id.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(storage_db_error)?
                .ok_or_else(|| not_found("active module revision"))?,
        ),
        lorepia_domain::ModuleRevisionResolutionMode::Pinned => binding
            .pinned_revision_id
            .clone()
            .ok_or_else(|| CoreError::invalid("pinned module binding requires a revision"))?,
    };
    validate_module_binding_revision(transaction, &resolved)?;
    Ok(resolved)
}

pub(super) fn module_activation_resolution_set(
    review: &lorepia_orchestration::ModuleActivationReview,
    plan: &lorepia_orchestration::ModuleActivationPlan,
) -> CoreResult<lorepia_orchestration::ModuleMergeResolutionSet> {
    let mut resolutions = Vec::with_capacity(review.conflicts.len());
    for conflict in &review.conflicts {
        let selected = plan
            .components
            .iter()
            .find(|component| component.component == conflict.component)
            .map(|component| lorepia_domain::ModuleConflictCandidate {
                module_id: component.selected_source.module_id.clone(),
                revision_id: component.selected_source.revision_id.clone(),
                component_hash: component.sha256.clone(),
            });
        if selected.is_none()
            && !plan
                .omitted_components
                .iter()
                .any(|component| component == &conflict.component)
        {
            return Err(CoreError::invalid(
                "module activation plan omits a reviewed conflict decision",
            ));
        }
        resolutions.push(lorepia_domain::ModuleConflictResolution {
            component: conflict.component.clone(),
            expected_candidates: conflict.candidates.clone(),
            selected,
        });
    }
    Ok(lorepia_orchestration::ModuleMergeResolutionSet {
        expected_review_sha256: review.review_sha256.clone(),
        resolutions,
    })
}

pub(super) fn module_component_storage_key(
    component: &lorepia_domain::ModuleComponentRef,
) -> (&'static str, &str) {
    match component {
        lorepia_domain::ModuleComponentRef::PromptBlock { id } => ("prompt_block", id.as_str()),
        lorepia_domain::ModuleComponentRef::Control { id } => ("control", id.as_str()),
        lorepia_domain::ModuleComponentRef::KnowledgeBook { id } => ("knowledge_book", id.as_str()),
        lorepia_domain::ModuleComponentRef::TransformSet { id } => ("transform_set", id.as_str()),
        lorepia_domain::ModuleComponentRef::InteractionRuleSet { id } => {
            ("interaction_rule_set", id.as_str())
        }
        lorepia_domain::ModuleComponentRef::Asset { id } => ("asset", id.as_str()),
    }
}

pub(super) fn insert_module_activation_audit(
    transaction: &Transaction<'_>,
    activation_plan_id: &str,
    sequence: u64,
    plan_revision: u64,
    event_kind: &str,
    payload: &Value,
    created_at: &str,
) -> CoreResult<()> {
    let payload_json = serde_json::to_string(payload).map_err(|error| {
        CoreError::invalid(format!("cannot encode module activation audit: {error}"))
    })?;
    validate_json_bounds("module activation audit", &payload_json)?;
    transaction
        .execute(
            "INSERT INTO module_activation_audit
             (activation_plan_id, sequence, plan_revision, event_kind,
              payload_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                activation_plan_id,
                i64_revision(sequence)?,
                i64_revision(plan_revision)?,
                event_kind,
                payload_json,
                created_at,
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn module_binding_affects_context(
    binding: &ModuleBinding,
    context: &lorepia_orchestration::ModuleResolutionContext,
) -> bool {
    match binding.scope {
        ModuleScope::App | ModuleScope::User => true,
        ModuleScope::Persona => {
            binding.target_id.as_deref()
                == context
                    .persona_id
                    .as_ref()
                    .map(lorepia_domain::PersonaId::as_str)
        }
        ModuleScope::Character => binding.target_id.as_deref() == context.character_id.as_deref(),
        ModuleScope::Conversation => {
            binding.target_id.as_deref() == context.conversation_id.as_deref()
        }
        ModuleScope::Branch => {
            binding.target_id.as_deref() == context.branch_id.as_deref()
                && binding.conversation_id.as_ref().map(|id| id.0.as_str())
                    == context.conversation_id.as_deref()
        }
    }
}

struct AppliedModuleActivationPlanRow {
    id: String,
    revision: i64,
    activation_binding_id: String,
    plan_sha256: String,
}

fn read_applied_module_activation_plans(
    transaction: &Transaction<'_>,
) -> CoreResult<Vec<AppliedModuleActivationPlanRow>> {
    let mut statement = transaction
        .prepare(
            "SELECT id, revision, activation_binding_id, plan_sha256
             FROM module_activation_plans
             WHERE state = 'applied'
             ORDER BY applied_at, id",
        )
        .map_err(storage_db_error)?;
    statement
        .query_map([], |row| {
            Ok(AppliedModuleActivationPlanRow {
                id: row.get(0)?,
                revision: row.get(1)?,
                activation_binding_id: row.get(2)?,
                plan_sha256: row.get(3)?,
            })
        })
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)
}

fn stale_applied_module_activation_plans(
    transaction: &Transaction<'_>,
    binding_ids: &BTreeSet<String>,
    reason: &str,
    replacement_plan_sha256: Option<&lorepia_domain::Sha256Digest>,
    created_at: &str,
) -> CoreResult<()> {
    for plan in read_applied_module_activation_plans(transaction)? {
        if !binding_ids.contains(&plan.activation_binding_id)
            || replacement_plan_sha256
                .is_some_and(|replacement| replacement.as_str() == plan.plan_sha256)
        {
            continue;
        }
        let plan_revision = u64_revision(plan.revision)?;
        let stale_revision = plan_revision
            .checked_add(1)
            .ok_or_else(|| CoreError::internal("module activation revision overflow"))?;
        let changed = transaction
            .execute(
                "UPDATE module_activation_plans
                 SET state = 'stale', revision = ?2
                 WHERE id = ?1 AND state = 'applied' AND revision = ?3",
                params![
                    plan.id,
                    i64_revision(stale_revision)?,
                    i64_revision(plan_revision)?,
                ],
            )
            .map_err(storage_db_error)?;
        if changed != 1 {
            return Err(CoreError::invalid(
                "concurrent module activation invalidated the reviewed state",
            ));
        }
        insert_module_activation_audit(
            transaction,
            &plan.id,
            stale_revision,
            stale_revision,
            "stale",
            &serde_json::json!({
                "reason": reason,
                "replacement_plan_sha256": replacement_plan_sha256,
            }),
            created_at,
        )?;
    }
    Ok(())
}

struct AppliedModuleRuntimeContextRow {
    applied_plan_sha256: String,
    context_json: String,
}

fn read_applied_module_runtime_contexts(
    transaction: &Transaction<'_>,
) -> CoreResult<Vec<AppliedModuleRuntimeContextRow>> {
    let mut statement = transaction
        .prepare(
            "SELECT applied_plan_sha256, context_json
             FROM applied_module_runtime_plans
             WHERE state = 'applied'
             ORDER BY created_at, applied_plan_sha256",
        )
        .map_err(storage_db_error)?;
    statement
        .query_map([], |row| {
            Ok(AppliedModuleRuntimeContextRow {
                applied_plan_sha256: row.get(0)?,
                context_json: row.get(1)?,
            })
        })
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)
}

fn stale_applied_module_runtime_plans(
    transaction: &Transaction<'_>,
    old_binding: Option<&ModuleBinding>,
    new_binding: Option<&ModuleBinding>,
    created_at: &str,
) -> CoreResult<()> {
    for row in read_applied_module_runtime_contexts(transaction)? {
        let context: lorepia_orchestration::ModuleResolutionContext =
            decode_document("applied module runtime context", &row.context_json)?;
        let affected = old_binding
            .is_some_and(|binding| module_binding_affects_context(binding, &context))
            || new_binding.is_some_and(|binding| module_binding_affects_context(binding, &context));
        if affected {
            transaction
                .execute(
                    "UPDATE applied_module_runtime_plans
                     SET state = 'stale', stale_at = ?2
                     WHERE applied_plan_sha256 = ?1 AND state = 'applied'",
                    params![row.applied_plan_sha256, created_at],
                )
                .map_err(storage_db_error)?;
        }
    }
    Ok(())
}

pub(super) fn stale_affected_module_activation_plans(
    transaction: &Transaction<'_>,
    old_binding: Option<&ModuleBinding>,
    new_binding: Option<&ModuleBinding>,
    reason: &str,
    replacement_plan_sha256: Option<&lorepia_domain::Sha256Digest>,
    created_at: &str,
) -> CoreResult<()> {
    let binding_ids = old_binding
        .into_iter()
        .chain(new_binding)
        .map(|binding| binding.id.as_str().to_owned())
        .collect::<BTreeSet<_>>();
    stale_applied_module_activation_plans(
        transaction,
        &binding_ids,
        reason,
        replacement_plan_sha256,
        created_at,
    )?;
    stale_applied_module_runtime_plans(transaction, old_binding, new_binding, created_at)
}

fn soft_delete_module_binding(
    storage: &Storage,
    id: &ModuleBindingId,
    expected_revision: u64,
) -> CoreResult<StoredRevision<ModuleBinding>> {
    let mut connection = storage.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_db_error)?;
    let row = transaction
        .query_row(
            "SELECT document_json, revision, created_at, updated_at, deleted_at
             FROM content_module_bindings WHERE id = ?1",
            [id.as_str()],
            module_binding_row,
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("module binding"))?;
    let current = decode_stored_document::<ModuleBinding>("module binding", row)?;
    if current.deleted_at.is_some() || current.revision != expected_revision {
        return Err(revision_conflict(
            "module binding",
            id.as_str(),
            Some(expected_revision),
            Some(current.revision),
        ));
    }
    let next_revision = expected_revision
        .checked_add(1)
        .ok_or_else(|| CoreError::internal("module binding revision overflow"))?;
    let now = Utc::now();
    let now_text = now.to_rfc3339();
    stale_affected_module_activation_plans(
        &transaction,
        Some(&current.value),
        None,
        "binding_deleted",
        None,
        &now_text,
    )?;
    let changed = transaction
        .execute(
            "UPDATE content_module_bindings
             SET revision = ?2, enabled = 0, updated_at = ?3, deleted_at = ?3
             WHERE id = ?1 AND revision = ?4 AND deleted_at IS NULL",
            params![
                id.as_str(),
                i64_revision(next_revision)?,
                now_text,
                i64_revision(expected_revision)?,
            ],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(revision_conflict(
            "module binding",
            id.as_str(),
            Some(expected_revision),
            None,
        ));
    }
    transaction.commit().map_err(storage_db_error)?;
    Ok(StoredRevision {
        value: current.value,
        revision: next_revision,
        revision_id: None,
        created_at: current.created_at,
        updated_at: now,
        deleted_at: Some(now),
    })
}
