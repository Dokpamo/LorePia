//! Pure validation and approval planning for already-inspected `LorePia` packages.
//!
//! Archive parsing, path extraction, MIME sniffing, and byte hashing belong to
//! `lorepia-content`. This module accepts only a bounded inspection snapshot and
//! turns it into a deterministic, review-hash-bound import plan. No package
//! component is enabled without a separate explicit approval.

use std::collections::{BTreeMap, BTreeSet};

use lorepia_domain::{
    AssetDescriptor, AssetId, AssetSourceKind, ContentCapability, PackageId, PackageManifest,
    Sha256Digest, SourceKind,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const LOREPIA_PACKAGE_FORMAT: &str = "lorepia_content_package";
pub const LOREPIA_PACKAGE_FORMAT_VERSION: u32 = 1;
pub const MAX_PACKAGE_COMPONENTS: usize = 4_096;
pub const MAX_PACKAGE_ASSETS: usize = 16_384;
pub const MAX_PACKAGE_CONTENT_HASHES: usize = 32_768;
pub const MAX_PACKAGE_DEPENDENCIES: usize = 16_384;
pub const MAX_PACKAGE_SOURCE_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_PACKAGE_TOTAL_OBSERVED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const MAX_PACKAGE_TOTAL_ASSET_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_LABEL_BYTES: usize = 1_024;
const MAX_PATH_BYTES: usize = 2_048;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignatureVerification {
    Absent,
    Verified,
    Invalid,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageComponentKind {
    PromptPreset,
    MemoryProfile,
    KnowledgeBook,
    TransformSet,
    InteractionRuleSet,
    ContentModule,
    AssetIndex,
    RawExtension,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageComponentDisposition {
    Importable,
    Unsupported,
    Quarantined,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageComponentDescriptor {
    pub id: String,
    pub kind: PackageComponentKind,
    pub logical_path: String,
    pub sha256: Sha256Digest,
    pub dependencies: Vec<String>,
    pub conflicts_with: Vec<String>,
    pub required_capabilities: Vec<ContentCapability>,
    pub asset_ids: Vec<AssetId>,
    pub disposition: PackageComponentDisposition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservedPackageEntry {
    pub logical_path: String,
    pub sha256: Sha256Digest,
    pub size_bytes: u64,
}

/// Immutable output of the hostile-input inspection layer.
///
/// The source and every entry digest must have been computed from bytes by the
/// content crate. This type intentionally contains no filesystem path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageInspectionSnapshot {
    pub source_sha256: Sha256Digest,
    pub source_size_bytes: u64,
    pub manifest: PackageManifest,
    pub signature_verification: SignatureVerification,
    pub components: Vec<PackageComponentDescriptor>,
    pub assets: Vec<AssetDescriptor>,
    pub observed_entries: Vec<ObservedPackageEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageValidationPolicy {
    pub supported_capabilities: Vec<ContentCapability>,
    pub redistributable_licenses: Vec<String>,
    pub allow_unsigned_packages: bool,
    pub max_components: usize,
    pub max_assets: usize,
    pub max_content_hashes: usize,
    pub max_dependencies: usize,
    pub max_source_bytes: u64,
    pub max_total_observed_bytes: u64,
    pub max_total_asset_bytes: u64,
    pub current_app_version: String,
}

impl Default for PackageValidationPolicy {
    fn default() -> Self {
        Self {
            supported_capabilities: vec![
                ContentCapability::PromptFragments,
                ContentCapability::Knowledge,
                ContentCapability::Variables,
                ContentCapability::Transforms,
                ContentCapability::DeclarativeInteractions,
                ContentCapability::ImageAssets,
                ContentCapability::AudioAssets,
                ContentCapability::VideoAssets,
                ContentCapability::AttachmentAssets,
            ],
            redistributable_licenses: vec![
                "Apache-2.0".to_owned(),
                "BSD-2-Clause".to_owned(),
                "BSD-3-Clause".to_owned(),
                "CC-BY-4.0".to_owned(),
                "CC0-1.0".to_owned(),
                "ISC".to_owned(),
                "MIT".to_owned(),
                "Zlib".to_owned(),
            ],
            allow_unsigned_packages: true,
            max_components: MAX_PACKAGE_COMPONENTS,
            max_assets: MAX_PACKAGE_ASSETS,
            max_content_hashes: MAX_PACKAGE_CONTENT_HASHES,
            max_dependencies: MAX_PACKAGE_DEPENDENCIES,
            max_source_bytes: MAX_PACKAGE_SOURCE_BYTES,
            max_total_observed_bytes: MAX_PACKAGE_TOTAL_OBSERVED_BYTES,
            max_total_asset_bytes: MAX_PACKAGE_TOTAL_ASSET_BYTES,
            current_app_version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageIssueSeverity {
    Warning,
    Blocker,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageValidationIssue {
    pub severity: PackageIssueSeverity,
    pub code: String,
    pub target: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetImportDisposition {
    Importable,
    Quarantined,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewedPackageAsset {
    pub descriptor: AssetDescriptor,
    pub disposition: AssetImportDisposition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedistributionStatus {
    Allowed,
    DeniedByManifest,
    LicenseUnclear,
    ProvenanceIncomplete,
    ValidationBlocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageReview {
    pub review_sha256: Sha256Digest,
    pub source_sha256: Sha256Digest,
    pub manifest: PackageManifest,
    pub components: Vec<PackageComponentDescriptor>,
    pub assets: Vec<ReviewedPackageAsset>,
    pub issues: Vec<PackageValidationIssue>,
    pub local_import_allowed: bool,
    pub redistribution_status: RedistributionStatus,
}

impl PackageReview {
    pub fn verify(&self) -> Result<(), PackageValidationError> {
        let expected = package_review_sha256(
            &self.source_sha256,
            &self.manifest,
            &self.components,
            &self.assets,
            &self.issues,
            self.local_import_allowed,
            self.redistribution_status,
        )?;
        if expected != self.review_sha256 {
            return Err(PackageValidationError::ReviewHashMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PackageValidationError {
    #[error("package review could not be encoded deterministically: {0}")]
    CanonicalEncoding(String),
    #[error("the package review hash does not match its canonical payload")]
    ReviewHashMismatch,
    #[error("the selection references a stale package review")]
    StaleReview,
    #[error("package import is blocked by validation issues")]
    ImportBlocked,
    #[error("at least one component or asset must be selected")]
    EmptySelection,
    #[error("selection contains a duplicate identifier: {0}")]
    DuplicateSelection(String),
    #[error("selection references an unknown component: {0}")]
    UnknownComponent(String),
    #[error("selection references an unknown asset: {0}")]
    UnknownAsset(String),
    #[error("component cannot be imported: {0}")]
    ComponentNotImportable(String),
    #[error("asset cannot be imported: {0}")]
    AssetNotImportable(String),
    #[error("selection requires explicit dependency confirmation: {0:?}")]
    DependencyConfirmationRequired(Vec<String>),
    #[error("selected components conflict: {0} and {1}")]
    SelectedComponentsConflict(String, String),
    #[error("the import plan hash does not match its canonical payload")]
    PlanHashMismatch,
    #[error("the import plan is in an invalid lifecycle state")]
    InvalidPlanState,
    #[error("the import plan capability set does not match its selected components")]
    InvalidPlanCapabilities,
    #[error("the approval references a stale import plan")]
    StalePlan,
    #[error("approval contains an unknown selected component: {0}")]
    UnknownApprovedComponent(String),
    #[error("approval contains a duplicate component decision: {0}")]
    DuplicateApprovalDecision(String),
    #[error("approval identifier must be non-empty, bounded, and free of control characters")]
    MissingApprovalId,
    #[error("approved package import hash does not match its canonical payload")]
    ApprovalHashMismatch,
}

/// Validate a bounded snapshot without opening, extracting, or executing it.
pub fn validate_package_snapshot(
    snapshot: &PackageInspectionSnapshot,
    policy: &PackageValidationPolicy,
) -> Result<PackageReview, PackageValidationError> {
    let mut canonical = snapshot.clone();
    canonicalize_snapshot(&mut canonical);
    let mut issues = Vec::new();

    validate_manifest(&canonical, policy, &mut issues);
    validate_observed_hashes(&canonical, policy, &mut issues);
    validate_components(&canonical, policy, &mut issues);
    let assets = validate_assets(&canonical, policy, &mut issues);
    validate_provenance(&canonical, &mut issues);
    validate_signature(&canonical, policy, &mut issues);

    issues.sort();
    issues.dedup();
    let local_import_allowed = !issues
        .iter()
        .any(|issue| issue.severity == PackageIssueSeverity::Blocker);
    let redistribution_status = redistribution_status(&canonical, policy, &issues);
    let review_sha256 = package_review_sha256(
        &canonical.source_sha256,
        &canonical.manifest,
        &canonical.components,
        &assets,
        &issues,
        local_import_allowed,
        redistribution_status,
    )?;

    Ok(PackageReview {
        review_sha256,
        source_sha256: canonical.source_sha256,
        manifest: canonical.manifest,
        components: canonical.components,
        assets,
        issues,
        local_import_allowed,
        redistribution_status,
    })
}

fn canonicalize_snapshot(snapshot: &mut PackageInspectionSnapshot) {
    snapshot
        .manifest
        .required_capabilities
        .sort_by_key(|capability| capability_rank(*capability));
    snapshot.manifest.required_capabilities.dedup();
    snapshot.manifest.content_hashes.sort_by(|left, right| {
        left.logical_path
            .cmp(&right.logical_path)
            .then_with(|| left.sha256.cmp(&right.sha256))
    });
    snapshot.components.iter_mut().for_each(|component| {
        component.dependencies.sort();
        component.dependencies.dedup();
        component.conflicts_with.sort();
        component.conflicts_with.dedup();
        component
            .required_capabilities
            .sort_by_key(|capability| capability_rank(*capability));
        component.required_capabilities.dedup();
        component.asset_ids.sort();
        component.asset_ids.dedup();
    });
    snapshot.components.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.id.cmp(&right.id))
    });
    snapshot
        .assets
        .sort_by(|left, right| left.id.cmp(&right.id));
    snapshot.observed_entries.sort_by(|left, right| {
        left.logical_path
            .cmp(&right.logical_path)
            .then_with(|| left.sha256.cmp(&right.sha256))
    });
}

#[allow(clippy::too_many_lines)] // Manifest validation reports every independent review issue.
fn validate_manifest(
    snapshot: &PackageInspectionSnapshot,
    policy: &PackageValidationPolicy,
    issues: &mut Vec<PackageValidationIssue>,
) {
    if snapshot.source_size_bytes == 0 || snapshot.source_size_bytes > policy.max_source_bytes {
        blocker(
            issues,
            "package.invalid_source_size",
            "source_size_bytes",
            "source byte size must be positive and within the configured limit",
        );
    }
    if snapshot.manifest.format != LOREPIA_PACKAGE_FORMAT {
        blocker(
            issues,
            "package.format",
            "manifest.format",
            "manifest format is not LorePia's package format",
        );
    }
    if snapshot.manifest.format_version != LOREPIA_PACKAGE_FORMAT_VERSION {
        blocker(
            issues,
            "package.format_version",
            "manifest.format_version",
            "manifest format version is unsupported",
        );
    }
    validate_label(
        &snapshot.manifest.package_id.0,
        "manifest.package_id",
        issues,
    );
    validate_label(&snapshot.manifest.name, "manifest.name", issues);
    validate_label(&snapshot.manifest.version, "manifest.version", issues);
    validate_label(&snapshot.manifest.license, "manifest.license", issues);
    if let Some(author) = &snapshot.manifest.author {
        validate_label(author, "manifest.author", issues);
    } else {
        warning(
            issues,
            "package.author_missing",
            "manifest.author",
            "package author is not declared",
        );
    }
    if let Some(version) = &snapshot.manifest.required_app_version {
        validate_label(version, "manifest.required_app_version", issues);
        match (
            parse_version_triplet(version),
            parse_version_triplet(&policy.current_app_version),
        ) {
            (Some(required), Some(current)) if required > current => blocker(
                issues,
                "package.app_version_too_old",
                "manifest.required_app_version",
                "package requires a newer LorePia application version",
            ),
            (Some(_), Some(_)) => {}
            _ => blocker(
                issues,
                "package.invalid_app_version",
                "manifest.required_app_version",
                "application versions must use major.minor.patch numeric form",
            ),
        }
    }

    if snapshot.manifest.content_hashes.len() > policy.max_content_hashes {
        blocker(
            issues,
            "package.too_many_hashes",
            "manifest.content_hashes",
            "content hash count exceeds the configured limit",
        );
    }
    let mut previous_path: Option<&str> = None;
    for entry in &snapshot.manifest.content_hashes {
        validate_logical_path(&entry.logical_path, "manifest.content_hashes", issues);
        if entry.size_bytes == 0 {
            blocker(
                issues,
                "package.empty_payload",
                &entry.logical_path,
                "manifest payload size must be positive",
            );
        }
        if entry.logical_path == "manifest.json" {
            blocker(
                issues,
                "package.recursive_manifest_hash",
                "manifest.content_hashes",
                "manifest.json cannot hash itself",
            );
        }
        if previous_path == Some(entry.logical_path.as_str()) {
            blocker(
                issues,
                "package.duplicate_hash_path",
                &entry.logical_path,
                "content hash paths must be unique",
            );
        }
        previous_path = Some(&entry.logical_path);
    }

    for capability in &snapshot.manifest.required_capabilities {
        if !policy.supported_capabilities.contains(capability) {
            blocker(
                issues,
                "package.unsupported_capability",
                &format!("capability:{capability:?}"),
                "package requires a capability unavailable in this application",
            );
        }
    }
}

fn validate_observed_hashes(
    snapshot: &PackageInspectionSnapshot,
    policy: &PackageValidationPolicy,
    issues: &mut Vec<PackageValidationIssue>,
) {
    if snapshot.observed_entries.len() > policy.max_content_hashes {
        blocker(
            issues,
            "package.too_many_entries",
            "observed_entries",
            "observed entry count exceeds the configured limit",
        );
    }

    let mut observed = BTreeMap::new();
    let mut total_observed_bytes = 0_u64;
    for entry in &snapshot.observed_entries {
        total_observed_bytes =
            if let Some(total) = total_observed_bytes.checked_add(entry.size_bytes) {
                total
            } else {
                blocker(
                    issues,
                    "package.observed_size_overflow",
                    "observed_entries",
                    "observed entry byte total overflowed",
                );
                u64::MAX
            };
        validate_logical_path(&entry.logical_path, "observed_entries", issues);
        if entry.logical_path == "manifest.json" {
            continue;
        }
        if observed
            .insert(
                entry.logical_path.clone(),
                (entry.sha256.clone(), entry.size_bytes),
            )
            .is_some()
        {
            blocker(
                issues,
                "package.duplicate_observed_path",
                &entry.logical_path,
                "the inspection contains duplicate normalized paths",
            );
        }
    }
    if total_observed_bytes > policy.max_total_observed_bytes {
        blocker(
            issues,
            "package.observed_payload_too_large",
            "observed_entries",
            "observed payload bytes exceed the configured limit",
        );
    }

    let manifest = snapshot
        .manifest
        .content_hashes
        .iter()
        .map(|entry| {
            (
                entry.logical_path.clone(),
                (entry.sha256.clone(), entry.size_bytes),
            )
        })
        .collect::<BTreeMap<_, _>>();
    for (path, (expected_hash, expected_size)) in &manifest {
        match observed.get(path) {
            None => blocker(
                issues,
                "package.hash_missing_payload",
                path,
                "manifest hash has no observed payload",
            ),
            Some((actual_hash, _)) if actual_hash != expected_hash => blocker(
                issues,
                "package.hash_mismatch",
                path,
                "observed payload digest does not match manifest",
            ),
            Some((_, actual_size)) if actual_size != expected_size => blocker(
                issues,
                "package.size_mismatch",
                path,
                "observed payload size does not match manifest",
            ),
            Some(_) => {}
        }
    }
    for path in observed.keys() {
        if !manifest.contains_key(path) {
            blocker(
                issues,
                "package.unhashed_payload",
                path,
                "observed payload is not covered by the manifest",
            );
        }
    }
}

#[allow(clippy::too_many_lines)] // Components need one deterministic, issue-accumulating pass.
fn validate_components(
    snapshot: &PackageInspectionSnapshot,
    policy: &PackageValidationPolicy,
    issues: &mut Vec<PackageValidationIssue>,
) {
    if snapshot.components.len() > policy.max_components {
        blocker(
            issues,
            "package.too_many_components",
            "components",
            "component count exceeds the configured limit",
        );
    }
    let manifest_hashes = snapshot
        .manifest
        .content_hashes
        .iter()
        .map(|entry| (&entry.logical_path, &entry.sha256))
        .collect::<BTreeMap<_, _>>();
    let asset_ids = snapshot
        .assets
        .iter()
        .map(|asset| asset.id.clone())
        .collect::<BTreeSet<_>>();
    let component_ids = snapshot
        .components
        .iter()
        .map(|component| component.id.clone())
        .collect::<BTreeSet<_>>();
    if component_ids.len() != snapshot.components.len() {
        blocker(
            issues,
            "package.duplicate_component_id",
            "components",
            "component identifiers must be unique across component kinds",
        );
    }

    let mut dependency_count = 0_usize;
    for component in &snapshot.components {
        validate_label(&component.id, "component.id", issues);
        validate_logical_path(&component.logical_path, &component.id, issues);
        match manifest_hashes.get(&component.logical_path) {
            Some(hash) if *hash == &component.sha256 => {}
            Some(_) => blocker(
                issues,
                "package.component_hash_mismatch",
                &component.id,
                "component digest does not match the manifest",
            ),
            None => blocker(
                issues,
                "package.component_hash_missing",
                &component.id,
                "component path is not covered by the manifest",
            ),
        }

        dependency_count = dependency_count.saturating_add(component.dependencies.len());
        for dependency in &component.dependencies {
            if dependency == &component.id {
                blocker(
                    issues,
                    "package.self_dependency",
                    &component.id,
                    "component cannot depend on itself",
                );
            } else if !component_ids.contains(dependency) {
                blocker(
                    issues,
                    "package.unknown_dependency",
                    &component.id,
                    "component dependency does not exist",
                );
            }
        }
        for conflict in &component.conflicts_with {
            if conflict == &component.id {
                blocker(
                    issues,
                    "package.self_conflict",
                    &component.id,
                    "component cannot conflict with itself",
                );
            } else if !component_ids.contains(conflict) {
                blocker(
                    issues,
                    "package.unknown_conflict",
                    &component.id,
                    "component conflict target does not exist",
                );
            }
        }
        for capability in &component.required_capabilities {
            if !snapshot.manifest.required_capabilities.contains(capability) {
                blocker(
                    issues,
                    "package.undeclared_component_capability",
                    &component.id,
                    "component requires a capability not declared by the manifest",
                );
            }
        }
        for asset_id in &component.asset_ids {
            if !asset_ids.contains(asset_id) {
                blocker(
                    issues,
                    "package.unknown_component_asset",
                    &component.id,
                    "component references an asset that is absent",
                );
            }
        }
        if component.disposition != PackageComponentDisposition::Importable {
            warning(
                issues,
                "package.inert_component",
                &component.id,
                "unsupported or quarantined component will remain inert",
            );
        }
        if component.kind == PackageComponentKind::RawExtension
            && component.disposition == PackageComponentDisposition::Importable
        {
            blocker(
                issues,
                "package.raw_extension_not_quarantined",
                &component.id,
                "raw extension content cannot be marked importable",
            );
        }
    }
    if dependency_count > policy.max_dependencies {
        blocker(
            issues,
            "package.too_many_dependencies",
            "components",
            "dependency count exceeds the configured limit",
        );
    }
    detect_dependency_cycles(&snapshot.components, issues);
}

fn detect_dependency_cycles(
    components: &[PackageComponentDescriptor],
    issues: &mut Vec<PackageValidationIssue>,
) {
    let dependencies = components
        .iter()
        .map(|component| (component.id.as_str(), component.dependencies.as_slice()))
        .collect::<BTreeMap<_, _>>();
    let mut complete = BTreeSet::new();
    for component in components {
        let mut visiting = BTreeSet::new();
        if has_dependency_cycle(&component.id, &dependencies, &mut visiting, &mut complete) {
            blocker(
                issues,
                "package.dependency_cycle",
                &component.id,
                "component dependency graph contains a cycle",
            );
        }
    }
}

fn has_dependency_cycle<'a>(
    id: &'a str,
    dependencies: &BTreeMap<&'a str, &'a [String]>,
    visiting: &mut BTreeSet<&'a str>,
    complete: &mut BTreeSet<&'a str>,
) -> bool {
    if complete.contains(id) {
        return false;
    }
    if !visiting.insert(id) {
        return true;
    }
    if let Some(children) = dependencies.get(id) {
        for child in *children {
            if dependencies.contains_key(child.as_str())
                && has_dependency_cycle(child, dependencies, visiting, complete)
            {
                return true;
            }
        }
    }
    visiting.remove(id);
    complete.insert(id);
    false
}

#[allow(clippy::too_many_lines)] // Asset metadata, hashes, risk, and bounds are reviewed together.
fn validate_assets(
    snapshot: &PackageInspectionSnapshot,
    policy: &PackageValidationPolicy,
    issues: &mut Vec<PackageValidationIssue>,
) -> Vec<ReviewedPackageAsset> {
    if snapshot.assets.len() > policy.max_assets {
        blocker(
            issues,
            "package.too_many_assets",
            "assets",
            "asset count exceeds the configured limit",
        );
    }
    let manifest_hashes = snapshot
        .manifest
        .content_hashes
        .iter()
        .map(|entry| (&entry.logical_path, &entry.sha256))
        .collect::<BTreeMap<_, _>>();
    let mut seen_ids = BTreeSet::new();
    let mut total_size = 0_u64;
    let mut reviewed = Vec::with_capacity(snapshot.assets.len());

    for asset in &snapshot.assets {
        if !seen_ids.insert(asset.id.clone()) {
            blocker(
                issues,
                "package.duplicate_asset_id",
                asset.id.as_str(),
                "asset identifiers must be unique",
            );
        }
        validate_label(&asset.name, "asset.name", issues);
        validate_label(&asset.media_type, "asset.media_type", issues);
        total_size = if let Some(total) = total_size.checked_add(asset.size_bytes) {
            total
        } else {
            blocker(
                issues,
                "package.asset_size_overflow",
                asset.id.as_str(),
                "asset size total overflowed",
            );
            u64::MAX
        };
        if asset.width.is_some() != asset.height.is_some() {
            blocker(
                issues,
                "package.incomplete_asset_dimensions",
                asset.id.as_str(),
                "asset width and height must be present together",
            );
        }
        if asset.source.kind != AssetSourceKind::LorepiaPackage {
            blocker(
                issues,
                "package.asset_provenance_kind_mismatch",
                asset.id.as_str(),
                "package assets must identify LorePia package provenance",
            );
        }
        if asset.source.source_sha256.as_ref() != Some(&snapshot.source_sha256) {
            blocker(
                issues,
                "package.asset_provenance_hash_mismatch",
                asset.id.as_str(),
                "asset provenance must bind the inspected package source hash",
            );
        }

        let disposition = if is_high_risk_media_type(&asset.media_type) {
            warning(
                issues,
                "package.high_risk_asset",
                asset.id.as_str(),
                "code, HTML, SVG, font, or executable assets are quarantined",
            );
            AssetImportDisposition::Quarantined
        } else {
            AssetImportDisposition::Importable
        };

        match &asset.source.logical_path {
            Some(path) => {
                validate_logical_path(path, asset.id.as_str(), issues);
                if !asset_path_matches_digest_and_media_type(path, &asset.sha256, &asset.media_type)
                {
                    blocker(
                        issues,
                        "package.asset_content_address_mismatch",
                        asset.id.as_str(),
                        "asset path must contain its exact SHA-256 digest and only an optional media-type extension",
                    );
                }
                match manifest_hashes.get(path) {
                    Some(hash) if *hash == &asset.sha256 => {}
                    Some(_) => blocker(
                        issues,
                        "package.asset_hash_mismatch",
                        asset.id.as_str(),
                        "asset digest does not match the manifest",
                    ),
                    None => blocker(
                        issues,
                        "package.asset_hash_missing",
                        asset.id.as_str(),
                        "asset path is not covered by the manifest",
                    ),
                }
                if let Some(observed) = snapshot
                    .observed_entries
                    .iter()
                    .find(|entry| entry.logical_path == *path)
                    && observed.size_bytes != asset.size_bytes
                {
                    blocker(
                        issues,
                        "package.asset_size_mismatch",
                        asset.id.as_str(),
                        "asset byte size does not match the inspected payload",
                    );
                }
            }
            None => blocker(
                issues,
                "package.asset_path_missing",
                asset.id.as_str(),
                "package asset must have a logical package path",
            ),
        }
        reviewed.push(ReviewedPackageAsset {
            descriptor: asset.clone(),
            disposition,
        });
    }
    if total_size > policy.max_total_asset_bytes {
        blocker(
            issues,
            "package.assets_too_large",
            "assets",
            "total asset bytes exceed the configured limit",
        );
    }
    reviewed
}

fn validate_provenance(
    snapshot: &PackageInspectionSnapshot,
    issues: &mut Vec<PackageValidationIssue>,
) {
    let provenance = &snapshot.manifest.provenance;
    if provenance.source_kind != SourceKind::ImportedPackage {
        blocker(
            issues,
            "package.invalid_provenance_kind",
            "manifest.provenance.source_kind",
            "an inspected import must be marked as imported package content",
        );
    }
    if provenance.source_id.as_deref() != Some(snapshot.manifest.package_id.as_str()) {
        blocker(
            issues,
            "package.provenance_source_id_mismatch",
            "manifest.provenance.source_id",
            "provenance source identifier must match package_id",
        );
    }
    match &provenance.source_hash {
        Some(hash) => match Sha256Digest::parse(hash) {
            Ok(hash) if hash == snapshot.source_sha256 => {}
            Ok(_) => blocker(
                issues,
                "package.provenance_hash_mismatch",
                "manifest.provenance.source_hash",
                "provenance source hash must match inspected source bytes",
            ),
            Err(_) => blocker(
                issues,
                "package.provenance_hash_invalid",
                "manifest.provenance.source_hash",
                "provenance source hash is not a SHA-256 digest",
            ),
        },
        None => blocker(
            issues,
            "package.provenance_hash_missing",
            "manifest.provenance.source_hash",
            "imported package provenance requires a source hash",
        ),
    }
    if provenance.author != snapshot.manifest.author {
        blocker(
            issues,
            "package.provenance_author_mismatch",
            "manifest.provenance.author",
            "provenance author must match manifest author",
        );
    }
    if provenance.license.as_deref() != Some(snapshot.manifest.license.as_str()) {
        blocker(
            issues,
            "package.provenance_license_mismatch",
            "manifest.provenance.license",
            "provenance license must match manifest license",
        );
    }
    if provenance.imported_at.is_none() {
        blocker(
            issues,
            "package.provenance_time_missing",
            "manifest.provenance.imported_at",
            "imported package provenance requires an import timestamp",
        );
    }
}

fn validate_signature(
    snapshot: &PackageInspectionSnapshot,
    policy: &PackageValidationPolicy,
    issues: &mut Vec<PackageValidationIssue>,
) {
    if let Some(signature) = &snapshot.manifest.signature {
        validate_label(&signature.algorithm, "manifest.signature.algorithm", issues);
        validate_label(&signature.key_id, "manifest.signature.key_id", issues);
        validate_label(
            &signature.signature_base64,
            "manifest.signature.signature_base64",
            issues,
        );
    }
    match (
        snapshot.manifest.signature.is_some(),
        snapshot.signature_verification,
    ) {
        (false, SignatureVerification::Absent) if policy.allow_unsigned_packages => warning(
            issues,
            "package.unsigned",
            "manifest.signature",
            "package has no verified signature",
        ),
        (false, SignatureVerification::Absent) => blocker(
            issues,
            "package.signature_required",
            "manifest.signature",
            "policy requires a verified package signature",
        ),
        (true, SignatureVerification::Verified) => {}
        (true, SignatureVerification::Invalid) => blocker(
            issues,
            "package.signature_invalid",
            "manifest.signature",
            "package signature verification failed",
        ),
        (true, SignatureVerification::Unsupported) => blocker(
            issues,
            "package.signature_unsupported",
            "manifest.signature",
            "package signature algorithm is unsupported",
        ),
        _ => blocker(
            issues,
            "package.signature_state_mismatch",
            "manifest.signature",
            "manifest signature and verification state disagree",
        ),
    }
}

fn redistribution_status(
    snapshot: &PackageInspectionSnapshot,
    policy: &PackageValidationPolicy,
    issues: &[PackageValidationIssue],
) -> RedistributionStatus {
    if issues
        .iter()
        .any(|issue| issue.severity == PackageIssueSeverity::Blocker)
    {
        return RedistributionStatus::ValidationBlocked;
    }
    if !snapshot.manifest.redistribution_allowed {
        return RedistributionStatus::DeniedByManifest;
    }
    if issues.iter().any(|issue| {
        issue.code.starts_with("package.provenance_") || issue.code == "package.author_missing"
    }) {
        return RedistributionStatus::ProvenanceIncomplete;
    }
    if is_unclear_license(&snapshot.manifest.license)
        || !policy
            .redistributable_licenses
            .iter()
            .any(|license| license.eq_ignore_ascii_case(&snapshot.manifest.license))
    {
        return RedistributionStatus::LicenseUnclear;
    }
    RedistributionStatus::Allowed
}

fn is_unclear_license(license: &str) -> bool {
    matches!(
        license.trim().to_ascii_uppercase().as_str(),
        "" | "UNKNOWN" | "NOASSERTION" | "UNLICENSED" | "LICENSE-UNKNOWN" | "LICENSEREF-UNKNOWN"
    ) || license
        .trim()
        .to_ascii_uppercase()
        .starts_with("LICENSEREF-")
}

fn is_high_risk_media_type(media_type: &str) -> bool {
    matches!(
        media_type.trim().to_ascii_lowercase().as_str(),
        "application/javascript"
            | "application/wasm"
            | "application/xhtml+xml"
            | "application/x-executable"
            | "image/svg+xml"
            | "text/css"
            | "text/html"
            | "text/javascript"
    ) || media_type.trim().to_ascii_lowercase().starts_with("font/")
}

fn asset_path_matches_digest_and_media_type(
    path: &str,
    digest: &Sha256Digest,
    media_type: &str,
) -> bool {
    let expected_prefix = format!("assets/sha256/{digest}");
    let Some(suffix) = path.strip_prefix(&expected_prefix) else {
        return false;
    };
    if suffix.is_empty() {
        return true;
    }
    let Some(extension) = suffix.strip_prefix('.') else {
        return false;
    };
    allowed_asset_extensions(media_type).contains(&extension)
}

fn allowed_asset_extensions(media_type: &str) -> &'static [&'static str] {
    match media_type.trim().to_ascii_lowercase().as_str() {
        "image/png" => &["png"],
        "image/jpeg" => &["jpg", "jpeg"],
        "image/gif" => &["gif"],
        "image/webp" => &["webp"],
        "image/avif" => &["avif"],
        "audio/mpeg" => &["mp3"],
        "audio/wav" => &["wav"],
        "audio/ogg" => &["ogg"],
        "video/mp4" => &["mp4"],
        "video/webm" => &["webm"],
        "application/pdf" => &["pdf"],
        "text/plain" => &["txt"],
        "application/javascript" | "text/javascript" => &["js"],
        "application/wasm" => &["wasm"],
        "application/xhtml+xml" => &["xhtml"],
        "application/x-executable" => &["exe"],
        "image/svg+xml" => &["svg"],
        "text/css" => &["css"],
        "text/html" => &["html", "htm"],
        "font/woff" => &["woff"],
        "font/woff2" => &["woff2"],
        "font/ttf" => &["ttf"],
        "font/otf" => &["otf"],
        _ => &[],
    }
}

fn parse_version_triplet(version: &str) -> Option<(u64, u64, u64)> {
    let core = version
        .trim()
        .split_once('-')
        .map_or(version.trim(), |pair| pair.0);
    let mut segments = core.split('.');
    let major = segments.next()?.parse().ok()?;
    let minor = segments.next()?.parse().ok()?;
    let patch = segments.next()?.parse().ok()?;
    if segments.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

fn validate_label(value: &str, target: &str, issues: &mut Vec<PackageValidationIssue>) {
    if value.trim().is_empty()
        || value.len() > MAX_LABEL_BYTES
        || value.chars().any(char::is_control)
    {
        blocker(
            issues,
            "package.invalid_label",
            target,
            "value must be non-empty, bounded, and free of control characters",
        );
    }
}

fn validate_logical_path(path: &str, target: &str, issues: &mut Vec<PackageValidationIssue>) {
    let invalid = path.is_empty()
        || path.len() > MAX_PATH_BYTES
        || path.starts_with('/')
        || path.starts_with('\\')
        || path.contains('\\')
        || path.contains('\0')
        || path
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
        || path
            .as_bytes()
            .get(1)
            .is_some_and(|byte| *byte == b':' && path.as_bytes()[0].is_ascii_alphabetic());
    if invalid {
        blocker(
            issues,
            "package.invalid_logical_path",
            target,
            "logical path must be normalized, relative, and traversal-free",
        );
    }
}

fn blocker(issues: &mut Vec<PackageValidationIssue>, code: &str, target: &str, message: &str) {
    issues.push(PackageValidationIssue {
        severity: PackageIssueSeverity::Blocker,
        code: code.to_owned(),
        target: target.to_owned(),
        message: message.to_owned(),
    });
}

fn warning(issues: &mut Vec<PackageValidationIssue>, code: &str, target: &str, message: &str) {
    issues.push(PackageValidationIssue {
        severity: PackageIssueSeverity::Warning,
        code: code.to_owned(),
        target: target.to_owned(),
        message: message.to_owned(),
    });
}

fn capability_rank(capability: ContentCapability) -> u8 {
    match capability {
        ContentCapability::PromptFragments => 0,
        ContentCapability::Knowledge => 1,
        ContentCapability::Variables => 2,
        ContentCapability::Transforms => 3,
        ContentCapability::DeclarativeInteractions => 4,
        ContentCapability::ImageAssets => 5,
        ContentCapability::AudioAssets => 6,
        ContentCapability::VideoAssets => 7,
        ContentCapability::AttachmentAssets => 8,
        ContentCapability::HighRiskAssets => 9,
    }
}

#[derive(Serialize)]
struct PackageReviewDigest<'a> {
    source_sha256: &'a Sha256Digest,
    manifest: &'a PackageManifest,
    components: &'a [PackageComponentDescriptor],
    assets: &'a [ReviewedPackageAsset],
    issues: &'a [PackageValidationIssue],
    local_import_allowed: bool,
    redistribution_status: RedistributionStatus,
}

fn package_review_sha256(
    source_sha256: &Sha256Digest,
    manifest: &PackageManifest,
    components: &[PackageComponentDescriptor],
    assets: &[ReviewedPackageAsset],
    issues: &[PackageValidationIssue],
    local_import_allowed: bool,
    redistribution_status: RedistributionStatus,
) -> Result<Sha256Digest, PackageValidationError> {
    let payload = PackageReviewDigest {
        source_sha256,
        manifest,
        components,
        assets,
        issues,
        local_import_allowed,
        redistribution_status,
    };
    canonical_sha256(&payload)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageSelectionRequest {
    pub expected_review_sha256: Sha256Digest,
    pub component_ids: Vec<String>,
    pub standalone_asset_ids: Vec<AssetId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportPlanState {
    AwaitingApproval,
    Approved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedPackageComponent {
    pub component: PackageComponentDescriptor,
    /// Imported components are always disabled until an exact plan is approved.
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectiveImportPlan {
    pub plan_sha256: Sha256Digest,
    pub review_sha256: Sha256Digest,
    pub source_sha256: Sha256Digest,
    pub package_id: PackageId,
    pub state: ImportPlanState,
    pub components: Vec<PlannedPackageComponent>,
    pub assets: Vec<AssetDescriptor>,
    pub required_capabilities: Vec<ContentCapability>,
    pub redistribution_status: RedistributionStatus,
}

impl SelectiveImportPlan {
    pub fn verify(&self) -> Result<(), PackageValidationError> {
        validate_selected_payload(
            &self.components,
            &self.assets,
            &self.source_sha256,
            &self.required_capabilities,
            false,
        )?;
        if self.state != ImportPlanState::AwaitingApproval {
            return Err(PackageValidationError::InvalidPlanState);
        }
        let expected = selective_import_plan_sha256(
            &self.review_sha256,
            &self.source_sha256,
            &self.package_id,
            self.state,
            &self.components,
            &self.assets,
            &self.required_capabilities,
            self.redistribution_status,
        )?;
        if expected != self.plan_sha256 {
            return Err(PackageValidationError::PlanHashMismatch);
        }
        Ok(())
    }
}

#[allow(clippy::too_many_lines)] // Selection, closure, conflict, and inert defaults form one plan gate.
pub fn build_selective_import_plan(
    review: &PackageReview,
    request: &PackageSelectionRequest,
) -> Result<SelectiveImportPlan, PackageValidationError> {
    review.verify()?;
    if request.expected_review_sha256 != review.review_sha256 {
        return Err(PackageValidationError::StaleReview);
    }
    if !review.local_import_allowed {
        return Err(PackageValidationError::ImportBlocked);
    }
    if request.component_ids.is_empty() && request.standalone_asset_ids.is_empty() {
        return Err(PackageValidationError::EmptySelection);
    }

    ensure_unique_strings(&request.component_ids)?;
    ensure_unique_assets(&request.standalone_asset_ids)?;
    let component_map = review
        .components
        .iter()
        .map(|component| (component.id.as_str(), component))
        .collect::<BTreeMap<_, _>>();
    let requested = request
        .component_ids
        .iter()
        .map(|id| {
            component_map
                .get(id.as_str())
                .copied()
                .ok_or_else(|| PackageValidationError::UnknownComponent(id.clone()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    for component in &requested {
        if component.disposition != PackageComponentDisposition::Importable {
            return Err(PackageValidationError::ComponentNotImportable(
                component.id.clone(),
            ));
        }
    }

    let requested_ids = request
        .component_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let closure = dependency_closure(&request.component_ids, &component_map);
    let missing = closure
        .difference(&requested_ids)
        .map(|id| (*id).to_owned())
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(PackageValidationError::DependencyConfirmationRequired(
            missing,
        ));
    }
    let mut selected_conflicts = BTreeSet::new();
    for component in &requested {
        for conflict in &component.conflicts_with {
            if requested_ids.contains(conflict.as_str()) {
                let pair = if component.id < *conflict {
                    (component.id.clone(), conflict.clone())
                } else {
                    (conflict.clone(), component.id.clone())
                };
                selected_conflicts.insert(pair);
            }
        }
    }
    if let Some((left, right)) = selected_conflicts.into_iter().next() {
        return Err(PackageValidationError::SelectedComponentsConflict(
            left, right,
        ));
    }

    let asset_map = review
        .assets
        .iter()
        .map(|asset| (asset.descriptor.id.clone(), asset))
        .collect::<BTreeMap<_, _>>();
    let mut selected_asset_ids = request
        .standalone_asset_ids
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    for component in &requested {
        selected_asset_ids.extend(component.asset_ids.iter().cloned());
    }
    let mut assets = Vec::with_capacity(selected_asset_ids.len());
    for id in selected_asset_ids {
        let asset = asset_map
            .get(&id)
            .ok_or_else(|| PackageValidationError::UnknownAsset(id.as_str().to_owned()))?;
        if asset.disposition != AssetImportDisposition::Importable {
            return Err(PackageValidationError::AssetNotImportable(
                id.as_str().to_owned(),
            ));
        }
        assets.push(asset.descriptor.clone());
    }
    assets.sort_by(|left, right| left.id.cmp(&right.id));

    let mut components = requested
        .into_iter()
        .cloned()
        .map(|component| PlannedPackageComponent {
            component,
            enabled: false,
        })
        .collect::<Vec<_>>();
    components.sort_by(|left, right| {
        left.component
            .kind
            .cmp(&right.component.kind)
            .then_with(|| left.component.id.cmp(&right.component.id))
    });
    let mut required_capabilities = components
        .iter()
        .flat_map(|component| component.component.required_capabilities.iter().copied())
        .collect::<Vec<_>>();
    required_capabilities.sort_by_key(|capability| capability_rank(*capability));
    required_capabilities.dedup();

    let state = ImportPlanState::AwaitingApproval;
    let plan_sha256 = selective_import_plan_sha256(
        &review.review_sha256,
        &review.source_sha256,
        &review.manifest.package_id,
        state,
        &components,
        &assets,
        &required_capabilities,
        review.redistribution_status,
    )?;
    Ok(SelectiveImportPlan {
        plan_sha256,
        review_sha256: review.review_sha256.clone(),
        source_sha256: review.source_sha256.clone(),
        package_id: review.manifest.package_id.clone(),
        state,
        components,
        assets,
        required_capabilities,
        redistribution_status: review.redistribution_status,
    })
}

fn dependency_closure<'a>(
    roots: &'a [String],
    components: &BTreeMap<&'a str, &'a PackageComponentDescriptor>,
) -> BTreeSet<&'a str> {
    let mut closure = BTreeSet::new();
    let mut pending = roots.iter().map(String::as_str).collect::<Vec<_>>();
    while let Some(id) = pending.pop() {
        if !closure.insert(id) {
            continue;
        }
        if let Some(component) = components.get(id) {
            pending.extend(component.dependencies.iter().map(String::as_str));
        }
    }
    closure
}

fn ensure_unique_strings(values: &[String]) -> Result<(), PackageValidationError> {
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            return Err(PackageValidationError::DuplicateSelection(value.clone()));
        }
    }
    Ok(())
}

fn ensure_unique_assets(values: &[AssetId]) -> Result<(), PackageValidationError> {
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            return Err(PackageValidationError::DuplicateSelection(
                value.as_str().to_owned(),
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageImportApproval {
    pub approval_id: String,
    pub expected_review_sha256: Sha256Digest,
    pub expected_plan_sha256: Sha256Digest,
    pub target_review_sha256: Sha256Digest,
    pub update_target_confirmations_sha256: Sha256Digest,
    pub enable_component_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovedPackageImportPlan {
    pub approval_sha256: Sha256Digest,
    pub plan_sha256: Sha256Digest,
    pub review_sha256: Sha256Digest,
    pub source_sha256: Sha256Digest,
    pub package_id: PackageId,
    pub state: ImportPlanState,
    pub approval_id: String,
    pub target_review_sha256: Sha256Digest,
    pub update_target_confirmations_sha256: Sha256Digest,
    pub components: Vec<PlannedPackageComponent>,
    pub assets: Vec<AssetDescriptor>,
    pub required_capabilities: Vec<ContentCapability>,
    pub redistribution_status: RedistributionStatus,
}

impl ApprovedPackageImportPlan {
    pub fn verify(&self) -> Result<(), PackageValidationError> {
        if self.state != ImportPlanState::Approved {
            return Err(PackageValidationError::InvalidPlanState);
        }
        if self.approval_id.trim().is_empty()
            || self.approval_id.len() > MAX_LABEL_BYTES
            || self.approval_id.chars().any(char::is_control)
        {
            return Err(PackageValidationError::MissingApprovalId);
        }
        validate_selected_payload(
            &self.components,
            &self.assets,
            &self.source_sha256,
            &self.required_capabilities,
            true,
        )?;
        let mut pending_components = self.components.clone();
        for component in &mut pending_components {
            component.enabled = false;
        }
        let original_plan = SelectiveImportPlan {
            plan_sha256: self.plan_sha256.clone(),
            review_sha256: self.review_sha256.clone(),
            source_sha256: self.source_sha256.clone(),
            package_id: self.package_id.clone(),
            state: ImportPlanState::AwaitingApproval,
            components: pending_components,
            assets: self.assets.clone(),
            required_capabilities: self.required_capabilities.clone(),
            redistribution_status: self.redistribution_status,
        };
        original_plan.verify()?;
        let expected = approved_package_import_sha256(
            &self.plan_sha256,
            &self.review_sha256,
            &self.source_sha256,
            &self.package_id,
            self.state,
            &self.approval_id,
            &self.target_review_sha256,
            &self.update_target_confirmations_sha256,
            &self.components,
            &self.assets,
            &self.required_capabilities,
            self.redistribution_status,
        )?;
        if expected != self.approval_sha256 {
            return Err(PackageValidationError::ApprovalHashMismatch);
        }
        Ok(())
    }
}

#[allow(clippy::too_many_lines)] // Durable plan verification repeats every safety invariant.
fn validate_selected_payload(
    components: &[PlannedPackageComponent],
    assets: &[AssetDescriptor],
    source_sha256: &Sha256Digest,
    required_capabilities: &[ContentCapability],
    approved: bool,
) -> Result<(), PackageValidationError> {
    if components.is_empty() && assets.is_empty() {
        return Err(PackageValidationError::EmptySelection);
    }
    if components.len() > MAX_PACKAGE_COMPONENTS || assets.len() > MAX_PACKAGE_ASSETS {
        return Err(PackageValidationError::ImportBlocked);
    }
    let dependency_count = components
        .iter()
        .try_fold(0_usize, |total, component| {
            total.checked_add(component.component.dependencies.len())
        })
        .ok_or(PackageValidationError::ImportBlocked)?;
    if dependency_count > MAX_PACKAGE_DEPENDENCIES {
        return Err(PackageValidationError::ImportBlocked);
    }
    let mut component_ids = BTreeSet::new();
    for component in components {
        if !component_ids.insert(component.component.id.as_str()) {
            return Err(PackageValidationError::DuplicateSelection(
                component.component.id.clone(),
            ));
        }
        if component.component.disposition != PackageComponentDisposition::Importable {
            return Err(PackageValidationError::ComponentNotImportable(
                component.component.id.clone(),
            ));
        }
        if !approved && component.enabled {
            return Err(PackageValidationError::InvalidPlanState);
        }
    }
    for component in components {
        let missing = component
            .component
            .dependencies
            .iter()
            .filter(|dependency| !component_ids.contains(dependency.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(PackageValidationError::DependencyConfirmationRequired(
                missing,
            ));
        }
        for conflict in &component.component.conflicts_with {
            if component_ids.contains(conflict.as_str()) {
                let (left, right) = if component.component.id < *conflict {
                    (component.component.id.clone(), conflict.clone())
                } else {
                    (conflict.clone(), component.component.id.clone())
                };
                return Err(PackageValidationError::SelectedComponentsConflict(
                    left, right,
                ));
            }
        }
    }
    let mut expected_capabilities = components
        .iter()
        .flat_map(|component| component.component.required_capabilities.iter().copied())
        .collect::<Vec<_>>();
    expected_capabilities.sort();
    expected_capabilities.dedup();
    if expected_capabilities != required_capabilities {
        return Err(PackageValidationError::InvalidPlanCapabilities);
    }

    let mut asset_ids = BTreeSet::new();
    let mut total_asset_bytes = 0_u64;
    for asset in assets {
        total_asset_bytes = total_asset_bytes
            .checked_add(asset.size_bytes)
            .ok_or(PackageValidationError::ImportBlocked)?;
        if total_asset_bytes > MAX_PACKAGE_TOTAL_ASSET_BYTES {
            return Err(PackageValidationError::ImportBlocked);
        }
        if !asset_ids.insert(asset.id.clone()) {
            return Err(PackageValidationError::DuplicateSelection(
                asset.id.as_str().to_owned(),
            ));
        }
        if is_high_risk_media_type(&asset.media_type)
            || asset.source.kind != AssetSourceKind::LorepiaPackage
            || asset.source.source_sha256.as_ref() != Some(source_sha256)
        {
            return Err(PackageValidationError::AssetNotImportable(
                asset.id.as_str().to_owned(),
            ));
        }
    }
    for component in components {
        if let Some(missing) = component
            .component
            .asset_ids
            .iter()
            .find(|asset_id| !asset_ids.contains(asset_id))
        {
            return Err(PackageValidationError::UnknownAsset(
                missing.as_str().to_owned(),
            ));
        }
    }
    Ok(())
}

pub fn approve_selective_import_plan(
    plan: &SelectiveImportPlan,
    approval: &PackageImportApproval,
) -> Result<ApprovedPackageImportPlan, PackageValidationError> {
    plan.verify()?;
    if approval.approval_id.trim().is_empty()
        || approval.approval_id.len() > MAX_LABEL_BYTES
        || approval.approval_id.chars().any(char::is_control)
    {
        return Err(PackageValidationError::MissingApprovalId);
    }
    if approval.expected_review_sha256 != plan.review_sha256 {
        return Err(PackageValidationError::StaleReview);
    }
    if approval.expected_plan_sha256 != plan.plan_sha256 {
        return Err(PackageValidationError::StalePlan);
    }
    ensure_unique_approval_decisions(&approval.enable_component_ids)?;
    let selected = plan
        .components
        .iter()
        .map(|component| component.component.id.as_str())
        .collect::<BTreeSet<_>>();
    for id in &approval.enable_component_ids {
        if !selected.contains(id.as_str()) {
            return Err(PackageValidationError::UnknownApprovedComponent(id.clone()));
        }
    }

    let enabled = approval
        .enable_component_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut components = plan.components.clone();
    for component in &mut components {
        component.enabled = enabled.contains(component.component.id.as_str());
    }
    let approval_sha256 = approved_package_import_sha256(
        &plan.plan_sha256,
        &plan.review_sha256,
        &plan.source_sha256,
        &plan.package_id,
        ImportPlanState::Approved,
        &approval.approval_id,
        &approval.target_review_sha256,
        &approval.update_target_confirmations_sha256,
        &components,
        &plan.assets,
        &plan.required_capabilities,
        plan.redistribution_status,
    )?;
    Ok(ApprovedPackageImportPlan {
        approval_sha256,
        plan_sha256: plan.plan_sha256.clone(),
        review_sha256: plan.review_sha256.clone(),
        source_sha256: plan.source_sha256.clone(),
        package_id: plan.package_id.clone(),
        state: ImportPlanState::Approved,
        approval_id: approval.approval_id.clone(),
        target_review_sha256: approval.target_review_sha256.clone(),
        update_target_confirmations_sha256: approval.update_target_confirmations_sha256.clone(),
        components,
        assets: plan.assets.clone(),
        required_capabilities: plan.required_capabilities.clone(),
        redistribution_status: plan.redistribution_status,
    })
}

#[derive(Serialize)]
struct ApprovedPackageImportDigest<'a> {
    plan_sha256: &'a Sha256Digest,
    review_sha256: &'a Sha256Digest,
    source_sha256: &'a Sha256Digest,
    package_id: &'a PackageId,
    state: ImportPlanState,
    approval_id: &'a str,
    target_review_sha256: &'a Sha256Digest,
    update_target_confirmations_sha256: &'a Sha256Digest,
    components: &'a [PlannedPackageComponent],
    assets: &'a [AssetDescriptor],
    required_capabilities: &'a [ContentCapability],
    redistribution_status: RedistributionStatus,
}

#[allow(clippy::too_many_arguments)] // Fields mirror the approved immutable payload one-to-one.
fn approved_package_import_sha256(
    plan_sha256: &Sha256Digest,
    review_sha256: &Sha256Digest,
    source_sha256: &Sha256Digest,
    package_id: &PackageId,
    state: ImportPlanState,
    approval_id: &str,
    target_review_sha256: &Sha256Digest,
    update_target_confirmations_sha256: &Sha256Digest,
    components: &[PlannedPackageComponent],
    assets: &[AssetDescriptor],
    required_capabilities: &[ContentCapability],
    redistribution_status: RedistributionStatus,
) -> Result<Sha256Digest, PackageValidationError> {
    canonical_sha256(&ApprovedPackageImportDigest {
        plan_sha256,
        review_sha256,
        source_sha256,
        package_id,
        state,
        approval_id,
        target_review_sha256,
        update_target_confirmations_sha256,
        components,
        assets,
        required_capabilities,
        redistribution_status,
    })
}

fn ensure_unique_approval_decisions(values: &[String]) -> Result<(), PackageValidationError> {
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            return Err(PackageValidationError::DuplicateApprovalDecision(
                value.clone(),
            ));
        }
    }
    Ok(())
}

#[derive(Serialize)]
struct SelectiveImportPlanDigest<'a> {
    review_sha256: &'a Sha256Digest,
    source_sha256: &'a Sha256Digest,
    package_id: &'a PackageId,
    state: ImportPlanState,
    components: &'a [PlannedPackageComponent],
    assets: &'a [AssetDescriptor],
    required_capabilities: &'a [ContentCapability],
    redistribution_status: RedistributionStatus,
}

#[allow(clippy::too_many_arguments)] // Fields mirror the immutable digest payload one-to-one.
fn selective_import_plan_sha256(
    review_sha256: &Sha256Digest,
    source_sha256: &Sha256Digest,
    package_id: &PackageId,
    state: ImportPlanState,
    components: &[PlannedPackageComponent],
    assets: &[AssetDescriptor],
    required_capabilities: &[ContentCapability],
    redistribution_status: RedistributionStatus,
) -> Result<Sha256Digest, PackageValidationError> {
    canonical_sha256(&SelectiveImportPlanDigest {
        review_sha256,
        source_sha256,
        package_id,
        state,
        components,
        assets,
        required_capabilities,
        redistribution_status,
    })
}

fn canonical_sha256<T: Serialize>(value: &T) -> Result<Sha256Digest, PackageValidationError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| PackageValidationError::CanonicalEncoding(error.to_string()))?;
    Sha256Digest::parse(hex::encode(Sha256::digest(bytes)))
        .map_err(PackageValidationError::CanonicalEncoding)
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use lorepia_domain::{AssetRole, AssetSource, AssetSourceKind, PackageContentHash, Provenance};

    use super::*;

    fn digest(byte: &str) -> Sha256Digest {
        Sha256Digest::parse(byte.repeat(32)).expect("synthetic digest")
    }

    fn provenance(source_sha256: &Sha256Digest) -> Provenance {
        Provenance {
            source_kind: SourceKind::ImportedPackage,
            source_id: Some("pkg.synthetic".to_owned()),
            source_hash: Some(source_sha256.as_str().to_owned()),
            author: Some("Synthetic Author".to_owned()),
            license: Some("MIT".to_owned()),
            imported_at: Some(
                Utc.with_ymd_and_hms(2026, 8, 3, 12, 0, 0)
                    .single()
                    .expect("valid timestamp"),
            ),
        }
    }

    fn asset(id: &str, asset_digest: Sha256Digest) -> AssetDescriptor {
        AssetDescriptor {
            id: AssetId(id.to_owned()),
            sha256: asset_digest.clone(),
            media_type: "image/png".to_owned(),
            role: AssetRole::Expression,
            name: format!("{id}.png"),
            size_bytes: 128,
            width: Some(16),
            height: Some(16),
            duration_ms: None,
            source: AssetSource {
                kind: AssetSourceKind::LorepiaPackage,
                source_sha256: Some(digest("aa")),
                logical_path: Some(format!("assets/sha256/{asset_digest}")),
            },
        }
    }

    fn component(
        id: &str,
        digest: Sha256Digest,
        dependencies: &[&str],
        asset_ids: &[&str],
    ) -> PackageComponentDescriptor {
        PackageComponentDescriptor {
            id: id.to_owned(),
            kind: PackageComponentKind::ContentModule,
            logical_path: format!("modules/{id}.json"),
            sha256: digest,
            dependencies: dependencies
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
            conflicts_with: Vec::new(),
            required_capabilities: vec![ContentCapability::PromptFragments],
            asset_ids: asset_ids
                .iter()
                .map(|value| AssetId((*value).to_owned()))
                .collect(),
            disposition: PackageComponentDisposition::Importable,
        }
    }

    fn snapshot() -> PackageInspectionSnapshot {
        let source_sha256 = digest("aa");
        let base_hash = digest("11");
        let scene_hash = digest("22");
        let asset_hash = digest("33");
        let components = vec![
            component("scene", scene_hash.clone(), &["base"], &["portrait"]),
            component("base", base_hash.clone(), &[], &[]),
        ];
        let assets = vec![asset("portrait", asset_hash.clone())];
        let content_hashes = vec![
            PackageContentHash {
                logical_path: "modules/scene.json".to_owned(),
                sha256: scene_hash.clone(),
                size_bytes: 128,
            },
            PackageContentHash {
                logical_path: format!("assets/sha256/{asset_hash}"),
                sha256: asset_hash.clone(),
                size_bytes: 128,
            },
            PackageContentHash {
                logical_path: "modules/base.json".to_owned(),
                sha256: base_hash.clone(),
                size_bytes: 128,
            },
        ];
        let observed_entries = content_hashes
            .iter()
            .map(|entry| ObservedPackageEntry {
                logical_path: entry.logical_path.clone(),
                sha256: entry.sha256.clone(),
                size_bytes: 128,
            })
            .chain(std::iter::once(ObservedPackageEntry {
                logical_path: "manifest.json".to_owned(),
                sha256: digest("44"),
                size_bytes: 256,
            }))
            .collect();
        PackageInspectionSnapshot {
            source_sha256: source_sha256.clone(),
            source_size_bytes: 1_024,
            manifest: PackageManifest {
                format: LOREPIA_PACKAGE_FORMAT.to_owned(),
                format_version: LOREPIA_PACKAGE_FORMAT_VERSION,
                package_id: PackageId("pkg.synthetic".to_owned()),
                name: "Synthetic package".to_owned(),
                version: "1.0.0".to_owned(),
                author: Some("Synthetic Author".to_owned()),
                license: "MIT".to_owned(),
                redistribution_allowed: true,
                required_app_version: Some("0.1.0".to_owned()),
                required_capabilities: vec![ContentCapability::PromptFragments],
                content_hashes,
                signature: None,
                provenance: provenance(&source_sha256),
            },
            signature_verification: SignatureVerification::Absent,
            components,
            assets,
            observed_entries,
        }
    }

    #[test]
    fn content_addressed_asset_paths_allow_only_matching_media_extensions() {
        let asset_hash = digest("33");
        let base = format!("assets/sha256/{asset_hash}");

        assert!(asset_path_matches_digest_and_media_type(
            &base,
            &asset_hash,
            "image/png"
        ));
        assert!(asset_path_matches_digest_and_media_type(
            &format!("{base}.png"),
            &asset_hash,
            "image/png"
        ));
        assert!(!asset_path_matches_digest_and_media_type(
            &format!("{base}.jpg"),
            &asset_hash,
            "image/png"
        ));
        assert!(!asset_path_matches_digest_and_media_type(
            &format!("{base}.png.exe"),
            &asset_hash,
            "image/png"
        ));
        assert!(!asset_path_matches_digest_and_media_type(
            &format!("assets/sha256/{}.png", digest("44")),
            &asset_hash,
            "image/png"
        ));
    }

    #[test]
    fn validation_is_deterministic_across_set_like_input_permutations() {
        let original = snapshot();
        let mut permuted = original.clone();
        permuted.components.reverse();
        permuted.manifest.content_hashes.reverse();
        permuted.observed_entries.reverse();
        permuted.assets.reverse();

        let first = validate_package_snapshot(&original, &PackageValidationPolicy::default())
            .expect("review");
        let second = validate_package_snapshot(&permuted, &PackageValidationPolicy::default())
            .expect("review");

        assert_eq!(first, second);
        first.verify().expect("valid review hash");
    }

    #[test]
    fn hash_mismatch_blocks_local_import() {
        let mut input = snapshot();
        input.observed_entries[0].sha256 = digest("ff");

        let review =
            validate_package_snapshot(&input, &PackageValidationPolicy::default()).expect("review");

        assert!(!review.local_import_allowed);
        assert_eq!(
            review.redistribution_status,
            RedistributionStatus::ValidationBlocked
        );
        assert!(
            review
                .issues
                .iter()
                .any(|issue| issue.code == "package.hash_mismatch")
        );
    }

    #[test]
    fn traversal_and_unhashed_payload_are_blocked() {
        let mut input = snapshot();
        input.observed_entries.push(ObservedPackageEntry {
            logical_path: "../escape".to_owned(),
            sha256: digest("55"),
            size_bytes: 1,
        });

        let review =
            validate_package_snapshot(&input, &PackageValidationPolicy::default()).expect("review");

        assert!(!review.local_import_allowed);
        assert!(
            review
                .issues
                .iter()
                .any(|issue| issue.code == "package.invalid_logical_path")
        );
        assert!(
            review
                .issues
                .iter()
                .any(|issue| issue.code == "package.unhashed_payload")
        );
    }

    #[test]
    fn unclear_license_allows_local_import_but_never_redistribution() {
        let mut input = snapshot();
        input.manifest.license = "LicenseRef-Unknown".to_owned();
        input.manifest.provenance.license = Some("LicenseRef-Unknown".to_owned());

        let review =
            validate_package_snapshot(&input, &PackageValidationPolicy::default()).expect("review");

        assert!(review.local_import_allowed);
        assert_eq!(
            review.redistribution_status,
            RedistributionStatus::LicenseUnclear
        );
    }

    #[test]
    fn unsupported_capability_blocks_import() {
        let mut input = snapshot();
        input
            .manifest
            .required_capabilities
            .push(ContentCapability::HighRiskAssets);

        let review =
            validate_package_snapshot(&input, &PackageValidationPolicy::default()).expect("review");

        assert!(!review.local_import_allowed);
        assert!(
            review
                .issues
                .iter()
                .any(|issue| issue.code == "package.unsupported_capability")
        );
    }

    #[test]
    fn newer_required_application_version_blocks_import() {
        let mut input = snapshot();
        input.manifest.required_app_version = Some("99.0.0".to_owned());

        let review =
            validate_package_snapshot(&input, &PackageValidationPolicy::default()).expect("review");

        assert!(!review.local_import_allowed);
        assert!(
            review
                .issues
                .iter()
                .any(|issue| issue.code == "package.app_version_too_old")
        );
    }

    #[test]
    fn dependency_cycles_are_blocked_before_selection() {
        let mut input = snapshot();
        input.components[0].dependencies = vec!["base".to_owned()];
        input.components[1].dependencies = vec!["scene".to_owned()];

        let review =
            validate_package_snapshot(&input, &PackageValidationPolicy::default()).expect("review");

        assert!(!review.local_import_allowed);
        assert!(
            review
                .issues
                .iter()
                .any(|issue| issue.code == "package.dependency_cycle")
        );
    }

    #[test]
    fn selected_conflicts_are_detected_even_when_only_higher_id_declares_them() {
        let mut input = snapshot();
        input.components[0].dependencies.clear();
        let base_index = input
            .components
            .iter()
            .position(|component| component.id == "base")
            .expect("base component");
        let scene_index = input
            .components
            .iter()
            .position(|component| component.id == "scene")
            .expect("scene component");
        input.components[scene_index].conflicts_with = vec!["base".to_owned()];
        input.components[base_index].conflicts_with.clear();
        let review =
            validate_package_snapshot(&input, &PackageValidationPolicy::default()).expect("review");

        let result = build_selective_import_plan(
            &review,
            &PackageSelectionRequest {
                expected_review_sha256: review.review_sha256.clone(),
                component_ids: vec!["base".to_owned(), "scene".to_owned()],
                standalone_asset_ids: Vec::new(),
            },
        );

        assert_eq!(
            result,
            Err(PackageValidationError::SelectedComponentsConflict(
                "base".to_owned(),
                "scene".to_owned()
            ))
        );
    }

    #[test]
    fn high_risk_assets_are_quarantined_and_cannot_be_selected() {
        let mut input = snapshot();
        input.assets[0].media_type = "text/html".to_owned();
        let review =
            validate_package_snapshot(&input, &PackageValidationPolicy::default()).expect("review");

        assert!(review.local_import_allowed);
        assert_eq!(
            review.assets[0].disposition,
            AssetImportDisposition::Quarantined
        );
        let result = build_selective_import_plan(
            &review,
            &PackageSelectionRequest {
                expected_review_sha256: review.review_sha256.clone(),
                component_ids: vec!["base".to_owned()],
                standalone_asset_ids: vec![AssetId("portrait".to_owned())],
            },
        );
        assert!(matches!(
            result,
            Err(PackageValidationError::AssetNotImportable(_))
        ));
    }

    #[test]
    fn asset_provenance_must_bind_the_inspected_source() {
        let mut input = snapshot();
        input.assets[0].source.source_sha256 = Some(digest("ff"));

        let review =
            validate_package_snapshot(&input, &PackageValidationPolicy::default()).expect("review");

        assert!(!review.local_import_allowed);
        assert!(
            review
                .issues
                .iter()
                .any(|issue| issue.code == "package.asset_provenance_hash_mismatch")
        );
    }

    #[test]
    fn dependencies_require_explicit_selection_confirmation() {
        let review = validate_package_snapshot(&snapshot(), &PackageValidationPolicy::default())
            .expect("review");
        let result = build_selective_import_plan(
            &review,
            &PackageSelectionRequest {
                expected_review_sha256: review.review_sha256.clone(),
                component_ids: vec!["scene".to_owned()],
                standalone_asset_ids: Vec::new(),
            },
        );

        assert_eq!(
            result,
            Err(PackageValidationError::DependencyConfirmationRequired(
                vec!["base".to_owned()]
            ))
        );
    }

    #[test]
    fn selective_plan_is_disabled_until_exact_explicit_approval() {
        let review = validate_package_snapshot(&snapshot(), &PackageValidationPolicy::default())
            .expect("review");
        let plan = build_selective_import_plan(
            &review,
            &PackageSelectionRequest {
                expected_review_sha256: review.review_sha256.clone(),
                component_ids: vec!["scene".to_owned(), "base".to_owned()],
                standalone_asset_ids: Vec::new(),
            },
        )
        .expect("plan");

        assert_eq!(plan.state, ImportPlanState::AwaitingApproval);
        assert!(plan.components.iter().all(|component| !component.enabled));
        assert_eq!(
            approve_selective_import_plan(
                &plan,
                &PackageImportApproval {
                    approval_id: "approval-1".to_owned(),
                    expected_review_sha256: review.review_sha256.clone(),
                    expected_plan_sha256: digest("ff"),
                    target_review_sha256: digest("aa"),
                    update_target_confirmations_sha256: digest("bb"),
                    enable_component_ids: vec!["base".to_owned()],
                }
            ),
            Err(PackageValidationError::StalePlan)
        );

        let approved = approve_selective_import_plan(
            &plan,
            &PackageImportApproval {
                approval_id: "approval-1".to_owned(),
                expected_review_sha256: review.review_sha256.clone(),
                expected_plan_sha256: plan.plan_sha256.clone(),
                target_review_sha256: digest("aa"),
                update_target_confirmations_sha256: digest("bb"),
                enable_component_ids: vec!["base".to_owned()],
            },
        )
        .expect("approved plan");
        assert_eq!(approved.state, ImportPlanState::Approved);
        assert!(
            approved
                .components
                .iter()
                .find(|component| component.component.id == "base")
                .expect("base")
                .enabled
        );
        assert!(
            !approved
                .components
                .iter()
                .find(|component| component.component.id == "scene")
                .expect("scene")
                .enabled
        );
        approved.verify().expect("approval hash");
        let mut target_tampered = approved.clone();
        target_tampered.target_review_sha256 = digest("cc");
        assert_eq!(
            target_tampered.verify(),
            Err(PackageValidationError::ApprovalHashMismatch)
        );
        let mut confirmation_tampered = approved.clone();
        confirmation_tampered.update_target_confirmations_sha256 = digest("dd");
        assert_eq!(
            confirmation_tampered.verify(),
            Err(PackageValidationError::ApprovalHashMismatch)
        );
        let mut tampered = approved;
        tampered.components[0].enabled = !tampered.components[0].enabled;
        assert_eq!(
            tampered.verify(),
            Err(PackageValidationError::ApprovalHashMismatch)
        );
    }

    #[test]
    fn tampered_review_and_plan_hashes_are_rejected() {
        let mut review =
            validate_package_snapshot(&snapshot(), &PackageValidationPolicy::default())
                .expect("review");
        review.components[0].id.push_str("-tampered");
        assert_eq!(
            review.verify(),
            Err(PackageValidationError::ReviewHashMismatch)
        );

        let review = validate_package_snapshot(&snapshot(), &PackageValidationPolicy::default())
            .expect("review");
        let mut plan = build_selective_import_plan(
            &review,
            &PackageSelectionRequest {
                expected_review_sha256: review.review_sha256.clone(),
                component_ids: vec!["base".to_owned()],
                standalone_asset_ids: Vec::new(),
            },
        )
        .expect("plan");
        plan.components[0].enabled = true;
        assert_eq!(plan.verify(), Err(PackageValidationError::InvalidPlanState));
    }
}
