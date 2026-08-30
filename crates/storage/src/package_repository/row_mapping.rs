//! Package repository row insertion, hydration, and lifecycle writes.

use super::{
    BTreeSet, Connection, CoreError, CoreResult, DateTime, OptionalExtension,
    PackageCapabilityDecision, PackageCapabilityReview, PackageId, PackageImportRecord,
    PackageImportStatus, PackageReview, PackageSourceRecord, SelectiveImportPlan, Transaction, Utc,
    Value, VersionedJson, decode_json, encode_json, i64_from_u64, import_status_str,
    license_fields, not_found, package_capability_review_sha256, params, parse_capability_support,
    parse_datetime, parse_import_status, parse_package_capability, revision_conflict, sha256_hex,
    storage_corrupted, storage_db_error, u32_from_i64, u64_from_i64, validate_source_record,
};

#[derive(Debug)]
pub(super) struct StoredImportState {
    pub(super) record: PackageImportRecord,
    pub(super) package_source_id: String,
    pub(super) inspection_sha256: String,
    pub(super) selection_sha256: Option<String>,
    pub(super) capability_review_sha256: String,
    pub(super) approved_selection_sha256: Option<String>,
    pub(super) approved_at: Option<DateTime<Utc>>,
}

pub(super) fn insert_package_source(
    transaction: &Transaction<'_>,
    source: &PackageSourceRecord,
) -> CoreResult<()> {
    validate_source_record(source)?;
    let content_source = transaction
        .query_row(
            "SELECT size_bytes FROM content_sources WHERE sha256 = ?1",
            [source.source_sha256.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            CoreError::invalid(
                "package source bytes must be durable in content-addressed storage first",
            )
        })?;
    if u64_from_i64("package source size", content_source)? != source.source_size_bytes {
        return Err(CoreError::invalid(
            "package source size does not match content-addressed storage",
        ));
    }
    if let Some(existing) = read_package_source(transaction, "source.id = ?1", &source.id)? {
        if existing == *source {
            return Ok(());
        }
        return Err(CoreError::invalid(
            "package source conflicts with an existing immutable source",
        ));
    }
    if let Some(existing) = read_package_source(
        transaction,
        "source.source_hash = ?1",
        &source.source_sha256,
    )? {
        if existing == *source {
            return Ok(());
        }
        return Err(CoreError::invalid(
            "package source hash is already bound to different immutable metadata",
        ));
    }
    let manifest_json = encode_json("package source manifest", &source.manifest)?;
    let manifest_sha256 = sha256_hex(manifest_json.as_bytes());
    let (license_expression, license_status) = license_fields(&source.license);
    let redistribution_status = if source.redistribution_allowed {
        "allowed"
    } else {
        "denied"
    };
    let signature = source
        .manifest
        .value
        .get("signature")
        .filter(|value| !value.is_null());
    let signature_json = signature
        .map(|value| encode_json("package signature", value))
        .transpose()?;
    let signature_status = if signature.is_some() {
        "untrusted"
    } else {
        "unsigned"
    };
    let required_app_version = source
        .manifest
        .value
        .get("required_app_version")
        .and_then(Value::as_str);
    transaction
        .execute(
            "INSERT INTO package_sources (
                id, source_hash, format, format_version, package_id, name,
                version, author, manifest_json, manifest_sha256,
                license_expression, license_status, redistribution_status,
                required_app_version, signature_json, signature_status,
                created_at
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                ?14, ?15, ?16, ?17
             )",
            params![
                source.id,
                source.source_sha256,
                source.format,
                i64::from(source.format_version),
                source.package_id.as_str(),
                source.name,
                source.version,
                source.author,
                manifest_json,
                manifest_sha256,
                license_expression,
                license_status,
                redistribution_status,
                required_app_version,
                signature_json,
                signature_status,
                source.created_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

pub(super) fn read_package_source(
    connection: &Connection,
    predicate: &str,
    value: &str,
) -> CoreResult<Option<PackageSourceRecord>> {
    let sql = format!(
        "SELECT source.id, source.package_id, source.format,
                source.format_version, source.name, source.version,
                source.source_hash, bytes.size_bytes, source.author,
                source.license_expression, source.license_status,
                source.redistribution_status, source.manifest_json,
                source.manifest_sha256,
                source.created_at
         FROM package_sources AS source
         JOIN content_sources AS bytes ON bytes.sha256 = source.source_hash
         WHERE {predicate}
         ORDER BY source.created_at, source.id
         LIMIT 1"
    );
    connection
        .query_row(&sql, [value], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, String>(10)?,
                row.get::<_, String>(11)?,
                row.get::<_, String>(12)?,
                row.get::<_, String>(13)?,
                row.get::<_, String>(14)?,
            ))
        })
        .optional()
        .map_err(storage_db_error)?
        .map(|row| {
            if sha256_hex(row.12.as_bytes()) != row.13 {
                return Err(storage_corrupted(
                    "stored package manifest hash does not match",
                ));
            }
            let manifest = decode_json("package source manifest", &row.12)?;
            let license = row.9.unwrap_or_else(|| match row.10.as_str() {
                "unknown" => "LicenseRef-Unknown".to_owned(),
                "invalid" => "LicenseRef-Invalid".to_owned(),
                _ => String::new(),
            });
            Ok(PackageSourceRecord {
                id: row.0,
                package_id: PackageId::from(row.1),
                format: row.2,
                format_version: u32_from_i64("package format version", row.3)?,
                name: row.4,
                version: row.5,
                source_sha256: row.6,
                source_size_bytes: u64_from_i64("package source size", row.7)?,
                author: row.8,
                license,
                redistribution_allowed: row.11 == "allowed",
                manifest,
                created_at: parse_datetime("package source created_at", &row.14)?,
            })
        })
        .transpose()
}

pub(super) fn read_package_source_by_id(
    connection: &Connection,
    id: &str,
) -> CoreResult<PackageSourceRecord> {
    read_package_source(connection, "source.id = ?1", id)?
        .ok_or_else(|| storage_corrupted("package import source is missing"))
}

pub(super) fn read_source_hash(connection: &Connection, source_id: &str) -> CoreResult<String> {
    connection
        .query_row(
            "SELECT source_hash FROM package_sources WHERE id = ?1",
            [source_id],
            |row| row.get(0),
        )
        .map_err(storage_db_error)
}

pub(super) fn insert_capability_review(
    transaction: &Transaction<'_>,
    import_id: &str,
    review: &PackageCapabilityReview,
) -> CoreResult<()> {
    for decision in &review.decisions {
        transaction
            .execute(
                "INSERT INTO package_capability_requests (
                    import_id, capability, support_status, approved,
                    executable, reason
                 ) VALUES (?1, ?2, ?3, ?4, 0, ?5)",
                params![
                    import_id,
                    decision.capability.as_str(),
                    decision.support.as_str(),
                    decision.approved,
                    decision.reason,
                ],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

#[allow(clippy::too_many_lines)] // A read rejects cross-table snapshot drift before returning state.
pub(super) fn read_import_state(
    connection: &Connection,
    id: &str,
) -> CoreResult<StoredImportState> {
    let row = connection
        .query_row(
            "SELECT source.package_id, import.state, import.revision,
                    import.inspection_json, import.inspection_sha256,
                    import.selection_json, import.selection_sha256,
                    import.capability_review_sha256,
                    import.approved_selection_sha256, import.approved_at,
                    import.failure_json, import.created_at, import.updated_at,
                    import.completed_at, import.package_source_id
             FROM package_imports AS import
             JOIN package_sources AS source
               ON source.id = import.package_source_id
             WHERE import.id = ?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, String>(11)?,
                    row.get::<_, String>(12)?,
                    row.get::<_, Option<String>>(13)?,
                    row.get::<_, String>(14)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("package import"))?;
    let selected_component_ids = {
        let mut statement = connection
            .prepare(
                "SELECT source_component_key
                 FROM package_import_components
                 WHERE import_id = ?1 AND selected = 1
                 ORDER BY source_component_key",
            )
            .map_err(storage_db_error)?;
        statement
            .query_map([id], |row| row.get::<_, String>(0))
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?
    };
    let inspection: VersionedJson = decode_json("package inspection", &row.3)?;
    if inspection.schema_version != 1 {
        return Err(storage_corrupted(
            "package inspection wrapper schema is unsupported",
        ));
    }
    let review: PackageReview = serde_json::from_value(inspection.value.clone())
        .map_err(|error| storage_corrupted(format!("stored package review is invalid: {error}")))?;
    review
        .verify()
        .map_err(|error| storage_corrupted(format!("stored package review is invalid: {error}")))?;
    let source_hash = read_source_hash(connection, &row.14)?;
    if review.review_sha256.as_str() != row.4
        || review.source_sha256.as_str() != source_hash
        || review.manifest.package_id.as_str() != row.0
    {
        return Err(storage_corrupted(
            "stored package inspection differs from its durable identity",
        ));
    }
    let selection: Option<VersionedJson> = row
        .5
        .as_deref()
        .map(|json| decode_json("package selection", json))
        .transpose()?;
    if selection.is_some() != row.6.is_some() {
        return Err(storage_corrupted(
            "package selection JSON and hash presence differ",
        ));
    }
    if let Some(wrapper) = &selection {
        if wrapper.schema_version != 1 {
            return Err(storage_corrupted(
                "package selection wrapper schema is unsupported",
            ));
        }
        let plan: SelectiveImportPlan =
            serde_json::from_value(wrapper.value.clone()).map_err(|error| {
                storage_corrupted(format!("stored package selection is invalid: {error}"))
            })?;
        plan.verify().map_err(|error| {
            storage_corrupted(format!("stored package selection is invalid: {error}"))
        })?;
        let selected = plan
            .components
            .iter()
            .map(|component| component.component.id.as_str())
            .collect::<BTreeSet<_>>();
        let stored_selected = selected_component_ids
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        if plan.plan_sha256.as_str() != row.6.as_deref().unwrap_or_default()
            || plan.review_sha256.as_str() != row.4
            || plan.source_sha256.as_str() != source_hash
            || plan.package_id.as_str() != row.0
            || selected.len() != plan.components.len()
            || stored_selected.len() != selected_component_ids.len()
            || stored_selected != selected
        {
            return Err(storage_corrupted(
                "stored package selection differs from its durable identity",
            ));
        }
    } else if !selected_component_ids.is_empty() {
        return Err(storage_corrupted(
            "unselected package import contains selected component rows",
        ));
    }
    let failure_code = row
        .10
        .as_deref()
        .map(|json| {
            let value: Value = decode_json("package failure", json)?;
            value
                .get("code")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| storage_corrupted("package failure has no code"))
        })
        .transpose()?;
    let _completed_at = row
        .13
        .as_deref()
        .map(|value| parse_datetime("package import completed_at", value))
        .transpose()?;
    let capability_review = read_capability_review(connection, id)?;
    if package_capability_review_sha256(&capability_review)? != row.7 {
        return Err(storage_corrupted(
            "stored package capability review hash does not match",
        ));
    }
    Ok(StoredImportState {
        record: PackageImportRecord {
            id: id.to_owned(),
            package_id: PackageId::from(row.0),
            status: parse_import_status(&row.1)?,
            revision: u64_from_i64("package import revision", row.2)?,
            inspection,
            selection,
            selected_component_ids,
            failure_code,
            created_at: parse_datetime("package import created_at", &row.11)?,
            updated_at: parse_datetime("package import updated_at", &row.12)?,
        },
        package_source_id: row.14,
        inspection_sha256: row.4,
        selection_sha256: row.6,
        capability_review_sha256: row.7,
        approved_selection_sha256: row.8,
        approved_at: row
            .9
            .as_deref()
            .map(|value| parse_datetime("package import approved_at", value))
            .transpose()?,
    })
}

pub(super) fn read_capability_review(
    connection: &Connection,
    import_id: &str,
) -> CoreResult<PackageCapabilityReview> {
    let mut statement = connection
        .prepare(
            "SELECT capability, support_status, approved, reason
             FROM package_capability_requests
             WHERE import_id = ?1
             ORDER BY capability",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map([import_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, bool>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    Ok(PackageCapabilityReview {
        schema_version: 1,
        decisions: rows
            .into_iter()
            .map(|row| {
                Ok(PackageCapabilityDecision {
                    capability: parse_package_capability(&row.0)?,
                    support: parse_capability_support(&row.1)?,
                    approved: row.2,
                    reason: row.3,
                })
            })
            .collect::<CoreResult<Vec<_>>>()?,
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn update_import_state(
    transaction: &Transaction<'_>,
    import_id: &str,
    old_revision: u64,
    status: PackageImportStatus,
    new_revision: u64,
    approved_selection_sha256: Option<&str>,
    approved_at: Option<DateTime<Utc>>,
    failure_json: Option<&str>,
    completed_at: Option<DateTime<Utc>>,
    updated_at: DateTime<Utc>,
) -> CoreResult<()> {
    let changed = transaction
        .execute(
            "UPDATE package_imports
             SET state = ?2, revision = ?3,
                 approved_selection_sha256 = ?4, approved_at = ?5,
                 failure_json = ?6, completed_at = ?7, updated_at = ?8
             WHERE id = ?1 AND revision = ?9",
            params![
                import_id,
                import_status_str(status),
                i64_from_u64("package import revision", new_revision)?,
                approved_selection_sha256,
                approved_at.map(|value| value.to_rfc3339()),
                failure_json,
                completed_at.map(|value| value.to_rfc3339()),
                updated_at.to_rfc3339(),
                i64_from_u64("package import revision", old_revision)?,
            ],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(revision_conflict(
            "package import",
            import_id,
            Some(old_revision),
            None,
        ));
    }
    Ok(())
}

pub(super) fn append_audit(
    transaction: &Transaction<'_>,
    import_id: &str,
    import_revision: u64,
    event_kind: &str,
    payload: &VersionedJson,
    created_at: DateTime<Utc>,
) -> CoreResult<()> {
    let payload_json = encode_json("package import audit payload", payload)?;
    let payload_sha256 = sha256_hex(payload_json.as_bytes());
    let sequence = transaction
        .query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1
             FROM package_import_audit_events
             WHERE import_id = ?1",
            [import_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(storage_db_error)?;
    transaction
        .execute(
            "INSERT INTO package_import_audit_events (
                import_id, sequence, import_revision, event_kind,
                payload_json, payload_sha256, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                import_id,
                sequence,
                i64_from_u64("package import revision", import_revision)?,
                event_kind,
                payload_json,
                payload_sha256,
                created_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}
