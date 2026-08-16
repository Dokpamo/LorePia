use lorepia_core::{CORE_API_VERSION, Core, CoreConfig, core_version};
use tempfile::tempdir;

#[test]
fn opens_reports_health_and_reopens_the_same_data_root() {
    let root = tempdir().expect("temporary data root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");

    let health = core.health_check().expect("health check");
    assert_eq!(health.core_version, core_version());
    assert!(health.database_open);
    assert!(health.data_root_writable);
    assert!(health.staging_writable);
    assert!(!health.recovery_pending);
    assert_eq!(CORE_API_VERSION, 9);

    drop(core);
    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen core");
    assert!(
        reopened
            .list_characters()
            .expect("empty library")
            .is_empty()
    );
}
