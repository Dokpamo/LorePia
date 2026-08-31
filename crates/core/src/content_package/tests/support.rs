    use std::{collections::BTreeMap, io::Write};

    use lorepia_domain::{
        ActivationRule, ApiFamily, BlockSource, CharacterPromptContent, ContentModule,
        ContentModuleId, ConversationBranchId, ConversationId, InstructionAuthority,
        InteractionRuleSetId, KnowledgeBook, KnowledgeBookId, KnowledgeEntry, KnowledgeEntryId,
        KnowledgePlacement, MemoryProfile, MemoryProfileId, MessageId, ModuleBindingId,
        ModuleRevisionId, ModuleRevisionResolutionMode, ModuleScope, PackageMetadata,
        PlacementZone, PromptBlockKind, PromptConversationMessage, PromptMessageRole, PromptPreset,
        PromptPresetId, PromptResolutionContext, PromptResolveRequest, ProviderMessageRole,
        RoleHint, SafeTemplate, SummarySchemaId, TaskProfileId, TemplatePart, TokenBudget,
        TokenPolicy, TransformSet, TransformSetId,
    };
    use lorepia_providers::parameter_mapping::PromptCacheWireDialect;
    use lorepia_providers::{DeveloperRoleCapability, ProviderPromptAdapterContract};
    use lorepia_storage::PackageDocumentTargetDisposition;
    use rusqlite::Connection;
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use tempfile::{NamedTempFile, tempdir};
    use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

    use super::*;
    use crate::{
        ContentModuleActivationRequest, ContentModuleBindingDraft,
        ContentModuleRollbackApplyRequest, ContentModuleRollbackResolutionRequest,
        ContentModuleRuntimeBindingDisposition, ContentModuleRuntimeTarget,
        ContentSourceExportKind, ContentSourceExportSelector, CoreConfig, ModuleActivationApproval,
        ModuleMergeResolutionSet, VariableMap,
    };

    fn synthetic_transform_package(path: &Path) {
        let transform = serde_json::to_vec(&json!({
            "id": "core-package-transform",
            "name": "Synthetic transform",
            "schema_version": 1,
            "enabled": true,
            "rules": [],
            "max_rules_per_phase": 8,
            "max_output_chars": 4096,
            "provenance": {
                "source_kind": "imported_package",
                "source_id": null,
                "source_hash": null,
                "author": null,
                "license": null,
                "imported_at": null
            }
        }))
        .expect("encode transform");
        let mut hashes = BTreeMap::new();
        hashes.insert(
            "transforms/rules.json",
            format!("{:x}", Sha256::digest(&transform)),
        );
        let manifest = json!({
            "format": "lorepia_content_package",
            "format_version": 1,
            "package_id": "dev.lorepia.core-package-test",
            "name": "Synthetic Core package",
            "version": "1.0.0",
            "author": "LorePia tests",
            "license": "MIT",
            "redistribution_allowed": true,
            "required_app_version": "0.1.0",
            "required_capabilities": ["safe_transforms"],
            "dependencies": [],
            "conflicts": [],
            "content_hashes": hashes,
            "content_types": {"transforms/rules.json": "application/json"},
            "components": [{
                "id": "transform",
                "path": "transforms/rules.json",
                "kind": "transform"
            }],
            "signature": null
        });
        let file = File::create(path).expect("create package");
        let mut archive = ZipWriter::new(file);
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .unix_permissions(0o644);
        archive
            .start_file("manifest.json", options)
            .expect("start manifest");
        archive
            .write_all(&serde_json::to_vec(&manifest).expect("encode manifest"))
            .expect("write manifest");
        archive
            .start_file("transforms/rules.json", options)
            .expect("start transform");
        archive.write_all(&transform).expect("write transform");
        archive.finish().expect("finish package");
    }

    fn synthetic_transform_array_package(path: &Path) {
        let transform = |id: &str, name: &str| {
            json!({
                "id": id,
                "name": name,
                "schema_version": 1,
                "enabled": true,
                "rules": [],
                "max_rules_per_phase": 8,
                "max_output_chars": 4096,
                "provenance": {
                    "source_kind": "imported_package",
                    "source_id": null,
                    "source_hash": null,
                    "author": null,
                    "license": null,
                    "imported_at": null
                }
            })
        };
        let payload = serde_json::to_vec(&json!([
            transform("array-transform-a", "Array A"),
            transform("array-transform-b", "Array B")
        ]))
        .expect("encode transform array");
        let digest = format!("{:x}", Sha256::digest(&payload));
        let manifest = json!({
            "format": "lorepia_content_package",
            "format_version": 1,
            "package_id": "dev.lorepia.core-array-test",
            "name": "Synthetic array package",
            "version": "1.0.0",
            "author": "LorePia tests",
            "license": "MIT",
            "redistribution_allowed": true,
            "required_app_version": "0.1.0",
            "required_capabilities": ["safe_transforms"],
            "dependencies": [],
            "conflicts": [],
            "content_hashes": {"transforms/array.json": digest},
            "content_types": {"transforms/array.json": "application/json"},
            "components": [{
                "id": "transform-array",
                "path": "transforms/array.json",
                "kind": "transform"
            }],
            "signature": null
        });
        let file = File::create(path).expect("create package");
        let mut archive = ZipWriter::new(file);
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .unix_permissions(0o644);
        archive
            .start_file("manifest.json", options)
            .expect("start manifest");
        archive
            .write_all(&serde_json::to_vec(&manifest).expect("encode manifest"))
            .expect("write manifest");
        archive
            .start_file("transforms/array.json", options)
            .expect("start transform array");
        archive.write_all(&payload).expect("write transform array");
        archive.finish().expect("finish package");
    }

    fn local_transform_set(id: &str, name: &str) -> TransformSet {
        TransformSet {
            id: TransformSetId::from(id),
            name: name.to_owned(),
            schema_version: 1,
            enabled: false,
            imported_author_enabled: false,
            rules: Vec::new(),
            max_rules_per_phase: 8,
            max_output_chars: 4096,
            provenance: Provenance {
                source_kind: SourceKind::UserCreated,
                source_id: Some(format!("test:{id}")),
                source_hash: None,
                author: None,
                license: None,
                imported_at: None,
            },
        }
    }

    fn synthetic_prompt_package(path: &Path, preset: &PromptPreset, package_id: &str) {
        let payload = serde_json::to_vec(preset).expect("encode prompt preset");
        let digest = format!("{:x}", Sha256::digest(&payload));
        let manifest = json!({
            "format": "lorepia_content_package",
            "format_version": 1,
            "package_id": package_id,
            "name": "Synthetic prompt package",
            "version": "1.0.0",
            "author": "LorePia tests",
            "license": "MIT",
            "redistribution_allowed": true,
            "required_app_version": "0.1.0",
            "required_capabilities": ["prompt_presets"],
            "dependencies": [],
            "conflicts": [],
            "content_hashes": {"prompt/preset.json": digest},
            "content_types": {"prompt/preset.json": "application/json"},
            "components": [{
                "id": "prompt",
                "path": "prompt/preset.json",
                "kind": "prompt"
            }],
            "signature": null
        });
        let file = File::create(path).expect("create package");
        let mut archive = ZipWriter::new(file);
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .unix_permissions(0o644);
        archive
            .start_file("manifest.json", options)
            .expect("start manifest");
        archive
            .write_all(&serde_json::to_vec(&manifest).expect("encode manifest"))
            .expect("write manifest");
        archive
            .start_file("prompt/preset.json", options)
            .expect("start prompt preset");
        archive.write_all(&payload).expect("write prompt preset");
        archive.finish().expect("finish package");
    }

    fn imported_prompt_preset(id: &str) -> PromptPreset {
        let built_in = built_in_prompt_presets()[0].clone();
        let mut metadata = built_in.metadata;
        metadata.description = "Synthetic imported prompt preset".to_owned();
        metadata.local_override_of = None;
        let mut preset = lorepia_orchestration::default_prompt_preset(
            PromptPresetId::from(id),
            "Imported prompt preset",
            metadata,
        );
        preset.blocks.insert(0, built_in.blocks[0].clone());
        preset
    }

    fn content_cas_path(root: &Path, namespace: &str, sha256: &str) -> PathBuf {
        assert_eq!(sha256.len(), 64, "test digest must be canonical");
        root.join(namespace)
            .join("sha256")
            .join(&sha256[..2])
            .join(&sha256[2..])
    }

    fn synthetic_media_package(path: &Path) -> Vec<String> {
        let media = [
            (
                "image",
                b"\x89PNG\r\n\x1a\nsynthetic".as_slice(),
                "image/png",
                "png",
            ),
            ("audio", b"ID3synthetic".as_slice(), "audio/mpeg", "mp3"),
            (
                "video",
                b"\x00\x00\x00\x18ftypisomsynthetic".as_slice(),
                "video/mp4",
                "mp4",
            ),
        ];
        let mut hashes = BTreeMap::new();
        let mut content_types = BTreeMap::new();
        let mut components = Vec::new();
        let mut entries = Vec::new();
        for (id, bytes, media_type, extension) in media {
            let digest = format!("{:x}", Sha256::digest(bytes));
            let logical_path = format!("assets/sha256/{digest}.{extension}");
            hashes.insert(logical_path.clone(), digest);
            content_types.insert(logical_path.clone(), media_type.to_owned());
            components.push(json!({
                "id": id,
                "path": logical_path,
                "kind": "asset",
                "required_capabilities": ["media_assets"]
            }));
            entries.push((logical_path, bytes));
        }
        let manifest = json!({
            "format": "lorepia_content_package",
            "format_version": 1,
            "package_id": "dev.lorepia.core-media-test",
            "name": "Synthetic media package",
            "version": "1.0.0",
            "author": "LorePia tests",
            "license": "MIT",
            "redistribution_allowed": true,
            "required_app_version": "0.1.0",
            "required_capabilities": ["media_assets"],
            "dependencies": [],
            "conflicts": [],
            "content_hashes": hashes,
            "content_types": content_types,
            "components": components,
            "signature": null
        });
        let file = File::create(path).expect("create package");
        let mut archive = ZipWriter::new(file);
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .unix_permissions(0o644);
        archive
            .start_file("manifest.json", options)
            .expect("start manifest");
        archive
            .write_all(&serde_json::to_vec(&manifest).expect("encode manifest"))
            .expect("write manifest");
        for (logical_path, bytes) in entries {
            archive
                .start_file(logical_path, options)
                .expect("start media");
            archive.write_all(bytes).expect("write media");
        }
        archive.finish().expect("finish package");
        vec!["audio".to_owned(), "image".to_owned(), "video".to_owned()]
    }

    fn synthetic_content_module_package(path: &Path) -> (ContentModuleId, AssetId, Vec<String>) {
        synthetic_content_module_package_revision(path, "1.0.0", "one")
    }

    fn synthetic_content_module_package_revision(
        path: &Path,
        version: &str,
        marker: &str,
    ) -> (ContentModuleId, AssetId, Vec<String>) {
        let mut asset_bytes = b"\x89PNG\r\n\x1a\nsynthetic module illustration ".to_vec();
        asset_bytes.extend_from_slice(marker.as_bytes());
        let asset_sha256 = format!("{:x}", Sha256::digest(&asset_bytes));
        let asset_id = AssetId::from(format!("sha256:{asset_sha256}"));
        let asset_path = format!("assets/sha256/{asset_sha256}.png");
        let module_id = ContentModuleId::from("core.package.content-module");
        let module = json!({
            "id": module_id.as_str(),
            "name": format!("Synthetic imported module {marker}"),
            "version": version,
            "schema_version": 1,
            "prompt_fragments": [],
            "knowledge_book_ids": [],
            "control_specs": [],
            "transform_set_ids": [],
            "interaction_rule_set_ids": [],
            "asset_ids": [asset_id.as_str()],
            "imported_components_enabled": false,
            "required_capabilities": ["image_assets"],
            "metadata": {
                "author": "Untrusted package field",
                "license": "LicenseRef-Untrusted",
                "redistribution_allowed": false,
                "homepage": null,
                "description": "Strictly declarative module fixture",
                "tags": ["synthetic"],
                "provenance": {
                    "source_kind": "user_created",
                    "source_id": null,
                    "source_hash": null,
                    "author": null,
                    "license": null,
                    "imported_at": null
                }
            }
        });
        let module_bytes = serde_json::to_vec(&module).expect("encode module");
        let module_sha256 = format!("{:x}", Sha256::digest(&module_bytes));
        let manifest = json!({
            "format": "lorepia_content_package",
            "format_version": 1,
            "package_id": "dev.lorepia.core-content-module-test",
            "name": "Synthetic content module package",
            "version": version,
            "author": "LorePia tests",
            "license": "MIT",
            "redistribution_allowed": true,
            "required_app_version": "0.1.0",
            "required_capabilities": ["content_modules", "media_assets"],
            "dependencies": [],
            "conflicts": [],
            "content_hashes": {
                asset_path.clone(): asset_sha256,
                "modules/module.json": module_sha256
            },
            "content_types": {
                asset_path.clone(): "image/png",
                "modules/module.json": "application/json"
            },
            "components": [
                {
                    "id": "00-module-image",
                    "path": asset_path.clone(),
                    "kind": "asset",
                    "required_capabilities": ["media_assets"]
                },
                {
                    "id": "10-content-module",
                    "path": "modules/module.json",
                    "kind": "content_module"
                }
            ],
            "signature": null
        });
        write_synthetic_content_module_archive(
            path,
            &manifest,
            &asset_path,
            &asset_bytes,
            &module_bytes,
        );
        (
            module_id,
            asset_id,
            vec!["00-module-image".to_owned(), "10-content-module".to_owned()],
        )
    }

    fn write_synthetic_content_module_archive(
        path: &Path,
        manifest: &serde_json::Value,
        asset_path: &str,
        asset_bytes: &[u8],
        module_bytes: &[u8],
    ) {
        let file = File::create(path).expect("create module package");
        let mut archive = ZipWriter::new(file);
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .unix_permissions(0o644);
        archive
            .start_file("manifest.json", options)
            .expect("start module manifest");
        archive
            .write_all(&serde_json::to_vec(manifest).expect("encode module manifest"))
            .expect("write module manifest");
        archive
            .start_file(asset_path, options)
            .expect("start module image");
        archive.write_all(asset_bytes).expect("write module image");
        archive
            .start_file("modules/module.json", options)
            .expect("start module document");
        archive
            .write_all(module_bytes)
            .expect("write module document");
        archive.finish().expect("finish module package");
    }

    struct SyntheticLinkedContentModulePackage {
        module_id: ContentModuleId,
        knowledge_book_id: KnowledgeBookId,
        transform_set_id: TransformSetId,
        interaction_rule_set_id: InteractionRuleSetId,
        component_ids: Vec<String>,
    }

    fn synthetic_linked_content_module_package(path: &Path) -> SyntheticLinkedContentModulePackage {
        let module_id = ContentModuleId::from("core.package.linked-content-module");
        let knowledge_book_id = KnowledgeBookId::from("core.package.linked-knowledge");
        let transform_set_id = TransformSetId::from("core.package.linked-transform");
        let interaction_rule_set_id =
            InteractionRuleSetId::from("core.package.linked-interactions");
        let entries = [
            (
                "knowledge/books.json",
                synthetic_linked_knowledge(&knowledge_book_id),
            ),
            (
                "transforms/rules.json",
                synthetic_linked_transform(&transform_set_id),
            ),
            (
                "interactions/rules.json",
                synthetic_linked_interactions(&interaction_rule_set_id),
            ),
            (
                "modules/module.json",
                synthetic_linked_module(
                    &module_id,
                    &knowledge_book_id,
                    &transform_set_id,
                    &interaction_rule_set_id,
                ),
            ),
        ];
        let component_ids = vec![
            "00-linked-knowledge".to_owned(),
            "10-linked-transform".to_owned(),
            "20-linked-interactions".to_owned(),
            "30-linked-module".to_owned(),
        ];
        let manifest = synthetic_linked_manifest(&entries, &component_ids);
        write_synthetic_linked_archive(path, &manifest, entries);
        SyntheticLinkedContentModulePackage {
            module_id,
            knowledge_book_id,
            transform_set_id,
            interaction_rule_set_id,
            component_ids,
        }
    }

    fn synthetic_linked_provenance() -> serde_json::Value {
        json!({
            "source_kind": "user_created",
            "source_id": null,
            "source_hash": null,
            "author": null,
            "license": null,
            "imported_at": null
        })
    }

    fn synthetic_linked_knowledge(id: &KnowledgeBookId) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "id": id.as_str(),
            "name": "Synthetic linked knowledge",
            "schema_version": 1,
            "entries": [],
            "scan_depth": 8,
            "token_budget": {"max_tokens": 1024},
            "recursive": false,
            "max_recursion_depth": 0,
            "provenance": synthetic_linked_provenance()
        }))
        .expect("encode linked knowledge")
    }

    fn synthetic_linked_transform(id: &TransformSetId) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "id": id.as_str(),
            "name": "Synthetic linked transform",
            "schema_version": 1,
            "enabled": true,
            "rules": [],
            "max_rules_per_phase": 8,
            "max_output_chars": 4096,
            "provenance": synthetic_linked_provenance()
        }))
        .expect("encode linked transform")
    }

    fn synthetic_linked_interactions(id: &InteractionRuleSetId) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "id": id.as_str(),
            "name": "Synthetic linked interactions",
            "schema_version": 1,
            "rules": [{
                "id": "core.package.linked-interaction-rule",
                "name": "Synthetic linked interaction rule",
                "enabled": true,
                "event": {"kind": "conversation_opened"},
                "condition": null,
                "actions": [{
                    "kind": "append_visible_system_event",
                    "text": {
                        "parts": [{
                            "kind": "text",
                            "value": "Synthetic linked interaction"
                        }],
                        "max_output_chars": 1024
                    }
                }],
                "priority": 0,
                "stop_after_match": false,
                "provenance": synthetic_linked_provenance()
            }],
            "max_actions_per_event": 8,
            "provenance": synthetic_linked_provenance()
        }))
        .expect("encode linked interactions")
    }

    fn synthetic_linked_module(
        module_id: &ContentModuleId,
        knowledge_book_id: &KnowledgeBookId,
        transform_set_id: &TransformSetId,
        interaction_rule_set_id: &InteractionRuleSetId,
    ) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "id": module_id.as_str(),
            "name": "Synthetic linked content module",
            "version": "1.0.0",
            "schema_version": 1,
            "prompt_fragments": [],
            "knowledge_book_ids": [knowledge_book_id.as_str()],
            "control_specs": [],
            "transform_set_ids": [transform_set_id.as_str()],
            "interaction_rule_set_ids": [interaction_rule_set_id.as_str()],
            "asset_ids": [],
            "imported_components_enabled": true,
            "required_capabilities": [
                "knowledge",
                "transforms",
                "declarative_interactions"
            ],
            "metadata": {
                "author": null,
                "license": "LicenseRef-Untrusted",
                "redistribution_allowed": false,
                "homepage": null,
                "description": "Declarative module with three immutable child revisions",
                "tags": ["synthetic"],
                "provenance": synthetic_linked_provenance()
            }
        }))
        .expect("encode linked content module")
    }

    fn synthetic_linked_manifest(
        entries: &[(&str, Vec<u8>)],
        component_ids: &[String],
    ) -> serde_json::Value {
        let hashes = entries
            .iter()
            .map(|(logical_path, bytes)| {
                (
                    (*logical_path).to_owned(),
                    format!("{:x}", Sha256::digest(bytes)),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let content_types = entries
            .iter()
            .map(|(logical_path, _)| ((*logical_path).to_owned(), "application/json"))
            .collect::<BTreeMap<_, _>>();
        json!({
            "format": "lorepia_content_package",
            "format_version": 1,
            "package_id": "dev.lorepia.core-linked-content-module-test",
            "name": "Synthetic linked content module package",
            "version": "1.0.0",
            "author": "LorePia tests",
            "license": "MIT",
            "redistribution_allowed": true,
            "required_app_version": "0.1.0",
            "required_capabilities": [
                "content_modules",
                "knowledge_books",
                "safe_transforms",
                "declarative_interactions"
            ],
            "dependencies": [],
            "conflicts": [],
            "content_hashes": hashes,
            "content_types": content_types,
            "components": [
                {
                    "id": component_ids[0],
                    "path": "knowledge/books.json",
                    "kind": "knowledge"
                },
                {
                    "id": component_ids[1],
                    "path": "transforms/rules.json",
                    "kind": "transform"
                },
                {
                    "id": component_ids[2],
                    "path": "interactions/rules.json",
                    "kind": "interaction"
                },
                {
                    "id": component_ids[3],
                    "path": "modules/module.json",
                    "kind": "content_module",
                    "depends_on": [component_ids[0], component_ids[1], component_ids[2]]
                }
            ],
            "signature": null
        })
    }

    fn write_synthetic_linked_archive(
        path: &Path,
        manifest: &serde_json::Value,
        entries: [(&str, Vec<u8>); 4],
    ) {
        let file = File::create(path).expect("create linked module package");
        let mut archive = ZipWriter::new(file);
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .unix_permissions(0o644);
        archive
            .start_file("manifest.json", options)
            .expect("start linked module manifest");
        archive
            .write_all(&serde_json::to_vec(manifest).expect("encode linked module manifest"))
            .expect("write linked module manifest");
        for (logical_path, bytes) in entries {
            archive
                .start_file(logical_path, options)
                .expect("start linked module entry");
            archive
                .write_all(&bytes)
                .expect("write linked module entry");
        }
        archive.finish().expect("finish linked module package");
    }

    fn synthetic_unbound_content_module_package(path: &Path) {
        let module = json!({
            "id": "core.package.unbound-content-module",
            "name": "Synthetic unbound imported module",
            "version": "1.0.0",
            "schema_version": 1,
            "prompt_fragments": [],
            "knowledge_book_ids": ["core.package.missing-knowledge"],
            "control_specs": [],
            "transform_set_ids": [],
            "interaction_rule_set_ids": [],
            "asset_ids": [],
            "imported_components_enabled": false,
            "required_capabilities": ["knowledge"],
            "metadata": {
                "author": null,
                "license": "MIT",
                "redistribution_allowed": true,
                "homepage": null,
                "description": "Dependency must be selected from this exact package",
                "tags": [],
                "provenance": {
                    "source_kind": "user_created",
                    "source_id": null,
                    "source_hash": null,
                    "author": null,
                    "license": null,
                    "imported_at": null
                }
            }
        });
        let module_bytes = serde_json::to_vec(&module).expect("encode unbound module");
        let module_sha256 = format!("{:x}", Sha256::digest(&module_bytes));
        let manifest = json!({
            "format": "lorepia_content_package",
            "format_version": 1,
            "package_id": "dev.lorepia.core-unbound-content-module-test",
            "name": "Synthetic unbound content module package",
            "version": "1.0.0",
            "author": "LorePia tests",
            "license": "MIT",
            "redistribution_allowed": true,
            "required_app_version": "0.1.0",
            "required_capabilities": ["content_modules", "knowledge_books"],
            "dependencies": [],
            "conflicts": [],
            "content_hashes": {"modules/unbound.json": module_sha256},
            "content_types": {"modules/unbound.json": "application/json"},
            "components": [{
                "id": "unbound-content-module",
                "path": "modules/unbound.json",
                "kind": "content_module"
            }],
            "signature": null
        });
        let file = File::create(path).expect("create unbound module package");
        let mut archive = ZipWriter::new(file);
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .unix_permissions(0o644);
        archive
            .start_file("manifest.json", options)
            .expect("start unbound module manifest");
        archive
            .write_all(&serde_json::to_vec(&manifest).expect("encode unbound module manifest"))
            .expect("write unbound module manifest");
        archive
            .start_file("modules/unbound.json", options)
            .expect("start unbound module");
        archive
            .write_all(&module_bytes)
            .expect("write unbound module");
        archive.finish().expect("finish unbound module package");
    }

    fn import_synthetic_character(core: &Core) -> String {
        let mut source = NamedTempFile::new().expect("temporary synthetic character");
        write!(
            source,
            r#"{{"spec":"chara_card_v3","data":{{"name":"Ari","description":"Entirely synthetic module package test character."}}}}"#
        )
        .expect("write synthetic character");
        let review = core
            .inspect_import(source.path())
            .expect("inspect synthetic character");
        core.commit_import(&review.id)
            .expect("commit synthetic character")
            .id
    }

    fn selection_request(
        inspection: &ContentPackageImportInspection,
        selected_component_ids: Vec<String>,
    ) -> ContentPackageSelectionRequest {
        ContentPackageSelectionRequest {
            expected_revision: inspection.revision,
            expected_package_plan_hash: inspection.inspection.plan_hash.clone(),
            expected_review_sha256: inspection.review.review_sha256.clone(),
            expected_capability_review_sha256: inspection.capability_review_sha256.clone(),
            selected_component_ids,
        }
    }

    fn approval_request(
        inspection: &ContentPackageImportInspection,
        selection: &ContentPackageSelectionReceipt,
        approval_id: &str,
        enable_component_ids: Vec<String>,
        approved_capabilities: Vec<PackageCapability>,
    ) -> ContentPackageApprovalRequest {
        ContentPackageApprovalRequest {
            expected_revision: selection.import.revision,
            expected_package_plan_hash: inspection.inspection.plan_hash.clone(),
            expected_content_selection_plan_hash: selection
                .content_selection
                .selection_plan_hash
                .clone(),
            expected_review_sha256: inspection.review.review_sha256.clone(),
            expected_import_plan_sha256: selection.import_plan.plan_sha256.clone(),
            expected_capability_review_sha256: inspection.capability_review_sha256.clone(),
            expected_normalization_evidence_sha256: selection.normalization_evidence_sha256.clone(),
            expected_target_review_sha256: selection.target_review.target_review_sha256.clone(),
            confirmed_update_targets: selection
                .target_review
                .documents
                .iter()
                .filter(|document| {
                    document.disposition
                        == lorepia_storage::PackageDocumentTargetDisposition::Update
                })
                .map(|document| PackageUpdateTargetConfirmation {
                    source_component_id: document.source_component_id.clone(),
                    component_document_ordinal: document.component_document_ordinal,
                    target_object_id: document.target_object_id.clone(),
                    expected_target_revision_id: document
                        .expected_target_revision_id
                        .clone()
                        .expect("reviewed update target revision"),
                    expected_target_state_revision: document
                        .expected_target_state_revision
                        .expect("reviewed update target state revision"),
                })
                .collect(),
            approval_id: approval_id.to_owned(),
            enable_component_ids,
            approved_capabilities,
        }
    }

    fn commit_request(
        inspection: &ContentPackageImportInspection,
        selection: &ContentPackageSelectionReceipt,
        approval: &ContentPackageApprovalReceipt,
    ) -> ContentPackageCommitRequest {
        ContentPackageCommitRequest {
            expected_revision: approval.import.revision,
            expected_package_plan_hash: inspection.inspection.plan_hash.clone(),
            expected_content_selection_plan_hash: selection
                .content_selection
                .selection_plan_hash
                .clone(),
            expected_review_sha256: inspection.review.review_sha256.clone(),
            expected_import_plan_sha256: selection.import_plan.plan_sha256.clone(),
            expected_approval_sha256: approval.approved_plan.approval_sha256.clone(),
            expected_capability_review_sha256: inspection.capability_review_sha256.clone(),
            expected_normalization_evidence_sha256: approval.normalization_evidence_sha256.clone(),
        }
    }

    fn content_module_commit_boundary_fixture() -> ContentModule {
        let mut block = built_in_prompt_presets()[0]
            .blocks
            .iter()
            .find(|block| {
                block.kind != PromptBlockKind::LatestUserTurn
                    && block.source != BlockSource::LatestUser
                    && !matches!(
                        block.placement_zone,
                        PlacementZone::ApplicationPolicy | PlacementZone::LatestUser
                    )
            })
            .expect("safe package-authored block fixture")
            .clone();
        block.authority = InstructionAuthority::Application;
        ContentModule {
            id: ContentModuleId::from("core.package.normalization-boundary"),
            name: "Core normalization boundary".to_owned(),
            version: "1.0.0".to_owned(),
            schema_version: 1,
            prompt_fragments: vec![block],
            knowledge_book_ids: Vec::new(),
            control_specs: Vec::new(),
            transform_set_ids: Vec::new(),
            interaction_rule_set_ids: Vec::new(),
            asset_ids: Vec::new(),
            imported_components_enabled: false,
            required_capabilities: vec![ContentCapability::PromptFragments],
            metadata: PackageMetadata {
                author: Some("Untrusted package".to_owned()),
                license: "LicenseRef-Untrusted".to_owned(),
                redistribution_allowed: false,
                homepage: None,
                description: "Prepared-document tamper fixture".to_owned(),
                tags: Vec::new(),
                provenance: Provenance {
                    source_kind: SourceKind::UserCreated,
                    source_id: None,
                    source_hash: None,
                    author: None,
                    license: None,
                    imported_at: None,
                },
            },
        }
    }

    fn imported_content_module_provenance() -> Provenance {
        Provenance {
            source_kind: SourceKind::ImportedPackage,
            source_id: Some("dev.lorepia.core-normalization-test".to_owned()),
            source_hash: Some("ab".repeat(32)),
            author: Some("LorePia tests".to_owned()),
            license: Some("MIT".to_owned()),
            imported_at: None,
        }
    }

