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

use crate::{StagedAsset, adapters, path::validate_archive_path};

const CARD_METADATA_PATH: &str = "card.json";
const ASSET_HEADER_BYTES: usize = 16;
const READ_BUFFER_BYTES: usize = 64 * 1024;
const END_OF_CENTRAL_DIRECTORY_MAGIC: &[u8; 4] = b"PK\x05\x06";
const END_OF_CENTRAL_DIRECTORY_BYTES: usize = 22;

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
    staged_assets: Vec<StagedAsset>,
}

struct EntryPlan {
    name: String,
    logical_asset_id: String,
    extension: String,
    is_metadata: bool,
    is_asset: bool,
    is_unknown_extension: bool,
    quarantine_kind: Option<ExtensionQuarantineKind>,
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
) -> CoreResult<ArchiveInspection> {
    inspect_archive_internal(path, limits, source_sha256, None)
}

pub(crate) fn stage_archive_assets(
    path: &Path,
    limits: ImportLimits,
    staging_directory: &Path,
    inspection_id: &str,
) -> CoreResult<Vec<StagedAsset>> {
    fs::create_dir_all(staging_directory).map_err(archive_io_error)?;
    let source_sha256 = crate::sha256_file(path)?;
    inspect_archive_internal(
        path,
        limits,
        &source_sha256,
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
    asset_staging: Option<AssetStaging<'_>>,
) -> CoreResult<ArchiveInspection> {
    let mut file = File::open(path).map_err(archive_io_error)?;
    let declared_entries = declared_entry_count(&mut file)?;
    if declared_entries > limits.max_entries {
        return Err(unsafe_archive(format!(
            "archive has {declared_entries} entries; maximum is {}",
            limits.max_entries
        )));
    }
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
        ) {
            cleanup_staged_assets(&state.staged_assets);
            return Err(error);
        }
    }

    let mut metadata = match state
        .metadata_bytes
        .ok_or_else(|| {
            unsupported_archive("CHARX package must contain a root card.json".to_owned())
        })
        .and_then(|bytes| adapters::parse_card_json_with_source(&bytes, source_sha256))
    {
        Ok(metadata) => metadata,
        Err(error) => {
            cleanup_staged_assets(&state.staged_assets);
            return Err(error);
        }
    };
    let source_digest = Sha256Digest::parse(source_sha256.to_owned())
        .map_err(|message| CoreError::new(CoreErrorCode::StorageCorrupted, message, false))?;
    for descriptor in &mut state.asset_descriptors {
        descriptor.source.source_sha256 = Some(source_digest.clone());
    }
    metadata.content.assets = state.asset_descriptors;
    if !state.archive_extensions.is_empty() {
        let mut entries = std::mem::take(&mut metadata.content.unknown_extensions.entries);
        entries.append(&mut state.archive_extensions);
        metadata.content.unknown_extensions =
            UnknownExtensionIndex::try_new(Some(source_digest), entries).map_err(|message| {
                CoreError::new(CoreErrorCode::UnsupportedContent, message, false)
            })?;
    }
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
        warnings: state.warnings,
        blocked_reasons: state.blocked_reasons,
        representative_image: state.representative_image,
        staged_assets: state.staged_assets,
    })
}

fn declared_entry_count(file: &mut File) -> CoreResult<usize> {
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

    let eocd = find_end_of_central_directory(&tail)
        .ok_or_else(|| unsafe_archive("archive has no valid central directory".to_owned()))?;
    let disk_number = read_u16(eocd, 4);
    let central_directory_disk = read_u16(eocd, 6);
    let entries_on_disk = read_u16(eocd, 8);
    let total_entries = read_u16(eocd, 10);
    if disk_number != 0 || central_directory_disk != 0 || entries_on_disk != total_entries {
        return Err(unsupported_archive(
            "multi-disk ZIP archives are not supported".to_owned(),
        ));
    }
    if total_entries == u16::MAX {
        return Err(unsupported_archive(
            "ZIP64 entry counts are not supported".to_owned(),
        ));
    }
    Ok(usize::from(total_entries))
}

fn find_end_of_central_directory(tail: &[u8]) -> Option<&[u8]> {
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
            (END_OF_CENTRAL_DIRECTORY_BYTES + comment_len == eocd.len()).then_some(eocd)
        })
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn inspect_entry<R: Read>(
    entry: &mut ZipFile<'_, R>,
    limits: ImportLimits,
    state: &mut InspectionState,
    buffer: &mut [u8],
    asset_path: Option<&Path>,
) -> CoreResult<()> {
    let plan = prepare_entry(entry, limits, state)?;
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
    } else if plan.is_asset {
        inspect_asset_entry(&plan, &mut read, state)?;
    } else if plan.is_unknown_extension {
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
    let signature_valid =
        asset_signature_matches(&plan.extension, &read.asset_header[..read.asset_header_len]);
    let media_type = asset_media_type(&plan.extension);
    if !signature_valid {
        state.warnings.push(ImportWarning {
            code: "mime_mismatch".to_owned(),
            message: format!(
                "Asset extension does not match file signature: {}",
                plan.name
            ),
        });
        state.blocked_reasons.push(format!(
            "Asset file signature does not match its declared extension: {}",
            plan.name
        ));
    } else if state.representative_image.is_none() && media_type.starts_with("image/") {
        state.representative_image = Some(ImportImagePreview {
            logical_asset_id: plan.logical_asset_id.clone(),
            media_type: media_type.to_owned(),
            size_bytes: read.size_bytes,
        });
    }
    if let Some(asset) = read.staged_asset.as_mut() {
        asset.signature_valid = signature_valid;
        if !signature_valid {
            "application/octet-stream".clone_into(&mut asset.media_type);
        }
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
        media_type: if signature_valid {
            media_type.to_owned()
        } else {
            "application/octet-stream".to_owned()
        },
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
            .quarantine_kind
            .map(|kind| ExtensionQuarantine::inactive(kind, quarantine_reason(kind))),
    });
    if plan.quarantine_kind.is_some() {
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
    let is_unknown_extension = !is_directory && !is_metadata && !is_asset;
    Ok(EntryPlan {
        name,
        logical_asset_id: collision_key,
        is_asset,
        is_unknown_extension,
        quarantine_kind: if is_unknown_extension {
            quarantined_extension_kind(&extension)
        } else {
            None
        },
        extension,
        is_metadata,
    })
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
        .is_metadata
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
    let mut asset_file = if plan.is_asset {
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
    let mut asset_digest = (plan.is_asset || plan.is_unknown_extension).then(Sha256::new);
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
    if plan.is_asset && result.asset_header_len < ASSET_HEADER_BYTES {
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

fn asset_signature_matches(extension: &str, header: &[u8]) -> bool {
    if extension == "png" {
        header.starts_with(b"\x89PNG\r\n\x1a\n")
    } else if matches!(extension, "jpg" | "jpeg") {
        header.starts_with(b"\xff\xd8\xff")
    } else if extension == "gif" {
        header.starts_with(b"GIF87a") || header.starts_with(b"GIF89a")
    } else if extension == "webp" {
        header.starts_with(b"RIFF") && header.get(8..12) == Some(b"WEBP")
    } else if extension == "avif" {
        header.get(4..8) == Some(b"ftyp") && matches!(header.get(8..12), Some(b"avif" | b"avis"))
    } else if extension == "wav" {
        header.starts_with(b"RIFF") && header.get(8..12) == Some(b"WAVE")
    } else if extension == "ogg" {
        header.starts_with(b"OggS")
    } else if extension == "mp3" {
        header.starts_with(b"ID3")
            || header
                .get(..2)
                .is_some_and(|bytes| bytes[0] == 0xff && bytes[1] & 0xe0 == 0xe0)
    } else {
        true
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
