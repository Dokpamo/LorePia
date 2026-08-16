PRAGMA foreign_keys = ON;

-- Every newly-created room records the exact character-content authority that
-- was observed while it was created. NULL is an intentional legacy-absence
-- binding, not an instruction to follow a future active revision.
CREATE TABLE conversation_greeting_bindings (
    conversation_id TEXT PRIMARY KEY NOT NULL
        REFERENCES conversations(id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE,
    character_content_revision_id TEXT
        REFERENCES character_content_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    greeting_id TEXT,
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    FOREIGN KEY (character_content_revision_id, greeting_id)
        REFERENCES character_greetings(
            character_content_revision_id,
            greeting_id
        )
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    CHECK (
        character_content_revision_id IS NOT NULL
        OR greeting_id IS NULL
    )
);

CREATE INDEX conversation_greeting_bindings_revision
    ON conversation_greeting_bindings(
        character_content_revision_id,
        greeting_id,
        conversation_id
    );

-- Defense in depth: the selected immutable revision must belong to the same
-- character as the conversation. Production writes already resolve this under
-- an IMMEDIATE transaction, but the database rejects mismatched direct writes.
CREATE TRIGGER conversation_greeting_bindings_character_guard
BEFORE INSERT ON conversation_greeting_bindings
WHEN NEW.character_content_revision_id IS NOT NULL
 AND NOT EXISTS (
    SELECT 1
    FROM conversations AS conversation
    JOIN character_content AS content
      ON content.character_id = conversation.character_id
    JOIN character_content_revisions AS revision
      ON revision.object_id = content.object_id
     AND revision.revision_id = NEW.character_content_revision_id
    WHERE conversation.id = NEW.conversation_id
 )
BEGIN
    SELECT RAISE(
        ABORT,
        'conversation greeting revision does not belong to its character'
    );
END;

CREATE TRIGGER conversation_greeting_bindings_no_update
BEFORE UPDATE ON conversation_greeting_bindings
BEGIN
    SELECT RAISE(ABORT, 'conversation greeting bindings are immutable');
END;
