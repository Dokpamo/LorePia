//! Approved module activation, rollback, and response-loss recovery.

use super::{
    CoreError, CoreResult, DateTime, Deserialize, ModuleBinding, ModuleBindingId,
    OptionalExtension, RecoveredModuleActivation, RecoveredModuleRollback, Storage, StoredRevision,
    Transaction, TransactionBehavior, Utc, Uuid, VerifiedCompletedPackageAuthorities,
    decode_document, decode_stored_document, insert_module_activation_audit,
    list_all_module_bindings_transaction, load_content_module_revision,
    module_activation_resolution_set, module_activation_snapshots, module_binding_row,
    module_binding_targets, module_component_storage_key, not_found, params,
    persist_applied_module_runtime_plan_transaction, resolve_module_binding_revision,
    revision_conflict, sha256_hex, stale_affected_module_activation_plans, storage_corrupted,
    storage_db_error, usize_to_i64, validate_identifier, validate_json_bounds,
    verify_module_import_authorities, write_module_binding_transaction,
};

impl Storage {
    /// Atomically revalidates and applies one hash-bound module activation.
    pub fn apply_approved_module_activation(
        &self,
        review: &lorepia_orchestration::ModuleActivationReview,
        approved: &lorepia_orchestration::ApprovedModuleActivationPlan,
    ) -> CoreResult<StoredRevision<ModuleBinding>> {
        apply_approved_module_activation(self, review, approved)
    }

    /// Recovers one already-applied activation after a lost response.
    ///
    /// The lookup is deliberately keyed by both the caller-stable approval id
    /// and the exact plan hash. Reusing either identity for a different
    /// activation is rejected instead of being treated as a new write.
    pub fn recover_applied_module_activation(
        &self,
        binding_id: &ModuleBindingId,
        approval: &lorepia_orchestration::ModuleActivationApproval,
    ) -> CoreResult<Option<RecoveredModuleActivation>> {
        recover_applied_module_activation(self, binding_id, approval)
    }

    /// Recovers one already-applied rollback after a lost response.
    ///
    /// The activation identity and final binding are checked by the ordinary
    /// recovery path first. The rollback-only plan and approval digest must
    /// then be present in, and verify against, the immutable prepared audit.
    pub fn recover_applied_module_rollback(
        &self,
        binding_id: &ModuleBindingId,
        approval: &lorepia_orchestration::ModuleActivationApproval,
    ) -> CoreResult<Option<RecoveredModuleRollback>> {
        recover_applied_module_rollback(self, binding_id, approval)
    }

    /// Applies a previously reviewed rollback only when every hash-bound
    /// revision and the binding CAS version still match the reviewed snapshot.
    pub fn apply_module_rollback_plan(
        &self,
        _plan: &lorepia_orchestration::ModuleRollbackPlan,
    ) -> CoreResult<StoredRevision<ModuleBinding>> {
        Err(CoreError::invalid(
            "module rollback requires an approved target runtime plan",
        ))
    }

    /// Atomically applies a rollback together with its freshly approved target
    /// runtime composition.
    pub fn apply_approved_module_rollback(
        &self,
        approved: &lorepia_orchestration::ApprovedModuleRollbackPlan,
    ) -> CoreResult<StoredRevision<ModuleBinding>> {
        apply_approved_module_rollback(self, approved)
    }
}

#[allow(clippy::too_many_lines)]
fn apply_approved_module_activation(
    storage: &Storage,
    review: &lorepia_orchestration::ModuleActivationReview,
    approved: &lorepia_orchestration::ApprovedModuleActivationPlan,
) -> CoreResult<StoredRevision<ModuleBinding>> {
    apply_approved_module_activation_internal(storage, review, approved, None, None)
}

#[allow(
    clippy::too_many_lines,
    reason = "one bounded recovery pass validates the persisted review, approval, and binding together"
)]
fn recover_applied_module_activation(
    storage: &Storage,
    binding_id: &ModuleBindingId,
    approval: &lorepia_orchestration::ModuleActivationApproval,
) -> CoreResult<Option<RecoveredModuleActivation>> {
    validate_identifier("module activation binding", binding_id.as_str())?;
    if approval.approval_id.trim().is_empty()
        || approval.approval_id.len()
            > lorepia_orchestration::MAX_MODULE_ACTIVATION_APPROVAL_ID_BYTES
        || approval.approval_id.chars().any(char::is_control)
    {
        return Err(CoreError::invalid(
            "module activation approval id is invalid",
        ));
    }
    let connection = storage.connection()?;
    let mut statement = connection
        .prepare(
            "SELECT review_json, approved_plan_json, state,
                    activation_binding_id, approval_id, approval_sha256,
                    plan_sha256, expected_bindings_revision_sha256
             FROM module_activation_plans
             WHERE plan_sha256 = ?1 OR approval_id = ?2
             ORDER BY id",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map(
            params![approval.expected_plan_sha256.as_str(), approval.approval_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                ))
            },
        )
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    drop(statement);
    let Some(row) = rows.first() else {
        return Ok(None);
    };
    if rows.len() != 1 {
        return Err(CoreError::invalid(
            "module approval id and plan hash belong to different activations",
        ));
    }
    if row.2 != "applied"
        || row.3 != binding_id.as_str()
        || row.4 != approval.approval_id
        || row.6 != approval.expected_plan_sha256.as_str()
        || row.7 != approval.expected_review_sha256.as_str()
    {
        return Err(CoreError::invalid(
            "module activation approval identity is already bound to another request",
        ));
    }
    let review: lorepia_orchestration::ModuleActivationReview =
        decode_document("applied module activation review", &row.0)?;
    let approved: lorepia_orchestration::ApprovedModuleActivationPlan =
        decode_document("applied module activation plan", &row.1)?;
    review.verify().map_err(|error| {
        storage_corrupted(format!(
            "applied module activation review is invalid: {error}"
        ))
    })?;
    approved.verify().map_err(|error| {
        storage_corrupted(format!(
            "applied module activation plan is invalid: {error}"
        ))
    })?;
    if approved.approval_id != approval.approval_id
        || approved.approval_sha256.as_str() != row.5
        || approved.plan.plan_sha256 != approval.expected_plan_sha256
        || approved.plan.review_sha256 != approval.expected_review_sha256
        || review.review_sha256 != approval.expected_review_sha256
        || approved.plan.expected_state_revision != review.state_revision
        || approved.plan.activation_binding_ids != review.activation_binding_ids
        || review.activation_binding_ids.len() != 1
        || review.activation_binding_ids.first() != Some(binding_id)
    {
        return Err(storage_corrupted(
            "applied module activation authority is internally inconsistent",
        ));
    }
    let binding_row = connection
        .query_row(
            "SELECT document_json, revision, created_at, updated_at, deleted_at
             FROM content_module_bindings
             WHERE id = ?1",
            [binding_id.as_str()],
            module_binding_row,
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| storage_corrupted("applied module activation binding is missing"))?;
    let binding =
        decode_stored_document::<ModuleBinding>("applied module activation binding", binding_row)?;
    if binding.deleted_at.is_some()
        || !binding.value.enabled
        || !binding.value.approved
        || binding.value.activation_approval_id.as_deref() != Some(approved.approval_id.as_str())
        || binding.value.activation_review_sha256.as_ref() != Some(&review.review_sha256)
        || binding.value.activation_plan_sha256.as_ref() != Some(&approved.plan.plan_sha256)
    {
        return Err(storage_corrupted(
            "applied module activation binding no longer matches its authority",
        ));
    }
    Ok(Some(RecoveredModuleActivation {
        review,
        approved,
        binding,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModuleActivationPreparedAuditPayload {
    review_sha256: lorepia_domain::Sha256Digest,
    plan_sha256: lorepia_domain::Sha256Digest,
    binding_id: ModuleBindingId,
    rollback: Option<lorepia_orchestration::ModuleRollbackPlan>,
    rollback_approval_sha256: Option<lorepia_domain::Sha256Digest>,
}

fn recover_applied_module_rollback(
    storage: &Storage,
    binding_id: &ModuleBindingId,
    approval: &lorepia_orchestration::ModuleActivationApproval,
) -> CoreResult<Option<RecoveredModuleRollback>> {
    let Some(recovered) = recover_applied_module_activation(storage, binding_id, approval)? else {
        return Ok(None);
    };
    let connection = storage.connection()?;
    let payload_json = connection
        .query_row(
            "SELECT audit.payload_json
             FROM module_activation_plans AS plan
             JOIN module_activation_audit AS audit
               ON audit.activation_plan_id = plan.id
             WHERE plan.plan_sha256 = ?1
               AND plan.approval_id = ?2
               AND plan.activation_binding_id = ?3
               AND audit.sequence = 1
               AND audit.plan_revision = 1
               AND audit.event_kind = 'prepared'",
            params![
                recovered.approved.plan.plan_sha256.as_str(),
                recovered.approved.approval_id.as_str(),
                binding_id.as_str(),
            ],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            storage_corrupted("applied module activation has no prepared audit authority")
        })?;
    let payload: ModuleActivationPreparedAuditPayload =
        decode_document("applied module rollback audit", &payload_json)?;
    if payload.review_sha256 != recovered.review.review_sha256
        || payload.plan_sha256 != recovered.approved.plan.plan_sha256
        || payload.binding_id != *binding_id
    {
        return Err(storage_corrupted(
            "applied module rollback audit disagrees with its activation authority",
        ));
    }
    let (rollback, approval_sha256) = match (payload.rollback, payload.rollback_approval_sha256) {
        (Some(rollback), Some(approval_sha256)) => (rollback, approval_sha256),
        (None, None) => {
            return Err(CoreError::invalid(
                "module activation approval identity belongs to a non-rollback activation",
            ));
        }
        _ => {
            return Err(storage_corrupted(
                "applied module rollback audit has incomplete rollback authority",
            ));
        }
    };
    let approved = lorepia_orchestration::ApprovedModuleRollbackPlan {
        approval_sha256,
        rollback,
        activation_review: recovered.review,
        activation: recovered.approved,
    };
    approved.verify().map_err(|error| {
        storage_corrupted(format!(
            "applied module rollback authority is invalid: {error}"
        ))
    })?;
    Ok(Some(RecoveredModuleRollback {
        approved,
        binding: recovered.binding,
    }))
}

fn apply_approved_module_rollback(
    storage: &Storage,
    approved: &lorepia_orchestration::ApprovedModuleRollbackPlan,
) -> CoreResult<StoredRevision<ModuleBinding>> {
    approved.verify().map_err(|error| {
        CoreError::invalid(format!("invalid approved module rollback: {error}"))
    })?;
    apply_approved_module_activation_internal(
        storage,
        &approved.activation_review,
        &approved.activation,
        Some(&approved.rollback),
        Some(&approved.approval_sha256),
    )
}

#[allow(clippy::too_many_lines)]
fn apply_approved_module_activation_internal(
    storage: &Storage,
    review: &lorepia_orchestration::ModuleActivationReview,
    approved: &lorepia_orchestration::ApprovedModuleActivationPlan,
    rollback: Option<&lorepia_orchestration::ModuleRollbackPlan>,
    rollback_approval_sha256: Option<&lorepia_domain::Sha256Digest>,
) -> CoreResult<StoredRevision<ModuleBinding>> {
    review.verify().map_err(|error| {
        CoreError::invalid(format!("invalid module activation review: {error}"))
    })?;
    approved
        .verify()
        .map_err(|error| CoreError::invalid(format!("invalid module activation plan: {error}")))?;
    if approved.plan.review_sha256 != review.review_sha256
        || approved.plan.expected_state_revision != review.state_revision
        || approved.plan.activation_binding_ids != review.activation_binding_ids
    {
        return Err(CoreError::invalid(
            "module activation approval does not match the reviewed state",
        ));
    }
    let activation_id = review
        .activation_binding_ids
        .as_slice()
        .first()
        .ok_or_else(|| CoreError::invalid("module activation requires one binding"))?;
    if review.activation_binding_ids.len() != 1 {
        return Err(CoreError::invalid(
            "module activation requires exactly one binding",
        ));
    }
    let proposed = review
        .ordered_bindings
        .iter()
        .find(|binding| &binding.id == activation_id)
        .cloned()
        .ok_or_else(|| {
            CoreError::invalid("activation binding is not effective in the reviewed context")
        })?;

    let resolution_set = module_activation_resolution_set(review, &approved.plan)?;
    let reconstructed = lorepia_orchestration::resolve_module_merge(review, &resolution_set)
        .map_err(|error| {
            CoreError::invalid(format!(
                "module activation plan is not review-derived: {error}"
            ))
        })?;
    if reconstructed != approved.plan {
        return Err(CoreError::invalid(
            "module activation plan differs from the reviewed resolution",
        ));
    }

    let verified_authorities = verify_module_import_authorities(storage, &review.ordered_bindings)?;

    let mut connection = storage.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_db_error)?;

    if let Some(state) = transaction
        .query_row(
            "SELECT state FROM module_activation_plans WHERE plan_sha256 = ?1",
            [approved.plan.plan_sha256.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?
    {
        if state != "applied" {
            return Err(CoreError::invalid(
                "module activation plan already exists in a nonterminal state",
            ));
        }
        let row = transaction
            .query_row(
                "SELECT document_json, revision, created_at, updated_at, deleted_at
                 FROM content_module_bindings WHERE id = ?1",
                [activation_id.as_str()],
                module_binding_row,
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| storage_corrupted("applied activation binding is missing"))?;
        let stored = decode_stored_document::<ModuleBinding>("module binding", row)?;
        if stored.deleted_at.is_some()
            || !stored.value.enabled
            || !stored.value.approved
            || stored.value.activation_approval_id.as_deref() != Some(approved.approval_id.as_str())
            || stored.value.activation_review_sha256.as_ref() != Some(&review.review_sha256)
            || stored.value.activation_plan_sha256.as_ref() != Some(&approved.plan.plan_sha256)
        {
            return Err(storage_corrupted(
                "applied module activation does not match its durable binding",
            ));
        }
        persist_initial_applied_module_runtime_plan(
            storage,
            &transaction,
            approved,
            &review.context,
            stored.updated_at,
            &verified_authorities,
        )?;
        transaction.commit().map_err(storage_db_error)?;
        return Ok(stored);
    }

    let current_rows = list_all_module_bindings_transaction(&transaction)?;
    let current_target = current_rows
        .iter()
        .find(|stored| stored.value.id == *activation_id);
    let old_binding = current_target.map(|stored| stored.value.clone());
    if let Some(rollback) = rollback {
        let current = current_target.ok_or_else(|| not_found("module rollback binding"))?;
        if current.revision != rollback.expected_state_revision
            || current.value.id != rollback.binding_id
            || current.value.revision_id != rollback.expected_current_revision_id
        {
            return Err(revision_conflict(
                "module rollback binding",
                rollback.binding_id.as_str(),
                Some(rollback.expected_state_revision),
                Some(current.revision),
            ));
        }
        let current_snapshot = load_content_module_revision(
            &transaction,
            &current.value.module_id,
            rollback.expected_current_revision_id.as_str(),
        )?;
        let target_snapshot = load_content_module_revision(
            &transaction,
            &current.value.module_id,
            rollback.target_revision_id.as_str(),
        )?;
        if current_snapshot.module_revision.source_hash != rollback.expected_current_source_sha256
            || target_snapshot.module_revision.source_hash != rollback.target_source_sha256
        {
            return Err(CoreError::invalid(
                "module rollback source hash changed after review",
            ));
        }
        let diff = lorepia_orchestration::diff_module_revisions(
            &lorepia_orchestration::ModuleRevisionSnapshot {
                module: current_snapshot.object.value.clone(),
                revision: current_snapshot.module_revision.clone(),
                import_approval: None,
            },
            &lorepia_orchestration::ModuleRevisionSnapshot {
                module: target_snapshot.object.value.clone(),
                revision: target_snapshot.module_revision.clone(),
                import_approval: None,
            },
        )
        .map_err(|error| {
            CoreError::invalid(format!(
                "cannot revalidate approved module rollback: {error}"
            ))
        })?;
        if diff.diff_sha256 != rollback.diff_sha256 {
            return Err(CoreError::invalid(
                "module rollback diff changed after approval",
            ));
        }
        let target_is_ancestor = transaction
            .query_row(
                "WITH RECURSIVE ancestors(revision_id, previous_revision_id) AS (
                     SELECT revision_id, previous_revision_id
                     FROM content_module_revisions
                     WHERE module_id = ?1 AND revision_id = ?2
                     UNION
                     SELECT parent.revision_id, parent.previous_revision_id
                     FROM content_module_revisions AS parent
                     JOIN ancestors AS child
                       ON child.previous_revision_id = parent.revision_id
                     WHERE parent.module_id = ?1
                 )
                 SELECT EXISTS(
                     SELECT 1 FROM ancestors WHERE revision_id = ?3
                 )",
                params![
                    current.value.module_id.as_str(),
                    rollback.expected_current_revision_id.as_str(),
                    rollback.target_revision_id.as_str(),
                ],
                |row| row.get::<_, bool>(0),
            )
            .map_err(storage_db_error)?;
        if !target_is_ancestor {
            return Err(CoreError::invalid(
                "approved module rollback target is not an ancestor",
            ));
        }
    }
    let expected_revision = if review.state_revision == 0 {
        if current_target.is_some() {
            return Err(revision_conflict(
                "module activation binding",
                activation_id.as_str(),
                None,
                current_target.map(|stored| stored.revision),
            ));
        }
        None
    } else {
        let current = current_target.ok_or_else(|| {
            revision_conflict(
                "module activation binding",
                activation_id.as_str(),
                Some(review.state_revision),
                None,
            )
        })?;
        if current.revision != review.state_revision
            || current.value.created_at != proposed.created_at
        {
            return Err(revision_conflict(
                "module activation binding",
                activation_id.as_str(),
                Some(review.state_revision),
                Some(current.revision),
            ));
        }
        Some(review.state_revision)
    };

    let mut current_bindings = current_rows
        .iter()
        .map(|stored| resolve_module_binding_revision(&transaction, &stored.value))
        .collect::<CoreResult<Vec<_>>>()?;
    let resolved_proposed = resolve_module_binding_revision(&transaction, &proposed)?;
    if resolved_proposed != proposed {
        return Err(CoreError::invalid(
            "module activation revision changed after review",
        ));
    }
    let mut snapshot_bindings = current_bindings.clone();
    if let Some(position) = snapshot_bindings
        .iter()
        .position(|binding| binding.id == proposed.id)
    {
        snapshot_bindings[position] = proposed.clone();
    } else {
        snapshot_bindings.push(proposed.clone());
    }
    let snapshots = module_activation_snapshots(
        storage,
        &transaction,
        &snapshot_bindings,
        &verified_authorities,
    )?;
    let rereview = lorepia_orchestration::review_module_activation(
        expected_revision,
        &review.context,
        &current_bindings,
        &proposed,
        &snapshots,
    )
    .map_err(|error| CoreError::invalid(format!("module activation review is stale: {error}")))?;
    if &rereview != review {
        return Err(CoreError::invalid(
            "module activation candidates changed after review",
        ));
    }

    let now = Utc::now();
    let now_text = now.to_rfc3339();
    let activation_plan_id = Uuid::new_v4().to_string();
    let targets = module_binding_targets(&proposed)?;
    let input_module_revisions = snapshots
        .iter()
        .map(|snapshot| {
            serde_json::json!({
                "module_id": snapshot.revision.module_id,
                "revision_id": snapshot.revision.id,
                "source_sha256": snapshot.revision.source_hash,
            })
        })
        .collect::<Vec<_>>();
    let input_module_revisions_json =
        serde_json::to_string(&input_module_revisions).map_err(|error| {
            CoreError::invalid(format!("cannot encode activation revisions: {error}"))
        })?;
    let conflicts_json = serde_json::to_string(&review.conflicts).map_err(|error| {
        CoreError::invalid(format!("cannot encode activation conflicts: {error}"))
    })?;
    let resolutions_json = serde_json::to_string(&resolution_set.resolutions).map_err(|error| {
        CoreError::invalid(format!("cannot encode activation resolutions: {error}"))
    })?;
    let review_json = serde_json::to_string(review).map_err(|error| {
        CoreError::invalid(format!("cannot encode module activation review: {error}"))
    })?;
    let approved_plan_json = serde_json::to_string(approved).map_err(|error| {
        CoreError::invalid(format!("cannot encode approved module activation: {error}"))
    })?;
    validate_json_bounds("module activation revisions", &input_module_revisions_json)?;
    validate_json_bounds("module activation conflicts", &conflicts_json)?;
    validate_json_bounds("module activation resolutions", &resolutions_json)?;
    validate_json_bounds("module activation review", &review_json)?;
    validate_json_bounds("approved module activation", &approved_plan_json)?;
    let merge_sha256 = sha256_hex(resolutions_json.as_bytes());

    stale_affected_module_activation_plans(
        &transaction,
        old_binding.as_ref(),
        Some(&proposed),
        if rollback.is_some() {
            "binding_rollback"
        } else {
            "binding_activation"
        },
        Some(&approved.plan.plan_sha256),
        &now_text,
    )?;
    transaction
        .execute(
            "INSERT INTO module_activation_plans
             (id, scope_kind, expected_bindings_revision_sha256,
              input_module_revisions_json, conflicts_json, resolutions_json,
              merge_sha256, plan_sha256, activation_binding_id, review_json,
              approved_plan_json, approval_id, approval_sha256, state,
              revision, prepared_at, approved_at, applied_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                     ?13, 'prepared', 1, ?14, NULL, NULL)",
            params![
                activation_plan_id,
                targets.scope_kind,
                review.review_sha256.as_str(),
                input_module_revisions_json,
                conflicts_json,
                resolutions_json,
                merge_sha256,
                approved.plan.plan_sha256.as_str(),
                activation_id.as_str(),
                review_json,
                approved_plan_json,
                approved.approval_id,
                approved.approval_sha256.as_str(),
                now_text,
            ],
        )
        .map_err(storage_db_error)?;
    for (ordinal, conflict) in review.conflicts.iter().enumerate() {
        let (component_kind, component_key) = module_component_storage_key(&conflict.component);
        let selected = resolution_set
            .resolutions
            .iter()
            .find(|resolution| resolution.component == conflict.component)
            .and_then(|resolution| resolution.selected.as_ref());
        let expected_json = serde_json::to_string(&conflict.candidates).map_err(|error| {
            CoreError::invalid(format!("cannot encode activation candidates: {error}"))
        })?;
        let selected_json = selected
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| {
                CoreError::invalid(format!("cannot encode selected module candidate: {error}"))
            })?;
        let resolution_sha256 = sha256_hex(
            serde_json::json!({
                "expected": conflict.candidates,
                "selected": selected,
            })
            .to_string()
            .as_bytes(),
        );
        transaction
            .execute(
                "INSERT INTO module_conflict_resolutions
                 (activation_plan_id, ordinal, component_kind, component_key,
                  expected_candidates_json, selected_candidate_json,
                  resolution_sha256)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    activation_plan_id,
                    usize_to_i64(ordinal, "module conflict ordinal")?,
                    component_kind,
                    component_key,
                    expected_json,
                    selected_json,
                    resolution_sha256,
                ],
            )
            .map_err(storage_db_error)?;
    }
    insert_module_activation_audit(
        &transaction,
        &activation_plan_id,
        1,
        1,
        "prepared",
        &serde_json::json!({
            "review_sha256": review.review_sha256,
            "plan_sha256": approved.plan.plan_sha256,
            "binding_id": activation_id,
            "rollback": rollback,
            "rollback_approval_sha256": rollback_approval_sha256,
        }),
        &now_text,
    )?;
    transaction
        .execute(
            "UPDATE module_activation_plans
             SET state = 'approved', revision = 2, approved_at = ?2
             WHERE id = ?1 AND state = 'prepared' AND revision = 1",
            params![activation_plan_id, now_text],
        )
        .map_err(storage_db_error)?;
    insert_module_activation_audit(
        &transaction,
        &activation_plan_id,
        2,
        2,
        "approved",
        &serde_json::json!({
            "approval_id": approved.approval_id,
            "approval_sha256": approved.approval_sha256,
        }),
        &now_text,
    )?;

    let mut activated = proposed;
    activated.enabled = true;
    activated.approved = true;
    activated.activation_approval_id = Some(approved.approval_id.clone());
    activated.activation_review_sha256 = Some(review.review_sha256.clone());
    activated.activation_plan_sha256 = Some(approved.plan.plan_sha256.clone());
    let stored = write_module_binding_transaction(&transaction, &activated, expected_revision)?;

    let changed = transaction
        .execute(
            "UPDATE module_activation_plans
             SET state = 'applied', revision = 3, applied_at = ?2
             WHERE id = ?1 AND state = 'approved' AND revision = 2",
            params![activation_plan_id, now_text],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(storage_corrupted(
            "module activation plan could not enter applied state",
        ));
    }
    insert_module_activation_audit(
        &transaction,
        &activation_plan_id,
        3,
        3,
        "applied",
        &serde_json::json!({
            "binding_id": activation_id,
            "binding_revision": stored.revision,
            "module_revision_id": stored.value.revision_id,
        }),
        &now_text,
    )?;
    persist_initial_applied_module_runtime_plan(
        storage,
        &transaction,
        approved,
        &review.context,
        now,
        &verified_authorities,
    )?;
    transaction.commit().map_err(storage_db_error)?;
    current_bindings.clear();
    Ok(stored)
}

fn persist_initial_applied_module_runtime_plan(
    storage: &Storage,
    transaction: &Transaction<'_>,
    approved: &lorepia_orchestration::ApprovedModuleActivationPlan,
    context: &lorepia_orchestration::ModuleResolutionContext,
    created_at: DateTime<Utc>,
    verified_authorities: &VerifiedCompletedPackageAuthorities,
) -> CoreResult<lorepia_orchestration::AppliedModuleRuntimePlan> {
    let current_rows = list_all_module_bindings_transaction(transaction)?;
    let current_bindings = current_rows
        .iter()
        .map(|stored| resolve_module_binding_revision(transaction, &stored.value))
        .collect::<CoreResult<Vec<_>>>()?;
    let snapshots = module_activation_snapshots(
        storage,
        transaction,
        &current_bindings,
        verified_authorities,
    )?;
    let current_review =
        lorepia_orchestration::review_module_merge(0, context, &current_bindings, &snapshots)
            .map_err(|error| {
                CoreError::invalid(format!(
                    "cannot review newly applied module runtime plan: {error}"
                ))
            })?;
    let runtime =
        lorepia_orchestration::materialize_approved_module_runtime_plan(approved, &current_review)
            .map_err(|error| {
                CoreError::invalid(format!(
                    "cannot materialize newly applied module runtime plan: {error}"
                ))
            })?;
    persist_applied_module_runtime_plan_transaction(transaction, &runtime, created_at)?;
    Ok(runtime)
}
