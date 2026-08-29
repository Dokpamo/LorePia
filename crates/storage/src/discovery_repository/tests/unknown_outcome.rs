use lorepia_domain::{
    CanonicalOrigin, CoreErrorCode, EvidenceId, ModelRouteId,
    discovery::{DiscoveryApprovalBinding, DiscoveryApprovalGrant, DiscoveryApprovalId},
};
use tempfile::tempdir;

use crate::Storage;

use super::{draft_session, now, restore_test_trigger, sha256_hex, suspend_test_trigger};

#[test]
fn unknown_billable_outcome_rejects_approval_with_missing_references() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let draft = draft_session("session-billable-unknown");
    storage
        .create_discovery_session(&draft, now())
        .expect("create draft");
    let approval_id = DiscoveryApprovalId::parse("approval-assistant").expect("approval id");
    let grant = DiscoveryApprovalGrant::AssistantConsent {
        assistant_route_id: ModelRouteId::from("assistant-route"),
        evidence_ids: vec![EvidenceId::from("evidence-assistant")],
        allowed_document_origins: vec![
            CanonicalOrigin::parse("https://provider.example/").expect("origin"),
        ],
        max_calls: 2,
        max_input_tokens: 4_096,
        max_output_tokens: 2_048,
        max_tool_calls: 4,
        max_retries: 1,
        max_cost_micro_units: 1_000_000,
    };
    let grant_json = serde_json::to_string(&grant).expect("grant JSON");
    let grant_sha256 = sha256_hex(grant_json.as_bytes());
    let binding = DiscoveryApprovalBinding {
        approval_id: approval_id.clone(),
        grant_sha256: grant_sha256.clone(),
    };
    let binding_json = serde_json::to_string(&binding).expect("binding JSON");
    {
        let mut connection = storage.connection().expect("database connection");
        let operation_guard = suspend_test_trigger(
            &connection,
            "provider_discovery_operation_initial_state_guard",
        );
        let transaction = connection.transaction().expect("transaction");
        transaction
            .execute(
                "INSERT INTO provider_discovery_approvals (
                     id, session_id, approval_kind, candidate_id, decision,
                     grant_json, session_revision, grant_sha256, redaction_version, created_at
                 ) VALUES (?1, ?2, 'assistant_consent', NULL, 'approved',
                     ?3, 0, ?4, 1, ?5)",
                rusqlite::params![
                    approval_id.as_str(),
                    draft.id.as_str(),
                    grant_json,
                    grant_sha256,
                    now().to_rfc3339(),
                ],
            )
            .expect("approval row");
        transaction
            .execute(
                "INSERT INTO provider_discovery_operations (
                     id, session_id, operation_kind, side_effect_class, status,
                     action_id, expected_revision, request_sha256, approval_id,
                     approval_grant_sha256, started_at, finished_at, created_at, updated_at
                 ) VALUES (
                     'operation-assistant', ?1, 'build_assistant_manifest_draft',
                     'billable_external', 'outcome_unknown', 'action-assistant', 0,
                     ?2, ?3, ?4, ?5, ?5, ?5, ?5
                 )",
                rusqlite::params![
                    draft.id.as_str(),
                    "a".repeat(64),
                    approval_id.as_str(),
                    binding.grant_sha256,
                    now().to_rfc3339(),
                ],
            )
            .expect("operation row");
        restore_test_trigger(&transaction, &operation_guard);
        transaction
            .execute(
                "UPDATE provider_discovery_sessions
                 SET state = 'unknown_outcome',
                     revision = 1,
                     next_event_sequence = 2,
                     unknown_operation = 'build_assistant_manifest_draft',
                     active_operation_id = NULL,
                     active_effect_approval_json = ?2,
                     updated_at = ?3
                 WHERE id = ?1",
                rusqlite::params![draft.id.as_str(), binding_json, now().to_rfc3339()],
            )
            .expect("unknown session state");
        transaction.commit().expect("commit fixture");
    }
    let error = storage
        .get_discovery_session(&draft.id)
        .expect_err("missing assistant evidence and route must fail closed");
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
}
