//! Immutable package import approval persistence and retrieval.

use super::{
    ApprovedPackageImportPlan, CoreError, CoreResult, Deserialize, MAX_PACKAGE_APPROVAL_BYTES,
    OptionalExtension, PackageCapability, PackageDocumentCommitBinding,
    PackageImportApprovalRecord, PackageImportExpectation, PackageImportRecord,
    PackageImportStatus, PackageNormalizationEvidence, PackageUpdateTargetConfirmation, Serialize,
    Storage, TransactionBehavior, Utc, VersionedJson, append_audit, assert_expectation,
    canonical_update_target_confirmations, decode_json, decode_selection, encode_json,
    load_package_import_target_review, load_selected_commit_components, not_found,
    package_update_target_confirmations_sha256, params, parse_datetime, read_approval_payload,
    read_import_state, read_source_hash, sha256_hex, storage_corrupted, storage_db_error,
    update_import_state, validate_approval_bindings, validate_approval_replay,
    validate_binding_snapshot_shape, validate_capability_approval_snapshot, validate_expectation,
    validate_identifier, validate_normalization_evidence_linkage,
    validate_normalization_evidence_shape, validate_sha256,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PackageApprovalPayload {
    pub(super) plan: ApprovedPackageImportPlan,
    pub(super) document_bindings: Vec<PackageDocumentCommitBinding>,
    pub(super) target_review_sha256: String,
    pub(super) confirmed_update_targets: Vec<PackageUpdateTargetConfirmation>,
    pub(super) approved_capabilities: Vec<PackageCapability>,
    pub(super) normalization_evidence_sha256: String,
    pub(super) normalization_evidence: Vec<PackageNormalizationEvidence>,
}

impl Storage {
    /// Persists an immutable approval and advances exactly the reviewed import
    /// revision. The typed plan verifies its own canonical review, plan and
    /// approval hashes before any mutation.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    // Approval CAS and every immutable evidence input form one atomic boundary.
    pub fn approve_package_import(
        &self,
        import_id: &str,
        expected: &PackageImportExpectation,
        approved: &ApprovedPackageImportPlan,
        document_bindings: &[PackageDocumentCommitBinding],
        expected_target_review_sha256: &str,
        confirmed_update_targets: &[PackageUpdateTargetConfirmation],
        approved_capabilities: &[PackageCapability],
        normalization_evidence: &[PackageNormalizationEvidence],
    ) -> CoreResult<PackageImportRecord> {
        validate_identifier("package import", import_id)?;
        validate_expectation(expected)?;
        approved
            .verify()
            .map_err(|error| CoreError::invalid(format!("package approval is invalid: {error}")))?;
        validate_binding_snapshot_shape(document_bindings)?;
        validate_sha256("package target review", expected_target_review_sha256)?;
        let confirmed_update_targets =
            canonical_update_target_confirmations(confirmed_update_targets)?;
        let update_target_confirmations_sha256 =
            package_update_target_confirmations_sha256(&confirmed_update_targets)?;
        if approved.target_review_sha256.as_str() != expected_target_review_sha256
            || approved.update_target_confirmations_sha256.as_str()
                != update_target_confirmations_sha256
        {
            return Err(CoreError::invalid(
                "package approval hash is not bound to its target review and confirmations",
            ));
        }
        let mut approved_capabilities = approved_capabilities.to_vec();
        approved_capabilities.sort_unstable();
        if approved_capabilities
            .windows(2)
            .any(|pair| pair[0] == pair[1])
        {
            return Err(CoreError::invalid(
                "package capability approval contains duplicates",
            ));
        }
        let mut normalization_evidence = normalization_evidence.to_vec();
        normalization_evidence.sort();
        validate_normalization_evidence_shape(&normalization_evidence)?;
        let normalization_evidence_sha256 =
            package_normalization_evidence_sha256(&normalization_evidence)?;
        let approval_payload = PackageApprovalPayload {
            plan: approved.clone(),
            document_bindings: document_bindings.to_vec(),
            target_review_sha256: expected_target_review_sha256.to_owned(),
            confirmed_update_targets: confirmed_update_targets.clone(),
            approved_capabilities: approved_capabilities.clone(),
            normalization_evidence_sha256,
            normalization_evidence,
        };
        let payload = VersionedJson {
            schema_version: 1,
            value: serde_json::to_value(&approval_payload).map_err(|error| {
                CoreError::invalid(format!("package approval cannot be encoded: {error}"))
            })?,
        };
        let payload_json = encode_json("package approval", &payload)?;
        if payload_json.len() > MAX_PACKAGE_APPROVAL_BYTES {
            return Err(CoreError::invalid(
                "package approval exceeds the durable payload limit",
            ));
        }
        let now = Utc::now();
        let next_revision = expected
            .revision
            .checked_add(1)
            .ok_or_else(|| CoreError::internal("package import revision overflow"))?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let current = read_import_state(&transaction, import_id)?;
        if current.record.status == PackageImportStatus::Approved
            && current.record.revision == next_revision
        {
            validate_approval_replay(
                &transaction,
                &current,
                expected,
                &approval_payload,
                &payload,
            )?;
            return Ok(current.record);
        }
        assert_expectation(&current, expected)?;
        if current.record.status != PackageImportStatus::AwaitingReview {
            return Err(CoreError::invalid(
                "only an awaiting-review package import can be approved",
            ));
        }
        if approved.review_sha256.as_str() != current.inspection_sha256
            || approved.plan_sha256.as_str()
                != current
                    .selection_sha256
                    .as_deref()
                    .ok_or_else(|| storage_corrupted("package selection hash is missing"))?
            || approved.source_sha256.as_str()
                != read_source_hash(&transaction, &current.package_source_id)?
            || approved.package_id != current.record.package_id
        {
            return Err(CoreError::invalid(
                "package approval does not match the exact reviewed snapshots",
            ));
        }
        let selected_plan = decode_selection(&current.record)?;
        if approved.plan_sha256 != selected_plan.plan_sha256
            || approved.review_sha256 != selected_plan.review_sha256
            || approved.components.len() != selected_plan.components.len()
        {
            return Err(CoreError::invalid(
                "package approval payload does not match the stored selection",
            ));
        }
        let component_rows = load_selected_commit_components(&transaction, import_id)?;
        let target_review = load_package_import_target_review(&transaction, &current)?;
        if target_review.target_review_sha256 != expected_target_review_sha256 {
            return Err(CoreError::invalid(
                "package approval target-review digest is stale",
            ));
        }
        validate_approval_bindings(
            &transaction,
            document_bindings,
            &component_rows,
            &target_review,
            &confirmed_update_targets,
        )?;
        validate_normalization_evidence_linkage(
            &approval_payload.normalization_evidence,
            document_bindings,
            &component_rows,
        )?;
        validate_capability_approval_snapshot(
            &transaction,
            import_id,
            &selected_plan.required_capabilities,
            &approved_capabilities,
        )?;
        transaction
            .execute(
                "INSERT INTO package_import_approvals (
                    id, import_id, inspection_sha256, selection_sha256,
                    capability_review_sha256, approval_payload_json, approved_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    approved.approval_id,
                    import_id,
                    current.inspection_sha256,
                    current.selection_sha256,
                    current.capability_review_sha256,
                    payload_json,
                    now.to_rfc3339(),
                ],
            )
            .map_err(storage_db_error)?;
        append_audit(
            &transaction,
            import_id,
            next_revision,
            "approved",
            &payload,
            now,
        )?;
        update_import_state(
            &transaction,
            import_id,
            expected.revision,
            PackageImportStatus::Approved,
            next_revision,
            current.selection_sha256.as_deref(),
            Some(now),
            None,
            None,
            now,
        )?;
        transaction.commit().map_err(storage_db_error)?;
        drop(connection);
        self.get_package_import(import_id)
    }

    pub fn get_package_import_approval(
        &self,
        import_id: &str,
    ) -> CoreResult<PackageImportApprovalRecord> {
        validate_identifier("package import", import_id)?;
        let connection = self.connection()?;
        let record = connection
            .query_row(
                "SELECT id, inspection_sha256, selection_sha256,
                        capability_review_sha256, approval_payload_json,
                        approved_at
                 FROM package_import_approvals
                 WHERE import_id = ?1
                 ORDER BY approved_at DESC, id
                 LIMIT 1",
                [import_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_db_error)?
            .map_or_else(
                || Err(not_found("package import approval")),
                |row| {
                    Ok(PackageImportApprovalRecord {
                        id: row.0,
                        import_id: import_id.to_owned(),
                        inspection_sha256: row.1,
                        selection_sha256: row.2,
                        capability_review_sha256: row.3,
                        payload: decode_json("package approval", &row.4)?,
                        approved_at: parse_datetime("package approval approved_at", &row.5)?,
                    })
                },
            )?;
        let current = read_import_state(&connection, import_id)?;
        let payload = read_approval_payload(&connection, import_id)?;
        if record.inspection_sha256 != current.inspection_sha256
            || current.selection_sha256.as_deref() != Some(&record.selection_sha256)
            || record.capability_review_sha256 != current.capability_review_sha256
            || payload.plan.review_sha256.as_str() != record.inspection_sha256
            || payload.plan.plan_sha256.as_str() != record.selection_sha256
        {
            return Err(storage_corrupted(
                "package approval differs from its reviewed import snapshots",
            ));
        }
        Ok(record)
    }
}

pub fn package_normalization_evidence_sha256(
    evidence: &[PackageNormalizationEvidence],
) -> CoreResult<String> {
    validate_normalization_evidence_shape(evidence)?;
    let wrapper = VersionedJson {
        schema_version: 1,
        value: serde_json::to_value(evidence).map_err(|error| {
            CoreError::invalid(format!(
                "package normalization evidence cannot be encoded: {error}"
            ))
        })?,
    };
    Ok(sha256_hex(
        encode_json("package normalization evidence", &wrapper)?.as_bytes(),
    ))
}
