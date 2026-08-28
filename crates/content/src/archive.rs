use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

use lorepia_domain::{
    AssetDescriptor, AssetId, AssetRole, AssetSource, AssetSourceKind, CoreError, CoreErrorCode,
    CoreResult, ExtensionQuarantine, ExtensionQuarantineKind, ImportImagePreview, ImportLimits,
    ImportWarning, Sha256Digest, UnknownExtensionEntry, UnknownExtensionIndex,
};
use sha2::{Digest, Sha256};
use zip::{ZipArchive, read::ZipFile};

use crate::{StagedAsset, adapters, path::validate_archive_path, runtime};

const CARD_METADATA_PATH: &str = "card.json";
const ASSET_HEADER_BYTES: usize = 16;
const READ_BUFFER_BYTES: usize = 64 * 1024;
const CENTRAL_DIRECTORY_HEADER_MAGIC: &[u8; 4] = b"PK\x01\x02";
const CENTRAL_DIRECTORY_HEADER_BYTES: u64 = 46;
const END_OF_CENTRAL_DIRECTORY_MAGIC: &[u8; 4] = b"PK\x05\x06";
const END_OF_CENTRAL_DIRECTORY_BYTES: usize = 22;
const ZIP64_END_OF_CENTRAL_DIRECTORY_MAGIC: &[u8; 4] = b"PK\x06\x06";
const ZIP64_END_OF_CENTRAL_DIRECTORY_BYTES: usize = 56;
const ZIP64_END_OF_CENTRAL_DIRECTORY_MIN_RECORD_SIZE: u64 = 44;
const ZIP64_END_OF_CENTRAL_DIRECTORY_LOCATOR_MAGIC: &[u8; 4] = b"PK\x06\x07";
const ZIP64_END_OF_CENTRAL_DIRECTORY_LOCATOR_BYTES: u64 = 20;

pub(crate) struct ArchiveInspection {
    pub(crate) metadata: adapters::CardMetadata,
    pub(crate) asset_count: u32,
    pub(crate) total_uncompressed: u64,
    pub(crate) warnings: Vec<ImportWarning>,
    pub(crate) blocked_reasons: Vec<String>,
    pub(crate) representative_image: Option<ImportImagePreview>,
    staged_assets: Vec<StagedAsset>,
}

#[derive(Default)]
struct InspectionState {
    archive_paths: HashMap<String, bool>,
    declared_total: u64,
    total_uncompressed: u64,
    metadata_bytes: Option<Vec<u8>>,
    asset_count: u32,
    warnings: Vec<ImportWarning>,
    blocked_reasons: Vec<String>,
    representative_image: Option<ImportImagePreview>,
    asset_descriptors: Vec<AssetDescriptor>,
    archive_extensions: Vec<UnknownExtensionEntry>,
    runtime_documents: Vec<runtime::DecodedRuntimeDocument>,
    omitted_nonportable_entries: u32,
    reclassified_asset_count: u32,
    unsupported_asset_count: u32,
    first_unsupported_asset: Option<String>,
    staged_assets: Vec<StagedAsset>,
}

struct EntryPlan {
    name: String,
    logical_asset_id: String,
    extension: String,
    kind: EntryKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    Directory,
    Metadata,
    Asset,
    UnknownExtension(Option<ExtensionQuarantineKind>),
    RuntimeProbe,
    OmittedNonportable,
}

impl EntryPlan {
    fn is_metadata(&self) -> bool {
        self.kind == EntryKind::Metadata
    }

    fn is_asset(&self) -> bool {
        self.kind == EntryKind::Asset
    }

    fn is_unknown_extension(&self) -> bool {
        matches!(self.kind, EntryKind::UnknownExtension(_))
    }

    fn quarantine_kind(&self) -> Option<ExtensionQuarantineKind> {
        match self.kind {
            EntryKind::UnknownExtension(kind) => kind,
            _ => None,
        }
    }
}

struct EntryRead {
    metadata: Option<Vec<u8>>,
    asset_header: [u8; ASSET_HEADER_BYTES],
    asset_header_len: usize,
    size_bytes: u64,
    entry_sha256: Option<String>,
    staged_asset: Option<StagedAsset>,
}

pub(crate) fn inspect_archive(
    path: &Path,
    limits: ImportLimits,
    source_sha256: &str,
    nonportable_policy: adapters::NonPortableContentPolicy,
) -> CoreResult<ArchiveInspection> {
    inspect_archive_internal(path, limits, source_sha256, nonportable_policy, None)
}

pub(crate) fn stage_archive_assets(
    path: &Path,
    limits: ImportLimits,
    staging_directory: &Path,
    inspection_id: &str,
    nonportable_policy: adapters::NonPortableContentPolicy,
) -> CoreResult<Vec<StagedAsset>> {
    fs::create_dir_all(staging_directory).map_err(archive_io_error)?;
    let source_sha256 = crate::sha256_file(path)?;
    inspect_archive_internal(
        path,
        limits,
        &source_sha256,
        nonportable_policy,
        Some(AssetStaging {
            directory: staging_directory,
            inspection_id,
        }),
    )
    .map(|inspection| inspection.staged_assets)
}

#[derive(Clone, Copy)]
struct AssetStaging<'a> {
    directory: &'a Path,
    inspection_id: &'a str,
}

fn inspect_archive_internal(
    path: &Path,
    limits: ImportLimits,
    source_sha256: &str,
    nonportable_policy: adapters::NonPortableContentPolicy,
    asset_staging: Option<AssetStaging<'_>>,
) -> CoreResult<ArchiveInspection> {
    let mut file = File::open(path).map_err(archive_io_error)?;
    let declared_entries = preflight_zip_archive(&mut file, limits)?;
    file.seek(SeekFrom::Start(0)).map_err(archive_io_error)?;
    let mut archive = ZipArchive::new(file).map_err(|error| unsafe_archive(error.to_string()))?;
    if archive.len() != declared_entries {
        return Err(unsafe_archive(
            "archive central directory contains duplicate file names".to_owned(),
        ));
    }

    let mut state = InspectionState::default();
    let mut buffer = vec![0_u8; READ_BUFFER_BYTES];
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| unsafe_archive(error.to_string()))?;
        let asset_path = asset_staging.map(|staging| {
            staging.directory.join(format!(
                "inspection-{}-asset-{index}.partial",
                staging.inspection_id
            ))
        });
        if let Err(error) = inspect_entry(
            &mut entry,
            limits,
            &mut state,
            &mut buffer,
            asset_path.as_deref(),
            nonportable_policy,
        ) {
            cleanup_staged_assets(&state.staged_assets);
            return Err(error);
        }
    }

    let result = finalize_archive_inspection(
        &mut state,
        source_sha256,
        nonportable_policy,
        asset_staging,
        limits,
    );
    if result.is_err() {
        cleanup_staged_assets(&state.staged_assets);
    }
    result
}

fn finalize_archive_inspection(
    state: &mut InspectionState,
    source_sha256: &str,
    nonportable_policy: adapters::NonPortableContentPolicy,
    asset_staging: Option<AssetStaging<'_>>,
    limits: ImportLimits,
) -> CoreResult<ArchiveInspection> {
    let mut metadata = parse_archive_metadata(state, source_sha256, nonportable_policy)?;
    merge_runtime_documents(state, &mut metadata, source_sha256, asset_staging, limits)?;
    bind_archive_assets(state, &mut metadata, source_sha256)?;
    append_archive_summary_warnings(state, &metadata, nonportable_policy);
    state.warnings.sort_by(|left, right| {
        left.code
            .cmp(&right.code)
            .then_with(|| left.message.cmp(&right.message))
    });
    state.blocked_reasons.sort();
    state.blocked_reasons.dedup();
    Ok(ArchiveInspection {
        metadata,
        asset_count: state.asset_count,
        total_uncompressed: state.total_uncompressed,
        warnings: std::mem::take(&mut state.warnings),
        blocked_reasons: std::mem::take(&mut state.blocked_reasons),
        representative_image: state.representative_image.take(),
        staged_assets: std::mem::take(&mut state.staged_assets),
    })
}

fn merge_runtime_documents(
    state: &mut InspectionState,
    metadata: &mut adapters::CardMetadata,
    source_sha256: &str,
    asset_staging: Option<AssetStaging<'_>>,
    limits: ImportLimits,
) -> CoreResult<()> {
    for (document_index, document) in std::mem::take(&mut state.runtime_documents)
        .into_iter()
        .enumerate()
    {
        let runtime::DecodedRuntimeDocument {
            profile,
            knowledge_entries,
            embedded_assets,
        } = document;
        if metadata.content.knowledge_book.is_none()
            && let Some(entries) = knowledge_entries
        {
            metadata.content.knowledge_book = adapters::parse_runtime_knowledge_book(
                &entries,
                source_sha256,
                "Embedded knowledge",
            )?;
        }
        merge_runtime_profile(&mut metadata.content.runtime, profile);
        for (asset_index, asset) in embedded_assets.into_iter().enumerate() {
            project_runtime_asset(
                state,
                asset,
                document_index,
                asset_index,
                asset_staging,
                limits,
            )?;
        }
    }
    if !metadata.content.runtime.transforms.is_empty() {
        metadata.content.runtime.source_id = Some(format!("card-runtime:{source_sha256}"));
        metadata.content.runtime.transform_set_id = Some(lorepia_domain::TransformSetId::from(
            format!("card-transforms:{source_sha256}"),
        ));
    }
    Ok(())
}

fn project_runtime_asset(
    state: &mut InspectionState,
    asset: runtime::DecodedRuntimeAsset,
    document_index: usize,
    asset_index: usize,
    asset_staging: Option<AssetStaging<'_>>,
    limits: ImportLimits,
) -> CoreResult<()> {
    let size_bytes = u64::try_from(asset.bytes.len())
        .map_err(|_| unsafe_archive("runtime asset is too large for this device".to_owned()))?;
    if size_bytes == 0 || size_bytes > limits.max_entry_bytes {
        return Err(unsafe_archive(
            "runtime asset is empty or exceeds the per-entry size limit".to_owned(),
        ));
    }
    if state.asset_descriptors.len().saturating_add(1) > limits.max_entries {
        return Err(unsafe_archive(
            "archive and runtime assets exceed the entry-count limit".to_owned(),
        ));
    }
    let media_type =
        detect_asset_media_type(&asset.bytes[..asset.bytes.len().min(ASSET_HEADER_BYTES)]);
    let Some(media_type) = media_type else {
        return Err(unsafe_archive(format!(
            "runtime asset has an unsupported media signature: {}",
            asset.name
        )));
    };
    let sha256 = hex::encode(Sha256::digest(&asset.bytes));
    let logical_path = format!("runtime-assets/{document_index}/{asset_index}");
    state.asset_count = state
        .asset_count
        .checked_add(1)
        .ok_or_else(|| unsafe_archive("archive asset count overflow".to_owned()))?;
    if state.representative_image.is_none() && media_type.starts_with("image/") {
        state.representative_image = Some(ImportImagePreview {
            logical_asset_id: logical_path.clone(),
            media_type: media_type.to_owned(),
            size_bytes,
        });
    }
    state.asset_descriptors.push(AssetDescriptor {
        id: AssetId::from(format!("sha256:{sha256}")),
        sha256: Sha256Digest::parse(sha256.clone()).map_err(unsafe_archive)?,
        media_type: media_type.to_owned(),
        role: infer_asset_role(&asset.name, media_type),
        name: asset.name.clone(),
        size_bytes,
        width: None,
        height: None,
        duration_ms: None,
        source: AssetSource {
            kind: AssetSourceKind::ContentModule,
            source_sha256: None,
            logical_path: Some(logical_path.clone()),
        },
    });
    if let Some(staging) = asset_staging {
        let path = staging.directory.join(format!(
            "inspection-{}-runtime-asset-{document_index}-{asset_index}.partial",
            staging.inspection_id
        ));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(archive_io_error)?;
        file.write_all(&asset.bytes).map_err(archive_io_error)?;
        file.sync_all().map_err(archive_io_error)?;
        state.staged_assets.push(StagedAsset {
            original_path: logical_path,
            staged_path: path,
            sha256,
            media_type: media_type.to_owned(),
            size_bytes,
            signature_valid: true,
        });
    }
    Ok(())
}

fn merge_runtime_profile(
    target: &mut lorepia_domain::CharacterRuntimeProfile,
    mut incoming: lorepia_domain::CharacterRuntimeProfile,
) {
    target.transforms.append(&mut incoming.transforms);
    target.scripts.append(&mut incoming.scripts);
    if target.background_markup.is_empty() {
        target.background_markup = incoming.background_markup;
    }
    if target.additional_text.is_empty() {
        target.additional_text = incoming.additional_text;
    }
    if target.toggle_schema.is_empty() {
        target.toggle_schema = incoming.toggle_schema;
    }
    target.initial_variables.extend(incoming.initial_variables);
    target.metadata.extend(incoming.metadata);
    if target.source_id.is_none() {
        target.source_id = incoming.source_id;
    }
}

fn parse_archive_metadata(
    state: &mut InspectionState,
    source_sha256: &str,
    nonportable_policy: adapters::NonPortableContentPolicy,
) -> CoreResult<adapters::CardMetadata> {
    let result = state
        .metadata_bytes
        .take()
        .ok_or_else(|| {
            unsupported_archive("CHARX package must contain a root card.json".to_owned())
        })
        .and_then(|bytes| {
            adapters::parse_card_json_with_source_and_policy(
                &bytes,
                source_sha256,
                nonportable_policy,
            )
        });
    if result.is_err() {
        cleanup_staged_assets(&state.staged_assets);
    }
    result
}

fn bind_archive_assets(
    state: &mut InspectionState,
    metadata: &mut adapters::CardMetadata,
    source_sha256: &str,
) -> CoreResult<()> {
    let source_digest = Sha256Digest::parse(source_sha256.to_owned())
        .map_err(|message| CoreError::new(CoreErrorCode::StorageCorrupted, message, false))?;
    for descriptor in &mut state.asset_descriptors {
        let logical_path = descriptor.source.logical_path.as_deref().ok_or_else(|| {
            unsafe_archive("archive asset is missing its logical path".to_owned())
        })?;
        descriptor.id = archive_asset_descriptor_id(source_sha256, logical_path);
        descriptor.source.source_sha256 = Some(source_digest.clone());
    }
    if let Some(preferred_path) = metadata.preferred_image_path.as_deref()
        && let Some(descriptor) = state.asset_descriptors.iter().find(|descriptor| {
            descriptor.media_type.starts_with("image/")
                && descriptor
                    .source
                    .logical_path
                    .as_deref()
                    .is_some_and(|path| path == preferred_path)
        })
    {
        state.representative_image = Some(ImportImagePreview {
            logical_asset_id: preferred_path.to_owned(),
            media_type: descriptor.media_type.clone(),
            size_bytes: descriptor.size_bytes,
        });
    }
    metadata.content.assets = std::mem::take(&mut state.asset_descriptors);
    if !state.archive_extensions.is_empty() {
        let mut entries = std::mem::take(&mut metadata.content.unknown_extensions.entries);
        entries.append(&mut state.archive_extensions);
        metadata.content.unknown_extensions =
            UnknownExtensionIndex::try_new(Some(source_digest), entries).map_err(|message| {
                CoreError::new(CoreErrorCode::UnsupportedContent, message, false)
            })?;
    }
    Ok(())
}

fn archive_asset_descriptor_id(source_sha256: &str, logical_path: &str) -> AssetId {
    let mut digest = Sha256::new();
    digest.update(b"character-archive-asset-descriptor-v1\0");
    digest.update(source_sha256.as_bytes());
    digest.update([0]);
    digest.update(logical_path.as_bytes());
    AssetId::from(format!(
        "asset-descriptor:{}",
        hex::encode(digest.finalize())
    ))
}

fn append_archive_summary_warnings(
    state: &mut InspectionState,
    metadata: &adapters::CardMetadata,
    nonportable_policy: adapters::NonPortableContentPolicy,
) {
    if nonportable_policy == adapters::NonPortableContentPolicy::Omit
        && (state.omitted_nonportable_entries > 0
            || !metadata.unsupported_optional_fields.is_empty())
    {
        state.warnings.push(ImportWarning {
            code: "nonportable_content_omitted".to_owned(),
            message: "Nonportable card data was omitted; only standard character fields and supported media will be imported."
                .to_owned(),
        });
    }
    if state.reclassified_asset_count > 0 {
        state.warnings.push(ImportWarning {
            code: "media_type_reclassified".to_owned(),
            message: format!(
                "{} assets used a misleading file extension and will be imported using their verified media signatures.",
                state.reclassified_asset_count
            ),
        });
    }
    if state.unsupported_asset_count == 0 {
        return;
    }
    let example = state
        .first_unsupported_asset
        .as_deref()
        .map(|path| format!(" First rejected asset: {path}."))
        .unwrap_or_default();
    state.warnings.push(ImportWarning {
        code: "mime_mismatch".to_owned(),
        message: format!(
            "{} assets do not contain a supported media signature.{example}",
            state.unsupported_asset_count
        ),
    });
    state.blocked_reasons.push(format!(
        "asset file signature validation failed for {} assets",
        state.unsupported_asset_count
    ));
}

/// Detects a valid character-card ZIP that starts after another file payload.
///
/// The probe is based on the ZIP end record and a root `card.json`, not the
/// selected filename. It reads directory metadata only and never executes or
/// renders archive content.
pub(crate) fn has_embedded_character_archive(
    path: &Path,
    limits: ImportLimits,
) -> CoreResult<bool> {
    let mut file = File::open(path).map_err(archive_io_error)?;
    if !has_end_of_central_directory(&mut file)? {
        return Ok(false);
    }
    let declared_entries = preflight_zip_archive(&mut file, limits)?;
    file.seek(SeekFrom::Start(0)).map_err(archive_io_error)?;
    let mut archive = ZipArchive::new(file).map_err(|error| unsafe_archive(error.to_string()))?;
    if archive.len() != declared_entries {
        return Err(unsafe_archive(
            "archive central directory contains duplicate file names".to_owned(),
        ));
    }
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| unsafe_archive(error.to_string()))?;
        if !entry.is_dir() && entry.name_raw() == CARD_METADATA_PATH.as_bytes() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn has_end_of_central_directory(file: &mut File) -> CoreResult<bool> {
    let file_len = file.seek(SeekFrom::End(0)).map_err(archive_io_error)?;
    let max_tail_len = u64::from(u16::MAX) + END_OF_CENTRAL_DIRECTORY_BYTES as u64;
    let tail_len = file_len.min(max_tail_len);
    let tail_len_usize = usize::try_from(tail_len)
        .map_err(|_| unsafe_archive("archive tail is too large for this device".to_owned()))?;
    let mut tail = vec![0_u8; tail_len_usize];
    let tail_offset = i64::try_from(tail_len)
        .map_err(|_| unsafe_archive("archive tail offset is too large".to_owned()))?;
    file.seek(SeekFrom::End(-tail_offset))
        .and_then(|_| file.read_exact(&mut tail))
        .map_err(archive_io_error)?;
    Ok(find_end_of_central_directory(&tail).is_some())
}

pub(crate) fn preflight_zip_archive(file: &mut File, limits: ImportLimits) -> CoreResult<usize> {
    let file_len = file.seek(SeekFrom::End(0)).map_err(archive_io_error)?;
    let max_tail_len = u64::from(u16::MAX) + END_OF_CENTRAL_DIRECTORY_BYTES as u64;
    let tail_len = file_len.min(max_tail_len);
    let tail_len_usize = usize::try_from(tail_len)
        .map_err(|_| unsafe_archive("archive tail is too large for this device".to_owned()))?;
    let mut tail = vec![0_u8; tail_len_usize];
    let tail_offset = i64::try_from(tail_len)
        .map_err(|_| unsafe_archive("archive tail offset is too large".to_owned()))?;
    file.seek(SeekFrom::End(-tail_offset))
        .and_then(|_| file.read_exact(&mut tail))
        .map_err(archive_io_error)?;

    let (eocd_position, eocd) = find_end_of_central_directory(&tail)
        .ok_or_else(|| unsafe_archive("archive has no valid central directory".to_owned()))?;
    let tail_start = file_len
        .checked_sub(tail_len)
        .ok_or_else(|| unsafe_archive("archive central directory offset underflow".to_owned()))?;
    let eocd_offset = tail_start
        .checked_add(u64::try_from(eocd_position).map_err(|_| {
            unsafe_archive("archive central directory offset is too large".to_owned())
        })?)
        .ok_or_else(|| unsafe_archive("archive central directory offset overflow".to_owned()))?;
    let disk_number = read_u16(eocd, 4);
    let central_directory_disk = read_u16(eocd, 6);
    let entries_on_disk = read_u16(eocd, 8);
    let total_entries = read_u16(eocd, 10);
    let central_directory_size = read_u32(eocd, 12);
    let central_directory_offset = read_u32(eocd, 16);
    if disk_number != 0 || central_directory_disk != 0 || entries_on_disk != total_entries {
        return Err(unsupported_archive(
            "multi-disk ZIP archives are not supported".to_owned(),
        ));
    }

    let (raw_entries, raw_central_size, raw_central_offset, central_end) =
        if total_entries == u16::MAX || central_directory_offset == u32::MAX {
            read_zip64_central_directory(
                file,
                eocd_offset,
                entries_on_disk,
                total_entries,
                central_directory_size,
                central_directory_offset,
            )?
        } else {
            if central_directory_size == u32::MAX {
                return Err(unsafe_archive(
                    "ZIP32 central-directory size uses an invalid ZIP64 sentinel".to_owned(),
                ));
            }
            (
                u64::from(total_entries),
                u64::from(central_directory_size),
                u64::from(central_directory_offset),
                eocd_offset,
            )
        };

    if raw_entries > limits.max_entries as u64 {
        return Err(unsafe_archive(format!(
            "archive has {raw_entries} raw central-directory entries; maximum is {}",
            limits.max_entries
        )));
    }
    if raw_central_size > limits.max_source_bytes || raw_central_size > file_len {
        return Err(unsafe_archive(
            "archive central directory exceeds the configured source limit".to_owned(),
        ));
    }
    let minimum_central_size = raw_entries
        .checked_mul(CENTRAL_DIRECTORY_HEADER_BYTES)
        .ok_or_else(|| unsafe_archive("archive central-directory size overflow".to_owned()))?;
    let maximum_record_size = CENTRAL_DIRECTORY_HEADER_BYTES + 3 * u64::from(u16::MAX);
    let maximum_central_size = raw_entries
        .checked_mul(maximum_record_size)
        .ok_or_else(|| unsafe_archive("archive central-directory size overflow".to_owned()))?;
    if raw_central_size < minimum_central_size || raw_central_size > maximum_central_size {
        return Err(unsafe_archive(
            "archive central-directory count and size are inconsistent".to_owned(),
        ));
    }

    let central_start = central_end.checked_sub(raw_central_size).ok_or_else(|| {
        unsafe_archive("archive central directory extends before the file".to_owned())
    })?;
    if central_start < raw_central_offset {
        return Err(unsafe_archive(
            "archive central-directory offset is inconsistent".to_owned(),
        ));
    }
    validate_raw_central_directory(file, central_start, raw_central_size, raw_entries)?;
    file.seek(SeekFrom::Start(0)).map_err(archive_io_error)?;
    usize::try_from(raw_entries)
        .map_err(|_| unsafe_archive("archive entry count is too large for this device".to_owned()))
}

fn read_zip64_central_directory(
    file: &mut File,
    eocd_offset: u64,
    classic_entries_on_disk: u16,
    classic_total_entries: u16,
    classic_central_size: u32,
    classic_central_offset: u32,
) -> CoreResult<(u64, u64, u64, u64)> {
    let locator_offset = eocd_offset
        .checked_sub(ZIP64_END_OF_CENTRAL_DIRECTORY_LOCATOR_BYTES)
        .ok_or_else(|| unsafe_archive("ZIP64 locator does not fit in the archive".to_owned()))?;
    let mut locator = [0_u8; 20];
    read_exact_at(file, locator_offset, &mut locator)?;
    if locator.get(..4) != Some(ZIP64_END_OF_CENTRAL_DIRECTORY_LOCATOR_MAGIC) {
        return Err(unsafe_archive(
            "archive is missing its ZIP64 central-directory locator".to_owned(),
        ));
    }
    let locator_disk = read_u32(&locator, 4);
    let zip64_eocd_offset = read_u64(&locator, 8);
    let number_of_disks = read_u32(&locator, 16);
    if locator_disk != 0 || number_of_disks != 1 {
        return Err(unsupported_archive(
            "multi-disk ZIP64 archives are not supported".to_owned(),
        ));
    }

    let mut zip64_eocd = [0_u8; ZIP64_END_OF_CENTRAL_DIRECTORY_BYTES];
    read_exact_at(file, zip64_eocd_offset, &mut zip64_eocd)?;
    if zip64_eocd.get(..4) != Some(ZIP64_END_OF_CENTRAL_DIRECTORY_MAGIC) {
        return Err(unsafe_archive(
            "archive has an invalid ZIP64 central-directory record".to_owned(),
        ));
    }
    let record_size = read_u64(&zip64_eocd, 4);
    let record_end = zip64_eocd_offset
        .checked_add(12)
        .and_then(|value| value.checked_add(record_size))
        .ok_or_else(|| unsafe_archive("ZIP64 central-directory record overflow".to_owned()))?;
    if record_size < ZIP64_END_OF_CENTRAL_DIRECTORY_MIN_RECORD_SIZE || record_end != locator_offset
    {
        return Err(unsafe_archive(
            "ZIP64 central-directory record size is inconsistent".to_owned(),
        ));
    }

    let disk_number = read_u32(&zip64_eocd, 16);
    let central_directory_disk = read_u32(&zip64_eocd, 20);
    let entries_on_disk = read_u64(&zip64_eocd, 24);
    let total_entries = read_u64(&zip64_eocd, 32);
    let central_directory_size = read_u64(&zip64_eocd, 40);
    let central_directory_offset = read_u64(&zip64_eocd, 48);
    if disk_number != 0
        || central_directory_disk != 0
        || entries_on_disk != total_entries
        || locator_disk != central_directory_disk
    {
        return Err(unsupported_archive(
            "multi-disk ZIP64 archives are not supported".to_owned(),
        ));
    }
    if (classic_entries_on_disk != u16::MAX
        && u64::from(classic_entries_on_disk) != entries_on_disk)
        || (classic_total_entries != u16::MAX && u64::from(classic_total_entries) != total_entries)
        || (classic_central_size != u32::MAX
            && u64::from(classic_central_size) != central_directory_size)
        || (classic_central_offset != u32::MAX
            && u64::from(classic_central_offset) != central_directory_offset)
    {
        return Err(unsafe_archive(
            "ZIP32 and ZIP64 central-directory metadata disagree".to_owned(),
        ));
    }
    Ok((
        total_entries,
        central_directory_size,
        central_directory_offset,
        zip64_eocd_offset,
    ))
}

fn validate_raw_central_directory(
    file: &mut File,
    central_start: u64,
    central_size: u64,
    raw_entries: u64,
) -> CoreResult<()> {
    file.seek(SeekFrom::Start(central_start))
        .map_err(archive_io_error)?;
    let mut consumed = 0_u64;
    let mut header = [0_u8; 46];
    for _ in 0..raw_entries {
        let remaining = central_size
            .checked_sub(consumed)
            .ok_or_else(|| unsafe_archive("archive central-directory size underflow".to_owned()))?;
        if remaining < CENTRAL_DIRECTORY_HEADER_BYTES {
            return Err(unsafe_archive(
                "archive central directory contains fewer records than declared".to_owned(),
            ));
        }
        file.read_exact(&mut header).map_err(|error| {
            if error.kind() == std::io::ErrorKind::UnexpectedEof {
                unsafe_archive("archive central directory is truncated".to_owned())
            } else {
                archive_io_error(error)
            }
        })?;
        if header.get(..4) != Some(CENTRAL_DIRECTORY_HEADER_MAGIC) {
            return Err(unsafe_archive(
                "archive central directory contains an invalid record".to_owned(),
            ));
        }
        let variable_size = u64::from(read_u16(&header, 28))
            + u64::from(read_u16(&header, 30))
            + u64::from(read_u16(&header, 32));
        let record_size = CENTRAL_DIRECTORY_HEADER_BYTES
            .checked_add(variable_size)
            .ok_or_else(|| unsafe_archive("archive central record size overflow".to_owned()))?;
        consumed = consumed
            .checked_add(record_size)
            .ok_or_else(|| unsafe_archive("archive central-directory size overflow".to_owned()))?;
        if consumed > central_size {
            return Err(unsafe_archive(
                "archive central-directory record exceeds its declared size".to_owned(),
            ));
        }
        file.seek(SeekFrom::Current(i64::try_from(variable_size).map_err(
            |_| unsafe_archive("archive central record is too large".to_owned()),
        )?))
        .map_err(archive_io_error)?;
    }
    if consumed != central_size {
        return Err(unsafe_archive(
            "archive central directory contains undeclared records or trailing data".to_owned(),
        ));
    }
    Ok(())
}

fn read_exact_at(file: &mut File, offset: u64, bytes: &mut [u8]) -> CoreResult<()> {
    file.seek(SeekFrom::Start(offset))
        .and_then(|_| file.read_exact(bytes))
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::UnexpectedEof {
                unsafe_archive("archive central-directory metadata is truncated".to_owned())
            } else {
                archive_io_error(error)
            }
        })
}

fn find_end_of_central_directory(tail: &[u8]) -> Option<(usize, &[u8])> {
    if tail.len() < END_OF_CENTRAL_DIRECTORY_BYTES {
        return None;
    }
    tail.windows(END_OF_CENTRAL_DIRECTORY_MAGIC.len())
        .enumerate()
        .rev()
        .find_map(|(position, magic)| {
            if magic != END_OF_CENTRAL_DIRECTORY_MAGIC
                || position + END_OF_CENTRAL_DIRECTORY_BYTES > tail.len()
            {
                return None;
            }
            let eocd = &tail[position..];
            let comment_len = usize::from(read_u16(eocd, 20));
            (END_OF_CENTRAL_DIRECTORY_BYTES + comment_len == eocd.len()).then_some((position, eocd))
        })
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("four bytes"))
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().expect("eight bytes"))
}

fn inspect_entry<R: Read>(
    entry: &mut ZipFile<'_, R>,
    limits: ImportLimits,
    state: &mut InspectionState,
    buffer: &mut [u8],
    asset_path: Option<&Path>,
    nonportable_policy: adapters::NonPortableContentPolicy,
) -> CoreResult<()> {
    let plan = prepare_entry(entry, limits, state, nonportable_policy)?;
    if plan.kind == EntryKind::OmittedNonportable {
        state.omitted_nonportable_entries = state
            .omitted_nonportable_entries
            .checked_add(1)
            .ok_or_else(|| unsafe_archive("omitted archive entry count overflow".to_owned()))?;
        return Ok(());
    }
    if plan.kind == EntryKind::RuntimeProbe {
        if let Some(document) = inspect_runtime_probe_entry(
            entry,
            &plan,
            limits,
            &mut state.total_uncompressed,
            buffer,
        )? {
            state.runtime_documents.push(document);
        } else {
            state.omitted_nonportable_entries = state
                .omitted_nonportable_entries
                .checked_add(1)
                .ok_or_else(|| unsafe_archive("omitted archive entry count overflow".to_owned()))?;
        }
        return Ok(());
    }
    let mut read = read_entry(
        entry,
        &plan,
        limits,
        &mut state.total_uncompressed,
        buffer,
        asset_path,
    )?;

    if let Some(bytes) = read.metadata {
        state.metadata_bytes = Some(bytes);
    } else if plan.is_asset() {
        inspect_asset_entry(&plan, &mut read, state)?;
    } else if plan.is_unknown_extension() {
        inspect_unknown_entry(&plan, &read, state)?;
    }
    if let Some(asset) = read.staged_asset {
        state.staged_assets.push(asset);
    }
    Ok(())
}

fn inspect_asset_entry(
    plan: &EntryPlan,
    read: &mut EntryRead,
    state: &mut InspectionState,
) -> CoreResult<()> {
    state.asset_count = state
        .asset_count
        .checked_add(1)
        .ok_or_else(|| unsafe_archive("archive asset count overflow".to_owned()))?;
    let declared_media_type = asset_media_type(&plan.extension);
    let detected_media_type = detect_asset_media_type(&read.asset_header[..read.asset_header_len]);
    let signature_valid = detected_media_type.is_some();
    let media_type = detected_media_type.unwrap_or("application/octet-stream");
    if !signature_valid {
        state.unsupported_asset_count = state
            .unsupported_asset_count
            .checked_add(1)
            .ok_or_else(|| unsafe_archive("unsupported asset count overflow".to_owned()))?;
        state
            .first_unsupported_asset
            .get_or_insert_with(|| plan.name.clone());
    } else if media_type != declared_media_type {
        state.reclassified_asset_count = state
            .reclassified_asset_count
            .checked_add(1)
            .ok_or_else(|| unsafe_archive("reclassified asset count overflow".to_owned()))?;
    }
    if signature_valid && state.representative_image.is_none() && media_type.starts_with("image/") {
        state.representative_image = Some(ImportImagePreview {
            logical_asset_id: plan.logical_asset_id.clone(),
            media_type: media_type.to_owned(),
            size_bytes: read.size_bytes,
        });
    }
    if let Some(asset) = read.staged_asset.as_mut() {
        asset.signature_valid = signature_valid;
        media_type.clone_into(&mut asset.media_type);
    }
    let asset_sha256 = read
        .entry_sha256
        .as_deref()
        .ok_or_else(|| unsafe_archive("asset digest was not computed".to_owned()))?;
    let sha256 = Sha256Digest::parse(asset_sha256.to_owned())
        .map_err(|message| CoreError::new(CoreErrorCode::StorageCorrupted, message, false))?;
    state.asset_descriptors.push(AssetDescriptor {
        id: AssetId::from(format!("sha256:{asset_sha256}")),
        sha256,
        media_type: media_type.to_owned(),
        role: infer_asset_role(&plan.logical_asset_id, media_type),
        name: Path::new(&plan.name)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or(&plan.name)
            .to_owned(),
        size_bytes: read.size_bytes,
        width: None,
        height: None,
        duration_ms: None,
        source: AssetSource {
            kind: AssetSourceKind::CharxPackage,
            source_sha256: None,
            logical_path: Some(plan.logical_asset_id.clone()),
        },
    });
    Ok(())
}

fn inspect_unknown_entry(
    plan: &EntryPlan,
    read: &EntryRead,
    state: &mut InspectionState,
) -> CoreResult<()> {
    let entry_sha256 = read
        .entry_sha256
        .as_deref()
        .ok_or_else(|| unsafe_archive("extension digest was not computed".to_owned()))?;
    let sha256 = Sha256Digest::parse(entry_sha256.to_owned())
        .map_err(|message| CoreError::new(CoreErrorCode::StorageCorrupted, message, false))?;
    state.archive_extensions.push(UnknownExtensionEntry {
        key: "archive_entry".to_owned(),
        source_path: archive_entry_json_pointer(&plan.logical_asset_id),
        sha256,
        size_bytes: read.size_bytes,
        quarantine: plan
            .quarantine_kind()
            .map(|kind| ExtensionQuarantine::inactive(kind, quarantine_reason(kind))),
    });
    if plan.quarantine_kind().is_some() {
        state.warnings.push(ImportWarning {
            code: "quarantined_active_content".to_owned(),
            message: format!(
                "Unsupported active-content entry is preserved but inactive: {}",
                plan.name
            ),
        });
    }
    Ok(())
}

fn prepare_entry<R: Read>(
    entry: &ZipFile<'_, R>,
    limits: ImportLimits,
    state: &mut InspectionState,
    nonportable_policy: adapters::NonPortableContentPolicy,
) -> CoreResult<EntryPlan> {
    let name = std::str::from_utf8(entry.name_raw())
        .map_err(|_| unsafe_archive("archive entry path is not valid UTF-8".to_owned()))?
        .to_owned();
    let collision_key =
        validate_archive_path(&name).map_err(|message| unsafe_archive(message.to_owned()))?;
    let is_directory = entry.is_dir();
    register_archive_path(
        &mut state.archive_paths,
        &collision_key,
        is_directory,
        &name,
    )?;
    validate_entry_type_and_size(entry, &name, is_directory, limits, state)?;

    let extension = Path::new(&name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let is_metadata = collision_key == CARD_METADATA_PATH && !is_directory;
    if is_metadata && entry.size() > adapters::MAX_METADATA_BYTES as u64 {
        return Err(unsupported_archive(
            "character metadata exceeds 4 MiB".to_owned(),
        ));
    }

    let is_asset = is_asset_extension(&extension) && !is_directory;
    let kind = if is_directory {
        EntryKind::Directory
    } else if is_metadata {
        EntryKind::Metadata
    } else if is_asset {
        EntryKind::Asset
    } else if nonportable_policy == adapters::NonPortableContentPolicy::PreserveForRoundTrip {
        EntryKind::UnknownExtension(quarantined_extension_kind(&extension))
    } else if !is_directory {
        EntryKind::RuntimeProbe
    } else {
        EntryKind::OmittedNonportable
    };
    Ok(EntryPlan {
        name,
        logical_asset_id: collision_key,
        extension,
        kind,
    })
}

fn inspect_runtime_probe_entry<R: Read>(
    entry: &mut ZipFile<'_, R>,
    plan: &EntryPlan,
    limits: ImportLimits,
    total_uncompressed: &mut u64,
    buffer: &mut [u8],
) -> CoreResult<Option<runtime::DecodedRuntimeDocument>> {
    let capacity = usize::try_from(entry.size())
        .map_err(|_| unsafe_archive("archive entry is too large for this device".to_owned()))?;
    let can_collect = capacity <= runtime::MAX_RUNTIME_DOCUMENT_BYTES;
    let mut candidate = None::<bool>;
    let mut bytes = Vec::new();
    let mut entry_uncompressed = 0_u64;
    loop {
        let read = entry
            .read(buffer)
            .map_err(|error| unsafe_archive(format!("cannot decode {}: {error}", plan.name)))?;
        if read == 0 {
            break;
        }
        entry_uncompressed = checked_entry_size(entry_uncompressed, read, &plan.name, limits)?;
        *total_uncompressed = checked_total_size(*total_uncompressed, read, limits)?;
        if candidate.is_none() {
            candidate = Some(can_collect && runtime::has_runtime_header(&buffer[..read]));
            if candidate == Some(true) {
                bytes.reserve(capacity);
            }
        }
        if candidate == Some(true) {
            bytes.extend_from_slice(&buffer[..read]);
        }
    }
    if entry_uncompressed != entry.size() {
        return Err(unsafe_archive(format!(
            "archive entry size does not match its header: {}",
            plan.name
        )));
    }
    if candidate != Some(true) {
        return Ok(None);
    }
    runtime::decode_runtime_document(&bytes, &hex::encode(Sha256::digest(&bytes))).map(Some)
}

fn validate_entry_type_and_size<R: Read>(
    entry: &ZipFile<'_, R>,
    name: &str,
    is_directory: bool,
    limits: ImportLimits,
    state: &mut InspectionState,
) -> CoreResult<()> {
    if entry.is_symlink() {
        return Err(unsafe_archive(format!(
            "symbolic links are not allowed: {name}"
        )));
    }
    if entry.encrypted() {
        return Err(unsupported_archive(format!(
            "encrypted archive entries are not supported: {name}"
        )));
    }
    if is_directory && entry.size() != 0 {
        return Err(unsafe_archive(format!(
            "archive directory contains file data: {name}"
        )));
    }
    if entry.size() > limits.max_entry_bytes {
        return Err(unsafe_archive(format!(
            "archive entry exceeds size limit: {name}"
        )));
    }
    state.declared_total = state
        .declared_total
        .checked_add(entry.size())
        .ok_or_else(|| unsafe_archive("archive size overflow".to_owned()))?;
    if state.declared_total > limits.max_total_uncompressed_bytes {
        return Err(unsafe_archive(
            "archive exceeds total uncompressed size limit".to_owned(),
        ));
    }

    let compressed = entry.compressed_size();
    if entry.size() > 0
        && (compressed == 0
            || entry.size() > compressed.saturating_mul(limits.max_compression_ratio))
    {
        return Err(unsafe_archive(format!(
            "archive entry exceeds compression ratio limit: {name}"
        )));
    }
    Ok(())
}

fn read_entry<R: Read>(
    entry: &mut ZipFile<'_, R>,
    plan: &EntryPlan,
    limits: ImportLimits,
    total_uncompressed: &mut u64,
    buffer: &mut [u8],
    asset_path: Option<&Path>,
) -> CoreResult<EntryRead> {
    let result = read_entry_inner(entry, plan, limits, total_uncompressed, buffer, asset_path);
    if result.is_err()
        && let Some(asset_path) = asset_path
    {
        let _ = fs::remove_file(asset_path);
    }
    result
}

fn read_entry_inner<R: Read>(
    entry: &mut ZipFile<'_, R>,
    plan: &EntryPlan,
    limits: ImportLimits,
    total_uncompressed: &mut u64,
    buffer: &mut [u8],
    asset_path: Option<&Path>,
) -> CoreResult<EntryRead> {
    let metadata_capacity = plan
        .is_metadata()
        .then(|| {
            usize::try_from(entry.size()).map_err(|_| {
                unsafe_archive("archive entry is too large for this device".to_owned())
            })
        })
        .transpose()?;
    let mut result = EntryRead {
        metadata: metadata_capacity.map(Vec::with_capacity),
        asset_header: [0_u8; ASSET_HEADER_BYTES],
        asset_header_len: 0,
        size_bytes: 0,
        entry_sha256: None,
        staged_asset: None,
    };
    let mut asset_file = if plan.is_asset() {
        asset_path
            .map(|path| {
                OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(path)
                    .map_err(archive_io_error)
            })
            .transpose()?
    } else {
        None
    };
    let mut asset_digest = (plan.is_asset() || plan.is_unknown_extension()).then(Sha256::new);
    let mut entry_uncompressed = 0_u64;

    loop {
        let read = entry
            .read(buffer)
            .map_err(|error| unsafe_archive(format!("cannot decode {}: {error}", plan.name)))?;
        if read == 0 {
            break;
        }
        entry_uncompressed = checked_entry_size(entry_uncompressed, read, &plan.name, limits)?;
        *total_uncompressed = checked_total_size(*total_uncompressed, read, limits)?;
        collect_entry_bytes(&mut result, plan, &buffer[..read])?;
        if let Some(file) = asset_file.as_mut() {
            file.write_all(&buffer[..read]).map_err(archive_io_error)?;
        }
        if let Some(digest) = asset_digest.as_mut() {
            digest.update(&buffer[..read]);
        }
    }
    if entry_uncompressed != entry.size() {
        return Err(unsafe_archive(format!(
            "archive entry size does not match its header: {}",
            plan.name
        )));
    }
    result.size_bytes = entry_uncompressed;
    let asset_sha256 = asset_digest.map(|digest| hex::encode(digest.finalize()));
    result.entry_sha256.clone_from(&asset_sha256);
    if let (Some(file), Some(sha256), Some(path)) =
        (asset_file, asset_sha256, asset_path.map(Path::to_path_buf))
    {
        file.sync_all().map_err(archive_io_error)?;
        result.staged_asset = Some(StagedAsset {
            original_path: plan.logical_asset_id.clone(),
            staged_path: path,
            sha256,
            media_type: asset_media_type(&plan.extension).to_owned(),
            size_bytes: entry_uncompressed,
            signature_valid: true,
        });
    }
    Ok(result)
}

fn checked_entry_size(
    current: u64,
    read: usize,
    name: &str,
    limits: ImportLimits,
) -> CoreResult<u64> {
    let updated = current
        .checked_add(read as u64)
        .ok_or_else(|| unsafe_archive("archive entry size overflow".to_owned()))?;
    if updated > limits.max_entry_bytes {
        return Err(unsafe_archive(format!(
            "archive entry exceeds size limit while decoding: {name}"
        )));
    }
    Ok(updated)
}

fn checked_total_size(current: u64, read: usize, limits: ImportLimits) -> CoreResult<u64> {
    let updated = current
        .checked_add(read as u64)
        .ok_or_else(|| unsafe_archive("archive size overflow".to_owned()))?;
    if updated > limits.max_total_uncompressed_bytes {
        return Err(unsafe_archive(
            "archive exceeds total uncompressed size limit while decoding".to_owned(),
        ));
    }
    Ok(updated)
}

fn collect_entry_bytes(result: &mut EntryRead, plan: &EntryPlan, bytes: &[u8]) -> CoreResult<()> {
    if let Some(metadata) = result.metadata.as_mut() {
        metadata.extend_from_slice(bytes);
        if metadata.len() > adapters::MAX_METADATA_BYTES {
            return Err(unsupported_archive(
                "character metadata exceeds 4 MiB".to_owned(),
            ));
        }
    }
    if plan.is_asset() && result.asset_header_len < ASSET_HEADER_BYTES {
        let copy_len = (ASSET_HEADER_BYTES - result.asset_header_len).min(bytes.len());
        result.asset_header[result.asset_header_len..result.asset_header_len + copy_len]
            .copy_from_slice(&bytes[..copy_len]);
        result.asset_header_len += copy_len;
    }
    Ok(())
}

fn register_archive_path(
    archive_paths: &mut HashMap<String, bool>,
    collision_key: &str,
    is_directory: bool,
    original_name: &str,
) -> CoreResult<()> {
    if archive_paths.contains_key(collision_key) {
        return Err(unsafe_archive(format!(
            "archive path collides after normalization: {original_name}"
        )));
    }

    let mut ancestor = collision_key;
    while let Some((parent, _)) = ancestor.rsplit_once('/') {
        if archive_paths.get(parent).is_some_and(|is_dir| !is_dir) {
            return Err(unsafe_archive(format!(
                "archive path descends through a file: {original_name}"
            )));
        }
        ancestor = parent;
    }

    if !is_directory {
        let descendant_prefix = format!("{collision_key}/");
        if archive_paths
            .keys()
            .any(|path| path.starts_with(&descendant_prefix))
        {
            return Err(unsafe_archive(format!(
                "archive file path collides with a directory: {original_name}"
            )));
        }
    }

    archive_paths.insert(collision_key.to_owned(), is_directory);
    Ok(())
}

fn is_asset_extension(extension: &str) -> bool {
    matches!(
        extension,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "mp3" | "wav" | "ogg"
    )
}

fn quarantined_extension_kind(extension: &str) -> Option<ExtensionQuarantineKind> {
    match extension {
        "js" | "mjs" | "cjs" | "ts" | "sh" | "bash" | "zsh" | "ps1" | "bat" | "cmd" => {
            Some(ExtensionQuarantineKind::Script)
        }
        "html" | "htm" | "svg" => Some(ExtensionQuarantineKind::Html),
        "py" | "rb" | "exe" | "dll" | "dylib" | "so" | "wasm" | "class" | "jar" => {
            Some(ExtensionQuarantineKind::Code)
        }
        "url" | "webloc" => Some(ExtensionQuarantineKind::ExternalUrl),
        "ttf" | "otf" | "woff" | "woff2" => Some(ExtensionQuarantineKind::UnknownActiveContent),
        _ => None,
    }
}

fn quarantine_reason(kind: ExtensionQuarantineKind) -> &'static str {
    match kind {
        ExtensionQuarantineKind::Code => "archive code is preserved but never executed",
        ExtensionQuarantineKind::Script => "archive script is preserved but never executed",
        ExtensionQuarantineKind::Html => {
            "archive HTML is preserved but never rendered as active UI"
        }
        ExtensionQuarantineKind::ExternalUrl => {
            "external URL reference is preserved but never fetched automatically"
        }
        ExtensionQuarantineKind::UnknownActiveContent => {
            "unknown active content is preserved but inactive"
        }
    }
}

fn archive_entry_json_pointer(path: &str) -> String {
    let escaped = path
        .split('/')
        .map(|component| component.replace('~', "~0").replace('/', "~1"))
        .collect::<Vec<_>>()
        .join("/");
    format!("/archive/{escaped}")
}

fn infer_asset_role(path: &str, media_type: &str) -> AssetRole {
    let lower = path.to_ascii_lowercase();
    if lower.contains("avatar") {
        AssetRole::Avatar
    } else if lower.contains("icon") {
        AssetRole::Icon
    } else if lower.contains("background") || lower.contains("/bg") {
        AssetRole::Background
    } else if lower.contains("expression") || lower.contains("emotion") {
        AssetRole::Expression
    } else if lower.contains("voice") {
        AssetRole::Voice
    } else if media_type.starts_with("audio/") {
        AssetRole::Audio
    } else {
        AssetRole::Other
    }
}

fn asset_media_type(extension: &str) -> &'static str {
    match extension {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        _ => "application/octet-stream",
    }
}

fn detect_asset_media_type(header: &[u8]) -> Option<&'static str> {
    if header.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if header.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if header.starts_with(b"GIF87a") || header.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if header.starts_with(b"RIFF") && header.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else if header.get(4..8) == Some(b"ftyp")
        && matches!(header.get(8..12), Some(b"avif" | b"avis"))
    {
        Some("image/avif")
    } else if header.starts_with(b"RIFF") && header.get(8..12) == Some(b"WAVE") {
        Some("audio/wav")
    } else if header.starts_with(b"OggS") {
        Some("audio/ogg")
    } else if header.starts_with(b"ID3")
        || header
            .get(..2)
            .is_some_and(|bytes| bytes[0] == 0xff && bytes[1] & 0xe0 == 0xe0)
    {
        Some("audio/mpeg")
    } else {
        None
    }
}

fn archive_io_error(error: std::io::Error) -> CoreError {
    CoreError::new(
        CoreErrorCode::StorageUnavailable,
        format!("cannot read archive: {error}"),
        true,
    )
}

fn unsupported_archive(message: String) -> CoreError {
    CoreError::new(CoreErrorCode::UnsupportedContent, message, false)
}

fn unsafe_archive(message: String) -> CoreError {
    CoreError::new(CoreErrorCode::UnsafeArchive, message, false)
}

fn cleanup_staged_assets(assets: &[StagedAsset]) {
    for asset in assets {
        let _ = fs::remove_file(&asset.staged_path);
    }
}
