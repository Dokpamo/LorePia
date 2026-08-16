PRAGMA foreign_keys = ON;

-- A generation BeforeGeneration review is durable before provider dispatch
-- and isolated from live branch state. This immutable snapshot is the only
-- authority from which attempt-scoped approvals and the eventual atomic
-- generation append may proceed, for both same-branch and fork attempts.
CREATE TABLE generation_attempt_before_event_snapshots (
    generation_id TEXT PRIMARY KEY NOT NULL
        REFERENCES generation_attempt_intents(generation_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    event_id TEXT NOT NULL UNIQUE CHECK (length(trim(event_id)) > 0),
    event_kind TEXT NOT NULL CHECK (event_kind = 'before_generation'),
    event_json TEXT NOT NULL CHECK (
        json_valid(event_json)
        AND json_type(event_json) = 'object'
        AND length(CAST(event_json AS BLOB)) <= 1048576
    ),
    event_sha256 TEXT NOT NULL CHECK (
        length(event_sha256) = 64
        AND event_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    occurred_at TEXT NOT NULL CHECK (length(trim(occurred_at)) > 0),
    context_head_message_id TEXT,
    context_checkpoint_sha256 TEXT NOT NULL CHECK (
        length(context_checkpoint_sha256) = 64
        AND context_checkpoint_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    previous_state_revision INTEGER NOT NULL CHECK (
        previous_state_revision >= 0
    ),
    previous_state_json TEXT NOT NULL CHECK (
        json_valid(previous_state_json)
        AND json_type(previous_state_json) = 'object'
        AND length(CAST(previous_state_json AS BLOB)) <= 8388608
    ),
    previous_state_document_sha256 TEXT NOT NULL CHECK (
        length(previous_state_document_sha256) = 64
        AND previous_state_document_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    previous_state_snapshot_sha256 TEXT NOT NULL CHECK (
        length(previous_state_snapshot_sha256) = 64
        AND previous_state_snapshot_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    previous_knowledge_json TEXT NOT NULL CHECK (
        json_valid(previous_knowledge_json)
        AND json_type(previous_knowledge_json) = 'array'
        AND length(CAST(previous_knowledge_json AS BLOB)) <= 8388608
    ),
    previous_knowledge_sha256 TEXT NOT NULL CHECK (
        length(previous_knowledge_sha256) = 64
        AND previous_knowledge_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    applied_runtime_plan_sha256 TEXT NOT NULL CHECK (
        length(applied_runtime_plan_sha256) = 64
        AND applied_runtime_plan_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    module_runtime_review_json TEXT NOT NULL CHECK (
        json_valid(module_runtime_review_json)
        AND json_type(module_runtime_review_json) = 'object'
        AND length(CAST(module_runtime_review_json AS BLOB)) <= 8388608
    ),
    module_runtime_review_sha256 TEXT NOT NULL CHECK (
        length(module_runtime_review_sha256) = 64
        AND module_runtime_review_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    memory_head_snapshot_json TEXT NOT NULL CHECK (
        json_valid(memory_head_snapshot_json)
        AND json_type(memory_head_snapshot_json) = 'object'
        AND length(CAST(memory_head_snapshot_json AS BLOB)) <= 8388608
    ),
    memory_head_snapshot_sha256 TEXT NOT NULL CHECK (
        length(memory_head_snapshot_sha256) = 64
        AND memory_head_snapshot_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    source_runtime_plan_sha256 TEXT
        REFERENCES applied_module_runtime_plans(applied_plan_sha256)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    source_activation_plan_sha256 TEXT
        REFERENCES module_activation_plans(plan_sha256)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    applied_runtime_plan_json TEXT CHECK (
        applied_runtime_plan_json IS NULL
        OR (
            json_valid(applied_runtime_plan_json)
            AND json_type(applied_runtime_plan_json) = 'object'
            AND length(CAST(applied_runtime_plan_json AS BLOB)) <= 8388608
        )
    ),
    policy_json TEXT NOT NULL CHECK (
        json_valid(policy_json)
        AND json_type(policy_json) = 'object'
        AND length(CAST(policy_json AS BLOB)) <= 1048576
    ),
    policy_sha256 TEXT NOT NULL CHECK (
        length(policy_sha256) = 64
        AND policy_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    reviewed_next_state_json TEXT NOT NULL CHECK (
        json_valid(reviewed_next_state_json)
        AND json_type(reviewed_next_state_json) = 'object'
        AND length(CAST(reviewed_next_state_json AS BLOB)) <= 8388608
    ),
    reviewed_next_state_document_sha256 TEXT NOT NULL CHECK (
        length(reviewed_next_state_document_sha256) = 64
        AND reviewed_next_state_document_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    reviewed_next_state_snapshot_sha256 TEXT NOT NULL CHECK (
        length(reviewed_next_state_snapshot_sha256) = 64
        AND reviewed_next_state_snapshot_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    knowledge_json TEXT NOT NULL CHECK (
        json_valid(knowledge_json)
        AND json_type(knowledge_json) = 'array'
        AND length(CAST(knowledge_json AS BLOB)) <= 8388608
    ),
    knowledge_sha256 TEXT NOT NULL CHECK (
        length(knowledge_sha256) = 64
        AND knowledge_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    action_results_json TEXT NOT NULL CHECK (
        json_valid(action_results_json)
        AND json_type(action_results_json) = 'array'
        AND length(CAST(action_results_json AS BLOB)) <= 8388608
    ),
    action_results_sha256 TEXT NOT NULL CHECK (
        length(action_results_sha256) = 64
        AND action_results_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    effects_json TEXT NOT NULL CHECK (
        json_valid(effects_json)
        AND json_type(effects_json) = 'array'
        AND length(CAST(effects_json AS BLOB)) <= 8388608
    ),
    effects_sha256 TEXT NOT NULL CHECK (
        length(effects_sha256) = 64
        AND effects_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    proposal_writes_json TEXT NOT NULL CHECK (
        json_valid(proposal_writes_json)
        AND json_type(proposal_writes_json) = 'array'
        AND length(CAST(proposal_writes_json AS BLOB)) <= 8388608
    ),
    proposal_writes_sha256 TEXT NOT NULL CHECK (
        length(proposal_writes_sha256) = 64
        AND proposal_writes_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    review_sha256 TEXT NOT NULL UNIQUE CHECK (
        length(review_sha256) = 64
        AND review_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    CHECK (
        (
            applied_runtime_plan_json IS NULL
            AND source_activation_plan_sha256 IS NULL
            AND source_runtime_plan_sha256 IS NULL
        )
        OR (
            applied_runtime_plan_json IS NOT NULL
            AND source_activation_plan_sha256 IS NOT NULL
        )
    )
);

CREATE TRIGGER generation_attempt_before_snapshot_authority_guard
BEFORE INSERT ON generation_attempt_before_event_snapshots
WHEN NOT EXISTS (
    SELECT 1
    FROM generation_attempt_intents AS attempt
    WHERE attempt.generation_id = NEW.generation_id
      AND attempt.context_head_message_id IS NEW.context_head_message_id
      AND attempt.module_plan_sha256 = NEW.applied_runtime_plan_sha256
      AND attempt.status IN (
          'prepared',
          'before_generation_applied',
          'awaiting_approval'
      )
)
BEGIN
    SELECT RAISE(ABORT, 'generation attempt before snapshot authority is invalid');
END;

CREATE TRIGGER generation_attempt_before_snapshot_no_update
BEFORE UPDATE ON generation_attempt_before_event_snapshots
BEGIN
    SELECT RAISE(ABORT, 'generation attempt before snapshot is immutable');
END;

CREATE TRIGGER generation_attempt_before_snapshot_no_delete
BEFORE DELETE ON generation_attempt_before_event_snapshots
BEGIN
    SELECT RAISE(ABORT, 'generation attempt before snapshot is immutable');
END;

-- The current reviewed attempt state is a serial CAS aggregate. Every
-- approval/rejection/expiry advances this row exactly once and records the
-- deterministic decision order later copied into GenerationApprovalEvidence.
CREATE TABLE generation_attempt_interaction_aggregates (
    generation_id TEXT PRIMARY KEY NOT NULL
        REFERENCES generation_attempt_before_event_snapshots(generation_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    before_review_sha256 TEXT NOT NULL
        REFERENCES generation_attempt_before_event_snapshots(review_sha256)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    aggregate_revision INTEGER NOT NULL CHECK (aggregate_revision >= 1),
    interaction_state_revision INTEGER NOT NULL CHECK (
        interaction_state_revision >= 0
    ),
    state_json TEXT NOT NULL CHECK (
        json_valid(state_json)
        AND json_type(state_json) = 'object'
        AND length(CAST(state_json AS BLOB)) <= 8388608
    ),
    state_document_sha256 TEXT NOT NULL CHECK (
        length(state_document_sha256) = 64
        AND state_document_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    state_snapshot_sha256 TEXT NOT NULL CHECK (
        length(state_snapshot_sha256) = 64
        AND state_snapshot_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    knowledge_json TEXT NOT NULL CHECK (
        json_valid(knowledge_json)
        AND json_type(knowledge_json) = 'array'
        AND length(CAST(knowledge_json AS BLOB)) <= 8388608
    ),
    knowledge_sha256 TEXT NOT NULL CHECK (
        length(knowledge_sha256) = 64
        AND knowledge_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    pending_proposal_count INTEGER NOT NULL CHECK (
        pending_proposal_count BETWEEN 0 AND 1024
    ),
    terminal_decision_count INTEGER NOT NULL CHECK (
        terminal_decision_count BETWEEN 0 AND 1024
    ),
    decision_event_ids_json TEXT NOT NULL CHECK (
        json_valid(decision_event_ids_json)
        AND json_type(decision_event_ids_json) = 'array'
        AND length(CAST(decision_event_ids_json AS BLOB)) <= 1048576
    ),
    decision_event_ids_sha256 TEXT NOT NULL CHECK (
        length(decision_event_ids_sha256) = 64
        AND decision_event_ids_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    decision_event_sha256s_json TEXT NOT NULL CHECK (
        json_valid(decision_event_sha256s_json)
        AND json_type(decision_event_sha256s_json) = 'array'
        AND length(CAST(decision_event_sha256s_json AS BLOB)) <= 1048576
    ),
    decision_event_sha256s_sha256 TEXT NOT NULL CHECK (
        length(decision_event_sha256s_sha256) = 64
        AND decision_event_sha256s_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    updated_at TEXT NOT NULL CHECK (length(trim(updated_at)) > 0)
);

CREATE TRIGGER generation_attempt_aggregate_update_guard
BEFORE UPDATE ON generation_attempt_interaction_aggregates
WHEN
    NEW.generation_id != OLD.generation_id
    OR NEW.before_review_sha256 != OLD.before_review_sha256
    OR NEW.created_at != OLD.created_at
    OR NEW.aggregate_revision != OLD.aggregate_revision + 1
    OR NEW.interaction_state_revision < OLD.interaction_state_revision
    OR OLD.pending_proposal_count <= 0
    OR NEW.pending_proposal_count != OLD.pending_proposal_count - 1
    OR NEW.terminal_decision_count != OLD.terminal_decision_count + 1
BEGIN
    SELECT RAISE(ABORT, 'generation attempt aggregate transition is invalid');
END;

CREATE TRIGGER generation_attempt_aggregate_no_delete
BEFORE DELETE ON generation_attempt_interaction_aggregates
BEGIN
    SELECT RAISE(ABORT, 'generation attempt aggregate is immutable audit state');
END;

-- Proposals are reviewed against the immutable BeforeGeneration snapshot but
-- deliberately have no conversation-branch foreign key. They become ordinary
-- branch-local proposals/effects only when the atomic generation append
-- consumes their exact terminal materialization.
CREATE TABLE generation_attempt_proposals (
    proposal_record_id TEXT PRIMARY KEY NOT NULL CHECK (
        length(trim(proposal_record_id)) BETWEEN 1 AND 256
    ),
    generation_id TEXT NOT NULL
        REFERENCES generation_attempt_before_event_snapshots(generation_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    ordinal INTEGER NOT NULL CHECK (ordinal BETWEEN 0 AND 1023),
    before_event_snapshot_sha256 TEXT NOT NULL,
    proposal_id TEXT NOT NULL CHECK (
        length(trim(proposal_id)) BETWEEN 1 AND 256
    ),
    proposal_record_json TEXT NOT NULL CHECK (
        json_valid(proposal_record_json)
        AND json_type(proposal_record_json) = 'object'
        AND length(CAST(proposal_record_json AS BLOB)) <= 1048576
    ),
    proposal_record_sha256 TEXT NOT NULL CHECK (
        length(proposal_record_sha256) = 64
        AND proposal_record_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    proposal_review_sha256 TEXT NOT NULL CHECK (
        length(proposal_review_sha256) = 64
        AND proposal_review_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    origin_policy_json TEXT NOT NULL CHECK (
        json_valid(origin_policy_json)
        AND json_type(origin_policy_json) = 'object'
        AND length(CAST(origin_policy_json AS BLOB)) <= 1048576
    ),
    origin_policy_sha256 TEXT NOT NULL CHECK (
        length(origin_policy_sha256) = 64
        AND origin_policy_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    rule_set_revision_id TEXT NOT NULL,
    rule_id TEXT NOT NULL,
    action_ordinal INTEGER NOT NULL CHECK (action_ordinal >= 0),
    action_payload_json TEXT NOT NULL CHECK (
        json_valid(action_payload_json)
        AND json_type(action_payload_json) = 'object'
        AND length(CAST(action_payload_json AS BLOB)) <= 1048576
    ),
    action_payload_sha256 TEXT NOT NULL CHECK (
        length(action_payload_sha256) = 64
        AND action_payload_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    source_interaction_state_revision INTEGER NOT NULL CHECK (
        source_interaction_state_revision >= 0
    ),
    status TEXT NOT NULL CHECK (
        status IN ('pending', 'approved', 'rejected', 'expired')
    ),
    proposal_revision INTEGER NOT NULL CHECK (proposal_revision >= 1),
    requested_at_epoch_seconds INTEGER NOT NULL,
    expires_at_epoch_seconds INTEGER,
    decision_kind TEXT CHECK (
        decision_kind IS NULL
        OR decision_kind IN ('approved', 'rejected', 'expired')
    ),
    decision_idempotency_key TEXT UNIQUE CHECK (
        decision_idempotency_key IS NULL
        OR length(trim(decision_idempotency_key)) BETWEEN 1 AND 256
    ),
    decision_event_id TEXT CHECK (
        decision_event_id IS NULL
        OR length(trim(decision_event_id)) BETWEEN 1 AND 256
    ),
    decision_event_sha256 TEXT CHECK (
        decision_event_sha256 IS NULL
        OR (
            length(decision_event_sha256) = 64
            AND decision_event_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    ),
    decision_evidence_json TEXT CHECK (
        decision_evidence_json IS NULL
        OR (
            json_valid(decision_evidence_json)
            AND json_type(decision_evidence_json) = 'object'
            AND length(CAST(decision_evidence_json AS BLOB)) <= 1048576
        )
    ),
    decision_evidence_sha256 TEXT CHECK (
        decision_evidence_sha256 IS NULL
        OR (
            length(decision_evidence_sha256) = 64
            AND decision_evidence_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    ),
    resulting_aggregate_revision INTEGER CHECK (
        resulting_aggregate_revision IS NULL
        OR resulting_aggregate_revision >= 2
    ),
    resulting_state_revision INTEGER CHECK (
        resulting_state_revision IS NULL OR resulting_state_revision >= 0
    ),
    resulting_state_json TEXT CHECK (
        resulting_state_json IS NULL
        OR (
            json_valid(resulting_state_json)
            AND json_type(resulting_state_json) = 'object'
            AND length(CAST(resulting_state_json AS BLOB)) <= 8388608
        )
    ),
    resulting_state_snapshot_sha256 TEXT CHECK (
        resulting_state_snapshot_sha256 IS NULL
        OR (
            length(resulting_state_snapshot_sha256) = 64
            AND resulting_state_snapshot_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    ),
    materialization_json TEXT CHECK (
        materialization_json IS NULL
        OR (
            json_valid(materialization_json)
            AND json_type(materialization_json) = 'object'
            AND length(CAST(materialization_json AS BLOB)) <= 8388608
        )
    ),
    materialization_sha256 TEXT CHECK (
        materialization_sha256 IS NULL
        OR (
            length(materialization_sha256) = 64
            AND materialization_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    ),
    decided_at_epoch_seconds INTEGER,
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    updated_at TEXT NOT NULL CHECK (length(trim(updated_at)) > 0),
    UNIQUE (generation_id, ordinal),
    UNIQUE (generation_id, proposal_id),
    UNIQUE (proposal_review_sha256),
    FOREIGN KEY (before_event_snapshot_sha256)
        REFERENCES generation_attempt_before_event_snapshots(review_sha256)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    FOREIGN KEY (rule_set_revision_id, rule_id, action_ordinal)
        REFERENCES interaction_actions(set_revision_id, rule_id, ordinal)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    CHECK (
        expires_at_epoch_seconds IS NULL
        OR expires_at_epoch_seconds > requested_at_epoch_seconds
    ),
    CHECK (
        (decision_event_id IS NULL) = (decision_event_sha256 IS NULL)
    ),
    CHECK (
        (decision_evidence_json IS NULL) = (decision_evidence_sha256 IS NULL)
    ),
    CHECK (
        (materialization_json IS NULL) = (materialization_sha256 IS NULL)
    ),
    CHECK (
        (
            status = 'pending'
            AND proposal_revision = 1
            AND decision_kind IS NULL
            AND decision_idempotency_key IS NULL
            AND decision_event_id IS NULL
            AND decision_evidence_json IS NULL
            AND resulting_aggregate_revision IS NULL
            AND resulting_state_revision IS NULL
            AND resulting_state_json IS NULL
            AND resulting_state_snapshot_sha256 IS NULL
            AND materialization_json IS NULL
            AND decided_at_epoch_seconds IS NULL
        )
        OR (
            status IN ('approved', 'rejected', 'expired')
            AND decision_kind = status
            AND decision_idempotency_key IS NOT NULL
            AND decision_evidence_json IS NOT NULL
            AND resulting_aggregate_revision IS NOT NULL
            AND resulting_state_revision IS NOT NULL
            AND resulting_state_json IS NOT NULL
            AND resulting_state_snapshot_sha256 IS NOT NULL
            AND materialization_json IS NOT NULL
            AND decided_at_epoch_seconds IS NOT NULL
        )
    ),
    CHECK (
        (status = 'approved' AND decision_event_id IS NOT NULL)
        OR (status IN ('pending', 'rejected', 'expired')
            AND decision_event_id IS NULL)
    )
);

CREATE INDEX generation_attempt_proposals_pending
    ON generation_attempt_proposals(
        generation_id,
        expires_at_epoch_seconds,
        ordinal,
        proposal_record_id
    )
    WHERE status = 'pending';

CREATE INDEX generation_attempt_proposals_status
    ON generation_attempt_proposals(
        generation_id,
        status,
        ordinal,
        proposal_record_id
    );

CREATE TRIGGER generation_attempt_proposals_transition_guard
BEFORE UPDATE ON generation_attempt_proposals
WHEN
    NEW.proposal_record_id != OLD.proposal_record_id
    OR NEW.generation_id != OLD.generation_id
    OR NEW.ordinal != OLD.ordinal
    OR NEW.before_event_snapshot_sha256
        != OLD.before_event_snapshot_sha256
    OR NEW.proposal_id != OLD.proposal_id
    OR NEW.proposal_record_json != OLD.proposal_record_json
    OR NEW.proposal_record_sha256 != OLD.proposal_record_sha256
    OR NEW.proposal_review_sha256 != OLD.proposal_review_sha256
    OR NEW.origin_policy_json != OLD.origin_policy_json
    OR NEW.origin_policy_sha256 != OLD.origin_policy_sha256
    OR NEW.rule_set_revision_id != OLD.rule_set_revision_id
    OR NEW.rule_id != OLD.rule_id
    OR NEW.action_ordinal != OLD.action_ordinal
    OR NEW.action_payload_json != OLD.action_payload_json
    OR NEW.action_payload_sha256 != OLD.action_payload_sha256
    OR NEW.source_interaction_state_revision
        != OLD.source_interaction_state_revision
    OR NEW.requested_at_epoch_seconds != OLD.requested_at_epoch_seconds
    OR NEW.expires_at_epoch_seconds IS NOT OLD.expires_at_epoch_seconds
    OR NEW.created_at != OLD.created_at
    OR OLD.status != 'pending'
    OR NEW.status NOT IN ('approved', 'rejected', 'expired')
    OR NEW.proposal_revision != OLD.proposal_revision + 1
BEGIN
    SELECT RAISE(ABORT, 'generation attempt proposal transition is invalid');
END;

CREATE TRIGGER generation_attempt_proposals_no_delete
BEFORE DELETE ON generation_attempt_proposals
BEGIN
    SELECT RAISE(ABORT, 'generation attempt proposals are immutable audit records');
END;
