
fn move_persona_sort_key(
    storage: &Storage,
    persona: &StoredRevision<Persona>,
    updated_at: DateTime<Utc>,
) {
    let mut moved = persona.value.clone();
    moved.description = "move the persona sort key after the first page".to_owned();
    moved.updated_at = updated_at;
    storage
        .save_persona(&moved, Some(persona.revision))
        .expect("move persona sort key through an authoritative revision switch");
}

#[test]
fn persona_keyset_pages_recover_all_records_and_honor_the_id_tie_breaker() {
    let root = tempfile::tempdir().expect("temporary persona page root");
    let storage = Storage::open(root.path()).expect("open persona page storage");
    let now = Utc::now();
    let local_user_id = storage
        .load_settings()
        .expect("load local identity")
        .local_user_id;
    for index in 0..101 {
        storage
            .save_persona(
                &Persona {
                    id: PersonaId::from(format!("persona-page-{index:03}")),
                    name: format!("Persona {index:03}"),
                    description: String::new(),
                    schema_version: 1,
                    provenance: Provenance {
                        source_kind: SourceKind::UserCreated,
                        source_id: Some(local_user_id.as_str().to_owned()),
                        source_hash: None,
                        author: None,
                        license: None,
                        imported_at: None,
                    },
                    created_at: now,
                    updated_at: now,
                },
                None,
            )
            .expect("save paged persona");
    }
    let first_page = storage
        .list_personas_page(None, None, 100)
        .expect("first persona page");
    let PersonaCatalogPage::Page {
        catalog_revision,
        items: first,
    } = first_page
    else {
        panic!("an initial persona page cannot require a restart");
    };
    assert_eq!(first.len(), 100);
    let boundary = first.last().expect("page boundary");
    let second_page = storage
        .list_personas_page(
            Some(&catalog_revision),
            Some((&boundary.updated_at, &boundary.value.id)),
            100,
        )
        .expect("second persona page");
    let PersonaCatalogPage::Page { items: second, .. } = second_page else {
        panic!("an unchanged persona catalog cannot require a restart");
    };
    assert_eq!(second.len(), 1);
    let ids = first
        .iter()
        .chain(&second)
        .map(|persona| persona.value.id.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        ids.len(),
        101,
        "keyset pages must recover every persona once"
    );

    let newest = first.first().expect("newest persona");
    let before_newest_id = PersonaId::from("persona-page-");
    let equal_timestamp_result = storage
        .list_personas_page(
            Some(&catalog_revision),
            Some((&newest.updated_at, &before_newest_id)),
            1,
        )
        .expect("equal-timestamp page");
    let PersonaCatalogPage::Page {
        items: equal_timestamp_page,
        ..
    } = equal_timestamp_result
    else {
        panic!("an unchanged persona catalog cannot require a restart");
    };
    assert_eq!(
        equal_timestamp_page
            .first()
            .expect("equal timestamp result")
            .value
            .id,
        newest.value.id,
        "the ascending identifier must break an equal timestamp boundary",
    );

    move_persona_sort_key(&storage, newest, now + chrono::Duration::seconds(1));
    assert!(matches!(
        storage
            .list_personas_page(
                Some(&catalog_revision),
                Some((&boundary.updated_at, &boundary.value.id)),
                100,
            )
            .expect("sort-key drift must be a typed restart"),
        PersonaCatalogPage::RestartRequired { .. }
    ));
}
