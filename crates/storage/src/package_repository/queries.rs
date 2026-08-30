//! Read-only package source, import, target review, and audit queries.

use super::{
    CoreError, CoreResult, MAX_COMPLETED_PACKAGE_EXPORTS, OptionalExtension,
    PackageCapabilityReview, PackageId, PackageImportAuditEvent, PackageImportExpectation,
    PackageImportRecord, PackageImportStatus, PackageImportTargetReview,
    PackageInspectionExpectation, PackageSourceRecord, Storage, decode_json,
    load_package_import_target_review, not_found, parse_datetime, parse_import_status,
    read_capability_review, read_import_state, read_package_source, read_package_source_by_id,
    sha256_hex, storage_corrupted, storage_db_error, u64_from_i64, validate_identifier,
    validate_sha256,
};

impl Storage {
    pub fn get_package_source(&self, id: &str) -> CoreResult<PackageSourceRecord> {
        validate_identifier("package source", id)?;
        let connection = self.connection()?;
        read_package_source(&connection, "source.id = ?1", id)?
            .ok_or_else(|| not_found("package source"))
    }

    pub fn get_package_source_by_hash(
        &self,
        source_sha256: &str,
    ) -> CoreResult<PackageSourceRecord> {
        validate_sha256("package source", source_sha256)?;
        let connection = self.connection()?;
        read_package_source(&connection, "source.source_hash = ?1", source_sha256)?
            .ok_or_else(|| not_found("package source"))
    }
    pub fn get_package_import(&self, id: &str) -> CoreResult<PackageImportRecord> {
        validate_identifier("package import", id)?;
        let connection = self.connection()?;
        read_import_state(&connection, id).map(|state| state.record)
    }

    pub fn get_package_import_target_review(
        &self,
        import_id: &str,
    ) -> CoreResult<PackageImportTargetReview> {
        validate_identifier("package import", import_id)?;
        let connection = self.connection()?;
        let current = read_import_state(&connection, import_id)?;
        if current.record.selection.is_none() {
            return Err(CoreError::invalid(
                "package import has no selected target review",
            ));
        }
        load_package_import_target_review(&connection, &current)
    }

    pub fn get_package_source_for_import(
        &self,
        import_id: &str,
    ) -> CoreResult<PackageSourceRecord> {
        validate_identifier("package import", import_id)?;
        let connection = self.connection()?;
        let source_id = connection
            .query_row(
                "SELECT package_source_id FROM package_imports WHERE id = ?1",
                [import_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("package import"))?;
        read_package_source_by_id(&connection, &source_id)
    }

    pub fn get_package_inspection_expectation(
        &self,
        import_id: &str,
    ) -> CoreResult<PackageInspectionExpectation> {
        validate_identifier("package import", import_id)?;
        let connection = self.connection()?;
        let current = read_import_state(&connection, import_id)?;
        if current.record.selection.is_some() {
            return Err(CoreError::invalid("package import already has a selection"));
        }
        Ok(PackageInspectionExpectation {
            revision: current.record.revision,
            inspection_sha256: current.inspection_sha256,
            capability_review_sha256: current.capability_review_sha256,
        })
    }

    pub fn get_package_import_expectation(
        &self,
        import_id: &str,
    ) -> CoreResult<PackageImportExpectation> {
        validate_identifier("package import", import_id)?;
        let connection = self.connection()?;
        let current = read_import_state(&connection, import_id)?;
        Ok(PackageImportExpectation {
            revision: current.record.revision,
            inspection_sha256: current.inspection_sha256,
            selection_sha256: current
                .selection_sha256
                .ok_or_else(|| CoreError::invalid("package import has no selection"))?,
            capability_review_sha256: current.capability_review_sha256,
        })
    }

    pub fn get_package_capability_review(
        &self,
        import_id: &str,
    ) -> CoreResult<PackageCapabilityReview> {
        validate_identifier("package import", import_id)?;
        let connection = self.connection()?;
        read_import_state(&connection, import_id)?;
        read_capability_review(&connection, import_id)
    }

    pub fn list_package_imports(
        &self,
        package_id: Option<&PackageId>,
    ) -> CoreResult<Vec<PackageImportRecord>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT import.id
                 FROM package_imports AS import
                 JOIN package_sources AS source
                   ON source.id = import.package_source_id
                 WHERE (?1 IS NULL OR source.package_id = ?1)
                 ORDER BY import.created_at DESC, import.id",
            )
            .map_err(storage_db_error)?;
        let ids = statement
            .query_map([package_id.map(PackageId::as_str)], |row| {
                row.get::<_, String>(0)
            })
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?;
        ids.into_iter()
            .map(|id| read_import_state(&connection, &id).map(|state| state.record))
            .collect()
    }

    /// Returns a bounded restart-discovery list of imports that can still
    /// require user action or crash recovery.
    pub fn list_pending_package_import_ids(&self, limit: u32) -> CoreResult<Vec<String>> {
        if !(1..=256).contains(&limit) {
            return Err(CoreError::invalid(
                "pending package import limit must be between 1 and 256",
            ));
        }
        let connection = self.connection()?;
        let ids = {
            let mut statement = connection
                .prepare(
                    "SELECT id
                     FROM package_imports
                     WHERE state IN (
                         'inspected',
                         'awaiting_review',
                         'approved',
                         'committing'
                     )
                     ORDER BY updated_at DESC, id
                     LIMIT ?1",
                )
                .map_err(storage_db_error)?;
            statement
                .query_map([i64::from(limit)], |row| row.get::<_, String>(0))
                .map_err(storage_db_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage_db_error)?
        };
        for id in &ids {
            let state = read_import_state(&connection, id)?;
            if !matches!(
                state.record.status,
                PackageImportStatus::Inspected
                    | PackageImportStatus::AwaitingReview
                    | PackageImportStatus::Approved
                    | PackageImportStatus::Committing
            ) {
                return Err(storage_corrupted(
                    "pending package import query returned a terminal state",
                ));
            }
        }
        Ok(ids)
    }

    /// Returns completed package import identities in deterministic restart
    /// discovery order. Callers must still resolve each identity through the
    /// exact completed-source export authority before projecting it.
    pub fn list_completed_package_import_ids(&self, limit: u32) -> CoreResult<Vec<String>> {
        if !(1..=MAX_COMPLETED_PACKAGE_EXPORTS).contains(&limit) {
            return Err(CoreError::invalid(format!(
                "completed package export limit must be between 1 and {MAX_COMPLETED_PACKAGE_EXPORTS}"
            )));
        }
        let connection = self.connection()?;
        let ids = {
            let mut statement = connection
                .prepare(
                    "SELECT id
                     FROM package_imports
                     WHERE state = 'completed'
                     ORDER BY updated_at DESC, id
                     LIMIT ?1",
                )
                .map_err(storage_db_error)?;
            statement
                .query_map([i64::from(limit)], |row| row.get::<_, String>(0))
                .map_err(storage_db_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage_db_error)?
        };
        if ids.len() > usize::try_from(limit).unwrap_or(usize::MAX) {
            return Err(storage_corrupted(
                "completed package export query exceeded its requested bound",
            ));
        }
        for id in &ids {
            validate_identifier("completed package import", id).map_err(|_| {
                storage_corrupted("completed package export query returned an invalid identity")
            })?;
            let (state, updated_at) = connection
                .query_row(
                    "SELECT state, updated_at FROM package_imports WHERE id = ?1",
                    [id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(storage_db_error)?
                .ok_or_else(|| {
                    storage_corrupted(
                        "completed package export identity disappeared during status verification",
                    )
                })?;
            if parse_import_status(&state)? != PackageImportStatus::Completed {
                return Err(storage_corrupted(
                    "completed package export query returned a non-completed state",
                ));
            }
            parse_datetime("completed package import updated_at", &updated_at)?;
        }
        Ok(ids)
    }

    pub fn list_package_import_audit(
        &self,
        import_id: &str,
    ) -> CoreResult<Vec<PackageImportAuditEvent>> {
        validate_identifier("package import", import_id)?;
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT sequence, import_revision, event_kind, payload_json,
                        payload_sha256, created_at
                 FROM package_import_audit_events
                 WHERE import_id = ?1
                 ORDER BY sequence",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map([import_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?;
        rows.into_iter()
            .map(|row| {
                let expected_hash = sha256_hex(row.3.as_bytes());
                if expected_hash != row.4 {
                    return Err(storage_corrupted(
                        "package import audit payload hash does not match",
                    ));
                }
                Ok(PackageImportAuditEvent {
                    sequence: u64_from_i64("package audit sequence", row.0)?,
                    import_revision: u64_from_i64("package audit revision", row.1)?,
                    event_kind: row.2,
                    payload: decode_json("package audit payload", &row.3)?,
                    payload_sha256: row.4,
                    created_at: parse_datetime("package audit created_at", &row.5)?,
                })
            })
            .collect()
    }
}
