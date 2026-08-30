
#[test]
fn reopening_storage_idempotently_upgrades_legacy_builtin_prompt_presets() {
    let root = tempfile::tempdir().expect("temporary storage root");
    let storage = Storage::open(root.path()).expect("open storage");
    for canonical in built_in_prompt_presets() {
        let current = storage
            .get_prompt_preset(&canonical.id)
            .expect("seeded built-in prompt preset");
        assert_eq!(current.revision, 1);
        let legacy = legacy_builtin_prompt_preset(canonical);
        storage
            .save_prompt_preset(&legacy, Some(current.revision))
            .expect("install legacy built-in prompt preset fixture");
    }
    drop(storage);

    let upgraded = Storage::open(root.path()).expect("upgrade legacy built-in prompt presets");
    for canonical in built_in_prompt_presets() {
        let current = upgraded
            .get_prompt_preset(&canonical.id)
            .expect("upgraded built-in prompt preset");
        assert_eq!(current.value, canonical);
        assert_eq!(current.revision, 3);
        assert_eq!(
            upgraded
                .list_prompt_preset_revisions(&canonical.id)
                .expect("built-in prompt preset revisions")
                .len(),
            3
        );
    }
    drop(upgraded);

    let reopened = Storage::open(root.path()).expect("reopen upgraded storage");
    for canonical in built_in_prompt_presets() {
        let current = reopened
            .get_prompt_preset(&canonical.id)
            .expect("stable built-in prompt preset");
        assert_eq!(current.value, canonical);
        assert_eq!(current.revision, 3);
        assert_eq!(
            reopened
                .list_prompt_preset_revisions(&canonical.id)
                .expect("stable built-in prompt preset revisions")
                .len(),
            3
        );
    }
}

#[test]
fn reopening_storage_never_overwrites_a_non_application_reserved_preset() {
    let root = tempfile::tempdir().expect("temporary storage root");
    let storage = Storage::open(root.path()).expect("open storage");
    let mut collision = built_in_prompt_presets()[0].clone();
    let current = storage
        .get_prompt_preset(&collision.id)
        .expect("seeded built-in prompt preset");
    collision.metadata.provenance.source_kind = SourceKind::UserCreated;
    collision.metadata.provenance.source_id = None;
    storage
        .save_prompt_preset(&collision, Some(current.revision))
        .expect("install non-application reserved-id fixture");
    let database_path = storage
        .connection()
        .expect("active database connection")
        .path()
        .expect("active database path")
        .to_owned();
    drop(storage);

    let Err(error) = Storage::open(root.path()) else {
        panic!("reserved-id collision must fail closed");
    };
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);

    let database = Connection::open(database_path).expect("inspect rolled-back seed transaction");
    let (state_revision, revision_count) = database
        .query_row(
            "SELECT state.state_version,
                        (SELECT COUNT(*) FROM content_revisions
                         WHERE object_id = object.id)
                 FROM content_objects AS object
                 JOIN content_object_state AS state ON state.object_id = object.id
                 WHERE object.id = ?1 AND object.object_kind = 'prompt_preset'",
            [collision.id.as_str()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .expect("reserved-id state after rejected reopen");
    assert_eq!(state_revision, 2);
    assert_eq!(revision_count, 2);
}
