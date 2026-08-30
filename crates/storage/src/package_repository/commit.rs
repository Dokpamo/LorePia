//! Atomic approved package commit transaction.

use super::{
    BTreeMap, CoreError, CoreResult, PackageCommitInput, PackageDocumentCommitBinding,
    PackageImportExpectation, PackageImportRecord, PackageImportStatus, Storage,
    TransactionBehavior, Utc, VersionedJson, append_audit, append_package_asset_descriptor,
    append_package_commit_document, assert_expectation, claim_package_asset_promotions,
    document_object_id, encode_json, json, load_package_import_target_review,
    load_selected_commit_components, params, read_approval_payload, read_import_state,
    read_package_source_by_id, sha256_hex, storage_corrupted, storage_db_error,
    update_import_state, validate_commit_bindings, validate_commit_input_shape,
    validate_completed_commit_replay, validate_document_normalization_evidence,
    validate_expectation,
};

impl Storage {
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
