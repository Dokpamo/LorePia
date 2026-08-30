//! Applied module runtime-plan materialization and durable authority.

use super::{
    BTreeMap, ContentModuleId, CoreError, CoreResult, DateTime, Deserialize, ModuleBinding,
    ModuleBindingId, OptionalExtension, Storage, Transaction, TransactionBehavior, Utc,
    decode_document, decode_stored_document, list_all_module_bindings_transaction,
    load_content_module_revision, module_activation_resolution_set, module_activation_snapshots,
    not_found, params, resolve_module_binding_revision, storage_corrupted, storage_db_error,
    validate_fresh_module_merge_review, validate_identifier, validate_json_bounds,
    validate_optional_sha256, verify_module_import_authorities,
};

impl Storage {
    /// Loads the one currently applied runtime overlay only after independently
    /// re-deriving and exact-matching the caller's fresh context review.
    pub fn get_applied_module_runtime_plan(
        &self,
        current_review: &lorepia_orchestration::ModuleMergeReview,
    ) -> CoreResult<lorepia_orchestration::AppliedModuleRuntimePlan> {
        get_applied_module_runtime_plan(self, current_review)
    }

    /// Loads an immutable applied runtime plan for a sealed historical event.
    /// A plan made stale by later binding changes remains valid historical
    /// authority, but every canonical payload and exact source revision is
    /// still verified before it is returned.
    pub fn get_historical_applied_module_runtime_plan(
        &self,
        applied_plan_sha256: &lorepia_domain::Sha256Digest,
    ) -> CoreResult<lorepia_orchestration::AppliedModuleRuntimePlan> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        let runtime = load_historical_applied_module_runtime_plan_transaction(
            &transaction,
            applied_plan_sha256,
        )?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(runtime)
    }

    /// Revalidates and uniquely materializes the active runtime authority
    /// without persisting a context row. Proposed branches use this preview
    /// before the branch exists and promote the exact returned object only in
    /// their atomic append transaction.
    pub fn preview_applied_module_runtime_plan(
        &self,
        current_review: &lorepia_orchestration::ModuleMergeReview,
    ) -> CoreResult<lorepia_orchestration::AppliedModuleRuntimePlan> {
        preview_applied_module_runtime_plan(self, current_review)
    }

    /// Revalidates a target review and derives a context-specific plan without
    /// persisting it. This is safe for a proposed branch that does not exist
    /// yet; the atomic branch append persists the returned plan only after the
    /// branch row and checkpoint have been created.
    pub fn derive_applied_module_runtime_plan(
        &self,
        source: &lorepia_orchestration::AppliedModuleRuntimePlan,
        target_review: &lorepia_orchestration::ModuleMergeReview,
    ) -> CoreResult<lorepia_orchestration::AppliedModuleRuntimePlan> {
        derive_applied_module_runtime_plan(self, source, target_review)
    }
}

fn verify_exact_applied_runtime_source(
    transaction: &Transaction<'_>,
    source: &lorepia_orchestration::ApprovedModuleActivationPlan,
) -> CoreResult<lorepia_orchestration::ModuleMergeReview> {
    verify_exact_applied_runtime_source_with_stale_authority(transaction, source, false)
}

fn verify_exact_applied_runtime_source_with_stale_authority(
    transaction: &Transaction<'_>,
    source: &lorepia_orchestration::ApprovedModuleActivationPlan,
    allow_stale: bool,
) -> CoreResult<lorepia_orchestration::ModuleMergeReview> {
    source.verify().map_err(|error| {
        storage_corrupted(format!(
            "applied runtime source approval is invalid: {error}"
        ))
    })?;
    let row = transaction
        .query_row(
            "SELECT review_json, approved_plan_json, state,
                    approval_id, approval_sha256, plan_sha256,
                    expected_bindings_revision_sha256
             FROM module_activation_plans
             WHERE plan_sha256 = ?1 AND approval_sha256 = ?2",
            params![
                source.plan.plan_sha256.as_str(),
                source.approval_sha256.as_str(),
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            CoreError::invalid("applied runtime plan source activation is not currently applied")
        })?;
    if row.2 != "applied" && !(allow_stale && row.2 == "stale") {
        return Err(CoreError::invalid(
            "applied runtime plan source activation is stale",
        ));
    }
    let review: lorepia_orchestration::ModuleMergeReview =
        decode_document("applied runtime source activation review", &row.0)?;
    let approved: lorepia_orchestration::ApprovedModuleActivationPlan =
        decode_document("applied runtime source activation approval", &row.1)?;
    review.verify().map_err(|error| {
        storage_corrupted(format!(
            "applied runtime source activation review is invalid: {error}"
        ))
    })?;
    approved.verify().map_err(|error| {
        storage_corrupted(format!(
            "applied runtime source activation approval is invalid: {error}"
        ))
    })?;
    if approved != *source
        || row.3 != source.approval_id
        || row.4 != source.approval_sha256.as_str()
        || row.5 != source.plan.plan_sha256.as_str()
        || row.6 != source.plan.review_sha256.as_str()
        || review.review_sha256 != source.plan.review_sha256
        || review.state_revision != source.plan.expected_state_revision
        || review.activation_binding_ids != source.plan.activation_binding_ids
    {
        return Err(storage_corrupted(
            "applied runtime source authority differs from its immutable activation row",
        ));
    }
    Ok(review)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModuleActivationRevisionEvidence {
    module_id: String,
    revision_id: String,
    source_sha256: String,
}

fn same_module_runtime_binding(left: &ModuleBinding, right: &ModuleBinding) -> bool {
    left.id == right.id
        && left.module_id == right.module_id
        && left.scope == right.scope
        && left.target_id == right.target_id
        && left.conversation_id == right.conversation_id
        && left.priority == right.priority
        && left.resolution_mode == right.resolution_mode
        && left.pinned_revision_id == right.pinned_revision_id
        && left.package_import_approval_id == right.package_import_approval_id
        && left.variable_overrides == right.variable_overrides
        && left.revision_id == right.revision_id
        && left.created_at == right.created_at
}

#[allow(clippy::too_many_lines)]
#[allow(dead_code)]
fn get_applied_module_runtime_plan_legacy(
    storage: &Storage,
    current_review: &lorepia_orchestration::ModuleMergeReview,
) -> CoreResult<lorepia_orchestration::ApprovedModuleActivationPlan> {
    current_review.verify().map_err(|error| {
        CoreError::invalid(format!("invalid current module runtime review: {error}"))
    })?;
    if !current_review.activation_binding_ids.is_empty() {
        return Err(CoreError::invalid(
            "runtime module review must not contain a pending activation",
        ));
    }
    let verified_authorities =
        verify_module_import_authorities(storage, &current_review.ordered_bindings)?;
    let mut connection = storage.connection()?;
    let transaction = connection.transaction().map_err(storage_db_error)?;
    let current_rows = list_all_module_bindings_transaction(&transaction)?;
    let current_bindings = current_rows
        .iter()
        .map(|stored| resolve_module_binding_revision(&transaction, &stored.value))
        .collect::<CoreResult<Vec<_>>>()?;
    let snapshots = module_activation_snapshots(
        storage,
        &transaction,
        &current_bindings,
        &verified_authorities,
    )?;
    let rereview = lorepia_orchestration::review_module_merge(
        current_review.state_revision,
        &current_review.context,
        &current_bindings,
        &snapshots,
    )
    .map_err(|error| CoreError::invalid(format!("current module review is stale: {error}")))?;
    if &rereview != current_review {
        return Err(CoreError::invalid(
            "current module review does not match durable bindings",
        ));
    }
    let matching_plan_ids = {
        let mut statement = transaction
            .prepare(
                "SELECT id, review_json
                 FROM module_activation_plans
                 WHERE state = 'applied'
                 ORDER BY applied_at, id",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?;
        rows.into_iter()
            .filter_map(|(id, review_json)| {
                let review = serde_json::from_str::<lorepia_orchestration::ModuleActivationReview>(
                    &review_json,
                )
                .ok()?;
                (review.context == current_review.context).then_some(id)
            })
            .collect::<Vec<_>>()
    };
    let matching_plan_id = match matching_plan_ids.as_slice() {
        [] => return Err(not_found("applied module runtime plan for context")),
        [id] => id,
        _ => {
            return Err(storage_corrupted(
                "multiple module runtime plans are applied to the same context",
            ));
        }
    };
    let row = transaction
        .query_row(
            "SELECT binding.document_json, binding.revision,
                    binding.created_at, binding.updated_at, binding.deleted_at,
                    plan.review_json, plan.approved_plan_json,
                    plan.input_module_revisions_json, plan.plan_sha256,
                    plan.approval_id, plan.approval_sha256,
                    plan.expected_bindings_revision_sha256,
                    plan.activation_binding_id
             FROM content_module_bindings AS binding
             JOIN module_activation_plans AS plan
              ON plan.activation_binding_id = binding.id
              AND plan.plan_sha256 = binding.activation_plan_sha256
             WHERE binding.deleted_at IS NULL
               AND binding.enabled = 1
               AND binding.approved = 1
               AND plan.state = 'applied'
               AND plan.id = ?1",
            [matching_plan_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, String>(11)?,
                    row.get::<_, String>(12)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("applied module activation plan"))?;
    let binding_id = ModuleBindingId::from(row.12.clone());
    let binding = decode_stored_document::<ModuleBinding>(
        "module binding",
        (row.0, row.1, None, row.2, row.3, row.4),
    )?;
    validate_optional_sha256("stored module activation plan hash", Some(&row.8)).map_err(
        |error| {
            storage_corrupted(format!(
                "stored module activation plan hash is invalid: {}",
                error.message
            ))
        },
    )?;
    validate_optional_sha256("stored module activation approval hash", Some(&row.10)).map_err(
        |error| {
            storage_corrupted(format!(
                "stored module activation approval hash is invalid: {}",
                error.message
            ))
        },
    )?;
    validate_json_bounds("stored module activation review", &row.5).map_err(|error| {
        storage_corrupted(format!(
            "stored module activation review violates bounds: {}",
            error.message
        ))
    })?;
    validate_json_bounds("stored approved module activation", &row.6).map_err(|error| {
        storage_corrupted(format!(
            "stored approved module activation violates bounds: {}",
            error.message
        ))
    })?;
    validate_json_bounds("stored module activation revisions", &row.7).map_err(|error| {
        storage_corrupted(format!(
            "stored module activation revisions violate bounds: {}",
            error.message
        ))
    })?;
    let review: lorepia_orchestration::ModuleActivationReview = serde_json::from_str(&row.5)
        .map_err(|error| {
            storage_corrupted(format!(
                "stored module activation review is invalid: {error}"
            ))
        })?;
    let approved: lorepia_orchestration::ApprovedModuleActivationPlan =
        serde_json::from_str(&row.6).map_err(|error| {
            storage_corrupted(format!(
                "stored approved module activation is invalid: {error}"
            ))
        })?;
    let revision_evidence: Vec<ModuleActivationRevisionEvidence> = serde_json::from_str(&row.7)
        .map_err(|error| {
            storage_corrupted(format!(
                "stored module activation revision evidence is invalid: {error}"
            ))
        })?;
    review.verify().map_err(|error| {
        storage_corrupted(format!(
            "stored module activation review failed verification: {error}"
        ))
    })?;
    approved.verify().map_err(|error| {
        storage_corrupted(format!(
            "stored approved module activation failed verification: {error}"
        ))
    })?;
    if approved.plan.review_sha256 != review.review_sha256
        || approved.plan.plan_sha256.as_str() != row.8
        || approved.approval_id != row.9
        || approved.approval_sha256.as_str() != row.10
        || review.review_sha256.as_str() != row.11
        || binding_id.as_str() != row.12
        || review.activation_binding_ids.as_slice() != [binding_id.clone()]
        || binding.value.activation_approval_id.as_deref() != Some(row.9.as_str())
        || binding.value.activation_review_sha256.as_ref() != Some(&review.review_sha256)
        || binding.value.activation_plan_sha256.as_ref() != Some(&approved.plan.plan_sha256)
    {
        return Err(storage_corrupted(
            "stored module activation plan, approval, and binding disagree",
        ));
    }
    let resolution_set = module_activation_resolution_set(&review, &approved.plan)?;
    let reconstructed = lorepia_orchestration::resolve_module_merge(&review, &resolution_set)
        .map_err(|error| {
            storage_corrupted(format!(
                "stored module activation plan is not review-derived: {error}"
            ))
        })?;
    if reconstructed != approved.plan {
        return Err(storage_corrupted(
            "stored module activation plan differs from its reviewed resolution",
        ));
    }
    if review.context != current_review.context
        || review.ignored_bindings != current_review.ignored_bindings
        || review.ordered_bindings.len() != current_review.ordered_bindings.len()
        || !review
            .ordered_bindings
            .iter()
            .zip(&current_review.ordered_bindings)
            .all(|(reviewed, current)| same_module_runtime_binding(reviewed, current))
    {
        return Err(CoreError::invalid(
            "applied module plan does not match the current context and binding set",
        ));
    }
    let current_resolution_set = module_activation_resolution_set(current_review, &approved.plan)?;
    let current_plan =
        lorepia_orchestration::resolve_module_merge(current_review, &current_resolution_set)
            .map_err(|error| {
                CoreError::invalid(format!(
                    "applied module selection is stale for the current context: {error}"
                ))
            })?;
    if current_plan.ordered_binding_ids != approved.plan.ordered_binding_ids
        || current_plan.components != approved.plan.components
        || current_plan.omitted_components != approved.plan.omitted_components
        || current_plan.effective_variable_overrides != approved.plan.effective_variable_overrides
    {
        return Err(CoreError::invalid(
            "applied module components are stale for the current context",
        ));
    }

    let mut persisted_snapshots = BTreeMap::new();
    for evidence in revision_evidence {
        validate_identifier("module activation module", &evidence.module_id)?;
        validate_identifier("module activation revision", &evidence.revision_id)?;
        validate_optional_sha256(
            "module activation revision source hash",
            Some(&evidence.source_sha256),
        )
        .map_err(|error| {
            storage_corrupted(format!(
                "stored activation revision source hash is invalid: {}",
                error.message
            ))
        })?;
        let key = (evidence.module_id.clone(), evidence.revision_id.clone());
        if persisted_snapshots.contains_key(&key) {
            return Err(storage_corrupted(
                "stored module activation revision evidence is duplicated",
            ));
        }
        let snapshot = load_content_module_revision(
            &transaction,
            &ContentModuleId::from(evidence.module_id),
            &evidence.revision_id,
        )?;
        if snapshot.module_revision.source_hash.as_str() != evidence.source_sha256 {
            return Err(storage_corrupted(
                "module revision source changed after activation approval",
            ));
        }
        persisted_snapshots.insert(key, snapshot.module_revision);
    }
    for component in &approved.plan.components {
        for source in
            std::iter::once(&component.selected_source).chain(component.coalesced_sources.iter())
        {
            let revision = persisted_snapshots
                .get(&(
                    source.module_id.as_str().to_owned(),
                    source.revision_id.as_str().to_owned(),
                ))
                .ok_or_else(|| {
                    storage_corrupted("approved module component lacks exact revision evidence")
                })?;
            if revision.source_hash != source.revision_source_sha256 {
                return Err(storage_corrupted(
                    "approved module component source hash is stale",
                ));
            }
            let component_hash = revision
                .component_hashes
                .iter()
                .find(|hash| hash.component == component.component)
                .ok_or_else(|| {
                    storage_corrupted(
                        "approved module component is missing from its immutable revision",
                    )
                })?;
            if component_hash.sha256 != component.sha256 {
                return Err(storage_corrupted("approved module component hash is stale"));
            }
        }
    }
    transaction.commit().map_err(storage_db_error)?;
    Ok(approved)
}

fn get_applied_module_runtime_plan(
    storage: &Storage,
    current_review: &lorepia_orchestration::ModuleMergeReview,
) -> CoreResult<lorepia_orchestration::AppliedModuleRuntimePlan> {
    let runtime = preview_applied_module_runtime_plan(storage, current_review)?;
    let verified_authorities =
        verify_module_import_authorities(storage, &current_review.ordered_bindings)?;
    let mut connection = storage.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_db_error)?;
    validate_fresh_module_merge_review(
        storage,
        &transaction,
        current_review,
        &verified_authorities,
    )?;
    persist_applied_module_runtime_plan_transaction(&transaction, &runtime, Utc::now())?;
    transaction.commit().map_err(storage_db_error)?;
    Ok(runtime)
}

fn preview_applied_module_runtime_plan(
    storage: &Storage,
    current_review: &lorepia_orchestration::ModuleMergeReview,
) -> CoreResult<lorepia_orchestration::AppliedModuleRuntimePlan> {
    current_review.verify().map_err(|error| {
        CoreError::invalid(format!("invalid current module runtime review: {error}"))
    })?;
    if !current_review.activation_binding_ids.is_empty() {
        return Err(CoreError::invalid(
            "runtime module review must not contain a pending activation",
        ));
    }
    let verified_authorities =
        verify_module_import_authorities(storage, &current_review.ordered_bindings)?;
    let mut connection = storage.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_db_error)?;
    validate_fresh_module_merge_review(
        storage,
        &transaction,
        current_review,
        &verified_authorities,
    )?;

    let candidates = {
        let mut statement = transaction
            .prepare(
                "SELECT plan_sha256, approval_sha256, approved_plan_json
                 FROM module_activation_plans
                 WHERE state = 'applied'
                 ORDER BY applied_at, id",
            )
            .map_err(storage_db_error)?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?
    };
    let mut applicable = Vec::new();
    for (plan_sha256, approval_sha256, approved_json) in candidates {
        let approved: lorepia_orchestration::ApprovedModuleActivationPlan =
            decode_document("applied module activation", &approved_json)?;
        approved.verify().map_err(|error| {
            storage_corrupted(format!(
                "stored applied module activation is invalid: {error}"
            ))
        })?;
        if approved.plan.plan_sha256.as_str() != plan_sha256
            || approved.approval_sha256.as_str() != approval_sha256
        {
            return Err(storage_corrupted(
                "stored applied module activation identity diverges",
            ));
        }
        match lorepia_orchestration::materialize_approved_module_runtime_plan(
            &approved,
            current_review,
        ) {
            Ok(runtime) => applicable.push(runtime),
            Err(
                lorepia_orchestration::ModuleMergeError::RuntimeDerivationChanged
                | lorepia_orchestration::ModuleMergeError::InvalidRuntimeMaterialization(_),
            ) => {}
            Err(error) => {
                return Err(CoreError::invalid(format!(
                    "cannot materialize applied module runtime plan: {error}"
                )));
            }
        }
    }
    let runtime = match applicable.as_slice() {
        [] => return Err(not_found("applied module runtime plan for context")),
        [runtime] => runtime.clone(),
        _ => {
            return Err(storage_corrupted(
                "multiple applied module activations select the same runtime context",
            ));
        }
    };
    transaction.commit().map_err(storage_db_error)?;
    Ok(runtime)
}

fn derive_applied_module_runtime_plan(
    storage: &Storage,
    source: &lorepia_orchestration::AppliedModuleRuntimePlan,
    target_review: &lorepia_orchestration::ModuleMergeReview,
) -> CoreResult<lorepia_orchestration::AppliedModuleRuntimePlan> {
    source.verify().map_err(|error| {
        CoreError::invalid(format!("invalid source applied module plan: {error}"))
    })?;
    target_review.verify().map_err(|error| {
        CoreError::invalid(format!("invalid target module runtime review: {error}"))
    })?;
    let verified_authorities =
        verify_module_import_authorities(storage, &target_review.ordered_bindings)?;
    let mut connection = storage.connection()?;
    let transaction = connection.transaction().map_err(storage_db_error)?;
    let stored_source =
        load_applied_module_runtime_plan_transaction(&transaction, &source.applied_plan_sha256)?;
    if &stored_source != source {
        return Err(CoreError::invalid(
            "source applied module runtime plan differs from durable authority",
        ));
    }
    validate_fresh_module_merge_review(
        storage,
        &transaction,
        target_review,
        &verified_authorities,
    )?;
    let derived = lorepia_orchestration::derive_applied_module_runtime_plan(source, target_review)
        .map_err(|error| {
            CoreError::invalid(format!("cannot derive module runtime plan: {error}"))
        })?;
    transaction.commit().map_err(storage_db_error)?;
    Ok(derived)
}

pub(crate) fn persist_applied_module_runtime_plan_transaction(
    transaction: &Transaction<'_>,
    runtime: &lorepia_orchestration::AppliedModuleRuntimePlan,
    created_at: DateTime<Utc>,
) -> CoreResult<()> {
    runtime.verify().map_err(|error| {
        CoreError::invalid(format!("invalid applied module runtime plan: {error}"))
    })?;
    verify_exact_applied_runtime_source(transaction, &runtime.source_approval)?;
    if let Some(parent) = runtime.derived_from_plan_sha256.as_ref() {
        let parent_valid = transaction
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM applied_module_runtime_plans
                     WHERE applied_plan_sha256 = ?1 AND state = 'applied'
                 )",
                [parent.as_str()],
                |row| row.get::<_, bool>(0),
            )
            .map_err(storage_db_error)?;
        if !parent_valid {
            return Err(CoreError::invalid(
                "derived runtime plan parent is not currently applied",
            ));
        }
    }
    let conversation_id = runtime.review.context.conversation_id.as_deref();
    let branch_id = runtime.review.context.branch_id.as_deref();
    if conversation_id.is_some() != branch_id.is_some() {
        return Err(CoreError::invalid(
            "applied runtime plan conversation and branch context are incomplete",
        ));
    }
    let context_json = serde_json::to_string(&runtime.review.context).map_err(|error| {
        CoreError::internal(format!("cannot encode module runtime context: {error}"))
    })?;
    let runtime_json = serde_json::to_string(runtime).map_err(|error| {
        CoreError::internal(format!(
            "cannot encode applied module runtime plan: {error}"
        ))
    })?;
    validate_json_bounds("module runtime context", &context_json)?;
    validate_json_bounds("applied module runtime plan", &runtime_json)?;
    let inserted = transaction
        .execute(
            "INSERT OR IGNORE INTO applied_module_runtime_plans
             (applied_plan_sha256, source_activation_plan_sha256,
              source_approval_sha256, derived_from_plan_sha256,
              conversation_id, branch_id, review_sha256, context_json,
              runtime_plan_json, state, created_at, stale_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                     'applied', ?10, NULL)",
            params![
                runtime.applied_plan_sha256.as_str(),
                runtime.source_approval.plan.plan_sha256.as_str(),
                runtime.source_approval.approval_sha256.as_str(),
                runtime
                    .derived_from_plan_sha256
                    .as_ref()
                    .map(lorepia_domain::Sha256Digest::as_str),
                conversation_id,
                branch_id,
                runtime.review.review_sha256.as_str(),
                context_json,
                runtime_json,
                created_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?
        == 1;
    if !inserted {
        let stored = load_applied_module_runtime_plan_transaction(
            transaction,
            &runtime.applied_plan_sha256,
        )?;
        if &stored != runtime {
            return Err(storage_corrupted(
                "applied module runtime hash was reused with different material",
            ));
        }
    }
    Ok(())
}

pub(super) fn load_applied_module_runtime_plan_transaction(
    transaction: &Transaction<'_>,
    applied_plan_sha256: &lorepia_domain::Sha256Digest,
) -> CoreResult<lorepia_orchestration::AppliedModuleRuntimePlan> {
    load_applied_module_runtime_plan_with_stale_authority(transaction, applied_plan_sha256, false)
}

fn load_historical_applied_module_runtime_plan_transaction(
    transaction: &Transaction<'_>,
    applied_plan_sha256: &lorepia_domain::Sha256Digest,
) -> CoreResult<lorepia_orchestration::AppliedModuleRuntimePlan> {
    load_applied_module_runtime_plan_with_stale_authority(transaction, applied_plan_sha256, true)
}

fn load_applied_module_runtime_plan_with_stale_authority(
    transaction: &Transaction<'_>,
    applied_plan_sha256: &lorepia_domain::Sha256Digest,
    allow_stale: bool,
) -> CoreResult<lorepia_orchestration::AppliedModuleRuntimePlan> {
    let row = transaction
        .query_row(
            "SELECT source_activation_plan_sha256, source_approval_sha256,
                    derived_from_plan_sha256, conversation_id, branch_id,
                    review_sha256, context_json, runtime_plan_json, state
             FROM applied_module_runtime_plans
             WHERE applied_plan_sha256 = ?1",
            [applied_plan_sha256.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("applied module runtime plan"))?;
    if row.8 != "applied" && !(allow_stale && row.8 == "stale") {
        return Err(CoreError::invalid("applied module runtime plan is stale"));
    }
    let runtime: lorepia_orchestration::AppliedModuleRuntimePlan =
        decode_document("applied module runtime plan", &row.7)?;
    let context: lorepia_orchestration::ModuleResolutionContext =
        decode_document("applied module runtime context", &row.6)?;
    runtime.verify().map_err(|error| {
        storage_corrupted(format!(
            "stored applied module runtime plan is invalid: {error}"
        ))
    })?;
    let canonical_runtime_json = serde_json::to_string(&runtime).map_err(|error| {
        CoreError::internal(format!(
            "cannot re-encode applied module runtime plan: {error}"
        ))
    })?;
    let canonical_context_json = serde_json::to_string(&context).map_err(|error| {
        CoreError::internal(format!(
            "cannot re-encode applied module runtime context: {error}"
        ))
    })?;
    if &runtime.applied_plan_sha256 != applied_plan_sha256
        || runtime.source_approval.plan.plan_sha256.as_str() != row.0
        || runtime.source_approval.approval_sha256.as_str() != row.1
        || runtime
            .derived_from_plan_sha256
            .as_ref()
            .map(lorepia_domain::Sha256Digest::as_str)
            != row.2.as_deref()
        || runtime.review.context != context
        || runtime.review.context.conversation_id.as_deref() != row.3.as_deref()
        || runtime.review.context.branch_id.as_deref() != row.4.as_deref()
        || runtime.review.review_sha256.as_str() != row.5
        || canonical_context_json != row.6
        || canonical_runtime_json != row.7
    {
        return Err(storage_corrupted(
            "applied module runtime plan authority columns diverge from its canonical payload",
        ));
    }
    verify_exact_applied_runtime_source_with_stale_authority(
        transaction,
        &runtime.source_approval,
        allow_stale,
    )?;
    if let Some(parent) = row.2.as_deref() {
        let parent_is_applied = transaction
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM applied_module_runtime_plans
                     WHERE applied_plan_sha256 = ?1
                       AND (state = 'applied' OR (?2 AND state = 'stale'))
                 )",
                params![parent, allow_stale],
                |row| row.get::<_, bool>(0),
            )
            .map_err(storage_db_error)?;
        if !parent_is_applied {
            return Err(CoreError::invalid("derived module runtime parent is stale"));
        }
    }
    Ok(runtime)
}
