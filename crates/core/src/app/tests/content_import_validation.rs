#[test]
fn import_and_restart_restore_library() {
    let (root, core, _) = imported_core();
    drop(core);
    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen");
    assert_eq!(reopened.list_characters().expect("library").len(), 1);
}

#[test]
fn import_uses_an_owned_snapshot_and_cleans_it_after_commit() {
    let root = tempdir().expect("temp root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let mut card = NamedTempFile::new_in(root.path()).expect("card");
    write!(
        card,
        r#"{{"spec":"chara_card_v3","data":{{"name":"Snapshot","description":"Safe"}}}}"#
    )
    .expect("write card");

    let inspection = core.inspect_import(card.path()).expect("inspect");
    fs::write(card.path(), b"changed after inspection").expect("mutate original");
    let character = core.commit_import(&inspection.id).expect("commit snapshot");

    assert_eq!(character.name, "Snapshot");
    assert!(
        fs::read_dir(core.inner.storage.staging_dir())
            .expect("staging directory")
            .next()
            .is_none(),
        "committed snapshots must be removed"
    );
}

#[test]
fn discard_and_restart_cleanup_owned_staging_files() {
    let root = tempdir().expect("temp root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let mut card = NamedTempFile::new_in(root.path()).expect("card");
    write!(
        card,
        r#"{{"spec":"chara_card_v3","data":{{"name":"Discard","description":"Safe"}}}}"#
    )
    .expect("write card");
    let inspection = core.inspect_import(card.path()).expect("inspect");
    core.discard_import(&inspection.id).expect("discard");
    assert!(
        fs::read_dir(core.inner.storage.staging_dir())
            .expect("staging directory")
            .next()
            .is_none()
    );

    let abandoned = core
        .inner
        .storage
        .staging_dir()
        .join("inspection-abandoned.json");
    fs::write(&abandoned, b"abandoned").expect("abandoned staging file");
    drop(core);
    let _reopened = open_core_after_drop(root.path());
    assert!(
        !abandoned.exists(),
        "restart must clean abandoned snapshots"
    );
}

#[test]
fn concurrent_commits_atomically_claim_one_inspection() {
    let root = tempdir().expect("temp root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let mut card = NamedTempFile::new_in(root.path()).expect("card");
    write!(
        card,
        r#"{{"spec":"chara_card_v3","data":{{"name":"Claim","description":"Safe"}}}}"#
    )
    .expect("write card");
    let inspection = core.inspect_import(card.path()).expect("inspect");
    let barrier = Arc::new(Barrier::new(3));
    let mut workers = Vec::new();
    for _ in 0..2 {
        let core = core.clone();
        let inspection_id = inspection.id.clone();
        let barrier = Arc::clone(&barrier);
        workers.push(thread::spawn(move || {
            barrier.wait();
            core.commit_import(&inspection_id)
        }));
    }
    barrier.wait();
    let results = workers
        .into_iter()
        .map(|worker| worker.join().expect("commit worker"))
        .collect::<Vec<_>>();

    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let loser = results
        .iter()
        .find_map(|result| result.as_ref().err())
        .expect("one losing commit");
    assert_eq!(loser.code, CoreErrorCode::NotFound);
    assert_eq!(core.list_characters().expect("characters").len(), 1);
}

#[test]
fn concurrent_commit_and_discard_have_one_atomic_winner() {
    let root = tempdir().expect("temp root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let mut card = NamedTempFile::new_in(root.path()).expect("card");
    write!(
        card,
        r#"{{"spec":"chara_card_v3","data":{{"name":"Race","description":"Safe"}}}}"#
    )
    .expect("write card");
    let inspection = core.inspect_import(card.path()).expect("inspect");
    let barrier = Arc::new(Barrier::new(3));
    let commit_core = core.clone();
    let commit_id = inspection.id.clone();
    let commit_barrier = Arc::clone(&barrier);
    let commit = thread::spawn(move || {
        commit_barrier.wait();
        commit_core.commit_import(&commit_id)
    });
    let discard_core = core.clone();
    let discard_id = inspection.id.clone();
    let discard_barrier = Arc::clone(&barrier);
    let discard = thread::spawn(move || {
        discard_barrier.wait();
        discard_core.discard_import(&discard_id)
    });
    barrier.wait();
    let commit = commit.join().expect("commit worker");
    let discard = discard.join().expect("discard worker");

    assert_ne!(commit.is_ok(), discard.is_ok());
    let loser = commit
        .as_ref()
        .err()
        .or_else(|| discard.as_ref().err())
        .expect("one losing operation");
    assert_eq!(loser.code, CoreErrorCode::NotFound);
    assert_eq!(
        core.list_characters().expect("characters").len(),
        usize::from(commit.is_ok())
    );
}

#[test]
fn precommit_failure_restores_the_claim_for_a_safe_retry() {
    let root = tempdir().expect("temp root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let mut card = NamedTempFile::new_in(root.path()).expect("card");
    let card_bytes = br#"{"spec":"chara_card_v3","data":{"name":"Retry","description":"Safe"}}"#;
    card.write_all(card_bytes).expect("write card");
    let inspection = core.inspect_import(card.path()).expect("inspect");
    let database_path = hard_crash_database_path(root.path());
    let database = rusqlite::Connection::open(database_path).expect("open database");
    database
        .execute_batch(
            "CREATE TRIGGER test_reject_character_import_journal
                 BEFORE INSERT ON import_jobs
                 BEGIN
                     SELECT RAISE(ABORT, 'synthetic character import failure');
                 END;",
        )
        .expect("install precommit failure injector");

    let error = core
        .commit_import(&inspection.id)
        .expect_err("precommit failure");
    assert_eq!(error.code, CoreErrorCode::StorageUnavailable);
    assert!(
        core.inner
            .pending_imports
            .read()
            .expect("pending imports")
            .contains_key(&inspection.id),
        "a definitely uncommitted claim must be restored"
    );

    database
        .execute("DROP TRIGGER test_reject_character_import_journal", [])
        .expect("remove precommit failure injector");
    let character = core.commit_import(&inspection.id).expect("safe retry");
    assert_eq!(character.name, "Retry");
    assert_eq!(core.list_characters().expect("characters").len(), 1);
}

#[test]
fn user_message_and_provider_fields_have_utf8_safe_inclusive_bounds() {
    let exact_message = "😀".repeat(MAX_USER_MESSAGE_CHARS);
    assert_eq!(exact_message.len(), MAX_USER_MESSAGE_BYTES);
    validate_bounded_text(
        "message text",
        &exact_message,
        MAX_USER_MESSAGE_BYTES,
        MAX_USER_MESSAGE_CHARS,
    )
    .expect("exact message boundary");
    let message_error = validate_bounded_text(
        "message text",
        &format!("{exact_message}😀"),
        MAX_USER_MESSAGE_BYTES,
        MAX_USER_MESSAGE_CHARS,
    )
    .expect_err("message over boundary");
    assert_eq!(message_error.code, CoreErrorCode::InvalidInput);
    assert_eq!(
        message_error.message,
        "message text exceeds the 65536-byte or 16384-character limit"
    );

    for (field, max_bytes, max_chars) in [
        (
            "provider profile id",
            MAX_PROVIDER_ID_BYTES,
            MAX_PROVIDER_ID_CHARS,
        ),
        (
            "provider display name",
            MAX_PROVIDER_DISPLAY_NAME_BYTES,
            MAX_PROVIDER_DISPLAY_NAME_CHARS,
        ),
        (
            "provider base URL",
            MAX_PROVIDER_BASE_URL_BYTES,
            MAX_PROVIDER_BASE_URL_CHARS,
        ),
        (
            "provider model",
            MAX_PROVIDER_MODEL_BYTES,
            MAX_PROVIDER_MODEL_CHARS,
        ),
    ] {
        let exact = "😀".repeat(max_chars);
        assert_eq!(exact.len(), max_bytes);
        validate_bounded_text(field, &exact, max_bytes, max_chars)
            .expect("exact provider field boundary");
        assert!(validate_bounded_text(field, &format!("{exact}😀"), max_bytes, max_chars).is_err());
    }
}

#[test]
fn oversized_user_input_and_provider_fields_are_not_persisted() {
    let (_root, core, character) = imported_core();
    let conversation = core.open_conversation(&character.id).expect("conversation");
    let error = core
        .send_message_with_provider(
            &conversation.id,
            &"😀".repeat(MAX_USER_MESSAGE_CHARS + 1),
            "static".to_owned(),
            None,
            Arc::new(StaticProvider::new("unused")),
        )
        .expect_err("oversized message");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert!(
        core.list_messages(&conversation.id)
            .expect("messages")
            .is_empty()
    );

    let profile_error = core
        .upsert_provider_profile(ProviderProfile {
            id: "provider".to_owned(),
            display_name: "Provider".to_owned(),
            base_url: "http://127.0.0.1:11434/v1".to_owned(),
            model: "😀".repeat(MAX_PROVIDER_MODEL_CHARS + 1),
            timeout_seconds: 30,
        })
        .expect_err("oversized model");
    assert_eq!(profile_error.code, CoreErrorCode::InvalidInput);
    assert_eq!(
        profile_error.message,
        "provider model exceeds the 1024-byte or 256-character limit"
    );
    assert!(core.list_provider_profiles().expect("profiles").is_empty());
}

#[test]
fn every_provider_profile_string_is_bounded_before_storage() {
    let root = tempdir().expect("temp root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let valid = || ProviderProfile {
        id: "provider".to_owned(),
        display_name: "Provider".to_owned(),
        base_url: "http://127.0.0.1:11434/v1".to_owned(),
        model: "model".to_owned(),
        timeout_seconds: 30,
    };
    let mut cases = Vec::new();
    let mut oversized_id = valid();
    oversized_id.id = "😀".repeat(MAX_PROVIDER_ID_CHARS + 1);
    cases.push(("provider profile id", oversized_id));
    let mut oversized_display = valid();
    oversized_display.display_name = "😀".repeat(MAX_PROVIDER_DISPLAY_NAME_CHARS + 1);
    cases.push(("provider display name", oversized_display));
    let mut oversized_url = valid();
    oversized_url.base_url = format!(
        "http://127.0.0.1/{}",
        "a".repeat(MAX_PROVIDER_BASE_URL_BYTES)
    );
    cases.push(("provider base URL", oversized_url));
    let mut oversized_model = valid();
    oversized_model.model = "😀".repeat(MAX_PROVIDER_MODEL_CHARS + 1);
    cases.push(("provider model", oversized_model));

    for (field, profile) in cases {
        let error = core
            .upsert_provider_profile(profile)
            .expect_err("oversized provider field");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(error.message.starts_with(field), "{:?}", error.message);
    }
    assert!(core.list_provider_profiles().expect("profiles").is_empty());
}
