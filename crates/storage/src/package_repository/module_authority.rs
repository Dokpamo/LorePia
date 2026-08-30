//! Completed package-backed module authority verification and projection.

use super::{
    ActiveContentModuleRevision, AssetDescriptor, AssetId, CompletedPackageAuthority,
    CompletedPackageComponentAuthority, CompletedPackageDocumentAuthority, Connection, ControlId,
    CoreError, CoreErrorCode, CoreResult, ModuleComponentRef, ModuleImportApprovalEvidence,
    ModuleImportComponentAuthority, OptionalExtension, PackageComponentKind, PackageImportStatus,
    PromptBlockId, Sha256Digest, SourceKind, decode_json, encode_json, not_found, params,
    sha256_hex, storage_corrupted, storage_db_error, validate_identifier,
};

pub(super) fn validate_completed_module_authority_target(
    stored: &ActiveContentModuleRevision,
) -> CoreResult<()> {
    validate_identifier("content module", stored.object.value.id.as_str())?;
    validate_identifier(
        "content module revision",
        stored.module_revision.id.as_str(),
    )?;
    let module_document_json =
        encode_json("imported module authority document", &stored.object.value)?;
    let provenance = &stored.object.value.metadata.provenance;
    if sha256_hex(module_document_json.as_bytes()) != stored.object.sha256
        || stored.object.revision_id != stored.module_revision.id.as_str()
        || stored.object.object_id != stored.object.value.id.as_str()
        || stored.object.value.id != stored.module_revision.module_id
        || provenance.source_kind != SourceKind::ImportedPackage
        || provenance.source_hash.as_deref() != Some(stored.module_revision.source_hash.as_str())
        || provenance.source_id.as_deref().is_none_or(str::is_empty)
    {
        return Err(storage_corrupted(
            "imported module authority target differs from its immutable revision",
        ));
    }
    Ok(())
}

enum ModuleAuthorityComponent {
    Embedded,
    Linked {
        target_object_id: String,
        target_revision_id: String,
    },
    Asset(AssetDescriptor),
}

fn read_module_authority_component(
    connection: &Connection,
    stored: &ActiveContentModuleRevision,
    component: &lorepia_domain::ComponentHash,
) -> CoreResult<ModuleAuthorityComponent> {
    let revision_id = stored.module_revision.id.as_str();
    match &component.component {
        ModuleComponentRef::PromptBlock { id } => {
            read_prompt_block_module_authority_component(connection, stored, component, id)
        }
        ModuleComponentRef::Control { id } => {
            read_control_module_authority_component(connection, stored, component, id)
        }
        ModuleComponentRef::KnowledgeBook { id } => {
            if !stored.object.value.knowledge_book_ids.contains(id) {
                return Err(storage_corrupted(
                    "module knowledge projection is absent from its immutable document",
                ));
            }
            read_linked_module_authority_component(
                connection,
                revision_id,
                "knowledge_book",
                "knowledge_book_revision_id",
                id.as_str(),
                component,
            )
        }
        ModuleComponentRef::TransformSet { id } => {
            if !stored.object.value.transform_set_ids.contains(id) {
                return Err(storage_corrupted(
                    "module transform projection is absent from its immutable document",
                ));
            }
            read_linked_module_authority_component(
                connection,
                revision_id,
                "transform_set",
                "transform_set_revision_id",
                id.as_str(),
                component,
            )
        }
        ModuleComponentRef::InteractionRuleSet { id } => {
            if !stored.object.value.interaction_rule_set_ids.contains(id) {
                return Err(storage_corrupted(
                    "module interaction projection is absent from its immutable document",
                ));
            }
            read_linked_module_authority_component(
                connection,
                revision_id,
                "interaction_rule_set",
                "interaction_rule_set_revision_id",
                id.as_str(),
                component,
            )
        }
        ModuleComponentRef::Asset { id } => {
            read_asset_module_authority_component(connection, stored, component, id)
        }
    }
}

fn read_prompt_block_module_authority_component(
    connection: &Connection,
    stored: &ActiveContentModuleRevision,
    component: &lorepia_domain::ComponentHash,
    id: &PromptBlockId,
) -> CoreResult<ModuleAuthorityComponent> {
    let expected = stored
        .object
        .value
        .prompt_fragments
        .iter()
        .find(|block| block.id == *id)
        .ok_or_else(|| {
            storage_corrupted(
                "module prompt-block projection is absent from its immutable document",
            )
        })?;
    let expected_json = encode_json("module prompt block authority", expected)?;
    let row = connection
        .query_row(
            "SELECT component.component_sha256, block.document_json
             FROM content_module_components AS component
             JOIN content_module_prompt_blocks AS block
               ON block.module_revision_id = component.module_revision_id
              AND block.block_id = component.prompt_block_id
             WHERE component.module_revision_id = ?1
               AND component.component_kind = 'prompt_block'
               AND component.prompt_block_id = ?2",
            params![stored.module_revision.id.as_str(), id.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("module prompt-block authority"))?;
    validate_embedded_module_authority(component, &expected_json, &row)?;
    Ok(ModuleAuthorityComponent::Embedded)
}

fn read_control_module_authority_component(
    connection: &Connection,
    stored: &ActiveContentModuleRevision,
    component: &lorepia_domain::ComponentHash,
    id: &ControlId,
) -> CoreResult<ModuleAuthorityComponent> {
    let expected = stored
        .object
        .value
        .control_specs
        .iter()
        .find(|control| control.id == *id)
        .ok_or_else(|| {
            storage_corrupted("module control projection is absent from its immutable document")
        })?;
    let expected_json = encode_json("module control authority", expected)?;
    let row = connection
        .query_row(
            "SELECT component.component_sha256, control.document_json
             FROM content_module_components AS component
             JOIN content_module_controls AS control
               ON control.module_revision_id = component.module_revision_id
              AND control.control_id = component.control_id
             WHERE component.module_revision_id = ?1
               AND component.component_kind = 'control'
               AND component.control_id = ?2",
            params![stored.module_revision.id.as_str(), id.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("module control authority"))?;
    validate_embedded_module_authority(component, &expected_json, &row)?;
    Ok(ModuleAuthorityComponent::Embedded)
}

fn read_asset_module_authority_component(
    connection: &Connection,
    stored: &ActiveContentModuleRevision,
    component: &lorepia_domain::ComponentHash,
    id: &AssetId,
) -> CoreResult<ModuleAuthorityComponent> {
    if !stored.object.value.asset_ids.contains(id) {
        return Err(storage_corrupted(
            "module asset projection is absent from its immutable document",
        ));
    }
    let row = connection
        .query_row(
            "SELECT component.component_sha256, descriptor.payload_json
             FROM content_module_components AS component
             JOIN asset_descriptors AS descriptor
               ON descriptor.id = component.asset_descriptor_id
             WHERE component.module_revision_id = ?1
               AND component.component_kind = 'asset'
               AND component.asset_descriptor_id = ?2",
            params![stored.module_revision.id.as_str(), id.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("module asset authority"))?;
    if row.0 != component.sha256.as_str() || sha256_hex(row.1.as_bytes()) != row.0 {
        return Err(storage_corrupted(
            "module asset authority hash differs from its immutable projection",
        ));
    }
    let descriptor: AssetDescriptor = decode_json("module asset authority descriptor", &row.1)?;
    if descriptor.id != *id {
        return Err(storage_corrupted(
            "module asset authority descriptor has a different identity",
        ));
    }
    Ok(ModuleAuthorityComponent::Asset(descriptor))
}

fn validate_embedded_module_authority(
    component: &lorepia_domain::ComponentHash,
    expected_json: &str,
    stored: &(String, String),
) -> CoreResult<()> {
    if stored.0 != component.sha256.as_str()
        || sha256_hex(stored.1.as_bytes()) != stored.0
        || stored.1 != expected_json
    {
        return Err(storage_corrupted(
            "embedded module authority differs from its immutable document",
        ));
    }
    Ok(())
}

fn read_linked_module_authority_component(
    connection: &Connection,
    module_revision_id: &str,
    object_kind: &'static str,
    revision_column: &'static str,
    object_id: &str,
    component: &lorepia_domain::ComponentHash,
) -> CoreResult<ModuleAuthorityComponent> {
    let query = match revision_column {
        "knowledge_book_revision_id" => {
            "SELECT component.component_sha256, content.object_id, content.id,
                    content.document_json, content.document_sha256
             FROM content_module_components AS component
             JOIN content_revisions AS content
               ON content.id = component.knowledge_book_revision_id
             WHERE component.module_revision_id = ?1
               AND component.component_kind = 'knowledge_book'
               AND content.object_kind = ?2 AND content.object_id = ?3"
        }
        "transform_set_revision_id" => {
            "SELECT component.component_sha256, content.object_id, content.id,
                    content.document_json, content.document_sha256
             FROM content_module_components AS component
             JOIN content_revisions AS content
               ON content.id = component.transform_set_revision_id
             WHERE component.module_revision_id = ?1
               AND component.component_kind = 'transform_set'
               AND content.object_kind = ?2 AND content.object_id = ?3"
        }
        "interaction_rule_set_revision_id" => {
            "SELECT component.component_sha256, content.object_id, content.id,
                    content.document_json, content.document_sha256
             FROM content_module_components AS component
             JOIN content_revisions AS content
               ON content.id = component.interaction_rule_set_revision_id
             WHERE component.module_revision_id = ?1
               AND component.component_kind = 'interaction_rule_set'
               AND content.object_kind = ?2 AND content.object_id = ?3"
        }
        _ => {
            return Err(CoreError::internal(
                "module authority linked revision column is unsupported",
            ));
        }
    };
    let row = connection
        .query_row(
            query,
            params![module_revision_id, object_kind, object_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("linked module component authority"))?;
    if row.0 != component.sha256.as_str()
        || row.4 != row.0
        || sha256_hex(row.3.as_bytes()) != row.4
        || row.1 != object_id
    {
        return Err(storage_corrupted(
            "linked module authority differs from its immutable revision",
        ));
    }
    Ok(ModuleAuthorityComponent::Linked {
        target_object_id: row.1,
        target_revision_id: row.2,
    })
}

#[allow(clippy::too_many_lines)] // One conversion covers every typed module component authority.
pub(super) fn build_module_import_approval_evidence_in_connection(
    connection: &Connection,
    stored: &ActiveContentModuleRevision,
    authority: &CompletedPackageAuthority,
) -> CoreResult<ModuleImportApprovalEvidence> {
    validate_completed_module_authority_target(stored)?;
    let source_sha256 = parse_authority_sha256("package source", &authority.source_sha256)?;
    let provenance = &stored.object.value.metadata.provenance;
    if authority.status != PackageImportStatus::Completed
        || authority.import_revision == 0
        || provenance.source_kind != SourceKind::ImportedPackage
        || provenance.source_id.as_deref() != Some(authority.package_id.as_str())
        || provenance.source_hash.as_deref() != Some(authority.source_sha256.as_str())
        || stored.module_revision.source_hash != source_sha256
    {
        return Err(package_authority_denied(
            "completed package authority does not own the imported module source",
        ));
    }
    // `document_sha256` authenticates the tagged `PackageCommitDocument`
    // approval payload and is revalidated while loading `authority`; it is not
    // the hash of the inner content revision. The immutable inner module is
    // authenticated by `validate_completed_module_authority_target`, while the
    // exact commit link is the object/revision/component tuple below.
    let module_matches = authority
        .enabled_components
        .iter()
        .filter(|component| component.kind == PackageComponentKind::ContentModule)
        .flat_map(|component| {
            component
                .committed_documents
                .iter()
                .filter(|document| {
                    document.target_object_id == stored.object.value.id.as_str()
                        && document.target_revision_id == stored.module_revision.id.as_str()
                        && document.source_component_sha256 == component.sha256
                })
                .map(move |document| (component, document))
        })
        .collect::<Vec<_>>();
    let [(module_component, module_document)] = module_matches.as_slice() else {
        return Err(package_authority_denied(
            "completed package authority does not select the exact module revision",
        ));
    };

    let mut selected_package_component_ids = authority
        .enabled_components
        .iter()
        .map(|component| component.component_id.clone())
        .collect::<Vec<_>>();
    selected_package_component_ids.sort();
    if selected_package_component_ids
        .windows(2)
        .any(|pair| pair[0] == pair[1])
    {
        return Err(storage_corrupted(
            "completed package authority contains duplicate enabled components",
        ));
    }
    let mut authorized_capabilities = authority.required_capabilities.clone();
    authorized_capabilities.sort();
    authorized_capabilities.dedup();

    let mut component_authorities =
        Vec::with_capacity(stored.module_revision.component_hashes.len());
    for component in &stored.module_revision.component_hashes {
        let material = read_module_authority_component(connection, stored, component)?;
        let component_authority = match (&component.component, material) {
            (
                ModuleComponentRef::PromptBlock { .. } | ModuleComponentRef::Control { .. },
                ModuleAuthorityComponent::Embedded,
            ) => module_document_component_authority(component, module_component, module_document)?,
            (
                ModuleComponentRef::KnowledgeBook { .. },
                ModuleAuthorityComponent::Linked {
                    target_object_id,
                    target_revision_id,
                },
            ) => document_module_component_authority(
                component,
                PackageComponentKind::KnowledgeBook,
                &target_object_id,
                &target_revision_id,
                authority,
            )?,
            (
                ModuleComponentRef::TransformSet { .. },
                ModuleAuthorityComponent::Linked {
                    target_object_id,
                    target_revision_id,
                },
            ) => document_module_component_authority(
                component,
                PackageComponentKind::TransformSet,
                &target_object_id,
                &target_revision_id,
                authority,
            )?,
            (
                ModuleComponentRef::InteractionRuleSet { .. },
                ModuleAuthorityComponent::Linked {
                    target_object_id,
                    target_revision_id,
                },
            ) => document_module_component_authority(
                component,
                PackageComponentKind::InteractionRuleSet,
                &target_object_id,
                &target_revision_id,
                authority,
            )?,
            (ModuleComponentRef::Asset { id }, ModuleAuthorityComponent::Asset(descriptor)) => {
                asset_module_component_authority(
                    component,
                    id,
                    &descriptor,
                    module_component,
                    authority,
                )?
            }
            _ => {
                return Err(storage_corrupted(
                    "module component material kind differs from its immutable reference",
                ));
            }
        };
        component_authorities.push(component_authority);
    }
    component_authorities.sort();

    Ok(ModuleImportApprovalEvidence {
        approval_id: authority.approval_id.clone(),
        approval_sha256: parse_authority_sha256("package approval", &authority.approval_sha256)?,
        import_id: authority.import_id.clone(),
        import_revision: authority.import_revision,
        package_id: authority.package_id.clone(),
        package_source_sha256: source_sha256,
        selection_sha256: parse_authority_sha256("package selection", &authority.selection_sha256)?,
        capability_review_sha256: parse_authority_sha256(
            "package capability review",
            &authority.capability_review_sha256,
        )?,
        module_id: stored.object.value.id.clone(),
        module_revision_id: stored.module_revision.id.clone(),
        module_revision_source_sha256: stored.module_revision.source_hash.clone(),
        module_package_component_id: module_component.component_id.clone(),
        module_package_component_sha256: parse_authority_sha256(
            "package module component",
            &module_component.sha256,
        )?,
        module_commit_result_sha256: parse_authority_sha256(
            "package module commit",
            &module_document.result_sha256,
        )?,
        selected_package_component_ids,
        authorized_capabilities,
        component_authorities,
    })
}

fn module_document_component_authority(
    component: &lorepia_domain::ComponentHash,
    module_component: &CompletedPackageComponentAuthority,
    module_document: &CompletedPackageDocumentAuthority,
) -> CoreResult<ModuleImportComponentAuthority> {
    Ok(ModuleImportComponentAuthority {
        component: component.component.clone(),
        component_sha256: component.sha256.clone(),
        package_component_id: module_component.component_id.clone(),
        package_component_sha256: parse_authority_sha256(
            "package module component",
            &module_component.sha256,
        )?,
        committed_target_object_id: module_document.target_object_id.clone(),
        committed_target_revision_id: module_document.target_revision_id.clone(),
        committed_result_sha256: parse_authority_sha256(
            "package module commit",
            &module_document.result_sha256,
        )?,
        committed_content_sha256: None,
    })
}

pub(super) fn document_module_component_authority(
    component: &lorepia_domain::ComponentHash,
    kind: PackageComponentKind,
    target_object_id: &str,
    target_revision_id: &str,
    authority: &CompletedPackageAuthority,
) -> CoreResult<ModuleImportComponentAuthority> {
    // The approved document hash covers the tagged package-commit envelope.
    // `read_linked_module_authority_component` has already authenticated the
    // inner child revision and its component hash, so bind that immutable child
    // to the exact committed object/revision/component tuple here.
    let matches = authority
        .enabled_components
        .iter()
        .filter(|candidate| candidate.kind == kind)
        .flat_map(|candidate| {
            candidate
                .committed_documents
                .iter()
                .filter(|document| {
                    document.target_object_id == target_object_id
                        && document.target_revision_id == target_revision_id
                        && document.source_component_sha256 == candidate.sha256
                })
                .map(move |document| (candidate, document))
        })
        .collect::<Vec<_>>();
    let [(package_component, document)] = matches.as_slice() else {
        return Err(package_authority_denied(
            "completed package authority does not cover an exact module component revision",
        ));
    };
    Ok(ModuleImportComponentAuthority {
        component: component.component.clone(),
        component_sha256: component.sha256.clone(),
        package_component_id: package_component.component_id.clone(),
        package_component_sha256: parse_authority_sha256(
            "package component",
            &package_component.sha256,
        )?,
        committed_target_object_id: document.target_object_id.clone(),
        committed_target_revision_id: document.target_revision_id.clone(),
        committed_result_sha256: parse_authority_sha256(
            "package component commit",
            &document.result_sha256,
        )?,
        committed_content_sha256: None,
    })
}

pub(super) fn asset_module_component_authority(
    component: &lorepia_domain::ComponentHash,
    asset_id: &AssetId,
    descriptor: &AssetDescriptor,
    module_component: &CompletedPackageComponentAuthority,
    authority: &CompletedPackageAuthority,
) -> CoreResult<ModuleImportComponentAuthority> {
    let asset_matches = authority
        .committed_assets
        .iter()
        .filter(|asset| {
            asset.asset_id == *asset_id
                && asset.descriptor == *descriptor
                && asset.descriptor_sha256 == component.sha256.as_str()
                && asset.cas_sha256 == descriptor.sha256.as_str()
        })
        .collect::<Vec<_>>();
    let [asset] = asset_matches.as_slice() else {
        return Err(package_authority_denied(
            "completed package authority does not cover an exact module asset",
        ));
    };
    let source_matches = asset
        .source_components
        .iter()
        .filter(|source| {
            source.component_id == module_component.component_id
                && source.component_sha256 == module_component.sha256
        })
        .collect::<Vec<_>>();
    let [source] = source_matches.as_slice() else {
        return Err(package_authority_denied(
            "completed package authority does not bind the exact asset to the module component",
        ));
    };
    let descriptor_sha256 =
        parse_authority_sha256("package asset descriptor", &asset.descriptor_sha256)?;
    Ok(ModuleImportComponentAuthority {
        component: component.component.clone(),
        component_sha256: component.sha256.clone(),
        package_component_id: source.component_id.clone(),
        package_component_sha256: parse_authority_sha256(
            "package asset component",
            &source.component_sha256,
        )?,
        committed_target_object_id: asset.asset_id.as_str().to_owned(),
        committed_target_revision_id: asset.descriptor_sha256.clone(),
        committed_result_sha256: descriptor_sha256,
        committed_content_sha256: Some(parse_authority_sha256(
            "package asset content",
            &asset.cas_sha256,
        )?),
    })
}

fn parse_authority_sha256(label: &str, value: &str) -> CoreResult<Sha256Digest> {
    Sha256Digest::parse(value.to_owned())
        .map_err(|error| storage_corrupted(format!("completed {label} hash is invalid: {error}")))
}

fn package_authority_denied(message: &'static str) -> CoreError {
    CoreError::new(CoreErrorCode::PermissionDenied, message, false)
}
