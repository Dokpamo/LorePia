use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

#[cfg(not(any(target_os = "android", target_os = "linux", target_vendor = "apple")))]
use std::fs::OpenOptions;

use lorepia_domain::{
    AssetDescriptor, AssetId, AssetRole, AssetSource, AssetSourceKind, BlockSource, ContentModule,
    CoreError, CoreErrorCode, CoreResult, ImportLimits, ImportWarning, InspectionId,
    InstructionAuthority, InteractionRuleSet, KnowledgeBook, MAX_IDENTIFIER_CHARS, MAX_NAME_CHARS,
    MemoryProfile, PlacementZone, PromptPreset, Provenance, Sha256Digest, SourceKind, TransformSet,
    ValidateOrchestration,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest, Sha256};
use zip::{ZipArchive, read::ZipFile};

use crate::{path::validate_archive_path, sha256_file, validated_source_metadata};

const PACKAGE_FORMAT: &str = "lorepia_content_package";
const PACKAGE_FORMAT_VERSION: u32 = 1;
const MANIFEST_PATH: &str = "manifest.json";
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;
const MAX_COMPONENT_JSON_BYTES: u64 = 8 * 1024 * 1024;
const MAX_PATH_BYTES: usize = 1_024;
const MAX_PATH_CHARS: usize = 512;
const MAX_LABEL_BYTES: usize = 4_096;
const MAX_LABEL_CHARS: usize = 2_048;
const MAX_MANIFEST_LIST_ITEMS: usize = 2_048;
const MAX_JSON_SCAN_NODES: usize = 100_000;
const ENTRY_HEADER_BYTES: usize = 16;
const READ_BUFFER_BYTES: usize = 64 * 1024;
const CONTENT_MODULE_SCHEMA_VERSION: u32 = 1;
const MAX_MODULE_METADATA_TAGS: usize = 64;
const MAX_MODULE_METADATA_TAG_CHARS: usize = 128;
const MAX_MODULE_HOMEPAGE_CHARS: usize = 2_048;

/// A declared application capability used by a `LorePia` content package.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContentCapability(pub String);

impl ContentCapability {
    pub fn is_supported(&self) -> bool {
        matches!(
            self.0.as_str(),
            "prompt_presets"
                | "knowledge_books"
                | "memory_profiles"
                | "safe_transforms"
                | "declarative_interactions"
                | "media_assets"
                | "content_modules"
                | "variables"
                | "image_assets"
                | "audio_assets"
                | "video_assets"
                | "attachment_assets"
        )
    }
}

/// A package-level dependency. The requirement is retained for the
/// orchestration layer; source inspection never resolves remote dependencies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentPackageDependency {
    pub package_id: String,
    #[serde(default)]
    pub version_requirement: Option<String>,
    #[serde(default)]
    pub optional: bool,
}

/// A package-level conflict retained for activation-time resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageConflict {
    pub package_id: String,
    #[serde(default)]
    pub version_requirement: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

/// Validated manifest metadata. Paths are normalized logical archive paths,
/// never host filesystem paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentPackageManifest {
    pub format: String,
    pub format_version: u32,
    pub package_id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub license: String,
    pub redistribution_allowed: bool,
    #[serde(default)]
    pub required_app_version: Option<String>,
    #[serde(default)]
    pub required_capabilities: Vec<ContentCapability>,
    #[serde(default)]
    pub dependencies: Vec<ContentPackageDependency>,
    #[serde(default)]
    pub conflicts: Vec<PackageConflict>,
    pub content_hashes: BTreeMap<String, String>,
    #[serde(default)]
    pub content_types: BTreeMap<String, String>,
    pub signature_present: bool,
}

impl ContentPackageManifest {
    pub fn can_redistribute(&self) -> bool {
        self.redistribution_allowed && has_usable_license(&self.license)
    }
}

/// Logical kind inferred from the package's fixed directory layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentPackageComponentKind {
    Prompt,
    Knowledge,
    Memory,
    Transform,
    Interaction,
    ContentModule,
    Asset,
    Unsupported,
}

/// Whether a component can be selected for import.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentPackageComponentState {
    Selectable,
    InactiveUnsupported,
    Quarantined,
}

/// Bounded, shell-safe component metadata generated during inspection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentPackageComponent {
    pub id: String,
    pub path: String,
    pub kind: ContentPackageComponentKind,
    pub media_type: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub state: ContentPackageComponentState,
    #[serde(default)]
    pub inactive_reasons: Vec<String>,
    #[serde(default)]
    pub required_capabilities: Vec<ContentCapability>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub conflicts_with: Vec<String>,
    /// Content-addressed assets referenced by a declarative content module.
    ///
    /// This bounded inspection projection contains identifiers only. Asset
    /// bytes remain streamed and no module gains activation authority here.
    #[serde(default)]
    pub referenced_asset_ids: Vec<AssetId>,
}

impl ContentPackageComponent {
    pub fn is_selectable(&self) -> bool {
        self.state == ContentPackageComponentState::Selectable
    }
}

/// Immutable review result for a version-1 `LorePia` content package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentPackageInspection {
    pub id: InspectionId,
    pub manifest: ContentPackageManifest,
    pub source_sha256: String,
    pub source_size: u64,
    pub total_uncompressed_size: u64,
    pub components: Vec<ContentPackageComponent>,
    pub warnings: Vec<ImportWarning>,
    pub blocked_reasons: Vec<String>,
    pub unsupported_manifest_fields: Vec<String>,
    pub plan_hash: String,
    pub local_use_only: bool,
}

impl ContentPackageInspection {
    pub fn is_allowed(&self) -> bool {
        self.blocked_reasons.is_empty()
    }

    pub fn selectable_component_ids(&self) -> Vec<String> {
        self.components
            .iter()
            .filter(|component| component.is_selectable())
            .map(|component| component.id.clone())
            .collect()
    }
}

/// A component selection bound to the inspected source and normalized plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentPackageSelectionPlan {
    pub inspection_id: InspectionId,
    pub source_sha256: String,
    pub package_plan_hash: String,
    pub selected_component_ids: Vec<String>,
    pub selection_plan_hash: String,
}

/// Typed, selected JSON document ready for an application-layer transaction.
/// Preparing documents performs no persistence or activation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "document", rename_all = "snake_case")]
pub enum PreparedContentDocument {
    PromptPreset(Box<PromptPreset>),
    KnowledgeBook(Box<KnowledgeBook>),
    MemoryProfile(Box<MemoryProfile>),
    TransformSet(Box<TransformSet>),
    InteractionRuleSet(Box<InteractionRuleSet>),
    ContentModule(Box<ContentModule>),
}

/// A typed document bound to the exact selected source component and its
/// original deterministic position within that component.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedContentDocumentEnvelope {
    pub source_component_id: String,
    pub source_component_ordinal: u32,
    pub document_ordinal: u32,
    pub document_kind: String,
    pub document_id: String,
    pub document_sha256: String,
    pub document: PreparedContentDocument,
}

/// An explicit normalization performed while preparing imported declarative
/// behavior. Imported transforms and interactions remain inactive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentPackageTransformation {
    pub component_id: String,
    pub object_id: String,
    pub field: String,
    pub before: bool,
    pub after: bool,
    pub reason: String,
}

/// Revalidated, typed commit input. This value contains no absolute source
/// path, performs no persistence, and never carries executable package bytes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreparedContentPackageImport {
    pub inspection: ContentPackageInspection,
    pub selection: ContentPackageSelectionPlan,
    pub documents: Vec<PreparedContentDocumentEnvelope>,
    pub assets: Vec<AssetDescriptor>,
    pub transformations: Vec<ContentPackageTransformation>,
}

/// One exact selected package asset streamed into a Core-owned temporary file.
///
/// The path is an internal Rust handoff to storage. It is never serialized or
/// returned through the shell API, and callers must either promote or discard
/// it before releasing the package commit claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedContentPackageAsset {
    pub component_id: String,
    pub staged_path: PathBuf,
    pub descriptor: AssetDescriptor,
}

#[derive(Debug, Deserialize)]
struct ManifestWire {
    format: String,
    format_version: u32,
    package_id: String,
    name: String,
    version: String,
    #[serde(default)]
    author: String,
    #[serde(default = "unknown_license")]
    license: String,
    #[serde(default)]
    redistribution_allowed: bool,
    #[serde(default)]
    required_app_version: Option<String>,
    #[serde(default)]
    required_capabilities: Vec<String>,
    #[serde(default)]
    dependencies: Vec<DependencyWire>,
    #[serde(default)]
    conflicts: Vec<ConflictWire>,
    content_hashes: BTreeMap<String, String>,
    #[serde(default, alias = "media_types")]
    content_types: BTreeMap<String, String>,
    #[serde(default)]
    components: Vec<ComponentDeclarationWire>,
    #[serde(default)]
    signature: Value,
    #[serde(flatten)]
    extensions: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum DependencyWire {
    Id(String),
    Detail {
        package_id: String,
        #[serde(default, alias = "version")]
        version_requirement: Option<String>,
        #[serde(default)]
        optional: bool,
    },
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ConflictWire {
    Id(String),
    Detail {
        package_id: String,
        #[serde(default, alias = "version")]
        version_requirement: Option<String>,
        #[serde(default)]
        reason: Option<String>,
    },
}

#[derive(Debug, Clone, Deserialize)]
struct ComponentDeclarationWire {
    id: String,
    path: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    media_type: Option<String>,
    #[serde(default)]
    required_capabilities: Vec<String>,
    #[serde(default)]
    depends_on: Vec<String>,
    #[serde(default)]
    conflicts_with: Vec<String>,
}

#[derive(Debug)]
struct ParsedManifest {
    manifest: ContentPackageManifest,
    declarations: BTreeMap<String, ComponentDeclarationWire>,
    unsupported_fields: Vec<String>,
    warnings: Vec<ImportWarning>,
}

#[derive(Debug)]
struct ScannedEntry {
    path: String,
    size_bytes: u64,
    sha256: String,
    header: [u8; ENTRY_HEADER_BYTES],
    header_len: usize,
    json_bytes: Option<Vec<u8>>,
    json_too_large: bool,
    json_hazards: BTreeSet<HazardKind>,
    json_error: Option<String>,
    referenced_asset_ids: Vec<AssetId>,
    module_required_capabilities: Vec<ContentCapability>,
}

#[derive(Debug, Clone, Copy)]
struct SelectedJsonComponentExpectation<'a> {
    id: &'a str,
    path: &'a str,
    sha256: &'a str,
    size_bytes: u64,
}

#[derive(Debug, Default)]
struct HazardScan {
    kinds: BTreeSet<HazardKind>,
    node_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum HazardKind {
    Code,
    Script,
    Html,
    ExternalUrl,
    UnsafeAction,
}

#[derive(Serialize)]
struct PackagePlanHashInput<'a> {
    manifest: &'a ContentPackageManifest,
    source_sha256: &'a str,
    source_size: u64,
    total_uncompressed_size: u64,
    components: &'a [ContentPackageComponent],
    warnings: &'a [ImportWarning],
    blocked_reasons: &'a [String],
    unsupported_manifest_fields: &'a [String],
    local_use_only: bool,
}

#[derive(Serialize)]
struct SelectionHashInput<'a> {
    source_sha256: &'a str,
    package_plan_hash: &'a str,
    selected_component_ids: &'a [String],
}

/// Inspect a standalone `LorePia` content package without extracting or
/// executing any entry.
pub fn inspect_content_package(
    path: &Path,
    limits: ImportLimits,
) -> CoreResult<ContentPackageInspection> {
    let source_metadata = validated_source_metadata(path, limits)?;
    let initial_source_hash = sha256_file(path)?;
    let (entries, total_uncompressed_size) = scan_package_archive(path, limits)?;
    let manifest_entry = entries
        .iter()
        .find(|entry| entry.path == MANIFEST_PATH)
        .ok_or_else(|| unsupported("content package must contain a root manifest.json"))?;
    let manifest_bytes = manifest_entry
        .json_bytes
        .as_deref()
        .ok_or_else(|| unsupported("manifest.json exceeds the metadata limit"))?;
    let parsed_manifest = parse_manifest(manifest_bytes)?;

    let final_source_hash = sha256_file(path)?;
    let final_source_size = path.metadata().map_err(storage_error)?.len();
    if final_source_hash != initial_source_hash || final_source_size != source_metadata.len() {
        return Err(unsafe_package(
            "content package changed while it was being inspected",
        ));
    }

    let mut warnings = parsed_manifest.warnings;
    let mut blocked_reasons = Vec::new();
    let mut components = build_component_inventory(
        &entries,
        &parsed_manifest.manifest,
        &parsed_manifest.declarations,
        &mut warnings,
        &mut blocked_reasons,
    )?;
    bind_content_module_asset_dependencies(&mut components, &mut blocked_reasons);
    validate_inventory_relationships(&components, &mut blocked_reasons);
    validate_manifest_coverage(&entries, &parsed_manifest.manifest, &mut blocked_reasons);

    warnings.sort_by(|left, right| {
        left.code
            .cmp(&right.code)
            .then_with(|| left.message.cmp(&right.message))
    });
    warnings.dedup_by(|left, right| left.code == right.code && left.message == right.message);
    blocked_reasons.sort();
    blocked_reasons.dedup();
    let local_use_only = !parsed_manifest.manifest.can_redistribute();
    let plan_hash = package_plan_hash(&PackagePlanHashInput {
        manifest: &parsed_manifest.manifest,
        source_sha256: &initial_source_hash,
        source_size: source_metadata.len(),
        total_uncompressed_size,
        components: &components,
        warnings: &warnings,
        blocked_reasons: &blocked_reasons,
        unsupported_manifest_fields: &parsed_manifest.unsupported_fields,
        local_use_only,
    })?;

    Ok(ContentPackageInspection {
        id: InspectionId::new(),
        manifest: parsed_manifest.manifest,
        source_sha256: initial_source_hash,
        source_size: source_metadata.len(),
        total_uncompressed_size,
        components,
        warnings,
        blocked_reasons,
        unsupported_manifest_fields: parsed_manifest.unsupported_fields,
        plan_hash,
        local_use_only,
    })
}

/// Construct a deterministic, dependency-checked selection plan.
pub fn select_content_package_components(
    inspection: &ContentPackageInspection,
    selected_component_ids: &[String],
) -> CoreResult<ContentPackageSelectionPlan> {
    if !inspection.is_allowed() {
        return Err(unsafe_package(
            "blocked content package cannot produce an import selection",
        ));
    }

    let mut selected = selected_component_ids.to_vec();
    selected.sort();
    selected.dedup();
    let components = inspection
        .components
        .iter()
        .map(|component| (component.id.as_str(), component))
        .collect::<BTreeMap<_, _>>();
    let selected_set = selected.iter().map(String::as_str).collect::<BTreeSet<_>>();

    for id in &selected {
        let component = components
            .get(id.as_str())
            .ok_or_else(|| invalid(format!("selected component does not exist: {id}")))?;
        if !component.is_selectable() {
            return Err(unsafe_package(format!(
                "selected component is inactive: {id}"
            )));
        }
        for dependency in &component.depends_on {
            if !selected_set.contains(dependency.as_str()) {
                return Err(invalid(format!(
                    "selected component {id} requires component {dependency}"
                )));
            }
        }
        for conflict in &component.conflicts_with {
            if selected_set.contains(conflict.as_str()) {
                return Err(invalid(format!(
                    "selected components conflict: {id} and {conflict}"
                )));
            }
        }
    }

    let selection_plan_hash = selection_hash(&SelectionHashInput {
        source_sha256: &inspection.source_sha256,
        package_plan_hash: &inspection.plan_hash,
        selected_component_ids: &selected,
    })?;
    Ok(ContentPackageSelectionPlan {
        inspection_id: inspection.id.clone(),
        source_sha256: inspection.source_sha256.clone(),
        package_plan_hash: inspection.plan_hash.clone(),
        selected_component_ids: selected,
        selection_plan_hash,
    })
}

/// Re-inspect a source immediately before commit and prove that both the
/// package plan and selected safe components are unchanged.
pub fn revalidate_content_package_selection(
    path: &Path,
    limits: ImportLimits,
    expected: &ContentPackageSelectionPlan,
) -> CoreResult<ContentPackageInspection> {
    let current = inspect_content_package(path, limits)?;
    if current.source_sha256 != expected.source_sha256 {
        return Err(unsafe_package(
            "content package source hash changed after inspection",
        ));
    }
    if current.plan_hash != expected.package_plan_hash {
        return Err(unsafe_package(
            "content package inspection plan changed before commit",
        ));
    }
    let current_selection =
        select_content_package_components(&current, &expected.selected_component_ids)?;
    if current_selection.selection_plan_hash != expected.selection_plan_hash {
        return Err(unsafe_package(
            "content package component selection changed before commit",
        ));
    }
    Ok(current)
}

/// Revalidate an approved selection and deserialize only the selected safe
/// JSON documents. Assets remain content-addressed metadata; their bytes are
/// not loaded into memory or persisted by this operation.
pub fn prepare_content_package_import(
    path: &Path,
    limits: ImportLimits,
    selection: &ContentPackageSelectionPlan,
) -> CoreResult<PreparedContentPackageImport> {
    let inspection = revalidate_content_package_selection(path, limits, selection)?;
    let selected = selection
        .selected_component_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let selected_components = inspection
        .components
        .iter()
        .enumerate()
        .filter(|(_, component)| selected.contains(component.id.as_str()))
        .collect::<Vec<_>>();
    if selected_components.len() != selected.len() {
        return Err(unsafe_package(
            "selected component inventory changed before preparation",
        ));
    }

    let json_components = selected_json_component_expectations(&selected_components)?;
    let selected_json = read_selected_json_entries(
        path,
        limits,
        &selection.source_sha256,
        inspection.source_size,
        &json_components,
    )?;
    let mut documents = Vec::new();
    let mut transformations = Vec::new();
    let source_sha256 =
        Sha256Digest::parse(inspection.source_sha256.clone()).map_err(unsafe_package)?;
    let package_provenance = Provenance {
        source_kind: SourceKind::ImportedPackage,
        source_id: Some(inspection.manifest.package_id.clone()),
        source_hash: Some(inspection.source_sha256.clone()),
        author: (!inspection.manifest.author.trim().is_empty())
            .then(|| inspection.manifest.author.clone()),
        license: Some(inspection.manifest.license.clone()),
        imported_at: None,
    };
    for (source_component_ordinal, component) in &selected_components {
        if !is_json_kind(component.kind) {
            continue;
        }
        let bytes = selected_json.get(&component.id).ok_or_else(|| {
            unsafe_package(format!(
                "selected JSON component disappeared: {}",
                component.id
            ))
        })?;
        decode_prepared_documents(
            u32::try_from(*source_component_ordinal)
                .map_err(|_| unsupported("package contains too many components"))?,
            component,
            bytes,
            &package_provenance,
            inspection.manifest.can_redistribute(),
            &mut documents,
            &mut transformations,
        )?;
    }

    let assets = selected_components
        .iter()
        .filter(|(_, component)| component.kind == ContentPackageComponentKind::Asset)
        .map(|(_, component)| package_asset_descriptor(component, &source_sha256))
        .collect::<CoreResult<Vec<_>>>()?;

    documents.sort_by(|left, right| {
        left.source_component_ordinal
            .cmp(&right.source_component_ordinal)
            .then_with(|| left.document_ordinal.cmp(&right.document_ordinal))
            .then_with(|| left.source_component_id.cmp(&right.source_component_id))
    });
    validate_prepared_document_envelopes(&documents)?;
    transformations.sort_by(|left, right| {
        left.component_id
            .cmp(&right.component_id)
            .then_with(|| left.object_id.cmp(&right.object_id))
            .then_with(|| left.field.cmp(&right.field))
    });
    Ok(PreparedContentPackageImport {
        inspection,
        selection: selection.clone(),
        documents,
        assets,
        transformations,
    })
}

/// Revalidate and stream only the exact selected package assets.
///
/// The full archive is never extracted. Every archive entry is rechecked for
/// path safety, symlinks, encryption, declared size and compression ratio.
/// Selected asset bytes are written through a fixed-size buffer while their
/// actual size, digest and media signature are verified against the reviewed
/// component. Any failure removes all files created by this call.
#[allow(clippy::too_many_lines)]
pub fn stage_selected_content_package_assets(
    path: &Path,
    limits: ImportLimits,
    selection: &ContentPackageSelectionPlan,
    staging_directory: &Path,
) -> CoreResult<Vec<StagedContentPackageAsset>> {
    let inspection = revalidate_content_package_selection(path, limits, selection)?;
    let selected_ids = selection
        .selected_component_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let selected_assets = inspection
        .components
        .iter()
        .filter(|component| {
            selected_ids.contains(component.id.as_str())
                && component.kind == ContentPackageComponentKind::Asset
        })
        .map(|component| (component.path.clone(), component))
        .collect::<BTreeMap<_, _>>();
    if selected_assets.is_empty() {
        return Ok(Vec::new());
    }

    fs::create_dir_all(staging_directory).map_err(storage_error)?;
    let staging_metadata = fs::symlink_metadata(staging_directory).map_err(storage_error)?;
    if !staging_metadata.file_type().is_dir() {
        return Err(unsafe_package(
            "package asset staging location is not an owned directory",
        ));
    }

    let before_hash = sha256_file(path)?;
    let before_size = path.metadata().map_err(storage_error)?.len();
    if before_hash != selection.source_sha256 || before_size != inspection.source_size {
        return Err(unsafe_package(
            "content package changed before selected assets were staged",
        ));
    }

    let file = File::open(path).map_err(storage_error)?;
    let mut archive = ZipArchive::new(file).map_err(|error| unsafe_package(error.to_string()))?;
    if archive.len() > limits.max_entries {
        return Err(unsafe_package(
            "archive entry count changed before asset staging",
        ));
    }

    let source_sha256 =
        Sha256Digest::parse(inspection.source_sha256.clone()).map_err(unsafe_package)?;
    let staging_nonce = InspectionId::new().0;
    let mut seen_paths = HashMap::new();
    let mut declared_total = 0_u64;
    let mut staged_total = 0_u64;
    let mut staged = Vec::with_capacity(selected_assets.len());
    let mut owned_paths = Vec::with_capacity(selected_assets.len());
    let mut buffer = vec![0_u8; READ_BUFFER_BYTES];

    let result = (|| {
        for index in 0..archive.len() {
            let mut entry = archive
                .by_index(index)
                .map_err(|error| unsafe_package(error.to_string()))?;
            let normalized =
                validate_package_entry(&entry, limits, &mut seen_paths, &mut declared_total)?;
            if entry.is_dir() {
                continue;
            }
            let Some(component) = selected_assets.get(&normalized).copied() else {
                continue;
            };
            if entry.size() != component.size_bytes {
                return Err(unsafe_package(format!(
                    "selected asset declared size changed: {normalized}"
                )));
            }
            staged_total = staged_total
                .checked_add(entry.size())
                .ok_or_else(|| unsafe_package("selected asset size overflow"))?;
            if staged_total > limits.max_total_uncompressed_bytes {
                return Err(unsafe_package(
                    "selected assets exceed the total uncompressed size limit",
                ));
            }

            let file_name = format!(".content-package-asset-{staging_nonce}-{index}.partial");
            let (staged_path, mut output) =
                create_owned_staging_file(staging_directory, &file_name)?;
            owned_paths.push(staged_path.clone());

            let mut digest = Sha256::new();
            let mut actual_size = 0_u64;
            let mut header = [0_u8; ENTRY_HEADER_BYTES];
            let mut header_len = 0_usize;
            loop {
                let read = entry.read(&mut buffer).map_err(|error| {
                    unsafe_package(format!(
                        "cannot decode selected asset {normalized}: {error}"
                    ))
                })?;
                if read == 0 {
                    break;
                }
                actual_size = actual_size
                    .checked_add(
                        u64::try_from(read)
                            .map_err(|_| unsafe_package("selected asset size overflow"))?,
                    )
                    .ok_or_else(|| unsafe_package("selected asset size overflow"))?;
                if actual_size > entry.size() || actual_size > component.size_bytes {
                    return Err(unsafe_package(format!(
                        "selected asset exceeds its declared size: {normalized}"
                    )));
                }
                if header_len < ENTRY_HEADER_BYTES {
                    let retained = (ENTRY_HEADER_BYTES - header_len).min(read);
                    header[header_len..header_len + retained].copy_from_slice(&buffer[..retained]);
                    header_len += retained;
                }
                digest.update(&buffer[..read]);
                output.write_all(&buffer[..read]).map_err(storage_error)?;
            }
            output.flush().map_err(storage_error)?;
            output.sync_all().map_err(storage_error)?;

            let actual_sha256 = hex::encode(digest.finalize());
            if actual_size != component.size_bytes
                || actual_sha256 != component.sha256
                || !media_signature_matches(&component.media_type, &header[..header_len])
            {
                return Err(unsafe_package(format!(
                    "selected asset bytes differ from the reviewed component: {normalized}"
                )));
            }
            let descriptor = package_asset_descriptor(component, &source_sha256)?;
            staged.push(StagedContentPackageAsset {
                component_id: component.id.clone(),
                staged_path,
                descriptor,
            });
        }

        if staged.len() != selected_assets.len() {
            return Err(unsafe_package(
                "one or more selected package assets were not found",
            ));
        }
        let after_hash = sha256_file(path)?;
        let after_size = path.metadata().map_err(storage_error)?.len();
        if after_hash != before_hash || after_size != before_size {
            return Err(unsafe_package(
                "content package changed while selected assets were staged",
            ));
        }
        staged.sort_by(|left, right| left.component_id.cmp(&right.component_id));
        Ok(())
    })();

    if let Err(error) = result {
        for path in owned_paths {
            let _ = fs::remove_file(path);
        }
        return Err(error);
    }
    Ok(staged)
}

/// Remove temporary files created by
/// [`stage_selected_content_package_assets`].
pub fn discard_staged_content_package_assets(
    staged: &[StagedContentPackageAsset],
    staging_directory: &Path,
) -> CoreResult<()> {
    for asset in staged {
        if asset.staged_path.parent() != Some(staging_directory)
            || !asset
                .staged_path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with(".content-package-asset-") && name.ends_with(".partial")
                })
        {
            return Err(unsafe_package(
                "selected asset staging path is outside its owned directory",
            ));
        }
    }
    let mut first_error = None;
    for asset in staged {
        if let Err(error) = fs::remove_file(&asset.staged_path)
            && error.kind() != std::io::ErrorKind::NotFound
            && first_error.is_none()
        {
            first_error = Some(storage_error(error));
        }
    }
    first_error.map_or(Ok(()), Err)
}

fn package_asset_descriptor(
    component: &ContentPackageComponent,
    source_sha256: &Sha256Digest,
) -> CoreResult<AssetDescriptor> {
    Ok(AssetDescriptor {
        id: AssetId::from(format!("sha256:{}", component.sha256)),
        sha256: Sha256Digest::parse(component.sha256.clone()).map_err(unsafe_package)?,
        media_type: component.media_type.clone(),
        role: package_asset_role(&component.media_type),
        name: component
            .path
            .rsplit('/')
            .next()
            .unwrap_or(&component.path)
            .to_owned(),
        size_bytes: component.size_bytes,
        width: None,
        height: None,
        duration_ms: None,
        source: AssetSource {
            kind: AssetSourceKind::LorepiaPackage,
            source_sha256: Some(source_sha256.clone()),
            logical_path: Some(component.path.clone()),
        },
    })
}

#[cfg(any(target_os = "android", target_os = "linux", target_vendor = "apple"))]
fn create_owned_staging_file(
    staging_directory: &Path,
    file_name: &str,
) -> CoreResult<(PathBuf, File)> {
    use rustix::fs::{Mode, OFlags, open, openat};

    let directory = open(
        staging_directory,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|error| storage_error(std::io::Error::from_raw_os_error(error.raw_os_error())))?;
    let file = openat(
        &directory,
        file_name,
        OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::from_raw_mode(0o600),
    )
    .map_err(|error| storage_error(std::io::Error::from_raw_os_error(error.raw_os_error())))?;
    Ok((staging_directory.join(file_name), File::from(file)))
}

#[cfg(windows)]
fn create_owned_staging_file(
    staging_directory: &Path,
    file_name: &str,
) -> CoreResult<(PathBuf, File)> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    let path = staging_directory.join(file_name);
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .share_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&path)
        .map_err(storage_error)?;
    Ok((path, file))
}

#[cfg(not(any(
    target_os = "android",
    target_os = "linux",
    target_vendor = "apple",
    windows
)))]
fn create_owned_staging_file(
    staging_directory: &Path,
    file_name: &str,
) -> CoreResult<(PathBuf, File)> {
    let path = staging_directory.join(file_name);
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .map_err(storage_error)?;
    Ok((path, file))
}

fn read_selected_json_entries(
    path: &Path,
    limits: ImportLimits,
    expected_source_sha256: &str,
    expected_source_size: u64,
    selected_components: &BTreeMap<String, SelectedJsonComponentExpectation<'_>>,
) -> CoreResult<BTreeMap<String, Vec<u8>>> {
    let mut file = File::open(path).map_err(storage_error)?;
    let before_size = file.metadata().map_err(storage_error)?.len();
    let before_hash = sha256_open_package_source(&mut file)?;
    if !selected_json_source_matches(
        &before_hash,
        before_size,
        expected_source_sha256,
        expected_source_size,
    ) {
        return Err(unsafe_package(
            "content package changed before selected documents were prepared",
        ));
    }
    let mut archive = ZipArchive::new(file).map_err(|error| unsafe_package(error.to_string()))?;
    if archive.len() > limits.max_entries {
        return Err(unsafe_package(
            "archive entry count changed before preparation",
        ));
    }
    let mut found = BTreeMap::new();
    let mut retained_total = 0_u64;
    let mut buffer = vec![0_u8; READ_BUFFER_BYTES];
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| unsafe_package(error.to_string()))?;
        if entry.is_dir() {
            continue;
        }
        let original = std::str::from_utf8(entry.name_raw())
            .map_err(|_| unsafe_package("archive entry path is not valid UTF-8"))?;
        let normalized = validate_archive_path(original)
            .map_err(|message| unsafe_package(message.to_owned()))?;
        let Some(expected) = selected_components.get(&normalized).copied() else {
            continue;
        };
        retained_total = retained_total
            .checked_add(entry.size())
            .ok_or_else(|| unsafe_package("selected JSON size overflow"))?;
        if retained_total > limits.max_total_uncompressed_bytes {
            return Err(unsafe_package(
                "selected JSON components exceed the total size limit",
            ));
        }
        let bytes =
            read_selected_json_entry(&mut entry, limits, &normalized, expected, &mut buffer)?;
        if found.insert(expected.id.to_owned(), bytes).is_some() {
            return Err(unsafe_package(format!(
                "selected JSON component appears more than once: {}",
                expected.id
            )));
        }
    }
    if found.len() != selected_components.len() {
        return Err(unsafe_package(
            "one or more selected JSON components were not found",
        ));
    }
    let mut file = archive.into_inner();
    let after_size = file.metadata().map_err(storage_error)?.len();
    let after_hash = sha256_open_package_source(&mut file)?;
    if !selected_json_source_matches(
        &after_hash,
        after_size,
        expected_source_sha256,
        expected_source_size,
    ) || before_hash != after_hash
        || before_size != after_size
    {
        return Err(unsafe_package(
            "content package changed while selected documents were prepared",
        ));
    }
    Ok(found)
}

fn selected_json_component_expectations<'a>(
    selected_components: &[(usize, &'a ContentPackageComponent)],
) -> CoreResult<BTreeMap<String, SelectedJsonComponentExpectation<'a>>> {
    let mut json_components = BTreeMap::new();
    for (_, component) in selected_components
        .iter()
        .filter(|(_, component)| is_json_kind(component.kind))
    {
        let expected = SelectedJsonComponentExpectation {
            id: &component.id,
            path: &component.path,
            sha256: &component.sha256,
            size_bytes: component.size_bytes,
        };
        if json_components
            .insert(component.path.clone(), expected)
            .is_some()
        {
            return Err(unsafe_package(
                "selected JSON component paths are not unique",
            ));
        }
    }
    Ok(json_components)
}

fn read_selected_json_entry<R: Read>(
    entry: &mut ZipFile<'_, R>,
    limits: ImportLimits,
    normalized: &str,
    expected: SelectedJsonComponentExpectation<'_>,
    buffer: &mut [u8],
) -> CoreResult<Vec<u8>> {
    if entry.size() != expected.size_bytes {
        return Err(unsafe_package(format!(
            "selected JSON bytes differ from the reviewed component: {normalized}"
        )));
    }
    if entry.size() > limits.max_entry_bytes || entry.size() > MAX_COMPONENT_JSON_BYTES {
        return Err(unsafe_package(format!(
            "selected JSON component exceeds preparation limits: {normalized}"
        )));
    }
    let capacity = usize::try_from(entry.size())
        .map_err(|_| unsafe_package("selected JSON is too large for this device"))?;
    let mut bytes = Vec::with_capacity(capacity);
    let mut digest = Sha256::new();
    let mut actual_size = 0_u64;
    loop {
        let read = entry.read(buffer).map_err(|error| {
            unsafe_package(format!("cannot decode selected {normalized}: {error}"))
        })?;
        if read == 0 {
            break;
        }
        actual_size = actual_size
            .checked_add(
                u64::try_from(read).map_err(|_| unsafe_package("selected JSON size overflow"))?,
            )
            .ok_or_else(|| unsafe_package("selected JSON size overflow"))?;
        digest.update(&buffer[..read]);
        bytes.extend_from_slice(&buffer[..read]);
        if actual_size > entry.size()
            || actual_size > expected.size_bytes
            || actual_size > MAX_COMPONENT_JSON_BYTES
        {
            return Err(unsafe_package(format!(
                "selected JSON exceeds its declared size: {normalized}"
            )));
        }
    }
    let actual_sha256 = hex::encode(digest.finalize());
    if !selected_json_component_matches(normalized, expected, actual_size, &actual_sha256) {
        return Err(unsafe_package(format!(
            "selected JSON bytes differ from the reviewed component: {normalized}"
        )));
    }
    Ok(bytes)
}

fn sha256_open_package_source(file: &mut File) -> CoreResult<String> {
    file.seek(SeekFrom::Start(0)).map_err(storage_error)?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; READ_BUFFER_BYTES];
    loop {
        let read = file.read(&mut buffer).map_err(storage_error)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    file.seek(SeekFrom::Start(0)).map_err(storage_error)?;
    Ok(hex::encode(digest.finalize()))
}

fn selected_json_source_matches(
    actual_sha256: &str,
    actual_size: u64,
    expected_sha256: &str,
    expected_size: u64,
) -> bool {
    actual_sha256 == expected_sha256 && actual_size == expected_size
}

fn selected_json_component_matches(
    normalized_path: &str,
    expected: SelectedJsonComponentExpectation<'_>,
    actual_size: u64,
    actual_sha256: &str,
) -> bool {
    normalized_path == expected.path
        && actual_size == expected.size_bytes
        && actual_sha256 == expected.sha256
}

#[allow(
    clippy::too_many_lines,
    reason = "one typed package decoder keeps provenance and inactive-import normalization atomic"
)]
fn decode_prepared_documents(
    source_component_ordinal: u32,
    component: &ContentPackageComponent,
    bytes: &[u8],
    package_provenance: &Provenance,
    package_redistribution_allowed: bool,
    documents: &mut Vec<PreparedContentDocumentEnvelope>,
    transformations: &mut Vec<ContentPackageTransformation>,
) -> CoreResult<()> {
    let mut component_documents = Vec::new();
    match component.kind {
        ContentPackageComponentKind::Prompt => {
            for mut preset in decode_document_list::<PromptPreset>(bytes, &["presets"])? {
                preset.metadata.provenance = package_provenance.clone();
                for block in &mut preset.blocks {
                    block.provenance = package_provenance.clone();
                }
                component_documents.push(PreparedContentDocument::PromptPreset(Box::new(preset)));
            }
        }
        ContentPackageComponentKind::Knowledge => {
            for mut book in decode_document_list::<KnowledgeBook>(bytes, &["books"])? {
                book.provenance = package_provenance.clone();
                for entry in &mut book.entries {
                    entry.provenance = package_provenance.clone();
                }
                component_documents.push(PreparedContentDocument::KnowledgeBook(Box::new(book)));
            }
        }
        ContentPackageComponentKind::Memory => {
            for mut profile in decode_document_list::<MemoryProfile>(bytes, &["profiles"])? {
                profile.provenance = package_provenance.clone();
                component_documents.push(PreparedContentDocument::MemoryProfile(Box::new(profile)));
            }
        }
        ContentPackageComponentKind::Transform => {
            for mut set in decode_document_list::<TransformSet>(bytes, &["sets", "transform_sets"])?
            {
                set.provenance = package_provenance.clone();
                transformations.push(ContentPackageTransformation {
                    component_id: component.id.clone(),
                    object_id: set.id.as_str().to_owned(),
                    field: "enabled".to_owned(),
                    before: set.enabled,
                    after: false,
                    reason: "imported transform sets require explicit activation".to_owned(),
                });
                set.imported_author_enabled = set.enabled;
                set.enabled = false;
                for rule in &mut set.rules {
                    rule.provenance = package_provenance.clone();
                    transformations.push(ContentPackageTransformation {
                        component_id: component.id.clone(),
                        object_id: rule.id.as_str().to_owned(),
                        field: "enabled".to_owned(),
                        before: rule.enabled,
                        after: false,
                        reason: "imported transform rules default inactive".to_owned(),
                    });
                    rule.imported_author_enabled = rule.enabled;
                    rule.enabled = false;
                    transformations.push(ContentPackageTransformation {
                        component_id: component.id.clone(),
                        object_id: rule.id.as_str().to_owned(),
                        field: "imported_enabled".to_owned(),
                        before: rule.imported_enabled,
                        after: false,
                        reason: "package input cannot pre-approve a transform rule".to_owned(),
                    });
                    rule.imported_enabled = false;
                }
                component_documents.push(PreparedContentDocument::TransformSet(Box::new(set)));
            }
        }
        ContentPackageComponentKind::Interaction => {
            for mut set in decode_document_list::<InteractionRuleSet>(
                bytes,
                &["sets", "interaction_rule_sets"],
            )? {
                set.provenance = package_provenance.clone();
                for rule in &mut set.rules {
                    rule.provenance = package_provenance.clone();
                    transformations.push(ContentPackageTransformation {
                        component_id: component.id.clone(),
                        object_id: rule.id.as_str().to_owned(),
                        field: "enabled".to_owned(),
                        before: rule.enabled,
                        after: false,
                        reason: "imported interaction rules require explicit activation".to_owned(),
                    });
                    rule.imported_author_enabled = rule.enabled;
                    rule.enabled = false;
                }
                component_documents
                    .push(PreparedContentDocument::InteractionRuleSet(Box::new(set)));
            }
        }
        ContentPackageComponentKind::ContentModule => {
            for mut module in
                decode_document_list::<ContentModule>(bytes, &["modules", "content_modules"])?
            {
                module.metadata.provenance = package_provenance.clone();
                module
                    .metadata
                    .author
                    .clone_from(&package_provenance.author);
                module.metadata.license = package_provenance
                    .license
                    .clone()
                    .unwrap_or_else(unknown_license);
                module.metadata.redistribution_allowed = package_redistribution_allowed;
                for block in &mut module.prompt_fragments {
                    block.provenance = package_provenance.clone();
                }
                normalize_imported_module_prompt_blocks(&mut module)?;
                validate_content_module_import_contract(&module)?;
                component_documents.push(PreparedContentDocument::ContentModule(Box::new(module)));
            }
        }
        ContentPackageComponentKind::Asset | ContentPackageComponentKind::Unsupported => {
            return Err(unsafe_package(
                "non-JSON component reached typed document preparation",
            ));
        }
    }
    for (ordinal, document) in component_documents.into_iter().enumerate() {
        let document_ordinal = u32::try_from(ordinal)
            .map_err(|_| unsupported("selected component contains too many documents"))?;
        let (document_kind, document_id) = prepared_document_identity(&document);
        let document_sha256 = hex::encode(Sha256::digest(serde_json::to_vec(&document).map_err(
            |error| unsupported(format!("prepared document cannot be encoded: {error}")),
        )?));
        documents.push(PreparedContentDocumentEnvelope {
            source_component_id: component.id.clone(),
            source_component_ordinal,
            document_ordinal,
            document_kind: document_kind.to_owned(),
            document_id: document_id.to_owned(),
            document_sha256,
            document,
        });
    }
    Ok(())
}

fn decode_document_list<T: DeserializeOwned>(
    bytes: &[u8],
    wrapper_keys: &[&str],
) -> CoreResult<Vec<T>> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|error| unsupported(format!("invalid selected component JSON: {error}")))?;
    let value = if let Value::Object(object) = &value {
        wrapper_keys
            .iter()
            .find_map(|key| object.get(*key))
            .cloned()
            .unwrap_or(value)
    } else {
        value
    };
    match value {
        Value::Array(values) => values
            .into_iter()
            .map(|value| {
                serde_json::from_value(value).map_err(|error| {
                    unsupported(format!("invalid typed package document: {error}"))
                })
            })
            .collect(),
        value => serde_json::from_value(value)
            .map(|value| vec![value])
            .map_err(|error| unsupported(format!("invalid typed package document: {error}"))),
    }
}

fn prepared_document_identity(document: &PreparedContentDocument) -> (&'static str, &str) {
    match document {
        PreparedContentDocument::PromptPreset(value) => ("prompt_preset", value.id.as_str()),
        PreparedContentDocument::KnowledgeBook(value) => ("knowledge_book", value.id.as_str()),
        PreparedContentDocument::MemoryProfile(value) => ("memory_profile", value.id.as_str()),
        PreparedContentDocument::TransformSet(value) => ("transform_set", value.id.as_str()),
        PreparedContentDocument::InteractionRuleSet(value) => {
            ("interaction_rule_set", value.id.as_str())
        }
        PreparedContentDocument::ContentModule(value) => ("content_module", value.id.as_str()),
    }
}

fn inspect_content_module_contract(
    bytes: &[u8],
) -> CoreResult<(Vec<AssetId>, Vec<ContentCapability>)> {
    let modules = decode_document_list::<ContentModule>(bytes, &["modules", "content_modules"])?;
    if modules.is_empty() {
        return Err(unsupported(
            "content module component must contain at least one module",
        ));
    }
    let mut asset_ids = Vec::new();
    let mut required_capabilities = Vec::new();
    for mut module in modules {
        normalize_imported_module_prompt_blocks(&mut module)?;
        validate_content_module_import_contract(&module)?;
        asset_ids.extend(module.asset_ids);
        required_capabilities.extend(
            module
                .required_capabilities
                .into_iter()
                .map(content_module_capability),
        );
    }
    asset_ids.sort();
    asset_ids.dedup();
    required_capabilities.push(ContentCapability("content_modules".to_owned()));
    required_capabilities.sort();
    required_capabilities.dedup();
    Ok((asset_ids, required_capabilities))
}

fn normalize_imported_module_prompt_blocks(module: &mut ContentModule) -> CoreResult<()> {
    if module.schema_version != CONTENT_MODULE_SCHEMA_VERSION {
        return Err(unsupported(format!(
            "unsupported content module schema version: {}",
            module.schema_version
        )));
    }
    for (index, block) in module.prompt_fragments.iter_mut().enumerate() {
        if block.kind == lorepia_domain::PromptBlockKind::LatestUserTurn {
            return Err(unsupported(format!(
                "content module prompt_fragments[{index}] cannot replace the latest user turn"
            )));
        }
        block.authority = InstructionAuthority::ImportedContent;
        if matches!(block.source, BlockSource::LatestUser | BlockSource::History) {
            if block.template.is_none() {
                return Err(unsupported(format!(
                    "content module prompt_fragments[{index}] cannot read conversation messages without a safe template"
                )));
            }
            block.source = BlockSource::Template;
            // Imported modules cannot read live conversation state. Once the
            // source is reduced to inert package-owned template text, retain
            // no dynamic history/latest-user semantic kind either.
            block.kind = lorepia_domain::PromptBlockKind::StaticInstruction;
            block.history_selector = None;
        }
        if matches!(
            block.placement_zone,
            PlacementZone::ApplicationPolicy | PlacementZone::LatestUser
        ) {
            block.placement_zone = PlacementZone::PresetInstruction;
        }
        if block.source == BlockSource::Template && block.template.is_none() {
            return Err(unsupported(format!(
                "content module prompt_fragments[{index}] template source is empty"
            )));
        }
    }
    Ok(())
}

fn validate_content_module_import_contract(module: &ContentModule) -> CoreResult<()> {
    if module.schema_version != CONTENT_MODULE_SCHEMA_VERSION {
        return Err(unsupported(format!(
            "unsupported content module schema version: {}",
            module.schema_version
        )));
    }
    module
        .validate()
        .map_err(|error| unsupported(format!("invalid declarative content module: {error}")))?;
    validate_module_identifier_list(
        "knowledge_book_ids",
        module
            .knowledge_book_ids
            .iter()
            .map(lorepia_domain::KnowledgeBookId::as_str),
    )?;
    validate_module_identifier_list(
        "transform_set_ids",
        module
            .transform_set_ids
            .iter()
            .map(lorepia_domain::TransformSetId::as_str),
    )?;
    validate_module_identifier_list(
        "interaction_rule_set_ids",
        module
            .interaction_rule_set_ids
            .iter()
            .map(lorepia_domain::InteractionRuleSetId::as_str),
    )?;
    validate_module_identifier_list("asset_ids", module.asset_ids.iter().map(AssetId::as_str))?;
    validate_module_identifier_list(
        "control_specs.id",
        module
            .control_specs
            .iter()
            .map(|control| control.id.as_str()),
    )?;
    validate_content_module_metadata(module)?;
    validate_content_module_capability_contract(module)
}

fn validate_module_identifier_list<'a>(
    label: &str,
    values: impl IntoIterator<Item = &'a str>,
) -> CoreResult<()> {
    let mut seen = BTreeSet::new();
    for value in values {
        if value.is_empty()
            || value.chars().count() > MAX_IDENTIFIER_CHARS
            || value.chars().any(|character| character == '\0')
        {
            return Err(unsupported(format!(
                "content module {label} contains an invalid identifier"
            )));
        }
        if !seen.insert(value) {
            return Err(unsupported(format!(
                "content module {label} contains duplicate identifiers"
            )));
        }
    }
    Ok(())
}

fn validate_content_module_metadata(module: &ContentModule) -> CoreResult<()> {
    if let Some(author) = module.metadata.author.as_deref() {
        validate_module_metadata_text("author", author, 0, MAX_NAME_CHARS)?;
    }
    if let Some(homepage) = module.metadata.homepage.as_deref() {
        validate_module_metadata_text("homepage", homepage, 0, MAX_MODULE_HOMEPAGE_CHARS)?;
    }
    if module.metadata.tags.len() > MAX_MODULE_METADATA_TAGS {
        return Err(unsupported(format!(
            "content module metadata contains more than {MAX_MODULE_METADATA_TAGS} tags"
        )));
    }
    let mut tags = BTreeSet::new();
    for tag in &module.metadata.tags {
        validate_module_metadata_text("tag", tag, 1, MAX_MODULE_METADATA_TAG_CHARS)?;
        if !tags.insert(tag.as_str()) {
            return Err(unsupported(
                "content module metadata contains duplicate tags",
            ));
        }
    }
    Ok(())
}

fn validate_module_metadata_text(
    label: &str,
    value: &str,
    minimum_chars: usize,
    maximum_chars: usize,
) -> CoreResult<()> {
    let chars = value.chars().count();
    if chars < minimum_chars
        || chars > maximum_chars
        || value.chars().any(|character| character == '\0')
    {
        return Err(unsupported(format!(
            "content module metadata {label} must contain between {minimum_chars} and {maximum_chars} characters and no NUL"
        )));
    }
    Ok(())
}

fn validate_content_module_capability_contract(module: &ContentModule) -> CoreResult<()> {
    let declared = module
        .required_capabilities
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let mut required = BTreeSet::new();
    if !module.prompt_fragments.is_empty() {
        required.insert(lorepia_domain::ContentCapability::PromptFragments);
    }
    if !module.knowledge_book_ids.is_empty() {
        required.insert(lorepia_domain::ContentCapability::Knowledge);
    }
    if !module.control_specs.is_empty() {
        required.insert(lorepia_domain::ContentCapability::Variables);
    }
    if !module.transform_set_ids.is_empty() {
        required.insert(lorepia_domain::ContentCapability::Transforms);
    }
    if !module.interaction_rule_set_ids.is_empty() {
        required.insert(lorepia_domain::ContentCapability::DeclarativeInteractions);
    }
    if !module.asset_ids.is_empty()
        && !declared.iter().any(|capability| {
            matches!(
                capability,
                lorepia_domain::ContentCapability::ImageAssets
                    | lorepia_domain::ContentCapability::AudioAssets
                    | lorepia_domain::ContentCapability::VideoAssets
                    | lorepia_domain::ContentCapability::AttachmentAssets
                    | lorepia_domain::ContentCapability::HighRiskAssets
            )
        })
    {
        return Err(unsupported(
            "content module asset references require an explicit asset capability",
        ));
    }
    if let Some(missing) = required
        .iter()
        .find(|capability| !declared.contains(capability))
    {
        return Err(unsupported(format!(
            "content module omits required capability: {}",
            content_module_capability_name(*missing)
        )));
    }
    Ok(())
}

const fn content_module_capability_name(
    capability: lorepia_domain::ContentCapability,
) -> &'static str {
    match capability {
        lorepia_domain::ContentCapability::PromptFragments => "prompt_fragments",
        lorepia_domain::ContentCapability::Knowledge => "knowledge",
        lorepia_domain::ContentCapability::Variables => "variables",
        lorepia_domain::ContentCapability::Transforms => "transforms",
        lorepia_domain::ContentCapability::DeclarativeInteractions => "declarative_interactions",
        lorepia_domain::ContentCapability::ImageAssets => "image_assets",
        lorepia_domain::ContentCapability::AudioAssets => "audio_assets",
        lorepia_domain::ContentCapability::VideoAssets => "video_assets",
        lorepia_domain::ContentCapability::AttachmentAssets => "attachment_assets",
        lorepia_domain::ContentCapability::HighRiskAssets => "high_risk_assets",
    }
}

fn content_module_capability(capability: lorepia_domain::ContentCapability) -> ContentCapability {
    let value = match capability {
        lorepia_domain::ContentCapability::PromptFragments => "prompt_presets",
        lorepia_domain::ContentCapability::Knowledge => "knowledge_books",
        lorepia_domain::ContentCapability::Variables => "variables",
        lorepia_domain::ContentCapability::Transforms => "safe_transforms",
        lorepia_domain::ContentCapability::DeclarativeInteractions => "declarative_interactions",
        lorepia_domain::ContentCapability::ImageAssets => "image_assets",
        lorepia_domain::ContentCapability::AudioAssets => "audio_assets",
        lorepia_domain::ContentCapability::VideoAssets => "video_assets",
        lorepia_domain::ContentCapability::AttachmentAssets => "attachment_assets",
        lorepia_domain::ContentCapability::HighRiskAssets => "high_risk_assets",
    };
    ContentCapability(value.to_owned())
}

fn validate_prepared_document_envelopes(
    documents: &[PreparedContentDocumentEnvelope],
) -> CoreResult<()> {
    let mut document_ids = BTreeSet::new();
    let mut document_hashes = BTreeSet::new();
    let mut component_ordinals = BTreeMap::<(&str, u32), u32>::new();
    for envelope in documents {
        let (expected_kind, expected_id) = prepared_document_identity(&envelope.document);
        let expected_sha256 = hex::encode(Sha256::digest(
            serde_json::to_vec(&envelope.document).map_err(|error| {
                unsupported(format!("prepared document cannot be encoded: {error}"))
            })?,
        ));
        if envelope.document_kind != expected_kind
            || envelope.document_id != expected_id
            || envelope.document_sha256 != expected_sha256
        {
            return Err(unsafe_package(
                "prepared document envelope differs from its typed document",
            ));
        }
        if !document_ids.insert(envelope.document_id.as_str()) {
            return Err(invalid(format!(
                "selected package documents contain a duplicate object id: {}",
                envelope.document_id
            )));
        }
        if !document_hashes.insert(envelope.document_sha256.as_str()) {
            return Err(invalid(
                "selected package documents contain duplicate canonical content",
            ));
        }
        let next = component_ordinals
            .entry((
                envelope.source_component_id.as_str(),
                envelope.source_component_ordinal,
            ))
            .or_insert(0);
        if envelope.document_ordinal != *next {
            return Err(unsafe_package(
                "prepared package document ordinals are not contiguous",
            ));
        }
        *next = next
            .checked_add(1)
            .ok_or_else(|| unsupported("selected component contains too many documents"))?;
    }
    Ok(())
}

fn package_asset_role(media_type: &str) -> AssetRole {
    if media_type.starts_with("audio/") {
        AssetRole::Audio
    } else if media_type.starts_with("video/") {
        AssetRole::Video
    } else if media_type.starts_with("image/") {
        AssetRole::Illustration
    } else {
        AssetRole::Attachment
    }
}

fn scan_package_archive(path: &Path, limits: ImportLimits) -> CoreResult<(Vec<ScannedEntry>, u64)> {
    let file = File::open(path).map_err(storage_error)?;
    let mut archive = ZipArchive::new(file).map_err(|error| unsafe_package(error.to_string()))?;
    if archive.len() > limits.max_entries {
        return Err(unsafe_package(format!(
            "archive has {} entries; maximum is {}",
            archive.len(),
            limits.max_entries
        )));
    }

    let mut seen_paths = HashMap::new();
    let mut declared_total = 0_u64;
    let mut actual_total = 0_u64;
    let mut entries = Vec::with_capacity(archive.len());
    let mut buffer = vec![0_u8; READ_BUFFER_BYTES];
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| unsafe_package(error.to_string()))?;
        let path = validate_package_entry(&entry, limits, &mut seen_paths, &mut declared_total)?;
        if entry.is_dir() {
            continue;
        }
        let scanned = read_package_entry(&mut entry, path, limits, &mut actual_total, &mut buffer)?;
        entries.push(scanned);
    }
    if entries.is_empty() {
        return Err(unsupported("content package archive is empty"));
    }
    Ok((entries, actual_total))
}

fn validate_package_entry<R: Read>(
    entry: &ZipFile<'_, R>,
    limits: ImportLimits,
    seen_paths: &mut HashMap<String, bool>,
    declared_total: &mut u64,
) -> CoreResult<String> {
    let original = std::str::from_utf8(entry.name_raw())
        .map_err(|_| unsafe_package("archive entry path is not valid UTF-8"))?;
    if original.len() > MAX_PATH_BYTES || original.chars().count() > MAX_PATH_CHARS {
        return Err(unsafe_package("archive entry path exceeds the path limit"));
    }
    let normalized =
        validate_archive_path(original).map_err(|message| unsafe_package(message.to_owned()))?;
    register_package_path(seen_paths, &normalized, entry.is_dir(), original)?;
    if entry.is_symlink() {
        return Err(unsafe_package(format!(
            "symbolic links are not allowed: {original}"
        )));
    }
    if entry.encrypted() {
        return Err(unsupported(format!(
            "encrypted archive entries are not supported: {original}"
        )));
    }
    if entry.is_dir() && entry.size() != 0 {
        return Err(unsafe_package(format!(
            "archive directory contains file data: {original}"
        )));
    }
    if entry.size() > limits.max_entry_bytes {
        return Err(unsafe_package(format!(
            "archive entry exceeds size limit: {original}"
        )));
    }
    if normalized == MANIFEST_PATH && entry.size() > MAX_MANIFEST_BYTES {
        return Err(unsupported(
            "manifest.json exceeds the 4 MiB metadata limit",
        ));
    }
    *declared_total = declared_total
        .checked_add(entry.size())
        .ok_or_else(|| unsafe_package("archive size overflow"))?;
    if *declared_total > limits.max_total_uncompressed_bytes {
        return Err(unsafe_package(
            "archive exceeds total uncompressed size limit",
        ));
    }
    let compressed = entry.compressed_size();
    if entry.size() > 0
        && (compressed == 0
            || entry.size() > compressed.saturating_mul(limits.max_compression_ratio))
    {
        return Err(unsafe_package(format!(
            "archive entry exceeds compression ratio limit: {original}"
        )));
    }
    Ok(normalized)
}

fn read_package_entry<R: Read>(
    entry: &mut ZipFile<'_, R>,
    path: String,
    limits: ImportLimits,
    actual_total: &mut u64,
    buffer: &mut [u8],
) -> CoreResult<ScannedEntry> {
    let retain_json = path == MANIFEST_PATH || is_json_component_path(&path);
    let json_too_large = retain_json && entry.size() > MAX_COMPONENT_JSON_BYTES;
    let mut json_bytes = (retain_json && !json_too_large).then(|| {
        usize::try_from(entry.size())
            .map(Vec::with_capacity)
            .unwrap_or_default()
    });
    let mut digest = Sha256::new();
    let mut header = [0_u8; ENTRY_HEADER_BYTES];
    let mut header_len = 0_usize;
    let mut entry_size = 0_u64;

    loop {
        let read = entry
            .read(buffer)
            .map_err(|error| unsafe_package(format!("cannot decode {path}: {error}")))?;
        if read == 0 {
            break;
        }
        entry_size = entry_size
            .checked_add(read as u64)
            .ok_or_else(|| unsafe_package("archive entry size overflow"))?;
        *actual_total = actual_total
            .checked_add(read as u64)
            .ok_or_else(|| unsafe_package("archive total size overflow"))?;
        if entry_size > limits.max_entry_bytes
            || *actual_total > limits.max_total_uncompressed_bytes
        {
            return Err(unsafe_package(
                "archive decoded data exceeds configured size limits",
            ));
        }
        digest.update(&buffer[..read]);
        if header_len < ENTRY_HEADER_BYTES {
            let copy_len = (ENTRY_HEADER_BYTES - header_len).min(read);
            header[header_len..header_len + copy_len].copy_from_slice(&buffer[..copy_len]);
            header_len += copy_len;
        }
        if let Some(bytes) = json_bytes.as_mut() {
            bytes.extend_from_slice(&buffer[..read]);
        }
    }
    if entry_size != entry.size() {
        return Err(unsafe_package(format!(
            "archive entry size does not match its header: {path}"
        )));
    }
    let mut referenced_asset_ids = Vec::new();
    let mut module_required_capabilities = Vec::new();
    let (json_hazards, json_error) = if retain_json && path != MANIFEST_PATH && !json_too_large {
        let retained = json_bytes
            .as_deref()
            .ok_or_else(|| unsupported("JSON component was not retained for validation"));
        match retained.and_then(inspect_json_component) {
            Ok(scan)
                if infer_component_kind(&path) == ContentPackageComponentKind::ContentModule =>
            {
                match inspect_content_module_contract(json_bytes.as_deref().ok_or_else(|| {
                    unsupported("content module JSON was not retained for validation")
                })?) {
                    Ok((asset_ids, required_capabilities)) => {
                        referenced_asset_ids = asset_ids;
                        module_required_capabilities = required_capabilities;
                        (scan.kinds, None)
                    }
                    Err(error) => (scan.kinds, Some(error.message)),
                }
            }
            Ok(scan) => (scan.kinds, None),
            Err(error) => (BTreeSet::new(), Some(error.message)),
        }
    } else {
        (BTreeSet::new(), None)
    };
    if path != MANIFEST_PATH {
        json_bytes = None;
    }
    Ok(ScannedEntry {
        path,
        size_bytes: entry_size,
        sha256: hex::encode(digest.finalize()),
        header,
        header_len,
        json_bytes,
        json_too_large,
        json_hazards,
        json_error,
        referenced_asset_ids,
        module_required_capabilities,
    })
}

fn parse_manifest(bytes: &[u8]) -> CoreResult<ParsedManifest> {
    let wire: ManifestWire = serde_json::from_slice(bytes)
        .map_err(|error| unsupported(format!("invalid manifest.json: {error}")))?;
    if wire.format != PACKAGE_FORMAT {
        return Err(unsupported(format!(
            "unsupported content package format: {}",
            wire.format
        )));
    }
    if wire.format_version != PACKAGE_FORMAT_VERSION {
        return Err(unsupported(format!(
            "unsupported content package format_version: {}",
            wire.format_version
        )));
    }
    validate_label("package_id", &wire.package_id)?;
    validate_label("name", &wire.name)?;
    validate_label("version", &wire.version)?;
    validate_optional_label("author", &wire.author)?;
    validate_label("license", &wire.license)?;
    if let Some(version) = &wire.required_app_version {
        validate_label("required_app_version", version)?;
    }
    validate_list_len("required_capabilities", wire.required_capabilities.len())?;
    validate_list_len("dependencies", wire.dependencies.len())?;
    validate_list_len("conflicts", wire.conflicts.len())?;
    validate_list_len("components", wire.components.len())?;
    if wire.content_hashes.len() > MAX_MANIFEST_LIST_ITEMS
        || wire.content_types.len() > MAX_MANIFEST_LIST_ITEMS
    {
        return Err(unsupported("manifest path metadata exceeds the item limit"));
    }

    let required_capabilities = normalize_capabilities(wire.required_capabilities)?;
    let dependencies = normalize_dependencies(wire.dependencies)?;
    let conflicts = normalize_conflicts(wire.conflicts)?;
    let content_hashes = normalize_hash_map(wire.content_hashes)?;
    let content_types = normalize_content_types(wire.content_types)?;
    let declarations = normalize_declarations(wire.components)?;
    let mut unsupported_fields = wire.extensions.keys().cloned().collect::<Vec<_>>();
    unsupported_fields.sort();
    for field in &unsupported_fields {
        validate_label("manifest extension field", field)?;
    }

    let signature_present = !wire.signature.is_null();
    let warnings = manifest_warnings(
        &wire.license,
        wire.redistribution_allowed,
        signature_present,
        &required_capabilities,
        &unsupported_fields,
    );

    Ok(ParsedManifest {
        manifest: ContentPackageManifest {
            format: wire.format,
            format_version: wire.format_version,
            package_id: wire.package_id,
            name: wire.name,
            version: wire.version,
            author: wire.author,
            license: wire.license,
            redistribution_allowed: wire.redistribution_allowed,
            required_app_version: wire.required_app_version,
            required_capabilities,
            dependencies,
            conflicts,
            content_hashes,
            content_types,
            signature_present,
        },
        declarations,
        unsupported_fields,
        warnings,
    })
}

fn manifest_warnings(
    license: &str,
    redistribution_allowed: bool,
    signature_present: bool,
    required_capabilities: &[ContentCapability],
    unsupported_fields: &[String],
) -> Vec<ImportWarning> {
    let mut warnings = Vec::new();
    if !has_usable_license(license) {
        warnings.push(ImportWarning {
            code: "license_unknown".to_owned(),
            message: "Package license is unknown; imported content is local-use only.".to_owned(),
        });
    }
    if redistribution_allowed && !has_usable_license(license) {
        warnings.push(ImportWarning {
            code: "redistribution_not_verified".to_owned(),
            message:
                "Manifest permits redistribution, but the declared license is not usable evidence."
                    .to_owned(),
        });
    }
    if signature_present {
        warnings.push(ImportWarning {
            code: "signature_unverified".to_owned(),
            message: "Package signature is retained as metadata but is not trusted by v1."
                .to_owned(),
        });
    }
    warnings.extend(
        required_capabilities
            .iter()
            .filter(|capability| !capability.is_supported())
            .map(|capability| ImportWarning {
                code: "unsupported_capability".to_owned(),
                message: format!("Unsupported package capability: {}", capability.0),
            }),
    );
    if !unsupported_fields.is_empty() {
        warnings.push(ImportWarning {
            code: "unsupported_manifest_fields".to_owned(),
            message: format!(
                "Manifest contains unsupported fields: {}",
                unsupported_fields.join(", ")
            ),
        });
    }
    warnings
}

fn build_component_inventory(
    entries: &[ScannedEntry],
    manifest: &ContentPackageManifest,
    declarations: &BTreeMap<String, ComponentDeclarationWire>,
    warnings: &mut Vec<ImportWarning>,
    blocked_reasons: &mut Vec<String>,
) -> CoreResult<Vec<ContentPackageComponent>> {
    let unsupported_package_capabilities = manifest
        .required_capabilities
        .iter()
        .filter(|capability| !capability.is_supported())
        .cloned()
        .collect::<Vec<_>>();
    let mut components = Vec::with_capacity(entries.len().saturating_sub(1));
    let mut declared_component_paths = declarations.keys().cloned().collect::<BTreeSet<_>>();

    for entry in entries {
        if entry.path == MANIFEST_PATH {
            continue;
        }
        declared_component_paths.remove(&entry.path);
        let declaration = declarations.get(&entry.path);
        components.push(build_component(
            entry,
            manifest,
            declaration,
            &unsupported_package_capabilities,
            warnings,
            blocked_reasons,
        )?);
    }

    for missing in declared_component_paths {
        blocked_reasons.push(format!(
            "Manifest component declaration has no archive entry: {missing}"
        ));
    }
    components.sort_by(|left, right| left.id.cmp(&right.id));
    if components.windows(2).any(|pair| pair[0].id == pair[1].id) {
        blocked_reasons.push("Manifest component ids must be unique".to_owned());
    }
    Ok(components)
}

fn build_component(
    entry: &ScannedEntry,
    manifest: &ContentPackageManifest,
    declaration: Option<&ComponentDeclarationWire>,
    unsupported_package_capabilities: &[ContentCapability],
    warnings: &mut Vec<ImportWarning>,
    blocked_reasons: &mut Vec<String>,
) -> CoreResult<ContentPackageComponent> {
    let inferred_kind = infer_component_kind(&entry.path);
    let kind = declaration
        .and_then(|value| value.kind.as_deref())
        .map(parse_component_kind)
        .transpose()?
        .unwrap_or(inferred_kind);
    let id = declaration.map_or_else(|| entry.path.clone(), |value| value.id.clone());
    validate_label("component id", &id)?;
    let media_type = declaration
        .and_then(|value| value.media_type.as_deref())
        .or_else(|| manifest.content_types.get(&entry.path).map(String::as_str))
        .map(normalize_media_type)
        .transpose()?
        .unwrap_or_else(|| default_media_type(kind, &entry.path).to_owned());
    let mut required_capabilities = declaration
        .map(|value| normalize_capabilities(value.required_capabilities.clone()))
        .transpose()?
        .unwrap_or_default();
    required_capabilities.extend(entry.module_required_capabilities.clone());
    required_capabilities.sort();
    required_capabilities.dedup();
    let (mut state, mut inactive_reasons) = classify_component_safety(
        entry,
        manifest.content_hashes.get(&entry.path).map(String::as_str),
        inferred_kind,
        kind,
        &media_type,
        &required_capabilities,
        unsupported_package_capabilities,
        warnings,
        blocked_reasons,
    );
    for capability in &required_capabilities {
        let covered = manifest.required_capabilities.contains(capability)
            || (capability.0.ends_with("_assets")
                && capability.0 != "high_risk_assets"
                && manifest
                    .required_capabilities
                    .contains(&ContentCapability("media_assets".to_owned())));
        if !covered {
            state = ContentPackageComponentState::Quarantined;
            inactive_reasons.push(format!(
                "component capability is not declared by the manifest: {}",
                capability.0
            ));
            blocked_reasons.push(format!(
                "Component {} requires undeclared capability {}",
                id, capability.0
            ));
        }
    }
    inactive_reasons.sort();
    inactive_reasons.dedup();
    let mut depends_on = declaration.map_or_else(Vec::new, |value| value.depends_on.clone());
    let mut conflicts_with =
        declaration.map_or_else(Vec::new, |value| value.conflicts_with.clone());
    normalize_component_ids(&mut depends_on)?;
    normalize_component_ids(&mut conflicts_with)?;
    Ok(ContentPackageComponent {
        id,
        path: entry.path.clone(),
        kind,
        media_type,
        sha256: entry.sha256.clone(),
        size_bytes: entry.size_bytes,
        state,
        inactive_reasons,
        required_capabilities,
        depends_on,
        conflicts_with,
        referenced_asset_ids: entry.referenced_asset_ids.clone(),
    })
}

#[allow(clippy::too_many_arguments)]
fn classify_component_safety(
    entry: &ScannedEntry,
    expected_hash: Option<&str>,
    inferred_kind: ContentPackageComponentKind,
    kind: ContentPackageComponentKind,
    media_type: &str,
    required_capabilities: &[ContentCapability],
    unsupported_package_capabilities: &[ContentCapability],
    warnings: &mut Vec<ImportWarning>,
    blocked_reasons: &mut Vec<String>,
) -> (ContentPackageComponentState, Vec<String>) {
    let mut inactive_reasons = Vec::new();
    let mut state = ContentPackageComponentState::Selectable;
    if inferred_kind == ContentPackageComponentKind::Unsupported {
        state = classify_unsupported_path(&entry.path);
        inactive_reasons.push(if state == ContentPackageComponentState::Quarantined {
            "unsupported executable or active-content path is quarantined".to_owned()
        } else {
            "path is outside the supported v1 component layout".to_owned()
        });
    }
    if kind != inferred_kind && inferred_kind != ContentPackageComponentKind::Unsupported {
        state = ContentPackageComponentState::Quarantined;
        inactive_reasons
            .push("declared component kind does not match its fixed package directory".to_owned());
    }
    classify_component_integrity(
        entry,
        expected_hash,
        kind,
        media_type,
        &mut state,
        &mut inactive_reasons,
        warnings,
        blocked_reasons,
    );
    for capability in required_capabilities
        .iter()
        .chain(unsupported_package_capabilities)
        .filter(|capability| !capability.is_supported())
    {
        if state == ContentPackageComponentState::Selectable {
            state = ContentPackageComponentState::InactiveUnsupported;
        }
        inactive_reasons.push(format!("unsupported capability: {}", capability.0));
    }
    classify_json_component(entry, kind, &mut state, &mut inactive_reasons);
    inactive_reasons.sort();
    inactive_reasons.dedup();
    (state, inactive_reasons)
}

#[allow(clippy::too_many_arguments)]
fn classify_component_integrity(
    entry: &ScannedEntry,
    expected_hash: Option<&str>,
    kind: ContentPackageComponentKind,
    media_type: &str,
    state: &mut ContentPackageComponentState,
    inactive_reasons: &mut Vec<String>,
    warnings: &mut Vec<ImportWarning>,
    blocked_reasons: &mut Vec<String>,
) {
    if expected_hash.is_none() {
        *state = ContentPackageComponentState::Quarantined;
        inactive_reasons.push("component is not covered by manifest content_hashes".to_owned());
        blocked_reasons.push(format!(
            "Manifest content_hashes does not cover component: {}",
            entry.path
        ));
    } else if expected_hash.is_some_and(|hash| hash != entry.sha256) {
        *state = ContentPackageComponentState::Quarantined;
        inactive_reasons.push("component SHA-256 does not match the manifest".to_owned());
        blocked_reasons.push(format!("Component hash mismatch: {}", entry.path));
    }
    if let Some(reason) = validate_component_media(kind, media_type, entry) {
        *state = ContentPackageComponentState::Quarantined;
        inactive_reasons.push(reason.clone());
        warnings.push(ImportWarning {
            code: "mime_mismatch".to_owned(),
            message: format!("{}: {reason}", entry.path),
        });
    }
}

fn classify_json_component(
    entry: &ScannedEntry,
    kind: ContentPackageComponentKind,
    state: &mut ContentPackageComponentState,
    inactive_reasons: &mut Vec<String>,
) {
    if !is_json_kind(kind) {
        return;
    }
    if entry.json_too_large {
        if *state == ContentPackageComponentState::Selectable {
            *state = ContentPackageComponentState::InactiveUnsupported;
        }
        inactive_reasons.push("JSON component exceeds the 8 MiB inspection limit".to_owned());
    } else if let Some(error) = &entry.json_error {
        *state = ContentPackageComponentState::Quarantined;
        inactive_reasons.push(error.clone());
    } else if !entry.json_hazards.is_empty() {
        *state = ContentPackageComponentState::Quarantined;
        inactive_reasons.extend(
            entry
                .json_hazards
                .iter()
                .map(|hazard| hazard_reason(*hazard).to_owned()),
        );
    }
}

fn validate_inventory_relationships(
    components: &[ContentPackageComponent],
    blocked_reasons: &mut Vec<String>,
) {
    let ids = components
        .iter()
        .map(|component| component.id.as_str())
        .collect::<BTreeSet<_>>();
    for component in components {
        for dependency in &component.depends_on {
            if !ids.contains(dependency.as_str()) {
                blocked_reasons.push(format!(
                    "Component {} depends on missing component {dependency}",
                    component.id
                ));
            }
        }
        for conflict in &component.conflicts_with {
            if !ids.contains(conflict.as_str()) {
                blocked_reasons.push(format!(
                    "Component {} conflicts with missing component {conflict}",
                    component.id
                ));
            }
        }
    }
}

fn bind_content_module_asset_dependencies(
    components: &mut [ContentPackageComponent],
    blocked_reasons: &mut Vec<String>,
) {
    let assets = components
        .iter()
        .filter(|component| component.kind == ContentPackageComponentKind::Asset)
        .fold(
            BTreeMap::<AssetId, Vec<(String, ContentPackageComponentState, String)>>::new(),
            |mut assets, component| {
                assets
                    .entry(AssetId::from(format!("sha256:{}", component.sha256)))
                    .or_default()
                    .push((
                        component.id.clone(),
                        component.state,
                        component.media_type.clone(),
                    ));
                assets
            },
        );
    for component in components
        .iter_mut()
        .filter(|component| component.kind == ContentPackageComponentKind::ContentModule)
    {
        for asset_id in component.referenced_asset_ids.clone() {
            let candidates = assets.get(&asset_id).map(Vec::as_slice).unwrap_or_default();
            match candidates {
                [(asset_component_id, ContentPackageComponentState::Selectable, media_type)] => {
                    let required_capability = asset_media_capability(media_type);
                    if component
                        .required_capabilities
                        .contains(&required_capability)
                    {
                        component.depends_on.push(asset_component_id.clone());
                    } else {
                        component.state = ContentPackageComponentState::Quarantined;
                        component.inactive_reasons.push(format!(
                            "content module asset capability does not match {media_type}: {}",
                            asset_id.as_str()
                        ));
                        blocked_reasons.push(format!(
                            "Component {} lacks {} for asset {}",
                            component.id,
                            required_capability.0,
                            asset_id.as_str()
                        ));
                    }
                }
                [(_, _, _)] => {
                    component.state = ContentPackageComponentState::Quarantined;
                    component.inactive_reasons.push(format!(
                        "content module asset is not selectable: {}",
                        asset_id.as_str()
                    ));
                    blocked_reasons.push(format!(
                        "Component {} references inactive asset {}",
                        component.id,
                        asset_id.as_str()
                    ));
                }
                [] => {
                    component.state = ContentPackageComponentState::Quarantined;
                    component.inactive_reasons.push(format!(
                        "content module asset is absent: {}",
                        asset_id.as_str()
                    ));
                    blocked_reasons.push(format!(
                        "Component {} references missing asset {}",
                        component.id,
                        asset_id.as_str()
                    ));
                }
                _ => {
                    component.state = ContentPackageComponentState::Quarantined;
                    component.inactive_reasons.push(format!(
                        "content module asset is ambiguous: {}",
                        asset_id.as_str()
                    ));
                    blocked_reasons.push(format!(
                        "Component {} references duplicate asset {}",
                        component.id,
                        asset_id.as_str()
                    ));
                }
            }
        }
        component.depends_on.sort();
        component.depends_on.dedup();
        component.inactive_reasons.sort();
        component.inactive_reasons.dedup();
    }
}

fn asset_media_capability(media_type: &str) -> ContentCapability {
    let capability = if media_type.starts_with("image/") {
        "image_assets"
    } else if media_type.starts_with("audio/") {
        "audio_assets"
    } else if media_type.starts_with("video/") {
        "video_assets"
    } else {
        "attachment_assets"
    };
    ContentCapability(capability.to_owned())
}

fn validate_manifest_coverage(
    entries: &[ScannedEntry],
    manifest: &ContentPackageManifest,
    blocked_reasons: &mut Vec<String>,
) {
    let entry_paths = entries
        .iter()
        .filter(|entry| entry.path != MANIFEST_PATH)
        .map(|entry| entry.path.as_str())
        .collect::<BTreeSet<_>>();
    for path in manifest.content_hashes.keys() {
        if path != MANIFEST_PATH && !entry_paths.contains(path.as_str()) {
            blocked_reasons.push(format!(
                "Manifest content_hashes references a missing entry: {path}"
            ));
        }
    }
    for path in manifest.content_types.keys() {
        if !entry_paths.contains(path.as_str()) {
            blocked_reasons.push(format!(
                "Manifest content_types references a missing entry: {path}"
            ));
        }
    }
}

fn normalize_hash_map(input: BTreeMap<String, String>) -> CoreResult<BTreeMap<String, String>> {
    let mut normalized = BTreeMap::new();
    for (path, hash) in input {
        let path = normalize_manifest_path(&path)?;
        let hash = normalize_sha256(&hash)?;
        if normalized.insert(path.clone(), hash).is_some() {
            return Err(unsupported(format!(
                "manifest paths collide after normalization: {path}"
            )));
        }
    }
    Ok(normalized)
}

fn normalize_content_types(
    input: BTreeMap<String, String>,
) -> CoreResult<BTreeMap<String, String>> {
    let mut normalized = BTreeMap::new();
    for (path, media_type) in input {
        let path = normalize_manifest_path(&path)?;
        let media_type = normalize_media_type(&media_type)?;
        if normalized.insert(path.clone(), media_type).is_some() {
            return Err(unsupported(format!(
                "manifest paths collide after normalization: {path}"
            )));
        }
    }
    Ok(normalized)
}

fn normalize_declarations(
    declarations: Vec<ComponentDeclarationWire>,
) -> CoreResult<BTreeMap<String, ComponentDeclarationWire>> {
    let mut normalized = BTreeMap::new();
    let mut ids = BTreeSet::new();
    for mut declaration in declarations {
        validate_label("component id", &declaration.id)?;
        if !ids.insert(declaration.id.clone()) {
            return Err(unsupported(format!(
                "duplicate manifest component id: {}",
                declaration.id
            )));
        }
        declaration.path = normalize_manifest_path(&declaration.path)?;
        if let Some(kind) = declaration.kind.as_deref() {
            parse_component_kind(kind)?;
        }
        if let Some(media_type) = declaration.media_type.as_deref() {
            declaration.media_type = Some(normalize_media_type(media_type)?);
        }
        normalize_component_ids(&mut declaration.depends_on)?;
        normalize_component_ids(&mut declaration.conflicts_with)?;
        normalize_capabilities(declaration.required_capabilities.clone())?;
        if normalized
            .insert(declaration.path.clone(), declaration)
            .is_some()
        {
            return Err(unsupported(
                "duplicate manifest component path after normalization",
            ));
        }
    }
    Ok(normalized)
}

fn normalize_dependencies(
    dependencies: Vec<DependencyWire>,
) -> CoreResult<Vec<ContentPackageDependency>> {
    let mut normalized = dependencies
        .into_iter()
        .map(|dependency| match dependency {
            DependencyWire::Id(package_id) => ContentPackageDependency {
                package_id,
                version_requirement: None,
                optional: false,
            },
            DependencyWire::Detail {
                package_id,
                version_requirement,
                optional,
            } => ContentPackageDependency {
                package_id,
                version_requirement,
                optional,
            },
        })
        .collect::<Vec<_>>();
    for dependency in &normalized {
        validate_label("dependency package_id", &dependency.package_id)?;
        if let Some(requirement) = &dependency.version_requirement {
            validate_label("dependency version requirement", requirement)?;
        }
    }
    normalized.sort_by(|left, right| {
        left.package_id
            .cmp(&right.package_id)
            .then_with(|| left.version_requirement.cmp(&right.version_requirement))
    });
    normalized.dedup();
    Ok(normalized)
}

fn normalize_conflicts(conflicts: Vec<ConflictWire>) -> CoreResult<Vec<PackageConflict>> {
    let mut normalized = conflicts
        .into_iter()
        .map(|conflict| match conflict {
            ConflictWire::Id(package_id) => PackageConflict {
                package_id,
                version_requirement: None,
                reason: None,
            },
            ConflictWire::Detail {
                package_id,
                version_requirement,
                reason,
            } => PackageConflict {
                package_id,
                version_requirement,
                reason,
            },
        })
        .collect::<Vec<_>>();
    for conflict in &normalized {
        validate_label("conflict package_id", &conflict.package_id)?;
        if let Some(requirement) = &conflict.version_requirement {
            validate_label("conflict version requirement", requirement)?;
        }
        if let Some(reason) = &conflict.reason {
            validate_label("conflict reason", reason)?;
        }
    }
    normalized.sort_by(|left, right| {
        left.package_id
            .cmp(&right.package_id)
            .then_with(|| left.version_requirement.cmp(&right.version_requirement))
    });
    normalized.dedup();
    Ok(normalized)
}

fn normalize_capabilities(values: Vec<String>) -> CoreResult<Vec<ContentCapability>> {
    let mut capabilities = values
        .into_iter()
        .map(|value| {
            validate_label("capability", &value)?;
            Ok(ContentCapability(value))
        })
        .collect::<CoreResult<Vec<_>>>()?;
    capabilities.sort();
    capabilities.dedup();
    Ok(capabilities)
}

fn normalize_component_ids(ids: &mut Vec<String>) -> CoreResult<()> {
    validate_list_len("component relationship", ids.len())?;
    for id in ids.iter() {
        validate_label("component relationship id", id)?;
    }
    ids.sort();
    ids.dedup();
    Ok(())
}

fn normalize_manifest_path(path: &str) -> CoreResult<String> {
    if path.len() > MAX_PATH_BYTES || path.chars().count() > MAX_PATH_CHARS {
        return Err(unsupported("manifest path exceeds the path limit"));
    }
    validate_archive_path(path).map_err(|message| unsupported(message.to_owned()))
}

fn parse_component_kind(value: &str) -> CoreResult<ContentPackageComponentKind> {
    match value {
        "prompt" => Ok(ContentPackageComponentKind::Prompt),
        "knowledge" => Ok(ContentPackageComponentKind::Knowledge),
        "memory" => Ok(ContentPackageComponentKind::Memory),
        "transform" | "transforms" => Ok(ContentPackageComponentKind::Transform),
        "interaction" | "interactions" => Ok(ContentPackageComponentKind::Interaction),
        "module" | "modules" | "content_module" | "content_modules" => {
            Ok(ContentPackageComponentKind::ContentModule)
        }
        "asset" | "assets" => Ok(ContentPackageComponentKind::Asset),
        "unsupported" => Ok(ContentPackageComponentKind::Unsupported),
        _ => Err(unsupported(format!(
            "unsupported manifest component kind: {value}"
        ))),
    }
}

fn infer_component_kind(path: &str) -> ContentPackageComponentKind {
    let Some((root, _)) = path.split_once('/') else {
        return ContentPackageComponentKind::Unsupported;
    };
    let is_json = Path::new(path)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"));
    match root {
        "prompt" if is_json => ContentPackageComponentKind::Prompt,
        "knowledge" if is_json => ContentPackageComponentKind::Knowledge,
        "memory" if is_json => ContentPackageComponentKind::Memory,
        "transforms" if is_json => ContentPackageComponentKind::Transform,
        "interactions" if is_json => ContentPackageComponentKind::Interaction,
        "modules" if is_json => ContentPackageComponentKind::ContentModule,
        "assets" => ContentPackageComponentKind::Asset,
        _ => ContentPackageComponentKind::Unsupported,
    }
}

fn is_json_component_path(path: &str) -> bool {
    is_json_kind(infer_component_kind(path))
}

fn is_json_kind(kind: ContentPackageComponentKind) -> bool {
    matches!(
        kind,
        ContentPackageComponentKind::Prompt
            | ContentPackageComponentKind::Knowledge
            | ContentPackageComponentKind::Memory
            | ContentPackageComponentKind::Transform
            | ContentPackageComponentKind::Interaction
            | ContentPackageComponentKind::ContentModule
    )
}

fn classify_unsupported_path(path: &str) -> ContentPackageComponentState {
    let root = path.split('/').next().unwrap_or_default();
    let extension = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    if matches!(root, "code" | "script" | "scripts" | "bin" | "web")
        || matches!(
            extension,
            "js" | "mjs"
                | "cjs"
                | "ts"
                | "py"
                | "rb"
                | "sh"
                | "bash"
                | "zsh"
                | "ps1"
                | "bat"
                | "cmd"
                | "exe"
                | "dll"
                | "dylib"
                | "so"
                | "wasm"
                | "class"
                | "jar"
                | "html"
                | "htm"
                | "svg"
        )
    {
        ContentPackageComponentState::Quarantined
    } else {
        ContentPackageComponentState::InactiveUnsupported
    }
}

fn default_media_type(kind: ContentPackageComponentKind, path: &str) -> &'static str {
    if is_json_kind(kind) {
        "application/json"
    } else {
        match Path::new(path)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
        {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            "avif" => "image/avif",
            "mp3" => "audio/mpeg",
            "wav" => "audio/wav",
            "ogg" => "audio/ogg",
            "mp4" => "video/mp4",
            "webm" => "video/webm",
            "pdf" => "application/pdf",
            "txt" => "text/plain",
            _ => "application/octet-stream",
        }
    }
}

fn normalize_media_type(value: &str) -> CoreResult<String> {
    if value.is_empty()
        || value.len() > 127
        || value.chars().any(char::is_control)
        || value.contains(';')
        || !value.contains('/')
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'+' | b'-' | b'.'))
    {
        return Err(unsupported("manifest contains an invalid media type"));
    }
    Ok(value.to_ascii_lowercase())
}

fn validate_component_media(
    kind: ContentPackageComponentKind,
    media_type: &str,
    entry: &ScannedEntry,
) -> Option<String> {
    if is_json_kind(kind) {
        return (media_type != "application/json")
            .then(|| "JSON component must use application/json".to_owned());
    }
    if kind != ContentPackageComponentKind::Asset {
        return None;
    }
    if !is_safe_asset_media_type(media_type) {
        return Some(format!(
            "unsupported or active asset media type: {media_type}"
        ));
    }
    if !media_signature_matches(media_type, &entry.header[..entry.header_len]) {
        return Some(format!(
            "asset bytes do not match declared media type {media_type}"
        ));
    }
    let relative = entry.path.strip_prefix("assets/sha256/");
    if let Some(relative) = relative {
        let name = relative.rsplit('/').next().unwrap_or(relative);
        let digest_name = name.split('.').next().unwrap_or(name);
        if digest_name.len() != 64
            || !digest_name.bytes().all(|byte| byte.is_ascii_hexdigit())
            || !digest_name.eq_ignore_ascii_case(&entry.sha256)
        {
            return Some(
                "content-addressed asset path does not match the asset SHA-256".to_owned(),
            );
        }
    } else {
        return Some("asset path must be below assets/sha256/".to_owned());
    }
    None
}

fn is_safe_asset_media_type(media_type: &str) -> bool {
    matches!(
        media_type,
        "image/png"
            | "image/jpeg"
            | "image/gif"
            | "image/webp"
            | "image/avif"
            | "audio/mpeg"
            | "audio/wav"
            | "audio/ogg"
            | "video/mp4"
            | "video/webm"
            | "application/pdf"
            | "text/plain"
    )
}

fn media_signature_matches(media_type: &str, header: &[u8]) -> bool {
    match media_type {
        "image/png" => header.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/jpeg" => header.starts_with(b"\xff\xd8\xff"),
        "image/gif" => header.starts_with(b"GIF87a") || header.starts_with(b"GIF89a"),
        "image/webp" => header.starts_with(b"RIFF") && header.get(8..12) == Some(b"WEBP"),
        "image/avif" => {
            header.get(4..8) == Some(b"ftyp")
                && matches!(header.get(8..12), Some(b"avif" | b"avis"))
        }
        "audio/mpeg" => {
            header.starts_with(b"ID3")
                || header
                    .get(..2)
                    .is_some_and(|bytes| bytes[0] == 0xff && bytes[1] & 0xe0 == 0xe0)
        }
        "audio/wav" => header.starts_with(b"RIFF") && header.get(8..12) == Some(b"WAVE"),
        "audio/ogg" => header.starts_with(b"OggS"),
        "video/mp4" => header.get(4..8) == Some(b"ftyp"),
        "video/webm" => header.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]),
        "application/pdf" => header.starts_with(b"%PDF-"),
        "text/plain" => !header.contains(&0),
        _ => false,
    }
}

fn inspect_json_component(bytes: &[u8]) -> CoreResult<HazardScan> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|error| unsupported(format!("invalid component JSON: {error}")))?;
    if !value.is_object() && !value.is_array() {
        return Err(unsupported(
            "component JSON must contain an object or array",
        ));
    }
    let mut scan = HazardScan::default();
    scan_json_hazards(&value, None, &mut scan)?;
    Ok(scan)
}

fn scan_json_hazards(value: &Value, key: Option<&str>, scan: &mut HazardScan) -> CoreResult<()> {
    scan.node_count = scan
        .node_count
        .checked_add(1)
        .ok_or_else(|| unsupported("component JSON node count overflow"))?;
    if scan.node_count > MAX_JSON_SCAN_NODES {
        return Err(unsupported(
            "component JSON exceeds the structural node limit",
        ));
    }
    match value {
        Value::Object(object) => {
            for (child_key, child) in object {
                let lower = child_key.to_ascii_lowercase();
                if matches!(lower.as_str(), "script" | "scripts" | "javascript")
                    || lower.starts_with("script_")
                    || lower.ends_with("_script")
                    || lower.ends_with("_scripts")
                {
                    scan.kinds.insert(HazardKind::Script);
                }
                if lower == "html" || lower.ends_with("_html") {
                    scan.kinds.insert(HazardKind::Html);
                }
                if lower == "code" || lower.ends_with("_code") {
                    scan.kinds.insert(HazardKind::Code);
                }
                if matches!(
                    lower.as_str(),
                    "shell" | "command" | "exec" | "network_request" | "filesystem"
                ) {
                    scan.kinds.insert(HazardKind::UnsafeAction);
                }
                scan_json_hazards(child, Some(&lower), scan)?;
            }
        }
        Value::Array(array) => {
            for child in array {
                scan_json_hazards(child, key, scan)?;
            }
        }
        Value::String(text) => {
            let lower = text.to_ascii_lowercase();
            if lower.starts_with("http://")
                || lower.starts_with("https://")
                || lower.starts_with("//")
            {
                scan.kinds.insert(HazardKind::ExternalUrl);
            }
            if lower.contains("<script")
                || lower.contains("<iframe")
                || lower.contains("javascript:")
                || lower.contains("data:text/html")
            {
                scan.kinds.insert(HazardKind::Html);
            }
            if key.is_some_and(|key| {
                matches!(
                    key,
                    "action" | "action_type" | "kind" | "type" | "operation"
                )
            }) && matches!(
                lower.as_str(),
                "execute"
                    | "exec"
                    | "shell"
                    | "run_script"
                    | "network_request"
                    | "fetch"
                    | "open_url"
                    | "read_file"
                    | "write_file"
                    | "javascript"
                    | "html"
            ) {
                scan.kinds.insert(HazardKind::UnsafeAction);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
    Ok(())
}

fn hazard_reason(kind: HazardKind) -> &'static str {
    match kind {
        HazardKind::Code => "component contains code-bearing fields and is quarantined",
        HazardKind::Script => "component contains script-bearing fields and is quarantined",
        HazardKind::Html => "component contains active HTML and is quarantined",
        HazardKind::ExternalUrl => {
            "component contains an external URL and is quarantined without downloading it"
        }
        HazardKind::UnsafeAction => {
            "component requests an unsupported executable or privileged action"
        }
    }
}

fn register_package_path(
    paths: &mut HashMap<String, bool>,
    normalized: &str,
    is_directory: bool,
    original: &str,
) -> CoreResult<()> {
    if paths.contains_key(normalized) {
        return Err(unsafe_package(format!(
            "archive path collides after normalization: {original}"
        )));
    }
    let mut ancestor = normalized;
    while let Some((parent, _)) = ancestor.rsplit_once('/') {
        if paths.get(parent).is_some_and(|directory| !directory) {
            return Err(unsafe_package(format!(
                "archive path descends through a file: {original}"
            )));
        }
        ancestor = parent;
    }
    if !is_directory {
        let descendant_prefix = format!("{normalized}/");
        if paths
            .keys()
            .any(|path| path.starts_with(&descendant_prefix))
        {
            return Err(unsafe_package(format!(
                "archive file path collides with a directory: {original}"
            )));
        }
    }
    paths.insert(normalized.to_owned(), is_directory);
    Ok(())
}

fn package_plan_hash(input: &PackagePlanHashInput<'_>) -> CoreResult<String> {
    stable_json_hash(input)
}

fn selection_hash(input: &SelectionHashInput<'_>) -> CoreResult<String> {
    stable_json_hash(input)
}

fn stable_json_hash(value: &impl Serialize) -> CoreResult<String> {
    let bytes = serde_json::to_vec(value).map_err(|error| {
        CoreError::new(
            CoreErrorCode::StorageCorrupted,
            format!("cannot serialize validated package plan: {error}"),
            false,
        )
    })?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn normalize_sha256(value: &str) -> CoreResult<String> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(unsupported(
            "manifest content hash must be a 64-character SHA-256 digest",
        ));
    }
    Ok(value.to_ascii_lowercase())
}

fn validate_label(field: &str, value: &str) -> CoreResult<()> {
    if value.is_empty() {
        return Err(unsupported(format!("{field} must not be empty")));
    }
    validate_optional_label(field, value)
}

fn validate_optional_label(field: &str, value: &str) -> CoreResult<()> {
    if value.chars().any(char::is_control)
        || value.len() > MAX_LABEL_BYTES
        || value.chars().count() > MAX_LABEL_CHARS
    {
        return Err(unsupported(format!(
            "{field} must be printable and at most {MAX_LABEL_BYTES} bytes or \
             {MAX_LABEL_CHARS} characters"
        )));
    }
    Ok(())
}

fn validate_list_len(field: &str, len: usize) -> CoreResult<()> {
    if len > MAX_MANIFEST_LIST_ITEMS {
        return Err(unsupported(format!(
            "{field} exceeds the {MAX_MANIFEST_LIST_ITEMS}-item limit"
        )));
    }
    Ok(())
}

fn has_usable_license(value: &str) -> bool {
    !value.is_empty()
        && !matches!(
            value.to_ascii_lowercase().as_str(),
            "licenseref-unknown" | "noassertion" | "none" | "unknown"
        )
}

fn unknown_license() -> String {
    "LicenseRef-Unknown".to_owned()
}

fn storage_error(error: std::io::Error) -> CoreError {
    CoreError::new(
        CoreErrorCode::StorageUnavailable,
        format!("content package access failed: {error}"),
        true,
    )
}

fn unsupported(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::UnsupportedContent, message, false)
}

fn unsafe_package(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::UnsafeArchive, message, false)
}

fn invalid(message: impl Into<String>) -> CoreError {
    CoreError::invalid(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_types_are_strict_and_lowercase() {
        assert_eq!(
            normalize_media_type("IMAGE/PNG").expect("valid"),
            "image/png"
        );
        for invalid in ["", "image", "image/png; charset=x", "image/\njson"] {
            assert!(normalize_media_type(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn hazard_scanner_finds_active_content_and_external_urls() {
        let value = serde_json::json!({
            "safe": "text",
            "script": "alert(1)",
            "asset_url": "https://invalid.example/image.png",
            "rules": [{"action": "network_request"}],
        });
        let mut scan = HazardScan::default();
        scan_json_hazards(&value, None, &mut scan).expect("scan");
        assert!(scan.kinds.contains(&HazardKind::Script));
        assert!(scan.kinds.contains(&HazardKind::ExternalUrl));
        assert!(scan.kinds.contains(&HazardKind::UnsafeAction));
    }

    #[test]
    fn hazard_scanner_does_not_treat_description_as_a_script_field() {
        let value = serde_json::json!({
            "description": "A normal inert content description.",
            "transcript": "A normal inert conversation transcript.",
        });
        let mut scan = HazardScan::default();
        scan_json_hazards(&value, None, &mut scan).expect("scan");
        assert!(!scan.kinds.contains(&HazardKind::Script));
    }

    #[test]
    fn package_hash_excludes_random_inspection_id() {
        let manifest = ContentPackageManifest {
            format: PACKAGE_FORMAT.into(),
            format_version: PACKAGE_FORMAT_VERSION,
            package_id: "synthetic".into(),
            name: "Synthetic".into(),
            version: "1.0.0".into(),
            author: String::new(),
            license: "MIT".into(),
            redistribution_allowed: true,
            required_app_version: None,
            required_capabilities: Vec::new(),
            dependencies: Vec::new(),
            conflicts: Vec::new(),
            content_hashes: BTreeMap::new(),
            content_types: BTreeMap::new(),
            signature_present: false,
        };
        let input = PackagePlanHashInput {
            manifest: &manifest,
            source_sha256: &"ab".repeat(32),
            source_size: 1,
            total_uncompressed_size: 1,
            components: &[],
            warnings: &[],
            blocked_reasons: &[],
            unsupported_manifest_fields: &[],
            local_use_only: false,
        };
        assert_eq!(
            package_plan_hash(&input).expect("first"),
            package_plan_hash(&input).expect("second")
        );
    }

    #[test]
    fn selected_json_source_binding_requires_the_approved_digest_and_size() {
        let source = b"ordinary package source";
        let approved_sha256 = hex::encode(Sha256::digest(source));
        let approved_size = source.len() as u64;
        assert!(selected_json_source_matches(
            &approved_sha256,
            approved_size,
            &approved_sha256,
            approved_size,
        ));
        assert!(!selected_json_source_matches(
            &approved_sha256,
            approved_size,
            &"00".repeat(32),
            approved_size,
        ));
        assert!(!selected_json_source_matches(
            &approved_sha256,
            approved_size + 1,
            &approved_sha256,
            approved_size,
        ));
    }

    #[test]
    fn selected_json_component_binding_requires_reviewed_identity_size_and_digest() {
        let bytes = br#"{"id":"ordinary-transform"}"#;
        let approved_sha256 = hex::encode(Sha256::digest(bytes));
        let expected = SelectedJsonComponentExpectation {
            id: "transform",
            path: "transforms/ordinary.json",
            sha256: &approved_sha256,
            size_bytes: bytes.len() as u64,
        };
        assert!(selected_json_component_matches(
            expected.path,
            expected,
            expected.size_bytes,
            expected.sha256,
        ));
        assert!(!selected_json_component_matches(
            "transforms/other.json",
            expected,
            expected.size_bytes,
            expected.sha256,
        ));
        assert!(!selected_json_component_matches(
            expected.path,
            expected,
            expected.size_bytes + 1,
            expected.sha256,
        ));
        assert!(!selected_json_component_matches(
            expected.path,
            expected,
            expected.size_bytes,
            &"00".repeat(32),
        ));
    }
}
