use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{File, OpenOptions},
    io::Write,
    path::Path,
};

use lorepia_domain::{CoreError, CoreErrorCode, CoreResult};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

use super::{
    container::copy_decoded_asset,
    convert::{NormalizedDocumentEntry, NormalizedExternalContent},
};

pub(super) struct WrittenPackage {
    pub(super) document_count: u32,
    pub(super) asset_count: u32,
}

pub(super) fn write_normalized_package(
    source_path: &Path,
    output_path: &Path,
    source_sha256: &str,
    content: &NormalizedExternalContent,
) -> CoreResult<WrittenPackage> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output_path)
        .map_err(storage_error)?;
    let mut archive = ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .unix_permissions(0o644);
    let mut content_hashes = BTreeMap::new();
    let mut content_types = BTreeMap::new();
    let mut components = Vec::new();

    for document in &content.documents {
        write_document(
            &mut archive,
            options,
            document,
            &mut content_hashes,
            &mut content_types,
            &mut components,
        )?;
    }

    let mut source = File::open(source_path).map_err(storage_error)?;
    let mut seen_assets = BTreeSet::new();
    let mut asset_count = 0_u32;
    for (index, asset) in content.assets.iter().enumerate() {
        if !seen_assets.insert(asset.sha256.clone()) {
            continue;
        }
        let path = format!("assets/sha256/{}.{}", asset.sha256, asset.extension);
        archive.start_file(&path, options).map_err(package_error)?;
        copy_decoded_asset(&mut source, asset, &mut archive)?;
        content_hashes.insert(path.clone(), asset.sha256.clone());
        content_types.insert(path.clone(), asset.media_type.clone());
        components.push(component_json(
            format!("asset-{index}"),
            path,
            "asset",
            Vec::new(),
            Vec::new(),
        ));
        asset_count = asset_count.saturating_add(1);
    }

    let package_id = format!("dev.lorepia.risu-import.{}", &source_sha256[..24]);
    let manifest = json!({
        "format": "lorepia_content_package",
        "format_version": 1,
        "package_id": package_id,
        "name": content.name,
        "version": "1.0.0",
        "author": "",
        "license": "LicenseRef-Risu-User-Content",
        "redistribution_allowed": false,
        "required_app_version": null,
        "required_capabilities": content.package_capabilities,
        "dependencies": [],
        "conflicts": [],
        "content_hashes": content_hashes,
        "content_types": content_types,
        "components": components,
        "signature": null
    });
    archive
        .start_file("manifest.json", options)
        .map_err(package_error)?;
    archive
        .write_all(&serde_json::to_vec(&manifest).map_err(json_error)?)
        .map_err(storage_error)?;
    let file = archive.finish().map_err(package_error)?;
    file.sync_all().map_err(storage_error)?;
    Ok(WrittenPackage {
        document_count: u32::try_from(content.documents.len()).unwrap_or(u32::MAX),
        asset_count,
    })
}

fn write_document(
    archive: &mut ZipWriter<File>,
    options: SimpleFileOptions,
    document: &NormalizedDocumentEntry,
    content_hashes: &mut BTreeMap<String, String>,
    content_types: &mut BTreeMap<String, String>,
    components: &mut Vec<Value>,
) -> CoreResult<()> {
    let bytes = serde_json::to_vec(&document.document).map_err(json_error)?;
    let digest = hex::encode(Sha256::digest(&bytes));
    archive
        .start_file(&document.path, options)
        .map_err(package_error)?;
    archive.write_all(&bytes).map_err(storage_error)?;
    content_hashes.insert(document.path.clone(), digest);
    content_types.insert(document.path.clone(), "application/json".to_owned());
    components.push(component_json(
        document.id.clone(),
        document.path.clone(),
        document.kind,
        document.required_capabilities.clone(),
        document.depends_on.clone(),
    ));
    Ok(())
}

fn component_json(
    id: String,
    path: String,
    kind: &str,
    required_capabilities: Vec<&str>,
    depends_on: Vec<String>,
) -> Value {
    json!({
        "id": id,
        "path": path,
        "kind": kind,
        "required_capabilities": required_capabilities,
        "depends_on": depends_on,
        "conflicts_with": []
    })
}

fn package_error(error: zip::result::ZipError) -> CoreError {
    CoreError::new(
        CoreErrorCode::StorageUnavailable,
        format!("cannot build normalized Risu package: {error}"),
        true,
    )
}

fn json_error(error: serde_json::Error) -> CoreError {
    CoreError::new(
        CoreErrorCode::UnsupportedContent,
        format!("cannot normalize Risu content: {error}"),
        false,
    )
}

fn storage_error(error: std::io::Error) -> CoreError {
    CoreError::new(
        CoreErrorCode::StorageUnavailable,
        format!("cannot write normalized Risu package: {error}"),
        true,
    )
}
