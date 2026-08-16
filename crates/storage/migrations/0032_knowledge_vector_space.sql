PRAGMA foreign_keys = ON;

-- The original knowledge-vector draft identified only route and dimensions.
-- Those mutable projections cannot prove that two vectors share a provider
-- space. Rebuild the immutable table so every new vector carries the exact
-- provider contract digest. Legacy vectors receive an impossible sentinel and
-- are never selected by provider-native queries.
DROP TRIGGER knowledge_embeddings_no_update;
DROP TRIGGER knowledge_embeddings_no_delete;
DROP INDEX knowledge_embeddings_entry;

ALTER TABLE knowledge_embeddings RENAME TO knowledge_embeddings_legacy;

CREATE TABLE knowledge_embeddings (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(trim(id)) > 0),
    book_revision_id TEXT NOT NULL,
    entry_id TEXT NOT NULL,
    task_profile_revision_id TEXT
        REFERENCES task_profile_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    model_route_id TEXT NOT NULL
        REFERENCES provider_models(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    dimensions INTEGER NOT NULL CHECK (
        dimensions BETWEEN 1 AND 1048576
    ),
    vector_space_sha256 TEXT NOT NULL CHECK (
        length(vector_space_sha256) = 64
        AND vector_space_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    encoding TEXT NOT NULL CHECK (encoding = 'f32le'),
    vector_blob BLOB NOT NULL CHECK (
        length(vector_blob) = dimensions * 4
    ),
    vector_sha256 TEXT NOT NULL CHECK (
        length(vector_sha256) = 64
        AND vector_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    FOREIGN KEY (book_revision_id, entry_id)
        REFERENCES knowledge_entries(book_revision_id, entry_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    UNIQUE (
        book_revision_id,
        entry_id,
        task_profile_revision_id,
        model_route_id,
        dimensions,
        vector_space_sha256
    )
);

INSERT INTO knowledge_embeddings (
    id, book_revision_id, entry_id, task_profile_revision_id,
    model_route_id, dimensions, vector_space_sha256, encoding,
    vector_blob, vector_sha256, created_at
)
SELECT
    id, book_revision_id, entry_id, task_profile_revision_id,
    model_route_id, dimensions,
    '0000000000000000000000000000000000000000000000000000000000000000',
    encoding, vector_blob, vector_sha256, created_at
FROM knowledge_embeddings_legacy;

DROP TABLE knowledge_embeddings_legacy;

CREATE INDEX knowledge_embeddings_entry
    ON knowledge_embeddings(
        book_revision_id,
        entry_id,
        model_route_id,
        id
    );

CREATE INDEX knowledge_embeddings_exact_space
    ON knowledge_embeddings(
        book_revision_id,
        task_profile_revision_id,
        model_route_id,
        dimensions,
        vector_space_sha256,
        entry_id,
        id
    );

CREATE TRIGGER knowledge_embeddings_no_update
BEFORE UPDATE ON knowledge_embeddings
BEGIN
    SELECT RAISE(ABORT, 'knowledge embeddings are immutable');
END;

CREATE TRIGGER knowledge_embeddings_no_delete
BEFORE DELETE ON knowledge_embeddings
BEGIN
    SELECT RAISE(ABORT, 'knowledge embeddings are immutable');
END;
