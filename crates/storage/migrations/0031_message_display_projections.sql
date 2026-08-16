PRAGMA foreign_keys = ON;

-- DisplayOnly is a render projection, never the canonical message body. The
-- exact projection and a digest of its content-free rule diagnostics are
-- committed in the same transaction as the terminal assistant row.
CREATE TABLE message_display_projections (
    message_id TEXT PRIMARY KEY NOT NULL
        REFERENCES messages(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    generation_id TEXT NOT NULL UNIQUE
        REFERENCES generations(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    canonical_content_sha256 TEXT NOT NULL CHECK (
        length(canonical_content_sha256) = 64
        AND canonical_content_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    display_content TEXT NOT NULL CHECK (
        length(display_content) <= 262144
        AND length(CAST(display_content AS BLOB)) <= 1048576
    ),
    display_content_sha256 TEXT NOT NULL CHECK (
        length(display_content_sha256) = 64
        AND display_content_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    pipeline_diagnostics_json TEXT NOT NULL CHECK (
        json_valid(pipeline_diagnostics_json)
        AND json_type(pipeline_diagnostics_json) = 'object'
        AND length(CAST(pipeline_diagnostics_json AS BLOB)) <= 16384
    ),
    diagnostics_sha256 TEXT NOT NULL CHECK (
        length(diagnostics_sha256) = 64
        AND diagnostics_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0)
);

CREATE TRIGGER message_display_projections_owner_guard
BEFORE INSERT ON message_display_projections
WHEN NOT EXISTS (
    SELECT 1
    FROM messages AS message
    JOIN generations AS generation
      ON generation.id = NEW.generation_id
     AND generation.assistant_message_id = message.id
     AND generation.conversation_id = message.conversation_id
    WHERE message.id = NEW.message_id
      AND message.role = 'assistant'
      AND message.status != 'pending'
      AND message.generation_id = NEW.generation_id
)
BEGIN
    SELECT RAISE(ABORT, 'display projection owner is inconsistent');
END;

CREATE TRIGGER message_display_projections_no_update
BEFORE UPDATE ON message_display_projections
BEGIN
    SELECT RAISE(ABORT, 'message display projections are immutable');
END;

CREATE TRIGGER message_display_projections_no_delete
BEFORE DELETE ON message_display_projections
BEGIN
    SELECT RAISE(ABORT, 'message display projections are immutable');
END;

CREATE INDEX message_display_projections_generation
    ON message_display_projections(generation_id, message_id);
