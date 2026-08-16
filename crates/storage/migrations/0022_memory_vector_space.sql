PRAGMA foreign_keys = ON;

-- Existing pre-vector-space rows receive an impossible legacy sentinel. New
-- provider-native queries always carry a real contract hash and therefore
-- cannot mix those rows into an exact vector space.
ALTER TABLE memory_embeddings
ADD COLUMN vector_space_sha256 TEXT NOT NULL
    DEFAULT '0000000000000000000000000000000000000000000000000000000000000000'
    CHECK (
        length(vector_space_sha256) = 64
        AND vector_space_sha256 NOT GLOB '*[^0-9a-f]*'
    );

CREATE INDEX memory_embeddings_exact_space
    ON memory_embeddings(
        task_profile_revision_id,
        model_route_id,
        dimensions,
        vector_space_sha256,
        record_revision_id,
        id
    );

-- Query embeddings are durable provider intents. A terminal or interrupted
-- row is never silently retried; explicit retry advances the CAS revision.
CREATE TABLE memory_query_embeddings (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(trim(id)) > 0),
    idempotency_key TEXT NOT NULL UNIQUE CHECK (
        length(trim(idempotency_key)) > 0
    ),
    memory_profile_revision_id TEXT NOT NULL
        REFERENCES memory_profile_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    task_profile_revision_id TEXT NOT NULL
        REFERENCES task_profile_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    conversation_id TEXT NOT NULL
        REFERENCES conversations(id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE,
    branch_id TEXT NOT NULL,
    source_start_message_id TEXT NOT NULL,
    source_end_message_id TEXT NOT NULL,
    query_sha256 TEXT NOT NULL CHECK (
        length(query_sha256) = 64
        AND query_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    vector_space_sha256 TEXT NOT NULL CHECK (
        length(vector_space_sha256) = 64
        AND vector_space_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    model_route_id TEXT NOT NULL
        REFERENCES provider_models(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    dimensions INTEGER NOT NULL CHECK (
        dimensions BETWEEN 1 AND 32768
    ),
    state TEXT NOT NULL CHECK (
        state IN (
            'queued',
            'running',
            'interrupted',
            'succeeded',
            'failed',
            'cancelled'
        )
    ),
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    started_at TEXT,
    finished_at TEXT,
    error_code TEXT CHECK (
        error_code IS NULL OR (
            length(error_code) BETWEEN 1 AND 128
            AND error_code NOT GLOB '*[^a-z0-9_]*'
        )
    ),
    encoding TEXT CHECK (encoding IS NULL OR encoding = 'f32le'),
    vector_blob BLOB,
    vector_sha256 TEXT CHECK (
        vector_sha256 IS NULL OR (
            length(vector_sha256) = 64
            AND vector_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    updated_at TEXT NOT NULL CHECK (length(trim(updated_at)) > 0),
    FOREIGN KEY (conversation_id, branch_id)
        REFERENCES conversation_branches(conversation_id, id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE,
    FOREIGN KEY (conversation_id, source_start_message_id)
        REFERENCES messages(conversation_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    FOREIGN KEY (conversation_id, source_end_message_id)
        REFERENCES messages(conversation_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    UNIQUE (
        memory_profile_revision_id,
        task_profile_revision_id,
        conversation_id,
        branch_id,
        source_start_message_id,
        source_end_message_id,
        query_sha256,
        vector_space_sha256
    ),
    CHECK (
        (state = 'queued'
            AND started_at IS NULL
            AND finished_at IS NULL
            AND error_code IS NULL)
        OR (state = 'running'
            AND started_at IS NOT NULL
            AND finished_at IS NULL
            AND error_code IS NULL)
        OR (state = 'interrupted'
            AND started_at IS NOT NULL
            AND finished_at IS NULL
            AND error_code IS NOT NULL)
        OR (state IN ('succeeded', 'failed', 'cancelled')
            AND finished_at IS NOT NULL)
    ),
    CHECK (
        (state = 'succeeded'
            AND encoding = 'f32le'
            AND vector_blob IS NOT NULL
            AND length(vector_blob) = dimensions * 4
            AND vector_sha256 IS NOT NULL
            AND error_code IS NULL)
        OR (state <> 'succeeded'
            AND encoding IS NULL
            AND vector_blob IS NULL
            AND vector_sha256 IS NULL)
    )
);

CREATE INDEX memory_query_embeddings_state
    ON memory_query_embeddings(state, created_at, id);

CREATE TRIGGER memory_query_embeddings_revision_guard
BEFORE UPDATE ON memory_query_embeddings
WHEN
    NEW.id != OLD.id
    OR NEW.idempotency_key != OLD.idempotency_key
    OR NEW.memory_profile_revision_id != OLD.memory_profile_revision_id
    OR NEW.task_profile_revision_id != OLD.task_profile_revision_id
    OR NEW.conversation_id != OLD.conversation_id
    OR NEW.branch_id != OLD.branch_id
    OR NEW.source_start_message_id != OLD.source_start_message_id
    OR NEW.source_end_message_id != OLD.source_end_message_id
    OR NEW.query_sha256 != OLD.query_sha256
    OR NEW.vector_space_sha256 != OLD.vector_space_sha256
    OR NEW.model_route_id != OLD.model_route_id
    OR NEW.dimensions != OLD.dimensions
    OR NEW.created_at != OLD.created_at
    OR NEW.revision != OLD.revision + 1
    OR (
        OLD.state = 'queued'
        AND NEW.state NOT IN ('running', 'cancelled')
    )
    OR (
        OLD.state = 'running'
        AND NEW.state NOT IN (
            'interrupted',
            'succeeded',
            'failed',
            'cancelled'
        )
    )
    OR (
        OLD.state = 'interrupted'
        AND NEW.state NOT IN ('queued', 'cancelled')
    )
    OR (
        OLD.state IN ('failed', 'cancelled')
        AND NEW.state != 'queued'
    )
    OR OLD.state = 'succeeded'
BEGIN
    SELECT RAISE(ABORT, 'memory query embedding update is not a legal revision');
END;

CREATE TRIGGER memory_query_embeddings_no_delete
BEFORE DELETE ON memory_query_embeddings
BEGIN
    SELECT RAISE(ABORT, 'memory query embeddings are immutable audit records');
END;
