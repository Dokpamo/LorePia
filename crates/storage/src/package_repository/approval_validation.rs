//! Approval replay, capability, expectation, and normalization validation.

use super::{
    BTreeMap, BTreeSet, Connection, ContentCapability, CoreError, CoreResult,
    MAX_NORMALIZATION_REASON_BYTES, OptionalExtension, PackageApprovalPayload, PackageCapability,
    PackageCapabilitySupport, PackageCommitDocument, PackageDocumentCommitBinding,
    PackageImportExpectation, PackageImportStatus, PackageInspectionExpectation,
    PackageNormalizationEvidence, ReviewedComponentRow, StoredImportState, VersionedJson,
    canonical_update_target_confirmations, decode_json, i64_from_u64,
    load_package_import_target_review, load_selected_commit_components,
    package_normalization_evidence_sha256, package_update_target_confirmations_sha256, params,
    read_capability_review, read_import_state, read_source_hash, revision_conflict, sha256_hex,
    storage_corrupted, storage_db_error, validate_binding_snapshot_shape,
    validate_exact_update_target_confirmations, validate_identifier, validate_sha256,
    validate_target_review_binding_snapshot,
};

pub(super) fn assert_expectation(
    current: &StoredImportState,
    expected: &PackageImportExpectation,
) -> CoreResult<()> {
    if current.record.revision != expected.revision
        || current.inspection_sha256 != expected.inspection_sha256
        || current.selection_sha256.as_deref() != Some(&expected.selection_sha256)
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

pub(super) fn validate_approval_replay(
    connection: &Connection,
    current: &StoredImportState,
    expected: &PackageImportExpectation,
    approval: &PackageApprovalPayload,
    audit: &VersionedJson,
) -> CoreResult<()> {
    let next_revision = expected
        .revision
        .checked_add(1)
        .ok_or_else(|| CoreError::invalid("package approval replay revision overflow"))?;
    if current.record.status != PackageImportStatus::Approved
        || current.record.revision != next_revision
        || current.inspection_sha256 != expected.inspection_sha256
        || current.selection_sha256.as_deref() != Some(&expected.selection_sha256)
        || current.capability_review_sha256 != expected.capability_review_sha256
        || current.approved_selection_sha256.as_deref() != Some(&expected.selection_sha256)
        || current.approved_at.is_none()
        || approval.plan.review_sha256.as_str() != expected.inspection_sha256
        || approval.plan.plan_sha256.as_str() != expected.selection_sha256
        || approval.plan.package_id != current.record.package_id
        || read_source_hash(connection, &current.package_source_id)?
            != approval.plan.source_sha256.as_str()
    {
        return Err(revision_conflict(
            "package approval replay",
            &current.record.id,
            Some(expected.revision),
            Some(current.record.revision),
        ));
    }
    if read_approval_payload(connection, &current.record.id)? != *approval {
        return Err(CoreError::invalid(
            "package approval retry differs from the immutable approval snapshot",
        ));
    }
    validate_audit_replay(
        connection,
        &current.record.id,
        next_revision,
        "approved",
        audit,
    )
}

pub(super) fn validate_audit_replay(
    connection: &Connection,
    import_id: &str,
    revision: u64,
    event_kind: &str,
    expected: &VersionedJson,
) -> CoreResult<()> {
    let row = connection
        .query_row(
            "SELECT event_kind, payload_json, payload_sha256
             FROM package_import_audit_events
             WHERE import_id = ?1 AND import_revision = ?2",
            params![import_id, i64_from_u64("package audit revision", revision)?],
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
        .ok_or_else(|| storage_corrupted("package transition has no matching audit event"))?;
    if sha256_hex(row.1.as_bytes()) != row.2 {
        return Err(storage_corrupted(
            "package transition audit payload hash does not match",
        ));
    }
    let stored: VersionedJson = decode_json("package transition audit payload", &row.1)?;
    if row.0 != event_kind || stored != *expected {
        return Err(CoreError::invalid(
            "package transition retry differs from its immutable audit event",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_lines)] // Replay rejects drift across every immutable approval seam.
pub(super) fn read_approval_payload(
    connection: &Connection,
    import_id: &str,
) -> CoreResult<PackageApprovalPayload> {
    let payload = connection
        .query_row(
            "SELECT approval_payload_json
             FROM package_import_approvals
             WHERE import_id = ?1
             ORDER BY approved_at DESC, id
             LIMIT 1",
            [import_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| storage_corrupted("approved package import has no approval snapshot"))?;
    let wrapper: VersionedJson = decode_json("package approval", &payload)?;
    if wrapper.schema_version != 1 {
        return Err(storage_corrupted(
            "stored package approval wrapper schema is unsupported",
        ));
    }
    let approved: PackageApprovalPayload =
        serde_json::from_value(wrapper.value).map_err(|error| {
            storage_corrupted(format!("stored package approval is invalid: {error}"))
        })?;
    approved.plan.verify().map_err(|error| {
        storage_corrupted(format!("stored package approval is invalid: {error}"))
    })?;
    validate_binding_snapshot_shape(&approved.document_bindings).map_err(|error| {
        storage_corrupted(format!(
            "stored package document binding snapshot is invalid: {}",
            error.message
        ))
    })?;
    validate_sha256("package target review", &approved.target_review_sha256).map_err(|error| {
        storage_corrupted(format!(
            "stored package target-review digest is invalid: {}",
            error.message
        ))
    })?;
    let canonical_confirmations = canonical_update_target_confirmations(
        &approved.confirmed_update_targets,
    )
    .map_err(|error| {
        storage_corrupted(format!(
            "stored package update confirmations are invalid: {}",
            error.message
        ))
    })?;
    if canonical_confirmations != approved.confirmed_update_targets {
        return Err(storage_corrupted(
            "stored package update confirmations are not canonical",
        ));
    }
    let confirmation_sha256 = package_update_target_confirmations_sha256(&canonical_confirmations)
        .map_err(|error| {
            storage_corrupted(format!(
                "stored package update confirmations cannot be hashed: {}",
                error.message
            ))
        })?;
    if approved.plan.target_review_sha256.as_str() != approved.target_review_sha256
        || approved.plan.update_target_confirmations_sha256.as_str() != confirmation_sha256
    {
        return Err(storage_corrupted(
            "stored package approval hash is detached from target review confirmations",
        ));
    }
    if !approved
        .approved_capabilities
        .windows(2)
        .all(|pair| pair[0] < pair[1])
    {
        return Err(storage_corrupted(
            "stored package capability approvals are not canonical",
        ));
    }
    validate_capability_approval_snapshot(
        connection,
        import_id,
        &approved.plan.required_capabilities,
        &approved.approved_capabilities,
    )
    .map_err(|error| {
        storage_corrupted(format!(
            "stored package capability approval is invalid: {}",
            error.message
        ))
    })?;
    validate_normalization_evidence_shape(&approved.normalization_evidence).map_err(|error| {
        storage_corrupted(format!(
            "stored package normalization evidence is invalid: {}",
            error.message
        ))
    })?;
    let evidence_sha256 = package_normalization_evidence_sha256(&approved.normalization_evidence)
        .map_err(|error| {
        storage_corrupted(format!(
            "stored package normalization evidence cannot be hashed: {}",
            error.message
        ))
    })?;
    if evidence_sha256 != approved.normalization_evidence_sha256 {
        return Err(storage_corrupted(
            "stored package normalization evidence hash does not match",
        ));
    }
    let components = load_selected_commit_components(connection, import_id)?;
    let current = read_import_state(connection, import_id)?;
    let target_review = load_package_import_target_review(connection, &current)?;
    if approved.target_review_sha256 != target_review.target_review_sha256 {
        return Err(storage_corrupted(
            "stored package approval target-review digest does not match selection",
        ));
    }
    validate_target_review_binding_snapshot(
        &approved.document_bindings,
        &components,
        &target_review,
    )
    .map_err(|error| {
        storage_corrupted(format!(
            "stored package approval target-review binding is invalid: {}",
            error.message
        ))
    })?;
    validate_exact_update_target_confirmations(
        &target_review.documents,
        &approved.confirmed_update_targets,
    )
    .map_err(|error| {
        storage_corrupted(format!(
            "stored package update confirmations differ from target review: {}",
            error.message
        ))
    })?;
    validate_normalization_evidence_linkage(
        &approved.normalization_evidence,
        &approved.document_bindings,
        &components,
    )
    .map_err(|error| {
        storage_corrupted(format!(
            "stored package normalization evidence linkage is invalid: {}",
            error.message
        ))
    })?;
    Ok(approved)
}

pub(super) fn validate_normalization_evidence_shape(
    evidence: &[PackageNormalizationEvidence],
) -> CoreResult<()> {
    if evidence.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(CoreError::invalid(
            "package normalization evidence must be unique and canonically ordered",
        ));
    }
    let mut keys = BTreeSet::new();
    for entry in evidence {
        validate_identifier("package normalization component", &entry.component_id)?;
        validate_identifier("package normalization object", &entry.object_id)?;
        if !keys.insert((
            entry.component_id.as_str(),
            entry.object_id.as_str(),
            entry.field.as_str(),
        )) || !matches!(entry.field.as_str(), "enabled" | "imported_enabled")
            || entry.after
            || entry.reason.trim().is_empty()
            || entry.reason.len() > MAX_NORMALIZATION_REASON_BYTES
            || entry.reason.chars().any(char::is_control)
        {
            return Err(CoreError::invalid(
                "package normalization evidence is invalid",
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_normalization_evidence_linkage(
    evidence: &[PackageNormalizationEvidence],
    bindings: &[PackageDocumentCommitBinding],
    components: &BTreeMap<String, ReviewedComponentRow>,
) -> CoreResult<()> {
    validate_normalization_evidence_shape(evidence)?;
    let bound_components = bindings
        .iter()
        .map(|binding| binding.source_component_key.as_str())
        .collect::<BTreeSet<_>>();
    for entry in evidence {
        if !bound_components.contains(entry.component_id.as_str()) {
            return Err(CoreError::invalid(
                "package normalization evidence names an unbound component",
            ));
        }
        let component = components.get(&entry.component_id).ok_or_else(|| {
            CoreError::invalid("package normalization evidence names an unknown component")
        })?;
        if !matches!(
            component.component_kind.as_str(),
            "transform_set" | "interaction_rule_set"
        ) || (entry.field == "imported_enabled" && component.component_kind != "transform_set")
        {
            return Err(CoreError::invalid(
                "package normalization evidence does not match declarative component type",
            ));
        }
    }
    for binding in bindings
        .iter()
        .filter(|binding| binding.document_kind == "transform_set")
    {
        if !evidence.iter().any(|entry| {
            entry.component_id == binding.source_component_key
                && entry.object_id == binding.target_object_id
                && entry.field == "enabled"
                && !entry.after
        }) {
            return Err(CoreError::invalid(
                "imported transform set lacks immutable disabled-state evidence",
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_document_normalization_evidence(
    documents: &[PackageCommitDocument],
    bindings: &[PackageDocumentCommitBinding],
    evidence: &[PackageNormalizationEvidence],
) -> CoreResult<()> {
    let mut expected = BTreeMap::<(String, String, String), Option<bool>>::new();
    let mut expected_entries = 0_usize;
    for (document, binding) in documents.iter().zip(bindings) {
        let component_id = binding.source_component_key.clone();
        match document {
            PackageCommitDocument::TransformSet(set) => {
                expected.insert(
                    (
                        component_id.clone(),
                        set.id.as_str().to_owned(),
                        "enabled".to_owned(),
                    ),
                    Some(set.imported_author_enabled),
                );
                expected_entries = expected_entries.saturating_add(1);
                for rule in &set.rules {
                    expected.insert(
                        (
                            component_id.clone(),
                            rule.id.as_str().to_owned(),
                            "enabled".to_owned(),
                        ),
                        Some(rule.imported_author_enabled),
                    );
                    expected_entries = expected_entries.saturating_add(1);
                    expected.insert(
                        (
                            component_id.clone(),
                            rule.id.as_str().to_owned(),
                            "imported_enabled".to_owned(),
                        ),
                        None,
                    );
                    expected_entries = expected_entries.saturating_add(1);
                }
            }
            PackageCommitDocument::InteractionRuleSet(set) => {
                for rule in &set.rules {
                    expected.insert(
                        (
                            component_id.clone(),
                            rule.id.as_str().to_owned(),
                            "enabled".to_owned(),
                        ),
                        Some(rule.imported_author_enabled),
                    );
                    expected_entries = expected_entries.saturating_add(1);
                }
            }
            PackageCommitDocument::PromptPreset(_)
            | PackageCommitDocument::KnowledgeBook(_)
            | PackageCommitDocument::MemoryProfile(_)
            | PackageCommitDocument::ContentModule(_)
            | PackageCommitDocument::CharacterContent { .. } => {}
        }
    }
    let actual = evidence
        .iter()
        .map(|entry| {
            (
                (
                    entry.component_id.clone(),
                    entry.object_id.clone(),
                    entry.field.clone(),
                ),
                entry,
            )
        })
        .collect::<BTreeMap<_, _>>();
    if expected.len() != expected_entries
        || actual.len() != evidence.len()
        || actual.keys().ne(expected.keys())
        || expected.iter().any(|(key, expected_before)| {
            expected_before.is_some_and(|value| actual[key].before != value)
        })
    {
        return Err(CoreError::invalid(
            "package normalization evidence differs from normalized document author intent",
        ));
    }
    Ok(())
}

pub(super) fn validate_expectation(expected: &PackageImportExpectation) -> CoreResult<()> {
    if expected.revision == 0 {
        return Err(CoreError::invalid(
            "package import expected revision must be positive",
        ));
    }
    validate_sha256("package inspection", &expected.inspection_sha256)?;
    validate_sha256("package selection", &expected.selection_sha256)?;
    validate_sha256(
        "package capability review",
        &expected.capability_review_sha256,
    )
}

pub(super) fn validate_inspection_expectation(
    expected: &PackageInspectionExpectation,
) -> CoreResult<()> {
    if expected.revision == 0 {
        return Err(CoreError::invalid(
            "package inspection expected revision must be positive",
        ));
    }
    validate_sha256("package inspection", &expected.inspection_sha256)?;
    validate_sha256(
        "package capability review",
        &expected.capability_review_sha256,
    )
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
