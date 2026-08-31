use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use lorepia_content::{
    ContentPackageSelectionPlan, PreparedContentDocument, PreparedContentDocumentEnvelope,
    PreparedContentPackageImport, StagedContentPackageAsset, discard_staged_content_package_assets,
    prepare_content_package_import, select_content_package_components,
    stage_selected_content_package_assets,
};
use lorepia_domain::{
    AssetDescriptor, AssetId, BlockSource, ContentCapability, CoreError, CoreErrorCode, CoreResult,
    ImportLimits, InstructionAuthority, PlacementZone, PromptBlockKind, Provenance, Sha256Digest,
    ValidateOrchestration,
};
use lorepia_orchestration::{RedistributionStatus, SelectiveImportPlan};
use lorepia_storage::{
    PackageCommitDocument, PackageCommitInput, PackageDocumentCommitBinding,
    PackageImportExpectation, PackageImportRecord, PackageImportStatus,
    PackageNormalizationEvidence, StagedAssetImport, built_in_prompt_presets,
    package_normalization_evidence_sha256,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    inspect::{
        OwnedContentPackageSnapshot, asset_capability, package_json_error, stale_package_review,
        with_cleanup_error, with_two_cleanup_errors,
    },
    lifecycle::{
        DurableContentPackageImport, load_durable_content_package, required_capability_approvals,
        stored_import_plan, stored_package_approval, validate_package_transition_expectations,
    },
};
use crate::Core;

/// Exact commit expectation. Commit re-inspects the private snapshot before
/// any durable content mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentPackageCommitRequest {
    pub expected_revision: u64,
    pub expected_package_plan_hash: String,
    pub expected_content_selection_plan_hash: String,
    pub expected_review_sha256: Sha256Digest,
    pub expected_import_plan_sha256: Sha256Digest,
    pub expected_approval_sha256: Sha256Digest,
    pub expected_capability_review_sha256: String,
    pub expected_normalization_evidence_sha256: String,
}
/// Durable result of one atomic selected package commit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentPackageCommitReceipt {
    pub import: PackageImportRecord,
    pub committed_document_ids: Vec<String>,
    pub asset_ids: Vec<AssetId>,
}
pub(super) struct PreparedPackageCommit {
    pub(super) content_selection: ContentPackageSelectionPlan,
    documents: Vec<PackageCommitDocument>,
    assets: Vec<AssetDescriptor>,
    pub(super) bindings: Vec<PackageDocumentCommitBinding>,
    pub(super) normalization_evidence: Vec<PackageNormalizationEvidence>,
}
impl OwnedContentPackageSnapshot {
    pub(super) fn prepare(
        &self,
        selection: &ContentPackageSelectionPlan,
        expected_package_plan_hash: &str,
        expected_selection_plan_hash: &str,
        limits: ImportLimits,
    ) -> CoreResult<PreparedContentPackageImport> {
        if selection.inspection_id.0 != self.import_id
            || selection.source_sha256 != self.inspection.source_sha256
            || selection.package_plan_hash != self.inspection.plan_hash
            || expected_package_plan_hash != self.inspection.plan_hash
            || expected_selection_plan_hash != selection.selection_plan_hash
        {
            return Err(stale_package_review());
        }
        let prepared = prepare_content_package_import(&self.path, limits, selection)?;
        if prepared.inspection.plan_hash != self.inspection.plan_hash
            || prepared.inspection.source_sha256 != self.inspection.source_sha256
            || prepared.selection != *selection
        {
            return Err(CoreError::new(
                CoreErrorCode::UnsafeArchive,
                "content package changed after approval",
                false,
            ));
        }
        Ok(prepared)
    }
}

impl Core {
    /// Re-inspects the durable source, recreates every approved binding,
    /// streams selected assets, and commits the package atomically.
    pub fn commit_content_package_import(
        &self,
        import_id: &str,
        request: &ContentPackageCommitRequest,
    ) -> CoreResult<ContentPackageCommitReceipt> {
        let loaded = load_durable_content_package(self, import_id, ImportLimits::default())?;
        if !matches!(
            loaded.record.status,
            PackageImportStatus::Approved | PackageImportStatus::Completed
        ) {
            return Err(CoreError::invalid(
                "content package must be approved before commit",
            ));
        }
        let import_plan = stored_import_plan(&loaded.record)?;
        validate_package_transition_expectations(
            &loaded,
            &import_plan,
            &request.expected_package_plan_hash,
            &request.expected_content_selection_plan_hash,
            &request.expected_review_sha256,
            &request.expected_import_plan_sha256,
        )?;
        let (approval, approved_at) = stored_package_approval(self, import_id)?;
        if approval.plan.approval_sha256 != request.expected_approval_sha256
            || approval.plan.plan_sha256 != request.expected_import_plan_sha256
            || approval.plan.review_sha256 != request.expected_review_sha256
            || approval.approved_capabilities != required_capability_approvals(&import_plan)
            || approval.normalization_evidence_sha256
                != request.expected_normalization_evidence_sha256
            || approval.normalization_evidence_sha256
                != package_normalization_evidence_sha256(&approval.normalization_evidence)?
        {
            return Err(stale_package_review());
        }
        let replay_bindings = (loaded.record.status == PackageImportStatus::Completed)
            .then_some(approval.document_bindings.as_slice());
        let prepared = prepare_package_commit(
            self,
            &loaded,
            &import_plan,
            ImportLimits::default(),
            replay_bindings,
        )?;
        if approval.document_bindings != prepared.bindings
            || approval.plan.assets != prepared.assets
            || approval.normalization_evidence != prepared.normalization_evidence
            || package_normalization_evidence_sha256(&prepared.normalization_evidence)?
                != request.expected_normalization_evidence_sha256
        {
            return Err(stale_package_review());
        }
        let expected = PackageImportExpectation {
            revision: request.expected_revision,
            inspection_sha256: request.expected_review_sha256.as_str().to_owned(),
            selection_sha256: request.expected_import_plan_sha256.as_str().to_owned(),
            capability_review_sha256: request.expected_capability_review_sha256.clone(),
        };
        persist_prepared_package_commit(
            self,
            import_id,
            loaded,
            prepared,
            expected,
            approved_at,
            request.expected_revision,
        )
    }
}

fn persist_prepared_package_commit(
    core: &Core,
    import_id: &str,
    loaded: DurableContentPackageImport,
    prepared: PreparedPackageCommit,
    expected: PackageImportExpectation,
    approved_at: DateTime<Utc>,
    expected_revision: u64,
) -> CoreResult<ContentPackageCommitReceipt> {
    let mut approved_import = loaded.record.clone();
    approved_import.status = PackageImportStatus::Approved;
    approved_import.revision = expected_revision;
    approved_import.updated_at = approved_at;
    let input = PackageCommitInput {
        source: loaded.source,
        import: approved_import,
        documents: prepared.documents,
        assets: prepared.assets.clone(),
    };
    let staged_assets = if loaded.record.status == PackageImportStatus::Completed {
        Vec::new()
    } else {
        stage_selected_content_package_assets(
            &loaded.owned.path,
            ImportLimits::default(),
            &prepared.content_selection,
            &core.storage().staging_dir(),
        )?
    };
    let staged_imports = staged_assets
        .iter()
        .map(staged_asset_import)
        .collect::<Vec<_>>();
    let promotion_result = core
        .storage()
        .promote_package_assets(import_id, &staged_imports);
    let staging_cleanup =
        discard_staged_content_package_assets(&staged_assets, &core.storage().staging_dir());
    if let Err(error) = promotion_result {
        let cas_cleanup = core
            .storage()
            .discard_unclaimed_package_assets(import_id, &staged_imports);
        return Err(with_two_cleanup_errors(
            error,
            staging_cleanup,
            cas_cleanup.map(|_| ()),
        ));
    }
    if let Err(error) = staging_cleanup {
        let cas_cleanup = core
            .storage()
            .discard_unclaimed_package_assets(import_id, &staged_imports);
        return Err(with_cleanup_error(error, cas_cleanup.map(|_| ())));
    }
    let committed = core
        .storage()
        .commit_package_import(&input, &expected, &prepared.bindings)
        .map_err(|error| {
            let cleanup = core
                .storage()
                .discard_unclaimed_package_assets(import_id, &staged_imports);
            with_cleanup_error(error, cleanup.map(|_| ()))
        })?;
    Ok(ContentPackageCommitReceipt {
        import: committed,
        committed_document_ids: prepared
            .bindings
            .iter()
            .map(|binding| binding.target_object_id.clone())
            .collect(),
        asset_ids: prepared.assets.into_iter().map(|asset| asset.id).collect(),
    })
}
pub(super) fn prepare_package_commit(
    core: &Core,
    loaded: &DurableContentPackageImport,
    import_plan: &SelectiveImportPlan,
    limits: ImportLimits,
    replay_bindings: Option<&[PackageDocumentCommitBinding]>,
) -> CoreResult<PreparedPackageCommit> {
    let content_selection = select_content_package_components(
        loaded.owned.inspection(),
        &loaded.record.selected_component_ids,
    )?;
    let prepared = loaded.owned.prepare(
        &content_selection,
        &loaded.owned.inspection.plan_hash,
        &content_selection.selection_plan_hash,
        limits,
    )?;
    let mut normalization_evidence = prepared
        .transformations
        .iter()
        .map(|transformation| PackageNormalizationEvidence {
            component_id: transformation.component_id.clone(),
            object_id: transformation.object_id.clone(),
            field: transformation.field.clone(),
            before: transformation.before,
            after: transformation.after,
            reason: transformation.reason.clone(),
        })
        .collect::<Vec<_>>();
    normalization_evidence.sort();
    package_normalization_evidence_sha256(&normalization_evidence)?;
    let mut assets = prepared.assets;
    assets.sort_by(|left, right| left.id.cmp(&right.id));
    if assets != import_plan.assets {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "content and orchestration package asset inventories disagree",
            false,
        ));
    }
    let imported_provenance = loaded.owned.review.manifest.provenance.clone();
    let mut prepared_documents = prepared.documents;
    prepared_documents.sort_by(|left, right| {
        matches!(&left.document, PreparedContentDocument::ContentModule(_))
            .cmp(&matches!(
                &right.document,
                PreparedContentDocument::ContentModule(_)
            ))
            .then_with(|| {
                left.source_component_ordinal
                    .cmp(&right.source_component_ordinal)
            })
            .then_with(|| left.document_ordinal.cmp(&right.document_ordinal))
            .then_with(|| left.source_component_id.cmp(&right.source_component_id))
    });
    let redistribution_allowed = import_plan.redistribution_status == RedistributionStatus::Allowed;
    let (documents, bindings) = prepare_package_commit_documents(
        core,
        loaded,
        prepared_documents,
        &imported_provenance,
        redistribution_allowed,
        replay_bindings,
    )?;
    validate_content_module_package_bindings(&documents, &assets, import_plan)?;
    Ok(PreparedPackageCommit {
        content_selection,
        documents,
        assets,
        bindings,
        normalization_evidence,
    })
}
fn prepare_package_commit_documents(
    core: &Core,
    loaded: &DurableContentPackageImport,
    prepared_documents: Vec<PreparedContentDocumentEnvelope>,
    imported_provenance: &Provenance,
    redistribution_allowed: bool,
    replay_bindings: Option<&[PackageDocumentCommitBinding]>,
) -> CoreResult<(
    Vec<PackageCommitDocument>,
    Vec<PackageDocumentCommitBinding>,
)> {
    let mut documents = Vec::with_capacity(prepared_documents.len());
    let mut bindings = Vec::with_capacity(prepared_documents.len());
    for (index, envelope) in prepared_documents.into_iter().enumerate() {
        let source_component = loaded
            .owned
            .review
            .components
            .iter()
            .find(|component| component.id == envelope.source_component_id)
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "prepared package document has no reviewed source component",
                    false,
                )
            })?;
        let document = normalize_prepared_document(
            envelope.document,
            imported_provenance,
            redistribution_allowed,
        )?;
        if let PackageCommitDocument::PromptPreset(preset) = &document {
            core.validate_prompt_preset(preset)?;
        }
        let (document_kind, target_object_id) = package_document_identity(&document);
        if envelope.document_kind != document_kind || envelope.document_id != target_object_id {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "normalized package document changed its reviewed identity",
                false,
            ));
        }
        let document_json = serde_json::to_vec(&document).map_err(package_json_error)?;
        let document_sha256 = format!("{:x}", Sha256::digest(&document_json));
        let document_index = u32::try_from(index)
            .map_err(|_| CoreError::invalid("package contains too many documents"))?;
        let expected_object_revision = if let Some(replay_bindings) = replay_bindings {
            let approved = replay_bindings
                .iter()
                .find(|binding| binding.document_index == document_index)
                .ok_or_else(stale_package_review)?;
            if approved.source_component_key != envelope.source_component_id
                || approved.component_document_ordinal != envelope.document_ordinal
                || approved.source_component_sha256 != source_component.sha256.as_str()
                || approved.target_object_id != target_object_id
                || approved.document_kind != document_kind
                || approved.document_sha256 != document_sha256
            {
                return Err(stale_package_review());
            }
            approved.expected_object_revision
        } else {
            expected_document_revision(core, &document)?
        };
        bindings.push(PackageDocumentCommitBinding {
            document_index,
            source_component_key: envelope.source_component_id,
            component_document_ordinal: envelope.document_ordinal,
            source_component_sha256: source_component.sha256.as_str().to_owned(),
            target_object_id: target_object_id.to_owned(),
            document_kind: document_kind.to_owned(),
            document_sha256,
            expected_object_revision,
        });
        documents.push(document);
    }
    Ok((documents, bindings))
}
pub(super) fn normalize_prepared_document(
    document: PreparedContentDocument,
    imported_provenance: &Provenance,
    redistribution_allowed: bool,
) -> CoreResult<PackageCommitDocument> {
    match document {
        PreparedContentDocument::PromptPreset(preset) => {
            let mut preset = *preset;
            if built_in_prompt_presets()
                .iter()
                .any(|built_in| built_in.id == preset.id)
            {
                return Err(CoreError::invalid(
                    "imported packages cannot replace built-in prompt presets",
                ));
            }
            preset.metadata.provenance = imported_provenance.clone();
            for block in &mut preset.blocks {
                block.provenance = imported_provenance.clone();
                block.authority = InstructionAuthority::ImportedContent;
            }
            crate::orchestration::enforce_application_policy(&mut preset);
            Ok(PackageCommitDocument::PromptPreset(preset))
        }
        PreparedContentDocument::KnowledgeBook(book) => {
            let mut book = *book;
            book.provenance = imported_provenance.clone();
            for entry in &mut book.entries {
                entry.provenance = imported_provenance.clone();
            }
            book.validate().map_err(|error| {
                CoreError::invalid(format!("invalid imported knowledge book: {error}"))
            })?;
            Ok(PackageCommitDocument::KnowledgeBook(book))
        }
        PreparedContentDocument::MemoryProfile(profile) => {
            let mut profile = *profile;
            profile.provenance = imported_provenance.clone();
            profile.validate().map_err(|error| {
                CoreError::invalid(format!("invalid imported memory profile: {error}"))
            })?;
            Ok(PackageCommitDocument::MemoryProfile(profile))
        }
        PreparedContentDocument::TransformSet(set) => {
            let mut set = *set;
            set.provenance = imported_provenance.clone();
            set.enabled = false;
            for rule in &mut set.rules {
                rule.provenance = imported_provenance.clone();
                rule.enabled = false;
                rule.imported_enabled = false;
            }
            Ok(PackageCommitDocument::TransformSet(set))
        }
        PreparedContentDocument::InteractionRuleSet(set) => {
            let mut set = *set;
            set.provenance = imported_provenance.clone();
            for rule in &mut set.rules {
                rule.provenance = imported_provenance.clone();
                rule.enabled = false;
            }
            Ok(PackageCommitDocument::InteractionRuleSet(set))
        }
        PreparedContentDocument::ContentModule(module) => {
            let mut module = *module;
            if module.schema_version != 1 {
                return Err(CoreError::invalid(
                    "imported content module schema_version must be 1",
                ));
            }
            module.metadata.provenance = imported_provenance.clone();
            module
                .metadata
                .author
                .clone_from(&imported_provenance.author);
            module.metadata.license = imported_provenance
                .license
                .clone()
                .unwrap_or_else(|| "UNKNOWN".to_owned());
            module.metadata.redistribution_allowed = redistribution_allowed;
            for (index, block) in module.prompt_fragments.iter_mut().enumerate() {
                if block.kind == PromptBlockKind::LatestUserTurn
                    || block.source == BlockSource::LatestUser
                    || matches!(
                        block.placement_zone,
                        PlacementZone::ApplicationPolicy | PlacementZone::LatestUser
                    )
                {
                    return Err(CoreError::invalid(format!(
                        "imported content module prompt_fragments[{index}] uses a reserved application or latest-user block",
                    )));
                }
                block.authority = InstructionAuthority::ImportedContent;
                block.provenance = imported_provenance.clone();
            }
            module.validate().map_err(|error| {
                CoreError::invalid(format!("invalid imported content module: {error}"))
            })?;
            Ok(PackageCommitDocument::ContentModule(module))
        }
    }
}
fn validate_content_module_package_bindings(
    documents: &[PackageCommitDocument],
    assets: &[AssetDescriptor],
    import_plan: &SelectiveImportPlan,
) -> CoreResult<()> {
    let knowledge_ids = documents
        .iter()
        .filter_map(|document| match document {
            PackageCommitDocument::KnowledgeBook(value) => Some(value.id.as_str()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let transform_ids = documents
        .iter()
        .filter_map(|document| match document {
            PackageCommitDocument::TransformSet(value) => Some(value.id.as_str()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let interaction_ids = documents
        .iter()
        .filter_map(|document| match document {
            PackageCommitDocument::InteractionRuleSet(value) => Some(value.id.as_str()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let assets_by_id = assets
        .iter()
        .map(|asset| (&asset.id, asset))
        .collect::<BTreeMap<_, _>>();
    let approved_capabilities = import_plan
        .required_capabilities
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();

    for module in documents.iter().filter_map(|document| match document {
        PackageCommitDocument::ContentModule(value) => Some(value),
        _ => None,
    }) {
        validate_content_module_package_binding(
            module,
            &knowledge_ids,
            &transform_ids,
            &interaction_ids,
            &assets_by_id,
            &approved_capabilities,
        )?;
    }
    Ok(())
}
fn validate_content_module_package_binding(
    module: &lorepia_domain::ContentModule,
    knowledge_ids: &BTreeSet<&str>,
    transform_ids: &BTreeSet<&str>,
    interaction_ids: &BTreeSet<&str>,
    assets_by_id: &BTreeMap<&AssetId, &AssetDescriptor>,
    approved_capabilities: &BTreeSet<ContentCapability>,
) -> CoreResult<()> {
    module
        .validate()
        .map_err(|error| CoreError::invalid(format!("invalid imported content module: {error}")))?;
    let missing_link = module
        .knowledge_book_ids
        .iter()
        .map(|id| {
            (
                "knowledge book",
                id.as_str(),
                knowledge_ids.contains(id.as_str()),
            )
        })
        .chain(module.transform_set_ids.iter().map(|id| {
            (
                "transform set",
                id.as_str(),
                transform_ids.contains(id.as_str()),
            )
        }))
        .chain(module.interaction_rule_set_ids.iter().map(|id| {
            (
                "interaction rule set",
                id.as_str(),
                interaction_ids.contains(id.as_str()),
            )
        }))
        .find(|(_, _, present)| !present);
    if let Some((kind, id, _)) = missing_link {
        return Err(CoreError::invalid(format!(
            "content module {} references a {kind} outside the approved package selection: {id}",
            module.id.as_str()
        )));
    }

    let declared = module
        .required_capabilities
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let mut required = BTreeSet::new();
    required.extend(
        (!module.prompt_fragments.is_empty()).then_some(ContentCapability::PromptFragments),
    );
    required
        .extend((!module.knowledge_book_ids.is_empty()).then_some(ContentCapability::Knowledge));
    required.extend((!module.control_specs.is_empty()).then_some(ContentCapability::Variables));
    required
        .extend((!module.transform_set_ids.is_empty()).then_some(ContentCapability::Transforms));
    required.extend(
        (!module.interaction_rule_set_ids.is_empty())
            .then_some(ContentCapability::DeclarativeInteractions),
    );
    for asset_id in &module.asset_ids {
        let asset = assets_by_id.get(asset_id).ok_or_else(|| {
            CoreError::invalid(format!(
                "content module {} references an asset outside the approved package selection: {}",
                module.id.as_str(),
                asset_id.as_str()
            ))
        })?;
        required.insert(asset_capability(&asset.media_type));
    }
    if let Some(missing) = required
        .iter()
        .find(|capability| !declared.contains(capability))
    {
        return Err(CoreError::invalid(format!(
            "content module {} omits required capability {missing:?}",
            module.id.as_str()
        )));
    }
    if let Some(unapproved) = declared
        .iter()
        .find(|capability| !approved_capabilities.contains(capability))
    {
        return Err(CoreError::invalid(format!(
            "content module {} capability was not part of the approved import plan: {unapproved:?}",
            module.id.as_str()
        )));
    }
    Ok(())
}
fn package_document_identity(document: &PackageCommitDocument) -> (&'static str, &str) {
    match document {
        PackageCommitDocument::PromptPreset(value) => ("prompt_preset", value.id.as_str()),
        PackageCommitDocument::KnowledgeBook(value) => ("knowledge_book", value.id.as_str()),
        PackageCommitDocument::MemoryProfile(value) => ("memory_profile", value.id.as_str()),
        PackageCommitDocument::TransformSet(value) => ("transform_set", value.id.as_str()),
        PackageCommitDocument::InteractionRuleSet(value) => {
            ("interaction_rule_set", value.id.as_str())
        }
        PackageCommitDocument::ContentModule(value) => ("content_module", value.id.as_str()),
        PackageCommitDocument::CharacterContent { character_id, .. } => {
            ("character_content", character_id)
        }
    }
}
fn expected_document_revision(
    core: &Core,
    document: &PackageCommitDocument,
) -> CoreResult<Option<u64>> {
    let result = match document {
        PackageCommitDocument::PromptPreset(value) => core
            .storage()
            .get_prompt_preset(&value.id)
            .map(|value| value.revision),
        PackageCommitDocument::KnowledgeBook(value) => core
            .storage()
            .get_knowledge_book(&value.id)
            .map(|value| value.revision),
        PackageCommitDocument::MemoryProfile(value) => core
            .storage()
            .get_memory_profile(&value.id)
            .map(|value| value.revision),
        PackageCommitDocument::TransformSet(value) => core
            .storage()
            .get_transform_set(&value.id)
            .map(|value| value.revision),
        PackageCommitDocument::InteractionRuleSet(value) => core
            .storage()
            .get_interaction_rule_set(&value.id)
            .map(|value| value.revision),
        PackageCommitDocument::ContentModule(value) => core
            .storage()
            .get_content_module(&value.id)
            .map(|value| value.revision),
        PackageCommitDocument::CharacterContent { .. } => {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "unsupported document kind reached a content package commit",
                false,
            ));
        }
    };
    match result {
        Ok(revision) => Ok(Some(revision)),
        Err(error) if error.code == CoreErrorCode::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}
fn staged_asset_import(asset: &StagedContentPackageAsset) -> StagedAssetImport {
    StagedAssetImport {
        staged_path: asset.staged_path.clone(),
        sha256: asset.descriptor.sha256.as_str().to_owned(),
        media_type: asset.descriptor.media_type.clone(),
        size_bytes: asset.descriptor.size_bytes,
    }
}
