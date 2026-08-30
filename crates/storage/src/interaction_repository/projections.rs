use std::collections::BTreeMap;

use lorepia_domain::{
    ConversationBranchId, ConversationId, CoreError, CoreResult, GenerationId, InteractionEvent,
    InteractionProposalRecord, InteractionProposalRecordId, InteractionProposalStatus,
    InteractionRuleId, MessageId, Sha256Digest,
};
use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};

use crate::{InteractionEvaluationSeal, Storage, interaction_evaluation_seal_sha256};

use super::{
    InteractionEffectHistoryCursor, InteractionEventOccurrenceLookup, InteractionPolicySnapshot,
    MAX_AUDIT_JSON_BYTES, MAX_EVENT_JSON_BYTES, MAX_STATE_JSON_BYTES, StoredEventPayload,
    StoredInteractionEffectHistory, StoredInteractionEvent, StoredInteractionProposal, decode_json,
    decode_stored_event_payload, encode_json, event_requires_argument, i64_from_u64,
    interaction_event_kind, interaction_policy_sha256, is_sha256, not_found, parse_datetime,
    proposal_status, proposal_status_wire, read_effect_history, read_effect_history_page,
    read_latest_region_effects, read_older_reopen_effect_history,
    read_pending_choice_effect_history, read_recent_reopen_effect_history, revision_conflict,
    sha256_hex, storage_corrupted, storage_db_error, stored_module_plan_sha256, u32_from_i64,
    u64_from_i64, validate_effect_poll_limit, validate_event_generation_attempt_shape,
    validate_event_owner_message_shape, validate_nonempty_id, validate_policy_shape,
    validate_proposal_list_limit, validate_stored_interaction_policy_rule_sets,
};

impl Storage {
    /// Returns an already-committed exact occurrence before Core reevaluates
    /// it against a potentially advanced interaction state.
    pub fn get_interaction_event_by_occurrence(
        &self,
        lookup: &InteractionEventOccurrenceLookup,
    ) -> CoreResult<Option<StoredInteractionEvent>> {
        validate_nonempty_id("interaction event id", &lookup.event_id)?;
        validate_nonempty_id("interaction event idempotency key", &lookup.idempotency_key)?;
        validate_event_generation_attempt_shape(
            &lookup.event,
            lookup.generation_attempt_id.as_ref(),
        )?;
        validate_event_owner_message_shape(&lookup.event, lookup.owner_message_id.as_ref())?;
        let connection = self.connection()?;
        read_event_by_occurrence(&connection, lookup)
    }

    /// Loads one exact committed interaction event by its durable event ID.
    ///
    /// The returned evidence is reconstructed only from immutable event,
    /// policy, checkpoint, and proposal rows. Stored fingerprints are verified
    /// before the event is returned.
    pub fn get_interaction_event(
        &self,
        event_id: &str,
    ) -> CoreResult<Option<StoredInteractionEvent>> {
        validate_nonempty_id("interaction event id", event_id)?;
        let connection = self.connection()?;
        let Some(lookup) = read_event_occurrence_lookup_by_id(&connection, event_id)? else {
            return Ok(None);
        };
        read_event_by_occurrence(&connection, &lookup)
    }
    /// Loads one proposal by its durable record ID and verifies its payload
    /// digest before returning it.
    pub fn get_interaction_proposal(
        &self,
        proposal_record_id: &InteractionProposalRecordId,
    ) -> CoreResult<StoredInteractionProposal> {
        let connection = self.connection()?;
        read_proposal(&connection, proposal_record_id)?
            .ok_or_else(|| not_found("interaction proposal"))
    }

    /// Lists proposals from one exact conversation branch and status.
    ///
    /// The result is bounded and deterministic: newest request first, with the
    /// durable record ID as a stable tie-breaker. Each entry includes both the
    /// proposal CAS revision and the current containing-state revision.
    pub fn list_interaction_proposals(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        status: InteractionProposalStatus,
        limit: u32,
    ) -> CoreResult<Vec<StoredInteractionProposal>> {
        validate_proposal_list_limit(limit)?;
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT proposal.id, proposal.interaction_state_id,
                        state.conversation_id, state.branch_id, state.revision,
                        origin.module_plan_sha256, origin.policy_json,
                        origin.policy_sha256,
                        revision.interaction_rule_set_id,
                        proposal.rule_set_revision_id, proposal.rule_id,
                        proposal.action_ordinal, proposal.proposal_id,
                        proposal.title, proposal.body, proposal.status,
                        proposal.source_interaction_state_revision,
                        proposal.proposal_revision, proposal.payload_json,
                        proposal.payload_sha256,
                        proposal.requested_at_epoch_seconds,
                        proposal.expires_at_epoch_seconds,
                        proposal.decided_at_epoch_seconds,
                        proposal.dispatched_at_epoch_seconds
                 FROM interaction_proposals AS proposal
                 JOIN interaction_state AS state
                   ON state.id = proposal.interaction_state_id
                 JOIN interaction_events AS origin
                   ON origin.interaction_state_id = proposal.interaction_state_id
                  AND origin.expected_state_revision =
                      proposal.source_interaction_state_revision
                 JOIN interaction_rule_set_revisions AS revision
                   ON revision.revision_id = proposal.rule_set_revision_id
                 WHERE state.conversation_id = ?1
                   AND state.branch_id = ?2
                   AND proposal.status = ?3
                 ORDER BY proposal.requested_at_epoch_seconds DESC,
                          proposal.id ASC
                 LIMIT ?4",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map(
                params![
                    conversation_id.0.as_str(),
                    branch_id.0.as_str(),
                    proposal_status_wire(status),
                    i64::from(limit),
                ],
                proposal_row,
            )
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?;
        rows.into_iter()
            .map(|raw| decode_proposal_row(&connection, raw))
            .collect()
    }

    /// Loads one exact durable effect, including immutable history context and
    /// mutable delivery/choice lifecycle metadata.
    pub fn get_interaction_effect(
        &self,
        effect_id: &str,
    ) -> CoreResult<StoredInteractionEffectHistory> {
        validate_nonempty_id("interaction effect id", effect_id)?;
        let connection = self.connection()?;
        read_effect_history(&connection, effect_id)?.ok_or_else(|| not_found("interaction effect"))
    }

    /// Pages immutable effect history for one exact conversation branch.
    ///
    /// Acknowledged rows remain visible. Ordering is the durable state
    /// transition order followed by the per-event effect sequence.
    pub fn list_interaction_effect_history(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        after: Option<InteractionEffectHistoryCursor>,
        limit: u32,
    ) -> CoreResult<Vec<StoredInteractionEffectHistory>> {
        validate_effect_poll_limit(limit)?;
        let connection = self.connection()?;
        read_effect_history_page(&connection, conversation_id, branch_id, after, limit, false)
    }

    /// Pages effects that may be reconstructed after reopening a branch.
    ///
    /// One-shot audio is deliberately omitted. Choice effects remain present
    /// with a durable pending/consumed/expired lifecycle.
    pub fn list_reopen_interaction_effects(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        after: Option<InteractionEffectHistoryCursor>,
        limit: u32,
    ) -> CoreResult<Vec<StoredInteractionEffectHistory>> {
        validate_effect_poll_limit(limit)?;
        let connection = self.connection()?;
        read_effect_history_page(&connection, conversation_id, branch_id, after, limit, true)
    }

    /// Returns the newest bounded reopen reconstruction window in chronological
    /// order. Callers needing older rows can page immutable history separately.
    pub fn list_recent_reopen_interaction_effects(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        limit: u32,
    ) -> CoreResult<Vec<StoredInteractionEffectHistory>> {
        validate_effect_poll_limit(limit)?;
        let connection = self.connection()?;
        read_recent_reopen_effect_history(&connection, conversation_id, branch_id, limit)
    }

    /// Pages older reopen reconstruction effects before an exclusive cursor.
    /// Rows are fetched newest-first for a bounded lookup and returned in
    /// chronological order.
    pub fn list_older_reopen_interaction_effects(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        before: InteractionEffectHistoryCursor,
        limit: u32,
    ) -> CoreResult<Vec<StoredInteractionEffectHistory>> {
        validate_effect_poll_limit(limit)?;
        if before.sequence == 0 {
            return Err(CoreError::invalid(
                "interaction effect history cursor sequence must be positive",
            ));
        }
        let connection = self.connection()?;
        read_older_reopen_effect_history(&connection, conversation_id, branch_id, before, limit)
    }

    /// Returns the latest durable `AssetShown` effect for each UI region.
    /// This bounded state projection prevents a long tail of later text events
    /// from hiding the current background, portrait, or status-panel asset.
    pub fn get_interaction_region_effects(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
    ) -> CoreResult<Vec<StoredInteractionEffectHistory>> {
        let connection = self.connection()?;
        read_latest_region_effects(&connection, conversation_id, branch_id)
    }

    /// Lists still-actionable durable choice effects for one exact room.
    pub fn list_pending_interaction_choice_effects(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        limit: u32,
    ) -> CoreResult<Vec<StoredInteractionEffectHistory>> {
        validate_effect_poll_limit(limit)?;
        let connection = self.connection()?;
        read_pending_choice_effect_history(&connection, conversation_id, branch_id, limit)
    }

    /// Reconstructs the bounded durable UI projection for one reopened branch
    /// from a single `SQLite` snapshot.
    ///
    /// The projection is the union of the recent replayable tail, the latest
    /// `AssetShown` effect in every UI region, and all bounded pending choices.
    /// Duplicate effects are removed by their durable occurrence identity and
    /// the result is returned in deterministic branch order.
    pub fn get_interaction_reopen_projection(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        recent_limit: u32,
        pending_choice_limit: u32,
    ) -> CoreResult<Vec<StoredInteractionEffectHistory>> {
        validate_effect_poll_limit(recent_limit)?;
        validate_effect_poll_limit(pending_choice_limit)?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(storage_db_error)?;
        let recent = read_recent_reopen_effect_history(
            &transaction,
            conversation_id,
            branch_id,
            recent_limit,
        )?;
        let regions = read_latest_region_effects(&transaction, conversation_id, branch_id)?;
        let pending_choices = read_pending_choice_effect_history(
            &transaction,
            conversation_id,
            branch_id,
            pending_choice_limit,
        )?;
        let mut projection = BTreeMap::new();
        for effect in recent.into_iter().chain(regions).chain(pending_choices) {
            projection.insert(effect.stored.effect_id.clone(), effect);
        }
        let mut projection = projection.into_values().collect::<Vec<_>>();
        projection.sort_by(|left, right| {
            (
                left.stored.resulting_state_revision,
                left.stored.sequence,
                left.stored.effect_id.as_str(),
            )
                .cmp(&(
                    right.stored.resulting_state_revision,
                    right.stored.sequence,
                    right.stored.effect_id.as_str(),
                ))
        });
        transaction.commit().map_err(storage_db_error)?;
        Ok(projection)
    }
}

pub(super) fn read_proposal(
    connection: &Connection,
    proposal_record_id: &InteractionProposalRecordId,
) -> CoreResult<Option<StoredInteractionProposal>> {
    let raw = connection
        .query_row(
            "SELECT proposal.id, proposal.interaction_state_id,
                    state.conversation_id, state.branch_id, state.revision,
                    origin.module_plan_sha256, origin.policy_json,
                    origin.policy_sha256,
                    revision.interaction_rule_set_id,
                    proposal.rule_set_revision_id, proposal.rule_id,
                    proposal.action_ordinal, proposal.proposal_id,
                    proposal.title, proposal.body, proposal.status,
                    proposal.source_interaction_state_revision,
                    proposal.proposal_revision, proposal.payload_json,
                    proposal.payload_sha256,
                    proposal.requested_at_epoch_seconds,
                    proposal.expires_at_epoch_seconds,
                    proposal.decided_at_epoch_seconds,
                    proposal.dispatched_at_epoch_seconds
             FROM interaction_proposals AS proposal
             JOIN interaction_state AS state
               ON state.id = proposal.interaction_state_id
             JOIN interaction_events AS origin
               ON origin.interaction_state_id = proposal.interaction_state_id
              AND origin.expected_state_revision =
                  proposal.source_interaction_state_revision
             JOIN interaction_rule_set_revisions AS revision
               ON revision.revision_id = proposal.rule_set_revision_id
             WHERE proposal.id = ?1",
            [proposal_record_id.as_str()],
            proposal_row,
        )
        .optional()
        .map_err(storage_db_error)?;
    raw.map(|raw| decode_proposal_row(connection, raw))
        .transpose()
}

#[derive(Debug)]
struct RawProposalRow {
    id: String,
    state_id: String,
    conversation_id: String,
    branch_id: String,
    state_revision: i64,
    origin_module_plan_sha256: String,
    origin_policy_json: String,
    origin_policy_sha256: String,
    rule_set_id: String,
    rule_set_revision_id: String,
    rule_id: String,
    action_ordinal: i64,
    proposal_id: String,
    title: String,
    body: String,
    status: String,
    source_revision: i64,
    proposal_revision: i64,
    payload_json: String,
    payload_sha256: String,
    requested_at: i64,
    expires_at: Option<i64>,
    decided_at: Option<i64>,
    dispatched_at: Option<i64>,
}

fn proposal_row(row: &Row<'_>) -> rusqlite::Result<RawProposalRow> {
    Ok(RawProposalRow {
        id: row.get(0)?,
        state_id: row.get(1)?,
        conversation_id: row.get(2)?,
        branch_id: row.get(3)?,
        state_revision: row.get(4)?,
        origin_module_plan_sha256: row.get(5)?,
        origin_policy_json: row.get(6)?,
        origin_policy_sha256: row.get(7)?,
        rule_set_id: row.get(8)?,
        rule_set_revision_id: row.get(9)?,
        rule_id: row.get(10)?,
        action_ordinal: row.get(11)?,
        proposal_id: row.get(12)?,
        title: row.get(13)?,
        body: row.get(14)?,
        status: row.get(15)?,
        source_revision: row.get(16)?,
        proposal_revision: row.get(17)?,
        payload_json: row.get(18)?,
        payload_sha256: row.get(19)?,
        requested_at: row.get(20)?,
        expires_at: row.get(21)?,
        decided_at: row.get(22)?,
        dispatched_at: row.get(23)?,
    })
}

fn decode_proposal_row(
    connection: &Connection,
    raw: RawProposalRow,
) -> CoreResult<StoredInteractionProposal> {
    if sha256_hex(raw.payload_json.as_bytes()) != raw.payload_sha256 {
        return Err(storage_corrupted(
            "interaction proposal payload digest does not match",
        ));
    }
    let payload_record: InteractionProposalRecord = decode_json(
        "stored interaction proposal",
        &raw.payload_json,
        MAX_EVENT_JSON_BYTES,
    )?;
    let record = InteractionProposalRecord {
        id: InteractionProposalRecordId::from(raw.id),
        rule_set_id: lorepia_domain::InteractionRuleSetId::from(raw.rule_set_id),
        rule_id: InteractionRuleId::from(raw.rule_id),
        proposal_id: raw.proposal_id,
        title: raw.title,
        body: raw.body,
        status: proposal_status(&raw.status)?,
        source_interaction_state_revision: u64_from_i64(
            "proposal source interaction state revision",
            raw.source_revision,
        )?,
        requested_at_epoch_seconds: raw.requested_at,
        expires_at_epoch_seconds: raw.expires_at,
        decided_at_epoch_seconds: raw.decided_at,
    };
    if !proposal_immutable_fields_match(&payload_record, &record) {
        return Err(storage_corrupted(
            "interaction proposal payload differs from normalized columns",
        ));
    }
    let origin_policy = decode_interaction_policy(
        &raw.origin_module_plan_sha256,
        &raw.origin_policy_json,
        &raw.origin_policy_sha256,
    )?;
    let origin_event_id = connection
        .query_row(
            "SELECT id
             FROM interaction_events
            WHERE interaction_state_id = ?1
               AND expected_state_revision = ?2",
            params![
                raw.state_id.as_str(),
                i64_from_u64(
                    "proposal source interaction state revision",
                    record.source_interaction_state_revision,
                )?,
            ],
            |row| row.get::<_, String>(0),
        )
        .map_err(storage_db_error)?;
    validate_stored_interaction_policy_rule_sets(connection, &origin_event_id, &origin_policy)?;
    Ok(StoredInteractionProposal {
        record,
        interaction_state_id: raw.state_id,
        conversation_id: ConversationId(raw.conversation_id),
        branch_id: ConversationBranchId(raw.branch_id),
        state_revision: u64_from_i64("interaction state revision", raw.state_revision)?,
        origin_policy,
        origin_policy_sha256: raw.origin_policy_sha256,
        rule_set_revision_id: raw.rule_set_revision_id,
        action_ordinal: u32_from_i64("proposal action ordinal", raw.action_ordinal)?,
        proposal_revision: u64_from_i64("interaction proposal revision", raw.proposal_revision)?,
        payload_sha256: raw.payload_sha256,
        dispatched_at_epoch_seconds: raw.dispatched_at,
    })
}

fn proposal_immutable_fields_match(
    payload: &InteractionProposalRecord,
    normalized: &InteractionProposalRecord,
) -> bool {
    payload.id == normalized.id
        && payload.rule_set_id == normalized.rule_set_id
        && payload.rule_id == normalized.rule_id
        && payload.proposal_id == normalized.proposal_id
        && payload.title == normalized.title
        && payload.body == normalized.body
        && payload.source_interaction_state_revision == normalized.source_interaction_state_revision
        && payload.requested_at_epoch_seconds == normalized.requested_at_epoch_seconds
        && payload.expires_at_epoch_seconds == normalized.expires_at_epoch_seconds
}

fn read_event_occurrence_lookup_by_id(
    connection: &Connection,
    event_id: &str,
) -> CoreResult<Option<InteractionEventOccurrenceLookup>> {
    let raw = connection
        .query_row(
            "SELECT event.idempotency_key, event.conversation_id,
                    event.branch_id, event.event_kind,
                    event.event_argument_json, event.created_at,
                    event.generation_attempt_id, checkpoint.message_id
             FROM interaction_events AS event
             LEFT JOIN interaction_state_checkpoints AS checkpoint
               ON checkpoint.source_interaction_state_id =
                    event.interaction_state_id
              AND checkpoint.state_revision =
                    event.resulting_state_revision
             WHERE event.id = ?1",
            [event_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?;
    raw.map(
        |(
            idempotency_key,
            conversation_id,
            branch_id,
            event_kind,
            event_argument_json,
            created_at,
            generation_attempt_id,
            owner_message_id,
        )| {
            let event =
                decode_stored_interaction_event(&event_kind, event_argument_json.as_deref())?;
            let lookup = InteractionEventOccurrenceLookup {
                event_id: event_id.to_owned(),
                idempotency_key,
                conversation_id: ConversationId(conversation_id),
                branch_id: ConversationBranchId(branch_id),
                event,
                generation_attempt_id: generation_attempt_id.map(GenerationId),
                owner_message_id: owner_message_id.map(MessageId),
                occurred_at: parse_datetime("interaction event created_at", &created_at)?,
            };
            validate_event_generation_attempt_shape(
                &lookup.event,
                lookup.generation_attempt_id.as_ref(),
            )?;
            validate_event_owner_message_shape(&lookup.event, lookup.owner_message_id.as_ref())?;
            Ok(lookup)
        },
    )
    .transpose()
}

pub(super) fn read_event_by_occurrence(
    connection: &Connection,
    lookup: &InteractionEventOccurrenceLookup,
) -> CoreResult<Option<StoredInteractionEvent>> {
    let mut statement = connection
        .prepare(
            "SELECT event.id, event.idempotency_key,
                    event.interaction_state_id,
                    event.expected_state_revision,
                    event.resulting_state_revision,
                    event.conversation_id, event.branch_id, event.event_kind,
                    event.event_argument_json, event.module_plan_sha256,
                    event.policy_json, event.policy_sha256, event.created_at,
                    event.generation_attempt_id, checkpoint.message_id,
                    event.payload_json, event.evaluation_seal_json,
                    event.evaluation_seal_sha256, event.evaluation_seal_version
             FROM interaction_events AS event
             LEFT JOIN interaction_state_checkpoints AS checkpoint
               ON checkpoint.source_interaction_state_id =
                    event.interaction_state_id
              AND checkpoint.state_revision =
                    event.resulting_state_revision
             WHERE event.id = ?1 OR event.idempotency_key = ?2
             ORDER BY event.id",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map(params![lookup.event_id, lookup.idempotency_key], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, String>(10)?,
                row.get::<_, String>(11)?,
                row.get::<_, String>(12)?,
                row.get::<_, Option<String>>(13)?,
                row.get::<_, Option<String>>(14)?,
                row.get::<_, String>(15)?,
                row.get::<_, Option<String>>(16)?,
                row.get::<_, Option<String>>(17)?,
                row.get::<_, i64>(18)?,
            ))
        })
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    if rows.is_empty() {
        return Ok(None);
    }
    if rows.len() != 1 {
        return Err(revision_conflict(
            "interaction occurrence event ID and idempotency key identify different events",
        ));
    }
    let (
        event_id,
        idempotency_key,
        interaction_state_id,
        expected_state_revision,
        resulting_state_revision,
        conversation_id,
        branch_id,
        event_kind,
        event_argument_json,
        module_plan_sha256,
        policy_json,
        policy_sha256,
        created_at,
        generation_attempt_id,
        owner_message_id,
        payload_json,
        evaluation_seal_json,
        evaluation_seal_sha256,
        evaluation_seal_version,
    ) = rows.into_iter().next().expect("one checked occurrence row");
    let stored_event =
        decode_stored_interaction_event(&event_kind, event_argument_json.as_deref())?;
    let created_at = parse_datetime("interaction event created_at", &created_at)?;
    if event_id != lookup.event_id
        || idempotency_key != lookup.idempotency_key
        || conversation_id != lookup.conversation_id.0
        || branch_id != lookup.branch_id.0
        || stored_event != lookup.event
        || generation_attempt_id.as_deref()
            != lookup
                .generation_attempt_id
                .as_ref()
                .map(|generation_id| generation_id.0.as_str())
        || owner_message_id.as_deref()
            != lookup
                .owner_message_id
                .as_ref()
                .map(|message_id| message_id.0.as_str())
        || created_at != lookup.occurred_at
    {
        return Err(revision_conflict(
            "interaction occurrence identity conflicts with the stored event",
        ));
    }
    let policy = decode_interaction_policy(&module_plan_sha256, &policy_json, &policy_sha256)?;
    validate_stored_interaction_policy_rule_sets(connection, &event_id, &policy)?;
    let event_payload = decode_stored_event_payload(&payload_json)?;
    validate_stored_event_evaluation_authority(
        &policy_sha256,
        evaluation_seal_json.as_deref(),
        evaluation_seal_sha256.as_deref(),
        evaluation_seal_version,
        &event_payload,
    )?;
    validate_stored_event_proposal_evidence(
        connection,
        &interaction_state_id,
        expected_state_revision,
        &event_payload.proposal_review_sha256s,
    )?;
    validate_stored_event_checkpoint_evidence(
        connection,
        &interaction_state_id,
        resulting_state_revision,
        owner_message_id.as_deref(),
        &event_payload.resulting_state_snapshot_sha256,
    )?;
    Ok(Some(StoredInteractionEvent {
        event_id,
        idempotency_key,
        interaction_state_id,
        expected_state_revision: u64_from_i64(
            "interaction event expected state revision",
            expected_state_revision,
        )?,
        resulting_state_revision: u64_from_i64(
            "interaction event resulting state revision",
            resulting_state_revision,
        )?,
        exact_replay: true,
        generation_attempt_id: generation_attempt_id.map(GenerationId),
        owner_message_id: owner_message_id.map(MessageId),
        commit_sha256: event_payload.commit_sha256,
        resulting_state_snapshot_sha256: event_payload.resulting_state_snapshot_sha256,
        proposal_review_sha256s: event_payload.proposal_review_sha256s,
        policy,
        policy_sha256,
        created_at,
    }))
}

pub(super) fn decode_stored_interaction_event(
    event_kind: &str,
    event_argument_json: Option<&str>,
) -> CoreResult<InteractionEvent> {
    let event = if let Some(argument_json) = event_argument_json {
        decode_json(
            "stored interaction event argument",
            argument_json,
            MAX_AUDIT_JSON_BYTES,
        )?
    } else {
        match event_kind {
            "conversation_opened" => InteractionEvent::ConversationOpened,
            "conversation_started" => InteractionEvent::ConversationStarted,
            "before_generation" => InteractionEvent::BeforeGeneration,
            "after_generation" => InteractionEvent::AfterGeneration,
            "message_committed" => InteractionEvent::MessageCommitted,
            "user_action" | "variable_changed" | "knowledge_activated" => {
                return Err(storage_corrupted(
                    "argument-bearing interaction event is missing its payload",
                ));
            }
            _ => {
                return Err(storage_corrupted(format!(
                    "stored interaction event kind `{event_kind}` is invalid"
                )));
            }
        }
    };
    if interaction_event_kind(&event) != event_kind
        || event_requires_argument(&event) != event_argument_json.is_some()
    {
        return Err(storage_corrupted(
            "stored interaction event kind differs from its payload",
        ));
    }
    Ok(event)
}
pub(super) fn validate_stored_event_proposal_evidence(
    connection: &Connection,
    interaction_state_id: &str,
    expected_state_revision: i64,
    expected_review_sha256s: &[String],
) -> CoreResult<()> {
    let mut statement = connection
        .prepare(
            "SELECT payload_sha256
             FROM interaction_proposals
             WHERE interaction_state_id = ?1
               AND source_interaction_state_revision = ?2
             ORDER BY payload_sha256",
        )
        .map_err(storage_db_error)?;
    let stored = statement
        .query_map(
            params![interaction_state_id, expected_state_revision],
            |row| row.get::<_, String>(0),
        )
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    if stored != expected_review_sha256s {
        return Err(storage_corrupted(
            "interaction event proposal evidence differs from durable proposals",
        ));
    }
    Ok(())
}

pub(super) fn validate_stored_event_checkpoint_evidence(
    connection: &Connection,
    interaction_state_id: &str,
    resulting_state_revision: i64,
    owner_message_id: Option<&str>,
    resulting_state_snapshot_sha256: &str,
) -> CoreResult<()> {
    let stored = connection
        .query_row(
            "SELECT message_id, checkpoint_sha256
             FROM interaction_state_checkpoints
             WHERE source_interaction_state_id = ?1
               AND state_revision = ?2",
            params![interaction_state_id, resulting_state_revision],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(storage_db_error)?;
    match (owner_message_id, stored) {
        (Some(expected_message_id), Some((stored_message_id, stored_sha256)))
            if stored_message_id == expected_message_id
                && stored_sha256 == resulting_state_snapshot_sha256 =>
        {
            Ok(())
        }
        (None, None) => Ok(()),
        (Some(_), _) => Err(storage_corrupted(
            "message-owned interaction event checkpoint evidence is missing or invalid",
        )),
        (None, Some(_)) => Err(storage_corrupted(
            "non-message interaction event unexpectedly owns a state checkpoint",
        )),
    }
}
pub(super) fn decode_interaction_policy(
    module_plan_sha256: &str,
    policy_json: &str,
    policy_sha256: &str,
) -> CoreResult<InteractionPolicySnapshot> {
    if !is_sha256(module_plan_sha256) || !is_sha256(policy_sha256) {
        return Err(storage_corrupted(
            "stored interaction policy hashes are invalid",
        ));
    }
    let policy: InteractionPolicySnapshot = decode_json(
        "stored interaction policy",
        policy_json,
        MAX_EVENT_JSON_BYTES,
    )?;
    validate_policy_shape(&policy).map_err(|error| {
        storage_corrupted(format!("stored interaction policy is invalid: {error}"))
    })?;
    if stored_module_plan_sha256(&policy) != module_plan_sha256
        || interaction_policy_sha256(&policy)? != policy_sha256
    {
        return Err(storage_corrupted(
            "stored interaction policy fingerprint does not match its payload",
        ));
    }
    Ok(policy)
}

pub(super) fn validate_stored_event_evaluation_authority(
    policy_sha256: &str,
    evaluation_seal_json: Option<&str>,
    evaluation_seal_sha256: Option<&str>,
    evaluation_seal_version: i64,
    payload: &StoredEventPayload,
) -> CoreResult<()> {
    match (
        evaluation_seal_version,
        evaluation_seal_json,
        evaluation_seal_sha256,
        payload.evaluation_seal_sha256.as_ref(),
        payload.deterministic_seed,
    ) {
        (0, None, None, None, None) => Ok(()),
        (1, Some(seal_json), Some(seal_sha256), Some(payload_sha256), Some(_)) => {
            let seal: InteractionEvaluationSeal = decode_json(
                "stored interaction evaluation seal",
                seal_json,
                MAX_STATE_JSON_BYTES,
            )?;
            let canonical_json = encode_json(
                "stored interaction evaluation seal",
                &seal,
                MAX_STATE_JSON_BYTES,
            )?;
            let verified_sha256 = interaction_evaluation_seal_sha256(&seal)?;
            let policy_sha256 = Sha256Digest::parse(policy_sha256).map_err(CoreError::invalid)?;
            if canonical_json != seal_json
                || verified_sha256.as_str() != seal_sha256
                || &verified_sha256 != payload_sha256
                || seal.policy_sha256 != policy_sha256
            {
                return Err(storage_corrupted(
                    "stored interaction evaluation authority is inconsistent",
                ));
            }
            Ok(())
        }
        _ => Err(storage_corrupted(
            "stored interaction evaluation authority is incomplete",
        )),
    }
}
