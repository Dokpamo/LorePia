PRAGMA foreign_keys = ON;

-- A proposal terminalization and its aggregate advancement are one logical
-- commit. SQLite has no general deferred CHECK constraint, so preserve both
-- sides as immutable handshake rows: the proposal trigger creates the child
-- commit first, the aggregate trigger creates the exact parent binding, and a
-- deferred foreign key rejects a transaction that commits only one side.
CREATE TABLE generation_attempt_aggregate_decision_bindings (
    generation_id TEXT NOT NULL,
    aggregate_revision INTEGER NOT NULL CHECK (aggregate_revision >= 2),
    proposal_record_id TEXT NOT NULL UNIQUE
        REFERENCES generation_attempt_proposals(proposal_record_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    interaction_state_revision INTEGER NOT NULL CHECK (
        interaction_state_revision >= 0
    ),
    state_snapshot_sha256 TEXT NOT NULL CHECK (
        length(state_snapshot_sha256) = 64
        AND state_snapshot_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    decision_kind TEXT NOT NULL CHECK (
        decision_kind IN ('approved', 'rejected', 'expired')
    ),
    decision_idempotency_key TEXT NOT NULL UNIQUE CHECK (
        length(trim(decision_idempotency_key)) BETWEEN 1 AND 256
    ),
    decision_updated_at TEXT NOT NULL CHECK (
        length(trim(decision_updated_at)) > 0
    ),
    PRIMARY KEY (generation_id, aggregate_revision),
    UNIQUE (
        generation_id,
        aggregate_revision,
        proposal_record_id,
        interaction_state_revision,
        state_snapshot_sha256,
        decision_kind,
        decision_idempotency_key,
        decision_updated_at
    )
);

CREATE TABLE generation_attempt_proposal_decision_commits (
    proposal_record_id TEXT PRIMARY KEY NOT NULL
        REFERENCES generation_attempt_proposals(proposal_record_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    generation_id TEXT NOT NULL,
    resulting_aggregate_revision INTEGER NOT NULL CHECK (
        resulting_aggregate_revision >= 2
    ),
    resulting_state_revision INTEGER NOT NULL CHECK (
        resulting_state_revision >= 0
    ),
    resulting_state_snapshot_sha256 TEXT NOT NULL CHECK (
        length(resulting_state_snapshot_sha256) = 64
        AND resulting_state_snapshot_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    decision_kind TEXT NOT NULL CHECK (
        decision_kind IN ('approved', 'rejected', 'expired')
    ),
    decision_idempotency_key TEXT NOT NULL UNIQUE CHECK (
        length(trim(decision_idempotency_key)) BETWEEN 1 AND 256
    ),
    proposal_revision INTEGER NOT NULL CHECK (proposal_revision = 2),
    decision_updated_at TEXT NOT NULL CHECK (
        length(trim(decision_updated_at)) > 0
    ),
    UNIQUE (generation_id, resulting_aggregate_revision),
    FOREIGN KEY (
        generation_id,
        resulting_aggregate_revision,
        proposal_record_id,
        resulting_state_revision,
        resulting_state_snapshot_sha256,
        decision_kind,
        decision_idempotency_key,
        decision_updated_at
    ) REFERENCES generation_attempt_aggregate_decision_bindings (
        generation_id,
        aggregate_revision,
        proposal_record_id,
        interaction_state_revision,
        state_snapshot_sha256,
        decision_kind,
        decision_idempotency_key,
        decision_updated_at
    )
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
        DEFERRABLE INITIALLY DEFERRED
);

-- Existing pre-version-29 decisions were written by the same Rust transaction.
-- Backfill their immutable handshake before enabling the write guards, then
-- verify the current aggregate is the exact final decision in that history.
INSERT INTO generation_attempt_aggregate_decision_bindings (
    generation_id,
    aggregate_revision,
    proposal_record_id,
    interaction_state_revision,
    state_snapshot_sha256,
    decision_kind,
    decision_idempotency_key,
    decision_updated_at
)
SELECT
    proposal.generation_id,
    proposal.resulting_aggregate_revision,
    proposal.proposal_record_id,
    proposal.resulting_state_revision,
    proposal.resulting_state_snapshot_sha256,
    proposal.decision_kind,
    proposal.decision_idempotency_key,
    proposal.updated_at
FROM generation_attempt_proposals AS proposal
WHERE proposal.status != 'pending'
ORDER BY proposal.generation_id, proposal.resulting_aggregate_revision;

INSERT INTO generation_attempt_proposal_decision_commits (
    proposal_record_id,
    generation_id,
    resulting_aggregate_revision,
    resulting_state_revision,
    resulting_state_snapshot_sha256,
    decision_kind,
    decision_idempotency_key,
    proposal_revision,
    decision_updated_at
)
SELECT
    proposal.proposal_record_id,
    proposal.generation_id,
    proposal.resulting_aggregate_revision,
    proposal.resulting_state_revision,
    proposal.resulting_state_snapshot_sha256,
    proposal.decision_kind,
    proposal.decision_idempotency_key,
    proposal.proposal_revision,
    proposal.updated_at
FROM generation_attempt_proposals AS proposal
WHERE proposal.status != 'pending'
ORDER BY proposal.generation_id, proposal.resulting_aggregate_revision;

CREATE TABLE migration_0029_generation_decision_validation (
    valid INTEGER NOT NULL CHECK (valid = 1)
);

INSERT INTO migration_0029_generation_decision_validation(valid)
SELECT CASE
    WHEN EXISTS (
        SELECT 1
        FROM generation_attempt_interaction_aggregates AS aggregate
        WHERE aggregate.aggregate_revision
                != aggregate.terminal_decision_count + 1
           OR aggregate.pending_proposal_count != (
                SELECT COUNT(*)
                FROM generation_attempt_proposals AS proposal
                WHERE proposal.generation_id = aggregate.generation_id
                  AND proposal.status = 'pending'
           )
           OR aggregate.terminal_decision_count != (
                SELECT COUNT(*)
                FROM generation_attempt_proposals AS proposal
                WHERE proposal.generation_id = aggregate.generation_id
                  AND proposal.status != 'pending'
           )
           OR aggregate.terminal_decision_count != (
                SELECT COUNT(*)
                FROM generation_attempt_proposal_decision_commits AS decision
                WHERE decision.generation_id = aggregate.generation_id
           )
           OR aggregate.terminal_decision_count != (
                SELECT COUNT(*)
                FROM generation_attempt_aggregate_decision_bindings AS binding
                WHERE binding.generation_id = aggregate.generation_id
           )
           OR EXISTS (
                SELECT 1
                FROM generation_attempt_aggregate_decision_bindings AS binding
                WHERE binding.generation_id = aggregate.generation_id
                  AND binding.aggregate_revision > aggregate.aggregate_revision
           )
           OR (
                aggregate.terminal_decision_count > 0
                AND NOT EXISTS (
                    SELECT 1
                    FROM generation_attempt_aggregate_decision_bindings AS binding
                    WHERE binding.generation_id = aggregate.generation_id
                      AND binding.aggregate_revision = aggregate.aggregate_revision
                      AND binding.interaction_state_revision
                            = aggregate.interaction_state_revision
                      AND binding.state_snapshot_sha256
                            = aggregate.state_snapshot_sha256
                      AND binding.decision_updated_at = aggregate.updated_at
                )
           )
    ) OR EXISTS (
        SELECT 1
        FROM generation_attempt_proposals AS proposal
        LEFT JOIN generation_attempt_interaction_aggregates AS aggregate
          ON aggregate.generation_id = proposal.generation_id
        WHERE aggregate.generation_id IS NULL
    ) OR EXISTS (
        SELECT 1
        FROM generation_attempt_proposal_decision_commits AS decision
        LEFT JOIN generation_attempt_interaction_aggregates AS aggregate
          ON aggregate.generation_id = decision.generation_id
        WHERE aggregate.generation_id IS NULL
    )
    THEN 0
    ELSE 1
END;

DROP TABLE migration_0029_generation_decision_validation;

CREATE TRIGGER generation_attempt_decision_binding_insert_guard
BEFORE INSERT ON generation_attempt_aggregate_decision_bindings
WHEN NOT EXISTS (
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
      AND proposal.decision_idempotency_key = NEW.decision_idempotency_key
      AND proposal.updated_at = NEW.decision_updated_at
      AND aggregate.aggregate_revision = NEW.aggregate_revision
      AND aggregate.interaction_state_revision = NEW.interaction_state_revision
      AND aggregate.state_snapshot_sha256 = NEW.state_snapshot_sha256
      AND aggregate.updated_at = NEW.decision_updated_at
)
BEGIN
    SELECT RAISE(
        ABORT,
        'generation attempt aggregate decision binding is detached'
    );
END;

CREATE TRIGGER generation_attempt_decision_binding_no_update
BEFORE UPDATE ON generation_attempt_aggregate_decision_bindings
BEGIN
    SELECT RAISE(ABORT, 'generation attempt aggregate decision bindings are immutable');
END;

CREATE TRIGGER generation_attempt_decision_binding_no_delete
BEFORE DELETE ON generation_attempt_aggregate_decision_bindings
BEGIN
    SELECT RAISE(ABORT, 'generation attempt aggregate decision bindings are immutable');
END;

CREATE TRIGGER generation_attempt_decision_commit_insert_guard
BEFORE INSERT ON generation_attempt_proposal_decision_commits
WHEN NOT EXISTS (
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
      AND proposal.decision_idempotency_key = NEW.decision_idempotency_key
      AND proposal.updated_at = NEW.decision_updated_at
)
BEGIN
    SELECT RAISE(
        ABORT,
        'generation attempt proposal decision commit is detached'
    );
END;

CREATE TRIGGER generation_attempt_decision_commit_no_update
BEFORE UPDATE ON generation_attempt_proposal_decision_commits
BEGIN
    SELECT RAISE(ABORT, 'generation attempt proposal decision commits are immutable');
END;

CREATE TRIGGER generation_attempt_decision_commit_no_delete
BEFORE DELETE ON generation_attempt_proposal_decision_commits
BEGIN
    SELECT RAISE(ABORT, 'generation attempt proposal decision commits are immutable');
END;

CREATE TRIGGER generation_attempt_proposals_terminal_insert_guard
BEFORE INSERT ON generation_attempt_proposals
WHEN NEW.status != 'pending'
BEGIN
    SELECT RAISE(ABORT, 'generation attempt proposals must begin pending');
END;

CREATE TRIGGER generation_attempt_aggregate_insert_guard_v2
BEFORE INSERT ON generation_attempt_interaction_aggregates
WHEN NEW.aggregate_revision != 1 OR NEW.terminal_decision_count != 0
BEGIN
    SELECT RAISE(ABORT, 'generation attempt aggregate must begin undecided at revision one');
END;

CREATE TRIGGER generation_attempt_proposal_decision_commit
AFTER UPDATE OF
    status,
    proposal_revision,
    decision_kind,
    decision_idempotency_key,
    resulting_aggregate_revision,
    resulting_state_revision,
    resulting_state_snapshot_sha256,
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
        decision_updated_at
    ) VALUES (
        NEW.proposal_record_id,
        NEW.generation_id,
        NEW.resulting_aggregate_revision,
        NEW.resulting_state_revision,
        NEW.resulting_state_snapshot_sha256,
        NEW.decision_kind,
        NEW.decision_idempotency_key,
        NEW.proposal_revision,
        NEW.updated_at
    );
END;

CREATE TRIGGER generation_attempt_aggregate_decision_bind
AFTER UPDATE OF
    aggregate_revision,
    interaction_state_revision,
    state_snapshot_sha256,
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
        decision_updated_at
    )
    SELECT
        decision.generation_id,
        decision.resulting_aggregate_revision,
        decision.proposal_record_id,
        decision.resulting_state_revision,
        decision.resulting_state_snapshot_sha256,
        decision.decision_kind,
        decision.decision_idempotency_key,
        decision.decision_updated_at
    FROM generation_attempt_proposal_decision_commits AS decision
    WHERE decision.generation_id = NEW.generation_id
      AND decision.resulting_aggregate_revision = NEW.aggregate_revision
      AND decision.resulting_state_revision = NEW.interaction_state_revision
      AND decision.resulting_state_snapshot_sha256 = NEW.state_snapshot_sha256
      AND decision.decision_updated_at = NEW.updated_at;
END;
