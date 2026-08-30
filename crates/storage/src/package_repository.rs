use std::collections::{BTreeMap, BTreeSet};
#[cfg(test)]
use std::{fs, path::PathBuf};

mod approval;
mod approval_validation;
mod commit;
mod commit_validation;
mod completed_authority;
mod completed_authority_query;
mod contract_codec;
mod inspection;
mod lifecycle;
mod module_authority;
mod module_authority_query;
mod queries;
mod row_mapping;
mod selection;
mod target_review;
mod types_contract;

use chrono::{DateTime, Utc};
use lorepia_domain::{
    AssetDescriptor, AssetId, ContentCapability, ControlId, CoreError, CoreErrorCode, CoreResult,
    InstructionAuthority, ModuleComponentRef, PackageId, PlacementZone, PromptBlockId,
    Sha256Digest, SourceKind, ValidateOrchestration, VersionedJson,
};
use lorepia_orchestration::{
    ApprovedPackageImportPlan, ModuleImportApprovalEvidence, ModuleImportComponentAuthority,
    PackageComponentDisposition, PackageComponentKind, PackageReview, SelectiveImportPlan,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::database::{
    StagedAssetImport, Storage, claim_package_asset_promotions, claim_package_source_promotion,
    storage_db_error,
};
use crate::orchestration::{
    ActiveContentModuleRevision, PackageCommitDocument, PackageCommitInput, PackageImportRecord,
    PackageImportStatus, PackageSourceRecord, append_package_asset_descriptor,
    append_package_commit_document,
};

use approval::PackageApprovalPayload;
pub use approval::package_normalization_evidence_sha256;
use approval_validation::{
    assert_expectation, read_approval_payload, validate_approval_replay, validate_audit_replay,
    validate_capability_approval_snapshot, validate_document_normalization_evidence,
    validate_expectation, validate_inspection_expectation, validate_normalization_evidence_linkage,
    validate_normalization_evidence_shape,
};
#[cfg(test)]
use commit_validation::validate_normalized_package_documents;
use commit_validation::{
    document_object_id, load_selected_commit_components, validate_binding_snapshot_shape,
    validate_commit_bindings, validate_commit_input_shape, validate_completed_authority_audit,
    validate_completed_commit_replay,
};
pub(crate) use completed_authority::VerifiedCompletedPackageAuthorities;
use completed_authority::{CompletedPackageAuthoritySnapshot, CompletedPackageCasFile};
use completed_authority_query::CompletedAuthorityCommitEvidence;
use contract_codec::{
    component_kind_str, decode_json, encode_json, i64_from_u64, import_status_str, license_fields,
    not_found, parse_capability_support, parse_datetime, parse_import_status,
    parse_package_capability, revision_conflict, sha256_hex, storage_corrupted, u32_from_i64,
    u64_from_i64, validate_identifier, validate_sha256,
};
use inspection::{validate_capability_review, validate_source_record};
#[cfg(test)]
use module_authority::{asset_module_component_authority, document_module_component_authority};
use module_authority::{
    build_module_import_approval_evidence_in_connection, validate_completed_module_authority_target,
};
use row_mapping::{
    StoredImportState, append_audit, insert_capability_review, insert_package_source,
    read_capability_review, read_import_state, read_package_source, read_package_source_by_id,
    read_source_hash, update_import_state,
};
use selection::{
    ReviewedComponentRow, assert_inspection_expectation, decode_selection,
    load_reviewed_components, reviewed_component_rows_sha256,
};
use target_review::{
    canonical_update_target_confirmations, insert_document_target_reviews,
    load_document_target_reviews, load_package_import_target_review, reviewed_document_target,
    validate_approval_bindings, validate_document_target_reviews,
    validate_exact_update_target_confirmations, validate_selection_target_review_replay,
    validate_target_review_binding_snapshot,
};
pub use target_review::{
    package_import_target_review_sha256, package_update_target_confirmations_sha256,
};

pub use types_contract::{
    CompletedPackageAssetAuthority, CompletedPackageAssetSourceAuthority,
    CompletedPackageAuthority, CompletedPackageComponentAuthority,
    CompletedPackageDocumentAuthority, MAX_COMPLETED_PACKAGE_EXPORTS,
    MAX_PACKAGE_TARGET_REVIEW_DOCUMENTS, PackageCapability, PackageCapabilityDecision,
    PackageCapabilityReview, PackageCapabilitySupport, PackageDocumentCommitBinding,
    PackageDocumentTargetDisposition, PackageDocumentTargetReview, PackageImportApprovalRecord,
    PackageImportAuditEvent, PackageImportExpectation, PackageImportTargetReview,
    PackageInspectionExpectation, PackageNormalizationEvidence, PackageUpdateTargetConfirmation,
};

const MAX_PACKAGE_JSON_BYTES: usize = 16 * 1024 * 1024;
const MAX_PACKAGE_JSON_DEPTH: usize = 40;
const MAX_PACKAGE_JSON_NODES: usize = 200_000;
const MAX_CAPABILITY_REASON_BYTES: usize = 4 * 1024;
const MAX_PACKAGE_APPROVAL_BYTES: usize = 256 * 1024;
const MAX_NORMALIZATION_REASON_BYTES: usize = 512;
const MAX_COMPLETED_MODULE_AUTHORITIES: usize = 64;

pub fn package_capability_review_sha256(review: &PackageCapabilityReview) -> CoreResult<String> {
    validate_capability_review(review, &[])?;
    let mut canonical = review.clone();
    canonical
        .decisions
        .sort_by_key(|decision| decision.capability);
    let json = encode_json("package capability review", &canonical)?;
    Ok(sha256_hex(json.as_bytes()))
}

#[cfg(test)]
mod tests;
