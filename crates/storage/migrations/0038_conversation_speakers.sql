-- Multi-speaker readiness for conversations.
--
-- The runtime still resolves exactly one character per conversation, and
-- `conversations.character_id` remains the authoritative primary speaker. These
-- structures record the same fact in a shape a later group-chat implementation
-- can extend without rewriting committed history, so adding a second speaker
-- becomes a planner and UI change rather than a migration of every prior room.
--
-- Attribution lives in its own table rather than a `messages` column: the
-- message table is on the hot append path and carries integrity triggers, so
-- keeping it untouched avoids a table rewrite both here and in any future
-- migration that has to reason about this data.

CREATE TABLE conversation_characters (
    conversation_id TEXT NOT NULL
        REFERENCES conversations(id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE,
    character_id TEXT NOT NULL
        REFERENCES characters(id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE,
    -- Stable presentation and turn order within one conversation.
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    -- A speaker may be retired from a room without losing its authored history.
    active INTEGER NOT NULL CHECK (active IN (0, 1)),
    PRIMARY KEY (conversation_id, character_id)
);

CREATE UNIQUE INDEX conversation_characters_order
    ON conversation_characters(conversation_id, ordinal);

CREATE INDEX conversation_characters_by_character
    ON conversation_characters(character_id, conversation_id);

-- Every existing room has exactly one speaker at ordinal zero.
INSERT INTO conversation_characters (conversation_id, character_id, ordinal, active)
SELECT id, character_id, 0, 1 FROM conversations;

-- Attribution for assistant turns. A message with no row here is spoken by the
-- conversation's primary speaker, which is exactly how every row written before
-- this migration and every row written by the single-speaker runtime reads.
CREATE TABLE message_speakers (
    message_id TEXT NOT NULL PRIMARY KEY
        REFERENCES messages(id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE,
    character_id TEXT NOT NULL
        REFERENCES characters(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

CREATE INDEX message_speakers_by_character
    ON message_speakers(character_id, message_id);
