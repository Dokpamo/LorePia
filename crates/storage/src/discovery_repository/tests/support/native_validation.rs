use super::*;

pub(in crate::discovery_repository::tests) fn assert_native_attestation_and_terminal_schema_rejected(
    database: &rusqlite::Connection,
    attestation: &DiscoveryNativeNoEffectAttestationWrite,
    attested_at: chrono::DateTime<Utc>,
) {
    let insert_error = database
        .execute(
            "INSERT INTO provider_discovery_native_no_effect_attestations (
                 operation_id, session_id, commit_attempt_id, commit_plan_sha256,
                 connection_id, attestation_kind, evidence_sha256, recovery_owner,
                 schema_version, redaction_version, attested_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, 1, ?9)",
            rusqlite::params![
                attestation.operation_id.as_str(),
                attestation.session_id.as_str(),
                attestation.commit_attempt_id.as_str(),
                attestation.commit_plan_sha256,
                attestation.connection_id.as_str(),
                attestation.kind.as_str(),
                attestation.evidence_sha256,
                attestation.recovery_owner.as_str(),
                attested_at.to_rfc3339(),
            ],
        )
        .expect_err("schema authority view must reject detached native authority");
    let insert_error = insert_error.to_string();
    assert!(
        insert_error.contains("detached")
            || insert_error.contains("requires physical execution authority"),
        "unexpected native attestation rejection: {insert_error}"
    );
    let transition_error = database
        .execute(
            "UPDATE provider_discovery_operations
             SET status = 'interrupted', finished_at = ?2, updated_at = ?2
             WHERE id = ?1",
            rusqlite::params![attestation.operation_id.as_str(), attested_at.to_rfc3339()],
        )
        .expect_err("schema must reject a detached native terminal transition");
    assert!(
        transition_error
            .to_string()
            .contains("illegal discovery operation status transition"),
        "unexpected native terminal rejection: {transition_error}"
    );
}

fn malformed_credential_grant_json(original: &str, corruption: &str) -> String {
    if corruption == "credential_origin" {
        let grant_value =
            serde_json::from_str::<Value>(original).expect("decode canonical credential grant");
        let original_origin = grant_value["origin"]
            .as_str()
            .expect("credential grant origin");
        let forged_origin = format!("{}/path", original_origin.trim_end_matches('/'));
        original.replacen(
            &format!("\"origin\":\"{original_origin}\""),
            &format!("\"origin\":\"{forged_origin}\""),
            1,
        )
    } else {
        original.replacen(
            "\"auth_binding\":{\"kind\":\"bearer_header\"}",
            "\"auth_binding\":{\"kind\":\"header_api_key\",\"header_name\":\"X-API-Key\"}",
            1,
        )
    }
}

fn corrupt_native_root_approval(
    database: &rusqlite::Connection,
    fixture: &NativeNoEffectFixture,
    credential_approval_id: &DiscoveryApprovalId,
    corruption: &str,
) {
    let approval_id = if corruption == "review_grant_digest" {
        let raw = database
            .query_row(
                "SELECT id FROM provider_discovery_approvals
                 WHERE session_id = ?1 AND approval_kind = 'review'",
                [fixture.session.id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .expect("load review approval id");
        DiscoveryApprovalId::parse(raw).expect("review approval id")
    } else {
        credential_approval_id.clone()
    };
    let original_grant_json = database
        .query_row(
            "SELECT grant_json FROM provider_discovery_approvals WHERE id = ?1",
            [approval_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .expect("load canonical approval grant");
    let immutable_error = database
        .execute(
            "UPDATE provider_discovery_approvals
             SET grant_sha256 = ?2 WHERE id = ?1",
            rusqlite::params![approval_id.as_str(), "0".repeat(64)],
        )
        .expect_err("immutable approval trigger must preserve the root grant");
    assert!(immutable_error.to_string().contains("immutable"));
    database
        .execute_batch("DROP TRIGGER provider_discovery_approval_no_update;")
        .expect("drop approval update guard only for corruption fixture");
    if matches!(
        corruption,
        "credential_grant_digest" | "review_grant_digest"
    ) {
        database
            .execute(
                "UPDATE provider_discovery_approvals
                 SET grant_sha256 = ?2 WHERE id = ?1",
                rusqlite::params![approval_id.as_str(), "0".repeat(64)],
            )
            .expect("inject mismatched approval grant digest");
    } else {
        let forged_grant_json = malformed_credential_grant_json(&original_grant_json, corruption);
        assert_ne!(forged_grant_json, original_grant_json);
        database
            .execute(
                "UPDATE provider_discovery_approvals
                 SET grant_json = ?2, grant_sha256 = ?3 WHERE id = ?1",
                rusqlite::params![
                    approval_id.as_str(),
                    forged_grant_json,
                    sha256_hex(forged_grant_json.as_bytes()),
                ],
            )
            .expect("inject canonical-hash malformed credential approval");
    }
}

fn corrupt_native_root_plan_digest(
    database: &rusqlite::Connection,
    fixture: &NativeNoEffectFixture,
) {
    let immutable_error = database
        .execute(
            "UPDATE provider_discovery_commit_attempts
             SET plan_json = plan_json || ' ' WHERE id = ?1",
            [fixture.attempt_id.as_str()],
        )
        .expect_err("immutable attempt trigger must preserve the root plan");
    assert!(immutable_error.to_string().contains("immutable"));
    database
        .execute_batch("DROP TRIGGER provider_discovery_commit_identity_no_update;")
        .expect("drop attempt update guard only for corruption fixture");
    database
        .execute(
            "UPDATE provider_discovery_commit_attempts
             SET plan_json = plan_json || ' ' WHERE id = ?1",
            [fixture.attempt_id.as_str()],
        )
        .expect("inject plan bytes that no longer match the stored digest");
}

fn malformed_commit_plan_json(original: &str, corruption: &str) -> String {
    let malformed = match corruption {
        "unknown_field" => format!(
            "{},\"forged_unknown_field\":true}}",
            original.strip_suffix('}').expect("commit plan object")
        ),
        "template_version_zero" => {
            original.replacen("\"template_version\":1", "\"template_version\":0", 1)
        }
        "noncanonical" => format!("{original}\n"),
        _ => panic!("unknown malformed plan case: {corruption}"),
    };
    assert_ne!(malformed, original);
    malformed
}

fn corrupt_native_root_with_self_consistent_plan(
    database: &rusqlite::Connection,
    fixture: &NativeNoEffectFixture,
    corruption: &str,
) -> String {
    let (action_id, original_plan_json) = database
        .query_row(
            "SELECT action_id, plan_json
             FROM provider_discovery_commit_attempts WHERE id = ?1",
            [fixture.attempt_id.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .expect("load canonical commit start plan");
    let malformed_plan_json = malformed_commit_plan_json(&original_plan_json, corruption);
    let malformed_plan_sha256 = sha256_hex(malformed_plan_json.as_bytes());
    database
        .execute_batch(
            "DROP TRIGGER provider_discovery_commit_identity_no_update;
             DROP TRIGGER provider_discovery_session_revision_guard;
             DROP TRIGGER provider_discovery_receipt_no_update;",
        )
        .expect("drop immutable plan bindings only for corruption fixture");
    database
        .execute(
            "UPDATE provider_discovery_commit_attempts
             SET plan_json = ?2, plan_sha256 = ?3 WHERE id = ?1",
            rusqlite::params![
                fixture.attempt_id.as_str(),
                malformed_plan_json,
                malformed_plan_sha256,
            ],
        )
        .expect("inject self-consistent malformed commit plan");
    database
        .execute(
            "UPDATE provider_discovery_sessions
             SET commit_plan_sha256 = ?2 WHERE id = ?1",
            rusqlite::params![fixture.session.id.as_str(), malformed_plan_sha256],
        )
        .expect("rebind active session to malformed commit plan");
    database
        .execute(
            "UPDATE provider_discovery_action_receipts
             SET response_json = replace(response_json, ?2, ?3)
             WHERE action_id = ?1",
            rusqlite::params![action_id, fixture.plan_sha256, malformed_plan_sha256,],
        )
        .expect("rebind commit start receipt to malformed commit plan");
    malformed_plan_sha256
}

pub(in crate::discovery_repository::tests) fn assert_self_consistent_malformed_native_plan_rejected(
    corruption: &str,
) {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let fixture = seed_started_native_credential_commit(
        &storage,
        &format!("native-self-consistent-malformed-plan-{corruption}"),
    );
    let database = storage.connection().expect("database connection");
    let malformed_plan_sha256 =
        corrupt_native_root_with_self_consistent_plan(&database, &fixture, corruption);
    drop(database);

    let mut active_session = fixture.session.clone();
    active_session.commit_plan_sha256 = Some(malformed_plan_sha256.clone());
    active_session
        .validate()
        .expect("malformed-plan session binding remains structurally valid");
    let interrupted = apply(
        &active_session,
        ProviderDiscoveryAction::Interrupt {
            operation: DiscoveryOperationKind::AtomicCommit,
            outcome: DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
        },
        '7',
    );
    let mut write = write(
        interrupted,
        None,
        Some(DiscoveryCompletedOperationWrite {
            id: fixture.operation_id.clone(),
            outcome: DurableOperationOutcome::AttestedNoExternalEffect,
        }),
    );
    write.occurred_at = now() + chrono::Duration::milliseconds(2);
    let attestation = DiscoveryNativeNoEffectAttestationWrite::credential_slot_missing(
        fixture.operation_id.clone(),
        raw_test_native_physical_authority_id(&storage, &fixture.operation_id),
        fixture.session.id.clone(),
        fixture.attempt_id.clone(),
        malformed_plan_sha256.clone(),
        fixture.session.input.connection_id.clone(),
    )
    .expect("malformed-plan native attestation");
    let database = storage.connection().expect("schema rejection database");
    assert_native_attestation_and_terminal_schema_rejected(
        &database,
        &attestation,
        write.occurred_at,
    );
    drop(database);

    let persist_error = storage
        .persist_native_no_effect_discovery_transition(&write, &attestation)
        .expect_err("public writer must reject a malformed typed commit plan");
    assert_eq!(persist_error.code, CoreErrorCode::StorageCorrupted);
    let authority_error = storage
        .validate_discovery_credential_install_operation_authority(
            &fixture.session.id,
            &fixture.attempt_id,
            &malformed_plan_sha256,
            &fixture.operation_id,
        )
        .expect_err("runtime authority must reject a malformed typed commit plan");
    assert_eq!(authority_error.code, CoreErrorCode::StorageCorrupted);
}

pub(in crate::discovery_repository::tests) fn assert_malformed_native_commit_root_rejected(
    corruption: &str,
) {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let fixture = seed_started_native_credential_commit(
        &storage,
        &format!("native-malformed-approval-{corruption}"),
    );
    let credential_approval_id = storage
        .get_discovery_commit_attempt(&fixture.attempt_id)
        .expect("load credential commit attempt")
        .plan
        .credential_approval_id
        .expect("credential approval id");
    storage
        .validate_discovery_credential_install_operation_authority(
            &fixture.session.id,
            &fixture.attempt_id,
            &fixture.plan_sha256,
            &fixture.operation_id,
        )
        .expect("intact credential approval root grants native authority");
    let (write, attestation) = native_no_effect_completion(&storage, &fixture, &fixture.session);
    let database = storage.connection().expect("database connection");
    if corruption == "plan_digest" {
        corrupt_native_root_plan_digest(&database, &fixture);
    } else {
        corrupt_native_root_approval(&database, &fixture, &credential_approval_id, corruption);
    }
    assert_native_attestation_and_terminal_schema_rejected(
        &database,
        &attestation,
        write.occurred_at,
    );
    drop(database);

    let persist_error = storage
        .persist_native_no_effect_discovery_transition(&write, &attestation)
        .expect_err("public writer must reject a malformed approval root");
    assert_eq!(persist_error.code, CoreErrorCode::StorageCorrupted);
    let authority_error = storage
        .validate_discovery_credential_install_operation_authority(
            &fixture.session.id,
            &fixture.attempt_id,
            &fixture.plan_sha256,
            &fixture.operation_id,
        )
        .expect_err("runtime authority must reject a malformed approval root");
    assert_eq!(authority_error.code, CoreErrorCode::StorageCorrupted);
}
