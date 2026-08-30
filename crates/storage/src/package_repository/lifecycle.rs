//! Package discard lifecycle transitions and exact replay validation.

use super::{
    Connection, CoreError, CoreResult, PackageImportExpectation, PackageImportRecord,
    PackageImportStatus, PackageInspectionExpectation, Storage, StoredImportState,
    TransactionBehavior, Utc, VersionedJson, append_audit, assert_expectation,
    assert_inspection_expectation, json, read_import_state, revision_conflict, storage_db_error,
    update_import_state, validate_audit_replay, validate_expectation, validate_identifier,
    validate_inspection_expectation,
};

impl Storage {
    /// Discards a reviewed or approved import without deleting its immutable
    /// source, review, approval, component inventory or audit evidence.
    pub fn discard_inspected_package_import(
        &self,
        import_id: &str,
        expected: &PackageInspectionExpectation,
    ) -> CoreResult<PackageImportRecord> {
        validate_identifier("package import", import_id)?;
        validate_inspection_expectation(expected)?;
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
        let payload = VersionedJson {
            schema_version: 1,
            value: json!({
                "revision": expected.revision,
                "inspection_sha256": expected.inspection_sha256,
                "capability_review_sha256": expected.capability_review_sha256,
            }),
        };
        if current.record.status == PackageImportStatus::Discarded
            && current.record.revision == next_revision
        {
            validate_inspected_discard_replay(&transaction, &current, expected, &payload)?;
            return Ok(current.record);
        }
        assert_inspection_expectation(&current, expected)?;
        if current.record.status != PackageImportStatus::Inspected
            || current.record.selection.is_some()
        {
            return Err(CoreError::invalid(
                "only an unselected inspected package import can use this discard path",
            ));
        }
        append_audit(
            &transaction,
            import_id,
            next_revision,
            "discarded",
            &payload,
            now,
        )?;
        update_import_state(
            &transaction,
            import_id,
            expected.revision,
            PackageImportStatus::Discarded,
            next_revision,
            None,
            None,
            None,
            Some(now),
            now,
        )?;
        transaction.commit().map_err(storage_db_error)?;
        drop(connection);
        self.get_package_import(import_id)
    }

    /// Discards a selected or approved import using all three reviewed hashes.
    pub fn discard_package_import(
        &self,
        import_id: &str,
        expected: &PackageImportExpectation,
    ) -> CoreResult<PackageImportRecord> {
        validate_identifier("package import", import_id)?;
        validate_expectation(expected)?;
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
        let payload = expectation_payload(expected);
        if current.record.status == PackageImportStatus::Discarded
            && current.record.revision == next_revision
        {
            validate_selected_discard_replay(&transaction, &current, expected, &payload)?;
            return Ok(current.record);
        }
        assert_expectation(&current, expected)?;
        if !matches!(
            current.record.status,
            PackageImportStatus::AwaitingReview | PackageImportStatus::Approved
        ) {
            return Err(CoreError::invalid(
                "package import cannot be discarded from its current state",
            ));
        }
        append_audit(
            &transaction,
            import_id,
            next_revision,
            "discarded",
            &payload,
            now,
        )?;
        update_import_state(
            &transaction,
            import_id,
            expected.revision,
            PackageImportStatus::Discarded,
            next_revision,
            current.approved_selection_sha256.as_deref(),
            current.approved_at,
            None,
            Some(now),
            now,
        )?;
        transaction.commit().map_err(storage_db_error)?;
        drop(connection);
        self.get_package_import(import_id)
    }
}

pub(super) fn validate_inspected_discard_replay(
    connection: &Connection,
    current: &StoredImportState,
    expected: &PackageInspectionExpectation,
    audit: &VersionedJson,
) -> CoreResult<()> {
    let next_revision = expected
        .revision
        .checked_add(1)
        .ok_or_else(|| CoreError::invalid("package discard replay revision overflow"))?;
    if current.record.status != PackageImportStatus::Discarded
        || current.record.revision != next_revision
        || current.record.selection.is_some()
        || current.selection_sha256.is_some()
        || current.inspection_sha256 != expected.inspection_sha256
        || current.capability_review_sha256 != expected.capability_review_sha256
    {
        return Err(revision_conflict(
            "package inspection discard replay",
            &current.record.id,
            Some(expected.revision),
            Some(current.record.revision),
        ));
    }
    validate_audit_replay(
        connection,
        &current.record.id,
        next_revision,
        "discarded",
        audit,
    )
}

pub(super) fn validate_selected_discard_replay(
    connection: &Connection,
    current: &StoredImportState,
    expected: &PackageImportExpectation,
    audit: &VersionedJson,
) -> CoreResult<()> {
    let next_revision = expected
        .revision
        .checked_add(1)
        .ok_or_else(|| CoreError::invalid("package discard replay revision overflow"))?;
    if current.record.status != PackageImportStatus::Discarded
        || current.record.revision != next_revision
        || current.inspection_sha256 != expected.inspection_sha256
        || current.selection_sha256.as_deref() != Some(&expected.selection_sha256)
        || current.capability_review_sha256 != expected.capability_review_sha256
    {
        return Err(revision_conflict(
            "package discard replay",
            &current.record.id,
            Some(expected.revision),
            Some(current.record.revision),
        ));
    }
    validate_audit_replay(
        connection,
        &current.record.id,
        next_revision,
        "discarded",
        audit,
    )
}

pub(super) fn expectation_payload(expected: &PackageImportExpectation) -> VersionedJson {
    VersionedJson {
        schema_version: 1,
        value: json!({
            "revision": expected.revision,
            "inspection_sha256": expected.inspection_sha256,
            "selection_sha256": expected.selection_sha256,
            "capability_review_sha256": expected.capability_review_sha256,
        }),
    }
}
