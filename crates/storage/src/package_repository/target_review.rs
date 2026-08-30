//! Immutable package document target review and confirmation validation.

use super::{
    BTreeMap, BTreeSet, Connection, CoreError, CoreResult, MAX_PACKAGE_TARGET_REVIEW_DOCUMENTS,
    OptionalExtension, PackageDocumentCommitBinding, PackageDocumentTargetDisposition,
    PackageDocumentTargetReview, PackageImportTargetReview, PackageUpdateTargetConfirmation,
    ReviewedComponentRow, SelectiveImportPlan, StoredImportState, Transaction, Value,
    VersionedJson, component_kind_str, decode_json, encode_json, i64_from_u64,
    load_reviewed_components, params, reviewed_component_rows_sha256, sha256_hex,
    storage_corrupted, storage_db_error, u64_from_i64, validate_binding_snapshot_shape,
    validate_identifier, validate_sha256,
};

pub(super) fn reviewed_document_target(
    connection: &Connection,
    component: &lorepia_orchestration::PackageComponentDescriptor,
    binding: &PackageDocumentCommitBinding,
) -> CoreResult<PackageDocumentTargetReview> {
    let (disposition, expected_target_revision_id, expected_target_state_revision) =
        if let Some(expected_revision) = binding.expected_object_revision {
            let target = connection
                .query_row(
                    "SELECT object.object_kind, object.deleted_at,
                    state.state_version, state.active_revision_id
             FROM content_objects AS object
             JOIN content_object_state AS state
               ON state.object_id = object.id
             WHERE object.id = ?1",
                    [binding.target_object_id.as_str()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )
                .optional()
                .map_err(storage_db_error)?
                .ok_or_else(|| {
                    CoreError::invalid("package update target is missing at review time")
                })?;
            if target.0 != binding.document_kind
                || target.1.is_some()
                || u64_from_i64("content state revision", target.2)? != expected_revision
            {
                return Err(CoreError::invalid(
                    "package update target changed before selection was stored",
                ));
            }
            (
                PackageDocumentTargetDisposition::Update,
                Some(target.3),
                Some(expected_revision),
            )
        } else {
            let target_exists = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM content_objects WHERE id = ?1)",
                    [binding.target_object_id.as_str()],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(storage_db_error)?;
            if target_exists {
                return Err(CoreError::invalid(
                    "new package target appeared before selection was stored",
                ));
            }
            (PackageDocumentTargetDisposition::Create, None, None)
        };
    Ok(PackageDocumentTargetReview {
        source_component_id: component.id.clone(),
        component_document_ordinal: binding.component_document_ordinal,
        document_index: binding.document_index,
        document_kind: binding.document_kind.clone(),
        target_object_id: binding.target_object_id.clone(),
        disposition,
        expected_target_revision_id,
        expected_target_state_revision,
        source_component_sha256: binding.source_component_sha256.clone(),
        document_sha256: binding.document_sha256.clone(),
    })
}

pub(super) fn validate_document_target_reviews(
    documents: &[PackageDocumentTargetReview],
) -> CoreResult<()> {
    if documents.len() > MAX_PACKAGE_TARGET_REVIEW_DOCUMENTS {
        return Err(CoreError::invalid(format!(
            "package target review exceeds the {MAX_PACKAGE_TARGET_REVIEW_DOCUMENTS}-document limit"
        )));
    }
    let mut component_documents = BTreeSet::new();
    let mut target_ids = BTreeSet::new();
    let mut ordinals_by_component = BTreeMap::<&str, Vec<u32>>::new();
    for (expected_index, document) in documents.iter().enumerate() {
        validate_identifier(
            "package target-review component",
            &document.source_component_id,
        )?;
        validate_identifier("package target-review object", &document.target_object_id)?;
        if !matches!(
            document.document_kind.as_str(),
            "prompt_preset"
                | "knowledge_book"
                | "memory_profile"
                | "transform_set"
                | "interaction_rule_set"
                | "content_module"
                | "character_content"
        ) {
            return Err(CoreError::invalid(
                "package target-review document kind is invalid",
            ));
        }
        validate_sha256(
            "package target-review component",
            &document.source_component_sha256,
        )?;
        validate_sha256("package target-review document", &document.document_sha256)?;
        if usize::try_from(document.document_index) != Ok(expected_index) {
            return Err(CoreError::invalid(
                "package target-review document indices must be contiguous",
            ));
        }
        if !component_documents.insert((
            document.source_component_id.as_str(),
            document.component_document_ordinal,
        )) {
            return Err(CoreError::invalid(
                "package target review contains a duplicate component document",
            ));
        }
        if !target_ids.insert(document.target_object_id.as_str()) {
            return Err(CoreError::invalid(
                "package target review contains a duplicate target object",
            ));
        }
        match document.disposition {
            PackageDocumentTargetDisposition::Create
                if document.expected_target_revision_id.is_none()
                    && document.expected_target_state_revision.is_none() => {}
            PackageDocumentTargetDisposition::Update
                if document
                    .expected_target_revision_id
                    .as_ref()
                    .is_some_and(|revision| !revision.trim().is_empty())
                    && document
                        .expected_target_state_revision
                        .is_some_and(|revision| revision > 0) =>
            {
                validate_identifier(
                    "package target-review revision",
                    document
                        .expected_target_revision_id
                        .as_deref()
                        .unwrap_or_default(),
                )?;
            }
            _ => {
                return Err(CoreError::invalid(
                    "package target-review disposition and expectation differ",
                ));
            }
        }
        ordinals_by_component
            .entry(&document.source_component_id)
            .or_default()
            .push(document.component_document_ordinal);
    }
    for ordinals in ordinals_by_component.values_mut() {
        ordinals.sort_unstable();
        if ordinals
            .iter()
            .enumerate()
            .any(|(expected, actual)| usize::try_from(*actual) != Ok(expected))
        {
            return Err(CoreError::invalid(
                "package target-review component ordinals must be contiguous",
            ));
        }
    }
    Ok(())
}

pub fn package_import_target_review_sha256(
    documents: &[PackageDocumentTargetReview],
) -> CoreResult<String> {
    validate_document_target_reviews(documents)?;
    let encoded = encode_json("package target review", &documents)?;
    Ok(sha256_hex(encoded.as_bytes()))
}

pub fn package_update_target_confirmations_sha256(
    confirmations: &[PackageUpdateTargetConfirmation],
) -> CoreResult<String> {
    let canonical = canonical_update_target_confirmations(confirmations)?;
    let encoded = encode_json("package update target confirmations", &canonical)?;
    Ok(sha256_hex(encoded.as_bytes()))
}

pub(super) fn insert_document_target_reviews(
    transaction: &Transaction<'_>,
    import_id: &str,
    components: &[ReviewedComponentRow],
    documents: &[PackageDocumentTargetReview],
) -> CoreResult<()> {
    validate_document_target_reviews(documents)?;
    let component_ordinals = components
        .iter()
        .map(|component| (component.source_component_key.as_str(), component.ordinal))
        .collect::<BTreeMap<_, _>>();
    for document in documents {
        let component_ordinal = component_ordinals
            .get(document.source_component_id.as_str())
            .copied()
            .ok_or_else(|| {
                CoreError::invalid("package target review names an unknown component")
            })?;
        transaction
            .execute(
                "INSERT INTO package_import_document_target_reviews (
                    import_id, component_ordinal, document_ordinal,
                    document_index, document_kind, target_object_id,
                    disposition, expected_target_revision_id,
                    expected_target_state_revision, source_component_sha256,
                    document_sha256
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    import_id,
                    i64::from(component_ordinal),
                    i64::from(document.component_document_ordinal),
                    i64::from(document.document_index),
                    document.document_kind,
                    document.target_object_id,
                    package_document_target_disposition_str(document.disposition),
                    document.expected_target_revision_id,
                    document
                        .expected_target_state_revision
                        .map(|revision| i64_from_u64("package target state revision", revision))
                        .transpose()?,
                    document.source_component_sha256,
                    document.document_sha256,
                ],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

pub(super) fn load_document_target_reviews(
    connection: &Connection,
    import_id: &str,
) -> CoreResult<Vec<PackageDocumentTargetReview>> {
    let mut statement = connection
        .prepare(
            "SELECT component.source_component_key,
                    target.document_ordinal, target.document_index,
                    target.document_kind, target.target_object_id,
                    target.disposition, target.expected_target_revision_id,
                    target.expected_target_state_revision,
                    target.source_component_sha256, target.document_sha256
             FROM package_import_document_target_reviews AS target
             JOIN package_import_components AS component
               ON component.import_id = target.import_id
              AND component.ordinal = target.component_ordinal
             WHERE target.import_id = ?1
             ORDER BY target.document_index",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map([import_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, u32>(1)?,
                row.get::<_, u32>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<i64>>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
            ))
        })
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    let documents = rows
        .into_iter()
        .map(|row| {
            Ok(PackageDocumentTargetReview {
                source_component_id: row.0,
                component_document_ordinal: row.1,
                document_index: row.2,
                document_kind: row.3,
                target_object_id: row.4,
                disposition: parse_package_document_target_disposition(&row.5)?,
                expected_target_revision_id: row.6,
                expected_target_state_revision: row
                    .7
                    .map(|revision| u64_from_i64("package target state revision", revision))
                    .transpose()?,
                source_component_sha256: row.8,
                document_sha256: row.9,
            })
        })
        .collect::<CoreResult<Vec<_>>>()?;
    validate_document_target_reviews(&documents).map_err(|error| {
        storage_corrupted(format!(
            "stored package target review is invalid: {}",
            error.message
        ))
    })?;
    Ok(documents)
}

pub(super) fn load_package_import_target_review(
    connection: &Connection,
    current: &StoredImportState,
) -> CoreResult<PackageImportTargetReview> {
    let component_rows = load_reviewed_components(connection, &current.record.id)?;
    let component_review_sha256 = reviewed_component_rows_sha256(&component_rows)?;
    let documents = load_document_target_reviews(connection, &current.record.id)?;
    let target_review_sha256 =
        package_import_target_review_sha256(&documents).map_err(|error| {
            storage_corrupted(format!(
                "stored package target review cannot be hashed: {}",
                error.message
            ))
        })?;
    let audit_rows = {
        let mut statement = connection
            .prepare(
                "SELECT payload_json, payload_sha256
                 FROM package_import_audit_events
                 WHERE import_id = ?1 AND event_kind = 'review_requested'
                 ORDER BY sequence",
            )
            .map_err(storage_db_error)?;
        statement
            .query_map([current.record.id.as_str()], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?
    };
    if audit_rows.len() != 1 || sha256_hex(audit_rows[0].0.as_bytes()) != audit_rows[0].1 {
        return Err(storage_corrupted(
            "package target review has no exact immutable selection audit",
        ));
    }
    let audit: VersionedJson = decode_json("package target-review audit", &audit_rows[0].0)?;
    if audit.schema_version != 1
        || audit
            .value
            .get("component_review_sha256")
            .and_then(Value::as_str)
            != Some(component_review_sha256.as_str())
        || audit
            .value
            .get("target_review_sha256")
            .and_then(Value::as_str)
            != Some(target_review_sha256.as_str())
    {
        return Err(storage_corrupted(
            "package target-review digest differs from its selection audit",
        ));
    }
    let target_review = PackageImportTargetReview {
        target_review_sha256,
        documents,
    };
    target_review.verify().map_err(|error| {
        storage_corrupted(format!(
            "stored package target review is invalid: {}",
            error.message
        ))
    })?;
    Ok(target_review)
}

pub(super) fn package_document_target_disposition_str(
    disposition: PackageDocumentTargetDisposition,
) -> &'static str {
    match disposition {
        PackageDocumentTargetDisposition::Create => "create",
        PackageDocumentTargetDisposition::Update => "update",
    }
}

pub(super) fn parse_package_document_target_disposition(
    value: &str,
) -> CoreResult<PackageDocumentTargetDisposition> {
    match value {
        "create" => Ok(PackageDocumentTargetDisposition::Create),
        "update" => Ok(PackageDocumentTargetDisposition::Update),
        _ => Err(storage_corrupted(
            "stored package target-review disposition is invalid",
        )),
    }
}

pub(super) fn validate_selection_target_review_replay(
    component_rows: &[ReviewedComponentRow],
    document_reviews: &[PackageDocumentTargetReview],
    selection: &SelectiveImportPlan,
    document_bindings: &[PackageDocumentCommitBinding],
) -> CoreResult<()> {
    validate_binding_snapshot_shape(document_bindings)?;
    validate_document_target_reviews(document_reviews).map_err(|error| {
        storage_corrupted(format!(
            "stored package target review is invalid: {}",
            error.message
        ))
    })?;
    if document_reviews.len() != document_bindings.len() {
        return Err(storage_corrupted(
            "stored package target-review document count differs from its selection",
        ));
    }
    for (review, binding) in document_reviews.iter().zip(document_bindings) {
        if review.document_index != binding.document_index
            || review.source_component_id != binding.source_component_key
            || review.component_document_ordinal != binding.component_document_ordinal
            || review.source_component_sha256 != binding.source_component_sha256
            || review.target_object_id != binding.target_object_id
            || review.document_kind != binding.document_kind
            || review.document_sha256 != binding.document_sha256
            || review.expected_target_state_revision != binding.expected_object_revision
        {
            return Err(CoreError::invalid(
                "package selection retry differs from its immutable target review",
            ));
        }
    }
    let selected = selection
        .components
        .iter()
        .map(|component| component.component.id.as_str())
        .collect::<BTreeSet<_>>();
    let documents_by_component = document_reviews.iter().fold(
        BTreeMap::<&str, Vec<&PackageDocumentTargetReview>>::new(),
        |mut grouped, document| {
            grouped
                .entry(document.source_component_id.as_str())
                .or_default()
                .push(document);
            grouped
        },
    );
    for row in component_rows {
        if sha256_hex(row.review_json.as_bytes()) != row.review_sha256 {
            return Err(storage_corrupted(
                "stored package component review digest does not match",
            ));
        }
        let is_selected = selected.contains(row.source_component_key.as_str());
        if row.selected != is_selected {
            return Err(storage_corrupted(
                "stored package component selection flag differs from its plan",
            ));
        }
        let documents = documents_by_component
            .get(row.source_component_key.as_str())
            .cloned()
            .unwrap_or_default();
        if !documents.is_empty() {
            let updates = documents
                .iter()
                .filter(|document| document.disposition == PackageDocumentTargetDisposition::Update)
                .count();
            let expected_disposition = if updates == 0 {
                "create"
            } else if updates == documents.len() {
                "update"
            } else {
                "conflict"
            };
            let single_update = (documents.len() == 1 && updates == 1).then(|| documents[0]);
            if row.disposition != expected_disposition
                || row.target_object_id.as_deref()
                    != single_update.map(|document| document.target_object_id.as_str())
                || row.target_revision_id.as_deref()
                    != single_update
                        .and_then(|document| document.expected_target_revision_id.as_deref())
            {
                return Err(storage_corrupted(
                    "stored package component target summary differs from document reviews",
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn validate_approval_bindings(
    connection: &Connection,
    bindings: &[PackageDocumentCommitBinding],
    components: &BTreeMap<String, ReviewedComponentRow>,
    target_review: &PackageImportTargetReview,
    confirmed_update_targets: &[PackageUpdateTargetConfirmation],
) -> CoreResult<()> {
    validate_target_review_binding_snapshot(bindings, components, target_review)?;
    validate_exact_update_target_confirmations(&target_review.documents, confirmed_update_targets)?;
    validate_current_target_review_state(connection, &target_review.documents)
}

#[allow(clippy::too_many_lines)] // Parent summaries and child rows are verified together.
pub(super) fn validate_target_review_binding_snapshot(
    bindings: &[PackageDocumentCommitBinding],
    components: &BTreeMap<String, ReviewedComponentRow>,
    target_review: &PackageImportTargetReview,
) -> CoreResult<()> {
    validate_binding_snapshot_shape(bindings)?;
    target_review.verify()?;
    if bindings.len() != target_review.documents.len() {
        return Err(CoreError::invalid(
            "package bindings differ from the immutable target-review document count",
        ));
    }
    let documents_by_component = target_review.documents.iter().fold(
        BTreeMap::<&str, Vec<&PackageDocumentTargetReview>>::new(),
        |mut grouped, document| {
            grouped
                .entry(document.source_component_id.as_str())
                .or_default()
                .push(document);
            grouped
        },
    );
    for row in components.values() {
        if sha256_hex(row.review_json.as_bytes()) != row.review_sha256 {
            return Err(storage_corrupted(
                "package component review hash does not match",
            ));
        }
        let descriptor: lorepia_orchestration::PackageComponentDescriptor =
            decode_json("package component review", &row.review_json)?;
        if !row.selected
            || descriptor.id != row.source_component_key
            || component_kind_str(descriptor.kind) != row.component_kind
        {
            return Err(storage_corrupted(
                "package selected component review identity is invalid",
            ));
        }
        let documents = documents_by_component
            .get(row.source_component_key.as_str())
            .cloned()
            .unwrap_or_default();
        if matches!(row.component_kind.as_str(), "asset" | "raw_extension") {
            if !documents.is_empty() {
                return Err(storage_corrupted(
                    "package non-document component has target-review rows",
                ));
            }
            continue;
        }
        if documents.is_empty() {
            return Err(CoreError::invalid(
                "every selected document component must review at least one target document",
            ));
        }
        let update_count = documents
            .iter()
            .filter(|document| document.disposition == PackageDocumentTargetDisposition::Update)
            .count();
        let expected_disposition = if update_count == 0 {
            "create"
        } else if update_count == documents.len() {
            "update"
        } else {
            "conflict"
        };
        let exact_single_update = (documents.len() == 1 && update_count == 1).then(|| documents[0]);
        if row.disposition != expected_disposition
            || row.target_object_id.as_deref()
                != exact_single_update.map(|document| document.target_object_id.as_str())
            || row.target_revision_id.as_deref()
                != exact_single_update
                    .and_then(|document| document.expected_target_revision_id.as_deref())
            || documents.iter().any(|document| {
                document.document_kind != row.component_kind
                    || document.source_component_sha256 != descriptor.sha256.as_str()
            })
        {
            return Err(storage_corrupted(
                "package component target summary differs from immutable document reviews",
            ));
        }
    }
    for (review, binding) in target_review.documents.iter().zip(bindings) {
        let row = components
            .get(&binding.source_component_key)
            .ok_or_else(|| CoreError::invalid("package binding names an unselected component"))?;
        if matches!(row.component_kind.as_str(), "asset" | "raw_extension")
            || !matches!(row.disposition.as_str(), "create" | "update" | "conflict")
            || review.source_component_id != binding.source_component_key
            || review.component_document_ordinal != binding.component_document_ordinal
            || review.document_index != binding.document_index
            || review.document_kind != binding.document_kind
            || review.target_object_id != binding.target_object_id
            || review.source_component_sha256 != binding.source_component_sha256
            || review.document_sha256 != binding.document_sha256
            || review.expected_target_state_revision != binding.expected_object_revision
        {
            return Err(CoreError::invalid(
                "package document binding differs from its immutable target review",
            ));
        }
    }
    if documents_by_component
        .keys()
        .any(|component_id| !components.contains_key(*component_id))
    {
        return Err(storage_corrupted(
            "package target review names an unselected component",
        ));
    }
    Ok(())
}

pub(super) fn validate_current_target_review_state(
    connection: &Connection,
    documents: &[PackageDocumentTargetReview],
) -> CoreResult<()> {
    for review in documents {
        let target = connection
            .query_row(
                "SELECT object.object_kind, object.deleted_at,
                        state.state_version, state.active_revision_id
                 FROM content_objects AS object
                 JOIN content_object_state AS state
                   ON state.object_id = object.id
                 WHERE object.id = ?1",
                [review.target_object_id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_db_error)?;
        match (review.disposition, target) {
            (PackageDocumentTargetDisposition::Create, None) => {}
            (PackageDocumentTargetDisposition::Create, Some(_)) => {
                return Err(CoreError::invalid(
                    "new package target appeared after its explicit review",
                ));
            }
            (PackageDocumentTargetDisposition::Update, None) => {
                return Err(CoreError::invalid(
                    "package update target disappeared after its explicit review",
                ));
            }
            (
                PackageDocumentTargetDisposition::Update,
                Some((kind, deleted_at, actual_revision, active_revision_id)),
            ) => {
                if kind != review.document_kind
                    || deleted_at.is_some()
                    || Some(u64_from_i64("content state revision", actual_revision)?)
                        != review.expected_target_state_revision
                    || review.expected_target_revision_id.as_deref()
                        != Some(active_revision_id.as_str())
                {
                    return Err(CoreError::invalid(
                        "package update target changed after its explicit review",
                    ));
                }
            }
        }
    }
    Ok(())
}

pub(super) fn canonical_update_target_confirmations(
    confirmations: &[PackageUpdateTargetConfirmation],
) -> CoreResult<Vec<PackageUpdateTargetConfirmation>> {
    let mut canonical = confirmations.to_vec();
    let mut identities = BTreeSet::new();
    for confirmation in &canonical {
        validate_identifier(
            "package update confirmation component",
            &confirmation.source_component_id,
        )?;
        validate_identifier(
            "package update confirmation object",
            &confirmation.target_object_id,
        )?;
        validate_identifier(
            "package update confirmation revision",
            &confirmation.expected_target_revision_id,
        )?;
        if confirmation.expected_target_state_revision == 0 {
            return Err(CoreError::invalid(
                "package update confirmation state revision must be positive",
            ));
        }
        if !identities.insert((
            confirmation.source_component_id.as_str(),
            confirmation.component_document_ordinal,
            confirmation.target_object_id.as_str(),
        )) {
            return Err(CoreError::invalid(
                "package update confirmations contain a duplicate target",
            ));
        }
    }
    canonical.sort();
    Ok(canonical)
}

pub(super) fn validate_exact_update_target_confirmations(
    documents: &[PackageDocumentTargetReview],
    confirmations: &[PackageUpdateTargetConfirmation],
) -> CoreResult<()> {
    let actual = canonical_update_target_confirmations(confirmations)?;
    let mut expected = documents
        .iter()
        .filter(|document| document.disposition == PackageDocumentTargetDisposition::Update)
        .map(|document| {
            Ok(PackageUpdateTargetConfirmation {
                source_component_id: document.source_component_id.clone(),
                component_document_ordinal: document.component_document_ordinal,
                target_object_id: document.target_object_id.clone(),
                expected_target_revision_id: document
                    .expected_target_revision_id
                    .clone()
                    .ok_or_else(|| {
                        storage_corrupted("reviewed update target has no immutable revision")
                    })?,
                expected_target_state_revision: document
                    .expected_target_state_revision
                    .ok_or_else(|| {
                        storage_corrupted("reviewed update target has no state revision")
                    })?,
            })
        })
        .collect::<CoreResult<Vec<_>>>()?;
    expected.sort();
    if actual != expected {
        return Err(CoreError::invalid(
            "package approval must explicitly confirm every and only reviewed update target",
        ));
    }
    Ok(())
}
