PRAGMA foreign_keys = ON;

-- Schema 24 stored Core review/proposal identities directly in globally
-- unique columns. Preserve those immutable rows as identity version 1, while
-- reserving version 2 for generation-bound Storage wrappers. The Rust
-- migration validator verifies every legacy JSON/hash/foreign-key binding
-- before this atomic backfill runs.
DROP TRIGGER generation_attempt_before_snapshot_no_update;
DROP TRIGGER generation_attempt_proposals_transition_guard;

ALTER TABLE generation_attempt_before_event_snapshots
    ADD COLUMN domain_review_sha256 TEXT;
ALTER TABLE generation_attempt_before_event_snapshots
    ADD COLUMN storage_identity_version INTEGER;

ALTER TABLE generation_attempt_proposals
    ADD COLUMN domain_proposal_record_id TEXT;
ALTER TABLE generation_attempt_proposals
    ADD COLUMN domain_proposal_review_sha256 TEXT;
ALTER TABLE generation_attempt_proposals
    ADD COLUMN storage_identity_version INTEGER;

UPDATE generation_attempt_before_event_snapshots
SET domain_review_sha256 = review_sha256,
    storage_identity_version = 1;

UPDATE generation_attempt_proposals
SET domain_proposal_record_id = proposal_record_id,
    domain_proposal_review_sha256 = proposal_review_sha256,
    storage_identity_version = 1;

CREATE TABLE generation_attempt_identity_migration_guard (
    invalid_count INTEGER NOT NULL CHECK (invalid_count = 0)
);

INSERT INTO generation_attempt_identity_migration_guard(invalid_count)
SELECT COUNT(*)
FROM generation_attempt_before_event_snapshots
WHERE domain_review_sha256 IS NULL
   OR length(domain_review_sha256) != 64
   OR domain_review_sha256 GLOB '*[^0-9a-f]*'
   OR storage_identity_version != 1;

INSERT INTO generation_attempt_identity_migration_guard(invalid_count)
SELECT COUNT(*)
FROM generation_attempt_proposals
WHERE domain_proposal_record_id IS NULL
   OR length(trim(domain_proposal_record_id)) NOT BETWEEN 1 AND 256
   OR domain_proposal_review_sha256 IS NULL
   OR length(domain_proposal_review_sha256) != 64
   OR domain_proposal_review_sha256 GLOB '*[^0-9a-f]*'
   OR storage_identity_version != 1;

DROP TABLE generation_attempt_identity_migration_guard;

CREATE UNIQUE INDEX generation_attempt_proposals_domain_identity
    ON generation_attempt_proposals(
        generation_id,
        domain_proposal_record_id
    );

CREATE TRIGGER generation_attempt_before_identity_insert_guard
BEFORE INSERT ON generation_attempt_before_event_snapshots
WHEN NEW.storage_identity_version != 2
  OR NEW.domain_review_sha256 IS NULL
  OR length(NEW.domain_review_sha256) != 64
  OR NEW.domain_review_sha256 GLOB '*[^0-9a-f]*'
BEGIN
    SELECT RAISE(ABORT, 'generation attempt review storage identity is invalid');
END;

CREATE TRIGGER generation_attempt_before_snapshot_no_update
BEFORE UPDATE ON generation_attempt_before_event_snapshots
BEGIN
    SELECT RAISE(ABORT, 'generation attempt before snapshot is immutable');
END;

CREATE TRIGGER generation_attempt_proposal_identity_insert_guard
BEFORE INSERT ON generation_attempt_proposals
WHEN NEW.storage_identity_version != 2
  OR NEW.domain_proposal_record_id IS NULL
  OR length(trim(NEW.domain_proposal_record_id)) NOT BETWEEN 1 AND 256
  OR NEW.domain_proposal_review_sha256 IS NULL
  OR length(NEW.domain_proposal_review_sha256) != 64
  OR NEW.domain_proposal_review_sha256 GLOB '*[^0-9a-f]*'
BEGIN
    SELECT RAISE(ABORT, 'generation attempt proposal storage identity is invalid');
END;

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
