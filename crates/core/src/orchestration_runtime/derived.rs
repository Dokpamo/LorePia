use std::collections::{BTreeSet, VecDeque};

use chrono::Utc;
use lorepia_domain::{
    CoreError, CoreErrorCode, CoreResult, GenerationId, InteractionEvent, InteractionState,
    Sha256Digest,
};
use lorepia_orchestration::{AppliedModuleRuntimePlan, ModuleMergeReview};
use lorepia_storage::{
    GenerationAttemptBeforeReviewCommit, GenerationAttemptDerivedClosure,
    GenerationAttemptDerivedGuardAudit, GenerationAttemptDerivedGuardKind,
    GenerationAttemptDerivedTransition, InteractionDerivedEventWrite, InteractionKnowledgeBinding,
    StoredGenerationAttempt, StoredGenerationAttemptProposal, StoredInteractionState,
    generation_attempt_derived_chain_sha256, generation_attempt_derived_closure_sha256,
    generation_attempt_derived_event_sha256, generation_attempt_derived_transition_commit_sha256,
    generation_attempt_derived_transition_sha256,
};
use serde::Serialize;

use super::{
    InteractionReviewRequest, PreparedInteractionReview, interaction_policy_snapshot,
    persistence::{InteractionCommitArtifacts, interaction_commit_artifacts},
    remap_generation_attempt_proposal_ids, versioned_digest,
};
use crate::Core;

const MAX_GENERATION_ATTEMPT_DERIVED_EVENTS: usize = 256;
const MAX_GENERATION_ATTEMPT_DERIVED_DEPTH: u32 = 16;
const MAX_GENERATION_ATTEMPT_DERIVED_GUARDS: usize = 1_024;

impl Core {
    pub(crate) fn prepare_generation_attempt_before_review(
        &self,
        attempt: &StoredGenerationAttempt,
        boundary: &StoredInteractionState,
        context_checkpoint_sha256: &str,
        module_runtime_review: &ModuleMergeReview,
        applied_runtime_plan: Option<&AppliedModuleRuntimePlan>,
        occurred_at: chrono::DateTime<Utc>,
    ) -> CoreResult<GenerationAttemptBeforeReviewCommit> {
        Self::validate_generation_attempt_before_review_authority(
            attempt,
            boundary,
            context_checkpoint_sha256,
            module_runtime_review,
            applied_runtime_plan,
        )?;

        let request = InteractionReviewRequest {
            conversation_id: attempt.input.conversation_id.clone(),
            branch_id: attempt.input.proposed_branch_id.clone(),
            expected_head: attempt.input.context_head_message_id.clone(),
            event: InteractionEvent::BeforeGeneration,
        };
        let prepared = self.prepare_proposed_branch_interaction_review_from_state(
            &request,
            boundary.state.clone(),
            &boundary.knowledge,
            occurred_at,
            applied_runtime_plan,
        )?;
        let policy = interaction_policy_snapshot(&prepared.policy);
        let artifacts = interaction_commit_artifacts(
            &boundary.state,
            &prepared.public.outcome,
            &prepared.policy,
            &request,
            &prepared.evaluation_seal,
            &boundary.knowledge,
        )?;
        let event_sha256 = versioned_digest(&(
            "lorepia.generation-attempt-before-event.v1",
            &attempt.generation_id,
            &request,
            occurred_at,
            &prepared.public.review_sha256,
        ))?;
        let event_id = format!("interaction-event-{event_sha256}");
        let derived_closure = prepare_generation_attempt_derived_closure(
            &attempt.generation_id,
            &event_id,
            &request,
            &boundary.state,
            &prepared,
            &artifacts,
            occurred_at,
        )?;
        let mut proposals = derived_closure
            .transitions
            .iter()
            .flat_map(|transition| transition.proposals.iter().cloned())
            .collect::<Vec<_>>();
        proposals.sort_by(|left, right| left.record.id.cmp(&right.record.id));
        let memory_head_snapshot = self
            .storage()
            .list_memory_records_at_head(
                &attempt.input.conversation_id,
                &attempt.input.source_branch_id,
                attempt.input.context_head_message_id.as_ref(),
                false,
            )?
            .snapshot;
        Ok(GenerationAttemptBeforeReviewCommit {
            generation_id: attempt.generation_id.clone(),
            expected_attempt_revision: attempt.revision,
            event_id,
            occurred_at,
            context_head_message_id: attempt.input.context_head_message_id.clone(),
            context_checkpoint_sha256: context_checkpoint_sha256.to_owned(),
            previous_state: boundary.state.clone(),
            previous_knowledge: boundary.knowledge.clone(),
            module_runtime_review: module_runtime_review.clone(),
            memory_head_snapshot,
            applied_runtime_plan: applied_runtime_plan.cloned(),
            policy,
            evaluation_seal: prepared.evaluation_seal.clone(),
            derived_closure,
            next_state: prepared.public.outcome.state.clone(),
            knowledge: artifacts.knowledge,
            action_results: artifacts.action_results,
            effects: prepared.public.outcome.effects.clone(),
            derived_events: artifacts.derived_events,
            proposals,
            review_sha256: prepared.public.review_sha256.clone(),
        })
    }
}

#[derive(Clone)]
struct GenerationAttemptDerivedCandidate {
    parent_ordinal: u32,
    depth: u32,
    event: InteractionEvent,
    deterministic_seed: u64,
    visited_event_sha256s: BTreeSet<Sha256Digest>,
}
#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct GenerationAttemptGuardFingerprint<'a> {
    schema_version: u32,
    kind: GenerationAttemptDerivedGuardKind,
    candidate_event_sha256: Option<&'a Sha256Digest>,
    parent_ordinal: u32,
    depth: u32,
    suppressed_count: u32,
}
struct GenerationAttemptTransitionInput<'a> {
    ordinal: u32,
    parent_ordinal: Option<u32>,
    depth: u32,
    event_id: &'a str,
    request: &'a InteractionReviewRequest,
    previous_state: &'a InteractionState,
    prepared: &'a PreparedInteractionReview,
    artifacts: &'a InteractionCommitArtifacts,
}
fn materialize_generation_attempt_transition(
    generation_id: &GenerationId,
    input: GenerationAttemptTransitionInput<'_>,
) -> CoreResult<GenerationAttemptDerivedTransition> {
    if input.prepared.public.expected_state_revision != input.previous_state.revision {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "generation derived transition review has the wrong state boundary",
            false,
        ));
    }
    let event_sha256 = generation_attempt_derived_event_sha256(&input.request.event)?;
    let deterministic_seed = input.prepared.deterministic_seed;
    let policy = interaction_policy_snapshot(&input.prepared.policy);
    let resulting_state_revision = input.prepared.public.outcome.state.revision;
    let mut transition = GenerationAttemptDerivedTransition {
        ordinal: input.ordinal,
        parent_ordinal: input.parent_ordinal,
        depth: input.depth,
        event_id: input.event_id.to_owned(),
        event: input.request.event.clone(),
        event_sha256,
        deterministic_seed,
        expected_state_revision: input.previous_state.revision,
        resulting_state_revision,
        policy,
        evaluation_seal: input.prepared.evaluation_seal.clone(),
        next_state: input.prepared.public.outcome.state.clone(),
        knowledge: input.artifacts.knowledge.clone(),
        action_results: input.artifacts.action_results.clone(),
        effects: input.prepared.public.outcome.effects.clone(),
        derived_events: input.artifacts.derived_events.clone(),
        proposals: input.artifacts.proposals.clone(),
        commit_sha256: Sha256Digest::parse("0".repeat(64)).map_err(CoreError::invalid)?,
    };
    transition.commit_sha256 =
        generation_attempt_derived_transition_commit_sha256(generation_id, &transition)?;
    generation_attempt_derived_transition_sha256(&transition)?;
    Ok(transition)
}
fn refresh_generation_attempt_guard_hash(
    audit: &mut GenerationAttemptDerivedGuardAudit,
) -> CoreResult<()> {
    audit.evidence_sha256 =
        Sha256Digest::parse(versioned_digest(&GenerationAttemptGuardFingerprint {
            schema_version: 1,
            kind: audit.kind,
            candidate_event_sha256: audit.candidate_event_sha256.as_ref(),
            parent_ordinal: audit.parent_ordinal,
            depth: audit.depth,
            suppressed_count: audit.suppressed_count,
        })?)
        .map_err(CoreError::invalid)?;
    Ok(())
}
fn record_generation_attempt_guard(
    audits: &mut Vec<GenerationAttemptDerivedGuardAudit>,
    kind: GenerationAttemptDerivedGuardKind,
    candidate_event_sha256: Option<Sha256Digest>,
    parent_ordinal: u32,
    depth: u32,
) -> CoreResult<()> {
    if let Some(existing) = audits.iter_mut().find(|audit| {
        audit.kind == kind
            && audit.candidate_event_sha256 == candidate_event_sha256
            && audit.parent_ordinal == parent_ordinal
            && audit.depth == depth
    }) {
        existing.suppressed_count = existing
            .suppressed_count
            .checked_add(1)
            .ok_or_else(|| CoreError::invalid("generation derived guard count overflowed"))?;
        return refresh_generation_attempt_guard_hash(existing);
    }
    if audits.len() >= MAX_GENERATION_ATTEMPT_DERIVED_GUARDS {
        return Err(CoreError::invalid(
            "generation derived guard audit bound was exceeded",
        ));
    }
    let mut audit = GenerationAttemptDerivedGuardAudit {
        kind,
        candidate_event_sha256,
        parent_ordinal,
        depth,
        suppressed_count: 1,
        evidence_sha256: Sha256Digest::parse("0".repeat(64)).map_err(CoreError::invalid)?,
    };
    refresh_generation_attempt_guard_hash(&mut audit)?;
    audits.push(audit);
    Ok(())
}
fn enqueue_generation_attempt_derived_candidates(
    queue: &mut VecDeque<GenerationAttemptDerivedCandidate>,
    audits: &mut Vec<GenerationAttemptDerivedGuardAudit>,
    accepted_event_count: usize,
    parent_ordinal: u32,
    parent_depth: u32,
    parent_visited_event_sha256s: &BTreeSet<Sha256Digest>,
    derived_events: &[InteractionDerivedEventWrite],
) -> CoreResult<()> {
    let depth = parent_depth
        .checked_add(1)
        .ok_or_else(|| CoreError::invalid("generation derived depth overflowed"))?;
    for derived in derived_events {
        let event_sha256 = generation_attempt_derived_event_sha256(&derived.event)?;
        if parent_visited_event_sha256s.contains(&event_sha256) {
            record_generation_attempt_guard(
                audits,
                GenerationAttemptDerivedGuardKind::Cycle,
                Some(event_sha256),
                parent_ordinal,
                depth,
            )?;
            continue;
        }
        if depth > MAX_GENERATION_ATTEMPT_DERIVED_DEPTH {
            record_generation_attempt_guard(
                audits,
                GenerationAttemptDerivedGuardKind::DepthLimit,
                Some(event_sha256),
                parent_ordinal,
                depth,
            )?;
            continue;
        }
        if accepted_event_count.saturating_add(queue.len()) >= MAX_GENERATION_ATTEMPT_DERIVED_EVENTS
        {
            record_generation_attempt_guard(
                audits,
                GenerationAttemptDerivedGuardKind::CountLimit,
                None,
                parent_ordinal,
                depth,
            )?;
            continue;
        }
        let mut visited_event_sha256s = parent_visited_event_sha256s.clone();
        visited_event_sha256s.insert(event_sha256);
        queue.push_back(GenerationAttemptDerivedCandidate {
            parent_ordinal,
            depth,
            event: derived.event.clone(),
            deterministic_seed: derived.deterministic_seed,
            visited_event_sha256s,
        });
    }
    Ok(())
}
pub(super) fn prepare_generation_attempt_derived_closure(
    generation_id: &GenerationId,
    root_event_id: &str,
    root_request: &InteractionReviewRequest,
    previous_state: &InteractionState,
    root_prepared: &PreparedInteractionReview,
    root_artifacts: &InteractionCommitArtifacts,
    occurred_at: chrono::DateTime<Utc>,
) -> CoreResult<GenerationAttemptDerivedClosure> {
    let root_transition = materialize_generation_attempt_transition(
        generation_id,
        GenerationAttemptTransitionInput {
            ordinal: 0,
            parent_ordinal: None,
            depth: 0,
            event_id: root_event_id,
            request: root_request,
            previous_state,
            prepared: root_prepared,
            artifacts: root_artifacts,
        },
    )?;
    let root_event_sha256 = root_transition.event_sha256.clone();
    let mut transitions = vec![root_transition];
    let mut guards = Vec::new();
    let mut queue = VecDeque::new();
    let mut root_visited = BTreeSet::new();
    root_visited.insert(root_event_sha256);
    enqueue_generation_attempt_derived_candidates(
        &mut queue,
        &mut guards,
        transitions.len(),
        0,
        0,
        &root_visited,
        &root_artifacts.derived_events,
    )?;
    let mut current_state = root_prepared.public.outcome.state.clone();
    let mut current_knowledge = root_artifacts.knowledge.clone();
    while let Some(candidate) = queue.pop_front() {
        let ordinal = u32::try_from(transitions.len())
            .map_err(|_| CoreError::invalid("generation derived ordinal overflowed"))?;
        let event_id =
            generation_attempt_derived_event_id(generation_id, root_event_id, ordinal, &candidate)?;
        let request = InteractionReviewRequest {
            conversation_id: root_request.conversation_id.clone(),
            branch_id: root_request.branch_id.clone(),
            expected_head: root_request.expected_head.clone(),
            event: candidate.event.clone(),
        };
        let prepared = Core::prepare_interaction_review_with_sealed_authority(
            &request,
            current_state.clone(),
            &current_knowledge,
            occurred_at,
            root_prepared.policy.clone(),
            root_prepared.evaluation_seal.clone(),
            candidate.deterministic_seed,
        )?;
        if prepared.evaluation_seal != root_prepared.evaluation_seal {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "generation derived transition changed its sealed evaluation context",
                false,
            ));
        }
        let artifacts = interaction_commit_artifacts(
            &current_state,
            &prepared.public.outcome,
            &prepared.policy,
            &request,
            &prepared.evaluation_seal,
            &current_knowledge,
        )?;
        let transition = materialize_generation_attempt_transition(
            generation_id,
            GenerationAttemptTransitionInput {
                ordinal,
                parent_ordinal: Some(candidate.parent_ordinal),
                depth: candidate.depth,
                event_id: &event_id,
                request: &request,
                previous_state: &current_state,
                prepared: &prepared,
                artifacts: &artifacts,
            },
        )?;
        current_state = transition.next_state.clone();
        current_knowledge.clone_from(&transition.knowledge);
        transitions.push(transition);
        enqueue_generation_attempt_derived_candidates(
            &mut queue,
            &mut guards,
            transitions.len(),
            ordinal,
            candidate.depth,
            &candidate.visited_event_sha256s,
            &artifacts.derived_events,
        )?;
    }
    finalize_generation_attempt_derived_closure(
        transitions,
        guards,
        current_state,
        current_knowledge,
    )
}
fn generation_attempt_derived_event_id(
    generation_id: &GenerationId,
    root_event_id: &str,
    ordinal: u32,
    candidate: &GenerationAttemptDerivedCandidate,
) -> CoreResult<String> {
    versioned_digest(&(
        "lorepia.generation-attempt-derived-occurrence.v1",
        generation_id,
        root_event_id,
        ordinal,
        candidate.parent_ordinal,
        &candidate.event,
    ))
    .map(|sha256| format!("interaction-event-{sha256}"))
}
fn finalize_generation_attempt_derived_closure(
    transitions: Vec<GenerationAttemptDerivedTransition>,
    guard_audits: Vec<GenerationAttemptDerivedGuardAudit>,
    final_state: InteractionState,
    final_knowledge: Vec<InteractionKnowledgeBinding>,
) -> CoreResult<GenerationAttemptDerivedClosure> {
    let event_count = u32::try_from(transitions.len())
        .map_err(|_| CoreError::invalid("generation derived event count overflowed"))?;
    let guard_count = u32::try_from(guard_audits.len())
        .map_err(|_| CoreError::invalid("generation derived guard count overflowed"))?;
    let mut closure = GenerationAttemptDerivedClosure {
        schema_version: 1,
        transitions,
        guard_audits,
        final_state,
        final_knowledge,
        event_count,
        guard_count,
        chain_sha256: Sha256Digest::parse("0".repeat(64)).map_err(CoreError::invalid)?,
    };
    closure.chain_sha256 = generation_attempt_derived_chain_sha256(&closure)?;
    generation_attempt_derived_closure_sha256(&closure)?;
    Ok(closure)
}
pub(super) fn remap_generation_attempt_derived_closure_existing_proposals(
    generation_id: &GenerationId,
    mut closure: GenerationAttemptDerivedClosure,
    proposals: &[StoredGenerationAttemptProposal],
) -> CoreResult<GenerationAttemptDerivedClosure> {
    for transition in &mut closure.transitions {
        transition.next_state = remap_generation_attempt_proposal_ids(
            generation_id,
            &transition.next_state,
            proposals,
            false,
        )?;
        transition.commit_sha256 =
            generation_attempt_derived_transition_commit_sha256(generation_id, transition)?;
    }
    closure.final_state = remap_generation_attempt_proposal_ids(
        generation_id,
        &closure.final_state,
        proposals,
        false,
    )?;
    closure.chain_sha256 = generation_attempt_derived_chain_sha256(&closure)?;
    generation_attempt_derived_closure_sha256(&closure)?;
    Ok(closure)
}
