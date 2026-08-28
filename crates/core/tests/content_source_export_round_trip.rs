use std::{
    fs::{self, File},
    io::Write,
    path::Path,
};

use lorepia_core::{
    ConnectionBoundCredential, ContentPackageApprovalRequest, ContentPackageCommitRequest,
    ContentPackageImportInspection, ContentPackageSelectionReceipt, ContentPackageSelectionRequest,
    ContentSourceExportKind, ContentSourceExportSelector, Core, CoreConfig, PackageCapability,
    ProviderConnectionId,
};
use lorepia_domain::CoreErrorCode;
use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::tempdir;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

const EXPORT_CREDENTIAL_CANARY: &str = "sk-synthetic-package-export-canary-4f91";
const EXPORT_PATH_CANARY: &str = "private-origin-path-canary";

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn write_synthetic_transform_package(path: &Path) -> Vec<u8> {
    let transform = serde_json::to_vec(&json!({
        "id": "export-canary-transform",
        "name": "Synthetic export transform",
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
    let manifest = json!({
        "format": "lorepia_content_package",
        "format_version": 1,
        "package_id": "dev.lorepia.export-canary-test",
        "name": "Synthetic export canary package",
        "version": "1.0.0",
        "author": "LorePia tests",
        "license": "MIT",
        "redistribution_allowed": true,
        "required_app_version": "0.1.0",
        "required_capabilities": ["safe_transforms"],
        "dependencies": [],
        "conflicts": [],
        "content_hashes": {"transforms/rules.json": sha256(&transform)},
        "content_types": {"transforms/rules.json": "application/json"},
        "components": [{
            "id": "transform",
            "path": "transforms/rules.json",
            "kind": "transform"
        }],
        "signature": null
    });
    let file = File::create(path).expect("create synthetic package");
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
    fs::read(path).expect("read synthetic package")
}

fn select_package(
    core: &Core,
    inspection: &ContentPackageImportInspection,
) -> ContentPackageSelectionReceipt {
    core.select_content_package_import(
        &inspection.import_id,
        &ContentPackageSelectionRequest {
            expected_revision: inspection.revision,
            expected_package_plan_hash: inspection.inspection.plan_hash.clone(),
            expected_review_sha256: inspection.review.review_sha256.clone(),
            expected_capability_review_sha256: inspection.capability_review_sha256.clone(),
            selected_component_ids: vec!["transform".to_owned()],
        },
    )
    .expect("select package component")
}

fn complete_package(core: &Core, path: &Path, approval_id: &str) -> ContentPackageImportInspection {
    let inspection = core
        .inspect_content_package_import(path)
        .expect("inspect package");
    let selection = select_package(core, &inspection);
    let approval = core
        .approve_content_package_import(
            &inspection.import_id,
            &ContentPackageApprovalRequest {
                expected_revision: selection.import.revision,
                expected_package_plan_hash: inspection.inspection.plan_hash.clone(),
                expected_content_selection_plan_hash: selection
                    .content_selection
                    .selection_plan_hash
                    .clone(),
                expected_review_sha256: inspection.review.review_sha256.clone(),
                expected_import_plan_sha256: selection.import_plan.plan_sha256.clone(),
                expected_capability_review_sha256: inspection.capability_review_sha256.clone(),
                expected_normalization_evidence_sha256: selection
                    .normalization_evidence_sha256
                    .clone(),
                expected_target_review_sha256: selection.target_review.target_review_sha256.clone(),
                confirmed_update_targets: Vec::new(),
                approval_id: approval_id.to_owned(),
                enable_component_ids: vec!["transform".to_owned()],
                approved_capabilities: vec![PackageCapability::Transforms],
            },
        )
        .expect("approve package");
    core.commit_content_package_import(
        &inspection.import_id,
        &ContentPackageCommitRequest {
            expected_revision: approval.import.revision,
            expected_package_plan_hash: inspection.inspection.plan_hash.clone(),
            expected_content_selection_plan_hash: selection.content_selection.selection_plan_hash,
            expected_review_sha256: inspection.review.review_sha256.clone(),
            expected_import_plan_sha256: selection.import_plan.plan_sha256,
            expected_approval_sha256: approval.approved_plan.approval_sha256,
            expected_capability_review_sha256: inspection.capability_review_sha256.clone(),
            expected_normalization_evidence_sha256: approval.normalization_evidence_sha256,
        },
    )
    .expect("commit package");
    inspection
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn assert_tree_omits(root: &Path, forbidden: &[u8]) {
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(path).expect("read data tree") {
            let entry = entry.expect("data entry");
            let file_type = entry.file_type().expect("entry type");
            if file_type.is_dir() {
                pending.push(entry.path());
            } else if file_type.is_file() {
                let bytes = fs::read(entry.path()).expect("read data file");
                assert!(
                    !contains_bytes(&bytes, forbidden),
                    "private canary reached a durable data file"
                );
            }
        }
    }
}

#[test]
fn nested_unknown_ccv3_source_round_trips_byte_for_byte_through_committed_cas_export() {
    let fixture_root = tempdir().expect("fixture root");
    let first_data_root = tempdir().expect("first data root");
    let second_data_root = tempdir().expect("second data root");
    let source_path = fixture_root.path().join("nested-unknown-card.json");
    let source_bytes = br#"{
  "spec": "chara_card_v3",
  "data": {
    "name": "Nested unknown export",
    "description": "Project-owned synthetic round-trip fixture",
    "personality": "Careful",
    "scenario": "A lossless export test",
    "first_mes": "Hello.",
    "extensions": {
      "synthetic_nested": {
        "layers": [
          {"ordinal": 1, "payload": {"enabled": false, "labels": ["one", "two"]}},
          {"ordinal": 2, "payload": {"nullable": null, "count": 7}}
        ],
        "metadata": {"owner": "project-synthetic", "version": 1}
      }
    },
    "future_nested_field": {
      "alpha": {"beta": [{"gamma": "preserve exactly"}]}
    }
  }
}"#;
    fs::write(&source_path, source_bytes).expect("write synthetic CCv3 source");

    let first_core = Core::open(CoreConfig::new(first_data_root.path())).expect("open first Core");
    let first_inspection = first_core
        .inspect_import(&source_path)
        .expect("inspect CCv3 source");
    let first_character = first_core
        .commit_import(&first_inspection.id)
        .expect("commit CCv3 source");
    let first_content = first_core
        .get_character_content(&first_character.id)
        .expect("load first normalized content");
    assert!(first_content.value.unknown_extensions.entries.len() >= 2);

    let prepared = first_core
        .prepare_content_source_export(&ContentSourceExportSelector::CharacterSource {
            character_id: first_character.id.clone(),
        })
        .expect("prepare exact committed source export");
    assert_eq!(
        prepared.descriptor().kind,
        ContentSourceExportKind::CharacterCardV3
    );
    assert_eq!(prepared.descriptor().sha256, first_inspection.source_sha256);
    assert_eq!(
        prepared.descriptor().size_bytes,
        u64::try_from(source_bytes.len()).expect("fixture size")
    );
    assert!(!prepared.descriptor().suggested_file_name.contains('/'));

    let exported_path = fixture_root.path().join("saved-card.json");
    fs::copy(prepared.source_path(), &exported_path).expect("simulate native scoped save");
    assert_eq!(
        fs::read(&exported_path).expect("read exported bytes"),
        source_bytes,
        "unknown nested fields and original formatting must remain byte-for-byte exact"
    );

    let second_core =
        Core::open(CoreConfig::new(second_data_root.path())).expect("open second Core");
    let second_inspection = second_core
        .inspect_import(&exported_path)
        .expect("reinspect exported CCv3 source");
    let second_character = second_core
        .commit_import(&second_inspection.id)
        .expect("recommit exported CCv3 source");
    let second_content = second_core
        .get_character_content(&second_character.id)
        .expect("load reimported normalized content");

    assert_eq!(
        second_inspection.source_sha256,
        first_inspection.source_sha256
    );
    assert_eq!(second_content.value, first_content.value);
}

#[test]
fn export_rejects_tampered_committed_character_source_bytes() {
    let fixture_root = tempdir().expect("fixture root");
    let data_root = tempdir().expect("data root");
    let source_path = fixture_root.path().join("card.json");
    let source_bytes = br#"{"spec":"chara_card_v3","data":{"name":"Tamper guard"}}"#;
    fs::write(&source_path, source_bytes).expect("write synthetic source");

    let core = Core::open(CoreConfig::new(data_root.path())).expect("open Core");
    let inspection = core.inspect_import(&source_path).expect("inspect source");
    let character = core.commit_import(&inspection.id).expect("commit source");
    let (prefix, suffix) = inspection.source_sha256.split_at(2);
    let cas_path = data_root
        .path()
        .join("sources")
        .join("sha256")
        .join(prefix)
        .join(suffix);
    fs::write(&cas_path, vec![b'x'; source_bytes.len()]).expect("tamper CAS source");

    let error = core
        .prepare_content_source_export(&ContentSourceExportSelector::CharacterSource {
            character_id: character.id,
        })
        .expect_err("tampered CAS bytes must not be exportable");
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
}

#[test]
fn package_export_vertical_omits_credentials_and_origin_paths_and_reimports_exact_bytes() {
    let fixture_root = tempdir().expect("fixture root");
    let first_data_root = tempdir().expect("first data root");
    let second_data_root = tempdir().expect("second data root");
    let source_path = fixture_root
        .path()
        .join(format!("{EXPORT_PATH_CANARY}.zip"));
    let source_bytes = write_synthetic_transform_package(&source_path);
    let origin_path = source_path.display().to_string();
    for forbidden in [EXPORT_CREDENTIAL_CANARY.as_bytes(), origin_path.as_bytes()] {
        assert!(!contains_bytes(&source_bytes, forbidden));
    }

    // The synthetic credential exists in the test process, but the package
    // APIs have no credential parameter and must not ambiently capture it.
    let credential = ConnectionBoundCredential::new(
        ProviderConnectionId::from("export-canary-provider"),
        Some(EXPORT_CREDENTIAL_CANARY.to_owned()),
    );
    assert!(!format!("{credential:?}").contains(EXPORT_CREDENTIAL_CANARY));

    let first_core = Core::open(CoreConfig::new(first_data_root.path())).expect("open first Core");
    let first_inspection = complete_package(&first_core, &source_path, "approval-export-first");
    let authority = first_core
        .get_content_package_import_review(&first_inspection.import_id)
        .expect("reopen completed package authority");
    let authority_json = serde_json::to_vec(&authority).expect("encode safe authority");
    let authority_debug = format!("{authority:?}");
    for forbidden in [EXPORT_CREDENTIAL_CANARY.as_bytes(), origin_path.as_bytes()] {
        assert!(!contains_bytes(&authority_json, forbidden));
        assert!(!contains_bytes(authority_debug.as_bytes(), forbidden));
    }

    let prepared = first_core
        .prepare_content_source_export(&ContentSourceExportSelector::ContentPackage {
            import_id: first_inspection.import_id.clone(),
        })
        .expect("prepare completed package export");
    assert_eq!(
        prepared.descriptor().kind,
        ContentSourceExportKind::LorepiaPackage
    );
    assert_eq!(prepared.descriptor().sha256, sha256(&source_bytes));
    let prepared_debug = format!("{prepared:?}");
    assert!(!prepared_debug.contains(EXPORT_CREDENTIAL_CANARY));
    assert!(!prepared_debug.contains(&origin_path));

    let exported_path = fixture_root.path().join("delivered-package.zip");
    fs::copy(prepared.source_path(), &exported_path).expect("simulate scoped native save");
    let exported_bytes = fs::read(&exported_path).expect("read delivered package");
    assert_eq!(exported_bytes, source_bytes);
    assert!(!contains_bytes(
        &exported_bytes,
        EXPORT_CREDENTIAL_CANARY.as_bytes()
    ));
    assert!(!contains_bytes(&exported_bytes, origin_path.as_bytes()));

    let second_core =
        Core::open(CoreConfig::new(second_data_root.path())).expect("open second Core");
    let second_inspection =
        complete_package(&second_core, &exported_path, "approval-export-second");
    assert_eq!(
        second_inspection.inspection.source_sha256,
        first_inspection.inspection.source_sha256
    );
    let second_prepared = second_core
        .prepare_content_source_export(&ContentSourceExportSelector::ContentPackage {
            import_id: second_inspection.import_id,
        })
        .expect("prepare reimported package export");
    assert_eq!(
        fs::read(second_prepared.source_path()).expect("read reimported CAS bytes"),
        source_bytes
    );
    let second_authority = second_core
        .get_content_package_import_review(second_prepared.descriptor().source_id.as_str())
        .expect("reopen reimported package authority");
    let second_authority_json =
        serde_json::to_vec(&second_authority).expect("encode reimported safe authority");
    for forbidden in [EXPORT_CREDENTIAL_CANARY.as_bytes(), origin_path.as_bytes()] {
        assert!(!contains_bytes(&second_authority_json, forbidden));
    }

    fs::write(
        prepared.source_path(),
        vec![b'x'; usize::try_from(prepared.descriptor().size_bytes).expect("small fixture")],
    )
    .expect("tamper first CAS");
    let error = first_core
        .prepare_content_source_export(&ContentSourceExportSelector::ContentPackage {
            import_id: first_inspection.import_id,
        })
        .expect_err("tampered package CAS must fail closed");
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
    let rendered_error = format!("{error:?}");
    assert!(!rendered_error.contains(EXPORT_CREDENTIAL_CANARY));
    assert!(!rendered_error.contains(&origin_path));

    // Windows holds SQLite files with sharing modes that correctly reject a
    // concurrent byte scan. Close every Core before inspecting durable files;
    // the in-memory DTO/export assertions above remain the functional half of
    // this regression test.
    drop(second_prepared);
    drop(second_core);
    drop(prepared);
    drop(first_core);
    for forbidden in [EXPORT_CREDENTIAL_CANARY.as_bytes(), origin_path.as_bytes()] {
        assert_tree_omits(first_data_root.path(), forbidden);
        assert_tree_omits(second_data_root.path(), forbidden);
    }
}
