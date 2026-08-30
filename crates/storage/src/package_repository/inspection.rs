//! Inspected package persistence and CAS compensation.

use super::{
    BTreeSet, ContentCapability, CoreError, CoreResult, MAX_CAPABILITY_REASON_BYTES,
    OptionalExtension, PackageCapability, PackageCapabilityReview, PackageCapabilitySupport,
    PackageImportRecord, PackageImportStatus, PackageReview, PackageSourceRecord,
    StagedAssetImport, Storage, TransactionBehavior, VersionedJson, append_audit,
    claim_package_source_promotion, encode_json, insert_capability_review, insert_package_source,
    json, package_capability_review_sha256, params, read_import_state, revision_conflict,
    storage_db_error, validate_identifier, validate_sha256,
};

impl Storage {
    /// Removes a promoted source only when no durable product or package
    /// record claimed it. This is the compensating operation for failure
    /// between CAS promotion and `create_inspected_package_import`.
    pub fn discard_unclaimed_package_source(
        &self,
        import_id: &str,
        source_sha256: &str,
        source_size_bytes: u64,
    ) -> CoreResult<bool> {
        validate_identifier("package import", import_id)?;
        validate_sha256("package source", source_sha256)?;
        self.cleanup_package_source_promotion(import_id, source_sha256, source_size_bytes)
    }

    /// Removes promoted asset rows and bytes that were not claimed by a
    /// descriptor, character, raw-extension preservation record or character
    /// asset link. Claimed/deduplicated assets are retained.
    pub fn discard_unclaimed_package_assets(
        &self,
        import_id: &str,
        staged_assets: &[StagedAssetImport],
    ) -> CoreResult<u64> {
        validate_identifier("package import", import_id)?;
        self.cleanup_package_asset_promotions(import_id, staged_assets)
    }

    /// Starts the durable lifecycle at `inspected` without manufacturing a
    /// selection. The immutable component review is retained in
    /// `inspection_json`; normalized selected component rows are inserted only
    /// by [`Storage::select_package_import`], because those rows are immutable.
    pub fn create_inspected_package_import(
        &self,
        source: &PackageSourceRecord,
        import: &PackageImportRecord,
        review: &PackageReview,
        capability_review: &PackageCapabilityReview,
    ) -> CoreResult<PackageImportRecord> {
        validate_inspected_import(source, import, review, capability_review)?;
        verify_source_cas(self, source)?;
        let capability_hash = package_capability_review_sha256(capability_review)?;
        let inspection_json = encode_json("package inspection", &import.inspection)?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        insert_package_source(&transaction, source)?;
        if transaction
            .query_row(
                "SELECT 1 FROM package_imports WHERE id = ?1",
                [import.id.as_str()],
                |_| Ok(()),
            )
            .optional()
            .map_err(storage_db_error)?
            .is_some()
        {
            let current = read_import_state(&transaction, &import.id)?;
            if current.record == *import
                && current.package_source_id == source.id
                && current.inspection_sha256 == review.review_sha256.as_str()
                && current.capability_review_sha256 == capability_hash
            {
                claim_package_source_promotion(
                    &transaction,
                    &import.id,
                    &source.source_sha256,
                    source.source_size_bytes,
                    false,
                )?;
                transaction.commit().map_err(storage_db_error)?;
                return Ok(current.record);
            }
            return Err(revision_conflict(
                "package import",
                &import.id,
                None,
                Some(current.record.revision),
            ));
        }
        transaction
            .execute(
                "INSERT INTO package_imports (
                    id, package_source_id, inspection_schema_version, state,
                    revision, inspection_json, inspection_sha256,
                    selection_json, selection_sha256,
                    capability_review_sha256, approved_selection_sha256,
                    approved_at, failure_json, created_at, updated_at,
                    completed_at
                 ) VALUES (
                    ?1, ?2, ?3, 'inspected', 1, ?4, ?5, NULL, NULL, ?6,
                    NULL, NULL, NULL, ?7, ?8, NULL
                 )",
                params![
                    import.id,
                    source.id,
                    i64::from(import.inspection.schema_version),
                    inspection_json,
                    review.review_sha256.as_str(),
                    capability_hash,
                    import.created_at.to_rfc3339(),
                    import.updated_at.to_rfc3339(),
                ],
            )
            .map_err(storage_db_error)?;
        insert_capability_review(&transaction, &import.id, capability_review)?;
        append_audit(
            &transaction,
            &import.id,
            1,
            "inspected",
            &VersionedJson {
                schema_version: 1,
                value: json!({
                    "inspection_sha256": review.review_sha256.as_str(),
                    "capability_review_sha256": capability_hash,
                    "source_sha256": source.source_sha256,
                }),
            },
            import.created_at,
        )?;
        claim_package_source_promotion(
            &transaction,
            &import.id,
            &source.source_sha256,
            source.source_size_bytes,
            true,
        )?;
        transaction.commit().map_err(storage_db_error)?;
        drop(connection);
        self.get_package_import(&import.id)
    }
}

pub(super) fn validate_inspected_import(
    source: &PackageSourceRecord,
    import: &PackageImportRecord,
    review: &PackageReview,
    capability_review: &PackageCapabilityReview,
) -> CoreResult<()> {
    validate_source_record(source)?;
    review
        .verify()
        .map_err(|error| CoreError::invalid(format!("package review is invalid: {error}")))?;
    if import.status != PackageImportStatus::Inspected
        || import.revision != 1
        || import.selection.is_some()
        || !import.selected_component_ids.is_empty()
        || import.failure_code.is_some()
    {
        return Err(CoreError::invalid(
            "a new package inspection must begin unselected at revision 1",
        ));
    }
    validate_identifier("package import", &import.id)?;
    if import.package_id != source.package_id
        || review.manifest.package_id != source.package_id
        || review.source_sha256.as_str() != source.source_sha256
    {
        return Err(CoreError::invalid(
            "package source and inspection identities do not match",
        ));
    }
    let manifest_value = serde_json::to_value(&review.manifest).map_err(|error| {
        CoreError::invalid(format!("package manifest cannot be encoded: {error}"))
    })?;
    let inspection_value = serde_json::to_value(review).map_err(|error| {
        CoreError::invalid(format!("package review cannot be encoded: {error}"))
    })?;
    if source.manifest.schema_version != 1
        || source.manifest.value != manifest_value
        || import.inspection.schema_version != 1
        || import.inspection.value != inspection_value
    {
        return Err(CoreError::invalid(
            "package inspection wrappers do not contain the exact reviewed payloads",
        ));
    }
    if source.format != review.manifest.format
        || source.format_version != review.manifest.format_version
        || source.name != review.manifest.name
        || source.version != review.manifest.version
        || source.author != review.manifest.author
        || source.license != review.manifest.license
        || source.redistribution_allowed != review.manifest.redistribution_allowed
        || import.updated_at < import.created_at
    {
        return Err(CoreError::invalid(
            "package inspection metadata does not match the reviewed manifest",
        ));
    }
    validate_capability_review(capability_review, &[])
}

pub(super) fn verify_source_cas(storage: &Storage, source: &PackageSourceRecord) -> CoreResult<()> {
    storage
        .package_source_path(&source.source_sha256, source.source_size_bytes)
        .map(|_| ())
}

pub(super) fn validate_source_record(source: &PackageSourceRecord) -> CoreResult<()> {
    validate_identifier("package source", &source.id)?;
    validate_identifier("package", source.package_id.as_str())?;
    validate_sha256("package source", &source.source_sha256)?;
    if source.format_version == 0
        || source.manifest.schema_version != 1
        || source.name.trim().is_empty()
        || source.version.trim().is_empty()
        || source.source_size_bytes > i64::MAX.unsigned_abs()
    {
        return Err(CoreError::invalid("package source metadata is invalid"));
    }
    if !matches!(
        source.format.as_str(),
        "lorepia_content_package" | "public_character_card" | "compat_import"
    ) {
        return Err(CoreError::invalid("package source format is unsupported"));
    }
    encode_json("package source manifest", &source.manifest)?;
    Ok(())
}

pub(super) fn validate_capability_review(
    review: &PackageCapabilityReview,
    required: &[ContentCapability],
) -> CoreResult<()> {
    if review.schema_version != 1 {
        return Err(CoreError::invalid(
            "package capability review schema version is unsupported",
        ));
    }
    if review.decisions.len() != PackageCapability::ALL.len() {
        return Err(CoreError::invalid(
            "package capability review must contain the complete policy matrix",
        ));
    }
    let mut seen = BTreeSet::new();
    for decision in &review.decisions {
        if !seen.insert(decision.capability) {
            return Err(CoreError::invalid(
                "package capability review contains duplicate decisions",
            ));
        }
        if decision.support != decision.capability.required_support() || decision.approved {
            return Err(CoreError::invalid(
                "package capability decision differs from the storage safety policy",
            ));
        }
        if decision.reason.trim().is_empty()
            || decision.reason.len() > MAX_CAPABILITY_REASON_BYTES
            || decision.reason.chars().any(char::is_control)
        {
            return Err(CoreError::invalid(
                "package capability decision reason is invalid",
            ));
        }
    }
    if seen != PackageCapability::ALL.into_iter().collect() {
        return Err(CoreError::invalid(
            "package capability review policy matrix is incomplete",
        ));
    }
    for capability in required {
        let expected = PackageCapability::from(*capability);
        let decision = review
            .decisions
            .iter()
            .find(|decision| decision.capability == expected)
            .ok_or_else(|| {
                CoreError::invalid(format!(
                    "required package capability {} has no review decision",
                    expected.as_str()
                ))
            })?;
        if decision.support == PackageCapabilitySupport::Unsupported {
            return Err(CoreError::invalid(format!(
                "selected package requires unsupported capability {}",
                expected.as_str()
            )));
        }
    }
    Ok(())
}
