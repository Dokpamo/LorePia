PRAGMA foreign_keys = ON;

CREATE TABLE memory_summary_schemas (
    id TEXT PRIMARY KEY NOT NULL
        REFERENCES content_objects(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    schema_version INTEGER NOT NULL CHECK (schema_version >= 1),
    revision INTEGER NOT NULL CHECK (revision >= 1),
    schema_json TEXT NOT NULL CHECK (
        json_valid(schema_json)
        AND json_type(schema_json) = 'object'
        AND length(CAST(schema_json AS BLOB)) <= 1048576
    ),
    document_json TEXT NOT NULL CHECK (
        json_valid(document_json)
        AND json_type(document_json) = 'object'
        AND length(CAST(document_json AS BLOB)) <= 2097152
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    updated_at TEXT NOT NULL CHECK (length(trim(updated_at)) > 0),
    deleted_at TEXT CHECK (
        deleted_at IS NULL OR length(trim(deleted_at)) > 0
    ),
    UNIQUE (id, revision)
);

CREATE TRIGGER memory_summary_schemas_kind_guard
BEFORE INSERT ON memory_summary_schemas
WHEN NOT EXISTS (
    SELECT 1
    FROM content_objects
    WHERE id = NEW.id
      AND object_kind = 'memory_summary_schema'
)
BEGIN
    SELECT RAISE(ABORT, 'memory summary schema object kind is invalid');
END;

CREATE TABLE memory_summary_schema_revisions (
    revision_id TEXT PRIMARY KEY NOT NULL,
    summary_schema_id TEXT NOT NULL
        REFERENCES memory_summary_schemas(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    revision_no INTEGER NOT NULL CHECK (revision_no >= 1),
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    schema_json TEXT NOT NULL CHECK (
        json_valid(schema_json)
        AND json_type(schema_json) = 'object'
        AND length(CAST(schema_json AS BLOB)) <= 1048576
    ),
    schema_sha256 TEXT NOT NULL CHECK (
        length(schema_sha256) = 64
        AND schema_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    document_json TEXT NOT NULL CHECK (
        json_valid(document_json)
        AND json_type(document_json) = 'object'
        AND length(CAST(document_json AS BLOB)) <= 2097152
    ),
    UNIQUE (summary_schema_id, revision_id),
    UNIQUE (summary_schema_id, revision_no),
    FOREIGN KEY (summary_schema_id, revision_id)
        REFERENCES content_revisions(object_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

CREATE TABLE memory_profiles (
    id TEXT PRIMARY KEY NOT NULL
        REFERENCES content_objects(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    schema_version INTEGER NOT NULL CHECK (schema_version >= 1),
    revision INTEGER NOT NULL CHECK (revision >= 1),
    summary_task_profile_id TEXT NOT NULL
        REFERENCES task_profiles(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    embedding_task_profile_id TEXT
        REFERENCES task_profiles(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    turns_per_summary INTEGER NOT NULL CHECK (turns_per_summary > 0),
    recent_raw_budget INTEGER NOT NULL CHECK (recent_raw_budget >= 0),
    episodic_budget INTEGER NOT NULL CHECK (episodic_budget >= 0),
    semantic_budget INTEGER NOT NULL CHECK (semantic_budget >= 0),
    retrieval_count INTEGER NOT NULL CHECK (retrieval_count >= 0),
    recency_weight REAL NOT NULL CHECK (
        recency_weight BETWEEN 0.0 AND 1.0
    ),
    similarity_weight REAL NOT NULL CHECK (
        similarity_weight BETWEEN 0.0 AND 1.0
    ),
    importance_weight REAL NOT NULL CHECK (
        importance_weight BETWEEN 0.0 AND 1.0
    ),
    preserve_invalidated_records INTEGER NOT NULL CHECK (
        preserve_invalidated_records IN (0, 1)
    ),
    summary_schema_id TEXT NOT NULL
        REFERENCES memory_summary_schemas(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    document_json TEXT NOT NULL CHECK (
        json_valid(document_json)
        AND json_type(document_json) = 'object'
        AND length(CAST(document_json AS BLOB)) <= 2097152
    ),
    provenance_json TEXT NOT NULL CHECK (
        json_valid(provenance_json)
        AND json_type(provenance_json) = 'object'
        AND length(CAST(provenance_json AS BLOB)) <= 65536
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    updated_at TEXT NOT NULL CHECK (length(trim(updated_at)) > 0),
    deleted_at TEXT CHECK (
        deleted_at IS NULL OR length(trim(deleted_at)) > 0
    ),
    UNIQUE (id, revision),
    CHECK (
        recency_weight + similarity_weight + importance_weight > 0.0
    )
);

CREATE TRIGGER memory_profiles_kind_guard
BEFORE INSERT ON memory_profiles
WHEN NOT EXISTS (
    SELECT 1
    FROM content_objects
    WHERE id = NEW.id
      AND object_kind = 'memory_profile'
)
BEGIN
    SELECT RAISE(ABORT, 'memory profile object kind is invalid');
END;

CREATE TRIGGER memory_profiles_task_guard
BEFORE INSERT ON memory_profiles
WHEN
    NOT EXISTS (
        SELECT 1
        FROM task_profiles
        WHERE id = NEW.summary_task_profile_id
          AND task_kind = 'memory_summary'
    )
    OR (
        NEW.embedding_task_profile_id IS NOT NULL
        AND NOT EXISTS (
            SELECT 1
            FROM task_profiles
            WHERE id = NEW.embedding_task_profile_id
              AND task_kind = 'memory_embedding'
        )
    )
BEGIN
    SELECT RAISE(ABORT, 'memory profile task kind is invalid');
END;

CREATE INDEX memory_profiles_active_name
    ON memory_profiles(name COLLATE NOCASE, id)
    WHERE deleted_at IS NULL;

CREATE TABLE memory_profile_revisions (
    revision_id TEXT PRIMARY KEY NOT NULL,
    memory_profile_id TEXT NOT NULL
        REFERENCES memory_profiles(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    revision_no INTEGER NOT NULL CHECK (revision_no >= 1),
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    summary_task_profile_revision_id TEXT NOT NULL
        REFERENCES task_profile_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    embedding_task_profile_revision_id TEXT
        REFERENCES task_profile_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    turns_per_summary INTEGER NOT NULL CHECK (turns_per_summary > 0),
    recent_raw_budget INTEGER NOT NULL CHECK (recent_raw_budget >= 0),
    episodic_budget INTEGER NOT NULL CHECK (episodic_budget >= 0),
    semantic_budget INTEGER NOT NULL CHECK (semantic_budget >= 0),
    retrieval_count INTEGER NOT NULL CHECK (retrieval_count >= 0),
    recency_weight_millionths INTEGER NOT NULL CHECK (
        recency_weight_millionths BETWEEN 0 AND 1000000
    ),
    similarity_weight_millionths INTEGER NOT NULL CHECK (
        similarity_weight_millionths BETWEEN 0 AND 1000000
    ),
    importance_weight_millionths INTEGER NOT NULL CHECK (
        importance_weight_millionths BETWEEN 0 AND 1000000
    ),
    preserve_invalidated_records INTEGER NOT NULL CHECK (
        preserve_invalidated_records IN (0, 1)
    ),
    summary_schema_revision_id TEXT NOT NULL
        REFERENCES memory_summary_schema_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    document_json TEXT NOT NULL CHECK (
        json_valid(document_json)
        AND json_type(document_json) = 'object'
        AND length(CAST(document_json AS BLOB)) <= 2097152
    ),
    UNIQUE (memory_profile_id, revision_id),
    UNIQUE (memory_profile_id, revision_no),
    FOREIGN KEY (memory_profile_id, revision_id)
        REFERENCES content_revisions(object_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    CHECK (
        recency_weight_millionths
        + similarity_weight_millionths
        + importance_weight_millionths > 0
    )
);

CREATE TRIGGER memory_profile_revisions_task_guard
BEFORE INSERT ON memory_profile_revisions
WHEN
    NOT EXISTS (
        SELECT 1
        FROM task_profile_revisions
        WHERE revision_id = NEW.summary_task_profile_revision_id
          AND task_kind = 'memory_summary'
    )
    OR (
        NEW.embedding_task_profile_revision_id IS NOT NULL
        AND NOT EXISTS (
            SELECT 1
            FROM task_profile_revisions
            WHERE revision_id = NEW.embedding_task_profile_revision_id
              AND task_kind = 'memory_embedding'
        )
    )
BEGIN
    SELECT RAISE(ABORT, 'memory profile revision task kind is invalid');
END;

CREATE TABLE memory_records (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(trim(id)) > 0),
    conversation_id TEXT NOT NULL
        REFERENCES conversations(id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE,
    branch_id TEXT NOT NULL,
    source_start_message_id TEXT NOT NULL,
    source_end_message_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (
        kind IN (
            'episodic_event',
            'character_fact',
            'relationship_change',
            'user_preference',
            'world_state',
            'unresolved_thread',
            'conversation_summary',
            'creator_pinned'
        )
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    UNIQUE (conversation_id, id),
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
        ON DELETE RESTRICT
);

CREATE INDEX memory_records_branch_kind
    ON memory_records(conversation_id, branch_id, kind, created_at, id);
CREATE INDEX memory_records_source_range
    ON memory_records(
        conversation_id,
        source_start_message_id,
        source_end_message_id,
        id
    );

CREATE TABLE memory_record_revisions (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(trim(id)) > 0),
    record_id TEXT NOT NULL
        REFERENCES memory_records(id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE,
    revision_no INTEGER NOT NULL CHECK (revision_no >= 1),
    parent_revision_id TEXT,
    title TEXT NOT NULL,
    summary TEXT NOT NULL CHECK (
        length(CAST(summary AS BLOB)) <= 4194304
    ),
    structured_data_json TEXT NOT NULL CHECK (
        json_valid(structured_data_json)
        AND json_type(structured_data_json) = 'object'
        AND json_type(structured_data_json, '$.schema_version') = 'integer'
        AND length(CAST(structured_data_json AS BLOB)) <= 4194304
    ),
    importance INTEGER NOT NULL CHECK (importance BETWEEN 0 AND 255),
    content_sha256 TEXT NOT NULL CHECK (
        length(content_sha256) = 64
        AND content_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    provenance_json TEXT NOT NULL CHECK (
        json_valid(provenance_json)
        AND json_type(provenance_json) = 'object'
        AND length(CAST(provenance_json AS BLOB)) <= 65536
    ),
    document_json TEXT NOT NULL CHECK (
        json_valid(document_json)
        AND json_type(document_json) = 'object'
        AND length(CAST(document_json AS BLOB)) <= 8388608
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    UNIQUE (record_id, revision_no),
    UNIQUE (record_id, id),
    FOREIGN KEY (record_id, parent_revision_id)
        REFERENCES memory_record_revisions(record_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    CHECK (
        (revision_no = 1 AND parent_revision_id IS NULL)
        OR (revision_no > 1 AND parent_revision_id IS NOT NULL)
    )
);

CREATE INDEX memory_record_revisions_history
    ON memory_record_revisions(record_id, revision_no DESC, id);

CREATE TRIGGER memory_record_revisions_append_guard
BEFORE INSERT ON memory_record_revisions
WHEN
    NEW.revision_no != (
        SELECT COALESCE(MAX(revision_no), 0) + 1
        FROM memory_record_revisions
        WHERE record_id = NEW.record_id
    )
    OR (
        NEW.parent_revision_id IS NOT NULL
        AND NOT EXISTS (
            SELECT 1
            FROM memory_record_revisions
            WHERE record_id = NEW.record_id
              AND id = NEW.parent_revision_id
              AND revision_no = NEW.revision_no - 1
        )
    )
BEGIN
    SELECT RAISE(ABORT, 'memory record revision is not append-only');
END;

CREATE TABLE memory_record_state (
    record_id TEXT PRIMARY KEY NOT NULL
        REFERENCES memory_records(id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE,
    active_revision_id TEXT NOT NULL,
    pinned INTEGER NOT NULL CHECK (pinned IN (0, 1)),
    invalidated_at TEXT,
    invalidation_reason TEXT,
    excluded_from_conversation_at TEXT,
    excluded_from_character_at TEXT,
    deleted_at TEXT,
    state_version INTEGER NOT NULL CHECK (state_version >= 1),
    updated_at TEXT NOT NULL CHECK (length(trim(updated_at)) > 0),
    FOREIGN KEY (record_id, active_revision_id)
        REFERENCES memory_record_revisions(record_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    CHECK (
        (invalidated_at IS NULL AND invalidation_reason IS NULL)
        OR (
            invalidated_at IS NOT NULL
            AND invalidation_reason IS NOT NULL
            AND length(trim(invalidation_reason)) > 0
        )
    )
);

CREATE INDEX memory_record_state_active
    ON memory_record_state(
        invalidated_at,
        deleted_at,
        pinned,
        record_id
    );

CREATE TRIGGER memory_record_state_version_guard
BEFORE UPDATE ON memory_record_state
WHEN
    NEW.record_id != OLD.record_id
    OR NEW.state_version != OLD.state_version + 1
BEGIN
    SELECT RAISE(ABORT, 'memory record state update is not versioned');
END;

CREATE TABLE memory_record_keywords (
    record_revision_id TEXT NOT NULL
        REFERENCES memory_record_revisions(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    keyword TEXT NOT NULL CHECK (length(trim(keyword)) > 0),
    normalized_keyword TEXT NOT NULL CHECK (
        length(trim(normalized_keyword)) > 0
    ),
    PRIMARY KEY (record_revision_id, ordinal),
    UNIQUE (record_revision_id, normalized_keyword)
);

CREATE INDEX memory_record_keywords_lookup
    ON memory_record_keywords(normalized_keyword, record_revision_id);

CREATE TABLE memory_jobs (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(trim(id)) > 0),
    idempotency_key TEXT NOT NULL UNIQUE CHECK (
        length(trim(idempotency_key)) > 0
    ),
    job_kind TEXT NOT NULL CHECK (
        job_kind IN ('summary', 'embedding', 'invalidate_range')
    ),
    memory_profile_revision_id TEXT
        REFERENCES memory_profile_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    task_profile_revision_id TEXT
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
    input_fingerprint_sha256 TEXT NOT NULL CHECK (
        length(input_fingerprint_sha256) = 64
        AND input_fingerprint_sha256 NOT GLOB '*[^0-9a-f]*'
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
    -- Mutable current-row CAS token. Immutable memory content history lives in
    -- memory_record_revisions and must not be conflated with this revision.
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
    attempts INTEGER NOT NULL CHECK (attempts >= 0),
    available_at TEXT NOT NULL CHECK (length(trim(available_at)) > 0),
    started_at TEXT,
    finished_at TEXT,
    result_record_id TEXT
        REFERENCES memory_records(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    failure_json TEXT CHECK (
        failure_json IS NULL
        OR (
            json_valid(failure_json)
            AND json_type(failure_json) = 'object'
            AND length(CAST(failure_json AS BLOB)) <= 65536
        )
    ),
    payload_json TEXT NOT NULL CHECK (
        json_valid(payload_json)
        AND json_type(payload_json) = 'object'
        AND length(CAST(payload_json AS BLOB)) <= 1048576
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
    CHECK (
        (memory_profile_revision_id IS NULL
            AND task_profile_revision_id IS NULL)
        OR (memory_profile_revision_id IS NOT NULL
            AND task_profile_revision_id IS NOT NULL)
    ),
    CHECK (
        (state = 'queued' AND started_at IS NULL AND finished_at IS NULL)
        OR (state = 'running' AND started_at IS NOT NULL AND finished_at IS NULL)
        OR (state = 'interrupted'
            AND started_at IS NOT NULL
            AND finished_at IS NULL)
        OR (state IN ('succeeded', 'failed', 'cancelled')
            AND finished_at IS NOT NULL)
    ),
    CHECK (
        state <> 'failed' OR failure_json IS NOT NULL
    ),
    CHECK (
        state = 'failed' OR failure_json IS NULL
    )
);

CREATE UNIQUE INDEX memory_jobs_one_live_input
    ON memory_jobs(
        job_kind,
        conversation_id,
        branch_id,
        source_start_message_id,
        source_end_message_id,
        input_fingerprint_sha256
    )
    WHERE state IN ('queued', 'running');
CREATE INDEX memory_jobs_queue
    ON memory_jobs(state, available_at, created_at, id);
CREATE INDEX memory_jobs_branch_history
    ON memory_jobs(conversation_id, branch_id, created_at DESC, id);

CREATE TRIGGER memory_jobs_initial_state_guard
BEFORE INSERT ON memory_jobs
WHEN NEW.state != 'queued' OR NEW.revision != 1
BEGIN
    SELECT RAISE(ABORT, 'memory job must begin queued');
END;

CREATE TRIGGER memory_jobs_revision_guard
BEFORE UPDATE ON memory_jobs
WHEN
    NEW.id != OLD.id
    OR NEW.idempotency_key != OLD.idempotency_key
    OR NEW.job_kind != OLD.job_kind
    OR NEW.memory_profile_revision_id
       IS NOT OLD.memory_profile_revision_id
    OR NEW.task_profile_revision_id
       IS NOT OLD.task_profile_revision_id
    OR NEW.conversation_id != OLD.conversation_id
    OR NEW.branch_id != OLD.branch_id
    OR NEW.source_start_message_id != OLD.source_start_message_id
    OR NEW.source_end_message_id != OLD.source_end_message_id
    OR NEW.input_fingerprint_sha256 != OLD.input_fingerprint_sha256
    OR NEW.created_at != OLD.created_at
    -- Every state write must advance the current-row CAS token exactly once.
    OR NEW.revision != OLD.revision + 1
    OR (
        OLD.state = 'queued'
        AND NEW.state NOT IN ('running', 'failed', 'cancelled')
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
    OR OLD.state IN ('succeeded', 'failed', 'cancelled')
BEGIN
    SELECT RAISE(ABORT, 'memory job update is not a legal revision');
END;

CREATE TABLE memory_embeddings (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(trim(id)) > 0),
    record_revision_id TEXT NOT NULL
        REFERENCES memory_record_revisions(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
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
    encoding TEXT NOT NULL CHECK (encoding = 'f32le'),
    vector_blob BLOB NOT NULL CHECK (
        length(vector_blob) = dimensions * 4
    ),
    vector_sha256 TEXT NOT NULL CHECK (
        length(vector_sha256) = 64
        AND vector_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    UNIQUE (
        record_revision_id,
        model_route_id,
        dimensions,
        vector_sha256
    )
);

CREATE INDEX memory_embeddings_record
    ON memory_embeddings(record_revision_id, model_route_id, id);

CREATE TABLE memory_retrieval_logs (
    plan_id TEXT NOT NULL
        REFERENCES generation_prompt_plans(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    record_revision_id TEXT NOT NULL
        REFERENCES memory_record_revisions(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    selected INTEGER NOT NULL CHECK (selected IN (0, 1)),
    recency_score_millionths INTEGER NOT NULL CHECK (
        recency_score_millionths BETWEEN 0 AND 1000000
    ),
    similarity_score_millionths INTEGER CHECK (
        similarity_score_millionths IS NULL
        OR similarity_score_millionths BETWEEN 0 AND 1000000
    ),
    importance_score_millionths INTEGER NOT NULL CHECK (
        importance_score_millionths BETWEEN 0 AND 1000000
    ),
    estimated_tokens INTEGER NOT NULL CHECK (estimated_tokens >= 0),
    reason_json TEXT NOT NULL CHECK (
        json_valid(reason_json)
        AND json_type(reason_json) = 'object'
        AND length(CAST(reason_json AS BLOB)) <= 262144
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    PRIMARY KEY (plan_id, ordinal),
    UNIQUE (plan_id, record_revision_id)
);

CREATE INDEX memory_retrieval_logs_plan_selected
    ON memory_retrieval_logs(plan_id, selected, ordinal);

CREATE TABLE prompt_preset_memory_profiles (
    prompt_preset_revision_id TEXT PRIMARY KEY NOT NULL
        REFERENCES prompt_preset_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    memory_profile_revision_id TEXT NOT NULL
        REFERENCES memory_profile_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    config_json TEXT NOT NULL CHECK (
        json_valid(config_json)
        AND json_type(config_json) = 'object'
        AND length(CAST(config_json AS BLOB)) <= 262144
    )
);

CREATE INDEX prompt_preset_memory_profiles_target
    ON prompt_preset_memory_profiles(
        memory_profile_revision_id,
        prompt_preset_revision_id
    );

CREATE TABLE generation_prompt_plan_memory_selections (
    plan_id TEXT NOT NULL
        REFERENCES generation_prompt_plans(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    record_revision_id TEXT NOT NULL
        REFERENCES memory_record_revisions(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    selected INTEGER NOT NULL CHECK (selected IN (0, 1)),
    recency_score_millionths INTEGER NOT NULL CHECK (
        recency_score_millionths BETWEEN 0 AND 1000000
    ),
    similarity_score_millionths INTEGER CHECK (
        similarity_score_millionths IS NULL
        OR similarity_score_millionths BETWEEN 0 AND 1000000
    ),
    importance_score_millionths INTEGER NOT NULL CHECK (
        importance_score_millionths BETWEEN 0 AND 1000000
    ),
    estimated_tokens INTEGER NOT NULL CHECK (estimated_tokens >= 0),
    reason_json TEXT NOT NULL CHECK (
        json_valid(reason_json)
        AND json_type(reason_json) = 'object'
        AND length(CAST(reason_json AS BLOB)) <= 262144
    ),
    PRIMARY KEY (plan_id, ordinal),
    UNIQUE (plan_id, record_revision_id)
);

CREATE TABLE memory_record_events (
    record_id TEXT NOT NULL
        REFERENCES memory_records(id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE,
    sequence INTEGER NOT NULL CHECK (sequence >= 1),
    event_kind TEXT NOT NULL CHECK (
        event_kind IN (
            'created',
            'edited',
            'pinned',
            'unpinned',
            'invalidated',
            'restored',
            'excluded_conversation',
            'excluded_character',
            'deleted'
        )
    ),
    from_revision_id TEXT,
    to_revision_id TEXT,
    payload_json TEXT NOT NULL CHECK (
        json_valid(payload_json)
        AND json_type(payload_json) = 'object'
        AND length(CAST(payload_json AS BLOB)) <= 262144
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    PRIMARY KEY (record_id, sequence),
    FOREIGN KEY (record_id, from_revision_id)
        REFERENCES memory_record_revisions(record_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    FOREIGN KEY (record_id, to_revision_id)
        REFERENCES memory_record_revisions(record_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

CREATE TRIGGER memory_record_events_append_guard
BEFORE INSERT ON memory_record_events
WHEN NEW.sequence != (
    SELECT COALESCE(MAX(sequence), 0) + 1
    FROM memory_record_events
    WHERE record_id = NEW.record_id
)
BEGIN
    SELECT RAISE(ABORT, 'memory record event sequence is not append-only');
END;

CREATE TRIGGER memory_summary_schema_revisions_no_update
BEFORE UPDATE ON memory_summary_schema_revisions
BEGIN
    SELECT RAISE(ABORT, 'memory summary schema revisions are immutable');
END;
CREATE TRIGGER memory_summary_schema_revisions_no_delete
BEFORE DELETE ON memory_summary_schema_revisions
BEGIN
    SELECT RAISE(ABORT, 'memory summary schema revisions are immutable');
END;
CREATE TRIGGER memory_profile_revisions_no_update
BEFORE UPDATE ON memory_profile_revisions
BEGIN
    SELECT RAISE(ABORT, 'memory profile revisions are immutable');
END;
CREATE TRIGGER memory_profile_revisions_no_delete
BEFORE DELETE ON memory_profile_revisions
BEGIN
    SELECT RAISE(ABORT, 'memory profile revisions are immutable');
END;
CREATE TRIGGER memory_record_revisions_no_update
BEFORE UPDATE ON memory_record_revisions
BEGIN
    SELECT RAISE(ABORT, 'memory record revisions are immutable');
END;
CREATE TRIGGER memory_record_revisions_no_delete
BEFORE DELETE ON memory_record_revisions
BEGIN
    SELECT RAISE(ABORT, 'memory record revisions are immutable');
END;
CREATE TRIGGER memory_record_keywords_no_update
BEFORE UPDATE ON memory_record_keywords
BEGIN
    SELECT RAISE(ABORT, 'memory record keywords are immutable');
END;
CREATE TRIGGER memory_record_keywords_no_delete
BEFORE DELETE ON memory_record_keywords
BEGIN
    SELECT RAISE(ABORT, 'memory record keywords are immutable');
END;
CREATE TRIGGER memory_embeddings_no_update
BEFORE UPDATE ON memory_embeddings
BEGIN
    SELECT RAISE(ABORT, 'memory embeddings are immutable');
END;
CREATE TRIGGER memory_embeddings_no_delete
BEFORE DELETE ON memory_embeddings
BEGIN
    SELECT RAISE(ABORT, 'memory embeddings are immutable');
END;
CREATE TRIGGER memory_retrieval_logs_no_update
BEFORE UPDATE ON memory_retrieval_logs
BEGIN
    SELECT RAISE(ABORT, 'memory retrieval logs are immutable');
END;
CREATE TRIGGER memory_retrieval_logs_seal_guard
BEFORE INSERT ON memory_retrieval_logs
WHEN EXISTS (
    SELECT 1 FROM generation_prompt_plan_seals
    WHERE plan_id = NEW.plan_id
)
BEGIN
    SELECT RAISE(ABORT, 'resolved prompt plan is already sealed');
END;
CREATE TRIGGER memory_retrieval_logs_no_delete
BEFORE DELETE ON memory_retrieval_logs
BEGIN
    SELECT RAISE(ABORT, 'memory retrieval logs are immutable');
END;
CREATE TRIGGER generation_plan_memory_no_update
BEFORE UPDATE ON generation_prompt_plan_memory_selections
BEGIN
    SELECT RAISE(ABORT, 'resolved memory selections are immutable');
END;
CREATE TRIGGER generation_plan_memory_seal_guard
BEFORE INSERT ON generation_prompt_plan_memory_selections
WHEN EXISTS (
    SELECT 1 FROM generation_prompt_plan_seals
    WHERE plan_id = NEW.plan_id
)
BEGIN
    SELECT RAISE(ABORT, 'resolved prompt plan is already sealed');
END;
CREATE TRIGGER generation_plan_memory_no_delete
BEFORE DELETE ON generation_prompt_plan_memory_selections
BEGIN
    SELECT RAISE(ABORT, 'resolved memory selections are immutable');
END;
CREATE TRIGGER memory_record_events_no_update
BEFORE UPDATE ON memory_record_events
BEGIN
    SELECT RAISE(ABORT, 'memory record events are immutable');
END;
CREATE TRIGGER memory_record_events_no_delete
BEFORE DELETE ON memory_record_events
BEGIN
    SELECT RAISE(ABORT, 'memory record events are immutable');
END;
