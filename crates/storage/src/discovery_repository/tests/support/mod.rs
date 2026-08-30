mod authority_rows;
mod completed_authority_seed;
mod native_fixture;
mod native_restart;
mod native_validation;

use super::*;

pub(super) use authority_rows::{
    active_credential_ownership_tail,
    assert_discovery_authority_history_replace_is_guarded_and_revalidated,
    complete_ordinary_credential_successor, direct_completed_discovery_replay_write,
    persist_pending_confirmed_commit_completion,
};
use authority_rows::{insert_test_discovery_approval, insert_test_discovery_receipt};
pub(super) use completed_authority_seed::{
    project_completed_discovery_credential_authority,
    project_completed_discovery_credential_authority_at, seed_completed_discovery_authority,
    seed_completed_discovery_authority_with_mode,
};
pub(super) use native_fixture::{
    assert_native_execution_table_is_append_only, bypass_native_execution_table_version_guard,
    native_credential_commit_plan, native_no_effect_completion,
    raw_test_native_physical_authority_id, reserve_and_start_test_native_execution,
    seed_prepared_native_credential_commit, seed_started_native_credential_commit,
    test_native_physical_authority_id,
};
pub(super) use native_restart::{
    assert_unstarted_prepared_retry_predecessors, corrupt_retry_start_terminal_event_sequence,
    operation_status, restart_attested_native_retry, restart_prepared_native_credential_commit,
    restart_started_native_credential_commit, restart_unknown_native_credential_commit,
    restart_unstarted_prepared_native_commit,
};
pub(super) use native_validation::{
    assert_malformed_native_commit_root_rejected,
    assert_native_attestation_and_terminal_schema_rejected,
    assert_self_consistent_malformed_native_plan_rejected,
};

pub(super) struct CompletedDiscoveryAuthorityFixture {
    pub(super) root: TempDir,
    pub(super) storage: Storage,
    pub(super) session_id: DiscoverySessionId,
    pub(super) connection_id: ProviderConnectionId,
    pub(super) attempt_id: DiscoveryCommitAttemptId,
    pub(super) operation_id: DiscoveryOperationId,
    pub(super) authority_operation_id: DiscoveryOperationId,
    pub(super) physical_authority_id: String,
    pub(super) evidence_id: EvidenceId,
    pub(super) binding_sha256: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CompletedDiscoveryAuthorityMode {
    Direct,
    Reconciled,
    PendingReconciled,
    PreparedInterruptedRetry,
    UnknownNoEffectRetry,
    ConfirmedCommitCompensation,
    ConfirmedNoEffectCompensation,
}

pub(super) struct NativeNoEffectFixture {
    pub(super) session: ProviderDiscoverySession,
    pub(super) operation_id: DiscoveryOperationId,
    pub(super) attempt_id: DiscoveryCommitAttemptId,
    pub(super) plan_sha256: String,
}

pub(super) struct RestartedNativeCommitFixture {
    pub(super) session: ProviderDiscoverySession,
    pub(super) operation_id: DiscoveryOperationId,
    pub(super) predecessor_action_id: DiscoveryActionId,
}

pub(super) struct UnstartedPreparedNativeRetryStep {
    pub(super) next_operation_id: DiscoveryOperationId,
    pub(super) interrupt_hash_byte: char,
    pub(super) restart_hash_byte: char,
    pub(super) interrupted_at_millis: i64,
    pub(super) restarted_at_millis: i64,
}

pub(super) fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 31, 12, 0, 0)
        .single()
        .expect("valid test time")
}

pub(super) fn suspend_test_trigger(
    connection: &rusqlite::Connection,
    trigger_name: &str,
) -> String {
    let trigger_sql = connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type = 'trigger' AND name = ?1",
            [trigger_name],
            |row| row.get::<_, String>(0),
        )
        .expect("load initial-state guard definition");
    connection
        .execute_batch(&format!("DROP TRIGGER {trigger_name};"))
        .expect("suspend initial-state guard for synthetic fixture");
    trigger_sql
}

pub(super) fn restore_test_trigger(connection: &rusqlite::Connection, trigger_sql: &str) {
    connection
        .execute_batch(trigger_sql)
        .expect("restore initial-state guard after synthetic fixture");
}

pub(super) fn draft_session(id: &str) -> ProviderDiscoverySession {
    ProviderDiscoverySession::new(
        DiscoverySessionId::from(id),
        SanitizedDiscoveryInput {
            connection_id: ProviderConnectionId::from(format!("{id}-connection")),
            display_name: "Test provider".to_owned(),
            site_url: HttpUrl::parse("https://provider.example/").expect("site URL"),
            docs_url: Some(HttpUrl::parse("https://provider.example/docs").expect("docs URL")),
            credential_ref: None,
            preferred_assistant: None,
            connection_options: ProviderDiscoveryConnectionOptions::default(),
            supplied_evidence_ids: Vec::new(),
        },
    )
    .expect("draft discovery session")
}

pub(super) fn archive_credential_bound_connection(
    storage: &Storage,
    connection_id: &ProviderConnectionId,
) {
    let archive = storage
        .prepare_provider_credential_operation(
            connection_id,
            ProviderCredentialOperationKind::RemoveForArchive,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("prepare terminal discovery provider archive");
    storage
        .finish_provider_credential_archive(
            &archive.plan.operation_id,
            &archive.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("terminal discovery history permits provider archive");
}

pub(super) fn apply(
    session: &ProviderDiscoverySession,
    action: ProviderDiscoveryAction,
    hash_byte: char,
) -> lorepia_domain::discovery::DiscoveryTransition {
    session
        .apply(&DiscoveryActionEnvelope {
            id: DiscoveryActionId::new(),
            expected_revision: session.revision,
            request_sha256: std::iter::repeat_n(hash_byte, 64).collect(),
            action,
        })
        .expect("valid discovery action")
}

pub(super) fn write(
    transition: lorepia_domain::discovery::DiscoveryTransition,
    new_operation_id: Option<DiscoveryOperationId>,
    completed_operation: Option<DiscoveryCompletedOperationWrite>,
) -> DiscoveryTransitionWrite {
    DiscoveryTransitionWrite {
        transition,
        draft: DiscoveryJsonUpdate::Preserve,
        review: DiscoveryJsonUpdate::Preserve,
        new_evidence: Vec::new(),
        new_candidates: Vec::new(),
        approval: None,
        new_operation_id,
        completed_operation,
        prepared_commit: None,
        provider_graph: None,
        occurred_at: now(),
    }
}
