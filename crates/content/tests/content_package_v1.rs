use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};

use lorepia_content::{
    ContentPackageComponentState, PreparedContentDocument, discard_staged_content_package_assets,
    inspect_content_package, prepare_content_package_import, revalidate_content_package_selection,
    select_content_package_components, stage_selected_content_package_assets,
};
use lorepia_domain::ImportLimits;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::{TempDir, tempdir};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

struct PackageFixture {
    _directory: TempDir,
    path: PathBuf,
}

impl PackageFixture {
    fn path(&self) -> &Path {
        &self.path
    }
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn write_package(
    entries: Vec<(String, Vec<u8>, Option<&'static str>)>,
    components: Vec<Value>,
    mutate_manifest: impl FnOnce(&mut Value),
) -> PackageFixture {
    let mut hashes = BTreeMap::new();
    let mut content_types = BTreeMap::new();
    for (path, bytes, media_type) in &entries {
        hashes.insert(path.clone(), sha256(bytes));
        if let Some(media_type) = media_type {
            content_types.insert(path.clone(), (*media_type).to_owned());
        }
    }
    let mut manifest = json!({
        "format": "lorepia_content_package",
        "format_version": 1,
        "package_id": "dev.lorepia.synthetic",
        "name": "Synthetic package",
        "version": "1.0.0",
        "author": "LorePia tests",
        "license": "MIT",
        "redistribution_allowed": true,
        "required_app_version": "0.1.0",
        "required_capabilities": [],
        "dependencies": [],
        "conflicts": [],
        "content_hashes": hashes,
        "content_types": content_types,
        "components": components,
        "signature": null
    });
    mutate_manifest(&mut manifest);

    let directory = tempdir().expect("temp directory");
    let path = directory.path().join("package.zip");
    let file = File::create(&path).expect("create package");
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
    for (path, bytes, _) in entries {
        archive.start_file(path, options).expect("start entry");
        archive.write_all(&bytes).expect("write entry");
    }
    archive.finish().expect("finish package");
    PackageFixture {
        _directory: directory,
        path,
    }
}

fn provenance() -> Value {
    json!({
        "source_kind": "imported_package",
        "source_id": null,
        "source_hash": null,
        "author": null,
        "license": null,
        "imported_at": null
    })
}

#[test]
fn inspects_selects_revalidates_and_prepares_typed_documents() {
    let transform = serde_json::to_vec(&json!({
        "id": "synthetic-transform",
        "name": "Synthetic",
        "schema_version": 1,
        "enabled": true,
        "rules": [{
            "id": "synthetic-rule",
            "name": "Synthetic rule",
            "enabled": true,
            "imported_enabled": true,
            "phase": "provider_output_canonical",
            "order": 0,
            "pattern": {
                "pattern": "before",
                "case_insensitive": false
            },
            "replacement": "after",
            "condition": null,
            "max_replacements": 1,
            "input_limit": 4096,
            "output_limit": 4096,
            "provenance": provenance()
        }],
        "max_rules_per_phase": 8,
        "max_output_chars": 4096,
        "provenance": provenance()
    }))
    .expect("encode transform");
    let knowledge = serde_json::to_vec(&json!({
        "id": "synthetic-book",
        "name": "Synthetic",
        "schema_version": 1,
        "entries": [],
        "scan_depth": 8,
        "token_budget": {"max_tokens": 1024},
        "recursive": false,
        "max_recursion_depth": 0,
        "provenance": provenance()
    }))
    .expect("encode knowledge");
    let fixture = write_package(
        vec![
            (
                "transforms/rules.json".into(),
                transform,
                Some("application/json"),
            ),
            (
                "knowledge/books.json".into(),
                knowledge,
                Some("application/json"),
            ),
        ],
        vec![
            json!({
                "id": "transform",
                "path": "transforms/rules.json",
                "kind": "transform",
                "depends_on": ["knowledge"]
            }),
            json!({
                "id": "knowledge",
                "path": "knowledge/books.json",
                "kind": "knowledge"
            }),
        ],
        |_| {},
    );

    let first =
        inspect_content_package(fixture.path(), ImportLimits::default()).expect("inspection");
    let second =
        inspect_content_package(fixture.path(), ImportLimits::default()).expect("inspection");
    assert_eq!(first.plan_hash, second.plan_hash);
    assert_ne!(first.id, second.id);
    assert!(first.is_allowed());
    assert!(
        select_content_package_components(&first, &["transform".into()]).is_err(),
        "component dependencies must be selected explicitly"
    );

    let selection =
        select_content_package_components(&first, &["transform".into(), "knowledge".into()])
            .expect("valid selection");
    revalidate_content_package_selection(fixture.path(), ImportLimits::default(), &selection)
        .expect("unchanged selection");
    let prepared =
        prepare_content_package_import(fixture.path(), ImportLimits::default(), &selection)
            .expect("typed preparation");
    assert_eq!(prepared.documents.len(), 2);
}

#[test]
fn preparation_forces_imported_transforms_inactive_and_reports_the_change() {
    let transform = serde_json::to_vec(&json!({
        "id": "synthetic-transform",
        "name": "Synthetic",
        "schema_version": 1,
        "enabled": true,
        "rules": [{
            "id": "synthetic-rule",
            "name": "Synthetic rule",
            "enabled": true,
            "imported_enabled": true,
            "phase": "provider_output_canonical",
            "order": 0,
            "pattern": {
                "pattern": "before",
                "case_insensitive": false
            },
            "replacement": "after",
            "condition": null,
            "max_replacements": 1,
            "input_limit": 4096,
            "output_limit": 4096,
            "provenance": provenance()
        }],
        "max_rules_per_phase": 8,
        "max_output_chars": 4096,
        "provenance": provenance()
    }))
    .expect("encode transform");
    let fixture = write_package(
        vec![(
            "transforms/rules.json".into(),
            transform,
            Some("application/json"),
        )],
        vec![json!({
            "id": "transform",
            "path": "transforms/rules.json",
            "kind": "transform"
        })],
        |_| {},
    );
    let inspection =
        inspect_content_package(fixture.path(), ImportLimits::default()).expect("inspection");
    let selection =
        select_content_package_components(&inspection, &["transform".into()]).expect("selection");
    let prepared =
        prepare_content_package_import(fixture.path(), ImportLimits::default(), &selection)
            .expect("prepare");

    assert_eq!(prepared.documents.len(), 1);
    assert_eq!(prepared.documents[0].source_component_id, "transform");
    assert_eq!(prepared.documents[0].document_ordinal, 0);
    assert_eq!(prepared.documents[0].document_kind, "transform_set");
    assert_eq!(
        prepared.documents[0].document_sha256,
        sha256(
            &serde_json::to_vec(&prepared.documents[0].document).expect("encode prepared document")
        )
    );
    match &prepared.documents[0].document {
        PreparedContentDocument::TransformSet(set) => {
            assert!(!set.enabled);
            assert!(set.imported_author_enabled);
            assert_eq!(set.rules.len(), 1);
            assert!(!set.rules[0].enabled);
            assert!(!set.rules[0].imported_enabled);
            assert!(set.rules[0].imported_author_enabled);
        }
        other => panic!("unexpected document: {other:?}"),
    }
    assert_eq!(prepared.transformations.len(), 3);
    assert!(
        prepared
            .transformations
            .iter()
            .all(|transformation| transformation.before && !transformation.after)
    );
}

#[test]
fn preparation_preserves_interaction_author_intent_while_forcing_rules_inactive() {
    let interactions = serde_json::to_vec(&json!({
        "id": "synthetic-interactions",
        "name": "Synthetic interactions",
        "schema_version": 1,
        "rules": [{
            "id": "synthetic-interaction-rule",
            "name": "Synthetic interaction rule",
            "enabled": true,
            "event": {"kind": "conversation_opened"},
            "condition": null,
            "actions": [],
            "priority": 0,
            "stop_after_match": false,
            "provenance": provenance()
        }],
        "max_actions_per_event": 8,
        "provenance": provenance()
    }))
    .expect("encode interactions");
    let fixture = write_package(
        vec![(
            "interactions/rules.json".into(),
            interactions,
            Some("application/json"),
        )],
        vec![json!({
            "id": "interactions",
            "path": "interactions/rules.json",
            "kind": "interaction"
        })],
        |_| {},
    );
    let inspection =
        inspect_content_package(fixture.path(), ImportLimits::default()).expect("inspection");
    let selection = select_content_package_components(&inspection, &["interactions".into()])
        .expect("selection");
    let prepared =
        prepare_content_package_import(fixture.path(), ImportLimits::default(), &selection)
            .expect("prepare");

    match &prepared.documents[0].document {
        PreparedContentDocument::InteractionRuleSet(set) => {
            assert_eq!(set.rules.len(), 1);
            assert!(!set.rules[0].enabled);
            assert!(set.rules[0].imported_author_enabled);
        }
        other => panic!("unexpected document: {other:?}"),
    }
    assert_eq!(prepared.transformations.len(), 1);
    assert!(prepared.transformations[0].before);
    assert!(!prepared.transformations[0].after);
}

#[test]
fn array_component_preserves_contiguous_source_bound_document_ordinals() {
    let transforms = serde_json::to_vec(&json!([
        {
            "id": "synthetic-transform-a",
            "name": "Synthetic A",
            "schema_version": 1,
            "enabled": true,
            "rules": [],
            "max_rules_per_phase": 8,
            "max_output_chars": 4096,
            "provenance": provenance()
        },
        {
            "id": "synthetic-transform-b",
            "name": "Synthetic B",
            "schema_version": 1,
            "enabled": true,
            "rules": [],
            "max_rules_per_phase": 8,
            "max_output_chars": 4096,
            "provenance": provenance()
        }
    ]))
    .expect("encode transform array");
    let fixture = write_package(
        vec![(
            "transforms/sets.json".into(),
            transforms,
            Some("application/json"),
        )],
        vec![json!({
            "id": "transform-array",
            "path": "transforms/sets.json",
            "kind": "transform"
        })],
        |_| {},
    );
    let inspection =
        inspect_content_package(fixture.path(), ImportLimits::default()).expect("inspection");
    let source_ordinal = inspection
        .components
        .iter()
        .position(|component| component.id == "transform-array")
        .and_then(|ordinal| u32::try_from(ordinal).ok())
        .expect("component ordinal");
    let selection = select_content_package_components(&inspection, &["transform-array".into()])
        .expect("selection");
    let prepared =
        prepare_content_package_import(fixture.path(), ImportLimits::default(), &selection)
            .expect("prepare array");

    assert_eq!(prepared.documents.len(), 2);
    assert_eq!(
        prepared
            .documents
            .iter()
            .map(|envelope| (
                envelope.source_component_id.as_str(),
                envelope.source_component_ordinal,
                envelope.document_ordinal,
                envelope.document_id.as_str(),
            ))
            .collect::<Vec<_>>(),
        vec![
            (
                "transform-array",
                source_ordinal,
                0,
                "synthetic-transform-a"
            ),
            (
                "transform-array",
                source_ordinal,
                1,
                "synthetic-transform-b"
            ),
        ]
    );
}

#[test]
fn duplicate_document_identity_in_one_array_is_rejected() {
    let transform = json!({
        "id": "synthetic-transform-duplicate",
        "name": "Synthetic",
        "schema_version": 1,
        "enabled": true,
        "rules": [],
        "max_rules_per_phase": 8,
        "max_output_chars": 4096,
        "provenance": provenance()
    });
    let transforms =
        serde_json::to_vec(&json!([transform.clone(), transform])).expect("encode duplicates");
    let fixture = write_package(
        vec![(
            "transforms/duplicates.json".into(),
            transforms,
            Some("application/json"),
        )],
        vec![json!({
            "id": "transform-duplicates",
            "path": "transforms/duplicates.json",
            "kind": "transform"
        })],
        |_| {},
    );
    let inspection =
        inspect_content_package(fixture.path(), ImportLimits::default()).expect("inspection");
    let selection =
        select_content_package_components(&inspection, &["transform-duplicates".into()])
            .expect("selection");
    prepare_content_package_import(fixture.path(), ImportLimits::default(), &selection)
        .expect_err("duplicate object identity must fail before storage");
}

#[test]
fn executable_html_and_external_urls_are_quarantined_and_inactive() {
    let fixture = write_package(
        vec![
            (
                "prompt/safe.json".into(),
                br#"{"text":"safe"}"#.to_vec(),
                Some("application/json"),
            ),
            (
                "transforms/external.json".into(),
                br#"{"asset_url":"https://invalid.example/tracker.png"}"#.to_vec(),
                Some("application/json"),
            ),
            (
                "scripts/run.js".into(),
                b"globalThis.compromised = true".to_vec(),
                Some("application/javascript"),
            ),
            (
                "assets/sha256/not-a-real-hash.html".into(),
                b"<script>alert(1)</script>".to_vec(),
                Some("text/html"),
            ),
        ],
        Vec::new(),
        |_| {},
    );
    let inspection =
        inspect_content_package(fixture.path(), ImportLimits::default()).expect("inspection");
    let by_path = inspection
        .components
        .iter()
        .map(|component| (component.path.as_str(), component))
        .collect::<BTreeMap<_, _>>();

    assert_eq!(
        by_path["prompt/safe.json"].state,
        ContentPackageComponentState::Selectable
    );
    for path in [
        "transforms/external.json",
        "scripts/run.js",
        "assets/sha256/not-a-real-hash.html",
    ] {
        assert_eq!(
            by_path[path].state,
            ContentPackageComponentState::Quarantined,
            "{path}"
        );
        assert!(
            select_content_package_components(&inspection, &[by_path[path].id.clone()]).is_err()
        );
    }
}

#[test]
fn detects_hash_and_mime_mismatches_without_materializing_components() {
    let png = b"\x89PNG\r\n\x1a\nsynthetic".to_vec();
    let digest = sha256(&png);
    let path = format!("assets/sha256/{digest}.png");
    let fixture = write_package(
        vec![(path.clone(), png, Some("image/jpeg"))],
        Vec::new(),
        |manifest| {
            manifest["content_hashes"][&path] = Value::String("00".repeat(32));
        },
    );
    let inspection =
        inspect_content_package(fixture.path(), ImportLimits::default()).expect("review");
    assert!(!inspection.is_allowed());
    assert_eq!(
        inspection.components[0].state,
        ContentPackageComponentState::Quarantined
    );
    assert!(
        inspection
            .blocked_reasons
            .iter()
            .any(|reason| reason.contains("hash mismatch"))
    );
    assert!(
        inspection
            .warnings
            .iter()
            .any(|warning| warning.code == "mime_mismatch")
    );
}

#[test]
fn streams_and_indexes_two_thousand_content_addressed_assets() {
    let mut entries = Vec::with_capacity(2_000);
    for index in 0..2_000_u32 {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&index.to_le_bytes());
        let digest = sha256(&bytes);
        entries.push((
            format!("assets/sha256/{digest}.png"),
            bytes,
            Some("image/png"),
        ));
    }
    let fixture = write_package(entries, Vec::new(), |_| {});
    let inspection =
        inspect_content_package(fixture.path(), ImportLimits::default()).expect("large inspection");

    assert_eq!(inspection.components.len(), 2_000);
    assert!(
        inspection
            .components
            .iter()
            .all(lorepia_content::ContentPackageComponent::is_selectable)
    );
    assert!(inspection.total_uncompressed_size < 4 * 1024 * 1024);
}

#[test]
fn stages_only_selected_assets_with_exact_reviewed_bytes_and_metadata() {
    let mut selected_bytes = b"\x89PNG\r\n\x1a\nselected".to_vec();
    selected_bytes.extend_from_slice(&[1, 2, 3, 4]);
    let selected_digest = sha256(&selected_bytes);
    let selected_path = format!("assets/sha256/{selected_digest}.png");
    let skipped_bytes = b"\x89PNG\r\n\x1a\nskipped".to_vec();
    let skipped_digest = sha256(&skipped_bytes);
    let skipped_path = format!("assets/sha256/{skipped_digest}.png");
    let fixture = write_package(
        vec![
            (
                selected_path.clone(),
                selected_bytes.clone(),
                Some("image/png"),
            ),
            (skipped_path, skipped_bytes, Some("image/png")),
        ],
        Vec::new(),
        |_| {},
    );
    let inspection =
        inspect_content_package(fixture.path(), ImportLimits::default()).expect("inspection");
    let selected_id = inspection
        .components
        .iter()
        .find(|component| component.path == selected_path)
        .expect("selected asset component")
        .id
        .clone();
    let selection =
        select_content_package_components(&inspection, std::slice::from_ref(&selected_id))
            .expect("asset selection");
    let staging = tempdir().expect("asset staging");
    let staged = stage_selected_content_package_assets(
        fixture.path(),
        ImportLimits::default(),
        &selection,
        staging.path(),
    )
    .expect("stage selected asset");

    assert_eq!(staged.len(), 1);
    assert_eq!(staged[0].component_id, selected_id);
    assert_eq!(staged[0].descriptor.sha256.as_str(), selected_digest);
    assert_eq!(
        staged[0].descriptor.source.logical_path.as_deref(),
        Some(selected_path.as_str())
    );
    assert_eq!(
        fs::read(&staged[0].staged_path).expect("read staged asset"),
        selected_bytes
    );
    assert_eq!(
        fs::read_dir(staging.path()).expect("read staging").count(),
        1,
        "unselected asset must never be materialized"
    );
    let prepared =
        prepare_content_package_import(fixture.path(), ImportLimits::default(), &selection)
            .expect("prepare selected asset");
    assert_eq!(prepared.assets, [staged[0].descriptor.clone()]);

    discard_staged_content_package_assets(&staged, staging.path()).expect("discard staged asset");
    assert!(
        fs::read_dir(staging.path())
            .expect("read cleaned staging")
            .next()
            .is_none()
    );
}

#[test]
fn changed_package_never_leaves_selected_asset_staging_files() {
    let bytes = b"\x89PNG\r\n\x1a\nreviewed".to_vec();
    let digest = sha256(&bytes);
    let path = format!("assets/sha256/{digest}.png");
    let fixture = write_package(vec![(path, bytes, Some("image/png"))], Vec::new(), |_| {});
    let inspection =
        inspect_content_package(fixture.path(), ImportLimits::default()).expect("inspection");
    let selection =
        select_content_package_components(&inspection, &[inspection.components[0].id.clone()])
            .expect("selection");
    fs::write(fixture.path(), b"changed after review").expect("tamper package");
    let staging = tempdir().expect("asset staging");
    stage_selected_content_package_assets(
        fixture.path(),
        ImportLimits::default(),
        &selection,
        staging.path(),
    )
    .expect_err("tampered package must not stage assets");
    assert!(
        fs::read_dir(staging.path())
            .expect("read staging")
            .next()
            .is_none()
    );
}

#[cfg(unix)]
#[test]
fn selected_assets_reject_a_symlink_staging_directory() {
    use std::os::unix::fs::symlink;

    let bytes = b"\x89PNG\r\n\x1a\nreviewed".to_vec();
    let digest = sha256(&bytes);
    let path = format!("assets/sha256/{digest}.png");
    let fixture = write_package(vec![(path, bytes, Some("image/png"))], Vec::new(), |_| {});
    let inspection =
        inspect_content_package(fixture.path(), ImportLimits::default()).expect("inspection");
    let selection =
        select_content_package_components(&inspection, &[inspection.components[0].id.clone()])
            .expect("selection");
    let staging_root = tempdir().expect("staging root");
    let actual = staging_root.path().join("actual");
    fs::create_dir(&actual).expect("actual staging directory");
    let linked = staging_root.path().join("linked");
    symlink(&actual, &linked).expect("staging symlink");

    stage_selected_content_package_assets(
        fixture.path(),
        ImportLimits::default(),
        &selection,
        &linked,
    )
    .expect_err("symlink staging directory must fail closed");
    assert!(
        fs::read_dir(&actual)
            .expect("read actual directory")
            .next()
            .is_none()
    );
}

#[test]
fn unknown_license_is_local_use_only_even_if_manifest_requests_redistribution() {
    let fixture = write_package(
        vec![(
            "prompt/preset.json".into(),
            br#"{"text":"safe"}"#.to_vec(),
            Some("application/json"),
        )],
        Vec::new(),
        |manifest| {
            manifest["license"] = Value::String("LicenseRef-Unknown".into());
            manifest["redistribution_allowed"] = Value::Bool(true);
        },
    );
    let inspection =
        inspect_content_package(fixture.path(), ImportLimits::default()).expect("inspection");
    assert!(inspection.local_use_only);
    assert!(!inspection.manifest.can_redistribute());
}
