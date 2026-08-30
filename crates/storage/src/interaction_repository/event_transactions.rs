use super::effects::{validate_stored_interaction_policy_rule_sets, write_effect_outbox};
use super::{
    DerivedChainParent, DerivedEventOutboxWrite, InteractionActionResultWrite,
    InteractionDerivedEventCommit, InteractionDerivedEventWrite, InteractionEvaluationSeal,
    InteractionEventCommit, InteractionKnowledgeBinding, InteractionPolicySnapshot,
    InteractionProposalWrite, InteractionStateKey, MAX_ACTION_RESULTS_PER_EVENT,
    MAX_AUDIT_JSON_BYTES, MAX_EFFECTS_PER_EVENT, MAX_EVENT_JSON_BYTES, MAX_STATE_JSON_BYTES,
    Storage, StoredInteractionEvent, decode_interaction_policy, decode_json, encode_json,
    i64_from_u64, interaction_evaluation_seal_sha256, interaction_policy_sha256,
    interaction_state_snapshot_sha256, is_sha256, parse_datetime, replace_normalized_state,
    require_no_pending_derived_predecessor, require_state_for_key, require_state_revision,
    revision_conflict, sha256_hex, storage_corrupted, storage_db_error, u64_from_i64,
    validate_action_result_sources, validate_action_results_belong_to_policy,
    validate_derived_event_writes, validate_existing_proposals_unchanged,
    validate_generation_attempt_binding, validate_interaction_policy_revisions, validate_key,
    validate_knowledge_bindings, validate_nonempty_id, validate_proposal_writes, validate_state,
    validate_stored_event_checkpoint_evidence, validate_stored_event_evaluation_authority,
    validate_stored_event_proposal_evidence, write_action_results, write_derived_event_outbox,
    write_interaction_policy_rule_sets, write_interaction_state_checkpoint, write_new_proposals,
    write_state_document_only,
};
use chrono::{DateTime, Utc};
use lorepia_domain::{
    CoreError, CoreErrorCode, CoreResult, GenerationId, InteractionEffect, InteractionEvent,
    InteractionState, MAX_INTERACTION_PROPOSALS, MessageId, Sha256Digest,
    validate_interaction_effect_native_text, validate_interaction_native_text,
};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
#[derive(Serialize)]
struct EventFingerprint<'a> {
    schema_version: u32,
    event_id: &'a str,
    idempotency_key: &'a str,
    key: &'a InteractionStateKey,
    expected_state_revision: u64,
    event: &'a InteractionEvent,
    generation_attempt_id: Option<&'a GenerationId>,
    owner_message_id: Option<&'a MessageId>,
    policy: &'a InteractionPolicySnapshot,
    #[serde(skip_serializing_if = "Option::is_none")]
    evaluation_seal_sha256: Option<&'a Sha256Digest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deterministic_seed: Option<u64>,
    next_state: &'a InteractionState,
    knowledge: &'a [InteractionKnowledgeBinding],
    action_results: &'a [InteractionActionResultWrite],
    effects: &'a [InteractionEffect],
    #[serde(skip_serializing_if = "<[InteractionDerivedEventWrite]>::is_empty")]
    derived_events: &'a [InteractionDerivedEventWrite],
    proposals: &'a [InteractionProposalWrite],
    created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StoredEventPayload {
    schema_version: u32,
    pub(super) commit_sha256: String,
    pub(super) resulting_state_snapshot_sha256: String,
    pub(super) proposal_review_sha256s: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) evaluation_seal_sha256: Option<Sha256Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) deterministic_seed: Option<u64>,
}

impl Storage {
    /// Commits an evaluated event, its state CAS, normalized state, action
    /// results, UI-effect outbox, proposal records, and audit rows atomically.
    ///
    /// Reusing an idempotency key returns `exact_replay = true` only when the
    /// entire commit fingerprint is identical. Any hash conflict is rejected.
    pub fn commit_interaction_event(
        &self,
        commit: &InteractionEventCommit,
    ) -> CoreResult<StoredInteractionEvent> {
        validate_event_commit(commit)?;
        let fingerprint = event_commit_sha256(commit)?;
        let event_payload = stored_event_payload(commit, fingerprint)?;
        let payload_json = encode_json(
            "interaction event payload",
            &event_payload,
            MAX_EVENT_JSON_BYTES,
        )?;

        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;

        if let Some(replay) =
            read_event_by_idempotency_key(&transaction, &commit.idempotency_key, &payload_json)?
        {
            transaction.commit().map_err(storage_db_error)?;
            return Ok(replay);
        }
        if event_id_exists(&transaction, &commit.event_id)? {
            return Err(revision_conflict(
                "interaction event id is already committed under another idempotency key",
            ));
        }
        require_no_pending_derived_predecessor(&transaction, &commit.key)?;

        let current = require_state_for_key(&transaction, &commit.key)?;
        require_state_revision(&current, commit.expected_state_revision)?;
        validate_existing_proposals_unchanged(
            &transaction,
            &current.id,
            &current.state,
            &commit.next_state,
            &commit.proposals,
        )?;

        write_event_transition(
            &transaction,
            InteractionEventTransitionWrite {
                key: &commit.key,
                expected_state_revision: commit.expected_state_revision,
                event: &commit.event,
                generation_attempt_id: commit.generation_attempt_id.as_ref(),
                proposal_namespace_generation_id: None,
                owner_message_id: commit.owner_message_id.as_ref(),
                policy: &commit.policy,
                evaluation_seal: commit.evaluation_seal.as_ref(),
                deterministic_seed: commit.deterministic_seed,
                next_state: &commit.next_state,
                knowledge: &commit.knowledge,
                action_results: &commit.action_results,
                effects: &commit.effects,
                derived_events: &commit.derived_events,
                proposals: &commit.proposals,
                event_id: &commit.event_id,
                idempotency_key: &commit.idempotency_key,
                payload_json: &payload_json,
                created_at: commit.created_at,
                generation_append_materialization: false,
                derived_chain_parent: None,
            },
        )?;

        transaction.commit().map_err(storage_db_error)?;
        Ok(StoredInteractionEvent {
            event_id: commit.event_id.clone(),
            idempotency_key: commit.idempotency_key.clone(),
            interaction_state_id: commit.key.state_id.clone(),
            expected_state_revision: commit.expected_state_revision,
            resulting_state_revision: commit.next_state.revision,
            exact_replay: false,
            generation_attempt_id: commit.generation_attempt_id.clone(),
            owner_message_id: commit.owner_message_id.clone(),
            commit_sha256: event_payload.commit_sha256,
            resulting_state_snapshot_sha256: event_payload.resulting_state_snapshot_sha256,
            proposal_review_sha256s: event_payload.proposal_review_sha256s,
            policy: commit.policy.clone(),
            policy_sha256: interaction_policy_sha256(&commit.policy)?,
            created_at: commit.created_at,
        })
    }
}

pub(super) struct InteractionEventTransitionWrite<'a> {
    pub(super) key: &'a InteractionStateKey,
    pub(super) expected_state_revision: u64,
    pub(super) event: &'a InteractionEvent,
    pub(super) generation_attempt_id: Option<&'a GenerationId>,
    pub(super) proposal_namespace_generation_id: Option<&'a GenerationId>,
    pub(super) owner_message_id: Option<&'a MessageId>,
    pub(super) policy: &'a InteractionPolicySnapshot,
    pub(super) evaluation_seal: Option<&'a InteractionEvaluationSeal>,
    pub(super) deterministic_seed: Option<u64>,
    pub(super) next_state: &'a InteractionState,
    pub(super) knowledge: &'a [InteractionKnowledgeBinding],
    pub(super) action_results: &'a [InteractionActionResultWrite],
    pub(super) effects: &'a [InteractionEffect],
    pub(super) derived_events: &'a [InteractionDerivedEventWrite],
    pub(super) proposals: &'a [InteractionProposalWrite],
    pub(super) event_id: &'a str,
    pub(super) idempotency_key: &'a str,
    pub(super) payload_json: &'a str,
    pub(super) created_at: DateTime<Utc>,
    pub(super) generation_append_materialization: bool,
    pub(super) derived_chain_parent: Option<DerivedChainParent<'a>>,
}

pub(super) fn write_event_transition(
    transaction: &Transaction<'_>,
    write: InteractionEventTransitionWrite<'_>,
) -> CoreResult<()> {
    let InteractionEventTransitionWrite {
        key,
        expected_state_revision,
        event,
        generation_attempt_id,
        proposal_namespace_generation_id,
        owner_message_id,
        policy,
        evaluation_seal,
        deterministic_seed,
        next_state,
        knowledge,
        action_results,
        effects,
        derived_events,
        proposals,
        event_id,
        idempotency_key,
        payload_json,
        created_at,
        generation_append_materialization,
        derived_chain_parent,
    } = write;
    let resulting_revision = expected_state_revision
        .checked_add(1)
        .ok_or_else(|| CoreError::invalid("interaction state revision overflowed"))?;
    if next_state.revision != resulting_revision {
        return Err(CoreError::invalid(format!(
            "interaction event next-state revision must be {resulting_revision}"
        )));
    }
    validate_state(next_state)?;
    validate_knowledge_bindings(next_state, knowledge)?;
    validate_generation_attempt_binding(
        transaction,
        key,
        event,
        generation_attempt_id,
        generation_append_materialization,
    )?;
    validate_interaction_policy_revisions(transaction, policy)?;
    validate_action_results_belong_to_policy(action_results, policy)?;
    validate_derived_event_writes(transaction, policy, action_results, effects, derived_events)?;
    validate_proposal_writes(
        transaction,
        expected_state_revision,
        next_state,
        effects,
        action_results,
        proposals,
        proposal_namespace_generation_id,
        None,
    )?;
    validate_action_result_sources(transaction, event, action_results)?;

    write_state_document_only(
        transaction,
        &key.state_id,
        expected_state_revision,
        next_state,
        created_at,
    )?;
    replace_normalized_state(
        transaction,
        &key.state_id,
        next_state,
        knowledge,
        created_at,
    )?;

    let event_argument_json = interaction_event_argument_json(event)?;
    let (module_plan_sha256, policy_json, policy_sha256) = encode_interaction_policy(policy)?;
    let (evaluation_seal_json, evaluation_seal_sha256, evaluation_seal_version) =
        encode_interaction_evaluation_authority(policy, evaluation_seal, deterministic_seed)?;
    transaction
        .execute(
            "INSERT INTO interaction_events
             (id, idempotency_key, interaction_state_id,
              expected_state_revision, resulting_state_revision,
              conversation_id, branch_id, event_kind, event_argument_json,
              module_plan_sha256, policy_json, policy_sha256,
              payload_json, created_at, generation_attempt_id,
              evaluation_seal_json, evaluation_seal_sha256,
              evaluation_seal_version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                     ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            params![
                event_id,
                idempotency_key,
                key.state_id,
                i64_from_u64(
                    "expected interaction state revision",
                    expected_state_revision
                )?,
                i64_from_u64("resulting interaction state revision", next_state.revision)?,
                key.conversation_id.0.as_str(),
                key.branch_id.0.as_str(),
                interaction_event_kind(event),
                event_argument_json,
                module_plan_sha256,
                policy_json,
                policy_sha256,
                payload_json,
                created_at.to_rfc3339(),
                generation_attempt_id.map(|id| id.0.as_str()),
                evaluation_seal_json,
                evaluation_seal_sha256,
                evaluation_seal_version,
            ],
        )
        .map_err(storage_db_error)?;

    write_interaction_policy_rule_sets(transaction, event_id, policy)?;
    write_action_results(transaction, event_id, action_results, created_at)?;
    write_effect_outbox(transaction, event_id, effects, created_at)?;
    write_new_proposals(transaction, &key.state_id, proposals, next_state.revision)?;
    if !generation_append_materialization {
        write_derived_event_outbox(
            transaction,
            &DerivedEventOutboxWrite {
                key,
                event,
                policy,
                evaluation_seal,
                deterministic_seed,
                effects,
                derived_events,
                event_id,
                parent_resulting_state_revision: next_state.revision,
                payload_json,
                created_at,
                chain_parent: derived_chain_parent,
            },
        )?;
    }
    if let Some(message_id) = owner_message_id {
        write_interaction_state_checkpoint(
            transaction,
            key,
            message_id,
            next_state,
            knowledge,
            created_at,
        )?;
    }
    Ok(())
}

fn read_event_by_idempotency_key(
    transaction: &Transaction<'_>,
    idempotency_key: &str,
    expected_payload_json: &str,
) -> CoreResult<Option<StoredInteractionEvent>> {
    let raw = transaction
        .query_row(
            "SELECT event.id, event.idempotency_key,
                    event.interaction_state_id,
                    event.expected_state_revision,
                    event.resulting_state_revision,
                    event.payload_json, event.created_at,
                    event.module_plan_sha256, event.policy_json,
                    event.policy_sha256, event.generation_attempt_id,
                    checkpoint.message_id, event.evaluation_seal_json,
                    event.evaluation_seal_sha256, event.evaluation_seal_version
             FROM interaction_events AS event
             LEFT JOIN interaction_state_checkpoints AS checkpoint
               ON checkpoint.source_interaction_state_id =
                    event.interaction_state_id
              AND checkpoint.state_revision =
                    event.resulting_state_revision
             WHERE event.idempotency_key = ?1",
            [idempotency_key],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, Option<String>>(12)?,
                    row.get::<_, Option<String>>(13)?,
                    row.get::<_, i64>(14)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?;
    raw.map(
        |(
            event_id,
            stored_key,
            state_id,
            expected_revision,
            resulting_revision,
            payload_json,
            created_at,
            module_plan_sha256,
            policy_json,
            policy_sha256,
            generation_attempt_id,
            owner_message_id,
            evaluation_seal_json,
            evaluation_seal_sha256,
            evaluation_seal_version,
        )| {
            if payload_json != expected_payload_json {
                return Err(CoreError::new(
                    CoreErrorCode::InvalidInput,
                    "interaction event idempotency key was reused with a different commit",
                    false,
                ));
            }
            let policy =
                decode_interaction_policy(&module_plan_sha256, &policy_json, &policy_sha256)?;
            validate_stored_interaction_policy_rule_sets(transaction, &event_id, &policy)?;
            let event_payload = decode_stored_event_payload(&payload_json)?;
            validate_stored_event_evaluation_authority(
                &policy_sha256,
                evaluation_seal_json.as_deref(),
                evaluation_seal_sha256.as_deref(),
                evaluation_seal_version,
                &event_payload,
            )?;
            validate_stored_event_proposal_evidence(
                transaction,
                &state_id,
                expected_revision,
                &event_payload.proposal_review_sha256s,
            )?;
            validate_stored_event_checkpoint_evidence(
                transaction,
                &state_id,
                resulting_revision,
                owner_message_id.as_deref(),
                &event_payload.resulting_state_snapshot_sha256,
            )?;
            Ok(StoredInteractionEvent {
                event_id,
                idempotency_key: stored_key,
                interaction_state_id: state_id,
                expected_state_revision: u64_from_i64(
                    "interaction event expected state revision",
                    expected_revision,
                )?,
                resulting_state_revision: u64_from_i64(
                    "interaction event resulting state revision",
                    resulting_revision,
                )?,
                exact_replay: true,
                generation_attempt_id: generation_attempt_id.map(GenerationId),
                owner_message_id: owner_message_id.map(MessageId),
                commit_sha256: event_payload.commit_sha256,
                resulting_state_snapshot_sha256: event_payload.resulting_state_snapshot_sha256,
                proposal_review_sha256s: event_payload.proposal_review_sha256s,
                policy,
                policy_sha256,
                created_at: parse_datetime("interaction event created_at", &created_at)?,
            })
        },
    )
    .transpose()
}

pub(super) fn event_id_or_idempotency_exists(
    transaction: &Transaction<'_>,
    event_id: &str,
    idempotency_key: &str,
) -> CoreResult<bool> {
    transaction
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM interaction_events
                 WHERE id = ?1 OR idempotency_key = ?2
             )",
            params![event_id, idempotency_key],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)
}

fn event_id_exists(transaction: &Transaction<'_>, event_id: &str) -> CoreResult<bool> {
    transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM interaction_events WHERE id = ?1)",
            [event_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)
}

pub(super) fn event_commit_sha256(commit: &InteractionEventCommit) -> CoreResult<String> {
    let evaluation_seal_sha256 = commit
        .evaluation_seal
        .as_ref()
        .map(interaction_evaluation_seal_sha256)
        .transpose()?;
    let fingerprint = EventFingerprint {
        schema_version: 1,
        event_id: &commit.event_id,
        idempotency_key: &commit.idempotency_key,
        key: &commit.key,
        expected_state_revision: commit.expected_state_revision,
        event: &commit.event,
        generation_attempt_id: commit.generation_attempt_id.as_ref(),
        owner_message_id: commit.owner_message_id.as_ref(),
        policy: &commit.policy,
        evaluation_seal_sha256: evaluation_seal_sha256.as_ref(),
        deterministic_seed: commit.deterministic_seed,
        next_state: &commit.next_state,
        knowledge: &commit.knowledge,
        action_results: &commit.action_results,
        effects: &commit.effects,
        derived_events: &commit.derived_events,
        proposals: &commit.proposals,
        created_at: commit.created_at,
    };
    let json = encode_json(
        "interaction event commit fingerprint",
        &fingerprint,
        MAX_STATE_JSON_BYTES,
    )?;
    Ok(sha256_hex(json.as_bytes()))
}

pub(super) fn stored_event_payload(
    commit: &InteractionEventCommit,
    commit_sha256: String,
) -> CoreResult<StoredEventPayload> {
    let mut proposal_review_sha256s = commit
        .proposals
        .iter()
        .map(|proposal| proposal.review_payload_sha256.clone())
        .collect::<Vec<_>>();
    proposal_review_sha256s.sort();
    let payload = StoredEventPayload {
        schema_version: 1,
        commit_sha256,
        resulting_state_snapshot_sha256: interaction_state_snapshot_sha256(
            &commit.next_state,
            &commit.knowledge,
        )?,
        proposal_review_sha256s,
        evaluation_seal_sha256: commit
            .evaluation_seal
            .as_ref()
            .map(interaction_evaluation_seal_sha256)
            .transpose()?,
        deterministic_seed: commit.deterministic_seed,
    };
    validate_stored_event_payload(&payload)?;
    Ok(payload)
}

pub(super) fn decode_stored_event_payload(payload_json: &str) -> CoreResult<StoredEventPayload> {
    let payload: StoredEventPayload = decode_json(
        "stored interaction event payload",
        payload_json,
        MAX_EVENT_JSON_BYTES,
    )?;
    validate_stored_event_payload(&payload).map_err(|error| {
        storage_corrupted(format!(
            "stored interaction event evidence is invalid: {error}"
        ))
    })?;
    Ok(payload)
}

fn validate_stored_event_payload(payload: &StoredEventPayload) -> CoreResult<()> {
    if payload.schema_version != 1
        || !is_sha256(&payload.commit_sha256)
        || !is_sha256(&payload.resulting_state_snapshot_sha256)
        || payload
            .proposal_review_sha256s
            .iter()
            .any(|sha256| !is_sha256(sha256))
        || !payload
            .proposal_review_sha256s
            .windows(2)
            .all(|window| window[0] <= window[1])
        || payload.evaluation_seal_sha256.is_some() != payload.deterministic_seed.is_some()
    {
        return Err(CoreError::invalid(
            "interaction event evidence fingerprint is invalid",
        ));
    }
    Ok(())
}

pub(super) fn validate_event_commit(commit: &InteractionEventCommit) -> CoreResult<()> {
    validate_key(&commit.key)?;
    validate_nonempty_id("interaction event id", &commit.event_id)?;
    validate_nonempty_id("interaction event idempotency key", &commit.idempotency_key)?;
    validate_policy_shape(&commit.policy)?;
    validate_event_evaluation_authority(
        &commit.policy,
        commit.evaluation_seal.as_ref(),
        commit.deterministic_seed,
        &commit.derived_events,
    )?;
    validate_event_generation_attempt_shape(&commit.event, commit.generation_attempt_id.as_ref())?;
    validate_event_owner_message_shape(&commit.event, commit.owner_message_id.as_ref())?;
    validate_state(&commit.next_state)?;
    validate_new_event_collections(&commit.action_results, &commit.effects, &commit.proposals)?;
    let expected_next = commit
        .expected_state_revision
        .checked_add(1)
        .ok_or_else(|| CoreError::invalid("interaction state revision overflowed"))?;
    if commit.next_state.revision != expected_next {
        return Err(CoreError::invalid(format!(
            "interaction event next-state revision must be {expected_next}"
        )));
    }
    validate_knowledge_bindings(&commit.next_state, &commit.knowledge)?;
    Ok(())
}

pub(super) fn validate_event_generation_attempt_shape(
    event: &InteractionEvent,
    generation_attempt_id: Option<&GenerationId>,
) -> CoreResult<()> {
    match (event, generation_attempt_id) {
        (
            InteractionEvent::BeforeGeneration | InteractionEvent::AfterGeneration,
            Some(generation_id),
        ) => validate_nonempty_id("interaction generation attempt id", &generation_id.0)?,
        (InteractionEvent::BeforeGeneration | InteractionEvent::AfterGeneration, None) => {
            return Err(CoreError::invalid(
                "generation interaction event requires an exact generation attempt",
            ));
        }
        (_, Some(_)) => {
            return Err(CoreError::invalid(
                "non-generation interaction event cannot bind a generation attempt",
            ));
        }
        (_, None) => {}
    }
    Ok(())
}

pub(super) fn validate_event_owner_message_shape(
    event: &InteractionEvent,
    owner_message_id: Option<&MessageId>,
) -> CoreResult<()> {
    match (event, owner_message_id) {
        (InteractionEvent::MessageCommitted, Some(message_id)) => {
            validate_nonempty_id("interaction owner message id", &message_id.0)
        }
        (InteractionEvent::MessageCommitted, None) => Err(CoreError::invalid(
            "message-committed interaction event requires its exact owner message",
        )),
        (_, Some(_)) => Err(CoreError::invalid(
            "non-message interaction event cannot bind an owner message",
        )),
        (_, None) => Ok(()),
    }
}

pub(super) fn validate_policy_shape(policy: &InteractionPolicySnapshot) -> CoreResult<()> {
    if policy.rule_sets.len() > 1_024 {
        return Err(CoreError::invalid(
            "interaction policy exceeds the rule-set limit",
        ));
    }
    if let Some(module_plan_sha256) = policy.module_plan_sha256.as_deref()
        && !is_sha256(module_plan_sha256)
    {
        return Err(CoreError::invalid(
            "interaction module plan hash must be lowercase SHA-256",
        ));
    }
    let mut rule_set_ids = BTreeSet::new();
    let mut revision_ids = BTreeSet::new();
    for revision in &policy.rule_sets {
        validate_nonempty_id(
            "interaction policy rule-set id",
            revision.rule_set_id.as_str(),
        )?;
        validate_nonempty_id(
            "interaction policy rule-set revision id",
            &revision.revision_id,
        )?;
        if !is_sha256(&revision.sha256) {
            return Err(CoreError::invalid(
                "interaction policy rule-set revision hash must be lowercase SHA-256",
            ));
        }
        if !rule_set_ids.insert(revision.rule_set_id.as_str())
            || !revision_ids.insert(revision.revision_id.as_str())
        {
            return Err(CoreError::invalid(
                "interaction policy contains duplicate rule-set identities",
            ));
        }
    }
    Ok(())
}

fn canonical_empty_module_plan_sha256() -> String {
    sha256_hex(b"lorepia.interaction-module-plan.none.v1")
}

pub(super) fn stored_module_plan_sha256(policy: &InteractionPolicySnapshot) -> String {
    policy
        .module_plan_sha256
        .clone()
        .unwrap_or_else(canonical_empty_module_plan_sha256)
}

pub(super) fn encode_interaction_policy(
    policy: &InteractionPolicySnapshot,
) -> CoreResult<(String, String, String)> {
    validate_policy_shape(policy)?;
    let policy_json = encode_json("interaction policy", policy, MAX_EVENT_JSON_BYTES)?;
    let policy_sha256 = interaction_policy_sha256(policy)?;
    Ok((
        stored_module_plan_sha256(policy),
        policy_json,
        policy_sha256,
    ))
}

pub(super) fn encode_interaction_evaluation_authority(
    policy: &InteractionPolicySnapshot,
    evaluation_seal: Option<&InteractionEvaluationSeal>,
    deterministic_seed: Option<u64>,
) -> CoreResult<(Option<String>, Option<String>, i64)> {
    validate_evaluation_authority_pair(policy, evaluation_seal, deterministic_seed)?;
    match evaluation_seal {
        Some(seal) => {
            let json = encode_json("interaction evaluation seal", seal, MAX_STATE_JSON_BYTES)?;
            let sha256 = interaction_evaluation_seal_sha256(seal)?;
            Ok((Some(json), Some(sha256.as_str().to_owned()), 1))
        }
        None => Ok((None, None, 0)),
    }
}

pub(super) fn validate_derived_event_commit(
    decision_state: &InteractionState,
    derived: &InteractionDerivedEventCommit,
) -> CoreResult<()> {
    validate_nonempty_id("interaction event id", &derived.event_id)?;
    validate_nonempty_id(
        "interaction event idempotency key",
        &derived.idempotency_key,
    )?;
    validate_policy_shape(&derived.policy)?;
    validate_event_evaluation_authority(
        &derived.policy,
        derived.evaluation_seal.as_ref(),
        derived.deterministic_seed,
        &derived.derived_events,
    )?;
    validate_state(&derived.next_state)?;
    validate_new_event_collections(
        &derived.action_results,
        &derived.effects,
        &derived.proposals,
    )?;
    let expected = decision_state
        .revision
        .checked_add(1)
        .ok_or_else(|| CoreError::invalid("interaction state revision overflowed"))?;
    if derived.next_state.revision != expected {
        return Err(CoreError::invalid(format!(
            "derived interaction event must advance state to revision {expected}"
        )));
    }
    let mut logical_next_state = derived.next_state.clone();
    logical_next_state.revision = decision_state.revision;
    if logical_next_state == *decision_state
        && derived.action_results.is_empty()
        && derived.effects.is_empty()
        && derived.proposals.is_empty()
    {
        return Err(CoreError::invalid(
            "no-op proposal approval must omit the derived event commit",
        ));
    }
    validate_knowledge_bindings(&derived.next_state, &derived.knowledge)?;
    Ok(())
}

fn validate_event_evaluation_authority(
    policy: &InteractionPolicySnapshot,
    evaluation_seal: Option<&InteractionEvaluationSeal>,
    deterministic_seed: Option<u64>,
    derived_events: &[InteractionDerivedEventWrite],
) -> CoreResult<()> {
    validate_evaluation_authority_pair(policy, evaluation_seal, deterministic_seed)?;
    if !derived_events.is_empty() && evaluation_seal.is_none() {
        return Err(CoreError::invalid(
            "derived interaction events require sealed evaluation authority",
        ));
    }
    Ok(())
}

fn validate_evaluation_authority_pair(
    policy: &InteractionPolicySnapshot,
    evaluation_seal: Option<&InteractionEvaluationSeal>,
    deterministic_seed: Option<u64>,
) -> CoreResult<()> {
    match (evaluation_seal, deterministic_seed) {
        (None, None) => Ok(()),
        (Some(seal), Some(_)) => {
            let policy_sha256 = Sha256Digest::parse(interaction_policy_sha256(policy)?)
                .map_err(CoreError::invalid)?;
            interaction_evaluation_seal_sha256(seal)?;
            if seal.policy_sha256 != policy_sha256 {
                return Err(CoreError::invalid(
                    "interaction evaluation seal does not match its event policy",
                ));
            }
            Ok(())
        }
        _ => Err(CoreError::invalid(
            "interaction evaluation seal and deterministic seed must be supplied together",
        )),
    }
}

pub(super) fn validate_event_collections(
    action_results: &[InteractionActionResultWrite],
    effects: &[InteractionEffect],
    proposals: &[InteractionProposalWrite],
) -> CoreResult<()> {
    if action_results.len() > MAX_ACTION_RESULTS_PER_EVENT
        || effects.len() > MAX_EFFECTS_PER_EVENT
        || proposals.len() > MAX_INTERACTION_PROPOSALS
    {
        return Err(CoreError::invalid(
            "interaction event exceeds action-result, effect, or proposal limits",
        ));
    }
    Ok(())
}

/// Write-side validation for effects and proposals that may later cross the
/// native boundary. Read paths intentionally retain the count-only validator
/// above so legacy evidence remains decodable and can be projected as a typed,
/// redacted rejection instead of being rewritten or hidden.
pub(super) fn validate_new_event_collections(
    action_results: &[InteractionActionResultWrite],
    effects: &[InteractionEffect],
    proposals: &[InteractionProposalWrite],
) -> CoreResult<()> {
    validate_event_collections(action_results, effects, proposals)?;
    for effect in effects {
        if let InteractionEffect::ChoicesPresented { choices } = effect {
            let mut choice_ids = BTreeSet::new();
            if choices.is_empty()
                || choices.iter().any(|choice| {
                    choice.id.trim().is_empty() || !choice_ids.insert(choice.id.as_str())
                })
            {
                return Err(CoreError::invalid(
                    "new choice interaction effect has invalid or duplicate choice IDs",
                ));
            }
        }
        validate_interaction_effect_native_text(effect)
            .map_err(|error| CoreError::invalid(error.to_string()))?;
    }
    for proposal in proposals {
        validate_interaction_native_text("interaction_proposal.title", &proposal.record.title)
            .map_err(|error| CoreError::invalid(error.to_string()))?;
        validate_interaction_native_text("interaction_proposal.body", &proposal.record.body)
            .map_err(|error| CoreError::invalid(error.to_string()))?;
    }
    Ok(())
}

pub(super) fn interaction_event_kind(event: &InteractionEvent) -> &'static str {
    match event {
        InteractionEvent::ConversationOpened => "conversation_opened",
        InteractionEvent::ConversationStarted => "conversation_started",
        InteractionEvent::BeforeGeneration => "before_generation",
        InteractionEvent::AfterGeneration => "after_generation",
        InteractionEvent::MessageCommitted => "message_committed",
        InteractionEvent::UserAction { .. } => "user_action",
        InteractionEvent::VariableChanged { .. } => "variable_changed",
        InteractionEvent::KnowledgeActivated { .. } => "knowledge_activated",
    }
}

pub(super) fn event_requires_argument(event: &InteractionEvent) -> bool {
    matches!(
        event,
        InteractionEvent::UserAction { .. }
            | InteractionEvent::VariableChanged { .. }
            | InteractionEvent::KnowledgeActivated { .. }
    )
}

pub(super) fn interaction_event_argument_json(
    event: &InteractionEvent,
) -> CoreResult<Option<String>> {
    event_requires_argument(event)
        .then(|| encode_json("interaction event argument", event, MAX_AUDIT_JSON_BYTES))
        .transpose()
}
