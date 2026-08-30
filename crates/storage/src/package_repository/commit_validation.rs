//! Package commit validation, binding, audit, and exact replay.

use super::{
    BTreeMap, BTreeSet, CompletedAuthorityCommitEvidence, Connection, CoreError, CoreResult,
    InstructionAuthority, OptionalExtension, PackageApprovalPayload, PackageCommitDocument,
    PackageCommitInput, PackageDocumentCommitBinding, PackageImportExpectation,
    PackageImportStatus, PackageImportTargetReview, PackageUpdateTargetConfirmation, PlacementZone,
    ReviewedComponentRow, SourceKind, StoredImportState, ValidateOrchestration, Value,
    VersionedJson, decode_json, encode_json, i64_from_u64, params, read_approval_payload,
    read_package_source_by_id, revision_conflict, sha256_hex, storage_corrupted, storage_db_error,
    validate_approval_bindings, validate_document_normalization_evidence, validate_identifier,
    validate_sha256, validate_source_record,
};

pub(super) fn validate_commit_input_shape(
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

pub(super) fn validate_normalized_package_documents(
    documents: &[PackageCommitDocument],
) -> CoreResult<()> {
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

pub(super) fn validate_binding_snapshot_shape(
    bindings: &[PackageDocumentCommitBinding],
) -> CoreResult<()> {
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

pub(super) fn load_selected_commit_components(
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

pub(super) fn validate_completed_authority_audit(
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
pub(super) fn validate_completed_commit_replay(
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

pub(super) fn validate_commit_bindings(
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

pub(super) fn document_object_id(document: &PackageCommitDocument) -> String {
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
