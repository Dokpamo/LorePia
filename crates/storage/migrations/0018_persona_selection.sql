PRAGMA foreign_keys = ON;

CREATE TABLE conversation_persona_selections (
    conversation_id TEXT PRIMARY KEY NOT NULL
        REFERENCES conversations(id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE,
    persona_id TEXT NOT NULL,
    persona_revision_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision >= 1),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    updated_at TEXT NOT NULL CHECK (length(trim(updated_at)) > 0),
    deleted_at TEXT CHECK (
        deleted_at IS NULL OR length(trim(deleted_at)) > 0
    ),
    FOREIGN KEY (persona_id, persona_revision_id)
        REFERENCES persona_revisions(persona_id, revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

CREATE INDEX conversation_persona_selections_persona
    ON conversation_persona_selections(persona_id, conversation_id)
    WHERE deleted_at IS NULL;

CREATE TRIGGER conversation_persona_selections_revision_guard
BEFORE UPDATE ON conversation_persona_selections
WHEN
    NEW.conversation_id != OLD.conversation_id
    OR NEW.revision != OLD.revision + 1
    OR NEW.created_at != OLD.created_at
    OR (
        OLD.deleted_at IS NOT NULL
        AND NEW.deleted_at IS NOT NULL
    )
BEGIN
    SELECT RAISE(ABORT, 'conversation persona selection update is invalid');
END;

CREATE TABLE conversation_persona_selection_events (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(trim(id)) > 0),
    conversation_id TEXT NOT NULL
        REFERENCES conversations(id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE,
    selection_revision INTEGER NOT NULL CHECK (selection_revision >= 1),
    event_kind TEXT NOT NULL CHECK (
        event_kind IN ('selected', 'changed', 'cleared', 'persona_deleted')
    ),
    persona_id TEXT NOT NULL,
    persona_revision_id TEXT NOT NULL,
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    UNIQUE (conversation_id, selection_revision),
    FOREIGN KEY (persona_id, persona_revision_id)
        REFERENCES persona_revisions(persona_id, revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

CREATE TRIGGER conversation_persona_selection_events_no_update
BEFORE UPDATE ON conversation_persona_selection_events
BEGIN
    SELECT RAISE(ABORT, 'conversation persona selection events are immutable');
END;

CREATE TRIGGER conversation_persona_selection_events_no_delete
BEFORE DELETE ON conversation_persona_selection_events
BEGIN
    SELECT RAISE(ABORT, 'conversation persona selection events are immutable');
END;
