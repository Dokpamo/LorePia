PRAGMA foreign_keys = ON;

-- Immutable state at one visible message boundary. Historical forks consume
-- this row instead of cloning the source branch's current interaction state.
CREATE TABLE interaction_state_checkpoints (
    conversation_id TEXT NOT NULL,
    branch_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    source_interaction_state_id TEXT NOT NULL CHECK (
        length(trim(source_interaction_state_id)) BETWEEN 1 AND 256
    ),
    state_revision INTEGER NOT NULL CHECK (state_revision >= 0),
    state_document_json TEXT NOT NULL CHECK (
        json_valid(state_document_json)
        AND json_type(state_document_json) = 'object'
        AND length(CAST(state_document_json AS BLOB)) <= 8388608
    ),
    knowledge_bindings_json TEXT NOT NULL CHECK (
        json_valid(knowledge_bindings_json)
        AND json_type(knowledge_bindings_json) = 'array'
        AND length(CAST(knowledge_bindings_json AS BLOB)) <= 8388608
    ),
    checkpoint_sha256 TEXT NOT NULL CHECK (
        length(checkpoint_sha256) = 64
        AND checkpoint_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    PRIMARY KEY (conversation_id, branch_id, message_id),
    UNIQUE (source_interaction_state_id, state_revision),
    FOREIGN KEY (conversation_id, branch_id)
        REFERENCES conversation_branches(conversation_id, id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE,
    FOREIGN KEY (conversation_id, message_id)
        REFERENCES messages(conversation_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

CREATE INDEX interaction_state_checkpoints_state
    ON interaction_state_checkpoints(
        conversation_id,
        branch_id,
        state_revision,
        message_id
    );

CREATE TRIGGER interaction_state_checkpoints_immutable
BEFORE UPDATE ON interaction_state_checkpoints
BEGIN
    SELECT RAISE(ABORT, 'interaction checkpoint is immutable');
END;

CREATE TABLE prompt_preset_rollback_reviews (
    review_sha256 TEXT PRIMARY KEY NOT NULL CHECK (
        length(review_sha256) = 64
        AND review_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    prompt_preset_id TEXT NOT NULL
        REFERENCES prompt_presets(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    expected_state_revision INTEGER NOT NULL CHECK (
        expected_state_revision >= 1
    ),
    expected_current_revision_id TEXT NOT NULL
        REFERENCES prompt_preset_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    target_revision_id TEXT NOT NULL
        REFERENCES prompt_preset_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    target_dependency_sha256 TEXT NOT NULL CHECK (
        length(target_dependency_sha256) = 64
        AND target_dependency_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    binding_snapshot_sha256 TEXT NOT NULL CHECK (
        length(binding_snapshot_sha256) = 64
        AND binding_snapshot_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    diff_sha256 TEXT NOT NULL CHECK (
        length(diff_sha256) = 64
        AND diff_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    review_json TEXT NOT NULL CHECK (
        json_valid(review_json)
        AND json_type(review_json) = 'object'
        AND length(CAST(review_json AS BLOB)) <= 2097152
    ),
    reviewed_at TEXT NOT NULL CHECK (length(trim(reviewed_at)) > 0)
);

CREATE TRIGGER prompt_preset_rollback_reviews_immutable
BEFORE UPDATE ON prompt_preset_rollback_reviews
BEGIN
    SELECT RAISE(ABORT, 'prompt preset rollback review is immutable');
END;

CREATE TRIGGER prompt_preset_rollback_reviews_no_delete
BEFORE DELETE ON prompt_preset_rollback_reviews
BEGIN
    SELECT RAISE(ABORT, 'prompt preset rollback review is immutable');
END;

CREATE TABLE prompt_preset_rollback_approvals (
    approval_id TEXT PRIMARY KEY NOT NULL CHECK (
        length(trim(approval_id)) BETWEEN 1 AND 256
    ),
    approval_sha256 TEXT NOT NULL UNIQUE CHECK (
        length(approval_sha256) = 64
        AND approval_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    review_sha256 TEXT NOT NULL
        REFERENCES prompt_preset_rollback_reviews(review_sha256)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    applied_revision_id TEXT
        REFERENCES prompt_preset_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    approved_at TEXT NOT NULL CHECK (length(trim(approved_at)) > 0),
    applied_at TEXT,
    CHECK (
        (applied_revision_id IS NULL AND applied_at IS NULL)
        OR (applied_revision_id IS NOT NULL AND applied_at IS NOT NULL)
    )
);

CREATE TRIGGER prompt_preset_rollback_approval_identity_guard
BEFORE UPDATE ON prompt_preset_rollback_approvals
WHEN
    NEW.approval_id != OLD.approval_id
    OR NEW.approval_sha256 != OLD.approval_sha256
    OR NEW.review_sha256 != OLD.review_sha256
    OR NEW.approved_at != OLD.approved_at
    OR OLD.applied_revision_id IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'prompt preset rollback approval identity is immutable');
END;
