-- Bounded, revisioned state for imported character runtimes.
--
-- State is scoped to the exact character-content revision and conversation
-- branch that produced it. Branch epochs prevent a stale webview from
-- resurrecting state after an explicit branch rewind.

CREATE TABLE portable_runtime_branch_epochs (
    conversation_id TEXT NOT NULL,
    branch_id TEXT NOT NULL,
    epoch INTEGER NOT NULL DEFAULT 0 CHECK (epoch BETWEEN 0 AND 9007199254740991),
    updated_at TEXT NOT NULL CHECK (length(trim(updated_at)) > 0),
    PRIMARY KEY (conversation_id, branch_id),
    UNIQUE (conversation_id, branch_id, epoch),
    FOREIGN KEY (conversation_id, branch_id)
        REFERENCES conversation_branches(conversation_id, id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE
);

INSERT INTO portable_runtime_branch_epochs (
    conversation_id, branch_id, epoch, updated_at
)
SELECT conversation_id, id, 0, updated_at
FROM conversation_branches;

CREATE TRIGGER portable_runtime_branch_epoch_on_branch_insert
AFTER INSERT ON conversation_branches
BEGIN
    INSERT INTO portable_runtime_branch_epochs (
        conversation_id, branch_id, epoch, updated_at
    ) VALUES (NEW.conversation_id, NEW.id, 0, NEW.updated_at);
END;

CREATE TABLE portable_runtime_state_sequence (
    singleton INTEGER PRIMARY KEY NOT NULL CHECK (singleton = 1),
    next_access_sequence INTEGER NOT NULL CHECK (next_access_sequence > 0)
);

INSERT INTO portable_runtime_state_sequence (singleton, next_access_sequence)
VALUES (1, 1);

CREATE TABLE portable_runtime_states (
    character_id TEXT NOT NULL
        REFERENCES characters(id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE,
    character_content_revision_id TEXT
        REFERENCES character_content_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    -- SQLite UNIQUE constraints allow repeated NULL values. This exact key
    -- keeps legacy characters without a content revision uniquely scoped.
    character_content_revision_key TEXT NOT NULL CHECK (
        character_content_revision_key = coalesce(character_content_revision_id, '')
    ),
    conversation_id TEXT NOT NULL,
    branch_id TEXT NOT NULL,
    branch_epoch INTEGER NOT NULL CHECK (
        branch_epoch BETWEEN 0 AND 9007199254740991
    ),
    revision INTEGER NOT NULL CHECK (revision BETWEEN 1 AND 9007199254740991),
    payload_schema_version INTEGER NOT NULL CHECK (payload_schema_version > 0),
    payload_json TEXT NOT NULL CHECK (
        json_valid(payload_json)
        AND json_type(payload_json) = 'object'
        AND length(CAST(payload_json AS BLOB)) <= 4194304
    ),
    payload_bytes INTEGER NOT NULL CHECK (
        payload_bytes >= 2
        AND payload_bytes <= 4194304
        AND payload_bytes = length(CAST(payload_json AS BLOB))
    ),
    access_sequence INTEGER NOT NULL UNIQUE CHECK (access_sequence > 0),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    updated_at TEXT NOT NULL CHECK (length(trim(updated_at)) > 0),
    PRIMARY KEY (
        character_id,
        character_content_revision_key,
        conversation_id,
        branch_id
    ),
    FOREIGN KEY (conversation_id, branch_id, branch_epoch)
        REFERENCES portable_runtime_branch_epochs(conversation_id, branch_id, epoch)
        ON UPDATE RESTRICT
        ON DELETE CASCADE
);

CREATE INDEX portable_runtime_states_lru
    ON portable_runtime_states(access_sequence, character_id,
                               character_content_revision_key,
                               conversation_id, branch_id);

CREATE TRIGGER portable_runtime_state_scope_guard_insert
BEFORE INSERT ON portable_runtime_states
WHEN
    NOT EXISTS (
        SELECT 1
        FROM conversations AS conversation
        JOIN conversation_branches AS branch
          ON branch.conversation_id = conversation.id
         AND branch.id = NEW.branch_id
        WHERE conversation.id = NEW.conversation_id
          AND conversation.character_id = NEW.character_id
    )
    OR (
        NEW.character_content_revision_id IS NOT NULL
        AND NOT EXISTS (
            SELECT 1
            FROM character_content AS content
            JOIN character_content_revisions AS revision
              ON revision.object_id = content.object_id
            WHERE content.character_id = NEW.character_id
              AND revision.revision_id = NEW.character_content_revision_id
        )
    )
BEGIN
    SELECT RAISE(ABORT, 'portable runtime state scope is invalid');
END;

CREATE TRIGGER portable_runtime_state_scope_guard_update
BEFORE UPDATE ON portable_runtime_states
WHEN
    NEW.character_id != OLD.character_id
    OR NEW.character_content_revision_id IS NOT OLD.character_content_revision_id
    OR NEW.character_content_revision_key != OLD.character_content_revision_key
    OR NEW.conversation_id != OLD.conversation_id
    OR NEW.branch_id != OLD.branch_id
    OR NEW.branch_epoch != OLD.branch_epoch
    OR NEW.created_at != OLD.created_at
    OR NOT EXISTS (
        SELECT 1
        FROM conversations AS conversation
        JOIN conversation_branches AS branch
          ON branch.conversation_id = conversation.id
         AND branch.id = NEW.branch_id
        WHERE conversation.id = NEW.conversation_id
          AND conversation.character_id = NEW.character_id
    )
BEGIN
    SELECT RAISE(ABORT, 'portable runtime state scope is immutable or invalid');
END;
