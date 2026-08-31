use chrono::{DateTime, Utc};
use lorepia_content::select_content_package_components;
use lorepia_domain::{
    ContentCapability, CoreError, CoreErrorCode, CoreResult, ImportLimits, PackageId, Sha256Digest,
};
use lorepia_orchestration::{ApprovedPackageImportPlan, PackageReview, SelectiveImportPlan};
use lorepia_storage::{
    CompletedPackageAuthority, PackageCapability, PackageDocumentCommitBinding,
    PackageImportExpectation, PackageImportRecord, PackageImportTargetReview,
    PackageInspectionExpectation, PackageNormalizationEvidence, PackageSourceRecord,
    PackageUpdateTargetConfirmation,
};
use serde::{Deserialize, Serialize};

use super::inspect::{
    ContentPackageImportInspection, OwnedContentPackageSnapshot, reopen_content_package,
    stale_package_review, validate_import_id,
};
use crate::Core;

/// Exact discard expectation for either an inspected or selected import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentPackageDiscardRequest {
    pub expected_revision: u64,
    pub expected_review_sha256: Sha256Digest,
    pub expected_import_plan_sha256: Option<Sha256Digest>,
    pub expected_capability_review_sha256: String,
}
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StoredPackageApprovalPayload {
    pub(super) plan: ApprovedPackageImportPlan,
    pub(super) document_bindings: Vec<PackageDocumentCommitBinding>,
    target_review_sha256: String,
    confirmed_update_targets: Vec<PackageUpdateTargetConfirmation>,
    pub(super) approved_capabilities: Vec<PackageCapability>,
    pub(super) normalization_evidence_sha256: String,
    pub(super) normalization_evidence: Vec<PackageNormalizationEvidence>,
}
pub(super) struct DurableContentPackageImport {
    pub(super) source: PackageSourceRecord,
    pub(super) record: PackageImportRecord,
    pub(super) owned: OwnedContentPackageSnapshot,
}
impl Core {
    /// Reopens an existing durable inspection without consulting the original
    /// caller path. This is the restart/continuation surface for native UI.
    pub fn get_content_package_import_inspection(
        &self,
        import_id: &str,
    ) -> CoreResult<ContentPackageImportInspection> {
        let loaded = load_durable_content_package(self, import_id, ImportLimits::default())?;
        let capability_review = self.storage().get_package_capability_review(import_id)?;
        loaded
            .owned
            .public_inspection(loaded.record.revision, capability_review)
    }
    /// Resolves an opaque approval id into exact, completed package authority.
    ///
    /// Approved-but-uncommitted, discarded, stale, or tampered imports fail
    /// closed. This safe projection is the only package authority that content
    /// module activation may consume.
    pub fn get_completed_content_package_authority(
        &self,
        approval_id: &str,
    ) -> CoreResult<CompletedPackageAuthority> {
        self.storage()
            .get_completed_package_authority_by_approval_id(approval_id)
    }
    pub fn get_content_package_import(&self, import_id: &str) -> CoreResult<PackageImportRecord> {
        validate_import_id(import_id)?;
        self.storage().get_package_import(import_id)
    }
    pub fn list_content_package_imports(
        &self,
        package_id: Option<&PackageId>,
    ) -> CoreResult<Vec<PackageImportRecord>> {
        self.storage().list_package_imports(package_id)
    }
    /// Discards either the unselected inspection or the exact selected plan.
    pub fn discard_content_package_import(
        &self,
        import_id: &str,
        request: &ContentPackageDiscardRequest,
    ) -> CoreResult<PackageImportRecord> {
        validate_import_id(import_id)?;
        let record = self.storage().get_package_import(import_id)?;
        let inspection = PackageInspectionExpectation {
            revision: request.expected_revision,
            inspection_sha256: request.expected_review_sha256.as_str().to_owned(),
            capability_review_sha256: request.expected_capability_review_sha256.clone(),
        };
        if record.selection.is_none() {
            if request.expected_import_plan_sha256.is_some() {
                return Err(stale_package_review());
            }
            self.storage()
                .discard_inspected_package_import(import_id, &inspection)
        } else {
            let selection = request
                .expected_import_plan_sha256
                .as_ref()
                .ok_or_else(stale_package_review)?;
            self.storage().discard_package_import(
                import_id,
                &PackageImportExpectation {
                    revision: request.expected_revision,
                    inspection_sha256: request.expected_review_sha256.as_str().to_owned(),
                    selection_sha256: selection.as_str().to_owned(),
                    capability_review_sha256: request.expected_capability_review_sha256.clone(),
                },
            )
        }
    }
}

pub(super) fn load_durable_content_package(
    core: &Core,
    import_id: &str,
    limits: ImportLimits,
) -> CoreResult<DurableContentPackageImport> {
    validate_import_id(import_id)?;
    let record = core.storage().get_package_import(import_id)?;
    if record.inspection.schema_version != 1 {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "stored package inspection schema is unsupported",
            false,
        ));
    }
    let review: PackageReview =
        serde_json::from_value(record.inspection.value.clone()).map_err(|error| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                format!("stored package inspection cannot be decoded: {error}"),
                false,
            )
        })?;
    review.verify().map_err(|error| {
        CoreError::new(
            CoreErrorCode::StorageCorrupted,
            format!("stored package review is invalid: {error}"),
            false,
        )
    })?;
    let source = core.storage().get_package_source_for_import(import_id)?;
    if source.source_sha256 != review.source_sha256.as_str()
        || source.package_id != review.manifest.package_id
    {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "stored package source and review identities disagree",
            false,
        ));
    }
    let source_path = core
        .storage()
        .package_source_path(&source.source_sha256, source.source_size_bytes)?;
    let owned = reopen_content_package(import_id, &source_path, &review, limits)?;
    Ok(DurableContentPackageImport {
        source,
        record,
        owned,
    })
}
pub(super) fn stored_import_plan(record: &PackageImportRecord) -> CoreResult<SelectiveImportPlan> {
    let selection = record.selection.as_ref().ok_or_else(|| {
        CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "stored package import has no selection",
            false,
        )
    })?;
    if selection.schema_version != 1 {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "stored package selection schema is unsupported",
            false,
        ));
    }
    let plan: SelectiveImportPlan =
        serde_json::from_value(selection.value.clone()).map_err(|error| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                format!("stored package selection cannot be decoded: {error}"),
                false,
            )
        })?;
    plan.verify().map_err(|error| {
        CoreError::new(
            CoreErrorCode::StorageCorrupted,
            format!("stored package selection is invalid: {error}"),
            false,
        )
    })?;
    Ok(plan)
}
pub(super) fn stored_package_approval(
    core: &Core,
    import_id: &str,
) -> CoreResult<(StoredPackageApprovalPayload, DateTime<Utc>)> {
    let approval = core.storage().get_package_import_approval(import_id)?;
    let approved_at = approval.approved_at;
    if approval.payload.schema_version != 1 {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "stored package approval schema is unsupported",
            false,
        ));
    }
    let payload: StoredPackageApprovalPayload = serde_json::from_value(approval.payload.value)
        .map_err(|error| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                format!("stored package approval cannot be decoded: {error}"),
                false,
            )
        })?;
    payload.plan.verify().map_err(|error| {
        CoreError::new(
            CoreErrorCode::StorageCorrupted,
            format!("stored package approval is invalid: {error}"),
            false,
        )
    })?;
    Ok((payload, approved_at))
}
pub(super) fn target_review_bindings(
    target_review: &PackageImportTargetReview,
) -> CoreResult<Vec<PackageDocumentCommitBinding>> {
    target_review.verify()?;
    Ok(target_review
        .documents
        .iter()
        .map(|document| PackageDocumentCommitBinding {
            document_index: document.document_index,
            source_component_key: document.source_component_id.clone(),
            component_document_ordinal: document.component_document_ordinal,
            source_component_sha256: document.source_component_sha256.clone(),
            target_object_id: document.target_object_id.clone(),
            document_kind: document.document_kind.clone(),
            document_sha256: document.document_sha256.clone(),
            expected_object_revision: document.expected_target_state_revision,
        })
        .collect())
}
pub(super) fn validate_package_transition_expectations(
    loaded: &DurableContentPackageImport,
    import_plan: &SelectiveImportPlan,
    expected_package_plan_hash: &str,
    expected_content_selection_plan_hash: &str,
    expected_review_sha256: &Sha256Digest,
    expected_import_plan_sha256: &Sha256Digest,
) -> CoreResult<()> {
    let content_selection = select_content_package_components(
        loaded.owned.inspection(),
        &loaded.record.selected_component_ids,
    )?;
    if loaded.owned.inspection.plan_hash != expected_package_plan_hash
        || content_selection.selection_plan_hash != expected_content_selection_plan_hash
        || loaded.owned.review.review_sha256 != *expected_review_sha256
        || import_plan.review_sha256 != *expected_review_sha256
        || import_plan.plan_sha256 != *expected_import_plan_sha256
        || import_plan.source_sha256.as_str() != loaded.source.source_sha256
    {
        return Err(stale_package_review());
    }
    Ok(())
}
pub(super) fn required_capability_approvals(plan: &SelectiveImportPlan) -> Vec<PackageCapability> {
    let mut approvals = plan
        .required_capabilities
        .iter()
        .filter_map(|capability| match capability {
            ContentCapability::Transforms => Some(PackageCapability::Transforms),
            ContentCapability::DeclarativeInteractions => {
                Some(PackageCapability::DeclarativeInteractions)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    approvals.sort_unstable();
    approvals.dedup();
    approvals
}
