use std::collections::{BTreeMap, BTreeSet};
#[cfg(test)]
use std::{fs, path::PathBuf};

mod approval;
mod approval_validation;
mod completed_authority;
mod contract_codec;
mod inspection;
mod lifecycle;
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
pub(crate) use completed_authority::VerifiedCompletedPackageAuthorities;
use completed_authority::{CompletedPackageAuthoritySnapshot, CompletedPackageCasFile};
use contract_codec::{
    component_kind_str, decode_json, encode_json, i64_from_u64, import_status_str, license_fields,
    not_found, parse_capability_support, parse_datetime, parse_import_status,
    parse_package_capability, revision_conflict, sha256_hex, storage_corrupted, u32_from_i64,
    u64_from_i64, validate_identifier, validate_sha256,
};
use inspection::{validate_capability_review, validate_source_record};
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

type CompletedAuthorityCommitEvidence =
    BTreeMap<(String, u32), (CompletedPackageDocumentAuthority, Value)>;

impl Storage {
    #[allow(clippy::too_many_lines)] // Every immutable approval and commit seam is revalidated.
    fn get_completed_package_authority_by_approval_id_in_connection(
        connection: &Connection,
        approval_id: &str,
    ) -> CoreResult<CompletedPackageAuthoritySnapshot> {
        validate_identifier("package approval", approval_id)?;
        let approval_row = connection
            .query_row(
                "SELECT import_id, inspection_sha256, selection_sha256,
                        capability_review_sha256, approved_at
                 FROM package_import_approvals
                 WHERE id = ?1",
                [approval_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("package approval"))?;
        let current = read_import_state(connection, &approval_row.0)?;
        let completed_at = connection
            .query_row(
                "SELECT completed_at FROM package_imports WHERE id = ?1",
                [approval_row.0.as_str()],
                |row| row.get::<_, Option<String>>(0),
            )
            .map_err(storage_db_error)?
            .ok_or_else(|| {
                storage_corrupted("completed package authority has no completion timestamp")
            })?;
        parse_datetime("package import completed_at", &completed_at)?;
        if current.record.status != PackageImportStatus::Completed
            || current.inspection_sha256 != approval_row.1
            || current.selection_sha256.as_deref() != Some(approval_row.2.as_str())
            || current.capability_review_sha256 != approval_row.3
            || current.approved_selection_sha256.as_deref() != Some(approval_row.2.as_str())
            || current.approved_at
                != Some(parse_datetime(
                    "package approval approved_at",
                    &approval_row.4,
                )?)
        {
            return Err(storage_corrupted(
                "package approval is not the exact authority for a completed import",
            ));
        }
        let source = read_package_source_by_id(connection, &current.package_source_id)?;
        validate_sha256("package source", &source.source_sha256)
            .map_err(|_| storage_corrupted("completed package source digest is invalid"))?;
        let source_cas = connection
            .query_row(
                "SELECT relative_path, size_bytes
                 FROM content_sources
                 WHERE sha256 = ?1",
                [source.source_sha256.as_str()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .map_err(storage_db_error)?;
        if u64_from_i64("package source size", source_cas.1)? != source.source_size_bytes {
            return Err(storage_corrupted(
                "completed package source CAS metadata differs from its package record",
            ));
        }
        let mut cas_files = vec![CompletedPackageCasFile {
            namespace: "sources",
            sha256: source.source_sha256.clone(),
            size_bytes: source.source_size_bytes,
            relative_path: source_cas.0,
        }];
        let approval = read_approval_payload(connection, &approval_row.0)?;
        if approval.plan.approval_id != approval_id
            || approval.plan.review_sha256.as_str() != approval_row.1
            || approval.plan.plan_sha256.as_str() != approval_row.2
            || approval.plan.source_sha256.as_str() != source.source_sha256
            || approval.plan.package_id != source.package_id
            || approval.plan.package_id != current.record.package_id
        {
            return Err(storage_corrupted(
                "completed package approval payload differs from its durable identity",
            ));
        }
        let mut committed_assets = Vec::with_capacity(approval.plan.assets.len());
        for asset in &approval.plan.assets {
            let stored = connection
                .query_row(
                    "SELECT cas.relative_path, cas.media_type, cas.size_bytes,
                            descriptor.payload_json
                     FROM assets AS cas
                     JOIN asset_descriptors AS descriptor
                       ON descriptor.asset_hash = cas.sha256
                     WHERE cas.sha256 = ?1 AND descriptor.id = ?2",
                    params![asset.sha256.as_str(), asset.id.as_str()],
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
                .map_err(storage_db_error)?
                .ok_or_else(|| {
                    storage_corrupted("completed package approval asset CAS row is missing")
                })?;
            if stored.1 != asset.media_type
                || u64_from_i64("package asset size", stored.2)? != asset.size_bytes
            {
                return Err(storage_corrupted(
                    "completed package approval asset metadata differs from its descriptor",
                ));
            }
            cas_files.push(CompletedPackageCasFile {
                namespace: "assets",
                sha256: asset.sha256.as_str().to_owned(),
                size_bytes: asset.size_bytes,
                relative_path: stored.0,
            });
            let expected_descriptor = encode_json("completed package asset descriptor", asset)?;
            if stored.3 != expected_descriptor {
                return Err(storage_corrupted(
                    "completed package asset descriptor differs from approval",
                ));
            }
            let source_components = approval
                .plan
                .components
                .iter()
                .filter(|component| component.component.asset_ids.contains(&asset.id))
                .map(|component| CompletedPackageAssetSourceAuthority {
                    component_id: component.component.id.clone(),
                    component_sha256: component.component.sha256.as_str().to_owned(),
                })
                .collect();
            committed_assets.push(CompletedPackageAssetAuthority {
                asset_id: asset.id.clone(),
                descriptor: asset.clone(),
                descriptor_sha256: sha256_hex(expected_descriptor.as_bytes()),
                cas_sha256: asset.sha256.as_str().to_owned(),
                source_components,
            });
        }

        let selected_rows = load_selected_commit_components(connection, &approval_row.0)?;
        if selected_rows.len() != approval.plan.components.len() {
            return Err(storage_corrupted(
                "completed package approval component count differs from selection",
            ));
        }
        for planned in &approval.plan.components {
            let row = selected_rows.get(&planned.component.id).ok_or_else(|| {
                storage_corrupted("completed package approval component is missing from selection")
            })?;
            if !row.selected
                || !matches!(row.disposition.as_str(), "create" | "update" | "conflict")
                || row.component_kind != component_kind_str(planned.component.kind)
                || sha256_hex(row.review_json.as_bytes()) != row.review_sha256
            {
                return Err(storage_corrupted(
                    "completed package component review metadata is invalid",
                ));
            }
            let descriptor: lorepia_orchestration::PackageComponentDescriptor =
                decode_json("package component review", &row.review_json)?;
            if descriptor != planned.component {
                return Err(storage_corrupted(
                    "completed package component differs from its approved descriptor",
                ));
            }
        }

        let mut statement = connection
            .prepare(
                "SELECT component.source_component_key,
                        committed_document.document_ordinal,
                        committed_document.target_object_id,
                        committed_document.target_revision_id,
                        committed_document.result_json,
                        committed_document.result_sha256
                 FROM package_import_component_commits AS committed_document
                 JOIN package_import_components AS component
                   ON component.import_id = committed_document.import_id
                  AND component.ordinal = committed_document.component_ordinal
                 WHERE committed_document.import_id = ?1
                 ORDER BY component.ordinal, committed_document.document_ordinal",
            )
            .map_err(storage_db_error)?;
        let committed_rows = statement
            .query_map([approval_row.0.as_str()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u32>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?;
        let mut committed_documents = CompletedAuthorityCommitEvidence::new();
        for row in committed_rows {
            validate_sha256("package component commit result", &row.5)?;
            if sha256_hex(row.4.as_bytes()) != row.5 {
                return Err(storage_corrupted(
                    "completed package component result hash does not match",
                ));
            }
            let result: VersionedJson = decode_json("package component commit result", &row.4)?;
            if result.schema_version != 1
                || result
                    .value
                    .get("source_component_key")
                    .and_then(Value::as_str)
                    != Some(row.0.as_str())
                || result
                    .value
                    .get("component_document_ordinal")
                    .and_then(Value::as_u64)
                    != Some(u64::from(row.1))
                || result.value.get("target_object_id").and_then(Value::as_str)
                    != Some(row.2.as_str())
                || result
                    .value
                    .get("target_revision_id")
                    .and_then(Value::as_str)
                    != Some(row.3.as_str())
            {
                return Err(storage_corrupted(
                    "completed package component result differs from its typed columns",
                ));
            }
            let source_component_sha256 = result
                .value
                .get("source_component_sha256")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    storage_corrupted("completed package component source hash is missing")
                })?
                .to_owned();
            let document_sha256 = result
                .value
                .get("document_sha256")
                .and_then(Value::as_str)
                .ok_or_else(|| storage_corrupted("completed package document hash is missing"))?
                .to_owned();
            validate_sha256("package component", &source_component_sha256)
                .map_err(|_| storage_corrupted("completed package component hash is invalid"))?;
            validate_sha256("package document", &document_sha256)
                .map_err(|_| storage_corrupted("completed package document hash is invalid"))?;
            let authority = CompletedPackageDocumentAuthority {
                document_ordinal: row.1,
                target_object_id: row.2,
                target_revision_id: row.3,
                source_component_sha256,
                document_sha256,
                result_sha256: row.5,
            };
            if committed_documents
                .insert((row.0, row.1), (authority, result.value))
                .is_some()
            {
                return Err(storage_corrupted(
                    "completed package component result identity is duplicated",
                ));
            }
        }
        if committed_documents.len() != approval.document_bindings.len() {
            return Err(storage_corrupted(
                "completed package commit evidence count differs from approval",
            ));
        }
        for binding in &approval.document_bindings {
            let (document, _) = committed_documents
                .get(&(
                    binding.source_component_key.clone(),
                    binding.component_document_ordinal,
                ))
                .ok_or_else(|| {
                    storage_corrupted("completed package approval binding has no commit evidence")
                })?;
            if document.target_object_id != binding.target_object_id
                || document.source_component_sha256 != binding.source_component_sha256
                || document.document_sha256 != binding.document_sha256
            {
                return Err(storage_corrupted(
                    "completed package commit evidence differs from approval binding",
                ));
            }
        }
        validate_completed_authority_audit(connection, &current, &approval, &committed_documents)?;

        let enabled_components = approval
            .plan
            .components
            .iter()
            .filter(|component| component.enabled)
            .map(|component| {
                let mut documents = committed_documents
                    .iter()
                    .filter(|((component_id, _), _)| component_id == &component.component.id)
                    .map(|(_, (document, _))| document.clone())
                    .collect::<Vec<_>>();
                documents.sort_by_key(|document| document.document_ordinal);
                CompletedPackageComponentAuthority {
                    component_id: component.component.id.clone(),
                    kind: component.component.kind,
                    sha256: component.component.sha256.as_str().to_owned(),
                    committed_documents: documents,
                }
            })
            .collect();
        Ok(CompletedPackageAuthoritySnapshot {
            authority: CompletedPackageAuthority {
                approval_id: approval_id.to_owned(),
                import_id: approval_row.0,
                package_id: approval.plan.package_id,
                status: current.record.status,
                import_revision: current.record.revision,
                source_sha256: source.source_sha256,
                inspection_sha256: approval_row.1,
                selection_sha256: approval_row.2,
                capability_review_sha256: approval_row.3,
                approval_sha256: approval.plan.approval_sha256.as_str().to_owned(),
                required_capabilities: approval.plan.required_capabilities,
                approved_capabilities: approval.approved_capabilities,
                enabled_components,
                committed_assets,
            },
            cas_files,
        })
    }

    /// Builds the exact imported-module authority consumed by the pure module
    /// resolver. The caller supplies only a stored immutable module revision
    /// and an approval id; all package and component evidence is reloaded.
    pub fn get_module_import_approval_evidence(
        &self,
        approval_id: &str,
        stored: &ActiveContentModuleRevision,
    ) -> CoreResult<ModuleImportApprovalEvidence> {
        let verified = self.verify_completed_package_authority_with(
            approval_id,
            |connection, approval_id| {
                Self::get_completed_package_authority_by_approval_id_in_connection(
                    connection,
                    approval_id,
                )
            },
            || {},
        )?;
        let connection = self.connection()?;
        let authority = Self::revalidate_completed_package_authority_in_connection(
            &connection,
            approval_id,
            &verified,
        )?;
        build_module_import_approval_evidence_in_connection(&connection, stored, &authority)
    }

    /// Lists every completed package authority that committed this exact
    /// imported module revision.
    ///
    /// The deterministic list exists for restart and lost-response recovery.
    /// Callers must present multiple candidates for an explicit choice; this
    /// method never selects an approval merely because it is newest.
    pub fn list_completed_package_import_authorities_for_module_revision(
        &self,
        stored: &ActiveContentModuleRevision,
    ) -> CoreResult<Vec<ModuleImportApprovalEvidence>> {
        validate_completed_module_authority_target(stored)?;
        let candidate_limit = i64::try_from(MAX_COMPLETED_MODULE_AUTHORITIES + 1)
            .map_err(|_| CoreError::internal("completed module authority limit overflow"))?;
        let approval_ids = {
            let connection = self.connection()?;
            let mut statement = connection
                .prepare(
                    "SELECT DISTINCT approval.id, approval.approved_at
                     FROM package_import_approvals AS approval
                     JOIN package_imports AS import
                       ON import.id = approval.import_id
                     JOIN package_sources AS source
                       ON source.id = import.package_source_id
                     JOIN package_import_component_commits AS committed_document
                       ON committed_document.import_id = import.id
                     JOIN package_import_components AS component
                       ON component.import_id = committed_document.import_id
                      AND component.ordinal =
                          committed_document.component_ordinal
                     WHERE import.state = 'completed'
                       AND source.source_hash = ?1
                       AND component.component_kind = 'content_module'
                       AND committed_document.target_object_id = ?2
                       AND committed_document.target_revision_id = ?3
                     ORDER BY approval.approved_at, approval.id
                     LIMIT ?4",
                )
                .map_err(storage_db_error)?;
            statement
                .query_map(
                    params![
                        stored.module_revision.source_hash.as_str(),
                        stored.object.value.id.as_str(),
                        stored.module_revision.id.as_str(),
                        candidate_limit,
                    ],
                    |row| row.get::<_, String>(0),
                )
                .map_err(storage_db_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage_db_error)?
        };
        if approval_ids.len() > MAX_COMPLETED_MODULE_AUTHORITIES {
            return Err(storage_corrupted(
                "completed module authority candidates exceed the bounded recovery limit",
            ));
        }
        let verified =
            self.verify_completed_package_authorities(approval_ids.iter().map(String::as_str))?;
        let connection = self.connection()?;
        approval_ids
            .into_iter()
            .map(|approval_id| {
                let verified = verified.get(&approval_id).ok_or_else(|| {
                    storage_corrupted("completed module authority was not CAS-verified")
                })?;
                let authority = Self::revalidate_completed_package_authority_in_connection(
                    &connection,
                    &approval_id,
                    verified,
                )?;
                build_module_import_approval_evidence_in_connection(&connection, stored, &authority)
            })
            .collect()
    }

    /// Transaction-local variant used while package-backed module activation
    /// is re-reviewed under the same database snapshot as its bindings.
    pub(crate) fn get_module_import_approval_evidence_in_transaction(
        transaction: &Transaction<'_>,
        approval_id: &str,
        stored: &ActiveContentModuleRevision,
        verified_authorities: &VerifiedCompletedPackageAuthorities,
    ) -> CoreResult<ModuleImportApprovalEvidence> {
        // No CAS path is opened here. The transaction performs only an exact
        // metadata/revision revalidation of the proof created before it began.
        let verified = verified_authorities.get(approval_id).ok_or_else(|| {
            CoreError::invalid(
                "module package approval changed after CAS authority preverification",
            )
        })?;
        let authority = Self::revalidate_completed_package_authority_in_connection(
            transaction,
            approval_id,
            verified,
        )?;
        build_module_import_approval_evidence_in_connection(transaction, stored, &authority)
    }

    /// Atomically appends all selected typed documents, immutable asset
    /// descriptors, per-component commit evidence and the final state change.
    ///
    /// Asset bytes and `assets` rows must already have been committed to CAS.
    #[allow(clippy::too_many_lines)] // Document, projection, audit, and state writes must stay atomic.
    pub fn commit_package_import(
        &self,
        input: &PackageCommitInput,
        expected: &PackageImportExpectation,
        bindings: &[PackageDocumentCommitBinding],
    ) -> CoreResult<PackageImportRecord> {
        validate_expectation(expected)?;
        validate_commit_input_shape(input, bindings)?;
        let source_hash = input.source.source_sha256.as_str();
        self.package_source_path(source_hash, input.source.source_size_bytes)?;
        for asset in &input.assets {
            self.verify_package_asset_cas(asset)?;
        }
        let now = Utc::now();
        let committing_revision = expected
            .revision
            .checked_add(1)
            .ok_or_else(|| CoreError::internal("package import revision overflow"))?;
        let completed_revision = committing_revision
            .checked_add(1)
            .ok_or_else(|| CoreError::internal("package import revision overflow"))?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let current = read_import_state(&transaction, &input.import.id)?;
        if current.record.status == PackageImportStatus::Completed {
            validate_completed_commit_replay(&transaction, &current, input, expected, bindings)?;
            claim_package_asset_promotions(&transaction, &input.import.id, &input.assets, false)?;
            transaction.commit().map_err(storage_db_error)?;
            return Ok(current.record);
        }
        assert_expectation(&current, expected)?;
        if current.record.status != PackageImportStatus::Approved {
            return Err(CoreError::invalid(
                "only an approved package import can be committed",
            ));
        }
        let stored_source = read_package_source_by_id(&transaction, &current.package_source_id)?;
        if stored_source != input.source || input.import != current.record {
            return Err(CoreError::invalid(
                "package commit input does not match the approved durable import",
            ));
        }
        let approval = read_approval_payload(&transaction, &input.import.id)?;
        if approval.plan.review_sha256.as_str() != expected.inspection_sha256
            || approval.plan.plan_sha256.as_str() != expected.selection_sha256
            || approval.plan.source_sha256.as_str() != source_hash
            || approval.plan.package_id != input.source.package_id
            || approval.document_bindings != bindings
        {
            return Err(CoreError::invalid(
                "package commit is not bound to the exact approval snapshot",
            ));
        }
        if input.assets != approval.plan.assets {
            return Err(CoreError::invalid(
                "package commit assets differ from the approved asset inventory",
            ));
        }
        let component_rows = load_selected_commit_components(&transaction, &input.import.id)?;
        let target_review = load_package_import_target_review(&transaction, &current)?;
        validate_commit_bindings(
            &transaction,
            &input.documents,
            bindings,
            &component_rows,
            &target_review,
            &approval.confirmed_update_targets,
        )?;
        validate_document_normalization_evidence(
            &input.documents,
            bindings,
            &approval.normalization_evidence,
        )?;

        let start_payload = VersionedJson {
            schema_version: 1,
            value: json!({
                "approval_sha256": approval.plan.approval_sha256.as_str(),
                "document_count": input.documents.len(),
                "asset_count": input.assets.len(),
            }),
        };
        append_audit(
            &transaction,
            &input.import.id,
            committing_revision,
            "commit_started",
            &start_payload,
            now,
        )?;
        update_import_state(
            &transaction,
            &input.import.id,
            expected.revision,
            PackageImportStatus::Committing,
            committing_revision,
            current.approved_selection_sha256.as_deref(),
            current.approved_at,
            None,
            None,
            now,
        )?;

        for asset in &input.assets {
            append_package_asset_descriptor(&transaction, asset, source_hash)?;
        }
        let binding_by_index = bindings
            .iter()
            .map(|binding| (binding.document_index as usize, binding))
            .collect::<BTreeMap<_, _>>();
        let mut committed = Vec::with_capacity(input.documents.len());
        for (index, document) in input.documents.iter().enumerate() {
            let binding = binding_by_index
                .get(&index)
                .ok_or_else(|| CoreError::internal("validated document binding disappeared"))?;
            let written = append_package_commit_document(
                &transaction,
                document,
                binding.expected_object_revision,
                source_hash,
            )?;
            let row = component_rows
                .get(&binding.source_component_key)
                .ok_or_else(|| CoreError::internal("validated component binding disappeared"))?;
            if written.object_id != document_object_id(document) {
                return Err(storage_corrupted(
                    "package document helper returned an unexpected object identity",
                ));
            }
            let result = VersionedJson {
                schema_version: 1,
                value: json!({
                    "source_component_key": binding.source_component_key,
                    "component_document_ordinal": binding.component_document_ordinal,
                    "source_component_sha256": binding.source_component_sha256,
                    "document_sha256": binding.document_sha256,
                    "target_object_id": written.object_id,
                    "target_revision_id": written.revision_id,
                    "target_state_revision": written.state_revision,
                }),
            };
            let result_json = encode_json("package component commit result", &result)?;
            let result_sha256 = sha256_hex(result_json.as_bytes());
            transaction
                .execute(
                    "INSERT INTO package_import_component_commits (
                        import_id, component_ordinal, document_ordinal, target_object_id,
                        target_revision_id, result_json, result_sha256,
                        committed_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        input.import.id,
                        i64::from(row.ordinal),
                        i64::from(binding.component_document_ordinal),
                        written.object_id,
                        written.revision_id,
                        result_json,
                        result_sha256,
                        now.to_rfc3339(),
                    ],
                )
                .map_err(storage_db_error)?;
            committed.push(result.value);
        }
        let completion_payload = VersionedJson {
            schema_version: 1,
            value: json!({
                "approval_sha256": approval.plan.approval_sha256.as_str(),
                "components": committed,
                "asset_ids": input.assets.iter()
                    .map(|asset| asset.id.as_str())
                    .collect::<Vec<_>>(),
            }),
        };
        append_audit(
            &transaction,
            &input.import.id,
            completed_revision,
            "commit_completed",
            &completion_payload,
            now,
        )?;
        update_import_state(
            &transaction,
            &input.import.id,
            committing_revision,
            PackageImportStatus::Completed,
            completed_revision,
            current.approved_selection_sha256.as_deref(),
            current.approved_at,
            None,
            Some(now),
            now,
        )?;
        claim_package_asset_promotions(&transaction, &input.import.id, &input.assets, true)?;
        transaction.commit().map_err(storage_db_error)?;
        drop(connection);
        self.get_package_import(&input.import.id)
    }
}

pub fn package_capability_review_sha256(review: &PackageCapabilityReview) -> CoreResult<String> {
    validate_capability_review(review, &[])?;
    let mut canonical = review.clone();
    canonical
        .decisions
        .sort_by_key(|decision| decision.capability);
    let json = encode_json("package capability review", &canonical)?;
    Ok(sha256_hex(json.as_bytes()))
}

fn validate_commit_input_shape(
    input: &PackageCommitInput,
    bindings: &[PackageDocumentCommitBinding],
) -> CoreResult<()> {
    validate_source_record(&input.source)?;
    validate_identifier("package import", &input.import.id)?;
    validate_normalized_package_documents(&input.documents)?;
    if input.import.status != PackageImportStatus::Approved {
        return Err(CoreError::invalid(
            "package commit input must contain the approved import snapshot",
        ));
    }
    if input.import.failure_code.is_some() {
        return Err(CoreError::invalid(
            "approved package commit input cannot contain a failure",
        ));
    }
    if bindings.len() != input.documents.len() {
        return Err(CoreError::invalid(
            "every committed package document requires exactly one binding",
        ));
    }
    validate_binding_snapshot_shape(bindings)?;
    for binding in bindings {
        let index = usize::try_from(binding.document_index)
            .map_err(|_| CoreError::invalid("package document index is invalid"))?;
        if index >= input.documents.len() {
            return Err(CoreError::invalid(
                "package document binding index is out of bounds",
            ));
        }
        let document_json = encode_json("package commit document", &input.documents[index])?;
        if sha256_hex(document_json.as_bytes()) != binding.document_sha256 {
            return Err(CoreError::invalid(
                "package document hash does not match the commit binding",
            ));
        }
    }
    Ok(())
}

fn validate_normalized_package_documents(documents: &[PackageCommitDocument]) -> CoreResult<()> {
    let built_ins = crate::orchestration::built_in_prompt_presets();
    let canonical_policy = built_ins
        .first()
        .and_then(|preset| preset.blocks.first())
        .ok_or_else(|| CoreError::internal("canonical application policy is missing"))?;
    let built_in_preset_ids = built_ins
        .iter()
        .map(|preset| preset.id.as_str())
        .collect::<BTreeSet<_>>();
    for document in documents {
        match document {
            PackageCommitDocument::PromptPreset(preset) => {
                if built_in_preset_ids.contains(preset.id.as_str()) {
                    return Err(CoreError::invalid(
                        "imported packages cannot replace built-in prompt presets",
                    ));
                }
                if preset.blocks.first() != Some(canonical_policy) {
                    return Err(CoreError::invalid(
                        "imported prompt preset lacks the canonical application policy",
                    ));
                }
                let canonical_count = preset
                    .blocks
                    .iter()
                    .filter(|block| *block == canonical_policy)
                    .count();
                if canonical_count != 1 {
                    return Err(CoreError::invalid(
                        "canonical application policy must appear exactly once",
                    ));
                }
                for block in preset.blocks.iter().skip(1) {
                    if block.authority != InstructionAuthority::ImportedContent
                        || block.placement_zone == PlacementZone::ApplicationPolicy
                        || block.provenance.source_kind == SourceKind::ApplicationBuiltIn
                    {
                        return Err(CoreError::invalid(
                            "imported prompt preset retains elevated package block authority",
                        ));
                    }
                }
            }
            PackageCommitDocument::ContentModule(module) => {
                if module.prompt_fragments.iter().any(|block| {
                    block.authority == InstructionAuthority::Application
                        || block.placement_zone == PlacementZone::ApplicationPolicy
                        || block.provenance.source_kind == SourceKind::ApplicationBuiltIn
                }) {
                    return Err(CoreError::invalid(
                        "imported content module retains application-owned prompt blocks",
                    ));
                }
            }
            PackageCommitDocument::TransformSet(set) => {
                if set.enabled
                    || set
                        .rules
                        .iter()
                        .any(|rule| rule.enabled || rule.imported_enabled)
                {
                    return Err(CoreError::invalid(
                        "imported transform sets and rules must remain inactive",
                    ));
                }
            }
            PackageCommitDocument::InteractionRuleSet(set) => {
                if set.rules.iter().any(|rule| rule.enabled) {
                    return Err(CoreError::invalid(
                        "imported interaction rules must remain inactive",
                    ));
                }
            }
            PackageCommitDocument::KnowledgeBook(book) => {
                book.validate().map_err(|error| {
                    CoreError::invalid(format!("invalid imported knowledge book: {error}"))
                })?;
            }
            PackageCommitDocument::MemoryProfile(profile) => {
                profile.validate().map_err(|error| {
                    CoreError::invalid(format!("invalid imported memory profile: {error}"))
                })?;
            }
            PackageCommitDocument::CharacterContent { .. } => {}
        }
    }
    Ok(())
}

fn validate_binding_snapshot_shape(bindings: &[PackageDocumentCommitBinding]) -> CoreResult<()> {
    let mut indices = BTreeSet::new();
    let mut component_documents = BTreeSet::new();
    let mut targets = BTreeSet::new();
    let mut ordinals_by_component = BTreeMap::<&str, Vec<u32>>::new();
    for (expected_index, binding) in bindings.iter().enumerate() {
        validate_identifier("package component", &binding.source_component_key)?;
        validate_identifier("package target object", &binding.target_object_id)?;
        if !matches!(
            binding.document_kind.as_str(),
            "prompt_preset"
                | "knowledge_book"
                | "memory_profile"
                | "transform_set"
                | "interaction_rule_set"
                | "content_module"
                | "character_content"
        ) {
            return Err(CoreError::invalid(
                "package document binding kind is invalid",
            ));
        }
        validate_sha256("package component", &binding.source_component_sha256)?;
        validate_sha256("package document", &binding.document_sha256)?;
        let index = usize::try_from(binding.document_index)
            .map_err(|_| CoreError::invalid("package document index is invalid"))?;
        if index != expected_index {
            return Err(CoreError::invalid(
                "package document bindings must be ordered by contiguous document index",
            ));
        }
        if !indices.insert(index) {
            return Err(CoreError::invalid(
                "package document bindings contain a duplicate index",
            ));
        }
        if !component_documents.insert((
            binding.source_component_key.as_str(),
            binding.component_document_ordinal,
        )) {
            return Err(CoreError::invalid(
                "package component document bindings contain a duplicate ordinal",
            ));
        }
        if !targets.insert(binding.target_object_id.as_str()) {
            return Err(CoreError::invalid(
                "package document bindings contain a duplicate target object",
            ));
        }
        ordinals_by_component
            .entry(binding.source_component_key.as_str())
            .or_default()
            .push(binding.component_document_ordinal);
    }
    if indices
        .iter()
        .copied()
        .enumerate()
        .any(|(expected, actual)| expected != actual)
    {
        return Err(CoreError::invalid(
            "package document indices must be contiguous from zero",
        ));
    }
    for ordinals in ordinals_by_component.values_mut() {
        ordinals.sort_unstable();
        if ordinals
            .iter()
            .enumerate()
            .any(|(expected, actual)| usize::try_from(*actual) != Ok(expected))
        {
            return Err(CoreError::invalid(
                "package component document ordinals must be contiguous from zero",
            ));
        }
    }
    Ok(())
}

fn load_selected_commit_components(
    connection: &Connection,
    import_id: &str,
) -> CoreResult<BTreeMap<String, ReviewedComponentRow>> {
    let mut statement = connection
        .prepare(
            "SELECT ordinal, source_component_key, component_kind,
                    disposition, selected, target_object_id,
                    target_revision_id, review_json, review_sha256
             FROM package_import_components
             WHERE import_id = ?1 AND selected = 1
             ORDER BY ordinal",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map([import_id], |row| {
            Ok(ReviewedComponentRow {
                ordinal: row.get::<_, u32>(0)?,
                source_component_key: row.get(1)?,
                component_kind: row.get(2)?,
                disposition: row.get(3)?,
                selected: row.get(4)?,
                target_object_id: row.get(5)?,
                target_revision_id: row.get(6)?,
                review_json: row.get(7)?,
                review_sha256: row.get(8)?,
            })
        })
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    rows.into_iter()
        .map(|row| Ok((row.source_component_key.clone(), row)))
        .collect()
}

fn validate_completed_authority_audit(
    connection: &Connection,
    current: &StoredImportState,
    approval: &PackageApprovalPayload,
    committed: &CompletedAuthorityCommitEvidence,
) -> CoreResult<()> {
    let row = connection
        .query_row(
            "SELECT event_kind, payload_json, payload_sha256
             FROM package_import_audit_events
             WHERE import_id = ?1 AND import_revision = ?2",
            params![
                current.record.id,
                i64_from_u64("package audit revision", current.record.revision)?,
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| storage_corrupted("completed package import has no completion audit"))?;
    if row.0 != "commit_completed" || sha256_hex(row.1.as_bytes()) != row.2 {
        return Err(storage_corrupted(
            "completed package authority audit kind or hash is invalid",
        ));
    }
    let wrapper: VersionedJson = decode_json("package completion audit", &row.1)?;
    if wrapper.schema_version != 1
        || wrapper.value.get("approval_sha256").and_then(Value::as_str)
            != Some(approval.plan.approval_sha256.as_str())
    {
        return Err(storage_corrupted(
            "completed package authority audit differs from approval",
        ));
    }
    let asset_ids = wrapper
        .value
        .get("asset_ids")
        .and_then(Value::as_array)
        .ok_or_else(|| storage_corrupted("package completion audit has no asset inventory"))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| storage_corrupted("package completion asset id is invalid"))
        })
        .collect::<CoreResult<Vec<_>>>()?;
    let expected_asset_ids = approval
        .plan
        .assets
        .iter()
        .map(|asset| asset.id.as_str().to_owned())
        .collect::<Vec<_>>();
    if asset_ids != expected_asset_ids {
        return Err(storage_corrupted(
            "package completion audit asset inventory differs from approval",
        ));
    }
    let mut audited = BTreeMap::new();
    for value in wrapper
        .value
        .get("components")
        .and_then(Value::as_array)
        .ok_or_else(|| storage_corrupted("package completion audit has no component evidence"))?
    {
        let component_id = value
            .get("source_component_key")
            .and_then(Value::as_str)
            .ok_or_else(|| storage_corrupted("package completion component id is invalid"))?;
        let document_ordinal = value
            .get("component_document_ordinal")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| storage_corrupted("package completion document ordinal is invalid"))?;
        if audited
            .insert((component_id.to_owned(), document_ordinal), value)
            .is_some()
        {
            return Err(storage_corrupted(
                "package completion audit contains duplicate component evidence",
            ));
        }
    }
    if audited.len() != committed.len()
        || committed
            .iter()
            .any(|(key, (_, result))| audited.get(key).is_none_or(|audited| *audited != result))
    {
        return Err(storage_corrupted(
            "package completion audit differs from durable commit evidence",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_lines)] // Exact replay validates every immutable result row and hash.
fn validate_completed_commit_replay(
    connection: &Connection,
    current: &StoredImportState,
    input: &PackageCommitInput,
    expected: &PackageImportExpectation,
    bindings: &[PackageDocumentCommitBinding],
) -> CoreResult<()> {
    let completed_revision = expected
        .revision
        .checked_add(2)
        .ok_or_else(|| CoreError::invalid("package replay revision overflow"))?;
    if current.record.revision != completed_revision
        || current.inspection_sha256 != expected.inspection_sha256
        || current.selection_sha256.as_deref() != Some(&expected.selection_sha256)
        || current.capability_review_sha256 != expected.capability_review_sha256
        || input.import.status != PackageImportStatus::Approved
        || input.import.revision != expected.revision
        || input.import.id != current.record.id
        || input.import.package_id != current.record.package_id
        || input.import.inspection != current.record.inspection
        || input.import.selection != current.record.selection
        || input.import.selected_component_ids != current.record.selected_component_ids
        || input.import.created_at != current.record.created_at
        || current.approved_at != Some(input.import.updated_at)
    {
        return Err(revision_conflict(
            "completed package import replay",
            &current.record.id,
            Some(expected.revision),
            Some(current.record.revision),
        ));
    }
    let stored_source = read_package_source_by_id(connection, &current.package_source_id)?;
    if stored_source != input.source {
        return Err(CoreError::invalid(
            "completed package replay source differs from the committed source",
        ));
    }
    let approval = read_approval_payload(connection, &input.import.id)?;
    if approval.document_bindings != bindings
        || approval.plan.review_sha256.as_str() != expected.inspection_sha256
        || approval.plan.plan_sha256.as_str() != expected.selection_sha256
        || approval.plan.source_sha256.as_str() != input.source.source_sha256
        || approval.plan.package_id != input.source.package_id
        || approval.plan.assets != input.assets
    {
        return Err(CoreError::invalid(
            "completed package replay differs from the approved snapshot",
        ));
    }
    validate_document_normalization_evidence(
        &input.documents,
        bindings,
        &approval.normalization_evidence,
    )?;
    let mut statement = connection
        .prepare(
            "SELECT component.source_component_key,
                    committed_document.document_ordinal,
                    committed_document.target_object_id,
                    committed_document.target_revision_id,
                    committed_document.result_json,
                    committed_document.result_sha256
             FROM package_import_component_commits AS committed_document
             JOIN package_import_components AS component
               ON component.import_id = committed_document.import_id
              AND component.ordinal = committed_document.component_ordinal
             WHERE committed_document.import_id = ?1
             ORDER BY component.source_component_key,
                      committed_document.document_ordinal",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map([input.import.id.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, u32>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    if rows.len() != bindings.len() {
        return Err(storage_corrupted(
            "completed package commit evidence count is incomplete",
        ));
    }
    let evidence = rows
        .into_iter()
        .map(|row| {
            if sha256_hex(row.4.as_bytes()) != row.5 {
                return Err(storage_corrupted(
                    "completed package commit result hash does not match",
                ));
            }
            let result: VersionedJson = decode_json("package component commit result", &row.4)?;
            Ok(((row.0, row.1), (row.2, row.3, result)))
        })
        .collect::<CoreResult<BTreeMap<_, _>>>()?;
    for binding in bindings {
        let (target_object_id, target_revision_id, result) = evidence
            .get(&(
                binding.source_component_key.clone(),
                binding.component_document_ordinal,
            ))
            .ok_or_else(|| storage_corrupted("completed package commit evidence is missing"))?;
        let document = input
            .documents
            .get(binding.document_index as usize)
            .ok_or_else(|| CoreError::invalid("package replay document index is invalid"))?;
        if target_object_id != &binding.target_object_id
            || target_object_id != &document_object_id(document)
            || result
                .value
                .get("target_revision_id")
                .and_then(Value::as_str)
                != Some(target_revision_id.as_str())
            || result
                .value
                .get("source_component_sha256")
                .and_then(Value::as_str)
                != Some(binding.source_component_sha256.as_str())
            || result.value.get("document_sha256").and_then(Value::as_str)
                != Some(binding.document_sha256.as_str())
        {
            return Err(storage_corrupted(
                "completed package commit evidence differs from replay input",
            ));
        }
    }
    Ok(())
}

fn validate_commit_bindings(
    connection: &Connection,
    documents: &[PackageCommitDocument],
    bindings: &[PackageDocumentCommitBinding],
    components: &BTreeMap<String, ReviewedComponentRow>,
    target_review: &PackageImportTargetReview,
    confirmed_update_targets: &[PackageUpdateTargetConfirmation],
) -> CoreResult<()> {
    validate_approval_bindings(
        connection,
        bindings,
        components,
        target_review,
        confirmed_update_targets,
    )?;
    for binding in bindings {
        let row = components
            .get(&binding.source_component_key)
            .ok_or_else(|| CoreError::invalid("package binding names an unselected component"))?;
        if !matches!(row.disposition.as_str(), "create" | "update" | "conflict") {
            return Err(CoreError::invalid(
                "package binding names a component that cannot be committed",
            ));
        }
        if sha256_hex(row.review_json.as_bytes()) != row.review_sha256 {
            return Err(storage_corrupted(
                "package component review hash does not match",
            ));
        }
        let descriptor: lorepia_orchestration::PackageComponentDescriptor =
            decode_json("package component review", &row.review_json)?;
        if descriptor.sha256.as_str() != binding.source_component_sha256 {
            return Err(CoreError::invalid(
                "package binding source hash differs from the approved component",
            ));
        }
        let index = binding.document_index as usize;
        let document = documents
            .get(index)
            .ok_or_else(|| CoreError::invalid("package document index is out of bounds"))?;
        if row.component_kind != document_kind(document)
            || binding.document_kind != document_kind(document)
            || descriptor.id != binding.source_component_key
            || binding.target_object_id != document_object_id(document)
        {
            return Err(CoreError::invalid(
                "package document kind does not match its approved component",
            ));
        }
    }
    Ok(())
}

fn document_object_id(document: &PackageCommitDocument) -> String {
    match document {
        PackageCommitDocument::PromptPreset(value) => value.id.as_str().to_owned(),
        PackageCommitDocument::KnowledgeBook(value) => value.id.as_str().to_owned(),
        PackageCommitDocument::MemoryProfile(value) => value.id.as_str().to_owned(),
        PackageCommitDocument::TransformSet(value) => value.id.as_str().to_owned(),
        PackageCommitDocument::InteractionRuleSet(value) => value.id.as_str().to_owned(),
        PackageCommitDocument::ContentModule(value) => value.id.as_str().to_owned(),
        PackageCommitDocument::CharacterContent { character_id, .. } => {
            format!("character-content:{character_id}")
        }
    }
}

fn document_kind(document: &PackageCommitDocument) -> &'static str {
    match document {
        PackageCommitDocument::PromptPreset(_) => "prompt_preset",
        PackageCommitDocument::KnowledgeBook(_) => "knowledge_book",
        PackageCommitDocument::MemoryProfile(_) => "memory_profile",
        PackageCommitDocument::TransformSet(_) => "transform_set",
        PackageCommitDocument::InteractionRuleSet(_) => "interaction_rule_set",
        PackageCommitDocument::ContentModule(_) => "content_module",
        PackageCommitDocument::CharacterContent { .. } => "character_content",
    }
}

fn validate_completed_module_authority_target(
    stored: &ActiveContentModuleRevision,
) -> CoreResult<()> {
    validate_identifier("content module", stored.object.value.id.as_str())?;
    validate_identifier(
        "content module revision",
        stored.module_revision.id.as_str(),
    )?;
    let module_document_json =
        encode_json("imported module authority document", &stored.object.value)?;
    let provenance = &stored.object.value.metadata.provenance;
    if sha256_hex(module_document_json.as_bytes()) != stored.object.sha256
        || stored.object.revision_id != stored.module_revision.id.as_str()
        || stored.object.object_id != stored.object.value.id.as_str()
        || stored.object.value.id != stored.module_revision.module_id
        || provenance.source_kind != SourceKind::ImportedPackage
        || provenance.source_hash.as_deref() != Some(stored.module_revision.source_hash.as_str())
        || provenance.source_id.as_deref().is_none_or(str::is_empty)
    {
        return Err(storage_corrupted(
            "imported module authority target differs from its immutable revision",
        ));
    }
    Ok(())
}

enum ModuleAuthorityComponent {
    Embedded,
    Linked {
        target_object_id: String,
        target_revision_id: String,
    },
    Asset(AssetDescriptor),
}

fn read_module_authority_component(
    connection: &Connection,
    stored: &ActiveContentModuleRevision,
    component: &lorepia_domain::ComponentHash,
) -> CoreResult<ModuleAuthorityComponent> {
    let revision_id = stored.module_revision.id.as_str();
    match &component.component {
        ModuleComponentRef::PromptBlock { id } => {
            read_prompt_block_module_authority_component(connection, stored, component, id)
        }
        ModuleComponentRef::Control { id } => {
            read_control_module_authority_component(connection, stored, component, id)
        }
        ModuleComponentRef::KnowledgeBook { id } => {
            if !stored.object.value.knowledge_book_ids.contains(id) {
                return Err(storage_corrupted(
                    "module knowledge projection is absent from its immutable document",
                ));
            }
            read_linked_module_authority_component(
                connection,
                revision_id,
                "knowledge_book",
                "knowledge_book_revision_id",
                id.as_str(),
                component,
            )
        }
        ModuleComponentRef::TransformSet { id } => {
            if !stored.object.value.transform_set_ids.contains(id) {
                return Err(storage_corrupted(
                    "module transform projection is absent from its immutable document",
                ));
            }
            read_linked_module_authority_component(
                connection,
                revision_id,
                "transform_set",
                "transform_set_revision_id",
                id.as_str(),
                component,
            )
        }
        ModuleComponentRef::InteractionRuleSet { id } => {
            if !stored.object.value.interaction_rule_set_ids.contains(id) {
                return Err(storage_corrupted(
                    "module interaction projection is absent from its immutable document",
                ));
            }
            read_linked_module_authority_component(
                connection,
                revision_id,
                "interaction_rule_set",
                "interaction_rule_set_revision_id",
                id.as_str(),
                component,
            )
        }
        ModuleComponentRef::Asset { id } => {
            read_asset_module_authority_component(connection, stored, component, id)
        }
    }
}

fn read_prompt_block_module_authority_component(
    connection: &Connection,
    stored: &ActiveContentModuleRevision,
    component: &lorepia_domain::ComponentHash,
    id: &PromptBlockId,
) -> CoreResult<ModuleAuthorityComponent> {
    let expected = stored
        .object
        .value
        .prompt_fragments
        .iter()
        .find(|block| block.id == *id)
        .ok_or_else(|| {
            storage_corrupted(
                "module prompt-block projection is absent from its immutable document",
            )
        })?;
    let expected_json = encode_json("module prompt block authority", expected)?;
    let row = connection
        .query_row(
            "SELECT component.component_sha256, block.document_json
             FROM content_module_components AS component
             JOIN content_module_prompt_blocks AS block
               ON block.module_revision_id = component.module_revision_id
              AND block.block_id = component.prompt_block_id
             WHERE component.module_revision_id = ?1
               AND component.component_kind = 'prompt_block'
               AND component.prompt_block_id = ?2",
            params![stored.module_revision.id.as_str(), id.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("module prompt-block authority"))?;
    validate_embedded_module_authority(component, &expected_json, &row)?;
    Ok(ModuleAuthorityComponent::Embedded)
}

fn read_control_module_authority_component(
    connection: &Connection,
    stored: &ActiveContentModuleRevision,
    component: &lorepia_domain::ComponentHash,
    id: &ControlId,
) -> CoreResult<ModuleAuthorityComponent> {
    let expected = stored
        .object
        .value
        .control_specs
        .iter()
        .find(|control| control.id == *id)
        .ok_or_else(|| {
            storage_corrupted("module control projection is absent from its immutable document")
        })?;
    let expected_json = encode_json("module control authority", expected)?;
    let row = connection
        .query_row(
            "SELECT component.component_sha256, control.document_json
             FROM content_module_components AS component
             JOIN content_module_controls AS control
               ON control.module_revision_id = component.module_revision_id
              AND control.control_id = component.control_id
             WHERE component.module_revision_id = ?1
               AND component.component_kind = 'control'
               AND component.control_id = ?2",
            params![stored.module_revision.id.as_str(), id.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("module control authority"))?;
    validate_embedded_module_authority(component, &expected_json, &row)?;
    Ok(ModuleAuthorityComponent::Embedded)
}

fn read_asset_module_authority_component(
    connection: &Connection,
    stored: &ActiveContentModuleRevision,
    component: &lorepia_domain::ComponentHash,
    id: &AssetId,
) -> CoreResult<ModuleAuthorityComponent> {
    if !stored.object.value.asset_ids.contains(id) {
        return Err(storage_corrupted(
            "module asset projection is absent from its immutable document",
        ));
    }
    let row = connection
        .query_row(
            "SELECT component.component_sha256, descriptor.payload_json
             FROM content_module_components AS component
             JOIN asset_descriptors AS descriptor
               ON descriptor.id = component.asset_descriptor_id
             WHERE component.module_revision_id = ?1
               AND component.component_kind = 'asset'
               AND component.asset_descriptor_id = ?2",
            params![stored.module_revision.id.as_str(), id.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("module asset authority"))?;
    if row.0 != component.sha256.as_str() || sha256_hex(row.1.as_bytes()) != row.0 {
        return Err(storage_corrupted(
            "module asset authority hash differs from its immutable projection",
        ));
    }
    let descriptor: AssetDescriptor = decode_json("module asset authority descriptor", &row.1)?;
    if descriptor.id != *id {
        return Err(storage_corrupted(
            "module asset authority descriptor has a different identity",
        ));
    }
    Ok(ModuleAuthorityComponent::Asset(descriptor))
}

fn validate_embedded_module_authority(
    component: &lorepia_domain::ComponentHash,
    expected_json: &str,
    stored: &(String, String),
) -> CoreResult<()> {
    if stored.0 != component.sha256.as_str()
        || sha256_hex(stored.1.as_bytes()) != stored.0
        || stored.1 != expected_json
    {
        return Err(storage_corrupted(
            "embedded module authority differs from its immutable document",
        ));
    }
    Ok(())
}

fn read_linked_module_authority_component(
    connection: &Connection,
    module_revision_id: &str,
    object_kind: &'static str,
    revision_column: &'static str,
    object_id: &str,
    component: &lorepia_domain::ComponentHash,
) -> CoreResult<ModuleAuthorityComponent> {
    let query = match revision_column {
        "knowledge_book_revision_id" => {
            "SELECT component.component_sha256, content.object_id, content.id,
                    content.document_json, content.document_sha256
             FROM content_module_components AS component
             JOIN content_revisions AS content
               ON content.id = component.knowledge_book_revision_id
             WHERE component.module_revision_id = ?1
               AND component.component_kind = 'knowledge_book'
               AND content.object_kind = ?2 AND content.object_id = ?3"
        }
        "transform_set_revision_id" => {
            "SELECT component.component_sha256, content.object_id, content.id,
                    content.document_json, content.document_sha256
             FROM content_module_components AS component
             JOIN content_revisions AS content
               ON content.id = component.transform_set_revision_id
             WHERE component.module_revision_id = ?1
               AND component.component_kind = 'transform_set'
               AND content.object_kind = ?2 AND content.object_id = ?3"
        }
        "interaction_rule_set_revision_id" => {
            "SELECT component.component_sha256, content.object_id, content.id,
                    content.document_json, content.document_sha256
             FROM content_module_components AS component
             JOIN content_revisions AS content
               ON content.id = component.interaction_rule_set_revision_id
             WHERE component.module_revision_id = ?1
               AND component.component_kind = 'interaction_rule_set'
               AND content.object_kind = ?2 AND content.object_id = ?3"
        }
        _ => {
            return Err(CoreError::internal(
                "module authority linked revision column is unsupported",
            ));
        }
    };
    let row = connection
        .query_row(
            query,
            params![module_revision_id, object_kind, object_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("linked module component authority"))?;
    if row.0 != component.sha256.as_str()
        || row.4 != row.0
        || sha256_hex(row.3.as_bytes()) != row.4
        || row.1 != object_id
    {
        return Err(storage_corrupted(
            "linked module authority differs from its immutable revision",
        ));
    }
    Ok(ModuleAuthorityComponent::Linked {
        target_object_id: row.1,
        target_revision_id: row.2,
    })
}

#[allow(clippy::too_many_lines)] // One conversion covers every typed module component authority.
fn build_module_import_approval_evidence_in_connection(
    connection: &Connection,
    stored: &ActiveContentModuleRevision,
    authority: &CompletedPackageAuthority,
) -> CoreResult<ModuleImportApprovalEvidence> {
    validate_completed_module_authority_target(stored)?;
    let source_sha256 = parse_authority_sha256("package source", &authority.source_sha256)?;
    let provenance = &stored.object.value.metadata.provenance;
    if authority.status != PackageImportStatus::Completed
        || authority.import_revision == 0
        || provenance.source_kind != SourceKind::ImportedPackage
        || provenance.source_id.as_deref() != Some(authority.package_id.as_str())
        || provenance.source_hash.as_deref() != Some(authority.source_sha256.as_str())
        || stored.module_revision.source_hash != source_sha256
    {
        return Err(package_authority_denied(
            "completed package authority does not own the imported module source",
        ));
    }
    // `document_sha256` authenticates the tagged `PackageCommitDocument`
    // approval payload and is revalidated while loading `authority`; it is not
    // the hash of the inner content revision. The immutable inner module is
    // authenticated by `validate_completed_module_authority_target`, while the
    // exact commit link is the object/revision/component tuple below.
    let module_matches = authority
        .enabled_components
        .iter()
        .filter(|component| component.kind == PackageComponentKind::ContentModule)
        .flat_map(|component| {
            component
                .committed_documents
                .iter()
                .filter(|document| {
                    document.target_object_id == stored.object.value.id.as_str()
                        && document.target_revision_id == stored.module_revision.id.as_str()
                        && document.source_component_sha256 == component.sha256
                })
                .map(move |document| (component, document))
        })
        .collect::<Vec<_>>();
    let [(module_component, module_document)] = module_matches.as_slice() else {
        return Err(package_authority_denied(
            "completed package authority does not select the exact module revision",
        ));
    };

    let mut selected_package_component_ids = authority
        .enabled_components
        .iter()
        .map(|component| component.component_id.clone())
        .collect::<Vec<_>>();
    selected_package_component_ids.sort();
    if selected_package_component_ids
        .windows(2)
        .any(|pair| pair[0] == pair[1])
    {
        return Err(storage_corrupted(
            "completed package authority contains duplicate enabled components",
        ));
    }
    let mut authorized_capabilities = authority.required_capabilities.clone();
    authorized_capabilities.sort();
    authorized_capabilities.dedup();

    let mut component_authorities =
        Vec::with_capacity(stored.module_revision.component_hashes.len());
    for component in &stored.module_revision.component_hashes {
        let material = read_module_authority_component(connection, stored, component)?;
        let component_authority = match (&component.component, material) {
            (
                ModuleComponentRef::PromptBlock { .. } | ModuleComponentRef::Control { .. },
                ModuleAuthorityComponent::Embedded,
            ) => module_document_component_authority(component, module_component, module_document)?,
            (
                ModuleComponentRef::KnowledgeBook { .. },
                ModuleAuthorityComponent::Linked {
                    target_object_id,
                    target_revision_id,
                },
            ) => document_module_component_authority(
                component,
                PackageComponentKind::KnowledgeBook,
                &target_object_id,
                &target_revision_id,
                authority,
            )?,
            (
                ModuleComponentRef::TransformSet { .. },
                ModuleAuthorityComponent::Linked {
                    target_object_id,
                    target_revision_id,
                },
            ) => document_module_component_authority(
                component,
                PackageComponentKind::TransformSet,
                &target_object_id,
                &target_revision_id,
                authority,
            )?,
            (
                ModuleComponentRef::InteractionRuleSet { .. },
                ModuleAuthorityComponent::Linked {
                    target_object_id,
                    target_revision_id,
                },
            ) => document_module_component_authority(
                component,
                PackageComponentKind::InteractionRuleSet,
                &target_object_id,
                &target_revision_id,
                authority,
            )?,
            (ModuleComponentRef::Asset { id }, ModuleAuthorityComponent::Asset(descriptor)) => {
                asset_module_component_authority(
                    component,
                    id,
                    &descriptor,
                    module_component,
                    authority,
                )?
            }
            _ => {
                return Err(storage_corrupted(
                    "module component material kind differs from its immutable reference",
                ));
            }
        };
        component_authorities.push(component_authority);
    }
    component_authorities.sort();

    Ok(ModuleImportApprovalEvidence {
        approval_id: authority.approval_id.clone(),
        approval_sha256: parse_authority_sha256("package approval", &authority.approval_sha256)?,
        import_id: authority.import_id.clone(),
        import_revision: authority.import_revision,
        package_id: authority.package_id.clone(),
        package_source_sha256: source_sha256,
        selection_sha256: parse_authority_sha256("package selection", &authority.selection_sha256)?,
        capability_review_sha256: parse_authority_sha256(
            "package capability review",
            &authority.capability_review_sha256,
        )?,
        module_id: stored.object.value.id.clone(),
        module_revision_id: stored.module_revision.id.clone(),
        module_revision_source_sha256: stored.module_revision.source_hash.clone(),
        module_package_component_id: module_component.component_id.clone(),
        module_package_component_sha256: parse_authority_sha256(
            "package module component",
            &module_component.sha256,
        )?,
        module_commit_result_sha256: parse_authority_sha256(
            "package module commit",
            &module_document.result_sha256,
        )?,
        selected_package_component_ids,
        authorized_capabilities,
        component_authorities,
    })
}

fn module_document_component_authority(
    component: &lorepia_domain::ComponentHash,
    module_component: &CompletedPackageComponentAuthority,
    module_document: &CompletedPackageDocumentAuthority,
) -> CoreResult<ModuleImportComponentAuthority> {
    Ok(ModuleImportComponentAuthority {
        component: component.component.clone(),
        component_sha256: component.sha256.clone(),
        package_component_id: module_component.component_id.clone(),
        package_component_sha256: parse_authority_sha256(
            "package module component",
            &module_component.sha256,
        )?,
        committed_target_object_id: module_document.target_object_id.clone(),
        committed_target_revision_id: module_document.target_revision_id.clone(),
        committed_result_sha256: parse_authority_sha256(
            "package module commit",
            &module_document.result_sha256,
        )?,
        committed_content_sha256: None,
    })
}

fn document_module_component_authority(
    component: &lorepia_domain::ComponentHash,
    kind: PackageComponentKind,
    target_object_id: &str,
    target_revision_id: &str,
    authority: &CompletedPackageAuthority,
) -> CoreResult<ModuleImportComponentAuthority> {
    // The approved document hash covers the tagged package-commit envelope.
    // `read_linked_module_authority_component` has already authenticated the
    // inner child revision and its component hash, so bind that immutable child
    // to the exact committed object/revision/component tuple here.
    let matches = authority
        .enabled_components
        .iter()
        .filter(|candidate| candidate.kind == kind)
        .flat_map(|candidate| {
            candidate
                .committed_documents
                .iter()
                .filter(|document| {
                    document.target_object_id == target_object_id
                        && document.target_revision_id == target_revision_id
                        && document.source_component_sha256 == candidate.sha256
                })
                .map(move |document| (candidate, document))
        })
        .collect::<Vec<_>>();
    let [(package_component, document)] = matches.as_slice() else {
        return Err(package_authority_denied(
            "completed package authority does not cover an exact module component revision",
        ));
    };
    Ok(ModuleImportComponentAuthority {
        component: component.component.clone(),
        component_sha256: component.sha256.clone(),
        package_component_id: package_component.component_id.clone(),
        package_component_sha256: parse_authority_sha256(
            "package component",
            &package_component.sha256,
        )?,
        committed_target_object_id: document.target_object_id.clone(),
        committed_target_revision_id: document.target_revision_id.clone(),
        committed_result_sha256: parse_authority_sha256(
            "package component commit",
            &document.result_sha256,
        )?,
        committed_content_sha256: None,
    })
}

fn asset_module_component_authority(
    component: &lorepia_domain::ComponentHash,
    asset_id: &AssetId,
    descriptor: &AssetDescriptor,
    module_component: &CompletedPackageComponentAuthority,
    authority: &CompletedPackageAuthority,
) -> CoreResult<ModuleImportComponentAuthority> {
    let asset_matches = authority
        .committed_assets
        .iter()
        .filter(|asset| {
            asset.asset_id == *asset_id
                && asset.descriptor == *descriptor
                && asset.descriptor_sha256 == component.sha256.as_str()
                && asset.cas_sha256 == descriptor.sha256.as_str()
        })
        .collect::<Vec<_>>();
    let [asset] = asset_matches.as_slice() else {
        return Err(package_authority_denied(
            "completed package authority does not cover an exact module asset",
        ));
    };
    let source_matches = asset
        .source_components
        .iter()
        .filter(|source| {
            source.component_id == module_component.component_id
                && source.component_sha256 == module_component.sha256
        })
        .collect::<Vec<_>>();
    let [source] = source_matches.as_slice() else {
        return Err(package_authority_denied(
            "completed package authority does not bind the exact asset to the module component",
        ));
    };
    let descriptor_sha256 =
        parse_authority_sha256("package asset descriptor", &asset.descriptor_sha256)?;
    Ok(ModuleImportComponentAuthority {
        component: component.component.clone(),
        component_sha256: component.sha256.clone(),
        package_component_id: source.component_id.clone(),
        package_component_sha256: parse_authority_sha256(
            "package asset component",
            &source.component_sha256,
        )?,
        committed_target_object_id: asset.asset_id.as_str().to_owned(),
        committed_target_revision_id: asset.descriptor_sha256.clone(),
        committed_result_sha256: descriptor_sha256,
        committed_content_sha256: Some(parse_authority_sha256(
            "package asset content",
            &asset.cas_sha256,
        )?),
    })
}

fn parse_authority_sha256(label: &str, value: &str) -> CoreResult<Sha256Digest> {
    Sha256Digest::parse(value.to_owned())
        .map_err(|error| storage_corrupted(format!("completed {label} hash is invalid: {error}")))
}

fn package_authority_denied(message: &'static str) -> CoreError {
    CoreError::new(CoreErrorCode::PermissionDenied, message, false)
}

#[cfg(test)]
mod tests;
