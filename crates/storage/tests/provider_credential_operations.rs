use chrono::Utc;
use lorepia_domain::{
    AuthBinding, CanonicalOrigin, ConnectionConfig, ConnectionConfigEntry, ConnectionConfigValue,
    ConnectionStatus, CoreErrorCode, CredentialRedirectPolicy, CredentialRef, CredentialScope,
    EndpointPath, ProviderConnection, ProviderConnectionId, ProviderNetworkMode, ProviderProfile,
    ProviderTemplateId,
};
use lorepia_storage::{
    ProviderCredentialAccessAuthority, ProviderCredentialObservedStatus,
    ProviderCredentialOperationKind, ProviderCredentialOperationStatus,
    ProviderCredentialOutcomeCode, Storage, StoredProviderCredentialOperation,
};
use rusqlite::{Connection, functions::FunctionFlags};
use sha2::{Digest, Sha256};
use tempfile::tempdir;

const TEMPLATE_ID: &str = "custom-openai-chat-v1";

#[test]
fn fresh_database_has_durable_provider_credential_operation_journal() {
    let root = tempdir().expect("temporary data root");
    drop(Storage::open(root.path()).expect("open fresh storage"));

    let database = Connection::open(active_database_path(root.path()))
        .expect("open generated active database");
    let sql = database
        .query_row(
            "SELECT sql
             FROM sqlite_schema
             WHERE type = 'table' AND name = 'provider_credential_operations'",
            [],
            |row| row.get::<_, String>(0),
        )
        .expect("provider credential operation journal must exist");

    assert!(sql.contains("plan_json"));
    assert!(sql.contains("plan_sha256"));
    assert!(sql.contains("outcome_unknown"));
    assert!(sql.contains("cleanup_required"));
    assert!(sql.contains("native_durability_unknown"));
    assert!(sql.contains("native_predecessor_durability_unknown"));
}

#[test]
fn explicit_native_durability_barrier_survives_reopen_and_visibility_reconciliation() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    insert_credential_connection(&storage, "durability-barrier");
    let prepared = prepare_authority_bound_install(&storage, "durability-barrier");
    storage
        .start_provider_credential_operation(&prepared.plan.operation_id, &prepared.plan_sha256)
        .expect("start exact native effect");
    let blocked = storage
        .mark_provider_credential_durability_recovery_required(
            &prepared.plan.operation_id,
            &prepared.plan_sha256,
            false,
        )
        .expect("persist explicit native durability barrier");
    assert_eq!(
        blocked.outcome_code,
        Some(ProviderCredentialOutcomeCode::NativeDurabilityUnknown)
    );
    assert_eq!(
        serde_json::to_string(&blocked.outcome_code.expect("outcome code"))
            .expect("serialize outcome code"),
        "\"native_durability_unknown\""
    );
    assert_eq!(
        serde_json::from_str::<ProviderCredentialOutcomeCode>(
            "\"native_predecessor_durability_unknown\""
        )
        .expect("deserialize predecessor durability outcome"),
        ProviderCredentialOutcomeCode::NativePredecessorDurabilityUnknown
    );
    assert_eq!(
        serde_json::from_str::<ProviderCredentialOutcomeCode>("\"native_status_unreadable\"")
            .expect("legacy unreadable outcome remains serde-compatible"),
        ProviderCredentialOutcomeCode::NativeStatusUnreadable
    );
    storage
        .ensure_provider_credential_access_settled(&ProviderConnectionId::from(
            "durability-barrier",
        ))
        .expect_err("durability barrier blocks credential access");
    drop(storage);

    let database = Connection::open(active_database_path(root.path()))
        .expect("open durability-barrier database wire");
    let persisted_outcome = database
        .query_row(
            "SELECT outcome_code FROM provider_credential_operations WHERE id = ?1",
            [&prepared.plan.operation_id],
            |row| row.get::<_, String>(0),
        )
        .expect("read stable durability-barrier outcome wire");
    assert_eq!(persisted_outcome, "native_durability_unknown");
    drop(database);

    let reopened = Storage::open(root.path()).expect("reopen durability barrier");
    for observed in [
        ProviderCredentialObservedStatus::Available,
        ProviderCredentialObservedStatus::Missing,
    ] {
        let preserved = reopened
            .reconcile_provider_credential_operation(
                &prepared.plan.operation_id,
                &prepared.plan_sha256,
                observed,
            )
            .expect("automatic visibility reconciliation remains fail closed");
        assert_eq!(
            preserved.outcome_code,
            Some(ProviderCredentialOutcomeCode::NativeDurabilityUnknown)
        );
        assert_eq!(
            preserved.status,
            ProviderCredentialOperationStatus::CleanupRequired
        );
    }
    let repaired = reopened
        .attest_provider_credential_durability_repaired(
            &prepared.plan.operation_id,
            &prepared.plan_sha256,
        )
        .expect("explicit exact delete repair clears only the durability barrier");
    assert_eq!(
        repaired.outcome_code,
        Some(ProviderCredentialOutcomeCode::ConnectionChanged)
    );
    let terminal = reopened
        .reconcile_provider_credential_operation(
            &prepared.plan.operation_id,
            &prepared.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("post-repair Missing may terminalize ordinary cleanup");
    assert!(!terminal.status.is_unresolved());
}

#[test]
fn predecessor_durability_barrier_reopens_with_its_exact_action_identity() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    insert_credential_connection(&storage, "predecessor-durability-barrier");
    install_owned_credential(&storage, "predecessor-durability-barrier");
    let replacement = prepare_authority_bound_install(&storage, "predecessor-durability-barrier");
    storage
        .start_provider_credential_operation(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
        )
        .expect("start replacement B");
    storage
        .attest_provider_credential_predecessor_delete_intent(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("persist predecessor A delete intent");
    storage
        .mark_provider_credential_predecessor_durability_recovery_required(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
            false,
        )
        .expect("persist predecessor-specific durability barrier");
    drop(storage);

    let reopened = Storage::open(root.path()).expect("reopen predecessor barrier");
    let blocked = reopened
        .list_unresolved_provider_credential_operations()
        .expect("list predecessor barrier")
        .pop()
        .expect("one unresolved replacement");
    assert_eq!(
        blocked.outcome_code,
        Some(ProviderCredentialOutcomeCode::NativePredecessorDurabilityUnknown)
    );
    reopened
        .reconcile_provider_credential_operation(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("automatic Missing remains blocked");
    assert_eq!(
        reopened
            .list_unresolved_provider_credential_operations()
            .expect("predecessor action identity is durable")[0]
            .outcome_code,
        Some(ProviderCredentialOutcomeCode::NativePredecessorDurabilityUnknown)
    );
}

#[test]
fn generic_credential_install_is_rejected_without_journaling() {
    for observed in [
        ProviderCredentialObservedStatus::Missing,
        ProviderCredentialObservedStatus::Available,
        ProviderCredentialObservedStatus::Unreadable,
    ] {
        let root = tempdir().expect("temporary data root");
        let storage = Storage::open(root.path()).expect("open storage");
        insert_credential_connection(&storage, "guarded-install");
        storage
            .prepare_provider_credential_operation(
                &ProviderConnectionId::from("guarded-install"),
                ProviderCredentialOperationKind::Install,
                observed,
            )
            .expect_err("generic installation must require derived physical-slot authority");
        assert!(
            storage
                .list_unresolved_provider_credential_operations()
                .expect("list journal")
                .is_empty()
        );
    }
}

#[test]
fn authority_bound_credential_install_recovery_is_conservative() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    insert_credential_connection(&storage, "prepared-install");
    let authority = storage
        .propose_provider_credential_install_authority(&ProviderConnectionId::from(
            "prepared-install",
        ))
        .expect("propose authority-bound install");
    let prepared = storage
        .prepare_provider_credential_operation_with_install_authority(
            &ProviderConnectionId::from("prepared-install"),
            ProviderCredentialOperationKind::Install,
            ProviderCredentialObservedStatus::Missing,
            Some(&authority),
        )
        .expect("prepare authority-bound install");
    let recovered = storage
        .reconcile_provider_credential_operation(
            &prepared.plan.operation_id,
            &prepared.plan_sha256,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("classify unexpected pre-start available slot");
    assert_eq!(
        recovered.status,
        ProviderCredentialOperationStatus::OutcomeUnknown,
        "an available slot cannot be adopted unless this exact operation durably started"
    );
    storage
        .ensure_provider_credential_access_settled(&ProviderConnectionId::from("prepared-install"))
        .expect_err("unknown native outcome must block provider use");
}

#[test]
fn started_install_recovers_only_from_immutable_missing_preflight() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    insert_credential_connection(&storage, "started-install");
    let prepared = prepare_authority_bound_install(&storage, "started-install");
    let started = storage
        .start_provider_credential_operation(&prepared.plan.operation_id, &prepared.plan_sha256)
        .expect("start install");
    assert_eq!(started.status, ProviderCredentialOperationStatus::Started);

    drop(storage);
    let reopened = Storage::open(root.path()).expect("reopen interrupted install");
    let completed = reopened
        .fence_started_provider_credential_operation_for_recovery(
            &prepared.plan.operation_id,
            &prepared.plan_sha256,
        )
        .expect("fence exact started install before visibility");
    assert_eq!(
        completed.status,
        ProviderCredentialOperationStatus::CleanupRequired
    );
    assert!(completed.operation_slot_recovery_required);
    reopened
        .ensure_provider_credential_access_settled(&ProviderConnectionId::from("started-install"))
        .expect_err("bare Started visibility never grants provider use");
    let replayed = reopened
        .reconcile_provider_credential_operation(
            &prepared.plan.operation_id,
            &prepared.plan_sha256,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("visibility cannot clear startup fence");
    assert_eq!(replayed, completed);
}

#[test]
fn replacement_durability_barriers_survive_together_and_repair_independently() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    insert_credential_connection(&storage, "dual-durability-barrier");
    install_owned_credential(&storage, "dual-durability-barrier");
    let replacement = prepare_authority_bound_install(&storage, "dual-durability-barrier");
    storage
        .start_provider_credential_operation(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
        )
        .expect("start replacement");
    storage
        .attest_provider_credential_predecessor_delete_intent(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("attest predecessor delete intent");
    storage
        .mark_provider_credential_predecessor_durability_recovery_required(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
            false,
        )
        .expect("mark predecessor barrier");
    let both = storage
        .mark_provider_credential_durability_recovery_required(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
            false,
        )
        .expect("mark operation barrier without overwriting predecessor");
    assert!(both.operation_slot_recovery_required);
    assert!(both.predecessor_slot_recovery_required);
    drop(storage);

    let reopened = Storage::open(root.path()).expect("reopen dual barrier");
    let one_left = reopened
        .attest_provider_credential_predecessor_durability_repaired(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
        )
        .expect("repair predecessor only");
    assert!(one_left.operation_slot_recovery_required);
    assert!(!one_left.predecessor_slot_recovery_required);
    assert_eq!(
        one_left.status,
        ProviderCredentialOperationStatus::CleanupRequired
    );
    let repaired = reopened
        .attest_provider_credential_durability_repaired(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
        )
        .expect("repair operation slot second");
    assert!(!repaired.operation_slot_recovery_required);
    assert!(!repaired.predecessor_slot_recovery_required);
}

#[test]
fn started_removal_and_predecessor_phase_are_fenced_before_visibility() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    insert_credential_connection(&storage, "started-removal-fence");
    install_owned_credential(&storage, "started-removal-fence");
    let removal = storage
        .prepare_provider_credential_operation(
            &ProviderConnectionId::from("started-removal-fence"),
            ProviderCredentialOperationKind::RemoveCredential,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("prepare removal");
    storage
        .start_provider_credential_operation(&removal.plan.operation_id, &removal.plan_sha256)
        .expect("start removal");
    let fenced_removal = storage
        .fence_started_provider_credential_operation_for_recovery(
            &removal.plan.operation_id,
            &removal.plan_sha256,
        )
        .expect("fence removal before Missing visibility");
    assert!(fenced_removal.operation_slot_recovery_required);
    assert!(!fenced_removal.predecessor_slot_recovery_required);

    insert_credential_connection(&storage, "started-predecessor-fence");
    install_owned_credential(&storage, "started-predecessor-fence");
    let replacement = prepare_authority_bound_install(&storage, "started-predecessor-fence");
    storage
        .start_provider_credential_operation(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
        )
        .expect("start replacement");
    storage
        .attest_provider_credential_predecessor_delete_intent(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("record predecessor delete intent");
    let fenced_predecessor = storage
        .fence_started_provider_credential_operation_for_recovery(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
        )
        .expect("fence predecessor phase before Missing visibility");
    assert!(!fenced_predecessor.operation_slot_recovery_required);
    assert!(fenced_predecessor.predecessor_slot_recovery_required);
}

#[test]
fn restored_started_replacement_with_available_b_never_adopts_without_predecessor_proof() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    let connection_id = ProviderConnectionId::from("replacement-rollback-gap");
    insert_credential_connection(&storage, connection_id.as_str());
    let authority_a = install_owned_credential(&storage, connection_id.as_str());
    let replacement_b = prepare_authority_bound_install(&storage, connection_id.as_str());
    storage
        .start_provider_credential_operation(
            &replacement_b.plan.operation_id,
            &replacement_b.plan_sha256,
        )
        .expect("start replacement B before the database snapshot");
    drop(storage);

    let reopened = Storage::open(root.path()).expect("reopen restored Started snapshot");
    let first = reopened
        .reconcile_provider_credential_operation(
            &replacement_b.plan.operation_id,
            &replacement_b.plan_sha256,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("available B without restored predecessor proof settles fail closed");
    assert_eq!(
        first.status,
        ProviderCredentialOperationStatus::OutcomeUnknown
    );
    assert_eq!(
        first.outcome_code,
        Some(ProviderCredentialOutcomeCode::ConnectionChanged)
    );
    assert_eq!(
        first.plan.predecessor_authority_id,
        Some(authority_a.authority_id)
    );
    reopened
        .ensure_provider_credential_access_settled(&connection_id)
        .expect_err("neither restored predecessor A nor newer B is usable while unresolved");

    drop(reopened);
    let reopened_again = Storage::open(root.path()).expect("reopen fail-closed settlement");
    let replayed = reopened_again
        .reconcile_provider_credential_operation(
            &replacement_b.plan.operation_id,
            &replacement_b.plan_sha256,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("a later bootstrap is idempotent and does not adopt B");
    assert_eq!(replayed, first);
    reopened_again
        .ensure_provider_credential_access_settled(&connection_id)
        .expect_err("durable outcome_unknown remains a provider-access barrier");
}

#[test]
fn ordinary_install_succeeds_and_replacement_requires_predecessor_missing_evidence() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    insert_credential_connection(&storage, "replacement-evidence");

    let authority_a = install_owned_credential(&storage, "replacement-evidence");
    let replacement = prepare_authority_bound_install(&storage, "replacement-evidence");
    assert_eq!(
        replacement.plan.predecessor_authority_id.as_deref(),
        Some(authority_a.authority_id.as_str())
    );
    assert_eq!(
        replacement
            .plan
            .predecessor_authority_binding_sha256
            .as_deref(),
        Some(authority_a.connection_binding_sha256.as_str())
    );
    storage
        .start_provider_credential_operation(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
        )
        .expect("start replacement B");

    let unproved = storage
        .finish_provider_credential_operation(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("replacement B without predecessor proof settles fail closed");
    assert_eq!(
        unproved.status,
        ProviderCredentialOperationStatus::OutcomeUnknown
    );
    assert_eq!(
        unproved.outcome_code,
        Some(ProviderCredentialOutcomeCode::ConnectionChanged)
    );

    storage
        .attest_provider_credential_predecessor_delete_intent(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("durably record predecessor delete intent");
    storage
        .attest_provider_credential_predecessor_missing(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
        )
        .expect("durably attest predecessor A missing");
    let completed = storage
        .finish_provider_credential_operation(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("replacement B succeeds after predecessor proof");
    assert_eq!(
        completed.status,
        ProviderCredentialOperationStatus::Succeeded
    );
    drop(storage);

    let reopened = Storage::open(root.path()).expect("reopen completed replacement");
    let authority_b = reopened
        .ensure_provider_credential_access_settled(&ProviderConnectionId::from(
            "replacement-evidence",
        ))
        .expect("replacement B owns access after reopen");
    assert_eq!(authority_b.authority_id, replacement.plan.operation_id);
    assert_ne!(authority_b.authority_id, authority_a.authority_id);
}

#[test]
fn missing_started_replacement_cleanup_removes_old_authority_after_predecessor_proof() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    insert_credential_connection(&storage, "replacement-cleanup-remove");
    let authority_a = install_owned_credential(&storage, "replacement-cleanup-remove");
    let replacement = prepare_authority_bound_install(&storage, "replacement-cleanup-remove");
    storage
        .start_provider_credential_operation(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
        )
        .expect("start replacement B");
    let uncertain = storage
        .reconcile_provider_credential_operation(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("recover replacement whose B slot is missing");
    assert_eq!(
        uncertain.status,
        ProviderCredentialOperationStatus::OutcomeUnknown
    );
    storage
        .ensure_provider_credential_access_settled(&ProviderConnectionId::from(
            "replacement-cleanup-remove",
        ))
        .expect_err("unresolved replacement blocks predecessor access");

    storage
        .mark_provider_credential_cleanup_required(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
            false,
        )
        .expect("persist remove-only cleanup intent");
    storage
        .attest_provider_credential_predecessor_delete_intent(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("persist predecessor A delete intent");
    storage
        .attest_provider_credential_predecessor_missing(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
        )
        .expect("attest predecessor A missing");
    let completed = storage
        .reconcile_provider_credential_operation(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("close missing replacement cleanup");
    assert_eq!(
        completed.status,
        ProviderCredentialOperationStatus::NoEffect
    );
    assert_eq!(
        completed.outcome_code,
        Some(ProviderCredentialOutcomeCode::ConnectionChanged)
    );
    assert_eq!(
        provider_credential_ownership_state(root.path(), "replacement-cleanup-remove"),
        "removed"
    );
    drop(storage);

    let reopened = Storage::open(root.path()).expect("reopen removed replacement");
    reopened
        .ensure_provider_credential_access_settled(&ProviderConnectionId::from(
            "replacement-cleanup-remove",
        ))
        .expect_err("old authority A cannot return after replacement cleanup");
    assert_ne!(authority_a.authority_id, replacement.plan.operation_id);
}

#[test]
fn replacement_archive_cleanup_waits_for_predecessor_missing_before_atomic_archive() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    insert_credential_connection(&storage, "replacement-cleanup-archive");
    install_owned_credential(&storage, "replacement-cleanup-archive");
    let replacement = prepare_authority_bound_install(&storage, "replacement-cleanup-archive");
    storage
        .start_provider_credential_operation(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
        )
        .expect("start replacement B");
    storage
        .reconcile_provider_credential_operation(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("recover missing B as uncertain");
    storage
        .mark_provider_credential_cleanup_required(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
            true,
        )
        .expect("persist archive cleanup intent");

    storage
        .reconcile_provider_credential_archive(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect_err("archive cannot commit while predecessor A may still exist");
    storage
        .get_provider_connection(&ProviderConnectionId::from("replacement-cleanup-archive"))
        .expect("failed archive proof keeps the connection active");

    storage
        .attest_provider_credential_predecessor_delete_intent(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("persist predecessor delete intent");
    storage
        .attest_provider_credential_predecessor_missing(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
        )
        .expect("attest predecessor missing");
    let completed = storage
        .reconcile_provider_credential_archive(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("atomically archive after predecessor proof");
    assert_eq!(
        completed.status,
        ProviderCredentialOperationStatus::NoEffect
    );
    storage
        .get_provider_connection(&ProviderConnectionId::from("replacement-cleanup-archive"))
        .expect_err("connection archives in the same terminal commit");
}

#[test]
fn unstarted_replacement_cleanup_requires_predecessor_proof_before_remove_or_archive() {
    for archives_connection in [false, true] {
        exercise_unstarted_replacement_cleanup(archives_connection);
    }
}

fn exercise_unstarted_replacement_cleanup(archives_connection: bool) {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    let suffix = if archives_connection {
        "archive"
    } else {
        "remove"
    };
    let connection_id = format!("unstarted-replacement-cleanup-{suffix}");
    insert_credential_connection(&storage, &connection_id);
    let authority_a = install_owned_credential(&storage, &connection_id);
    let replacement = prepare_authority_bound_install(&storage, &connection_id);
    let uncertain = storage
        .reconcile_provider_credential_operation(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("classify an available pre-start replacement as unknown");
    assert_eq!(
        uncertain.status,
        ProviderCredentialOperationStatus::OutcomeUnknown
    );
    assert!(uncertain.started_at.is_none());
    storage
        .mark_provider_credential_cleanup_required(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
            archives_connection,
        )
        .expect("persist explicit replacement cleanup intent");
    if !archives_connection {
        assert_sql_rejects_unproven_replacement_cleanup(
            root.path(),
            &connection_id,
            &replacement.plan.operation_id,
        );
    }
    let case = ReplacementCleanupCase {
        root: root.path().to_path_buf(),
        connection_id,
        predecessor_authority_id: authority_a.authority_id,
        operation_id: replacement.plan.operation_id,
        plan_sha256: replacement.plan_sha256,
        archives_connection,
    };
    assert_unproven_replacement_cleanup_is_conservative(&storage, &case);
    drop(storage);
    complete_replacement_cleanup_after_reopen(&case);
}

struct ReplacementCleanupCase {
    root: PathBuf,
    connection_id: String,
    predecessor_authority_id: String,
    operation_id: String,
    plan_sha256: String,
    archives_connection: bool,
}

fn assert_unproven_replacement_cleanup_is_conservative(
    storage: &Storage,
    case: &ReplacementCleanupCase,
) {
    reconcile_replacement_cleanup(storage, case)
        .expect_err("replacement cleanup cannot terminalize without predecessor-missing proof");
    let unresolved = storage
        .get_provider_credential_operation(&case.operation_id)
        .expect("load unresolved replacement cleanup");
    assert_eq!(
        unresolved.status,
        ProviderCredentialOperationStatus::CleanupRequired
    );
    assert!(unresolved.started_at.is_none());
    let database = Connection::open(active_database_path(&case.root))
        .expect("open replacement cleanup proof database");
    let ownership = database
        .query_row(
            "SELECT ownership_state, authority_id
             FROM provider_credential_ownership
             WHERE connection_id = ?1",
            [case.connection_id.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .expect("load predecessor ownership projection");
    assert_eq!(
        ownership,
        (
            "ordinary_owned".to_owned(),
            case.predecessor_authority_id.clone()
        )
    );
    let queued_gc = database
        .query_row(
            "SELECT COUNT(*) FROM provider_credential_slot_gc WHERE connection_id = ?1",
            [case.connection_id.as_str()],
            |row| row.get::<_, u64>(0),
        )
        .expect("count replacement cleanup garbage collection");
    assert_eq!(queued_gc, 0);
}

fn complete_replacement_cleanup_after_reopen(case: &ReplacementCleanupCase) {
    let reopened = Storage::open(&case.root).expect("reopen unresolved replacement cleanup");
    reopened
        .get_provider_connection(&ProviderConnectionId::from(case.connection_id.as_str()))
        .expect("unproven archive cleanup keeps the connection active");
    reconcile_replacement_cleanup(&reopened, case)
        .expect_err("reopen must not invent predecessor-missing evidence");
    reopened
        .attest_provider_credential_predecessor_delete_intent(
            &case.operation_id,
            &case.plan_sha256,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("persist exact predecessor delete intent");
    reopened
        .attest_provider_credential_predecessor_missing(&case.operation_id, &case.plan_sha256)
        .expect("persist exact predecessor missing evidence");
    let completed = reconcile_replacement_cleanup(&reopened, case)
        .expect("complete replacement cleanup after predecessor proof");
    assert_eq!(
        completed.status,
        ProviderCredentialOperationStatus::NoEffect
    );
    if case.archives_connection {
        reopened
            .get_provider_connection(&ProviderConnectionId::from(case.connection_id.as_str()))
            .expect_err("proved archive cleanup archives the connection atomically");
    } else {
        assert_eq!(
            provider_credential_ownership_state(&case.root, &case.connection_id),
            "removed"
        );
    }
}

fn reconcile_replacement_cleanup(
    storage: &Storage,
    case: &ReplacementCleanupCase,
) -> lorepia_domain::CoreResult<lorepia_storage::StoredProviderCredentialOperation> {
    if case.archives_connection {
        storage.reconcile_provider_credential_archive(
            &case.operation_id,
            &case.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
        )
    } else {
        storage.reconcile_provider_credential_operation(
            &case.operation_id,
            &case.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
        )
    }
}

#[test]
fn terminal_replacement_cleanup_fails_closed_if_predecessor_evidence_is_removed() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    let connection_id = "tampered-replacement-cleanup";
    insert_credential_connection(&storage, connection_id);
    install_owned_credential(&storage, connection_id);
    let replacement = prepare_authority_bound_install(&storage, connection_id);
    storage
        .reconcile_provider_credential_operation(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("classify pre-start replacement as unknown");
    storage
        .mark_provider_credential_cleanup_required(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
            false,
        )
        .expect("persist replacement cleanup intent");
    storage
        .attest_provider_credential_predecessor_delete_intent(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("persist predecessor delete intent");
    storage
        .attest_provider_credential_predecessor_missing(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
        )
        .expect("persist predecessor missing evidence");
    storage
        .reconcile_provider_credential_operation(
            &replacement.plan.operation_id,
            &replacement.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("terminalize exact proved replacement cleanup");

    let database = Connection::open(active_database_path(root.path()))
        .expect("open replacement cleanup tamper database");
    let intact_error = database
        .execute(
            "DELETE FROM provider_credential_operation_attestations
             WHERE operation_id = ?1 AND stage = 'predecessor_missing'",
            [replacement.plan.operation_id.as_str()],
        )
        .expect_err("append-only evidence guard blocks predecessor proof deletion");
    assert!(
        intact_error
            .to_string()
            .contains("credential operation attestations are append-only")
    );
    let guard = suspend_trigger(
        &database,
        "provider_credential_operation_attestation_no_delete",
    );
    assert_eq!(
        database
            .execute(
                "DELETE FROM provider_credential_operation_attestations
                 WHERE operation_id = ?1 AND stage = 'predecessor_missing'",
                [replacement.plan.operation_id.as_str()],
            )
            .expect("inject synthetic predecessor-proof corruption"),
        1
    );
    restore_trigger(&database, &guard);
    drop(database);

    let error = storage
        .get_provider_credential_operation(&replacement.plan.operation_id)
        .expect_err("terminal replacement cleanup must revalidate predecessor history");
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
}

#[test]
fn persistent_unreadable_recovery_is_an_idempotent_unresolved_barrier() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    insert_credential_connection(&storage, "persistent-unreadable");
    let prepared = prepare_authority_bound_install(&storage, "persistent-unreadable");
    storage
        .start_provider_credential_operation(&prepared.plan.operation_id, &prepared.plan_sha256)
        .expect("start install");
    let uncertain = storage
        .reconcile_provider_credential_operation(
            &prepared.plan.operation_id,
            &prepared.plan_sha256,
            ProviderCredentialObservedStatus::Unreadable,
        )
        .expect("first unreadable recovery");
    assert_eq!(
        uncertain.status,
        ProviderCredentialOperationStatus::OutcomeUnknown
    );
    let replayed = storage
        .reconcile_provider_credential_operation(
            &prepared.plan.operation_id,
            &prepared.plan_sha256,
            ProviderCredentialObservedStatus::Unreadable,
        )
        .expect("same unreadable recovery is a no-op");
    assert_eq!(replayed, uncertain);
    drop(storage);

    let reopened = Storage::open(root.path()).expect("reopen unresolved operation");
    let replayed_after_reopen = reopened
        .reconcile_provider_credential_operation(
            &prepared.plan.operation_id,
            &prepared.plan_sha256,
            ProviderCredentialObservedStatus::Unreadable,
        )
        .expect("persistent unreadable remains idempotent after reopen");
    assert_eq!(replayed_after_reopen, uncertain);
    reopened
        .ensure_provider_credential_access_settled(&ProviderConnectionId::from(
            "persistent-unreadable",
        ))
        .expect_err("persistent unknown outcome remains access-blocking");
}

#[test]
fn outcome_unknown_accepts_alternating_attested_observations_without_reopening_access() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    insert_credential_connection(&storage, "alternating-unknown");
    let prepared = prepare_authority_bound_install(&storage, "alternating-unknown");
    storage
        .start_provider_credential_operation(&prepared.plan.operation_id, &prepared.plan_sha256)
        .expect("start install");

    let database =
        Connection::open(active_database_path(root.path())).expect("open binding drift injector");
    database
        .execute(
            "UPDATE provider_connections
             SET api_origin = 'https://alternating.example.test'
             WHERE id = 'alternating-unknown'",
            [],
        )
        .expect("inject binding drift");
    drop(database);

    let unreadable = storage
        .reconcile_provider_credential_operation(
            &prepared.plan.operation_id,
            &prepared.plan_sha256,
            ProviderCredentialObservedStatus::Unreadable,
        )
        .expect("record unreadable uncertainty");
    assert_eq!(
        unreadable.status,
        ProviderCredentialOperationStatus::OutcomeUnknown
    );
    assert_eq!(
        unreadable.outcome_code,
        Some(ProviderCredentialOutcomeCode::NativeStatusUnreadable)
    );

    let available = storage
        .reconcile_provider_credential_operation(
            &prepared.plan.operation_id,
            &prepared.plan_sha256,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("append changed available observation");
    assert_eq!(
        available.status,
        ProviderCredentialOperationStatus::OutcomeUnknown
    );
    assert_eq!(
        available.outcome_code,
        Some(ProviderCredentialOutcomeCode::ConnectionChanged)
    );
    assert!(
        available.outcome_attestation_sequence > unreadable.outcome_attestation_sequence,
        "a changed uncertainty observation must append evidence"
    );

    let unreadable_again = storage
        .reconcile_provider_credential_operation(
            &prepared.plan.operation_id,
            &prepared.plan_sha256,
            ProviderCredentialObservedStatus::Unreadable,
        )
        .expect("append later unreadable observation");
    assert_eq!(
        unreadable_again.status,
        ProviderCredentialOperationStatus::OutcomeUnknown
    );
    assert_eq!(
        unreadable_again.outcome_code,
        Some(ProviderCredentialOutcomeCode::NativeStatusUnreadable)
    );
    storage
        .ensure_provider_credential_access_settled(&ProviderConnectionId::from(
            "alternating-unknown",
        ))
        .expect_err("alternating uncertainty must remain access-blocking");
}

#[test]
fn binding_drift_is_attested_uncertain_until_original_slot_is_missing() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    insert_credential_connection(&storage, "binding-drift");
    let prepared = prepare_authority_bound_install(&storage, "binding-drift");
    storage
        .start_provider_credential_operation(&prepared.plan.operation_id, &prepared.plan_sha256)
        .expect("start install");

    let database =
        Connection::open(active_database_path(root.path())).expect("open binding drift injector");
    database
        .execute(
            "UPDATE provider_connections
             SET api_origin = 'https://changed.example.test'
             WHERE id = 'binding-drift'",
            [],
        )
        .expect("inject a post-start binding change");
    drop(database);

    let uncertain = storage
        .reconcile_provider_credential_operation(
            &prepared.plan.operation_id,
            &prepared.plan_sha256,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("attest available original slot without adopting it");
    assert_eq!(
        uncertain.status,
        ProviderCredentialOperationStatus::OutcomeUnknown
    );
    storage
        .ensure_provider_credential_access_settled(&ProviderConnectionId::from("binding-drift"))
        .expect_err("binding drift remains use-blocking");

    let cleaned = storage
        .reconcile_provider_credential_operation(
            &prepared.plan.operation_id,
            &prepared.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("missing original slot safely closes the reserved plan");
    assert_eq!(cleaned.status, ProviderCredentialOperationStatus::NoEffect);
}

#[test]
fn prepared_owned_removal_missing_recovery_truthfully_removes_authority() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    insert_credential_connection(&storage, "prepared-owned-remove");
    install_owned_credential(&storage, "prepared-owned-remove");

    let removal = storage
        .prepare_provider_credential_operation(
            &ProviderConnectionId::from("prepared-owned-remove"),
            ProviderCredentialOperationKind::RemoveCredential,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("prepare owned removal");
    let completed = storage
        .reconcile_provider_credential_operation(
            &removal.plan.operation_id,
            &removal.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("recover externally missing owned slot");
    assert_eq!(
        completed.status,
        ProviderCredentialOperationStatus::NoEffect
    );
    assert_eq!(
        completed.outcome_code,
        Some(ProviderCredentialOutcomeCode::ConnectionChanged)
    );
    assert_eq!(
        provider_credential_ownership_state(root.path(), "prepared-owned-remove"),
        "removed"
    );
    storage
        .get_provider_connection(&ProviderConnectionId::from("prepared-owned-remove"))
        .expect("ordinary credential removal keeps the connection active");
    drop(storage);

    let reopened = Storage::open(root.path()).expect("reopen removed credential state");
    reopened
        .ensure_provider_credential_access_settled(&ProviderConnectionId::from(
            "prepared-owned-remove",
        ))
        .expect_err("reopen cannot resurrect the old authority");
}

#[test]
fn provider_credential_slot_garbage_rejects_missing_ordinary_ownership_source() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    let connection_id = "gc-missing-ordinary-source";
    insert_credential_connection(&storage, connection_id);
    let installed = install_owned_credential(&storage, connection_id);

    let removal = storage
        .prepare_provider_credential_operation(
            &ProviderConnectionId::from(connection_id),
            ProviderCredentialOperationKind::RemoveCredential,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("prepare credential removal");
    storage
        .start_provider_credential_operation(&removal.plan.operation_id, &removal.plan_sha256)
        .expect("start credential removal");
    storage
        .finish_provider_credential_operation(
            &removal.plan.operation_id,
            &removal.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("finish credential removal");

    let garbage = storage
        .list_provider_credential_slot_garbage()
        .expect("intact superseded ownership history is valid");
    assert_eq!(garbage.len(), 1);
    assert_eq!(garbage[0].authority.authority_id, installed.authority_id);

    let database = Connection::open(active_database_path(root.path()))
        .expect("open missing ownership source corruption fixture");
    let attestation_guard = suspend_trigger(
        &database,
        "provider_credential_operation_attestation_no_delete",
    );
    let operation_guard = suspend_trigger(&database, "provider_credential_operation_no_delete");
    database
        .execute(
            "DELETE FROM provider_credential_operation_attestations
             WHERE operation_id = ?1",
            [installed.authority_id.as_str()],
        )
        .expect("remove only the superseded install evidence in corruption fixture");
    database
        .execute(
            "DELETE FROM provider_credential_operations WHERE id = ?1",
            [installed.authority_id.as_str()],
        )
        .expect("remove only the superseded install source in corruption fixture");
    restore_trigger(&database, &operation_guard);
    restore_trigger(&database, &attestation_guard);
    let foreign_key_violations = database
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get::<_, u64>(0)
        })
        .expect("check schema-valid ownership source corruption fixture");
    assert_eq!(foreign_key_violations, 0);
    drop(database);

    let error = storage
        .list_provider_credential_slot_garbage()
        .expect_err("garbage collection must reject an ownership event with no durable source");
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
}

#[test]
fn prepared_owned_archive_removal_missing_recovery_keeps_connection_active_but_removed() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    insert_credential_connection(&storage, "prepared-owned-archive-remove");
    install_owned_credential(&storage, "prepared-owned-archive-remove");

    let removal = storage
        .prepare_provider_credential_operation(
            &ProviderConnectionId::from("prepared-owned-archive-remove"),
            ProviderCredentialOperationKind::RemoveForArchive,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("prepare owned archive removal");
    let completed = storage
        .reconcile_provider_credential_operation(
            &removal.plan.operation_id,
            &removal.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("recover no-start archive removal as remove-only cleanup");
    assert_eq!(
        completed.status,
        ProviderCredentialOperationStatus::NoEffect
    );
    assert_eq!(
        completed.outcome_code,
        Some(ProviderCredentialOutcomeCode::ConnectionChanged)
    );
    assert_eq!(
        provider_credential_ownership_state(root.path(), "prepared-owned-archive-remove"),
        "removed"
    );
    storage
        .get_provider_connection(&ProviderConnectionId::from("prepared-owned-archive-remove"))
        .expect("unstarted archive removal recovery must not archive the connection");
    drop(storage);

    let reopened = Storage::open(root.path()).expect("reopen active removed connection");
    reopened
        .get_provider_connection(&ProviderConnectionId::from("prepared-owned-archive-remove"))
        .expect("connection stays active after reopen");
    reopened
        .ensure_provider_credential_access_settled(&ProviderConnectionId::from(
            "prepared-owned-archive-remove",
        ))
        .expect_err("old credential authority stays revoked after reopen");
}

#[test]
fn remove_for_archive_terminal_and_connection_archive_are_atomic() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    insert_credential_connection(&storage, "archive-credential");
    let prepared = storage
        .prepare_provider_credential_operation(
            &ProviderConnectionId::from("archive-credential"),
            ProviderCredentialOperationKind::RemoveForArchive,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("prepare archive removal");
    storage
        .start_provider_credential_operation(&prepared.plan.operation_id, &prepared.plan_sha256)
        .expect("start archive removal");
    let completed = storage
        .reconcile_provider_credential_archive(
            &prepared.plan.operation_id,
            &prepared.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("atomically archive after missing attestation");
    assert_eq!(
        completed.status,
        ProviderCredentialOperationStatus::Succeeded
    );
    storage
        .get_provider_connection(&ProviderConnectionId::from("archive-credential"))
        .expect_err("connection must be archived in the same commit");

    drop(storage);
    let reopened = Storage::open(root.path()).expect("reopen archived connection");
    let completed = reopened
        .get_provider_credential_operation(&prepared.plan.operation_id)
        .expect("terminal journal survives reopen");
    assert_eq!(
        completed.status,
        ProviderCredentialOperationStatus::Succeeded
    );
}

#[test]
fn started_archive_cleanup_remove_records_commit_failure_and_retry_converges() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    insert_credential_connection(&storage, "archive-cleanup-remove-retry");
    install_owned_credential(&storage, "archive-cleanup-remove-retry");
    let removal = storage
        .prepare_provider_credential_operation(
            &ProviderConnectionId::from("archive-cleanup-remove-retry"),
            ProviderCredentialOperationKind::RemoveForArchive,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("prepare archive removal");
    storage
        .start_provider_credential_operation(&removal.plan.operation_id, &removal.plan_sha256)
        .expect("start archive removal");
    let cleanup = storage
        .mark_provider_credential_cleanup_required(
            &removal.plan.operation_id,
            &removal.plan_sha256,
            ProviderCredentialObservedStatus::Available,
            false,
        )
        .expect("persist remove-only cleanup intent");
    assert!(!cleanup.cleanup_archives_connection);

    let database_path = active_database_path(root.path());
    let database = Connection::open(&database_path).expect("open archive failure injector");
    database
        .execute_batch(
            "CREATE TRIGGER synthetic_started_cleanup_remove_archive_abort
             BEFORE UPDATE OF archived_at ON provider_connections
             WHEN NEW.id = 'archive-cleanup-remove-retry'
             BEGIN
                 SELECT RAISE(ABORT, 'synthetic started cleanup remove archive failure');
             END;",
        )
        .expect("install archive abort trigger");
    drop(database);

    storage
        .reconcile_provider_credential_archive(
            &removal.plan.operation_id,
            &removal.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect_err("archive database failure must remain retryable");
    let pending = storage
        .get_provider_credential_operation(&removal.plan.operation_id)
        .expect("read durable archive failure");
    assert_eq!(
        pending.status,
        ProviderCredentialOperationStatus::CleanupRequired
    );
    assert_eq!(
        pending.outcome_code,
        Some(ProviderCredentialOutcomeCode::ArchiveCommitFailed)
    );
    assert!(!pending.cleanup_archives_connection);
    storage
        .get_provider_connection(&ProviderConnectionId::from("archive-cleanup-remove-retry"))
        .expect("failed archive keeps connection active");
    drop(storage);

    let database = Connection::open(database_path).expect("remove archive failure injector");
    database
        .execute_batch("DROP TRIGGER synthetic_started_cleanup_remove_archive_abort;")
        .expect("drop archive abort trigger");
    drop(database);
    let reopened = Storage::open(root.path()).expect("reopen retryable archive cleanup");
    let completed = reopened
        .reconcile_provider_credential_archive(
            &removal.plan.operation_id,
            &removal.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("retry atomically archives original RFA");
    assert_eq!(
        completed.status,
        ProviderCredentialOperationStatus::Succeeded
    );
    reopened
        .get_provider_connection(&ProviderConnectionId::from("archive-cleanup-remove-retry"))
        .expect_err("successful retry archives the connection");
}

#[test]
fn missing_archive_preflight_is_atomic_no_effect_and_reopen_idempotent() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    insert_credential_connection(&storage, "archive-missing");
    let prepared = storage
        .prepare_provider_credential_operation(
            &ProviderConnectionId::from("archive-missing"),
            ProviderCredentialOperationKind::RemoveForArchive,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("prepare already-missing archive");
    let completed = storage
        .finish_provider_credential_archive(
            &prepared.plan.operation_id,
            &prepared.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("atomically archive without claiming a native delete");
    assert_eq!(
        completed.status,
        ProviderCredentialOperationStatus::NoEffect
    );
    storage
        .get_provider_connection(&ProviderConnectionId::from("archive-missing"))
        .expect_err("connection is archived in the no-effect terminal commit");
    drop(storage);

    let reopened = Storage::open(root.path()).expect("reopen missing-slot archive");
    let replayed = reopened
        .reconcile_provider_credential_archive(
            &prepared.plan.operation_id,
            &prepared.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("missing-slot archive replay is idempotent");
    assert_eq!(replayed, completed);
}

#[test]
fn missing_archive_commit_failure_is_durable_and_retries_as_no_effect() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    insert_credential_connection(&storage, "archive-missing-retry");
    let prepared = storage
        .prepare_provider_credential_operation(
            &ProviderConnectionId::from("archive-missing-retry"),
            ProviderCredentialOperationKind::RemoveForArchive,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("prepare missing-slot archive");
    let database_path = active_database_path(root.path());
    let database = Connection::open(&database_path).expect("open archive failure injector");
    database
        .execute_batch(
            "CREATE TRIGGER synthetic_missing_archive_abort
             BEFORE UPDATE OF archived_at ON provider_connections
             WHEN NEW.id = 'archive-missing-retry'
             BEGIN
                 SELECT RAISE(ABORT, 'synthetic missing archive failure');
             END;",
        )
        .expect("install missing archive abort trigger");
    drop(database);

    storage
        .finish_provider_credential_archive(
            &prepared.plan.operation_id,
            &prepared.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect_err("failed archive commit must remain durably retryable");
    let pending = storage
        .get_provider_credential_operation(&prepared.plan.operation_id)
        .expect("read missing archive failure");
    assert_eq!(
        pending.status,
        ProviderCredentialOperationStatus::CleanupRequired
    );
    assert_eq!(
        pending.outcome_code,
        Some(ProviderCredentialOutcomeCode::ArchiveCommitFailed)
    );
    drop(storage);

    let database = Connection::open(database_path).expect("remove archive failure injector");
    database
        .execute_batch("DROP TRIGGER synthetic_missing_archive_abort;")
        .expect("drop missing archive abort trigger");
    drop(database);
    let reopened = Storage::open(root.path()).expect("reopen retryable missing archive");
    let completed = reopened
        .reconcile_provider_credential_archive(
            &prepared.plan.operation_id,
            &prepared.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("retry missing archive as truthful native no-effect");
    assert_eq!(
        completed.status,
        ProviderCredentialOperationStatus::NoEffect
    );
    reopened
        .get_provider_connection(&ProviderConnectionId::from("archive-missing-retry"))
        .expect_err("retry atomically archives connection");
}

#[test]
fn missing_cleanup_archive_commit_failure_preserves_disposition_and_retries() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    insert_credential_connection(&storage, "cleanup-archive-missing-retry");
    let prepared = storage
        .prepare_provider_credential_operation(
            &ProviderConnectionId::from("cleanup-archive-missing-retry"),
            ProviderCredentialOperationKind::RemoveCredential,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("prepare ordinary removal");
    storage
        .finish_provider_credential_operation(
            &prepared.plan.operation_id,
            &prepared.plan_sha256,
            ProviderCredentialObservedStatus::Unreadable,
        )
        .expect("record uncertain pre-effect removal");
    let cleanup = storage
        .mark_provider_credential_cleanup_required(
            &prepared.plan.operation_id,
            &prepared.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
            true,
        )
        .expect("persist missing archive cleanup disposition");
    assert!(cleanup.cleanup_archives_connection);
    assert!(cleanup.started_at.is_none());

    let database_path = active_database_path(root.path());
    let database = Connection::open(&database_path).expect("open archive failure injector");
    database
        .execute_batch(
            "CREATE TRIGGER synthetic_cleanup_archive_abort
             BEFORE UPDATE OF archived_at ON provider_connections
             WHEN NEW.id = 'cleanup-archive-missing-retry'
             BEGIN
                 SELECT RAISE(ABORT, 'synthetic cleanup archive failure');
             END;",
        )
        .expect("install cleanup archive abort trigger");
    drop(database);

    storage
        .reconcile_provider_credential_archive(
            &prepared.plan.operation_id,
            &prepared.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect_err("failed cleanup archive commit remains retryable");
    let pending = storage
        .get_provider_credential_operation(&prepared.plan.operation_id)
        .expect("read cleanup archive failure");
    assert_eq!(
        pending.status,
        ProviderCredentialOperationStatus::CleanupRequired
    );
    assert_eq!(
        pending.outcome_code,
        Some(ProviderCredentialOutcomeCode::ArchiveCommitFailed)
    );
    assert!(pending.cleanup_archives_connection);
    drop(storage);

    let database = Connection::open(database_path).expect("remove archive failure injector");
    database
        .execute_batch("DROP TRIGGER synthetic_cleanup_archive_abort;")
        .expect("drop cleanup archive abort trigger");
    drop(database);
    let reopened = Storage::open(root.path()).expect("reopen cleanup archive retry");
    let completed = reopened
        .reconcile_provider_credential_archive(
            &prepared.plan.operation_id,
            &prepared.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("retry persisted cleanup archive disposition");
    assert_eq!(
        completed.status,
        ProviderCredentialOperationStatus::NoEffect
    );
    reopened
        .get_provider_connection(&ProviderConnectionId::from("cleanup-archive-missing-retry"))
        .expect_err("retry atomically archives connection");
}

#[test]
fn archive_commit_failure_is_durable_and_startup_retry_converges() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    insert_credential_connection(&storage, "archive-retry");
    let prepared = storage
        .prepare_provider_credential_operation(
            &ProviderConnectionId::from("archive-retry"),
            ProviderCredentialOperationKind::RemoveForArchive,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("prepare archive removal");
    storage
        .start_provider_credential_operation(&prepared.plan.operation_id, &prepared.plan_sha256)
        .expect("start archive removal");
    let database =
        Connection::open(active_database_path(root.path())).expect("open archive failure injector");
    database
        .execute_batch(
            "CREATE TRIGGER synthetic_archive_abort
             BEFORE UPDATE OF archived_at ON provider_connections
             WHEN NEW.id = 'archive-retry'
             BEGIN
                 SELECT RAISE(ABORT, 'synthetic archive failure');
             END;",
        )
        .expect("install archive abort trigger");
    drop(database);

    storage
        .finish_provider_credential_archive(
            &prepared.plan.operation_id,
            &prepared.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect_err("archive failure must be reported");
    let pending = storage
        .get_provider_credential_operation(&prepared.plan.operation_id)
        .expect("read durable archive failure");
    assert_eq!(
        pending.status,
        ProviderCredentialOperationStatus::CleanupRequired
    );
    drop(storage);

    let database = Connection::open(active_database_path(root.path()))
        .expect("remove archive failure injector");
    database
        .execute_batch("DROP TRIGGER synthetic_archive_abort;")
        .expect("drop archive abort trigger");
    drop(database);
    let reopened = Storage::open(root.path()).expect("reopen retryable archive");
    let completed = reopened
        .reconcile_provider_credential_archive(
            &prepared.plan.operation_id,
            &prepared.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("startup-style archive retry converges");
    assert_eq!(
        completed.status,
        ProviderCredentialOperationStatus::Succeeded
    );
}

#[test]
fn journal_is_secret_free_and_plan_identity_is_immutable() {
    const SECRET: &str = "synthetic-provider-secret-canary";
    let secret_sha256 = hex::encode(Sha256::digest(SECRET.as_bytes()));
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    insert_credential_connection(&storage, "secret-free-journal");
    let prepared = prepare_authority_bound_install(&storage, "secret-free-journal");
    drop(storage);

    let database_path = active_database_path(root.path());
    let database = Connection::open(&database_path).expect("open journal database");
    database
        .execute(
            "UPDATE provider_credential_operations SET plan_sha256 = ?2 WHERE id = ?1",
            [&prepared.plan.operation_id, &"0".repeat(64)],
        )
        .expect_err("immutable plan digest must reject direct mutation");
    drop(database);

    let bytes = std::fs::read(database_path).expect("read synthetic journal database");
    assert!(!contains_bytes(&bytes, SECRET.as_bytes()));
    assert!(!contains_bytes(&bytes, secret_sha256.as_bytes()));
}

#[test]
fn unowned_binding_cannot_be_forged_into_credential_authority() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    insert_credential_connection(&storage, "legacy-pending");
    let database =
        Connection::open(active_database_path(root.path())).expect("open ownership projection");
    database
        .execute(
            "UPDATE provider_credential_ownership
             SET ownership_state = 'ordinary_owned', connection_binding_sha256 = ?2,
                 authority_id = 'forged-install'
             WHERE connection_id = ?1",
            ["legacy-pending", &"a".repeat(64)],
        )
        .expect_err("owned projection must require an exact succeeded install");
    database
        .execute(
            "DELETE FROM provider_credential_ownership WHERE connection_id = ?1",
            ["legacy-pending"],
        )
        .expect_err("ownership cannot be detached while the connection exists");
    drop(database);

    storage
        .ensure_provider_credential_access_settled(&ProviderConnectionId::from("legacy-pending"))
        .expect_err("unowned metadata cannot adopt a native credential without install authority");
}

#[test]
fn historical_ownership_projection_cannot_reauthorize_removed_authority() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    insert_credential_connection(&storage, "historical-authority-rewind");
    let authority_a = install_owned_credential(&storage, "historical-authority-rewind");
    let removal = storage
        .prepare_provider_credential_operation(
            &ProviderConnectionId::from("historical-authority-rewind"),
            ProviderCredentialOperationKind::RemoveCredential,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("prepare removal R");
    storage
        .start_provider_credential_operation(&removal.plan.operation_id, &removal.plan_sha256)
        .expect("start removal R");
    let removed = storage
        .finish_provider_credential_operation(
            &removal.plan.operation_id,
            &removal.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("terminalize removal R");
    assert_eq!(removed.status, ProviderCredentialOperationStatus::Succeeded);

    let database_path = active_database_path(root.path());
    let database = Connection::open(&database_path).expect("open ownership rewind fixture");
    let authority_a_sequence = database
        .query_row(
            "SELECT authority_sequence
             FROM provider_credential_ownership_events
             WHERE connection_id = ?1
               AND source_kind = 'ordinary_operation'
               AND source_id = ?2",
            rusqlite::params!["historical-authority-rewind", authority_a.authority_id],
            |row| row.get::<_, u64>(0),
        )
        .expect("load historical A event sequence");
    let rewind_error = database
        .execute(
            "UPDATE provider_credential_ownership
             SET ownership_state = 'ordinary_owned',
                 connection_binding_sha256 = ?2,
                 authority_id = ?3,
                 authority_sequence = ?4
             WHERE connection_id = ?1",
            rusqlite::params![
                "historical-authority-rewind",
                authority_a.connection_binding_sha256,
                authority_a.authority_id,
                authority_a_sequence,
            ],
        )
        .expect_err("latest-event guard must reject rewind to historical A");
    assert!(
        rewind_error
            .to_string()
            .contains("provider credential ownership lacks durable authority")
    );
    assert_eq!(
        provider_credential_ownership_state(root.path(), "historical-authority-rewind"),
        "removed"
    );

    database
        .execute_batch("DROP TRIGGER provider_credential_ownership_authority_guard;")
        .expect("drop only the projection authority guard in corruption fixture");
    database
        .execute(
            "UPDATE provider_credential_ownership
             SET ownership_state = 'ordinary_owned',
                 connection_binding_sha256 = ?2,
                 authority_id = ?3,
                 authority_sequence = ?4
             WHERE connection_id = ?1",
            rusqlite::params![
                "historical-authority-rewind",
                authority_a.connection_binding_sha256,
                authority_a.authority_id,
                authority_a_sequence,
            ],
        )
        .expect("inject stale A projection after removing only its SQL guard");
    drop(database);

    let error = storage
        .ensure_provider_credential_access_settled(&ProviderConnectionId::from(
            "historical-authority-rewind",
        ))
        .expect_err("Rust authority validation rejects stale historical projection");
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
}

#[test]
fn ownership_and_event_insert_or_replace_guards_preserve_current_authority() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    insert_credential_connection(&storage, "ownership-no-replace");
    let authority = install_owned_credential(&storage, "ownership-no-replace");
    let database =
        Connection::open(active_database_path(root.path())).expect("open no-replace fixture");
    register_discovery_integrity_function_names(&database);

    let ownership_error = database
        .execute(
            "INSERT OR REPLACE INTO provider_credential_ownership
             (connection_id, credential_ref, ownership_state,
              connection_binding_sha256, authority_id, authority_sequence,
              created_at, updated_at)
             SELECT connection_id, credential_ref, 'unowned', NULL, NULL, 0,
                    created_at, updated_at
             FROM provider_credential_ownership
             WHERE connection_id = ?1",
            ["ownership-no-replace"],
        )
        .expect_err("ownership projection cannot be replaced");
    assert!(
        ownership_error
            .to_string()
            .contains("provider credential ownership cannot replace existing authority")
    );

    let event_error = database
        .execute(
            "INSERT OR REPLACE INTO provider_credential_ownership_events
             SELECT *
             FROM provider_credential_ownership_events
             WHERE connection_id = ?1 AND source_id = ?2",
            rusqlite::params!["ownership-no-replace", authority.authority_id],
        )
        .expect_err("ownership event cannot replace existing history");
    assert!(
        event_error
            .to_string()
            .contains("provider credential ownership event cannot replace history"),
        "unexpected ownership-event replacement error: {event_error}"
    );
    drop(database);

    let current = storage
        .ensure_provider_credential_access_settled(&ProviderConnectionId::from(
            "ownership-no-replace",
        ))
        .expect("rejected replacements leave current authority valid");
    assert_eq!(current, authority);
}

#[test]
fn terminal_attestation_insert_or_replace_is_rejected_and_corruption_fails_closed() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    insert_credential_connection(&storage, "attestation-no-replace");
    let authority = install_owned_credential(&storage, "attestation-no-replace");
    let database_path = active_database_path(root.path());
    let database = Connection::open(&database_path).expect("open attestation corruption fixture");
    let replace_sql = "INSERT OR REPLACE INTO provider_credential_operation_attestations
         SELECT operation_id, sequence, stage, 'missing', ?2,
                native_owner, schema_version, redaction_version, attested_at
         FROM provider_credential_operation_attestations
         WHERE operation_id = ?1
           AND sequence = (
             SELECT outcome_attestation_sequence
             FROM provider_credential_operations
             WHERE id = ?1
           )";
    let wrong_evidence = "0".repeat(64);
    let replace_error = database
        .execute(
            replace_sql,
            rusqlite::params![authority.authority_id, wrong_evidence],
        )
        .expect_err("terminal attestation cannot be replaced");
    assert!(
        replace_error
            .to_string()
            .contains("credential operation attestation cannot replace existing evidence")
    );
    storage
        .get_provider_credential_operation(&authority.authority_id)
        .expect("rejected replacement preserves exact operation evidence");
    assert_eq!(
        storage
            .ensure_provider_credential_access_settled(&ProviderConnectionId::from(
                "attestation-no-replace",
            ))
            .expect("rejected replacement preserves provider access"),
        authority
    );

    database
        .execute_batch("DROP TRIGGER provider_credential_operation_attestation_no_replace;")
        .expect("drop only the attestation no-replace guard in corruption fixture");
    database
        .execute(
            replace_sql,
            rusqlite::params![authority.authority_id, wrong_evidence],
        )
        .expect("inject wrong terminal slot and evidence after dropping only no-replace guard");
    drop(database);

    let exact_error = storage
        .get_provider_credential_operation(&authority.authority_id)
        .expect_err("exact operation load rejects replaced terminal evidence");
    assert_eq!(exact_error.code, CoreErrorCode::StorageCorrupted);
    let access_error = storage
        .ensure_provider_credential_access_settled(&ProviderConnectionId::from(
            "attestation-no-replace",
        ))
        .expect_err("settled access rejects authority backed by replaced evidence");
    assert_eq!(access_error.code, CoreErrorCode::StorageCorrupted);
}

#[test]
fn legacy_raw_access_requires_exact_pending_projection_without_unresolved_work() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    storage
        .save_provider_profile(&ProviderProfile {
            id: "legacy-raw-gate".to_owned(),
            display_name: "Legacy raw gate".to_owned(),
            base_url: "https://api.example.test/v1".to_owned(),
            model: "synthetic-model".to_owned(),
            timeout_seconds: 30,
        })
        .expect("seed dual-written legacy profile");
    storage
        .ensure_legacy_profile_raw_credential_access("legacy-raw-gate")
        .expect_err("post-schema-37 unowned binding is not legacy raw authority");

    let database = Connection::open(active_database_path(root.path()))
        .expect("open synthetic migration projection injector");
    database
        .execute_batch("DROP TRIGGER provider_credential_ownership_authority_guard;")
        .expect("drop projection guard only inside synthetic fixture");
    database
        .execute(
            "UPDATE provider_credential_ownership
             SET ownership_state = 'legacy_pending',
                 connection_binding_sha256 = NULL,
                 authority_id = 'schema-36-cutover'
             WHERE connection_id = 'legacy-raw-gate'",
            [],
        )
        .expect("project exact schema-36 legacy pending state");
    drop(database);
    storage
        .ensure_legacy_profile_raw_credential_access("legacy-raw-gate")
        .expect("exact migrated legacy pending binding permits isolated raw access");
    assert!(
        storage
            .provider_connection_uses_legacy_raw_credential(&ProviderConnectionId::from(
                "legacy-raw-gate",
            ))
            .expect("classify the exact dual-written legacy target")
    );

    let prepared = prepare_authority_bound_install(&storage, "legacy-raw-gate");
    storage
        .ensure_legacy_profile_raw_credential_access("legacy-raw-gate")
        .expect_err("unresolved install blocks legacy raw access");
    storage
        .start_provider_credential_operation(&prepared.plan.operation_id, &prepared.plan_sha256)
        .expect("start re-entry install");
    storage
        .finish_provider_credential_operation(
            &prepared.plan.operation_id,
            &prepared.plan_sha256,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("finish bound re-entry install");
    storage
        .ensure_legacy_profile_raw_credential_access("legacy-raw-gate")
        .expect_err("durably owned binding can no longer use the raw legacy path");
    assert!(
        !storage
            .provider_connection_uses_legacy_raw_credential(&ProviderConnectionId::from(
                "legacy-raw-gate",
            ))
            .expect("bound re-entry no longer aliases the legacy raw surface")
    );
}

#[test]
fn missing_archive_cannot_start_or_terminalize_outside_atomic_archive() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    insert_credential_connection(&storage, "missing-archive-boundary");
    let prepared = storage
        .prepare_provider_credential_operation(
            &ProviderConnectionId::from("missing-archive-boundary"),
            ProviderCredentialOperationKind::RemoveForArchive,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("prepare missing archive");

    storage
        .start_provider_credential_operation(&prepared.plan.operation_id, &prepared.plan_sha256)
        .expect_err("a missing removal has no native effect to start");
    storage
        .finish_provider_credential_operation(
            &prepared.plan.operation_id,
            &prepared.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect_err("generic finish must not detach archive terminal from row archive");
    storage
        .get_provider_connection(&ProviderConnectionId::from("missing-archive-boundary"))
        .expect("rejected generic finish keeps connection active");

    let completed = storage
        .finish_provider_credential_archive(
            &prepared.plan.operation_id,
            &prepared.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("atomic archive closes missing removal");
    assert_eq!(
        completed.status,
        ProviderCredentialOperationStatus::NoEffect
    );
    storage
        .get_provider_connection(&ProviderConnectionId::from("missing-archive-boundary"))
        .expect_err("atomic terminal archives the connection");
}

#[test]
fn outcome_code_rewrite_requires_matching_new_attestation_and_fails_rust_validation() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    insert_credential_connection(&storage, "outcome-code-tamper");
    let prepared = prepare_authority_bound_install(&storage, "outcome-code-tamper");
    storage
        .start_provider_credential_operation(&prepared.plan.operation_id, &prepared.plan_sha256)
        .expect("start install");
    storage
        .finish_provider_credential_operation(
            &prepared.plan.operation_id,
            &prepared.plan_sha256,
            ProviderCredentialObservedStatus::Unreadable,
        )
        .expect("record unreadable uncertainty");

    let database = Connection::open(active_database_path(root.path())).expect("open journal");
    database
        .execute(
            "UPDATE provider_credential_operations
             SET outcome_code = 'connection_changed'
             WHERE id = ?1",
            [&prepared.plan.operation_id],
        )
        .expect_err("outcome code cannot change without matching new attestation");
    database
        .execute_batch("DROP TRIGGER provider_credential_operation_outcome_attestation_guard;")
        .expect("remove SQL guard only inside corruption fixture");
    database
        .execute(
            "UPDATE provider_credential_operations
             SET outcome_code = 'connection_changed'
             WHERE id = ?1",
            [&prepared.plan.operation_id],
        )
        .expect("inject mismatched outcome after removing only attestation trigger");
    drop(database);

    storage
        .get_provider_credential_operation(&prepared.plan.operation_id)
        .expect_err("Rust validation rejects outcome code detached from attested observation");
}

#[test]
fn forged_historical_archive_cleanup_intent_fails_closed_without_archiving() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    insert_credential_connection(&storage, "forged-archive-intent");
    let prepared = storage
        .prepare_provider_credential_operation(
            &ProviderConnectionId::from("forged-archive-intent"),
            ProviderCredentialOperationKind::RemoveCredential,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("prepare ordinary removal");
    storage
        .finish_provider_credential_operation(
            &prepared.plan.operation_id,
            &prepared.plan_sha256,
            ProviderCredentialObservedStatus::Unreadable,
        )
        .expect("record uncertain removal");
    storage
        .mark_provider_credential_cleanup_required(
            &prepared.plan.operation_id,
            &prepared.plan_sha256,
            ProviderCredentialObservedStatus::Available,
            false,
        )
        .expect("persist valid remove-only cleanup intent");

    let database = Connection::open(active_database_path(root.path())).expect("open journal");
    database
        .execute(
            "INSERT INTO provider_credential_operation_attestations
             (operation_id, sequence, stage, slot_status, evidence_sha256,
              native_owner, schema_version, redaction_version, attested_at)
             SELECT ?1, COALESCE(MAX(sequence), 0) + 1,
                    'cleanup_archive_intent', 'available', ?2,
                    'native_platform', 1, 1, ?3
             FROM provider_credential_operation_attestations
             WHERE operation_id = ?1",
            rusqlite::params![
                prepared.plan.operation_id,
                "0".repeat(64),
                Utc::now().to_rfc3339(),
            ],
        )
        .expect("inject structurally valid but unauthenticated archive intent");
    drop(database);
    drop(storage);

    let reopened = Storage::open(root.path()).expect("reopen corrupted journal root");
    reopened
        .list_unresolved_provider_credential_operations()
        .expect_err("unresolved scan rejects forged historical cleanup evidence");
    reopened
        .get_provider_credential_operation(&prepared.plan.operation_id)
        .expect_err("exact load rejects forged historical cleanup evidence");
    reopened
        .get_provider_connection(&ProviderConnectionId::from("forged-archive-intent"))
        .expect("forged archive intent never archives the active connection");
}

#[test]
fn insert_or_replace_cannot_rewrite_operation_sequence_or_unresolved_slot_history() {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open storage");
    insert_credential_connection(&storage, "operation-no-replace");
    let authority_a = install_owned_credential(&storage, "operation-no-replace");
    let database_path = active_database_path(root.path());
    let database = Connection::open(&database_path).expect("open no-replace fixture");
    assert_provider_credential_operation_replace_rejected(
        &database,
        &authority_a.authority_id,
        "alternate-id-same-sequence",
        0,
    );
    drop(database);

    let unresolved = prepare_authority_bound_install(&storage, "operation-no-replace");
    let database = Connection::open(database_path).expect("reopen no-replace fixture");
    assert_provider_credential_operation_replace_rejected(
        &database,
        &unresolved.plan.operation_id,
        "alternate-id-different-unresolved-sequence",
        1,
    );
}

fn prepare_authority_bound_install(
    storage: &Storage,
    connection_id: &str,
) -> StoredProviderCredentialOperation {
    let connection_id = ProviderConnectionId::from(connection_id);
    let authority = storage
        .propose_provider_credential_install_authority(&connection_id)
        .expect("propose authority-bound install");
    storage
        .prepare_provider_credential_operation_with_install_authority(
            &connection_id,
            ProviderCredentialOperationKind::Install,
            ProviderCredentialObservedStatus::Missing,
            Some(&authority),
        )
        .expect("prepare authority-bound install")
}

fn install_owned_credential(
    storage: &Storage,
    connection_id: &str,
) -> ProviderCredentialAccessAuthority {
    let prepared = prepare_authority_bound_install(storage, connection_id);
    assert!(prepared.plan.predecessor_authority_id.is_none());
    storage
        .start_provider_credential_operation(&prepared.plan.operation_id, &prepared.plan_sha256)
        .expect("start first ordinary install");
    let completed = storage
        .finish_provider_credential_operation(
            &prepared.plan.operation_id,
            &prepared.plan_sha256,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("finish first ordinary install");
    assert_eq!(
        completed.status,
        ProviderCredentialOperationStatus::Succeeded
    );
    let authority = storage
        .ensure_provider_credential_access_settled(&ProviderConnectionId::from(connection_id))
        .expect("first ordinary install grants durable access");
    assert_eq!(authority.authority_id, prepared.plan.operation_id);
    authority
}

fn provider_credential_ownership_state(root: &Path, connection_id: &str) -> String {
    Connection::open(active_database_path(root))
        .expect("open ownership projection")
        .query_row(
            "SELECT ownership_state
             FROM provider_credential_ownership
             WHERE connection_id = ?1 AND credential_ref = ?1",
            [connection_id],
            |row| row.get(0),
        )
        .expect("read provider credential ownership state")
}

fn register_discovery_integrity_function_names(database: &Connection) {
    // This test deliberately bypasses `Storage` to issue a raw SQL REPLACE
    // against an ordinary ownership row. Register fail-closed placeholders so
    // SQLite can prepare the trigger's discovery-only view references; a
    // configured production connection installs the real implementations.
    let flags = FunctionFlags::SQLITE_UTF8
        | FunctionFlags::SQLITE_DETERMINISTIC
        | FunctionFlags::SQLITE_INNOCUOUS;
    for (name, arguments) in [
        ("lorepia_sha256_hex", 1),
        ("lorepia_discovery_commit_plan_sha256", 1),
        ("lorepia_canonical_origin", 1),
        ("lorepia_header_name", 1),
        ("lorepia_native_no_effect_evidence_sha256", 8),
    ] {
        database
            .create_scalar_function(name, arguments, flags, |_| {
                Ok::<Option<String>, rusqlite::Error>(None)
            })
            .expect("register discovery integrity function name for ordinary-row trigger test");
    }
}

fn suspend_trigger(database: &Connection, trigger_name: &str) -> String {
    let trigger_sql = database
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type = 'trigger' AND name = ?1",
            [trigger_name],
            |row| row.get::<_, String>(0),
        )
        .expect("load trigger definition");
    database
        .execute_batch(&format!("DROP TRIGGER {trigger_name};"))
        .expect("suspend trigger for synthetic corruption fixture");
    trigger_sql
}

fn assert_sql_rejects_unproven_replacement_cleanup(
    root: &Path,
    connection_id: &str,
    operation_id: &str,
) {
    let mut database = Connection::open(active_database_path(root))
        .expect("open replacement cleanup schema proof database");
    register_discovery_integrity_function_names(&database);
    let transaction = database
        .transaction()
        .expect("begin replacement cleanup schema proof");
    let sequence = transaction
        .query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1
             FROM provider_credential_operation_attestations
             WHERE operation_id = ?1",
            [operation_id],
            |row| row.get::<_, u64>(0),
        )
        .expect("allocate synthetic recovery attestation sequence");
    transaction
        .execute(
            "INSERT INTO provider_credential_operation_attestations
             (operation_id, sequence, stage, slot_status, evidence_sha256,
              native_owner, schema_version, redaction_version, attested_at)
             VALUES (?1, ?2, 'recovery', 'missing', ?3,
                     'native_platform', 1, 1, ?4)",
            rusqlite::params![
                operation_id,
                sequence,
                "0".repeat(64),
                Utc::now().to_rfc3339()
            ],
        )
        .expect("insert synthetic recovery attestation");
    let terminal_sql = "UPDATE provider_credential_operations
         SET status = 'no_effect', outcome_code = 'native_effect_absent',
             outcome_attestation_sequence = ?2
         WHERE id = ?1";
    let terminal_error = transaction
        .execute(terminal_sql, rusqlite::params![operation_id, sequence])
        .expect_err("SQL guard rejects replacement cleanup without predecessor proof");
    assert!(
        terminal_error
            .to_string()
            .contains("credential operation outcome lacks a matching native attestation")
    );

    suspend_trigger(
        &transaction,
        "provider_credential_operation_outcome_attestation_guard",
    );
    assert_eq!(
        transaction
            .execute(terminal_sql, rusqlite::params![operation_id, sequence])
            .expect("bypass only the outcome guard in the synthetic fixture"),
        1
    );
    let ownership_error = transaction
        .execute(
            "INSERT INTO provider_credential_ownership_events
             (connection_id, authority_sequence, ownership_state,
              connection_binding_sha256, authority_id, source_kind, source_id, created_at)
             SELECT ?1, COALESCE(MAX(authority_sequence), 0) + 1, 'removed',
                    NULL, ?2, 'ordinary_operation', ?2, ?3
             FROM provider_credential_ownership_events
             WHERE connection_id = ?1",
            rusqlite::params![connection_id, operation_id, Utc::now().to_rfc3339()],
        )
        .expect_err("ownership event guard independently rejects missing predecessor proof");
    assert!(
        ownership_error
            .to_string()
            .contains("provider credential ownership event lacks durable authority"),
        "unexpected ownership-event guard error: {ownership_error}"
    );
    transaction
        .rollback()
        .expect("roll back synthetic schema bypass");
}

fn restore_trigger(database: &Connection, trigger_sql: &str) {
    database
        .execute_batch(trigger_sql)
        .expect("restore trigger after synthetic corruption fixture");
}

fn assert_provider_credential_operation_replace_rejected(
    database: &Connection,
    source_id: &str,
    replacement_id: &str,
    operation_sequence_delta: u64,
) {
    let error = database
        .execute(
            "INSERT OR REPLACE INTO provider_credential_operations
             SELECT ?2, connection_id, credential_ref,
                    operation_sequence + ?3, operation_kind,
                    connection_binding_sha256, ?2,
                    credential_authority_binding_sha256,
                    predecessor_authority_id,
                    predecessor_authority_binding_sha256,
                    json_set(
                        plan_json,
                        '$.operation_id', ?2,
                        '$.operation_sequence', operation_sequence + ?3,
                        '$.credential_authority_id', ?2
                    ),
                    plan_sha256, preflight_status, preflight_evidence_sha256,
                    preflight_attested_at, native_owner, status, outcome_code,
                    outcome_attestation_sequence, schema_version,
                    redaction_version, created_at, started_at, finished_at,
                    updated_at
             FROM provider_credential_operations
             WHERE id = ?1",
            rusqlite::params![source_id, replacement_id, operation_sequence_delta],
        )
        .expect_err("INSERT OR REPLACE must not erase credential operation history");
    assert!(
        error
            .to_string()
            .contains("provider credential operation cannot replace existing history"),
        "the append-only trigger, not a later incidental constraint, must reject replacement: {error}"
    );
}

fn insert_credential_connection(storage: &Storage, id: &str) {
    let api_origin = CanonicalOrigin::parse("https://api.example.test").expect("origin");
    let now = Utc::now();
    storage
        .insert_provider_connection(&ProviderConnection {
            id: ProviderConnectionId::from(id),
            template_id: ProviderTemplateId::from(TEMPLATE_ID),
            template_version: 1,
            display_name: format!("Credential {id}"),
            api_origin: api_origin.clone(),
            config: ConnectionConfig {
                api_base_path: Some(EndpointPath::parse("/v1").expect("base path")),
                network_mode: ProviderNetworkMode::Public,
                local_network_approval: None,
                values: vec![ConnectionConfigEntry {
                    key: "api_base_url".to_owned(),
                    value: ConnectionConfigValue::Text("https://api.example.test/v1".to_owned()),
                }],
            },
            credential_ref: Some(CredentialRef(id.to_owned())),
            credential_scope: Some(CredentialScope {
                allowed_origins: vec![api_origin],
                auth_binding: AuthBinding::BearerHeader,
                redirect_policy: CredentialRedirectPolicy::Deny,
            }),
            timeout_seconds: 30,
            status: ConnectionStatus::Untested,
            created_at: now,
            updated_at: now,
        })
        .expect("insert credential-bound connection");
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn active_database_path(root: &Path) -> PathBuf {
    let cutover = root.join("db/schema-cutover");
    let (_, relative) = std::fs::read_dir(cutover)
        .expect("read committed database generations")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join("generation-committed.json").is_file())
        .map(|entry| {
            let manifest = serde_json::from_slice::<serde_json::Value>(
                &std::fs::read(entry.path().join("generation-manifest.json"))
                    .expect("read generation manifest"),
            )
            .expect("parse generation manifest");
            let sequence = manifest["activation_sequence"]
                .as_u64()
                .expect("generation activation sequence");
            let relative = manifest["active_database_relative_path"]
                .as_str()
                .expect("active database relative path")
                .to_owned();
            (sequence, relative)
        })
        .max_by_key(|(sequence, _)| *sequence)
        .expect("at least one committed database generation");
    root.join(relative)
}
use std::path::{Path, PathBuf};
