-- Durable, bounded dispatch of state-derived interaction events.
--
-- VariableChanged and KnowledgeActivated are never accepted from a caller.
-- They are derived from an exact committed action/effect pair and processed
-- through this restart-safe outbox. The immutable chain evidence lets Storage
-- reject retargeting and lets tests/audits distinguish cycle/depth/count
-- suppression from successful delivery.

CREATE TABLE interaction_derived_event_outbox (
    occurrence_id TEXT PRIMARY KEY NOT NULL CHECK (
        length(trim(occurrence_id)) BETWEEN 1 AND 256
    ),
    chain_id TEXT NOT NULL CHECK (length(trim(chain_id)) BETWEEN 1 AND 256),
    root_event_id TEXT NOT NULL
        REFERENCES interaction_events(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    parent_event_id TEXT NOT NULL
        REFERENCES interaction_events(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    parent_occurrence_id TEXT
        REFERENCES interaction_derived_event_outbox(occurrence_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    conversation_id TEXT NOT NULL,
    branch_id TEXT NOT NULL,
    depth INTEGER NOT NULL CHECK (depth BETWEEN 1 AND 16),
    chain_ordinal INTEGER NOT NULL CHECK (chain_ordinal BETWEEN 1 AND 256),
    source_effect_ordinal INTEGER NOT NULL CHECK (source_effect_ordinal >= 0),
    parent_event_commit_sha256 TEXT NOT NULL CHECK (
        length(parent_event_commit_sha256) = 64
        AND parent_event_commit_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    source_effect_sha256 TEXT NOT NULL CHECK (
        length(source_effect_sha256) = 64
        AND source_effect_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    source_action_sha256 TEXT NOT NULL CHECK (
        length(source_action_sha256) = 64
        AND source_action_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    source_set_revision_id TEXT NOT NULL,
    source_rule_id TEXT NOT NULL,
    source_action_ordinal INTEGER NOT NULL CHECK (source_action_ordinal >= 0),
    event_kind TEXT NOT NULL CHECK (
        event_kind IN ('variable_changed', 'knowledge_activated')
    ),
    event_argument_json TEXT NOT NULL CHECK (
        json_valid(event_argument_json)
        AND json_type(event_argument_json) = 'object'
        AND length(CAST(event_argument_json AS BLOB)) <= 262144
    ),
    event_sha256 TEXT NOT NULL CHECK (
        length(event_sha256) = 64
        AND event_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    visited_event_sha256s_json TEXT NOT NULL CHECK (
        json_valid(visited_event_sha256s_json)
        AND json_type(visited_event_sha256s_json) = 'array'
        AND json_array_length(visited_event_sha256s_json) BETWEEN 1 AND 16
        AND length(CAST(visited_event_sha256s_json AS BLOB)) <= 4096
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
    occurred_at TEXT NOT NULL CHECK (length(trim(occurred_at)) > 0),
    available_at TEXT NOT NULL CHECK (length(trim(available_at)) > 0),
    status TEXT NOT NULL CHECK (status IN ('pending', 'claimed', 'acknowledged')),
    delivery_attempts INTEGER NOT NULL DEFAULT 0 CHECK (delivery_attempts >= 0),
    lease_until TEXT,
    acknowledged_at TEXT,
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    FOREIGN KEY (conversation_id, branch_id)
        REFERENCES conversation_branches(conversation_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    FOREIGN KEY (source_set_revision_id, source_rule_id, source_action_ordinal)
        REFERENCES interaction_actions(set_revision_id, rule_id, ordinal)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    UNIQUE (chain_id, chain_ordinal),
    UNIQUE (parent_event_id, source_effect_ordinal),
    CHECK (
        (status = 'pending' AND lease_until IS NULL AND acknowledged_at IS NULL)
        OR (status = 'claimed' AND lease_until IS NOT NULL AND acknowledged_at IS NULL)
        OR (status = 'acknowledged' AND lease_until IS NULL AND acknowledged_at IS NOT NULL)
    )
);

CREATE INDEX interaction_derived_event_outbox_delivery
    ON interaction_derived_event_outbox(
        status, available_at, lease_until, chain_id, chain_ordinal
    )
    WHERE status != 'acknowledged';

CREATE INDEX interaction_derived_event_outbox_branch
    ON interaction_derived_event_outbox(
        conversation_id, branch_id, chain_id, chain_ordinal
    );

CREATE TABLE interaction_derived_event_guard_audit (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(trim(id)) BETWEEN 1 AND 256),
    chain_id TEXT NOT NULL CHECK (length(trim(chain_id)) BETWEEN 1 AND 256),
    root_event_id TEXT NOT NULL
        REFERENCES interaction_events(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    parent_event_id TEXT NOT NULL
        REFERENCES interaction_events(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    parent_occurrence_id TEXT
        REFERENCES interaction_derived_event_outbox(occurrence_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    guard_kind TEXT NOT NULL CHECK (
        guard_kind IN ('cycle', 'depth_limit', 'count_limit')
    ),
    candidate_event_sha256 TEXT CHECK (
        candidate_event_sha256 IS NULL
        OR (
            length(candidate_event_sha256) = 64
            AND candidate_event_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    ),
    suppressed_count INTEGER NOT NULL CHECK (suppressed_count BETWEEN 1 AND 1024),
    evidence_json TEXT NOT NULL CHECK (
        json_valid(evidence_json)
        AND json_type(evidence_json) = 'object'
        AND length(CAST(evidence_json AS BLOB)) <= 262144
    ),
    evidence_sha256 TEXT NOT NULL CHECK (
        length(evidence_sha256) = 64
        AND evidence_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    UNIQUE (chain_id, parent_event_id, guard_kind, candidate_event_sha256)
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
    OR NEW.occurred_at != OLD.occurred_at
    OR NEW.created_at != OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'derived interaction occurrence identity is immutable');
END;

CREATE TRIGGER interaction_derived_event_outbox_transition_guard
BEFORE UPDATE ON interaction_derived_event_outbox
WHEN NOT (
    (OLD.status = 'pending' AND NEW.status = 'claimed'
        AND NEW.delivery_attempts = OLD.delivery_attempts + 1)
    OR (OLD.status = 'claimed' AND NEW.status = 'claimed'
        AND NEW.delivery_attempts = OLD.delivery_attempts + 1)
    OR (OLD.status = 'claimed' AND NEW.status = 'pending'
        AND NEW.delivery_attempts = OLD.delivery_attempts)
    OR (OLD.status = 'claimed' AND NEW.status = 'acknowledged'
        AND NEW.delivery_attempts = OLD.delivery_attempts)
)
BEGIN
    SELECT RAISE(ABORT, 'derived interaction occurrence transition is invalid');
END;

CREATE TRIGGER interaction_derived_event_guard_audit_no_update
BEFORE UPDATE ON interaction_derived_event_guard_audit
BEGIN
    SELECT RAISE(ABORT, 'derived interaction guard audit is immutable');
END;

CREATE TRIGGER interaction_derived_event_guard_audit_no_delete
BEFORE DELETE ON interaction_derived_event_guard_audit
BEGIN
    SELECT RAISE(ABORT, 'derived interaction guard audit is immutable');
END;
