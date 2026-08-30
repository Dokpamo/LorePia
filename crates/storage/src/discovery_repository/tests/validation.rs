use super::support::*;
use super::*;

#[test]
fn discovery_operation_timestamps_must_follow_creation_and_start() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let draft = draft_session("operation-start-chronology");
    let begin = apply(&draft, ProviderDiscoveryAction::Begin, '8');
    let operation_id =
        DiscoveryOperationId::parse("operation-start-chronology").expect("operation id");
    let mut begin_write = write(begin, Some(operation_id.clone()), None);
    begin_write.occurred_at = now() + chrono::Duration::seconds(2);
    storage
        .begin_discovery_session(&draft, &begin_write)
        .expect("persist future-created discovery operation");
    let start_error = storage
        .mark_discovery_operation_started(&operation_id, now() + chrono::Duration::seconds(1))
        .expect_err("operation cannot start before creation");
    assert_eq!(start_error.code, CoreErrorCode::InvalidInput);
    assert_eq!(operation_status(&storage, &operation_id), "prepared");

    let native = seed_started_native_credential_commit(&storage, "native-finish-chronology");
    let (mut finish_write, attestation) =
        native_no_effect_completion(&storage, &native, &native.session);
    let started_at = storage
        .get_current_discovery_operation(&native.session.id)
        .expect("load started native operation")
        .expect("active native operation")
        .started_at
        .expect("native operation start timestamp");
    finish_write.occurred_at = started_at - chrono::Duration::milliseconds(1);
    let finish_error = storage
        .persist_native_no_effect_discovery_transition(&finish_write, &attestation)
        .expect_err("native operation cannot finish before its start");
    assert_eq!(finish_error.code, CoreErrorCode::InvalidInput);
    assert_eq!(operation_status(&storage, &native.operation_id), "started");
    assert!(
        storage
            .get_discovery_native_no_effect_attestation(&native.operation_id)
            .expect("load rejected reverse-time attestation")
            .is_none()
    );
}

#[test]
fn reopened_storage_registers_discovery_integrity_functions() {
    let root = tempdir().expect("temp directory");
    drop(Storage::open(root.path()).expect("create current-schema storage"));
    let reopened = Storage::open(root.path()).expect("reopen current-schema storage");
    let attestation = DiscoveryNativeNoEffectAttestationWrite::credential_slot_missing(
        DiscoveryOperationId::parse("operation-reopened-integrity-udf").expect("operation id"),
        format!("discovery-native-{}", uuid::Uuid::new_v4()),
        DiscoverySessionId::from("session-reopened-integrity-udf"),
        DiscoveryCommitAttemptId::parse("attempt-reopened-integrity-udf").expect("attempt id"),
        "1".repeat(64),
        ProviderConnectionId::from("connection-reopened-integrity-udf"),
    )
    .expect("native evidence binding");
    let mut plan_session = draft_session("reopened-integrity-plan-udf");
    plan_session.input.credential_ref = Some(CredentialRef(
        plan_session.input.connection_id.as_str().to_owned(),
    ));
    let plan = native_credential_commit_plan(
        &plan_session,
        "reopened-integrity-plan-udf",
        DiscoveryCommitAttemptId::parse("attempt-reopened-integrity-plan-udf")
            .expect("plan attempt id"),
        DiscoveryApprovalId::parse("approval-reopened-integrity-plan-udf")
            .expect("plan approval id"),
    );
    let plan_json = encode_commit_plan_json(&plan).expect("canonical commit plan JSON");
    let database = reopened.connection().expect("reopened database connection");
    let (
        native_evidence_sha256,
        ordinary_sha256,
        canonical_origin,
        canonical_header,
        upper_header,
        invalid_header,
        invalid_origin,
        canonical_plan_sha256,
        noncanonical_plan_sha256,
        invalid_plan_sha256,
    ) = database
        .query_row(
            "SELECT lorepia_native_no_effect_evidence_sha256(
                        1, ?1, ?2, ?3, ?4, ?5, ?6, ?7
                    ),
                    lorepia_sha256_hex('abc'),
                    lorepia_canonical_origin('https://provider.example/'),
                    lorepia_header_name('x-api-key'),
                    lorepia_header_name('X-API-Key'),
                    lorepia_header_name('bad header'),
                    lorepia_canonical_origin('https://provider.example/path'),
                    lorepia_discovery_commit_plan_sha256(?8),
                    lorepia_discovery_commit_plan_sha256(?8 || ' '),
                    lorepia_discovery_commit_plan_sha256(
                        json_set(?8, '$.forged_unknown_field', 1)
                    )",
            rusqlite::params![
                attestation.kind.as_str(),
                attestation.recovery_owner.as_str(),
                attestation.operation_id.as_str(),
                attestation.session_id.as_str(),
                attestation.commit_attempt_id.as_str(),
                attestation.commit_plan_sha256,
                attestation.connection_id.as_str(),
                plan_json,
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                ))
            },
        )
        .expect("execute integrity functions on reopened connection");
    assert_eq!(native_evidence_sha256, attestation.evidence_sha256);
    assert_eq!(ordinary_sha256, sha256_hex(b"abc"));
    assert_eq!(
        canonical_origin,
        CanonicalOrigin::parse("https://provider.example/")
            .expect("canonical origin")
            .to_string()
    );
    assert_eq!(canonical_header, "x-api-key");
    assert_eq!(upper_header.as_deref(), Some("x-api-key"));
    assert!(invalid_header.is_none());
    assert!(invalid_origin.is_none());
    assert_eq!(canonical_plan_sha256, sha256_hex(plan_json.as_bytes()));
    assert!(noncanonical_plan_sha256.is_none());
    assert!(invalid_plan_sha256.is_none());
}

#[test]
fn native_commit_start_rejects_self_consistent_malformed_plan() {
    for corruption in ["unknown_field", "template_version_zero", "noncanonical"] {
        assert_self_consistent_malformed_native_plan_rejected(corruption);
    }
}
