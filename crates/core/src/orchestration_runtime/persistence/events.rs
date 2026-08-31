use std::collections::{BTreeMap, BTreeSet};

use chrono::Utc;
use lorepia_domain::{
    ConversationBranchId, ConversationId, CoreError, CoreErrorCode, CoreResult, GenerationId,
    InteractionAction, InteractionEffect, InteractionEvent, InteractionProposalStatus,
    InteractionRule, InteractionRuleId, InteractionState, MessageId, Sha256Digest, VersionedJson,
};
use lorepia_orchestration::{InteractionOutcome, InteractionRuleStatus};
use lorepia_storage::{
    InteractionActionResultStatus, InteractionActionResultWrite, InteractionChoiceSelectionCommit,
    InteractionDerivedEventCommit, InteractionDerivedEventWrite,
    InteractionDerivedOccurrenceCommit, InteractionEvaluationSeal, InteractionEventCommit,
    InteractionEventOccurrenceLookup, InteractionKnowledgeBinding, InteractionPolicySnapshot,
    InteractionProposalWrite, StoredInteractionDerivedEvent, StoredInteractionEvent,
    interaction_action_sha256, interaction_proposal_review_sha256,
};

use super::super::{
    InteractionReviewRequest, InteractionRuleSetRevision, ResolvedInteractionPolicy,
    initial_interaction_state, interaction_knowledge_bindings, interaction_policy_snapshot,
    interaction_seed, interaction_state_key, reconcile_interaction_knowledge_state,
    versioned_digest,
};
use crate::{
    Core, InteractionChoiceSelectionReceipt,
    interaction_projection::project_interaction_choice_selection_receipt,
};

impl Core {
    /// Commits one trusted durable lifecycle occurrence.
    ///
    /// A persisted outbox occurrence may legitimately lag behind the branch
    /// head. Such delivery validates the immutable room identity and exact
    /// occurrence fields, but does not reinterpret `expected_head` as a fresh
    /// optimistic concurrency token. Generation-owned occurrences also bind
    /// the freshly resolved policy to the immutable attempt module-plan hash.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn process_interaction_event_with_authority(
        &self,
        request: &InteractionReviewRequest,
        occurrence_id: &str,
        generation_attempt_id: Option<&GenerationId>,
        owner_message_id: Option<&MessageId>,
        occurred_at: chrono::DateTime<Utc>,
        enforce_current_head: bool,
        expected_module_plan_sha256: Option<&Sha256Digest>,
    ) -> CoreResult<StoredInteractionEvent> {
        validate_runtime_occurrence_id(occurrence_id)?;
        validate_interaction_event_authority_binding(
            &request.event,
            generation_attempt_id,
            owner_message_id,
        )?;
        let (event_id, idempotency_key) = interaction_occurrence_identity(
            request,
            occurrence_id,
            generation_attempt_id,
            owner_message_id,
            occurred_at,
        )?;
        if let Some(replay) = self.storage().get_interaction_event_by_occurrence(
            &InteractionEventOccurrenceLookup {
                event_id: event_id.clone(),
                idempotency_key: idempotency_key.clone(),
                conversation_id: request.conversation_id.clone(),
                branch_id: request.branch_id.clone(),
                event: request.event.clone(),
                generation_attempt_id: generation_attempt_id.cloned(),
                owner_message_id: owner_message_id.cloned(),
                occurred_at,
            },
        )? {
            validate_expected_interaction_module_plan(&replay.policy, expected_module_plan_sha256)?;
            self.drain_interaction_derived_events()?;
            return Ok(replay);
        }
        if enforce_current_head {
            self.validate_runtime_branch_head(
                &request.conversation_id,
                &request.branch_id,
                request.expected_head.as_ref(),
            )?;
        } else {
            self.validate_runtime_branch_identity(&request.conversation_id, &request.branch_id)?;
        }
        let policy =
            self.resolve_interaction_policy(&request.conversation_id, &request.branch_id)?;
        let state_key = interaction_state_key(&request.conversation_id, &request.branch_id)?;
        let initial_state = initial_interaction_state(&policy);
        let initial_knowledge = interaction_knowledge_bindings(&initial_state, &policy, &[])?;
        self.storage().get_or_init_interaction_state(
            &state_key,
            &initial_state,
            &initial_knowledge,
            occurred_at,
        )?;
        let snapshot = self
            .storage()
            .get_interaction_state_snapshot(&request.conversation_id, &request.branch_id)?;
        let state = snapshot.state;
        // This review is intentionally created only after the durable state
        // read/init. The subsequent transaction independently CAS-checks the
        // same revision, so no caller-supplied review can be committed.
        let prepared = self.prepare_interaction_review_from_state(
            request,
            state.clone(),
            &snapshot.knowledge,
            Some(occurred_at),
            enforce_current_head,
        )?;
        let policy_snapshot = interaction_policy_snapshot(&prepared.policy);
        validate_expected_interaction_module_plan(&policy_snapshot, expected_module_plan_sha256)?;
        let artifacts = interaction_commit_artifacts(
            &state,
            &prepared.public.outcome,
            &prepared.policy,
            request,
            &prepared.evaluation_seal,
            &snapshot.knowledge,
        )?;
        let stored = self
            .storage()
            .commit_interaction_event(&InteractionEventCommit {
                event_id,
                idempotency_key,
                key: snapshot.key,
                expected_state_revision: state.revision,
                event: request.event.clone(),
                generation_attempt_id: generation_attempt_id.cloned(),
                owner_message_id: owner_message_id.cloned(),
                policy: policy_snapshot,
                evaluation_seal: Some(prepared.evaluation_seal.clone()),
                deterministic_seed: Some(prepared.deterministic_seed),
                next_state: prepared.public.outcome.state,
                knowledge: artifacts.knowledge,
                action_results: artifacts.action_results,
                effects: prepared.public.outcome.effects,
                derived_events: artifacts.derived_events,
                proposals: artifacts.proposals,
                created_at: occurred_at,
            })?;
        self.drain_interaction_derived_events()?;
        Ok(stored)
    }
    pub(crate) fn process_interaction_derived_occurrence(
        &self,
        occurrence: &StoredInteractionDerivedEvent,
    ) -> CoreResult<Option<StoredInteractionEvent>> {
        let branch = self
            .validate_runtime_branch_identity(&occurrence.conversation_id, &occurrence.branch_id)?;
        let policy = match self.resolve_sealed_interaction_policy(
            &occurrence.conversation_id,
            &occurrence.branch_id,
            &occurrence.policy,
            &occurrence.evaluation_seal,
        ) {
            Ok(policy) => policy,
            Err(error) if error.recoverable => return Err(error),
            Err(_) => {
                let active_policy = self
                    .resolve_interaction_policy(&occurrence.conversation_id, &occurrence.branch_id)
                    .ok()
                    .map(|policy| interaction_policy_snapshot(&policy));
                self.storage()
                    .quarantine_interaction_derived_event_authority_failure(
                        &occurrence.occurrence_id,
                        occurrence.delivery_attempts,
                        active_policy.as_ref(),
                        Utc::now(),
                    )?;
                return Ok(None);
            }
        };
        let snapshot = self
            .storage()
            .get_interaction_state_snapshot(&occurrence.conversation_id, &occurrence.branch_id)?;
        let request = InteractionReviewRequest {
            conversation_id: occurrence.conversation_id.clone(),
            branch_id: occurrence.branch_id.clone(),
            expected_head: branch.head_message_id,
            event: occurrence.event.clone(),
        };
        let prepared = Self::prepare_interaction_review_with_sealed_authority(
            &request,
            snapshot.state.clone(),
            &snapshot.knowledge,
            occurrence.occurred_at,
            policy,
            occurrence.evaluation_seal.clone(),
            occurrence.deterministic_seed,
        )?;
        let artifacts = interaction_commit_artifacts(
            &snapshot.state,
            &prepared.public.outcome,
            &prepared.policy,
            &request,
            &prepared.evaluation_seal,
            &snapshot.knowledge,
        )?;
        self.storage()
            .commit_interaction_derived_occurrence(&InteractionDerivedOccurrenceCommit {
                occurrence_id: occurrence.occurrence_id.clone(),
                expected_delivery_attempts: occurrence.delivery_attempts,
                key: snapshot.key,
                expected_state_revision: snapshot.state.revision,
                next_state: prepared.public.outcome.state,
                knowledge: artifacts.knowledge,
                action_results: artifacts.action_results,
                effects: prepared.public.outcome.effects,
                derived_events: artifacts.derived_events,
                proposals: artifacts.proposals,
                committed_at: Utc::now(),
            })
            .map(Some)
    }
    /// Selects one exact option from one exact durable `ChoicesPresented`
    /// effect and atomically commits the storage-derived `UserAction`.
    ///
    /// The frontend supplies neither an event kind nor action arguments. Core
    /// reloads the effect, validates room ownership and the exact stored
    /// option, recreates current policy/state review, and storage consumes the
    /// choice exactly once in the same transaction as the derived event.
    pub fn submit_interaction_choice(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        effect_id: &str,
        choice_id: &str,
        expected_state_revision: u64,
    ) -> CoreResult<InteractionChoiceSelectionReceipt> {
        let branch = self.validate_runtime_branch_identity(conversation_id, branch_id)?;
        let stored = self.storage().get_interaction_effect(effect_id)?;
        if stored.stored.conversation_id != *conversation_id
            || stored.stored.branch_id != *branch_id
        {
            return Err(CoreError::new(
                CoreErrorCode::NotFound,
                "interaction choice effect was not found in this branch",
                false,
            ));
        }
        let InteractionEffect::ChoicesPresented { choices } = &stored.stored.effect else {
            return Err(CoreError::invalid(
                "interaction effect does not present choices",
            ));
        };
        if !choices.iter().any(|choice| choice.id == choice_id) {
            return Err(CoreError::invalid(
                "interaction choice is not one of the exact stored options",
            ));
        }

        let snapshot = self
            .storage()
            .get_interaction_state_snapshot(conversation_id, branch_id)?;
        if snapshot.state.revision != expected_state_revision {
            return Err(CoreError::new(
                CoreErrorCode::InvalidInput,
                "interaction state changed before choice selection",
                true,
            ));
        }
        let now = Utc::now();
        let event = InteractionEvent::UserAction {
            action_id: choice_id.to_owned(),
        };
        let request = InteractionReviewRequest {
            conversation_id: conversation_id.clone(),
            branch_id: branch_id.clone(),
            expected_head: branch.head_message_id,
            event: event.clone(),
        };
        let prepared = self.prepare_interaction_review_from_state(
            &request,
            snapshot.state.clone(),
            &snapshot.knowledge,
            Some(now),
            true,
        )?;
        let artifacts = interaction_commit_artifacts(
            &snapshot.state,
            &prepared.public.outcome,
            &prepared.policy,
            &request,
            &prepared.evaluation_seal,
            &snapshot.knowledge,
        )?;
        let event_sha256 = versioned_digest(&(
            "lorepia.interaction-choice-action.v1",
            conversation_id,
            branch_id,
            effect_id,
            choice_id,
            expected_state_revision,
        ))?;
        let current_policy = interaction_policy_snapshot(&prepared.policy);
        let receipt =
            self.storage()
                .consume_interaction_choice(&InteractionChoiceSelectionCommit {
                    effect_id: effect_id.to_owned(),
                    choice_id: choice_id.to_owned(),
                    expected_state_revision,
                    selected_at_epoch_seconds: now.timestamp(),
                    current_policy: current_policy.clone(),
                    derived: InteractionDerivedEventCommit {
                        event_id: format!("interaction-event-{event_sha256}"),
                        idempotency_key: format!("interaction-choice-action:v1:{event_sha256}"),
                        policy: current_policy,
                        evaluation_seal: Some(prepared.evaluation_seal.clone()),
                        deterministic_seed: Some(prepared.deterministic_seed),
                        next_state: prepared.public.outcome.state,
                        knowledge: artifacts.knowledge,
                        action_results: artifacts.action_results,
                        effects: prepared.public.outcome.effects,
                        derived_events: artifacts.derived_events,
                        proposals: artifacts.proposals,
                        created_at: now,
                    },
                })?;
        self.drain_interaction_derived_events()?;
        Ok(project_interaction_choice_selection_receipt(receipt))
    }
}

fn validate_expected_interaction_module_plan(
    policy: &InteractionPolicySnapshot,
    expected: Option<&Sha256Digest>,
) -> CoreResult<()> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let matches = policy.module_plan_sha256.as_deref().map_or_else(
        || *expected == lorepia_orchestration::no_applied_module_runtime_plan_sha256(),
        |actual| actual == expected.as_str(),
    );
    if matches {
        Ok(())
    } else {
        Err(CoreError::new(
            CoreErrorCode::PermissionDenied,
            "interaction lifecycle policy differs from the immutable generation attempt",
            true,
        ))
    }
}
fn interaction_occurrence_identity(
    request: &InteractionReviewRequest,
    occurrence_id: &str,
    generation_attempt_id: Option<&GenerationId>,
    owner_message_id: Option<&MessageId>,
    occurred_at: chrono::DateTime<Utc>,
) -> CoreResult<(String, String)> {
    let occurrence_sha256 = versioned_digest(&(
        "lorepia.interaction-occurrence.v1",
        &request.conversation_id,
        &request.branch_id,
        occurrence_id,
        generation_attempt_id,
        owner_message_id,
        occurred_at,
        &request.event,
    ))?;
    Ok((
        format!("interaction-event-{occurrence_sha256}"),
        format!("interaction-event:v1:{occurrence_sha256}"),
    ))
}
#[derive(Debug)]
pub(in crate::orchestration_runtime) struct InteractionCommitArtifacts {
    pub(in crate::orchestration_runtime) knowledge: Vec<InteractionKnowledgeBinding>,
    pub(in crate::orchestration_runtime) action_results: Vec<InteractionActionResultWrite>,
    pub(in crate::orchestration_runtime) derived_events: Vec<InteractionDerivedEventWrite>,
    pub(in crate::orchestration_runtime) proposals: Vec<InteractionProposalWrite>,
}
fn validate_runtime_occurrence_id(value: &str) -> CoreResult<()> {
    if value.is_empty()
        || value.len() > 256
        || value
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || b"._:-".contains(&byte)))
    {
        return Err(CoreError::invalid(
            "interaction occurrence ID is empty or non-canonical",
        ));
    }
    Ok(())
}
fn validate_interaction_event_authority_binding(
    event: &InteractionEvent,
    generation_attempt_id: Option<&GenerationId>,
    owner_message_id: Option<&MessageId>,
) -> CoreResult<()> {
    match (event, generation_attempt_id, owner_message_id) {
        (InteractionEvent::BeforeGeneration | InteractionEvent::AfterGeneration, Some(_), None)
        | (InteractionEvent::MessageCommitted, None, Some(_))
        | (
            InteractionEvent::ConversationStarted
            | InteractionEvent::ConversationOpened
            | InteractionEvent::UserAction { .. }
            | InteractionEvent::VariableChanged { .. }
            | InteractionEvent::KnowledgeActivated { .. },
            None,
            None,
        ) => Ok(()),
        (InteractionEvent::BeforeGeneration | InteractionEvent::AfterGeneration, None, _) => Err(
            CoreError::invalid("generation lifecycle event requires its exact generation attempt"),
        ),
        (InteractionEvent::MessageCommitted, _, None) => Err(CoreError::invalid(
            "message-committed interaction event requires its exact owner message",
        )),
        (_, Some(_), _) => Err(CoreError::invalid(
            "non-generation lifecycle event cannot bind a generation attempt",
        )),
        (_, _, Some(_)) => Err(CoreError::invalid(
            "only message-committed interaction events bind an owner message",
        )),
    }
}
pub(in crate::orchestration_runtime) fn interaction_commit_artifacts(
    previous: &InteractionState,
    outcome: &InteractionOutcome,
    policy: &ResolvedInteractionPolicy,
    request: &InteractionReviewRequest,
    evaluation_seal: &InteractionEvaluationSeal,
    existing_knowledge: &[InteractionKnowledgeBinding],
) -> CoreResult<InteractionCommitArtifacts> {
    let (_, reconciled_knowledge) =
        reconcile_interaction_knowledge_state(previous.clone(), policy, existing_knowledge)?;
    let rule_sources = interaction_rule_sources(policy)?;
    let action_results =
        interaction_action_results(outcome, policy, &request.event, &rule_sources)?;
    let derived_events =
        interaction_derived_event_writes(outcome, policy, request, evaluation_seal, &rule_sources)?;
    let proposals = interaction_proposal_writes(previous, outcome, &rule_sources)?;
    Ok(InteractionCommitArtifacts {
        knowledge: interaction_knowledge_bindings(&outcome.state, policy, &reconciled_knowledge)?,
        action_results,
        derived_events,
        proposals,
    })
}
type InteractionRuleSource<'a> = (&'a InteractionRuleSetRevision, &'a InteractionRule);
fn interaction_rule_sources(
    policy: &ResolvedInteractionPolicy,
) -> CoreResult<BTreeMap<InteractionRuleId, InteractionRuleSource<'_>>> {
    let mut rule_sources = BTreeMap::new();
    for set in &policy.rule_sets {
        let revision = policy
            .rule_set_revisions
            .iter()
            .find(|revision| revision.rule_set_id == set.id)
            .ok_or_else(|| CoreError::internal("interaction rule set revision is missing"))?;
        for rule in &set.rules {
            if rule_sources
                .insert(rule.id.clone(), (revision, rule))
                .is_some()
            {
                return Err(CoreError::invalid(
                    "interaction rule IDs are ambiguous across approved sets",
                ));
            }
        }
    }
    Ok(rule_sources)
}
fn interaction_action_results(
    outcome: &InteractionOutcome,
    policy: &ResolvedInteractionPolicy,
    event: &InteractionEvent,
    rule_sources: &BTreeMap<InteractionRuleId, InteractionRuleSource<'_>>,
) -> CoreResult<Vec<InteractionActionResultWrite>> {
    let mut action_results = Vec::new();
    for trace in &outcome.trace {
        let Some((set_revision, rule)) = rule_sources.get(&trace.rule_id).copied() else {
            return Err(CoreError::internal(
                "interaction trace references an unknown rule",
            ));
        };
        if &rule.event != event || trace.status == InteractionRuleStatus::EventDidNotMatch {
            continue;
        }
        for (ordinal, action) in rule.actions.iter().enumerate() {
            let action_ordinal = u32::try_from(ordinal)
                .map_err(|_| CoreError::invalid("interaction action ordinal overflowed"))?;
            let asset_diagnostic = policy
                .asset_action_diagnostics
                .get(&(rule.id.as_str().to_owned(), action_ordinal));
            let status = if asset_diagnostic.is_some() {
                InteractionActionResultStatus::Failed
            } else {
                match trace.status {
                    InteractionRuleStatus::Applied
                        if matches!(action, InteractionAction::RequestUserApproval { .. }) =>
                    {
                        InteractionActionResultStatus::Proposed
                    }
                    InteractionRuleStatus::Applied => InteractionActionResultStatus::Applied,
                    InteractionRuleStatus::Failed | InteractionRuleStatus::ActionBudgetExceeded => {
                        InteractionActionResultStatus::Failed
                    }
                    InteractionRuleStatus::ConditionFalse
                    | InteractionRuleStatus::Disabled
                    | InteractionRuleStatus::PendingImportApproval
                    | InteractionRuleStatus::EventDidNotMatch => {
                        InteractionActionResultStatus::Skipped
                    }
                }
            };
            action_results.push(InteractionActionResultWrite {
                set_revision_id: set_revision.revision_id.clone(),
                rule_id: rule.id.clone(),
                action_ordinal,
                status,
                result: asset_diagnostic.cloned().unwrap_or_else(|| VersionedJson {
                    schema_version: 1,
                    value: serde_json::json!({
                        "rule_status": &trace.status,
                        "state_changed": trace.state_changed,
                        "effect_count": trace.effect_count,
                    }),
                }),
            });
        }
    }
    Ok(action_results)
}
fn interaction_derived_event_writes(
    outcome: &InteractionOutcome,
    policy: &ResolvedInteractionPolicy,
    request: &InteractionReviewRequest,
    evaluation_seal: &InteractionEvaluationSeal,
    rule_sources: &BTreeMap<InteractionRuleId, InteractionRuleSource<'_>>,
) -> CoreResult<Vec<InteractionDerivedEventWrite>> {
    let mut derived_events = Vec::with_capacity(outcome.derived_events.len());
    for derived in &outcome.derived_events {
        let Some((set_revision, rule)) = rule_sources.get(&derived.source_rule_id).copied() else {
            return Err(CoreError::internal(
                "derived interaction event references an unknown source rule",
            ));
        };
        if set_revision.rule_set_id != derived.source_rule_set_id {
            return Err(CoreError::internal(
                "derived interaction event references a mismatched source rule set",
            ));
        }
        let action_index = usize::try_from(derived.source_action_ordinal)
            .map_err(|_| CoreError::invalid("derived interaction action ordinal overflowed"))?;
        let action = rule.actions.get(action_index).ok_or_else(|| {
            CoreError::internal("derived interaction event source action disappeared")
        })?;
        let child_request = InteractionReviewRequest {
            conversation_id: request.conversation_id.clone(),
            branch_id: request.branch_id.clone(),
            expected_head: request.expected_head.clone(),
            event: derived.event.clone(),
        };
        let deterministic_seed = interaction_seed(
            &child_request,
            outcome.state.revision,
            &policy.rule_set_revisions,
            evaluation_seal.event_epoch_seconds,
        )?;
        derived_events.push(InteractionDerivedEventWrite {
            event: derived.event.clone(),
            source_set_revision_id: set_revision.revision_id.clone(),
            source_rule_id: derived.source_rule_id.clone(),
            source_action_ordinal: derived.source_action_ordinal,
            source_effect_ordinal: derived.source_effect_ordinal,
            source_action_sha256: interaction_action_sha256(action)?,
            deterministic_seed,
        });
    }
    Ok(derived_events)
}
fn interaction_proposal_writes(
    previous: &InteractionState,
    outcome: &InteractionOutcome,
    rule_sources: &BTreeMap<InteractionRuleId, InteractionRuleSource<'_>>,
) -> CoreResult<Vec<InteractionProposalWrite>> {
    let existing_ids = previous
        .proposals
        .iter()
        .map(|proposal| proposal.id.clone())
        .collect::<BTreeSet<_>>();
    let mut proposals = Vec::new();
    for record in outcome
        .state
        .proposals
        .iter()
        .filter(|record| !existing_ids.contains(&record.id))
    {
        if record.status != InteractionProposalStatus::Pending {
            return Err(CoreError::invalid(
                "new interaction proposal is not pending",
            ));
        }
        let Some((set_revision, rule)) = rule_sources.get(&record.rule_id).copied() else {
            return Err(CoreError::invalid(
                "new interaction proposal references an unknown rule",
            ));
        };
        if set_revision.rule_set_id != record.rule_set_id {
            return Err(CoreError::invalid(
                "new interaction proposal rule set identity is inconsistent",
            ));
        }
        let matching_actions = rule
            .actions
            .iter()
            .enumerate()
            .filter(|(_, action)| {
                matches!(
                    action,
                    InteractionAction::RequestUserApproval { proposal }
                        if proposal.id == record.proposal_id
                )
            })
            .map(|(ordinal, _)| ordinal)
            .collect::<Vec<_>>();
        let [action_ordinal] = matching_actions.as_slice() else {
            return Err(CoreError::invalid(
                "interaction proposal does not have one exact source action",
            ));
        };
        proposals.push(InteractionProposalWrite {
            record: record.clone(),
            rule_set_revision_id: set_revision.revision_id.clone(),
            action_ordinal: u32::try_from(*action_ordinal)
                .map_err(|_| CoreError::invalid("interaction proposal action overflowed"))?,
            review_payload_sha256: interaction_proposal_review_sha256(record)?,
        });
    }
    proposals.sort_by(|left, right| left.record.id.cmp(&right.record.id));
    Ok(proposals)
}
