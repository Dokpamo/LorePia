use chrono::{DateTime, Utc};
use lorepia_domain::{
    ConversationBranchId, ConversationId, CoreError, CoreResult, InteractionEffect,
    InteractionEvent, InteractionRuleSetId,
};
use rusqlite::{Connection, Transaction, TransactionBehavior, params};

use super::event_transactions::{
    InteractionEventTransitionWrite, event_commit_sha256, event_id_or_idempotency_exists,
    stored_event_payload, validate_event_commit, write_event_transition,
};
use super::{
    InteractionChoiceExpirationCommit, InteractionChoiceSelectionCommit,
    InteractionChoiceSelectionReceipt, InteractionEventCommit, InteractionPolicyRuleSetRevision,
    InteractionPolicySnapshot, InteractionStateKey, MAX_EVENT_JSON_BYTES, Storage,
    StoredInteractionEffect, StoredInteractionEffectHistory, StoredInteractionEvent,
    decode_choice_effect_lifecycle, decode_interaction_policy, decode_json, effect_outbox_kind,
    encode_json, i64_from_u64, interaction_effect_id, interaction_policy_sha256, not_found,
    parse_datetime, read_effect_history, read_pending_effects, read_state_by_id,
    require_pending_choice, require_pending_choice_effect, require_state_revision,
    revision_conflict, storage_corrupted, storage_db_error, u64_from_i64,
    validate_effect_delivery_token, validate_effect_poll_limit,
    validate_existing_proposals_unchanged, validate_interaction_policy_revisions,
    validate_nonempty_id, validate_normalized_state, validate_stored_effect_identity,
};

impl Storage {
    /// Consumes one exact durable choice and atomically saves the
    /// storage-derived `UserAction(stored_choice_id)` transition.
    ///
    /// The caller cannot provide an event kind, action name, or action
    /// arguments. A consumed or expired choice cannot be selected again.
    pub fn consume_interaction_choice(
        &self,
        commit: &InteractionChoiceSelectionCommit,
    ) -> CoreResult<InteractionChoiceSelectionReceipt> {
        validate_nonempty_id("interaction effect id", &commit.effect_id)?;
        validate_nonempty_id("interaction choice id", &commit.choice_id)?;
        if commit.selected_at_epoch_seconds < 0 {
            return Err(CoreError::invalid(
                "interaction choice selection timestamp must be non-negative",
            ));
        }

        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let choice_effect = read_effect_history(&transaction, &commit.effect_id)?
            .ok_or_else(|| not_found("interaction effect"))?;
        require_pending_choice(
            &choice_effect,
            &commit.choice_id,
            commit.selected_at_epoch_seconds,
        )?;
        validate_interaction_policy_revisions(&transaction, &commit.current_policy)?;
        if choice_effect.stored.policy != commit.current_policy
            || commit.derived.policy != commit.current_policy
        {
            return Err(revision_conflict(
                "interaction choice policy changed after presentation",
            ));
        }

        let current = read_state_by_id(&transaction, &choice_effect.stored.interaction_state_id)?
            .ok_or_else(|| storage_corrupted("choice interaction state is missing"))?;
        validate_normalized_state(&transaction, &current)?;
        require_state_revision(&current, commit.expected_state_revision)?;

        let key = InteractionStateKey {
            state_id: current.id.clone(),
            conversation_id: current.conversation_id.clone(),
            branch_id: current.branch_id.clone(),
        };
        let event = InteractionEvent::UserAction {
            action_id: commit.choice_id.clone(),
        };
        let ordinary = InteractionEventCommit {
            event_id: commit.derived.event_id.clone(),
            idempotency_key: commit.derived.idempotency_key.clone(),
            key: key.clone(),
            expected_state_revision: commit.expected_state_revision,
            event: event.clone(),
            generation_attempt_id: None,
            owner_message_id: None,
            policy: commit.derived.policy.clone(),
            evaluation_seal: commit.derived.evaluation_seal.clone(),
            deterministic_seed: commit.derived.deterministic_seed,
            next_state: commit.derived.next_state.clone(),
            knowledge: commit.derived.knowledge.clone(),
            action_results: commit.derived.action_results.clone(),
            effects: commit.derived.effects.clone(),
            derived_events: commit.derived.derived_events.clone(),
            proposals: commit.derived.proposals.clone(),
            created_at: commit.derived.created_at,
        };
        validate_event_commit(&ordinary)?;
        validate_existing_proposals_unchanged(
            &transaction,
            &current.id,
            &current.state,
            &commit.derived.next_state,
            &commit.derived.proposals,
        )?;
        if event_id_or_idempotency_exists(
            &transaction,
            &commit.derived.event_id,
            &commit.derived.idempotency_key,
        )? {
            return Err(revision_conflict(
                "interaction choice derived event was already committed",
            ));
        }
        let fingerprint = event_commit_sha256(&ordinary)?;
        let event_payload = stored_event_payload(&ordinary, fingerprint)?;
        let payload_json = encode_json(
            "interaction event payload",
            &event_payload,
            MAX_EVENT_JSON_BYTES,
        )?;
        write_event_transition(
            &transaction,
            InteractionEventTransitionWrite {
                key: &key,
                expected_state_revision: commit.expected_state_revision,
                event: &event,
                generation_attempt_id: None,
                proposal_namespace_generation_id: None,
                owner_message_id: None,
                policy: &commit.derived.policy,
                evaluation_seal: commit.derived.evaluation_seal.as_ref(),
                deterministic_seed: commit.derived.deterministic_seed,
                next_state: &commit.derived.next_state,
                knowledge: &commit.derived.knowledge,
                action_results: &commit.derived.action_results,
                effects: &commit.derived.effects,
                derived_events: &commit.derived.derived_events,
                proposals: &commit.derived.proposals,
                event_id: &commit.derived.event_id,
                idempotency_key: &commit.derived.idempotency_key,
                payload_json: &payload_json,
                created_at: commit.derived.created_at,
                generation_append_materialization: false,
                derived_chain_parent: None,
            },
        )?;
        let changed = transaction
            .execute(
                "UPDATE interaction_effect_outbox
                 SET choice_status = 'consumed', choice_id = ?1,
                     choice_decided_at_epoch_seconds = ?2
                 WHERE effect_id = ?3 AND effect_kind = 'choices_presented'
                   AND choice_status = 'pending' AND choice_id IS NULL
                   AND choice_decided_at_epoch_seconds IS NULL",
                params![
                    commit.choice_id,
                    commit.selected_at_epoch_seconds,
                    commit.effect_id,
                ],
            )
            .map_err(storage_db_error)?;
        if changed != 1 {
            return Err(revision_conflict(
                "interaction choice was already consumed or expired",
            ));
        }
        transaction.commit().map_err(storage_db_error)?;

        let consumed = read_effect_history(&connection, &commit.effect_id)?
            .ok_or_else(|| storage_corrupted("consumed interaction choice is missing"))?;
        let event = StoredInteractionEvent {
            event_id: commit.derived.event_id.clone(),
            idempotency_key: commit.derived.idempotency_key.clone(),
            interaction_state_id: key.state_id,
            expected_state_revision: commit.expected_state_revision,
            resulting_state_revision: commit.derived.next_state.revision,
            exact_replay: false,
            generation_attempt_id: None,
            owner_message_id: None,
            commit_sha256: event_payload.commit_sha256,
            resulting_state_snapshot_sha256: event_payload.resulting_state_snapshot_sha256,
            proposal_review_sha256s: event_payload.proposal_review_sha256s,
            policy: commit.derived.policy.clone(),
            policy_sha256: interaction_policy_sha256(&commit.derived.policy)?,
            created_at: commit.derived.created_at,
        };
        Ok(InteractionChoiceSelectionReceipt {
            choice_effect: consumed,
            resulting_state_revision: event.resulting_state_revision,
            event,
        })
    }

    /// Expires one pending choice without modifying interaction state.
    ///
    /// Selection and expiration race through the same pending-status CAS, so
    /// exactly one transition can win.
    pub fn expire_interaction_choice(
        &self,
        commit: &InteractionChoiceExpirationCommit,
    ) -> CoreResult<StoredInteractionEffectHistory> {
        validate_nonempty_id("interaction effect id", &commit.effect_id)?;
        if commit.expired_at_epoch_seconds < 0 {
            return Err(CoreError::invalid(
                "interaction choice expiration timestamp must be non-negative",
            ));
        }
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let choice_effect = read_effect_history(&transaction, &commit.effect_id)?
            .ok_or_else(|| not_found("interaction effect"))?;
        require_pending_choice_effect(&choice_effect, commit.expired_at_epoch_seconds)?;
        let changed = transaction
            .execute(
                "UPDATE interaction_effect_outbox
                 SET choice_status = 'expired',
                     choice_decided_at_epoch_seconds = ?1
                 WHERE effect_id = ?2 AND effect_kind = 'choices_presented'
                   AND choice_status = 'pending' AND choice_id IS NULL
                   AND choice_decided_at_epoch_seconds IS NULL",
                params![commit.expired_at_epoch_seconds, commit.effect_id],
            )
            .map_err(storage_db_error)?;
        if changed != 1 {
            return Err(revision_conflict(
                "interaction choice was already consumed or expired",
            ));
        }
        transaction.commit().map_err(storage_db_error)?;
        read_effect_history(&connection, &commit.effect_id)?
            .ok_or_else(|| storage_corrupted("expired interaction choice is missing"))
    }

    /// Lists due effects without claiming them. Results are bounded and use
    /// the same stable order as the claiming API.
    pub fn list_pending_interaction_effects(
        &self,
        now: DateTime<Utc>,
        limit: u32,
    ) -> CoreResult<Vec<StoredInteractionEffect>> {
        validate_effect_poll_limit(limit)?;
        let connection = self.connection()?;
        read_pending_effects(&connection, now, limit)
    }

    /// Claims pending interaction effects with a lease expressed through
    /// `available_at`. A crashed dispatcher naturally makes them claimable
    /// again once the lease expires.
    pub fn claim_pending_interaction_effects(
        &self,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        limit: u32,
    ) -> CoreResult<Vec<StoredInteractionEffect>> {
        validate_effect_poll_limit(limit)?;
        if lease_until <= now {
            return Err(CoreError::invalid(
                "interaction effect lease must end after the claim time",
            ));
        }
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let candidates = {
            let mut statement = transaction
                .prepare(
                    "SELECT effect.effect_id, effect.event_id, effect.sequence,
                            effect.effect_kind, effect.effect_json,
                            effect.available_at, effect.delivery_attempts,
                            effect.delivered_at, effect.choice_status,
                            effect.choice_id,
                            effect.choice_decided_at_epoch_seconds,
                            event.interaction_state_id, event.conversation_id,
                            event.branch_id, event.resulting_state_revision,
                            event.created_at, event.module_plan_sha256,
                            event.policy_json, event.policy_sha256
                     FROM interaction_effect_outbox AS effect
                     JOIN interaction_events AS event
                       ON event.id = effect.event_id
                     WHERE effect.delivered_at IS NULL
                       AND effect.available_at <= ?1
                     ORDER BY effect.available_at, effect.event_id,
                              effect.sequence
                     LIMIT ?2",
                )
                .map_err(storage_db_error)?;
            statement
                .query_map(params![now.to_rfc3339(), i64::from(limit)], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, Option<String>>(8)?,
                        row.get::<_, Option<String>>(9)?,
                        row.get::<_, Option<i64>>(10)?,
                        row.get::<_, String>(11)?,
                        row.get::<_, String>(12)?,
                        row.get::<_, String>(13)?,
                        row.get::<_, i64>(14)?,
                        row.get::<_, String>(15)?,
                        row.get::<_, String>(16)?,
                        row.get::<_, String>(17)?,
                        row.get::<_, String>(18)?,
                    ))
                })
                .map_err(storage_db_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage_db_error)?
        };
        let mut claimed = Vec::with_capacity(candidates.len());
        for (
            effect_id,
            event_id,
            sequence,
            effect_kind,
            effect_json,
            _available_at,
            attempts,
            delivered_at,
            choice_status,
            selected_choice_id,
            choice_decided_at,
            interaction_state_id,
            conversation_id,
            branch_id,
            resulting_state_revision,
            event_created_at,
            module_plan_sha256,
            policy_json,
            policy_sha256,
        ) in candidates
        {
            if attempts == i64::MAX {
                return Err(storage_corrupted(
                    "interaction effect delivery attempt count is exhausted",
                ));
            }
            let changed = transaction
                .execute(
                    "UPDATE interaction_effect_outbox
                     SET delivery_attempts = delivery_attempts + 1,
                         available_at = ?1
                     WHERE event_id = ?2 AND sequence = ?3
                       AND delivery_attempts = ?4
                       AND delivered_at IS NULL AND available_at <= ?5",
                    params![
                        lease_until.to_rfc3339(),
                        event_id,
                        sequence,
                        attempts,
                        now.to_rfc3339(),
                    ],
                )
                .map_err(storage_db_error)?;
            if changed != 1 {
                continue;
            }
            let effect: InteractionEffect = decode_json(
                "stored interaction effect",
                &effect_json,
                MAX_EVENT_JSON_BYTES,
            )?;
            if effect_outbox_kind(&effect) != Some(effect_kind.as_str()) {
                return Err(storage_corrupted(
                    "interaction effect kind differs from its stored payload",
                ));
            }
            validate_stored_effect_identity(&effect_id, &event_id, sequence)?;
            let choice_status = decode_choice_effect_lifecycle(
                &effect,
                choice_status.as_deref(),
                selected_choice_id.as_deref(),
                choice_decided_at,
            )?;
            let policy =
                decode_interaction_policy(&module_plan_sha256, &policy_json, &policy_sha256)?;
            validate_stored_interaction_policy_rule_sets(&transaction, &event_id, &policy)?;
            claimed.push(StoredInteractionEffect {
                effect_id,
                event_id,
                sequence: u64_from_i64("interaction effect sequence", sequence)?,
                interaction_state_id,
                conversation_id: ConversationId(conversation_id),
                branch_id: ConversationBranchId(branch_id),
                resulting_state_revision: u64_from_i64(
                    "interaction effect resulting state revision",
                    resulting_state_revision,
                )?,
                event_created_at: parse_datetime(
                    "interaction effect event created_at",
                    &event_created_at,
                )?,
                policy,
                policy_sha256,
                effect,
                available_at: lease_until,
                delivery_attempts: u64_from_i64("interaction effect attempts", attempts)?
                    .checked_add(1)
                    .ok_or_else(|| {
                        storage_corrupted("interaction effect attempt count overflowed")
                    })?,
                delivered_at: delivered_at
                    .map(|value| parse_datetime("interaction effect delivered_at", &value))
                    .transpose()?,
                choice_status,
                selected_choice_id,
                choice_decided_at_epoch_seconds: choice_decided_at,
            });
        }
        transaction.commit().map_err(storage_db_error)?;
        Ok(claimed)
    }

    /// Acknowledges one claimed UI effect using its attempt count as a CAS
    /// token. Stale workers cannot acknowledge a newer lease.
    pub fn mark_interaction_effect_delivered(
        &self,
        event_id: &str,
        sequence: u64,
        expected_delivery_attempts: u64,
        delivered_at: DateTime<Utc>,
    ) -> CoreResult<()> {
        validate_effect_delivery_token(event_id, sequence, expected_delivery_attempts)?;
        let connection = self.connection()?;
        let changed = connection
            .execute(
                "UPDATE interaction_effect_outbox
                 SET delivered_at = ?1
                 WHERE event_id = ?2 AND sequence = ?3
                   AND delivery_attempts = ?4 AND delivered_at IS NULL",
                params![
                    delivered_at.to_rfc3339(),
                    event_id,
                    i64_from_u64("interaction effect sequence", sequence)?,
                    i64_from_u64(
                        "interaction effect delivery attempts",
                        expected_delivery_attempts,
                    )?,
                ],
            )
            .map_err(storage_db_error)?;
        if changed != 1 {
            return Err(revision_conflict(
                "interaction effect delivery compare-and-swap failed",
            ));
        }
        Ok(())
    }

    /// Releases one claimed effect for a later retry without losing its
    /// durable attempt count.
    pub fn retry_interaction_effect_after(
        &self,
        event_id: &str,
        sequence: u64,
        expected_delivery_attempts: u64,
        available_at: DateTime<Utc>,
    ) -> CoreResult<()> {
        validate_effect_delivery_token(event_id, sequence, expected_delivery_attempts)?;
        let connection = self.connection()?;
        let changed = connection
            .execute(
                "UPDATE interaction_effect_outbox
                 SET available_at = ?1
                 WHERE event_id = ?2 AND sequence = ?3
                   AND delivery_attempts = ?4 AND delivered_at IS NULL",
                params![
                    available_at.to_rfc3339(),
                    event_id,
                    i64_from_u64("interaction effect sequence", sequence)?,
                    i64_from_u64(
                        "interaction effect delivery attempts",
                        expected_delivery_attempts,
                    )?,
                ],
            )
            .map_err(storage_db_error)?;
        if changed != 1 {
            return Err(revision_conflict(
                "interaction effect retry compare-and-swap failed",
            ));
        }
        Ok(())
    }
}

pub(super) fn validate_stored_interaction_policy_rule_sets(
    connection: &Connection,
    event_id: &str,
    policy: &InteractionPolicySnapshot,
) -> CoreResult<()> {
    let mut statement = connection
        .prepare(
            "SELECT rule_set_id, rule_set_revision_id, revision_sha256
             FROM interaction_event_policy_rule_sets
             WHERE event_id = ?1
             ORDER BY ordinal",
        )
        .map_err(storage_db_error)?;
    let stored = statement
        .query_map([event_id], |row| {
            Ok(InteractionPolicyRuleSetRevision {
                rule_set_id: InteractionRuleSetId::from(row.get::<_, String>(0)?),
                revision_id: row.get(1)?,
                sha256: row.get(2)?,
            })
        })
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    if stored != policy.rule_sets {
        return Err(storage_corrupted(
            "normalized interaction event policy differs from its payload",
        ));
    }
    Ok(())
}

pub(super) fn write_effect_outbox(
    transaction: &Transaction<'_>,
    event_id: &str,
    effects: &[InteractionEffect],
    created_at: DateTime<Utc>,
) -> CoreResult<()> {
    let mut sequence = 0_u64;
    for effect in effects {
        let Some(kind) = effect_outbox_kind(effect) else {
            continue;
        };
        sequence = sequence
            .checked_add(1)
            .ok_or_else(|| CoreError::invalid("interaction effect sequence overflowed"))?;
        let effect_json = encode_json("interaction effect", effect, MAX_EVENT_JSON_BYTES)?;
        let sequence_i64 = i64_from_u64("interaction effect sequence", sequence)?;
        let effect_id = interaction_effect_id(event_id, sequence_i64);
        let choice_status =
            matches!(effect, InteractionEffect::ChoicesPresented { .. }).then_some("pending");
        transaction
            .execute(
                "INSERT INTO interaction_effect_outbox
                 (event_id, sequence, effect_id, effect_kind, effect_json,
                  available_at, delivery_attempts, delivered_at, choice_status,
                  choice_id, choice_decided_at_epoch_seconds)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, NULL, ?7, NULL, NULL)",
                params![
                    event_id,
                    sequence_i64,
                    effect_id,
                    kind,
                    effect_json,
                    created_at.to_rfc3339(),
                    choice_status,
                ],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}
