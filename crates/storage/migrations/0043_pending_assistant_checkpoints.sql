-- Pending streaming bodies are durable append-only chunks. Terminal/replacement
-- writes still publish one canonical messages.content value in their transaction.
CREATE TABLE pending_assistant_checkpoints (
    message_id TEXT PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
    generation_id TEXT NOT NULL,
    base_bytes INTEGER NOT NULL CHECK (typeof(base_bytes) = 'integer' AND base_bytes >= 0),
    total_bytes INTEGER NOT NULL CHECK (typeof(total_bytes) = 'integer' AND total_bytes >= base_bytes),
    next_sequence INTEGER NOT NULL CHECK (typeof(next_sequence) = 'integer' AND next_sequence >= 0)
);

CREATE TABLE pending_assistant_checkpoint_chunks (
    message_id TEXT NOT NULL REFERENCES pending_assistant_checkpoints(message_id) ON DELETE CASCADE,
    sequence INTEGER NOT NULL CHECK (typeof(sequence) = 'integer' AND sequence >= 0),
    offset_bytes INTEGER NOT NULL CHECK (typeof(offset_bytes) = 'integer' AND offset_bytes >= 0),
    byte_length INTEGER NOT NULL CHECK (typeof(byte_length) = 'integer' AND byte_length BETWEEN 1 AND 65536),
    content TEXT NOT NULL CHECK (typeof(content) = 'text' AND byte_length = length(CAST(content AS BLOB))),
    PRIMARY KEY (message_id, sequence),
    UNIQUE (message_id, offset_bytes)
);

CREATE TRIGGER pending_assistant_checkpoint_initial_guard
BEFORE INSERT ON pending_assistant_checkpoints
WHEN NEW.total_bytes != NEW.base_bytes OR NEW.next_sequence != 0
    OR NOT EXISTS (
        SELECT 1 FROM messages WHERE id = NEW.message_id
          AND generation_id = NEW.generation_id AND role = 'assistant' AND status = 'pending'
          AND typeof(content) = 'text' AND length(CAST(content AS BLOB)) = NEW.base_bytes
    )
BEGIN SELECT RAISE(ABORT, 'pending checkpoint owner or initial offset is invalid'); END;

CREATE TRIGGER pending_assistant_checkpoint_state_guard
BEFORE UPDATE ON pending_assistant_checkpoints
WHEN NEW.message_id != OLD.message_id OR NEW.generation_id != OLD.generation_id
    OR NEW.base_bytes != OLD.base_bytes OR NEW.next_sequence != OLD.next_sequence + 1
    OR NOT EXISTS (
        SELECT 1 FROM pending_assistant_checkpoint_chunks
        WHERE message_id = OLD.message_id AND sequence = OLD.next_sequence
          AND offset_bytes = OLD.total_bytes AND NEW.total_bytes = OLD.total_bytes + byte_length
    )
BEGIN SELECT RAISE(ABORT, 'pending checkpoint state transition is invalid'); END;

CREATE TRIGGER pending_assistant_checkpoint_chunk_guard
BEFORE INSERT ON pending_assistant_checkpoint_chunks
WHEN NOT EXISTS (
    SELECT 1 FROM pending_assistant_checkpoints checkpoint JOIN messages message
      ON message.id = checkpoint.message_id
    WHERE checkpoint.message_id = NEW.message_id
      AND checkpoint.next_sequence = NEW.sequence AND checkpoint.total_bytes = NEW.offset_bytes
      AND message.generation_id = checkpoint.generation_id
      AND message.role = 'assistant' AND message.status = 'pending'
)
BEGIN SELECT RAISE(ABORT, 'pending checkpoint chunk is out of order or unowned'); END;

CREATE TRIGGER pending_assistant_checkpoint_chunk_immutable
BEFORE UPDATE ON pending_assistant_checkpoint_chunks
BEGIN SELECT RAISE(ABORT, 'pending checkpoint chunk is immutable'); END;

CREATE TRIGGER pending_assistant_checkpoint_chunk_advance
AFTER INSERT ON pending_assistant_checkpoint_chunks
BEGIN
    UPDATE pending_assistant_checkpoints
    SET total_bytes = total_bytes + NEW.byte_length, next_sequence = next_sequence + 1
    WHERE message_id = NEW.message_id;
END;

CREATE TRIGGER pending_assistant_checkpoint_content_replaced
AFTER UPDATE OF content ON messages
BEGIN DELETE FROM pending_assistant_checkpoints WHERE message_id = OLD.id; END;

CREATE TRIGGER pending_assistant_checkpoint_owner_closed
AFTER UPDATE OF status, role, generation_id ON messages
WHEN NEW.status != OLD.status OR NEW.role != OLD.role
    OR NEW.generation_id IS NOT OLD.generation_id
BEGIN DELETE FROM pending_assistant_checkpoints WHERE message_id = OLD.id; END;

-- Invalid/gapped state raises a SQL error even before a content/length filter.
-- CASE evaluates this deliberately invalid JSON only on the corruption path.
CREATE VIEW messages_with_checkpoints AS
SELECT message.id, message.conversation_id, message.parent_id, message.role,
       CASE WHEN checkpoint.message_id IS NULL THEN message.content
            WHEN message.status != 'pending' OR message.role != 'assistant'
              OR message.generation_id IS NOT checkpoint.generation_id
              OR typeof(message.content) != 'text'
              OR length(CAST(message.content AS BLOB)) != checkpoint.base_bytes THEN json('invalid pending checkpoint journal')
            ELSE (
                SELECT CASE
                    WHEN COUNT(*) = checkpoint.next_sequence
                     AND COALESCE(MIN(chunk.sequence), 0) = 0
                     AND COALESCE(MAX(chunk.sequence), -1) = checkpoint.next_sequence - 1
                     AND COALESCE(SUM(chunk.byte_length), 0) = checkpoint.total_bytes - checkpoint.base_bytes
                     AND COALESCE(SUM(length(CAST(chunk.content AS BLOB))), 0) = checkpoint.total_bytes - checkpoint.base_bytes
                     AND NOT EXISTS (
                         SELECT 1 FROM pending_assistant_checkpoint_chunks current
                         LEFT JOIN pending_assistant_checkpoint_chunks previous
                           ON previous.message_id = current.message_id
                          AND previous.sequence = current.sequence - 1
                         WHERE current.message_id = message.id
                           AND (current.offset_bytes != CASE WHEN current.sequence = 0
                                    THEN checkpoint.base_bytes
                                    ELSE previous.offset_bytes + previous.byte_length END
                                OR (current.sequence > 0 AND previous.sequence IS NULL))
                     )
                    THEN message.content || COALESCE(group_concat(chunk.content, '' ORDER BY chunk.sequence), '')
                    ELSE json('invalid pending checkpoint journal') END
                FROM pending_assistant_checkpoint_chunks chunk WHERE chunk.message_id = message.id
            ) END AS content,
       message.status, message.generation_id, message.created_at
FROM messages message
LEFT JOIN pending_assistant_checkpoints checkpoint ON checkpoint.message_id = message.id;
