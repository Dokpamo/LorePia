use lorepia_domain::{
    ConversationBranchId, ConversationId, CoreError, CoreResult, InteractionEvent,
    InteractionRuleId, Sha256Digest,
};
use rusqlite::{Connection, OptionalExtension};

use crate::{InteractionEvaluationSeal, interaction_evaluation_seal_sha256};

use super::super::super::event_transactions::{StoredEventPayload, stored_module_plan_sha256};
use super::super::super::projections::{
    decode_interaction_policy, decode_stored_interaction_event,
    validate_stored_event_evaluation_authority,
};
use super::super::super::types::{
    InteractionPolicySnapshot, MAX_AUDIT_JSON_BYTES, MAX_EVENT_JSON_BYTES, MAX_STATE_JSON_BYTES,
    interaction_event_sha256,
};
use super::super::super::{
    decode_json, decode_u64_hex, encode_json, parse_datetime, sha256_hex, storage_corrupted,
    storage_db_error, u64_from_i64,
};
use super::super::StoredInteractionDerivedEvent;
use super::RawDerivedOutboxRow;

pub(in super::super) fn decode_claimed_derived_outbox_row(
    connection: &Connection,
    raw: RawDerivedOutboxRow,
) -> CoreResult<StoredInteractionDerivedEvent> {
    if raw.status != "claimed" || raw.lease_until.is_none() {
        return Err(storage_corrupted(
            "derived interaction occurrence is not durably claimed",
        ));
    }
    decode_derived_outbox_row(connection, raw)
}

pub(in super::super) fn decode_derived_outbox_row(
    connection: &Connection,
    raw: RawDerivedOutboxRow,
) -> CoreResult<StoredInteractionDerivedEvent> {
    if !matches!(raw.status.as_str(), "pending" | "claimed" | "acknowledged") {
        return Err(storage_corrupted(
            "derived interaction occurrence status is invalid",
        ));
    }
    let depth = u32::try_from(raw.depth)
        .map_err(|_| storage_corrupted("derived interaction depth is invalid"))?;
    let chain_ordinal = u32::try_from(raw.chain_ordinal)
        .map_err(|_| storage_corrupted("derived interaction ordinal is invalid"))?;
    let source_effect_ordinal = u32::try_from(raw.source_effect_ordinal)
        .map_err(|_| storage_corrupted("derived source effect ordinal is invalid"))?;
    let source_action_ordinal = u32::try_from(raw.source_action_ordinal)
        .map_err(|_| storage_corrupted("derived source action ordinal is invalid"))?;
    let event =
        decode_stored_interaction_event(&raw.event_kind, Some(raw.event_argument_json.as_str()))?;
    if !matches!(
        event,
        InteractionEvent::VariableChanged { .. } | InteractionEvent::KnowledgeActivated { .. }
    ) {
        return Err(storage_corrupted(
            "derived interaction occurrence has a forbidden event kind",
        ));
    }
    let event_sha256 = Sha256Digest::parse(raw.event_sha256).map_err(CoreError::invalid)?;
    if interaction_event_sha256(&event)? != event_sha256 {
        return Err(storage_corrupted(
            "derived interaction event digest is invalid",
        ));
    }
    let policy = decode_interaction_policy(
        &stored_module_plan_sha256_from_json(&raw.policy_json)?,
        &raw.policy_json,
        &raw.policy_sha256,
    )?;
    let policy_sha256 = Sha256Digest::parse(raw.policy_sha256).map_err(CoreError::invalid)?;
    if raw.evaluation_seal_version != 1 {
        return Err(storage_corrupted(
            "derived interaction occurrence has no v1 evaluation seal",
        ));
    }
    let evaluation_seal_json = raw.evaluation_seal_json.ok_or_else(|| {
        storage_corrupted("derived interaction occurrence evaluation seal is missing")
    })?;
    let stored_evaluation_seal_sha256 = raw.evaluation_seal_sha256.ok_or_else(|| {
        storage_corrupted("derived interaction occurrence evaluation seal hash is missing")
    })?;
    let evaluation_seal: InteractionEvaluationSeal = decode_json(
        "derived interaction evaluation seal",
        &evaluation_seal_json,
        MAX_STATE_JSON_BYTES,
    )?;
    let canonical_evaluation_seal_json = encode_json(
        "derived interaction evaluation seal",
        &evaluation_seal,
        MAX_STATE_JSON_BYTES,
    )?;
    let evaluation_seal_sha256 = interaction_evaluation_seal_sha256(&evaluation_seal)?;
    if canonical_evaluation_seal_json != evaluation_seal_json
        || evaluation_seal_sha256.as_str() != stored_evaluation_seal_sha256
        || evaluation_seal.policy_sha256 != policy_sha256
    {
        return Err(storage_corrupted(
            "derived interaction occurrence evaluation seal is invalid",
        ));
    }
    let deterministic_seed_hex = raw.deterministic_seed_hex.ok_or_else(|| {
        storage_corrupted("derived interaction occurrence deterministic seed is missing")
    })?;
    let deterministic_seed = decode_u64_hex(
        "derived interaction deterministic seed",
        &deterministic_seed_hex,
    )?;
    let source_action_sha256 =
        Sha256Digest::parse(&raw.source_action_sha256).map_err(CoreError::invalid)?;
    let expected_occurrence_hash = sha256_hex(
        encode_json(
            "derived interaction occurrence identity",
            &(
                "lorepia.interaction-derived-occurrence.v1",
                &raw.chain_id,
                &raw.parent_event_id,
                source_effect_ordinal,
                &event_sha256,
                &source_action_sha256,
                evaluation_seal_sha256.as_str(),
                deterministic_seed,
            ),
            MAX_AUDIT_JSON_BYTES,
        )?
        .as_bytes(),
    );
    if raw.occurrence_id != format!("interaction-derived-{expected_occurrence_hash}") {
        return Err(storage_corrupted(
            "derived interaction occurrence identity fingerprint is invalid",
        ));
    }
    let visited_event_sha256s: Vec<Sha256Digest> = decode_json(
        "derived interaction visited events",
        &raw.visited_event_sha256s_json,
        MAX_AUDIT_JSON_BYTES,
    )?;
    if visited_event_sha256s.len() != usize::try_from(depth).unwrap_or(usize::MAX)
        || visited_event_sha256s.contains(&event_sha256)
    {
        return Err(storage_corrupted(
            "derived interaction visited-set evidence is invalid",
        ));
    }
    let (
        parent_payload_json,
        parent_resulting_state_revision,
        parent_evaluation_seal_json,
        parent_evaluation_seal_sha256,
        parent_evaluation_seal_version,
        parent_policy_sha256,
    ) = connection
        .query_row(
            "SELECT payload_json, resulting_state_revision,
                    evaluation_seal_json, evaluation_seal_sha256,
                    evaluation_seal_version, policy_sha256
             FROM interaction_events WHERE id = ?1",
            [&raw.parent_event_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| storage_corrupted("derived interaction parent event is missing"))?;
    let parent_payload: StoredEventPayload = decode_json(
        "derived interaction parent payload",
        &parent_payload_json,
        MAX_EVENT_JSON_BYTES,
    )?;
    validate_stored_event_evaluation_authority(
        &parent_policy_sha256,
        parent_evaluation_seal_json.as_deref(),
        parent_evaluation_seal_sha256.as_deref(),
        parent_evaluation_seal_version,
        &parent_payload,
    )?;
    if parent_payload.commit_sha256 != raw.parent_event_commit_sha256
        || parent_resulting_state_revision != raw.parent_resulting_state_revision
        || parent_payload.evaluation_seal_sha256.as_ref() != Some(&evaluation_seal_sha256)
    {
        return Err(storage_corrupted(
            "derived interaction parent event evidence is invalid",
        ));
    }
    Ok(StoredInteractionDerivedEvent {
        occurrence_id: raw.occurrence_id,
        chain_id: raw.chain_id,
        root_event_id: raw.root_event_id,
        parent_event_id: raw.parent_event_id,
        parent_occurrence_id: raw.parent_occurrence_id,
        conversation_id: ConversationId(raw.conversation_id),
        branch_id: ConversationBranchId(raw.branch_id),
        depth,
        chain_ordinal,
        source_effect_ordinal,
        parent_event_commit_sha256: Sha256Digest::parse(raw.parent_event_commit_sha256)
            .map_err(CoreError::invalid)?,
        parent_resulting_state_revision: u64_from_i64(
            "derived parent resulting state revision",
            raw.parent_resulting_state_revision,
        )?,
        source_effect_sha256: Sha256Digest::parse(raw.source_effect_sha256)
            .map_err(CoreError::invalid)?,
        source_action_sha256,
        source_set_revision_id: raw.source_set_revision_id,
        source_rule_id: InteractionRuleId::from(raw.source_rule_id),
        source_action_ordinal,
        event,
        event_sha256,
        visited_event_sha256s,
        policy,
        policy_sha256,
        evaluation_seal,
        evaluation_seal_sha256,
        deterministic_seed,
        occurred_at: parse_datetime("derived interaction occurred_at", &raw.occurred_at)?,
        available_at: parse_datetime("derived interaction available_at", &raw.available_at)?,
        delivery_attempts: u64_from_i64(
            "derived interaction delivery attempts",
            raw.delivery_attempts,
        )?,
        lease_until: raw
            .lease_until
            .as_deref()
            .map(|value| parse_datetime("derived interaction lease_until", value))
            .transpose()?,
    })
}

fn stored_module_plan_sha256_from_json(policy_json: &str) -> CoreResult<String> {
    let policy: InteractionPolicySnapshot = decode_json(
        "derived interaction policy",
        policy_json,
        MAX_EVENT_JSON_BYTES,
    )?;
    Ok(stored_module_plan_sha256(&policy))
}
