PRAGMA foreign_keys = ON;

CREATE TABLE generation_attempt_intents (
    generation_id TEXT PRIMARY KEY NOT NULL CHECK (
        length(trim(generation_id)) BETWEEN 1 AND 256
    ),
    operation_id TEXT NOT NULL CHECK (
        length(trim(operation_id)) BETWEEN 1 AND 256
    ),
    conversation_id TEXT NOT NULL
        REFERENCES conversations(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    source_branch_id TEXT NOT NULL,
    proposed_branch_id TEXT NOT NULL CHECK (
        length(trim(proposed_branch_id)) BETWEEN 1 AND 256
    ),
    expected_head_message_id TEXT,
    context_head_message_id TEXT,
    module_plan_sha256 TEXT NOT NULL CHECK (
        length(module_plan_sha256) = 64
        AND module_plan_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    base_input_fingerprint_sha256 TEXT NOT NULL CHECK (
        length(base_input_fingerprint_sha256) = 64
        AND base_input_fingerprint_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    before_generation_evidence_json TEXT CHECK (
        before_generation_evidence_json IS NULL
        OR (
            json_valid(before_generation_evidence_json)
            AND json_type(before_generation_evidence_json) = 'object'
            AND length(CAST(before_generation_evidence_json AS BLOB))
                <= 1048576
        )
    ),
    before_generation_evidence_sha256 TEXT CHECK (
        before_generation_evidence_sha256 IS NULL
        OR (
            length(before_generation_evidence_sha256) = 64
            AND before_generation_evidence_sha256
                NOT GLOB '*[^0-9a-f]*'
        )
    ),
    approval_evidence_json TEXT CHECK (
        approval_evidence_json IS NULL
        OR (
            json_valid(approval_evidence_json)
            AND json_type(approval_evidence_json) = 'object'
            AND length(CAST(approval_evidence_json AS BLOB)) <= 1048576
        )
    ),
    approval_evidence_sha256 TEXT CHECK (
        approval_evidence_sha256 IS NULL
        OR (
            length(approval_evidence_sha256) = 64
            AND approval_evidence_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    ),
    dispatch_seal_json TEXT CHECK (
        dispatch_seal_json IS NULL
        OR (
            json_valid(dispatch_seal_json)
            AND json_type(dispatch_seal_json) = 'object'
            AND length(CAST(dispatch_seal_json AS BLOB)) <= 1048576
        )
    ),
    dispatch_seal_sha256 TEXT CHECK (
        dispatch_seal_sha256 IS NULL
        OR (
            length(dispatch_seal_sha256) = 64
            AND dispatch_seal_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    ),
    attempt_sha256 TEXT NOT NULL UNIQUE CHECK (
        length(attempt_sha256) = 64
        AND attempt_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    status TEXT NOT NULL CHECK (
        status IN (
            'prepared',
            'before_generation_applied',
            'awaiting_approval',
            'dispatch_ready',
            'running',
            'failed_before_dispatch',
            'completed'
        )
    ),
    revision INTEGER NOT NULL CHECK (revision >= 1),
    failure_code TEXT CHECK (
        failure_code IS NULL
        OR length(trim(failure_code)) BETWEEN 1 AND 128
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    updated_at TEXT NOT NULL CHECK (length(trim(updated_at)) > 0),
    FOREIGN KEY (conversation_id, source_branch_id)
        REFERENCES conversation_branches(conversation_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    FOREIGN KEY (conversation_id, expected_head_message_id)
        REFERENCES messages(conversation_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    FOREIGN KEY (conversation_id, context_head_message_id)
        REFERENCES messages(conversation_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    UNIQUE (conversation_id, operation_id),
    CHECK (
        (status = 'failed_before_dispatch' AND failure_code IS NOT NULL)
        OR (status != 'failed_before_dispatch' AND failure_code IS NULL)
    ),
    CHECK (
        status NOT IN ('dispatch_ready', 'running', 'completed')
        OR (
            dispatch_seal_json IS NOT NULL
            AND dispatch_seal_sha256 IS NOT NULL
        )
    ),
    CHECK (
        status NOT IN (
            'before_generation_applied',
            'awaiting_approval',
            'dispatch_ready',
            'running',
            'completed'
        )
        OR before_generation_evidence_sha256 IS NOT NULL
    ),
    CHECK (
        (before_generation_evidence_json IS NULL)
            = (before_generation_evidence_sha256 IS NULL)
    ),
    CHECK (
        (approval_evidence_json IS NULL)
            = (approval_evidence_sha256 IS NULL)
    ),
    CHECK (
        (dispatch_seal_json IS NULL) = (dispatch_seal_sha256 IS NULL)
    )
);

CREATE INDEX generation_attempt_intents_status
    ON generation_attempt_intents(status, updated_at, generation_id);

CREATE TRIGGER generation_attempt_intents_identity_guard
BEFORE UPDATE ON generation_attempt_intents
WHEN
    NEW.generation_id != OLD.generation_id
    OR NEW.operation_id != OLD.operation_id
    OR NEW.conversation_id != OLD.conversation_id
    OR NEW.source_branch_id != OLD.source_branch_id
    OR NEW.proposed_branch_id != OLD.proposed_branch_id
    OR NEW.expected_head_message_id IS NOT OLD.expected_head_message_id
    OR NEW.context_head_message_id IS NOT OLD.context_head_message_id
    OR NEW.module_plan_sha256 != OLD.module_plan_sha256
    OR NEW.base_input_fingerprint_sha256
        != OLD.base_input_fingerprint_sha256
    OR NEW.attempt_sha256 != OLD.attempt_sha256
    OR NEW.created_at != OLD.created_at
    OR NEW.revision != OLD.revision + 1
BEGIN
    SELECT RAISE(ABORT, 'generation attempt identity is immutable');
END;

CREATE TRIGGER generation_attempt_intents_evidence_guard
BEFORE UPDATE ON generation_attempt_intents
WHEN
    (
        OLD.before_generation_evidence_sha256 IS NOT NULL
        AND (
            NEW.before_generation_evidence_sha256
                IS NOT OLD.before_generation_evidence_sha256
            OR NEW.before_generation_evidence_json
                IS NOT OLD.before_generation_evidence_json
        )
    )
    OR (
        OLD.approval_evidence_sha256 IS NOT NULL
        AND (
            NEW.approval_evidence_sha256
                IS NOT OLD.approval_evidence_sha256
            OR NEW.approval_evidence_json
                IS NOT OLD.approval_evidence_json
        )
    )
    OR (
        OLD.dispatch_seal_sha256 IS NOT NULL
        AND (
            NEW.dispatch_seal_sha256 IS NOT OLD.dispatch_seal_sha256
            OR NEW.dispatch_seal_json IS NOT OLD.dispatch_seal_json
        )
    )
BEGIN
    SELECT RAISE(ABORT, 'generation attempt evidence is immutable');
END;

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
            ))
    )
BEGIN
    SELECT RAISE(ABORT, 'generation attempt transition is invalid');
END;

CREATE TABLE core_lifecycle_outbox (
    occurrence_id TEXT PRIMARY KEY NOT NULL CHECK (
        length(trim(occurrence_id)) BETWEEN 1 AND 256
    ),
    event_kind TEXT NOT NULL CHECK (
        event_kind IN (
            'conversation_opened',
            'conversation_started',
            'before_generation',
            'after_generation',
            'message_committed'
        )
    ),
    conversation_id TEXT NOT NULL
        REFERENCES conversations(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    branch_id TEXT NOT NULL,
    exact_head_message_id TEXT,
    owner_message_id TEXT,
    -- Before-generation occurrences name an attempt that necessarily precedes
    -- the generations row; terminal occurrences name that eventual
    -- generation. Conditional triggers below enforce the correct authority.
    generation_id TEXT,
    occurred_at TEXT NOT NULL CHECK (length(trim(occurred_at)) > 0),
    available_at TEXT NOT NULL CHECK (length(trim(available_at)) > 0),
    status TEXT NOT NULL CHECK (
        status IN ('pending', 'claimed', 'acknowledged')
    ),
    delivery_attempts INTEGER NOT NULL DEFAULT 0 CHECK (
        delivery_attempts >= 0
    ),
    lease_until TEXT,
    acknowledged_at TEXT,
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    FOREIGN KEY (conversation_id, branch_id)
        REFERENCES conversation_branches(conversation_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    FOREIGN KEY (conversation_id, exact_head_message_id)
        REFERENCES messages(conversation_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    FOREIGN KEY (conversation_id, owner_message_id)
        REFERENCES messages(conversation_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    CHECK (
        (status = 'pending'
            AND lease_until IS NULL
            AND acknowledged_at IS NULL)
        OR (status = 'claimed'
            AND lease_until IS NOT NULL
            AND acknowledged_at IS NULL)
        OR (status = 'acknowledged'
            AND lease_until IS NULL
            AND acknowledged_at IS NOT NULL)
    )
);

CREATE INDEX core_lifecycle_outbox_delivery
    ON core_lifecycle_outbox(
        status,
        available_at,
        lease_until,
        occurred_at,
        occurrence_id
    )
    WHERE status != 'acknowledged';

CREATE INDEX core_lifecycle_outbox_conversation
    ON core_lifecycle_outbox(
        conversation_id,
        branch_id,
        occurred_at,
        occurrence_id
    );

CREATE TRIGGER core_lifecycle_outbox_identity_guard
BEFORE UPDATE ON core_lifecycle_outbox
WHEN
    NEW.occurrence_id != OLD.occurrence_id
    OR NEW.event_kind != OLD.event_kind
    OR NEW.conversation_id != OLD.conversation_id
    OR NEW.branch_id != OLD.branch_id
    OR NEW.exact_head_message_id IS NOT OLD.exact_head_message_id
    OR NEW.owner_message_id IS NOT OLD.owner_message_id
    OR NEW.generation_id IS NOT OLD.generation_id
    OR NEW.occurred_at != OLD.occurred_at
    OR NEW.created_at != OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'lifecycle occurrence identity is immutable');
END;

CREATE TRIGGER core_lifecycle_outbox_transition_guard
BEFORE UPDATE ON core_lifecycle_outbox
WHEN
    NOT (
        (OLD.status = 'pending'
            AND NEW.status = 'claimed'
            AND NEW.delivery_attempts = OLD.delivery_attempts + 1)
        OR (OLD.status = 'claimed'
            AND NEW.status = 'claimed'
            AND NEW.delivery_attempts = OLD.delivery_attempts + 1)
        OR (OLD.status = 'claimed'
            AND NEW.status = 'pending'
            AND NEW.delivery_attempts = OLD.delivery_attempts)
        OR (OLD.status = 'claimed'
            AND NEW.status = 'acknowledged'
            AND NEW.delivery_attempts = OLD.delivery_attempts)
    )
BEGIN
    SELECT RAISE(ABORT, 'lifecycle occurrence transition is invalid');
END;

CREATE TRIGGER core_lifecycle_outbox_generation_guard
BEFORE INSERT ON core_lifecycle_outbox
WHEN
    (
        NEW.event_kind IN ('conversation_opened', 'conversation_started')
        AND NEW.generation_id IS NOT NULL
    )
    OR (
        NEW.event_kind IN ('before_generation', 'after_generation')
        AND NEW.generation_id IS NULL
    )
    OR (
        NEW.event_kind = 'before_generation'
        AND NOT EXISTS (
            SELECT 1
            FROM generation_attempt_intents AS attempt
            WHERE attempt.generation_id = NEW.generation_id
              AND attempt.conversation_id = NEW.conversation_id
              AND attempt.source_branch_id = NEW.branch_id
        )
    )
    OR (
        NEW.event_kind = 'after_generation'
        AND NOT EXISTS (
            SELECT 1
            FROM generations AS generation
            WHERE generation.id = NEW.generation_id
              AND generation.conversation_id = NEW.conversation_id
              AND generation.branch_id = NEW.branch_id
        )
    )
BEGIN
    SELECT RAISE(ABORT, 'lifecycle generation authority is invalid');
END;

CREATE TRIGGER package_imports_require_inspected_initial_state_v19
BEFORE INSERT ON package_imports
WHEN NEW.state != 'inspected'
BEGIN
    SELECT RAISE(ABORT, 'package import must begin inspected');
END;

ALTER TABLE interaction_events
    ADD COLUMN generation_attempt_id TEXT
        REFERENCES generation_attempt_intents(generation_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT;

CREATE TRIGGER interaction_events_generation_attempt_guard
BEFORE INSERT ON interaction_events
WHEN
    (
        NEW.event_kind IN ('before_generation', 'after_generation')
        AND NEW.generation_attempt_id IS NULL
    )
    OR (
        NEW.event_kind NOT IN ('before_generation', 'after_generation')
        AND NEW.generation_attempt_id IS NOT NULL
    )
BEGIN
    SELECT RAISE(ABORT, 'interaction generation event requires exact attempt');
END;
