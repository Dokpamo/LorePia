pub(super) fn drop_additive_migrations(
    connection: &rusqlite::Connection,
    migrations: &[(u32, &str)],
) {
    for (version, migration) in migrations {
        for (object_type, name) in additive_migration_objects(migration).into_iter().rev() {
            connection
                .execute(&format!("DROP {object_type} \"{name}\""), [])
                .unwrap_or_else(|error| {
                    panic!("drop schema-{version} {object_type} {name}: {error}")
                });
        }
        assert_eq!(
            connection
                .execute(
                    "DELETE FROM schema_migrations WHERE version = ?1",
                    [version],
                )
                .unwrap_or_else(|error| panic!("remove schema-{version} registry row: {error}")),
            1
        );
    }
}

pub(super) fn drop_post_schema_37_additive_migrations(connection: &rusqlite::Connection) {
    const MIGRATION_0038: &str =
        include_str!("../../../storage/migrations/0038_conversation_speakers.sql");
    const MIGRATION_0039: &str =
        include_str!("../../../storage/migrations/0039_runtime_model_audit.sql");
    const MIGRATION_0040: &str =
        include_str!("../../../storage/migrations/0040_portable_runtime_state.sql");
    const SCHEMA_40_OBJECTS: &[(&str, &str)] = &[
        ("TABLE", "portable_runtime_branch_epochs"),
        ("TRIGGER", "portable_runtime_branch_epoch_on_branch_insert"),
        ("TABLE", "portable_runtime_state_sequence"),
        ("TABLE", "portable_runtime_states"),
        ("INDEX", "portable_runtime_states_lru"),
        ("TRIGGER", "portable_runtime_state_scope_guard_insert"),
        ("TRIGGER", "portable_runtime_state_scope_guard_update"),
    ];
    assert_additive_migration_objects(MIGRATION_0040, SCHEMA_40_OBJECTS);
    drop_additive_migrations(
        connection,
        &[
            (40, MIGRATION_0040),
            (39, MIGRATION_0039),
            (38, MIGRATION_0038),
        ],
    );
}

pub(super) fn assert_additive_migration_objects(migration: &str, expected: &[(&str, &str)]) {
    let actual = additive_migration_objects(migration);
    assert_eq!(
        actual.as_slice(),
        expected,
        "additive migration object parser did not find the exact expected object set"
    );
}

fn additive_migration_objects(migration: &str) -> Vec<(&str, &str)> {
    migration
        .lines()
        .filter_map(|line| {
            let mut tokens = line.split_ascii_whitespace();
            (tokens.next() == Some("CREATE")).then_some(())?;
            let object_type = tokens.next()?;
            let (object_type, name) = if object_type == "UNIQUE" {
                (tokens.next()?, tokens.next()?)
            } else {
                (object_type, tokens.next()?)
            };
            Some((object_type, name.trim_end_matches(';')))
        })
        .collect()
}
