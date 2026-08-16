PRAGMA foreign_keys = ON;

ALTER TABLE generation_attempt_intents
ADD COLUMN prompt_selection_authority_json TEXT CHECK (
    prompt_selection_authority_json IS NULL
    OR (
        json_valid(prompt_selection_authority_json)
        AND json_type(prompt_selection_authority_json) = 'object'
        AND length(CAST(prompt_selection_authority_json AS BLOB)) <= 8388608
    )
);
ALTER TABLE generation_attempt_intents
ADD COLUMN prompt_selection_authority_sha256 TEXT CHECK (
    prompt_selection_authority_sha256 IS NULL
    OR (
        length(prompt_selection_authority_sha256) = 64
        AND prompt_selection_authority_sha256 NOT GLOB '*[^0-9a-f]*'
    )
);
ALTER TABLE generation_attempt_intents
ADD COLUMN prompt_selection_authority_version INTEGER NOT NULL DEFAULT 0 CHECK (
    prompt_selection_authority_version IN (0, 1)
);

ALTER TABLE generation_attempt_intents
ADD COLUMN module_runtime_review_authority_json TEXT CHECK (
    module_runtime_review_authority_json IS NULL
    OR (
        json_valid(module_runtime_review_authority_json)
        AND json_type(module_runtime_review_authority_json) = 'object'
        AND length(CAST(module_runtime_review_authority_json AS BLOB)) <= 8388608
    )
);
ALTER TABLE generation_attempt_intents
ADD COLUMN module_runtime_review_authority_sha256 TEXT CHECK (
    module_runtime_review_authority_sha256 IS NULL
    OR (
        length(module_runtime_review_authority_sha256) = 64
        AND module_runtime_review_authority_sha256 NOT GLOB '*[^0-9a-f]*'
    )
);
ALTER TABLE generation_attempt_intents
ADD COLUMN applied_runtime_plan_authority_json TEXT CHECK (
    applied_runtime_plan_authority_json IS NULL
    OR (
        json_valid(applied_runtime_plan_authority_json)
        AND json_type(applied_runtime_plan_authority_json) = 'object'
        AND length(CAST(applied_runtime_plan_authority_json AS BLOB)) <= 8388608
    )
);
ALTER TABLE generation_attempt_intents
ADD COLUMN applied_runtime_plan_authority_sha256 TEXT CHECK (
    applied_runtime_plan_authority_sha256 IS NULL
    OR (
        length(applied_runtime_plan_authority_sha256) = 64
        AND applied_runtime_plan_authority_sha256 NOT GLOB '*[^0-9a-f]*'
    )
);
ALTER TABLE generation_attempt_intents
ADD COLUMN module_runtime_authority_version INTEGER NOT NULL DEFAULT 0 CHECK (
    module_runtime_authority_version IN (0, 1)
);

-- Seal every value consulted while evaluating an interaction and preserve the
-- complete bounded derived-event closure inside a generation attempt.  Legacy
-- rows remain readable: nullable evidence is the explicit schema-35 form and
-- is never silently upgraded from current mutable policy inputs.
ALTER TABLE generation_attempt_before_event_snapshots
ADD COLUMN evaluation_seal_json TEXT CHECK (
    evaluation_seal_json IS NULL
    OR (
        json_valid(evaluation_seal_json)
        AND json_type(evaluation_seal_json) = 'object'
        AND length(CAST(evaluation_seal_json AS BLOB)) <= 8388608
    )
);

ALTER TABLE generation_attempt_before_event_snapshots
ADD COLUMN evaluation_seal_sha256 TEXT CHECK (
    evaluation_seal_sha256 IS NULL
    OR (
        length(evaluation_seal_sha256) = 64
        AND evaluation_seal_sha256 NOT GLOB '*[^0-9a-f]*'
    )
);

ALTER TABLE generation_attempt_before_event_snapshots
ADD COLUMN derived_closure_json TEXT CHECK (
    derived_closure_json IS NULL
    OR (
        json_valid(derived_closure_json)
        AND json_type(derived_closure_json) = 'object'
        AND length(CAST(derived_closure_json AS BLOB)) <= 16777216
    )
);

ALTER TABLE generation_attempt_before_event_snapshots
ADD COLUMN derived_closure_sha256 TEXT CHECK (
    derived_closure_sha256 IS NULL
    OR (
        length(derived_closure_sha256) = 64
        AND derived_closure_sha256 NOT GLOB '*[^0-9a-f]*'
    )
);

ALTER TABLE generation_attempt_before_event_snapshots
ADD COLUMN closure_authority_version INTEGER NOT NULL DEFAULT 0 CHECK (
    closure_authority_version IN (0, 1)
);

ALTER TABLE generation_attempt_interaction_aggregates
ADD COLUMN derived_chain_sha256 TEXT CHECK (
    derived_chain_sha256 IS NULL
    OR (
        length(derived_chain_sha256) = 64
        AND derived_chain_sha256 NOT GLOB '*[^0-9a-f]*'
    )
);

ALTER TABLE generation_attempt_interaction_aggregates
ADD COLUMN evaluation_seal_sha256 TEXT CHECK (
    evaluation_seal_sha256 IS NULL
    OR (
        length(evaluation_seal_sha256) = 64
        AND evaluation_seal_sha256 NOT GLOB '*[^0-9a-f]*'
    )
);

ALTER TABLE generation_attempt_interaction_aggregates
ADD COLUMN derived_event_count INTEGER NOT NULL DEFAULT 0 CHECK (
    derived_event_count BETWEEN 0 AND 256
);

ALTER TABLE generation_attempt_interaction_aggregates
ADD COLUMN derived_guard_count INTEGER NOT NULL DEFAULT 0 CHECK (
    derived_guard_count BETWEEN 0 AND 1024
);

ALTER TABLE generation_attempt_interaction_aggregates
ADD COLUMN closure_authority_version INTEGER NOT NULL DEFAULT 0 CHECK (
    closure_authority_version IN (0, 1)
);

ALTER TABLE generation_attempt_proposals
ADD COLUMN origin_event_id TEXT CHECK (
    origin_event_id IS NULL OR length(trim(origin_event_id)) BETWEEN 1 AND 256
);

ALTER TABLE generation_attempt_proposals
ADD COLUMN origin_chain_ordinal INTEGER CHECK (
    origin_chain_ordinal IS NULL OR origin_chain_ordinal BETWEEN 0 AND 256
);

ALTER TABLE generation_attempt_proposals
ADD COLUMN origin_aggregate_revision INTEGER CHECK (
    origin_aggregate_revision IS NULL OR origin_aggregate_revision >= 1
);

ALTER TABLE generation_attempt_proposals
ADD COLUMN origin_evaluation_seal_json TEXT CHECK (
    origin_evaluation_seal_json IS NULL
    OR (
        json_valid(origin_evaluation_seal_json)
        AND json_type(origin_evaluation_seal_json) = 'object'
        AND length(CAST(origin_evaluation_seal_json AS BLOB)) <= 8388608
    )
);

ALTER TABLE generation_attempt_proposals
ADD COLUMN origin_evaluation_seal_sha256 TEXT CHECK (
    origin_evaluation_seal_sha256 IS NULL
    OR (
        length(origin_evaluation_seal_sha256) = 64
        AND origin_evaluation_seal_sha256 NOT GLOB '*[^0-9a-f]*'
    )
);

ALTER TABLE generation_attempt_proposals
ADD COLUMN resulting_derived_chain_sha256 TEXT CHECK (
    resulting_derived_chain_sha256 IS NULL
    OR (status != 'pending'
        AND
        length(resulting_derived_chain_sha256) = 64
        AND resulting_derived_chain_sha256 NOT GLOB '*[^0-9a-f]*'
    )
);
ALTER TABLE generation_attempt_proposals
ADD COLUMN resulting_derived_event_count INTEGER CHECK (
    resulting_derived_event_count IS NULL
    OR (status != 'pending' AND resulting_derived_event_count BETWEEN 0 AND 256)
);
ALTER TABLE generation_attempt_proposals
ADD COLUMN resulting_derived_guard_count INTEGER CHECK (
    resulting_derived_guard_count IS NULL
    OR (status != 'pending' AND resulting_derived_guard_count BETWEEN 0 AND 1024)
);
ALTER TABLE generation_attempt_proposals
ADD COLUMN resulting_pending_proposal_count INTEGER CHECK (
    resulting_pending_proposal_count IS NULL
    OR (status != 'pending' AND resulting_pending_proposal_count BETWEEN 0 AND 1024)
);

ALTER TABLE generation_attempt_aggregate_decision_bindings
ADD COLUMN resulting_derived_chain_sha256 TEXT CHECK (
    resulting_derived_chain_sha256 IS NULL
    OR (
        length(resulting_derived_chain_sha256) = 64
        AND resulting_derived_chain_sha256 NOT GLOB '*[^0-9a-f]*'
    )
);
ALTER TABLE generation_attempt_aggregate_decision_bindings
ADD COLUMN resulting_derived_event_count INTEGER CHECK (
    resulting_derived_event_count IS NULL
    OR resulting_derived_event_count BETWEEN 0 AND 256
);
ALTER TABLE generation_attempt_aggregate_decision_bindings
ADD COLUMN resulting_derived_guard_count INTEGER CHECK (
    resulting_derived_guard_count IS NULL
    OR resulting_derived_guard_count BETWEEN 0 AND 1024
);
ALTER TABLE generation_attempt_aggregate_decision_bindings
ADD COLUMN resulting_pending_proposal_count INTEGER CHECK (
    resulting_pending_proposal_count IS NULL
    OR resulting_pending_proposal_count BETWEEN 0 AND 1024
);

ALTER TABLE generation_attempt_proposal_decision_commits
ADD COLUMN resulting_derived_chain_sha256 TEXT CHECK (
    resulting_derived_chain_sha256 IS NULL
    OR (
        length(resulting_derived_chain_sha256) = 64
        AND resulting_derived_chain_sha256 NOT GLOB '*[^0-9a-f]*'
    )
);
ALTER TABLE generation_attempt_proposal_decision_commits
ADD COLUMN resulting_derived_event_count INTEGER CHECK (
    resulting_derived_event_count IS NULL
    OR resulting_derived_event_count BETWEEN 0 AND 256
);
ALTER TABLE generation_attempt_proposal_decision_commits
ADD COLUMN resulting_derived_guard_count INTEGER CHECK (
    resulting_derived_guard_count IS NULL
    OR resulting_derived_guard_count BETWEEN 0 AND 1024
);
ALTER TABLE generation_attempt_proposal_decision_commits
ADD COLUMN resulting_pending_proposal_count INTEGER CHECK (
    resulting_pending_proposal_count IS NULL
    OR resulting_pending_proposal_count BETWEEN 0 AND 1024
);

ALTER TABLE interaction_events
ADD COLUMN evaluation_seal_json TEXT CHECK (
    evaluation_seal_json IS NULL
    OR (
        json_valid(evaluation_seal_json)
        AND json_type(evaluation_seal_json) = 'object'
        AND length(CAST(evaluation_seal_json AS BLOB)) <= 8388608
    )
);
ALTER TABLE interaction_events ADD COLUMN evaluation_seal_sha256 TEXT CHECK (
    evaluation_seal_sha256 IS NULL
    OR (
        length(evaluation_seal_sha256) = 64
        AND evaluation_seal_sha256 NOT GLOB '*[^0-9a-f]*'
    )
);
ALTER TABLE interaction_events
ADD COLUMN evaluation_seal_version INTEGER NOT NULL DEFAULT 0 CHECK (
    evaluation_seal_version IN (0, 1)
);

DROP TRIGGER interaction_derived_event_outbox_identity_guard;
ALTER TABLE interaction_derived_event_outbox
ADD COLUMN evaluation_seal_json TEXT CHECK (
    evaluation_seal_json IS NULL
    OR (
        json_valid(evaluation_seal_json)
        AND json_type(evaluation_seal_json) = 'object'
        AND length(CAST(evaluation_seal_json AS BLOB)) <= 8388608
    )
);
ALTER TABLE interaction_derived_event_outbox
ADD COLUMN evaluation_seal_sha256 TEXT CHECK (
    evaluation_seal_sha256 IS NULL
    OR (
        length(evaluation_seal_sha256) = 64
        AND evaluation_seal_sha256 NOT GLOB '*[^0-9a-f]*'
    )
);
ALTER TABLE interaction_derived_event_outbox
ADD COLUMN evaluation_seal_version INTEGER NOT NULL DEFAULT 0 CHECK (
    evaluation_seal_version IN (0, 1)
);
-- SQLite INTEGER is signed.  A fixed-width lowercase hexadecimal string is
-- the lossless canonical representation of the engine's full u64 seed.
ALTER TABLE interaction_derived_event_outbox
ADD COLUMN deterministic_seed_hex TEXT CHECK (
    deterministic_seed_hex IS NULL
    OR (
        length(deterministic_seed_hex) = 16
        AND deterministic_seed_hex NOT GLOB '*[^0-9a-f]*'
    )
);

CREATE TRIGGER interaction_derived_event_outbox_identity_guard
BEFORE UPDATE ON interaction_derived_event_outbox
WHEN
    NEW.occurrence_id != OLD.occurrence_id
    OR NEW.chain_id != OLD.chain_id
    OR NEW.root_event_id != OLD.root_event_id
    OR NEW.parent_event_id != OLD.parent_event_id
    OR NEW.parent_occurrence_id IS NOT OLD.parent_occurrence_id
    OR NEW.conversation_id != OLD.conversation_id
    OR NEW.branch_id != OLD.branch_id
    OR NEW.depth != OLD.depth
    OR NEW.chain_ordinal != OLD.chain_ordinal
    OR NEW.source_effect_ordinal != OLD.source_effect_ordinal
    OR NEW.parent_event_commit_sha256 != OLD.parent_event_commit_sha256
    OR NEW.parent_resulting_state_revision != OLD.parent_resulting_state_revision
    OR NEW.source_effect_sha256 != OLD.source_effect_sha256
    OR NEW.source_action_sha256 != OLD.source_action_sha256
    OR NEW.source_set_revision_id != OLD.source_set_revision_id
    OR NEW.source_rule_id != OLD.source_rule_id
    OR NEW.source_action_ordinal != OLD.source_action_ordinal
    OR NEW.event_kind != OLD.event_kind
    OR NEW.event_argument_json != OLD.event_argument_json
    OR NEW.event_sha256 != OLD.event_sha256
    OR NEW.visited_event_sha256s_json != OLD.visited_event_sha256s_json
    OR NEW.policy_json != OLD.policy_json
    OR NEW.policy_sha256 != OLD.policy_sha256
    OR NEW.evaluation_seal_json IS NOT OLD.evaluation_seal_json
    OR NEW.evaluation_seal_sha256 IS NOT OLD.evaluation_seal_sha256
    OR NEW.evaluation_seal_version IS NOT OLD.evaluation_seal_version
    OR NEW.deterministic_seed_hex IS NOT OLD.deterministic_seed_hex
    OR NEW.occurred_at != OLD.occurred_at
    OR NEW.created_at != OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'derived interaction occurrence identity is immutable');
END;

-- Active schema-35 attempts cannot reconstruct the values that were consulted
-- by their original evaluation.  Record an immutable audit and force an
-- explicit regenerate instead of mixing old evidence with current state.
CREATE TABLE generation_attempt_legacy_closure_interruptions (
    generation_id TEXT PRIMARY KEY NOT NULL
        REFERENCES generation_attempt_intents(generation_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    prior_status TEXT NOT NULL CHECK (
        prior_status IN (
            'prepared',
            'before_generation_applied',
            'awaiting_approval',
            'dispatch_ready'
        )
    ),
    prior_revision INTEGER NOT NULL CHECK (prior_revision >= 1),
    attempt_sha256 TEXT NOT NULL CHECK (
        length(attempt_sha256) = 64
        AND attempt_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    before_generation_evidence_sha256 TEXT CHECK (
        before_generation_evidence_sha256 IS NULL
        OR (
            length(before_generation_evidence_sha256) = 64
            AND before_generation_evidence_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    ),
    approval_evidence_sha256 TEXT CHECK (
        approval_evidence_sha256 IS NULL
        OR (
            length(approval_evidence_sha256) = 64
            AND approval_evidence_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    ),
    dispatch_seal_sha256 TEXT CHECK (
        dispatch_seal_sha256 IS NULL
        OR (
            length(dispatch_seal_sha256) = 64
            AND dispatch_seal_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    ),
    reason_kind TEXT NOT NULL CHECK (
        reason_kind = 'stale_generation_derived_closure_authority'
    ),
    recorded_at TEXT NOT NULL CHECK (length(trim(recorded_at)) > 0)
);

CREATE TRIGGER generation_attempt_legacy_closure_interruption_no_update
BEFORE UPDATE ON generation_attempt_legacy_closure_interruptions
BEGIN
    SELECT RAISE(ABORT, 'generation attempt legacy closure audit is immutable');
END;

CREATE TRIGGER generation_attempt_legacy_closure_interruption_no_delete
BEFORE DELETE ON generation_attempt_legacy_closure_interruptions
BEGIN
    SELECT RAISE(ABORT, 'generation attempt legacy closure audit is immutable');
END;

INSERT INTO generation_attempt_legacy_closure_interruptions (
    generation_id, prior_status, prior_revision, attempt_sha256,
    before_generation_evidence_sha256, approval_evidence_sha256,
    dispatch_seal_sha256, reason_kind, recorded_at
)
SELECT
    generation_id,
    status,
    revision,
    attempt_sha256,
    before_generation_evidence_sha256,
    approval_evidence_sha256,
    dispatch_seal_sha256,
    'stale_generation_derived_closure_authority',
    updated_at
FROM generation_attempt_intents
WHERE status IN (
    'prepared',
    'before_generation_applied',
    'awaiting_approval',
    'dispatch_ready'
);

DROP TRIGGER generation_attempt_intents_transition_guard;
UPDATE generation_attempt_intents
SET status = 'failed_before_dispatch',
    failure_code = 'stale_generation_derived_closure_authority',
    revision = revision + 1,
    updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
WHERE status IN (
    'prepared',
    'before_generation_applied',
    'awaiting_approval',
    'dispatch_ready'
);

CREATE TRIGGER generation_attempt_intents_transition_guard
BEFORE UPDATE ON generation_attempt_intents
WHEN
    NOT (
        (OLD.status = 'prepared'
            AND NEW.status IN (
                'before_generation_applied',
                'awaiting_approval',
                'failed_before_dispatch'
            ))
        OR (OLD.status = 'before_generation_applied'
            AND NEW.status IN (
                'dispatch_ready',
                'awaiting_approval',
                'failed_before_dispatch'
            ))
        OR (OLD.status = 'awaiting_approval'
            AND NEW.status IN (
                'before_generation_applied',
                'failed_before_dispatch'
            ))
        OR (OLD.status = 'dispatch_ready' AND NEW.status = 'running')
        OR (OLD.status = 'running' AND NEW.status = 'completed')
        OR (OLD.status = 'failed_before_dispatch'
            AND NEW.status IN (
                'prepared',
                'before_generation_applied',
                'awaiting_approval'
            )
            AND OLD.prompt_selection_authority_version = 1
            AND OLD.prompt_selection_authority_json IS NOT NULL
            AND OLD.prompt_selection_authority_sha256 IS NOT NULL
            AND OLD.module_runtime_authority_version = 1
            AND OLD.module_runtime_review_authority_json IS NOT NULL
            AND OLD.module_runtime_review_authority_sha256 IS NOT NULL
            AND NOT EXISTS (
                SELECT 1
                FROM generation_attempt_legacy_closure_interruptions AS legacy
                WHERE legacy.generation_id = OLD.generation_id
            ))
    )
BEGIN
    SELECT RAISE(ABORT, 'generation attempt transition is invalid');
END;

CREATE TRIGGER generation_attempt_before_closure_insert_guard
BEFORE INSERT ON generation_attempt_before_event_snapshots
WHEN NEW.closure_authority_version != 1
  OR NEW.evaluation_seal_json IS NULL
  OR NEW.evaluation_seal_sha256 IS NULL
  OR NEW.derived_closure_json IS NULL
  OR NEW.derived_closure_sha256 IS NULL
BEGIN
    SELECT RAISE(ABORT, 'generation attempt derived closure authority is incomplete');
END;

CREATE TRIGGER generation_attempt_prompt_selection_insert_guard
BEFORE INSERT ON generation_attempt_intents
WHEN NEW.prompt_selection_authority_version != 1
  OR NEW.prompt_selection_authority_json IS NULL
  OR NEW.prompt_selection_authority_sha256 IS NULL
BEGIN
    SELECT RAISE(ABORT, 'generation attempt prompt selection authority is incomplete');
END;

CREATE TRIGGER generation_attempt_prompt_selection_update_guard
BEFORE UPDATE ON generation_attempt_intents
WHEN NEW.prompt_selection_authority_version
        IS NOT OLD.prompt_selection_authority_version
  OR NEW.prompt_selection_authority_json
        IS NOT OLD.prompt_selection_authority_json
  OR NEW.prompt_selection_authority_sha256
        IS NOT OLD.prompt_selection_authority_sha256
BEGIN
    SELECT RAISE(ABORT, 'generation attempt prompt selection authority is immutable');
END;

CREATE TRIGGER generation_attempt_module_runtime_authority_insert_guard
BEFORE INSERT ON generation_attempt_intents
WHEN NEW.module_runtime_authority_version != 1
  OR NEW.module_runtime_review_authority_json IS NULL
  OR NEW.module_runtime_review_authority_sha256 IS NULL
  OR (NEW.applied_runtime_plan_authority_json IS NULL)
        != (NEW.applied_runtime_plan_authority_sha256 IS NULL)
BEGIN
    SELECT RAISE(ABORT, 'generation attempt module runtime authority is incomplete');
END;

CREATE TRIGGER generation_attempt_module_runtime_authority_update_guard
BEFORE UPDATE ON generation_attempt_intents
WHEN NEW.module_runtime_authority_version
        IS NOT OLD.module_runtime_authority_version
  OR NEW.module_runtime_review_authority_json
        IS NOT OLD.module_runtime_review_authority_json
  OR NEW.module_runtime_review_authority_sha256
        IS NOT OLD.module_runtime_review_authority_sha256
  OR NEW.applied_runtime_plan_authority_json
        IS NOT OLD.applied_runtime_plan_authority_json
  OR NEW.applied_runtime_plan_authority_sha256
        IS NOT OLD.applied_runtime_plan_authority_sha256
BEGIN
    SELECT RAISE(ABORT, 'generation attempt module runtime authority is immutable');
END;

DROP TRIGGER generation_attempt_aggregate_insert_guard_v2;
CREATE TRIGGER generation_attempt_aggregate_insert_guard_v2
BEFORE INSERT ON generation_attempt_interaction_aggregates
WHEN NEW.aggregate_revision != 1
  OR NEW.terminal_decision_count != 0
  OR NEW.closure_authority_version != 1
  OR NEW.derived_chain_sha256 IS NULL
  OR NEW.evaluation_seal_sha256 IS NULL
BEGIN
    SELECT RAISE(ABORT, 'generation attempt aggregate must begin with sealed closure authority');
END;

CREATE TRIGGER generation_attempt_proposal_origin_insert_guard
BEFORE INSERT ON generation_attempt_proposals
WHEN NEW.origin_event_id IS NULL
  OR NEW.origin_chain_ordinal IS NULL
  OR NEW.origin_aggregate_revision IS NULL
  OR NEW.origin_evaluation_seal_json IS NULL
  OR NEW.origin_evaluation_seal_sha256 IS NULL
BEGIN
    SELECT RAISE(ABORT, 'generation attempt proposal origin authority is incomplete');
END;

DROP TRIGGER generation_attempt_proposals_transition_guard;
CREATE TRIGGER generation_attempt_proposals_transition_guard
BEFORE UPDATE ON generation_attempt_proposals
WHEN
    NEW.proposal_record_id != OLD.proposal_record_id
    OR NEW.domain_proposal_record_id != OLD.domain_proposal_record_id
    OR NEW.generation_id != OLD.generation_id
    OR NEW.ordinal != OLD.ordinal
    OR NEW.before_event_snapshot_sha256
        != OLD.before_event_snapshot_sha256
    OR NEW.proposal_id != OLD.proposal_id
    OR NEW.proposal_record_json != OLD.proposal_record_json
    OR NEW.proposal_record_sha256 != OLD.proposal_record_sha256
    OR NEW.proposal_review_sha256 != OLD.proposal_review_sha256
    OR NEW.domain_proposal_review_sha256
        != OLD.domain_proposal_review_sha256
    OR NEW.storage_identity_version != OLD.storage_identity_version
    OR NEW.origin_policy_json != OLD.origin_policy_json
    OR NEW.origin_policy_sha256 != OLD.origin_policy_sha256
    OR NEW.origin_event_id IS NOT OLD.origin_event_id
    OR NEW.origin_chain_ordinal IS NOT OLD.origin_chain_ordinal
    OR NEW.origin_aggregate_revision IS NOT OLD.origin_aggregate_revision
    OR NEW.origin_evaluation_seal_json IS NOT OLD.origin_evaluation_seal_json
    OR NEW.origin_evaluation_seal_sha256
        IS NOT OLD.origin_evaluation_seal_sha256
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
    OR NEW.resulting_derived_chain_sha256 IS NULL
    OR NEW.resulting_derived_event_count IS NULL
    OR NEW.resulting_derived_guard_count IS NULL
    OR NEW.resulting_pending_proposal_count IS NULL
BEGIN
    SELECT RAISE(ABORT, 'generation attempt proposal transition is invalid');
END;

DROP TRIGGER generation_attempt_aggregate_update_guard;
CREATE TRIGGER generation_attempt_aggregate_update_guard
BEFORE UPDATE ON generation_attempt_interaction_aggregates
WHEN
    NEW.generation_id != OLD.generation_id
    OR NEW.before_review_sha256 != OLD.before_review_sha256
    OR NEW.created_at != OLD.created_at
    OR NEW.aggregate_revision != OLD.aggregate_revision + 1
    OR NEW.interaction_state_revision < OLD.interaction_state_revision
    OR OLD.pending_proposal_count <= 0
    OR NEW.terminal_decision_count != OLD.terminal_decision_count + 1
    OR NEW.pending_proposal_count != (
        SELECT COUNT(*)
        FROM generation_attempt_proposals AS proposal
        WHERE proposal.generation_id = NEW.generation_id
          AND proposal.status = 'pending'
    )
    OR NEW.terminal_decision_count != (
        SELECT COUNT(*)
        FROM generation_attempt_proposals AS proposal
        WHERE proposal.generation_id = NEW.generation_id
          AND proposal.status != 'pending'
    )
    OR NEW.closure_authority_version != OLD.closure_authority_version
    OR NEW.evaluation_seal_sha256 IS NOT OLD.evaluation_seal_sha256
    OR NEW.derived_chain_sha256 IS NULL
    OR NEW.derived_event_count < OLD.derived_event_count
    OR NEW.derived_guard_count < OLD.derived_guard_count
BEGIN
    SELECT RAISE(ABORT, 'generation attempt aggregate transition is invalid');
END;

DROP TRIGGER generation_attempt_decision_binding_insert_guard;
CREATE TRIGGER generation_attempt_decision_binding_insert_guard
BEFORE INSERT ON generation_attempt_aggregate_decision_bindings
WHEN NEW.resulting_derived_chain_sha256 IS NULL
  OR NEW.resulting_derived_event_count IS NULL
  OR NEW.resulting_derived_guard_count IS NULL
  OR NEW.resulting_pending_proposal_count IS NULL
  OR NOT EXISTS (
    SELECT 1
    FROM generation_attempt_proposals AS proposal
    JOIN generation_attempt_interaction_aggregates AS aggregate
      ON aggregate.generation_id = proposal.generation_id
    WHERE proposal.proposal_record_id = NEW.proposal_record_id
      AND proposal.generation_id = NEW.generation_id
      AND proposal.status = NEW.decision_kind
      AND proposal.decision_kind = NEW.decision_kind
      AND proposal.proposal_revision = 2
      AND proposal.resulting_aggregate_revision = NEW.aggregate_revision
      AND proposal.resulting_state_revision = NEW.interaction_state_revision
      AND proposal.resulting_state_snapshot_sha256 = NEW.state_snapshot_sha256
      AND proposal.resulting_derived_chain_sha256
            = NEW.resulting_derived_chain_sha256
      AND proposal.resulting_derived_event_count
            = NEW.resulting_derived_event_count
      AND proposal.resulting_derived_guard_count
            = NEW.resulting_derived_guard_count
      AND proposal.resulting_pending_proposal_count
            = NEW.resulting_pending_proposal_count
      AND proposal.decision_idempotency_key = NEW.decision_idempotency_key
      AND proposal.updated_at = NEW.decision_updated_at
      AND aggregate.aggregate_revision = NEW.aggregate_revision
      AND aggregate.interaction_state_revision = NEW.interaction_state_revision
      AND aggregate.state_snapshot_sha256 = NEW.state_snapshot_sha256
      AND aggregate.derived_chain_sha256 = NEW.resulting_derived_chain_sha256
      AND aggregate.derived_event_count = NEW.resulting_derived_event_count
      AND aggregate.derived_guard_count = NEW.resulting_derived_guard_count
      AND aggregate.pending_proposal_count
            = NEW.resulting_pending_proposal_count
      AND aggregate.updated_at = NEW.decision_updated_at
)
BEGIN
    SELECT RAISE(
        ABORT,
        'generation attempt aggregate decision binding is detached'
    );
END;

DROP TRIGGER generation_attempt_decision_commit_insert_guard;
CREATE TRIGGER generation_attempt_decision_commit_insert_guard
BEFORE INSERT ON generation_attempt_proposal_decision_commits
WHEN NEW.resulting_derived_chain_sha256 IS NULL
  OR NEW.resulting_derived_event_count IS NULL
  OR NEW.resulting_derived_guard_count IS NULL
  OR NEW.resulting_pending_proposal_count IS NULL
  OR NOT EXISTS (
    SELECT 1
    FROM generation_attempt_proposals AS proposal
    WHERE proposal.proposal_record_id = NEW.proposal_record_id
      AND proposal.generation_id = NEW.generation_id
      AND proposal.status = NEW.decision_kind
      AND proposal.decision_kind = NEW.decision_kind
      AND proposal.proposal_revision = NEW.proposal_revision
      AND proposal.resulting_aggregate_revision
            = NEW.resulting_aggregate_revision
      AND proposal.resulting_state_revision = NEW.resulting_state_revision
      AND proposal.resulting_state_snapshot_sha256
            = NEW.resulting_state_snapshot_sha256
      AND proposal.resulting_derived_chain_sha256
            = NEW.resulting_derived_chain_sha256
      AND proposal.resulting_derived_event_count
            = NEW.resulting_derived_event_count
      AND proposal.resulting_derived_guard_count
            = NEW.resulting_derived_guard_count
      AND proposal.resulting_pending_proposal_count
            = NEW.resulting_pending_proposal_count
      AND proposal.decision_idempotency_key = NEW.decision_idempotency_key
      AND proposal.updated_at = NEW.decision_updated_at
)
BEGIN
    SELECT RAISE(
        ABORT,
        'generation attempt proposal decision commit is detached'
    );
END;

DROP TRIGGER generation_attempt_proposal_decision_commit;
CREATE TRIGGER generation_attempt_proposal_decision_commit
AFTER UPDATE OF
    status,
    proposal_revision,
    decision_kind,
    decision_idempotency_key,
    resulting_aggregate_revision,
    resulting_state_revision,
    resulting_state_snapshot_sha256,
    resulting_derived_chain_sha256,
    resulting_derived_event_count,
    resulting_derived_guard_count,
    resulting_pending_proposal_count,
    updated_at
ON generation_attempt_proposals
WHEN OLD.status = 'pending' AND NEW.status != 'pending'
BEGIN
    INSERT INTO generation_attempt_proposal_decision_commits (
        proposal_record_id,
        generation_id,
        resulting_aggregate_revision,
        resulting_state_revision,
        resulting_state_snapshot_sha256,
        decision_kind,
        decision_idempotency_key,
        proposal_revision,
        decision_updated_at,
        resulting_derived_chain_sha256,
        resulting_derived_event_count,
        resulting_derived_guard_count,
        resulting_pending_proposal_count
    ) VALUES (
        NEW.proposal_record_id,
        NEW.generation_id,
        NEW.resulting_aggregate_revision,
        NEW.resulting_state_revision,
        NEW.resulting_state_snapshot_sha256,
        NEW.decision_kind,
        NEW.decision_idempotency_key,
        NEW.proposal_revision,
        NEW.updated_at,
        NEW.resulting_derived_chain_sha256,
        NEW.resulting_derived_event_count,
        NEW.resulting_derived_guard_count,
        NEW.resulting_pending_proposal_count
    );
END;

DROP TRIGGER generation_attempt_aggregate_decision_bind;
CREATE TRIGGER generation_attempt_aggregate_decision_bind
AFTER UPDATE OF
    aggregate_revision,
    interaction_state_revision,
    state_snapshot_sha256,
    derived_chain_sha256,
    derived_event_count,
    derived_guard_count,
    pending_proposal_count,
    terminal_decision_count,
    updated_at
ON generation_attempt_interaction_aggregates
BEGIN
    SELECT RAISE(
        ABORT,
        'generation attempt aggregate update has no exact proposal decision'
    )
    WHERE NOT EXISTS (
        SELECT 1
        FROM generation_attempt_proposal_decision_commits AS decision
        WHERE decision.generation_id = NEW.generation_id
          AND decision.resulting_aggregate_revision = NEW.aggregate_revision
          AND decision.resulting_state_revision = NEW.interaction_state_revision
          AND decision.resulting_state_snapshot_sha256 = NEW.state_snapshot_sha256
          AND decision.resulting_derived_chain_sha256 = NEW.derived_chain_sha256
          AND decision.resulting_derived_event_count = NEW.derived_event_count
          AND decision.resulting_derived_guard_count = NEW.derived_guard_count
          AND decision.resulting_pending_proposal_count
                = NEW.pending_proposal_count
          AND decision.decision_updated_at = NEW.updated_at
    );

    INSERT INTO generation_attempt_aggregate_decision_bindings (
        generation_id,
        aggregate_revision,
        proposal_record_id,
        interaction_state_revision,
        state_snapshot_sha256,
        decision_kind,
        decision_idempotency_key,
        decision_updated_at,
        resulting_derived_chain_sha256,
        resulting_derived_event_count,
        resulting_derived_guard_count,
        resulting_pending_proposal_count
    )
    SELECT
        decision.generation_id,
        decision.resulting_aggregate_revision,
        decision.proposal_record_id,
        decision.resulting_state_revision,
        decision.resulting_state_snapshot_sha256,
        decision.decision_kind,
        decision.decision_idempotency_key,
        decision.decision_updated_at,
        decision.resulting_derived_chain_sha256,
        decision.resulting_derived_event_count,
        decision.resulting_derived_guard_count,
        decision.resulting_pending_proposal_count
    FROM generation_attempt_proposal_decision_commits AS decision
    WHERE decision.generation_id = NEW.generation_id
      AND decision.resulting_aggregate_revision = NEW.aggregate_revision
      AND decision.resulting_state_revision = NEW.interaction_state_revision
      AND decision.resulting_state_snapshot_sha256 = NEW.state_snapshot_sha256
      AND decision.resulting_derived_chain_sha256 = NEW.derived_chain_sha256
      AND decision.resulting_derived_event_count = NEW.derived_event_count
      AND decision.resulting_derived_guard_count = NEW.derived_guard_count
      AND decision.resulting_pending_proposal_count = NEW.pending_proposal_count
      AND decision.decision_updated_at = NEW.updated_at;
END;

CREATE TRIGGER interaction_event_evaluation_seal_insert_guard
BEFORE INSERT ON interaction_events
WHEN NOT (
    (NEW.evaluation_seal_version = 0
        AND NEW.evaluation_seal_json IS NULL
        AND NEW.evaluation_seal_sha256 IS NULL)
    OR (NEW.evaluation_seal_version = 1
        AND NEW.evaluation_seal_json IS NOT NULL
        AND NEW.evaluation_seal_sha256 IS NOT NULL)
)
BEGIN
    SELECT RAISE(ABORT, 'interaction event evaluation seal is incomplete');
END;

CREATE TRIGGER interaction_derived_event_outbox_seal_insert_guard
BEFORE INSERT ON interaction_derived_event_outbox
WHEN NEW.evaluation_seal_version != 1
  OR NEW.evaluation_seal_json IS NULL
  OR NEW.evaluation_seal_sha256 IS NULL
  OR NEW.deterministic_seed_hex IS NULL
BEGIN
    SELECT RAISE(ABORT, 'derived interaction occurrence evaluation seal is incomplete');
END;
