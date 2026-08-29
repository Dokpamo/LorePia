pub(super) fn drop_additive_migrations(
    connection: &rusqlite::Connection,
    migrations: &[(u32, &str)],
) {
    for (version, migration) in migrations {
        for (object_type, name) in migration
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
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
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
