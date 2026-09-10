use std::path::Path;

use lorepia_content::{inspect_content_package, prepare_external_import};
use lorepia_domain::{ContentKind, ImportLimits};
use tempfile::tempdir;

#[test]
fn external_large_imported_module_is_self_inspecting_when_fixture_is_available() {
    let Ok(path) = std::env::var("LOREPIA_EXTERNAL_MODULE_FIXTURE") else {
        return;
    };
    let staging = tempdir().expect("staging directory");

    let prepared =
        prepare_external_import(Path::new(&path), ImportLimits::default(), staging.path())
            .expect("prepare external module")
            .expect("recognize external module");

    assert_eq!(prepared.inspection.kind, ContentKind::RisuModule);
    assert!(prepared.inspection.is_allowed());
    assert!(prepared.inspection.asset_count > 0);
    let package =
        inspect_content_package(&prepared.normalized_package_path, ImportLimits::default())
            .expect("inspect normalized package");
    assert!(package.is_allowed());
}
