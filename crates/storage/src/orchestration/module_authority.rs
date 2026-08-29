//! Package-authority snapshots used by module resolution transactions.

use std::collections::{BTreeMap, BTreeSet};

use lorepia_domain::{CoreError, CoreResult, ModuleBinding};
use rusqlite::Transaction;

use crate::{
    database::{Storage, storage_db_error},
    package_repository::VerifiedCompletedPackageAuthorities,
};

use super::{
    list_all_module_bindings_transaction, load_content_module_revision,
    resolve_module_binding_revision, validate_module_resolution_context_authority,
};

/// Verifies package-backed module authorities before any caller opens the
/// transaction that will consume them. The preliminary DB read includes all
/// current bindings; reviewed bindings add a not-yet-persisted activation.
pub(crate) fn verify_module_import_authorities(
    storage: &Storage,
    reviewed_bindings: &[ModuleBinding],
) -> CoreResult<VerifiedCompletedPackageAuthorities> {
    let mut approval_ids = {
        let connection = storage.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT DISTINCT package_import_approval_id
                 FROM content_module_bindings
                 WHERE deleted_at IS NULL
                   AND package_import_approval_id IS NOT NULL
                 ORDER BY package_import_approval_id",
            )
            .map_err(storage_db_error)?;
        let approval_ids = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(storage_db_error)?
            .collect::<Result<BTreeSet<_>, _>>()
            .map_err(storage_db_error)?;
        approval_ids
    };
    approval_ids.extend(
        reviewed_bindings
            .iter()
            .filter_map(|binding| binding.package_import_approval_id.clone()),
    );
    storage.verify_completed_package_authorities(approval_ids.iter().map(String::as_str))
}

pub(super) fn module_activation_snapshots(
    storage: &Storage,
    transaction: &Transaction<'_>,
    bindings: &[ModuleBinding],
    verified_authorities: &VerifiedCompletedPackageAuthorities,
) -> CoreResult<Vec<lorepia_orchestration::ModuleRevisionSnapshot>> {
    let mut revision_approvals = BTreeMap::<(String, String), Option<String>>::new();
    let mut snapshots = Vec::new();
    for binding in bindings {
        let key = (
            binding.module_id.as_str().to_owned(),
            binding.revision_id.as_str().to_owned(),
        );
        if let Some(existing) = revision_approvals.get(&key) {
            if existing != &binding.package_import_approval_id {
                return Err(CoreError::invalid(
                    "the same module revision is bound to different package import approvals",
                ));
            }
            continue;
        }
        let stored = load_content_module_revision(
            transaction,
            &binding.module_id,
            binding.revision_id.as_str(),
        )?;
        let import_approval = binding
            .package_import_approval_id
            .as_deref()
            .map(|approval_id| {
                storage.get_module_import_approval_evidence_in_transaction(
                    transaction,
                    approval_id,
                    &stored,
                    verified_authorities,
                )
            })
            .transpose()?;
        snapshots.push(lorepia_orchestration::ModuleRevisionSnapshot {
            module: stored.object.value,
            revision: stored.module_revision,
            import_approval,
        });
        revision_approvals.insert(key, binding.package_import_approval_id.clone());
    }
    Ok(snapshots)
}

pub(crate) fn validate_fresh_module_merge_review(
    storage: &Storage,
    transaction: &Transaction<'_>,
    review: &lorepia_orchestration::ModuleMergeReview,
    verified_authorities: &VerifiedCompletedPackageAuthorities,
) -> CoreResult<()> {
    validate_module_resolution_context_authority(transaction, &review.context)?;
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
    let rereview = lorepia_orchestration::review_module_merge(
        review.state_revision,
        &review.context,
        &current_bindings,
        &snapshots,
    )
    .map_err(|error| CoreError::invalid(format!("current module review is stale: {error}")))?;
    if &rereview != review {
        return Err(CoreError::invalid(
            "module runtime review does not match durable bindings",
        ));
    }
    Ok(())
}
