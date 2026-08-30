mod occurrence;

use lorepia_domain::CoreResult;
use rusqlite::{Connection, OptionalExtension};

use super::super::storage_db_error;

pub(super) use occurrence::{decode_claimed_derived_outbox_row, decode_derived_outbox_row};

#[derive(Debug)]
pub(super) struct RawDerivedOutboxRow {
    occurrence_id: String,
    chain_id: String,
    root_event_id: String,
    parent_event_id: String,
    parent_occurrence_id: Option<String>,
    conversation_id: String,
    branch_id: String,
    depth: i64,
    chain_ordinal: i64,
    source_effect_ordinal: i64,
    parent_event_commit_sha256: String,
    parent_resulting_state_revision: i64,
    source_effect_sha256: String,
    source_action_sha256: String,
    source_set_revision_id: String,
    source_rule_id: String,
    source_action_ordinal: i64,
    event_kind: String,
    event_argument_json: String,
    event_sha256: String,
    visited_event_sha256s_json: String,
    policy_json: String,
    policy_sha256: String,
    evaluation_seal_json: Option<String>,
    evaluation_seal_sha256: Option<String>,
    evaluation_seal_version: i64,
    deterministic_seed_hex: Option<String>,
    occurred_at: String,
    available_at: String,
    pub(super) status: String,
    delivery_attempts: i64,
    lease_until: Option<String>,
}

pub(super) fn read_derived_outbox_row(
    connection: &Connection,
    occurrence_id: &str,
) -> CoreResult<Option<RawDerivedOutboxRow>> {
    connection
        .query_row(
            "SELECT occurrence_id, chain_id, root_event_id, parent_event_id,
                    parent_occurrence_id, conversation_id, branch_id, depth,
                    chain_ordinal, source_effect_ordinal,
                    parent_event_commit_sha256, parent_resulting_state_revision,
                    source_effect_sha256,
                    source_action_sha256, source_set_revision_id, source_rule_id,
                    source_action_ordinal, event_kind, event_argument_json,
                    event_sha256, visited_event_sha256s_json, policy_json,
                    policy_sha256, evaluation_seal_json,
                    evaluation_seal_sha256, evaluation_seal_version,
                    deterministic_seed_hex, occurred_at, available_at, status,
                    delivery_attempts, lease_until
             FROM interaction_derived_event_outbox WHERE occurrence_id = ?1",
            [occurrence_id],
            |row| {
                Ok(RawDerivedOutboxRow {
                    occurrence_id: row.get(0)?,
                    chain_id: row.get(1)?,
                    root_event_id: row.get(2)?,
                    parent_event_id: row.get(3)?,
                    parent_occurrence_id: row.get(4)?,
                    conversation_id: row.get(5)?,
                    branch_id: row.get(6)?,
                    depth: row.get(7)?,
                    chain_ordinal: row.get(8)?,
                    source_effect_ordinal: row.get(9)?,
                    parent_event_commit_sha256: row.get(10)?,
                    parent_resulting_state_revision: row.get(11)?,
                    source_effect_sha256: row.get(12)?,
                    source_action_sha256: row.get(13)?,
                    source_set_revision_id: row.get(14)?,
                    source_rule_id: row.get(15)?,
                    source_action_ordinal: row.get(16)?,
                    event_kind: row.get(17)?,
                    event_argument_json: row.get(18)?,
                    event_sha256: row.get(19)?,
                    visited_event_sha256s_json: row.get(20)?,
                    policy_json: row.get(21)?,
                    policy_sha256: row.get(22)?,
                    evaluation_seal_json: row.get(23)?,
                    evaluation_seal_sha256: row.get(24)?,
                    evaluation_seal_version: row.get(25)?,
                    deterministic_seed_hex: row.get(26)?,
                    occurred_at: row.get(27)?,
                    available_at: row.get(28)?,
                    status: row.get(29)?,
                    delivery_attempts: row.get(30)?,
                    lease_until: row.get(31)?,
                })
            },
        )
        .optional()
        .map_err(storage_db_error)
}
