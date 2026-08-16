-- Preserve the exact derived-event evidence produced by an attempt-owned
-- BeforeGeneration review. Existing snapshots are backfilled with the empty
-- authority only; materialization revalidates it against the sealed effects,
-- so an older state-changing snapshot fails closed instead of dropping its
-- VariableChanged/KnowledgeActivated cascade.

ALTER TABLE generation_attempt_before_event_snapshots
ADD COLUMN derived_events_json TEXT NOT NULL DEFAULT '[]' CHECK (
    json_valid(derived_events_json)
    AND json_type(derived_events_json) = 'array'
    AND length(CAST(derived_events_json AS BLOB)) <= 8388608
);

ALTER TABLE generation_attempt_before_event_snapshots
ADD COLUMN derived_events_sha256 TEXT NOT NULL
DEFAULT '4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945'
CHECK (
    length(derived_events_sha256) = 64
    AND derived_events_sha256 NOT GLOB '*[^0-9a-f]*'
);

-- Root chains on one branch are dispatched in committed state-revision order,
-- never by timestamps or hashed chain identifiers. Version 33 rows are
-- backfilled from their immutable parent interaction event.
DROP TRIGGER interaction_derived_event_outbox_identity_guard;
DROP TRIGGER interaction_derived_event_outbox_transition_guard;

ALTER TABLE interaction_derived_event_outbox
ADD COLUMN parent_resulting_state_revision INTEGER NOT NULL DEFAULT 1 CHECK (
    parent_resulting_state_revision >= 1
);

UPDATE interaction_derived_event_outbox
SET parent_resulting_state_revision = (
    SELECT event.resulting_state_revision
    FROM interaction_events AS event
    WHERE event.id = interaction_derived_event_outbox.parent_event_id
);

CREATE INDEX interaction_derived_event_outbox_causal_delivery
    ON interaction_derived_event_outbox(
        status, available_at, lease_until, conversation_id, branch_id,
        parent_resulting_state_revision, chain_id, chain_ordinal
    )
    WHERE status != 'acknowledged';

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
