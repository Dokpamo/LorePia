#![allow(unsafe_code)]

use std::{
    alloc::{GlobalAlloc, Layout, System},
    collections::BTreeMap,
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use lorepia_content::inspect_content_package;
use lorepia_domain::ImportLimits;
use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::{TempDir, tempdir};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

struct CountingAllocator;

static CURRENT_BYTES: AtomicUsize = AtomicUsize::new(0);
static PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            record_allocation(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            record_allocation(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        CURRENT_BYTES.fetch_sub(layout.size(), Ordering::SeqCst);
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let resized = unsafe { System.realloc(pointer, layout, new_size) };
        if !resized.is_null() {
            if new_size >= layout.size() {
                record_allocation(new_size - layout.size());
            } else {
                CURRENT_BYTES.fetch_sub(layout.size() - new_size, Ordering::SeqCst);
            }
        }
        resized
    }
}

fn record_allocation(size: usize) {
    let current = CURRENT_BYTES.fetch_add(size, Ordering::SeqCst) + size;
    PEAK_BYTES.fetch_max(current, Ordering::SeqCst);
}

fn reset_peak() -> usize {
    let current = CURRENT_BYTES.load(Ordering::SeqCst);
    PEAK_BYTES.store(current, Ordering::SeqCst);
    current
}

fn peak_delta(baseline: usize) -> usize {
    PEAK_BYTES.load(Ordering::SeqCst).saturating_sub(baseline)
}

struct PackageFixture {
    _directory: TempDir,
    path: PathBuf,
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn asset_bytes(index: u32, size: usize) -> Vec<u8> {
    let mut bytes = vec![0_u8; size];
    bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    for chunk in bytes[8..].chunks_mut(4) {
        let encoded = index.to_le_bytes();
        chunk.copy_from_slice(&encoded[..chunk.len()]);
    }
    bytes
}

fn write_large_asset_package(
    asset_count: u32,
    asset_size: usize,
) -> (PackageFixture, BTreeMap<String, String>) {
    let mut hashes = BTreeMap::new();
    let mut content_types = BTreeMap::new();
    for index in 0..asset_count {
        let bytes = asset_bytes(index, asset_size);
        let digest = sha256(&bytes);
        let path = format!("assets/sha256/{digest}.png");
        hashes.insert(path.clone(), digest);
        content_types.insert(path, "image/png");
    }
    let manifest = json!({
        "format": "lorepia_content_package",
        "format_version": 1,
        "package_id": "dev.lorepia.streaming-memory",
        "name": "Streaming memory fixture",
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
        "components": [],
        "signature": null
    });
    let directory = tempdir().expect("fixture directory");
    let path = directory.path().join("large-assets.zip");
    let file = File::create(&path).expect("create package");
    let mut archive = ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .unix_permissions(0o644);
    archive
        .start_file("manifest.json", options)
        .expect("manifest entry");
    archive
        .write_all(&serde_json::to_vec(&manifest).expect("manifest JSON"))
        .expect("write manifest");
    for index in 0..asset_count {
        let bytes = asset_bytes(index, asset_size);
        let digest = sha256(&bytes);
        archive
            .start_file(format!("assets/sha256/{digest}.png"), options)
            .expect("asset entry");
        archive.write_all(&bytes).expect("write asset");
    }
    archive.finish().expect("finish package");
    (
        PackageFixture {
            _directory: directory,
            path,
        },
        manifest["content_hashes"]
            .as_object()
            .expect("manifest hashes")
            .iter()
            .map(|(path, digest)| {
                (
                    path.clone(),
                    digest.as_str().expect("digest string").to_owned(),
                )
            })
            .collect(),
    )
}

fn write_single_asset_package(
    asset_size: usize,
    compression: CompressionMethod,
) -> (PackageFixture, usize) {
    let bytes = asset_bytes(7, asset_size);
    let digest = sha256(&bytes);
    let asset_path = format!("assets/sha256/{digest}.png");
    let manifest = json!({
        "format": "lorepia_content_package",
        "format_version": 1,
        "package_id": "dev.lorepia.streaming-limits",
        "name": "Streaming limit fixture",
        "version": "1.0.0",
        "author": "LorePia tests",
        "license": "MIT",
        "redistribution_allowed": true,
        "required_app_version": "0.1.0",
        "required_capabilities": ["media_assets"],
        "dependencies": [],
        "conflicts": [],
        "content_hashes": {asset_path.clone(): digest},
        "content_types": {asset_path.clone(): "image/png"},
        "components": [],
        "signature": null
    });
    let manifest_bytes = serde_json::to_vec(&manifest).expect("manifest JSON");
    let directory = tempdir().expect("fixture directory");
    let path = directory.path().join("limited-asset.zip");
    let file = File::create(&path).expect("create package");
    let mut archive = ZipWriter::new(file);
    let stored = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .unix_permissions(0o644);
    let selected = SimpleFileOptions::default()
        .compression_method(compression)
        .unix_permissions(0o644);
    archive
        .start_file("manifest.json", stored)
        .expect("manifest entry");
    archive.write_all(&manifest_bytes).expect("write manifest");
    archive
        .start_file(asset_path, selected)
        .expect("asset entry");
    archive.write_all(&bytes).expect("write asset");
    archive.finish().expect("finish package");
    (
        PackageFixture {
            _directory: directory,
            path,
        },
        manifest_bytes.len(),
    )
}

fn inspect_error(path: &Path, limits: ImportLimits) -> String {
    inspect_content_package(path, limits)
        .expect_err("configured import limit must fail")
        .message
}

#[test]
fn inspection_memory_is_bounded_and_every_archive_limit_fails_closed() {
    const ASSET_COUNT: u32 = 2_048;
    const ASSET_SIZE: usize = 32 * 1024;
    let decoded_asset_bytes = usize::try_from(ASSET_COUNT).expect("asset count") * ASSET_SIZE;
    let (large, expected_hashes) = write_large_asset_package(ASSET_COUNT, ASSET_SIZE);
    let baseline = reset_peak();
    let inspection = inspect_content_package(
        &large.path,
        ImportLimits {
            max_entries: 2_100,
            ..ImportLimits::default()
        },
    )
    .expect("streaming inspection");
    let allocated_peak = peak_delta(baseline);
    eprintln!("decoded_asset_bytes={decoded_asset_bytes} peak_allocation_delta={allocated_peak}");

    assert_eq!(inspection.components.len(), expected_hashes.len());
    assert_eq!(
        inspection
            .components
            .iter()
            .map(|component| (component.path.clone(), component.sha256.clone()))
            .collect::<BTreeMap<_, _>>(),
        expected_hashes
    );
    assert!(inspection.total_uncompressed_size >= decoded_asset_bytes as u64);
    assert!(
        allocated_peak < decoded_asset_bytes / 3,
        "inspection peak allocation {allocated_peak} must stay far below \
         {decoded_asset_bytes} decoded asset bytes"
    );

    let entry_count_error = inspect_error(
        &large.path,
        ImportLimits {
            max_entries: 2_000,
            ..ImportLimits::default()
        },
    );
    assert!(entry_count_error.contains("entries"));

    let (stored, manifest_size) =
        write_single_asset_package(1024 * 1024, CompressionMethod::Stored);
    let entry_size_error = inspect_error(
        &stored.path,
        ImportLimits {
            max_entry_bytes: u64::try_from(manifest_size + 1).expect("manifest size"),
            ..ImportLimits::default()
        },
    );
    assert!(entry_size_error.contains("entry exceeds size limit"));
    let total_size_error = inspect_error(
        &stored.path,
        ImportLimits {
            max_entry_bytes: 2 * 1024 * 1024,
            max_total_uncompressed_bytes: u64::try_from(manifest_size + 512 * 1024)
                .expect("total limit"),
            ..ImportLimits::default()
        },
    );
    assert!(total_size_error.contains("total uncompressed size limit"));

    let (compressed, _) = write_single_asset_package(1024 * 1024, CompressionMethod::Deflated);
    let ratio_error = inspect_error(
        &compressed.path,
        ImportLimits {
            max_entry_bytes: 2 * 1024 * 1024,
            max_compression_ratio: 2,
            ..ImportLimits::default()
        },
    );
    assert!(ratio_error.contains("compression ratio limit"));
}
