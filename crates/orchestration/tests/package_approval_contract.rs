use chrono::{TimeZone, Utc};
use lorepia_domain::{
    AssetDescriptor, AssetId, AssetRole, AssetSource, AssetSourceKind, ContentCapability,
    PackageContentHash, PackageId, PackageManifest, Provenance, Sha256Digest, SourceKind,
};
use lorepia_orchestration::{
    AssetImportDisposition, ImportPlanState, LOREPIA_PACKAGE_FORMAT,
    LOREPIA_PACKAGE_FORMAT_VERSION, ObservedPackageEntry, PackageComponentDescriptor,
    PackageComponentDisposition, PackageComponentKind, PackageImportApproval,
    PackageInspectionSnapshot, PackageSelectionRequest, PackageValidationError,
    PackageValidationPolicy, RedistributionStatus, SignatureVerification,
    approve_selective_import_plan, build_selective_import_plan, validate_package_snapshot,
};

fn digest(byte: &str) -> Sha256Digest {
    Sha256Digest::parse(byte.repeat(32)).expect("synthetic digest")
}

fn imported_provenance(source: &Sha256Digest) -> Provenance {
    Provenance {
        source_kind: SourceKind::ImportedPackage,
        source_id: Some("synthetic.package".to_owned()),
        source_hash: Some(source.as_str().to_owned()),
        author: Some("Synthetic Author".to_owned()),
        license: Some("LicenseRef-Unknown".to_owned()),
        imported_at: Some(
            Utc.with_ymd_and_hms(2026, 8, 3, 12, 0, 0)
                .single()
                .expect("valid timestamp"),
        ),
    }
}

fn package_snapshot() -> PackageInspectionSnapshot {
    let source = digest("aa");
    let component_hash = digest("11");
    let html_hash = digest("22");
    let component_path = "transforms/synthetic.json".to_owned();
    let html_path = format!("assets/sha256/{html_hash}");
    let content_hashes = vec![
        PackageContentHash {
            logical_path: component_path.clone(),
            sha256: component_hash.clone(),
            size_bytes: 256,
        },
        PackageContentHash {
            logical_path: html_path.clone(),
            sha256: html_hash.clone(),
            size_bytes: 128,
        },
    ];

    PackageInspectionSnapshot {
        source_sha256: source.clone(),
        source_size_bytes: 1_024,
        manifest: PackageManifest {
            format: LOREPIA_PACKAGE_FORMAT.to_owned(),
            format_version: LOREPIA_PACKAGE_FORMAT_VERSION,
            package_id: PackageId::from("synthetic.package"),
            name: "Synthetic package".to_owned(),
            version: "1.0.0".to_owned(),
            author: Some("Synthetic Author".to_owned()),
            license: "LicenseRef-Unknown".to_owned(),
            redistribution_allowed: true,
            required_app_version: Some("0.1.0".to_owned()),
            required_capabilities: vec![ContentCapability::Transforms],
            content_hashes,
            signature: None,
            provenance: imported_provenance(&source),
        },
        signature_verification: SignatureVerification::Absent,
        components: vec![
            PackageComponentDescriptor {
                id: "synthetic.transform-set".to_owned(),
                kind: PackageComponentKind::TransformSet,
                logical_path: component_path.clone(),
                sha256: component_hash.clone(),
                dependencies: Vec::new(),
                conflicts_with: Vec::new(),
                required_capabilities: vec![ContentCapability::Transforms],
                asset_ids: Vec::new(),
                disposition: PackageComponentDisposition::Importable,
            },
            PackageComponentDescriptor {
                id: "synthetic.raw-html".to_owned(),
                kind: PackageComponentKind::RawExtension,
                logical_path: html_path.clone(),
                sha256: html_hash.clone(),
                dependencies: Vec::new(),
                conflicts_with: Vec::new(),
                required_capabilities: Vec::new(),
                asset_ids: vec![AssetId::from("synthetic.html")],
                disposition: PackageComponentDisposition::Quarantined,
            },
        ],
        assets: vec![AssetDescriptor {
            id: AssetId::from("synthetic.html"),
            sha256: html_hash.clone(),
            media_type: "text/html".to_owned(),
            role: AssetRole::Attachment,
            name: "synthetic.html".to_owned(),
            size_bytes: 128,
            width: None,
            height: None,
            duration_ms: None,
            source: AssetSource {
                kind: AssetSourceKind::LorepiaPackage,
                source_sha256: Some(source.clone()),
                logical_path: Some(html_path.clone()),
            },
        }],
        observed_entries: vec![
            ObservedPackageEntry {
                logical_path: component_path,
                sha256: component_hash,
                size_bytes: 256,
            },
            ObservedPackageEntry {
                logical_path: html_path,
                sha256: html_hash,
                size_bytes: 128,
            },
            ObservedPackageEntry {
                logical_path: "manifest.json".to_owned(),
                sha256: digest("33"),
                size_bytes: 512,
            },
        ],
    }
}

#[test]
fn imported_components_remain_disabled_until_hash_bound_approval() {
    let review =
        validate_package_snapshot(&package_snapshot(), &PackageValidationPolicy::default())
            .expect("package review");
    review.verify().expect("review hash");

    assert!(review.local_import_allowed);
    assert_eq!(
        review.redistribution_status,
        RedistributionStatus::LicenseUnclear,
        "unclear licensing must never silently become shareable"
    );
    assert_eq!(
        review
            .assets
            .iter()
            .find(|asset| asset.descriptor.id.as_str() == "synthetic.html")
            .expect("HTML asset review")
            .disposition,
        AssetImportDisposition::Quarantined
    );
    assert_eq!(
        review
            .components
            .iter()
            .find(|component| component.id == "synthetic.raw-html")
            .expect("raw HTML component")
            .disposition,
        PackageComponentDisposition::Quarantined
    );

    let plan = build_selective_import_plan(
        &review,
        &PackageSelectionRequest {
            expected_review_sha256: review.review_sha256.clone(),
            component_ids: vec!["synthetic.transform-set".to_owned()],
            standalone_asset_ids: Vec::new(),
        },
    )
    .expect("selective import plan");

    assert_eq!(plan.state, ImportPlanState::AwaitingApproval);
    assert!(plan.components.iter().all(|component| !component.enabled));

    let stale = approve_selective_import_plan(
        &plan,
        &PackageImportApproval {
            approval_id: "synthetic.approval".to_owned(),
            expected_review_sha256: plan.review_sha256.clone(),
            expected_plan_sha256: digest("ff"),
            target_review_sha256: digest("aa"),
            update_target_confirmations_sha256: digest("bb"),
            enable_component_ids: vec!["synthetic.transform-set".to_owned()],
        },
    );
    assert_eq!(stale, Err(PackageValidationError::StalePlan));

    let approved = approve_selective_import_plan(
        &plan,
        &PackageImportApproval {
            approval_id: "synthetic.approval".to_owned(),
            expected_review_sha256: plan.review_sha256.clone(),
            expected_plan_sha256: plan.plan_sha256.clone(),
            target_review_sha256: digest("aa"),
            update_target_confirmations_sha256: digest("bb"),
            enable_component_ids: vec!["synthetic.transform-set".to_owned()],
        },
    )
    .expect("exact approval");
    assert_eq!(approved.state, ImportPlanState::Approved);
    assert_eq!(approved.components.len(), 1);
    assert!(approved.components[0].enabled);
    assert_eq!(
        approved.redistribution_status,
        RedistributionStatus::LicenseUnclear
    );
}

#[test]
fn quarantined_html_cannot_be_selected_as_an_asset_or_component() {
    let review =
        validate_package_snapshot(&package_snapshot(), &PackageValidationPolicy::default())
            .expect("package review");

    let component_error = build_selective_import_plan(
        &review,
        &PackageSelectionRequest {
            expected_review_sha256: review.review_sha256.clone(),
            component_ids: vec!["synthetic.raw-html".to_owned()],
            standalone_asset_ids: Vec::new(),
        },
    )
    .expect_err("quarantined component");
    assert_eq!(
        component_error,
        PackageValidationError::ComponentNotImportable("synthetic.raw-html".to_owned())
    );

    let asset_error = build_selective_import_plan(
        &review,
        &PackageSelectionRequest {
            expected_review_sha256: review.review_sha256.clone(),
            component_ids: Vec::new(),
            standalone_asset_ids: vec![AssetId::from("synthetic.html")],
        },
    )
    .expect_err("quarantined asset");
    assert_eq!(
        asset_error,
        PackageValidationError::AssetNotImportable("synthetic.html".to_owned())
    );
}
