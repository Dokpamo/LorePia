//! Deterministic package component selection and replay.

use super::{
    BTreeMap, BTreeSet, Connection, ContentCapability, CoreError, CoreResult, OptionalExtension,
    PackageCapability, PackageCapabilitySupport, PackageComponentDisposition, PackageComponentKind,
    PackageDocumentCommitBinding, PackageDocumentTargetDisposition, PackageDocumentTargetReview,
    PackageImportRecord, PackageImportStatus, PackageInspectionExpectation, PackageReview,
    SelectiveImportPlan, Serialize, Storage, StoredImportState, Transaction, TransactionBehavior,
    Utc, VersionedJson, append_audit, component_kind_str, encode_json, i64_from_u64,
    insert_document_target_reviews, json, load_document_target_reviews,
    package_import_target_review_sha256, params, read_capability_review, read_import_state,
    read_source_hash, reviewed_document_target, revision_conflict, sha256_hex, storage_corrupted,
    storage_db_error, validate_audit_replay, validate_binding_snapshot_shape,
    validate_document_target_reviews, validate_identifier, validate_inspection_expectation,
    validate_selection_target_review_replay,
};

impl Storage {
    /// Binds an exact deterministic selection to a durable inspection and
    /// advances `inspected -> awaiting_review`.
    #[allow(clippy::too_many_lines)] // One transaction revalidates every immutable review seam.
    pub fn select_package_import(
        &self,
        import_id: &str,
        expected: &PackageInspectionExpectation,
        selection: &SelectiveImportPlan,
        document_bindings: &[PackageDocumentCommitBinding],
    ) -> CoreResult<PackageImportRecord> {
        validate_identifier("package import", import_id)?;
        validate_inspection_expectation(expected)?;
        selection.verify().map_err(|error| {
            CoreError::invalid(format!("package selection is invalid: {error}"))
        })?;
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
        if current.record.status == PackageImportStatus::AwaitingReview
            && current.record.revision == next_revision
        {
            validate_selection_replay(
                &transaction,
                &current,
                expected,
                selection,
                document_bindings,
            )?;
            return Ok(current.record);
        }
        assert_inspection_expectation(&current, expected)?;
        if current.record.status != PackageImportStatus::Inspected
            || current.record.selection.is_some()
        {
            return Err(CoreError::invalid(
                "only an unselected inspected package import can be selected",
            ));
        }
        let review: PackageReview = serde_json::from_value(current.record.inspection.value.clone())
            .map_err(|error| {
                storage_corrupted(format!("stored package review is invalid: {error}"))
            })?;
        review.verify().map_err(|error| {
            storage_corrupted(format!("stored package review is invalid: {error}"))
        })?;
        validate_selection_against_review(&review, selection)?;
        if selection.review_sha256.as_str() != current.inspection_sha256
            || selection.package_id != current.record.package_id
            || selection.source_sha256.as_str()
                != read_source_hash(&transaction, &current.package_source_id)?
        {
            return Err(CoreError::invalid(
                "package selection does not match the durable inspection",
            ));
        }
        validate_stored_capability_decisions(
            &transaction,
            import_id,
            &selection.required_capabilities,
        )?;
        let reviewed =
            reviewed_selection_rows(&transaction, &review, selection, document_bindings)?;
        let component_review_sha256 = reviewed_component_rows_sha256(&reviewed.components)?;
        insert_reviewed_components(&transaction, import_id, &reviewed.components)?;
        insert_document_target_reviews(
            &transaction,
            import_id,
            &reviewed.components,
            &reviewed.documents,
        )?;
        let selection_wrapper = VersionedJson {
            schema_version: 1,
            value: serde_json::to_value(selection).map_err(|error| {
                CoreError::invalid(format!("package selection cannot be encoded: {error}"))
            })?,
        };
        let selection_json = encode_json("package selection", &selection_wrapper)?;
        let audit = VersionedJson {
            schema_version: 1,
            value: json!({
                "inspection_sha256": current.inspection_sha256,
                "selection_sha256": selection.plan_sha256.as_str(),
                "capability_review_sha256": current.capability_review_sha256,
                "selected_component_ids": selection.components.iter()
                    .map(|component| component.component.id.as_str())
                    .collect::<Vec<_>>(),
                "component_review_sha256": component_review_sha256,
                "target_review_sha256": reviewed.target_review_sha256,
            }),
        };
        append_audit(
            &transaction,
            import_id,
            next_revision,
            "review_requested",
            &audit,
            now,
        )?;
        let changed = transaction
            .execute(
                "UPDATE package_imports
                 SET state = 'awaiting_review', revision = ?2,
                     selection_json = ?3, selection_sha256 = ?4,
                     updated_at = ?5
                 WHERE id = ?1 AND state = 'inspected' AND revision = ?6",
                params![
                    import_id,
                    i64_from_u64("package import revision", next_revision)?,
                    selection_json,
                    selection.plan_sha256.as_str(),
                    now.to_rfc3339(),
                    i64_from_u64("package import revision", expected.revision)?,
                ],
            )
            .map_err(storage_db_error)?;
        if changed != 1 {
            return Err(revision_conflict(
                "package import",
                import_id,
                Some(expected.revision),
                None,
            ));
        }
        transaction.commit().map_err(storage_db_error)?;
        drop(connection);
        self.get_package_import(import_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct ReviewedComponentRow {
    pub(super) ordinal: u32,
    pub(super) source_component_key: String,
    pub(super) component_kind: String,
    pub(super) disposition: String,
    pub(super) selected: bool,
    pub(super) target_object_id: Option<String>,
    pub(super) target_revision_id: Option<String>,
    pub(super) review_json: String,
    pub(super) review_sha256: String,
}

pub(super) fn validate_stored_capability_decisions(
    connection: &Connection,
    import_id: &str,
    required: &[ContentCapability],
) -> CoreResult<()> {
    for capability in required {
        let capability = PackageCapability::from(*capability);
        let decision = connection
            .query_row(
                "SELECT support_status
                 FROM package_capability_requests
                 WHERE import_id = ?1 AND capability = ?2",
                params![import_id, capability.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| {
                CoreError::invalid(format!(
                    "required package capability {} has no durable review",
                    capability.as_str()
                ))
            })?;
        if decision == "unsupported" {
            return Err(CoreError::invalid(format!(
                "required package capability {} is unsupported",
                capability.as_str()
            )));
        }
    }
    Ok(())
}

pub(super) fn validate_capability_approval_snapshot(
    connection: &Connection,
    import_id: &str,
    required: &[ContentCapability],
    approved: &[PackageCapability],
) -> CoreResult<()> {
    let review = read_capability_review(connection, import_id)?;
    let required_set = required
        .iter()
        .copied()
        .map(PackageCapability::from)
        .collect::<BTreeSet<_>>();
    let mut expected_approvals = BTreeSet::new();
    for capability in required_set {
        let decision = review
            .decisions
            .iter()
            .find(|decision| decision.capability == capability)
            .ok_or_else(|| {
                storage_corrupted("required package capability review decision is missing")
            })?;
        match decision.support {
            PackageCapabilitySupport::Supported => {}
            PackageCapabilitySupport::ApprovalRequired => {
                if capability.is_never_approvable() {
                    return Err(CoreError::invalid(
                        "unsafe package capability cannot be approved",
                    ));
                }
                expected_approvals.insert(capability);
            }
            PackageCapabilitySupport::Unsupported => {
                return Err(CoreError::invalid(
                    "unsupported package capability cannot be approved",
                ));
            }
        }
    }
    let supplied = approved.iter().copied().collect::<BTreeSet<_>>();
    if supplied.len() != approved.len() || supplied != expected_approvals {
        return Err(CoreError::invalid(
            "package capability approval does not match the exact required review",
        ));
    }
    Ok(())
}

pub(super) fn validate_selection_against_review(
    review: &PackageReview,
    selection: &SelectiveImportPlan,
) -> CoreResult<()> {
    if !review.local_import_allowed {
        return Err(CoreError::invalid(
            "blocked package review cannot be selected for import",
        ));
    }
    if selection.review_sha256 != review.review_sha256
        || selection.source_sha256 != review.source_sha256
        || selection.package_id != review.manifest.package_id
        || selection.redistribution_status != review.redistribution_status
    {
        return Err(CoreError::invalid(
            "package selection differs from its exact review",
        ));
    }
    let reviewed_components = review
        .components
        .iter()
        .map(|component| (component.id.as_str(), component))
        .collect::<BTreeMap<_, _>>();
    let mut required_asset_ids = BTreeSet::new();
    for planned in &selection.components {
        let reviewed = reviewed_components
            .get(planned.component.id.as_str())
            .ok_or_else(|| CoreError::invalid("package selection contains an unknown component"))?;
        if **reviewed != planned.component
            || planned.component.disposition != PackageComponentDisposition::Importable
        {
            return Err(CoreError::invalid(
                "package selection component differs from its reviewed descriptor",
            ));
        }
        required_asset_ids.extend(
            planned
                .component
                .asset_ids
                .iter()
                .map(lorepia_domain::AssetId::as_str),
        );
    }
    let reviewed_assets = review
        .assets
        .iter()
        .map(|asset| (asset.descriptor.id.as_str(), asset))
        .collect::<BTreeMap<_, _>>();
    let selected_asset_ids = selection
        .assets
        .iter()
        .map(|asset| asset.id.as_str())
        .collect::<BTreeSet<_>>();
    if selected_asset_ids.len() != selection.assets.len()
        || !required_asset_ids.is_subset(&selected_asset_ids)
    {
        return Err(CoreError::invalid(
            "package selection asset closure is incomplete or duplicated",
        ));
    }
    for asset in &selection.assets {
        let reviewed = reviewed_assets
            .get(asset.id.as_str())
            .ok_or_else(|| CoreError::invalid("package selection contains an unknown asset"))?;
        if reviewed.descriptor != *asset
            || reviewed.disposition != lorepia_orchestration::AssetImportDisposition::Importable
        {
            return Err(CoreError::invalid(
                "package selection asset differs from its reviewed descriptor",
            ));
        }
    }
    Ok(())
}

struct ReviewedPackageSelectionRows {
    components: Vec<ReviewedComponentRow>,
    documents: Vec<PackageDocumentTargetReview>,
    target_review_sha256: String,
}

#[allow(clippy::too_many_lines)] // One pass must bind parent summaries to every child target row.
fn reviewed_selection_rows(
    connection: &Connection,
    review: &PackageReview,
    selection: &SelectiveImportPlan,
    document_bindings: &[PackageDocumentCommitBinding],
) -> CoreResult<ReviewedPackageSelectionRows> {
    validate_binding_snapshot_shape(document_bindings)?;
    let selected = selection
        .components
        .iter()
        .map(|component| component.component.id.clone())
        .collect::<BTreeSet<_>>();
    let mut bindings_by_component = BTreeMap::<String, Vec<&PackageDocumentCommitBinding>>::new();
    for binding in document_bindings {
        if !selected.contains(&binding.source_component_key) {
            return Err(CoreError::invalid(
                "package selection binding names an unselected component",
            ));
        }
        bindings_by_component
            .entry(binding.source_component_key.clone())
            .or_default()
            .push(binding);
    }
    let mut rows = Vec::with_capacity(review.components.len());
    let mut document_reviews = Vec::with_capacity(document_bindings.len());
    for (ordinal, component) in review.components.iter().enumerate() {
        let ordinal = u32::try_from(ordinal)
            .map_err(|_| CoreError::invalid("too many package components"))?;
        let component_kind = component_kind_str(component.kind);
        let is_selected = selected.contains(&component.id);
        let component_bindings = bindings_by_component
            .remove(&component.id)
            .unwrap_or_default();
        let (disposition, target_object_id, target_revision_id) = match component.disposition {
            PackageComponentDisposition::Unsupported => ("unsupported".to_owned(), None, None),
            PackageComponentDisposition::Quarantined => ("quarantine".to_owned(), None, None),
            PackageComponentDisposition::Importable if !is_selected => {
                ("skip".to_owned(), None, None)
            }
            PackageComponentDisposition::Importable
                if matches!(
                    component.kind,
                    PackageComponentKind::AssetIndex | PackageComponentKind::RawExtension
                ) =>
            {
                if !component_bindings.is_empty() {
                    return Err(CoreError::invalid(
                        "package asset selection cannot carry document bindings",
                    ));
                }
                ("create".to_owned(), None, None)
            }
            PackageComponentDisposition::Importable => {
                if component_bindings.is_empty() {
                    return Err(CoreError::invalid(
                        "selected package document has no reviewed target binding",
                    ));
                }
                let mut component_document_reviews = Vec::with_capacity(component_bindings.len());
                for binding in &component_bindings {
                    if binding.source_component_key != component.id
                        || binding.source_component_sha256 != component.sha256.as_str()
                        || binding.document_kind != component_kind
                    {
                        return Err(CoreError::invalid(
                            "package selection target differs from its reviewed component",
                        ));
                    }
                    component_document_reviews
                        .push(reviewed_document_target(connection, component, binding)?);
                }
                let update_count = component_document_reviews
                    .iter()
                    .filter(|review| review.disposition == PackageDocumentTargetDisposition::Update)
                    .count();
                let aggregate = if update_count == 0 {
                    "create"
                } else if update_count == component_document_reviews.len() {
                    "update"
                } else {
                    "conflict"
                };
                let exact_single_update = (component_document_reviews.len() == 1
                    && update_count == 1)
                    .then(|| &component_document_reviews[0]);
                let target_object_id =
                    exact_single_update.map(|review| review.target_object_id.clone());
                let target_revision_id = exact_single_update
                    .and_then(|review| review.expected_target_revision_id.clone());
                document_reviews.extend(component_document_reviews);
                (aggregate.to_owned(), target_object_id, target_revision_id)
            }
        };
        if !is_selected && !component_bindings.is_empty() {
            return Err(CoreError::invalid(
                "unselected package component cannot carry document bindings",
            ));
        }
        let review_json = encode_json("package component review", component)?;
        let review_sha256 = sha256_hex(review_json.as_bytes());
        rows.push(ReviewedComponentRow {
            ordinal,
            source_component_key: component.id.clone(),
            component_kind: component_kind.to_owned(),
            disposition,
            selected: is_selected,
            target_object_id,
            target_revision_id,
            review_json,
            review_sha256,
        });
    }
    if !bindings_by_component.is_empty() {
        return Err(CoreError::invalid(
            "package selection binding names an unknown reviewed component",
        ));
    }
    rows.sort_by_key(|row| row.ordinal);
    document_reviews.sort_by_key(|review| review.document_index);
    validate_document_target_reviews(&document_reviews)?;
    let target_review_sha256 = package_import_target_review_sha256(&document_reviews)?;
    Ok(ReviewedPackageSelectionRows {
        components: rows,
        documents: document_reviews,
        target_review_sha256,
    })
}

#[derive(Serialize)]
pub(super) struct ReviewedComponentRowDigest<'a> {
    pub(super) ordinal: u32,
    pub(super) source_component_key: &'a str,
    pub(super) component_kind: &'a str,
    pub(super) disposition: &'a str,
    pub(super) selected: bool,
    pub(super) target_object_id: Option<&'a str>,
    pub(super) target_revision_id: Option<&'a str>,
    pub(super) review_sha256: &'a str,
}

pub(super) fn reviewed_component_rows_sha256(rows: &[ReviewedComponentRow]) -> CoreResult<String> {
    let mut digests = Vec::with_capacity(rows.len());
    for row in rows {
        if sha256_hex(row.review_json.as_bytes()) != row.review_sha256 {
            return Err(storage_corrupted(
                "package component review digest does not match its payload",
            ));
        }
        digests.push(ReviewedComponentRowDigest {
            ordinal: row.ordinal,
            source_component_key: &row.source_component_key,
            component_kind: &row.component_kind,
            disposition: &row.disposition,
            selected: row.selected,
            target_object_id: row.target_object_id.as_deref(),
            target_revision_id: row.target_revision_id.as_deref(),
            review_sha256: &row.review_sha256,
        });
    }
    let encoded = encode_json("package component review rows", &digests)?;
    Ok(sha256_hex(encoded.as_bytes()))
}

pub(super) fn load_reviewed_components(
    connection: &Connection,
    import_id: &str,
) -> CoreResult<Vec<ReviewedComponentRow>> {
    let mut statement = connection
        .prepare(
            "SELECT ordinal, source_component_key, component_kind,
                    disposition, selected, target_object_id,
                    target_revision_id, review_json, review_sha256
             FROM package_import_components
             WHERE import_id = ?1
             ORDER BY ordinal",
        )
        .map_err(storage_db_error)?;
    statement
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
        .map_err(storage_db_error)
}

pub(super) fn insert_reviewed_components(
    transaction: &Transaction<'_>,
    import_id: &str,
    rows: &[ReviewedComponentRow],
) -> CoreResult<()> {
    for row in rows {
        transaction
            .execute(
                "INSERT INTO package_import_components (
                    import_id, ordinal, source_component_key, component_kind,
                    disposition, selected, target_object_id,
                    target_revision_id, review_json, review_sha256
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    import_id,
                    i64::from(row.ordinal),
                    row.source_component_key,
                    row.component_kind,
                    row.disposition,
                    row.selected,
                    row.target_object_id,
                    row.target_revision_id,
                    row.review_json,
                    row.review_sha256,
                ],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

pub(super) fn assert_inspection_expectation(
    current: &StoredImportState,
    expected: &PackageInspectionExpectation,
) -> CoreResult<()> {
    if current.record.revision != expected.revision
        || current.inspection_sha256 != expected.inspection_sha256
        || current.capability_review_sha256 != expected.capability_review_sha256
    {
        return Err(revision_conflict(
            "package import",
            &current.record.id,
            Some(expected.revision),
            Some(current.record.revision),
        ));
    }
    Ok(())
}

pub(super) fn decode_selection(record: &PackageImportRecord) -> CoreResult<SelectiveImportPlan> {
    let wrapper = record
        .selection
        .as_ref()
        .ok_or_else(|| storage_corrupted("package selection is missing"))?;
    serde_json::from_value(wrapper.value.clone())
        .map_err(|error| storage_corrupted(format!("stored package selection is invalid: {error}")))
}

pub(super) fn validate_selection_replay(
    connection: &Connection,
    current: &StoredImportState,
    expected: &PackageInspectionExpectation,
    selection: &SelectiveImportPlan,
    document_bindings: &[PackageDocumentCommitBinding],
) -> CoreResult<()> {
    let next_revision = expected
        .revision
        .checked_add(1)
        .ok_or_else(|| CoreError::invalid("package selection replay revision overflow"))?;
    let selection_value = serde_json::to_value(selection).map_err(|error| {
        CoreError::invalid(format!("package selection cannot be encoded: {error}"))
    })?;
    let selected = selection
        .components
        .iter()
        .map(|component| component.component.id.clone())
        .collect::<BTreeSet<_>>();
    let stored_selected = current
        .record
        .selected_component_ids
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    if current.record.status != PackageImportStatus::AwaitingReview
        || current.record.revision != next_revision
        || current.inspection_sha256 != expected.inspection_sha256
        || current.capability_review_sha256 != expected.capability_review_sha256
        || current.selection_sha256.as_deref() != Some(selection.plan_sha256.as_str())
        || selection.review_sha256.as_str() != expected.inspection_sha256
        || selection.package_id != current.record.package_id
        || current
            .record
            .selection
            .as_ref()
            .is_none_or(|wrapper| wrapper.schema_version != 1 || wrapper.value != selection_value)
        || selected.len() != selection.components.len()
        || stored_selected.len() != current.record.selected_component_ids.len()
        || stored_selected != selected
        || read_source_hash(connection, &current.package_source_id)?
            != selection.source_sha256.as_str()
    {
        return Err(revision_conflict(
            "package selection replay",
            &current.record.id,
            Some(expected.revision),
            Some(current.record.revision),
        ));
    }
    let review: PackageReview = serde_json::from_value(current.record.inspection.value.clone())
        .map_err(|error| storage_corrupted(format!("stored package review is invalid: {error}")))?;
    review
        .verify()
        .map_err(|error| storage_corrupted(format!("stored package review is invalid: {error}")))?;
    validate_selection_against_review(&review, selection)?;
    let stored_rows = load_reviewed_components(connection, &current.record.id)?;
    let stored_documents = load_document_target_reviews(connection, &current.record.id)?;
    validate_selection_target_review_replay(
        &stored_rows,
        &stored_documents,
        selection,
        document_bindings,
    )?;
    let target_review_sha256 = package_import_target_review_sha256(&stored_documents)?;
    let component_review_sha256 = reviewed_component_rows_sha256(&stored_rows)?;
    let audit = VersionedJson {
        schema_version: 1,
        value: json!({
            "inspection_sha256": current.inspection_sha256,
            "selection_sha256": selection.plan_sha256.as_str(),
            "capability_review_sha256": current.capability_review_sha256,
            "selected_component_ids": selection.components.iter()
                .map(|component| component.component.id.as_str())
                .collect::<Vec<_>>(),
            "component_review_sha256": component_review_sha256,
            "target_review_sha256": target_review_sha256,
        }),
    };
    validate_audit_replay(
        connection,
        &current.record.id,
        next_revision,
        "review_requested",
        &audit,
    )
}
