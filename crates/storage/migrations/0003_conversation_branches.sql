ALTER TABLE messages RENAME TO messages_legacy_v2;

CREATE TABLE messages (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    parent_id TEXT,
    role TEXT NOT NULL CHECK (role IN ('system', 'user', 'assistant')),
    content TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'complete', 'cancelled', 'failed')),
    generation_id TEXT,
    created_at TEXT NOT NULL,
    UNIQUE (conversation_id, id),
    FOREIGN KEY (conversation_id, parent_id)
        REFERENCES messages(conversation_id, id),
    CHECK (parent_id IS NULL OR parent_id <> id),
    CHECK (role = 'assistant' OR status = 'complete'),
    CHECK (
        (role = 'assistant' AND generation_id IS NOT NULL)
        OR (role <> 'assistant' AND generation_id IS NULL)
    )
);

WITH migration_order AS (
    SELECT
        message.id,
        message.conversation_id,
        message.role,
        message.content,
        message.status,
        message.generation_id,
        message.created_at,
        CASE
            WHEN message.role = 'assistant' THEN parent.created_at
            ELSE message.created_at
        END AS turn_created_at,
        CASE
            WHEN message.role = 'assistant' THEN parent.id
            ELSE message.id
        END AS turn_id,
        CASE
            WHEN message.role = 'assistant' THEN 1
            ELSE 0
        END AS turn_position
    FROM messages_legacy_v2 AS message
    LEFT JOIN messages_legacy_v2 AS parent
      ON message.role = 'assistant'
     AND parent.conversation_id = message.conversation_id
     AND parent.id = message.parent_id
     AND parent.role = 'user'
)
INSERT INTO messages (
    id,
    conversation_id,
    parent_id,
    role,
    content,
    status,
    generation_id,
    created_at
)
SELECT
    id,
    conversation_id,
    LAG(id) OVER (
        PARTITION BY conversation_id
        ORDER BY turn_created_at, turn_id, turn_position, created_at, id
    ),
    role,
    content,
    status,
    generation_id,
    created_at
FROM migration_order;

DROP TABLE messages_legacy_v2;

CREATE INDEX messages_conversation_created
    ON messages(conversation_id, created_at, id);
CREATE INDEX messages_conversation_parent
    ON messages(conversation_id, parent_id, created_at, id);
CREATE UNIQUE INDEX messages_generation_unique
    ON messages(generation_id)
    WHERE generation_id IS NOT NULL;

CREATE TABLE conversation_branches (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    title TEXT,
    fork_message_id TEXT,
    head_message_id TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (conversation_id, id),
    FOREIGN KEY (conversation_id, fork_message_id)
        REFERENCES messages(conversation_id, id),
    FOREIGN KEY (conversation_id, head_message_id)
        REFERENCES messages(conversation_id, id),
    CHECK (fork_message_id IS NULL OR fork_message_id <> '')
);

CREATE INDEX conversation_branches_conversation_updated
    ON conversation_branches(conversation_id, updated_at DESC, id);

INSERT INTO conversation_branches (
    id,
    conversation_id,
    title,
    fork_message_id,
    head_message_id,
    created_at,
    updated_at
)
SELECT
    'branch:' || conversations.id,
    conversations.id,
    NULL,
    NULL,
    (
        SELECT message.id
        FROM messages AS message
        WHERE message.conversation_id = conversations.id
          AND NOT EXISTS (
            SELECT 1
            FROM messages AS child
            WHERE child.conversation_id = message.conversation_id
              AND child.parent_id = message.id
          )
        ORDER BY message.created_at DESC, message.id DESC
        LIMIT 1
    ),
    conversations.created_at,
    conversations.updated_at
FROM conversations;

CREATE TABLE conversation_state (
    conversation_id TEXT PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
    active_branch_id TEXT NOT NULL,
    selected_mode TEXT NOT NULL CHECK (selected_mode IN ('chat', 'story')),
    updated_at TEXT NOT NULL,
    FOREIGN KEY (conversation_id, active_branch_id)
        REFERENCES conversation_branches(conversation_id, id)
);

INSERT INTO conversation_state (
    conversation_id,
    active_branch_id,
    selected_mode,
    updated_at
)
SELECT
    id,
    'branch:' || id,
    'chat',
    updated_at
FROM conversations;

CREATE TABLE generations (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    branch_id TEXT NOT NULL,
    user_message_id TEXT NOT NULL REFERENCES messages(id),
    assistant_message_id TEXT UNIQUE REFERENCES messages(id) ON DELETE SET NULL,
    mode TEXT NOT NULL CHECK (mode IN ('chat', 'story')),
    model TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('running', 'complete', 'cancelled', 'failed')),
    input_tokens INTEGER,
    output_tokens INTEGER,
    error_code TEXT,
    started_at TEXT NOT NULL,
    finished_at TEXT,
    FOREIGN KEY (conversation_id, branch_id)
        REFERENCES conversation_branches(conversation_id, id),
    CHECK (input_tokens IS NULL OR input_tokens >= 0),
    CHECK (output_tokens IS NULL OR output_tokens >= 0),
    CHECK (
        (status = 'running' AND finished_at IS NULL)
        OR (status <> 'running' AND finished_at IS NOT NULL)
    )
);

CREATE INDEX generations_conversation_branch_started
    ON generations(conversation_id, branch_id, started_at, id);

INSERT INTO generations (
    id,
    conversation_id,
    branch_id,
    user_message_id,
    assistant_message_id,
    mode,
    model,
    status,
    input_tokens,
    output_tokens,
    error_code,
    started_at,
    finished_at
)
SELECT
    assistant.generation_id,
    assistant.conversation_id,
    'branch:' || assistant.conversation_id,
    user_message.id,
    assistant.id,
    'chat',
    '',
    CASE assistant.status
        WHEN 'pending' THEN 'running'
        ELSE assistant.status
    END,
    NULL,
    NULL,
    NULL,
    assistant.created_at,
    CASE
        WHEN assistant.status = 'pending' THEN NULL
        ELSE assistant.created_at
    END
FROM messages AS assistant
JOIN messages AS user_message
  ON user_message.conversation_id = assistant.conversation_id
 AND user_message.id = assistant.parent_id
 AND user_message.role = 'user'
WHERE assistant.role = 'assistant'
  AND assistant.generation_id IS NOT NULL;
