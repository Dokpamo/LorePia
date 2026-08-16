PRAGMA foreign_keys = ON;

CREATE TABLE knowledge_books (
    id TEXT PRIMARY KEY NOT NULL
        REFERENCES content_objects(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    schema_version INTEGER NOT NULL CHECK (schema_version >= 1),
    revision INTEGER NOT NULL CHECK (revision >= 1),
    scan_depth INTEGER NOT NULL CHECK (scan_depth >= 0),
    token_budget INTEGER NOT NULL CHECK (token_budget >= 0),
    recursive INTEGER NOT NULL CHECK (recursive IN (0, 1)),
    max_recursion_depth INTEGER NOT NULL CHECK (
        max_recursion_depth >= 0
    ),
    document_json TEXT NOT NULL CHECK (
        json_valid(document_json)
        AND json_type(document_json) = 'object'
        AND length(CAST(document_json AS BLOB)) <= 16777216
    ),
    provenance_json TEXT NOT NULL CHECK (
        json_valid(provenance_json)
        AND json_type(provenance_json) = 'object'
        AND length(CAST(provenance_json AS BLOB)) <= 65536
    ),
    source_kind TEXT NOT NULL CHECK (
        source_kind IN (
            'application_built_in',
            'user_created',
            'imported_standard',
            'imported_package',
            'generated',
            'local_override',
            'migrated'
        )
    ),
    source_hash TEXT CHECK (
        source_hash IS NULL
        OR (
            length(source_hash) = 64
            AND source_hash NOT GLOB '*[^0-9a-f]*'
        )
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    updated_at TEXT NOT NULL CHECK (length(trim(updated_at)) > 0),
    deleted_at TEXT CHECK (
        deleted_at IS NULL OR length(trim(deleted_at)) > 0
    ),
    UNIQUE (id, revision),
    CHECK (
        recursive = 1 OR max_recursion_depth = 0
    )
);

CREATE TRIGGER knowledge_books_kind_guard
BEFORE INSERT ON knowledge_books
WHEN NOT EXISTS (
    SELECT 1
    FROM content_objects
    WHERE id = NEW.id
      AND object_kind = 'knowledge_book'
)
BEGIN
    SELECT RAISE(ABORT, 'knowledge book object kind is invalid');
END;

CREATE INDEX knowledge_books_active_name
    ON knowledge_books(name COLLATE NOCASE, id)
    WHERE deleted_at IS NULL;

CREATE TABLE knowledge_book_revisions (
    revision_id TEXT PRIMARY KEY NOT NULL,
    knowledge_book_id TEXT NOT NULL
        REFERENCES knowledge_books(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    revision_no INTEGER NOT NULL CHECK (revision_no >= 1),
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    description TEXT NOT NULL DEFAULT '',
    token_budget INTEGER NOT NULL CHECK (token_budget >= 0),
    scan_depth INTEGER NOT NULL CHECK (scan_depth >= 0),
    recursive INTEGER NOT NULL CHECK (recursive IN (0, 1)),
    max_recursion_depth INTEGER NOT NULL CHECK (
        max_recursion_depth >= 0
    ),
    document_json TEXT NOT NULL CHECK (
        json_valid(document_json)
        AND json_type(document_json) = 'object'
        AND length(CAST(document_json AS BLOB)) <= 16777216
    ),
    UNIQUE (knowledge_book_id, revision_id),
    UNIQUE (knowledge_book_id, revision_no),
    FOREIGN KEY (knowledge_book_id, revision_id)
        REFERENCES content_revisions(object_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    CHECK (recursive = 1 OR max_recursion_depth = 0)
);

CREATE INDEX knowledge_book_revisions_history
    ON knowledge_book_revisions(
        knowledge_book_id,
        revision_no DESC,
        revision_id
    );

CREATE TABLE knowledge_entries (
    book_revision_id TEXT NOT NULL
        REFERENCES knowledge_book_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    entry_id TEXT NOT NULL CHECK (length(trim(entry_id)) > 0),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    parent_entry_id TEXT,
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    content TEXT NOT NULL CHECK (
        length(CAST(content AS BLOB)) BETWEEN 1 AND 8388608
    ),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    activation_kind TEXT NOT NULL CHECK (
        activation_kind IN (
            'always',
            'manual',
            'keyword',
            'regex',
            'semantic',
            'condition',
            'any',
            'all'
        )
    ),
    activation_json TEXT NOT NULL CHECK (
        json_valid(activation_json)
        AND json_type(activation_json) = 'object'
        AND length(CAST(activation_json AS BLOB)) <= 1048576
    ),
    priority INTEGER NOT NULL,
    importance INTEGER NOT NULL CHECK (importance BETWEEN 0 AND 255),
    placement TEXT NOT NULL CHECK (
        placement IN (
            'retrieved_context',
            'before_older_history',
            'before_recent_history',
            'post_history'
        )
    ),
    token_priority INTEGER NOT NULL CHECK (
        token_priority BETWEEN 0 AND 65535
    ),
    min_tokens INTEGER CHECK (min_tokens IS NULL OR min_tokens >= 0),
    max_tokens INTEGER CHECK (max_tokens IS NULL OR max_tokens >= 0),
    reserve_tokens INTEGER CHECK (
        reserve_tokens IS NULL OR reserve_tokens >= 0
    ),
    overflow_policy TEXT NOT NULL CHECK (
        overflow_policy IN (
            'reject',
            'drop_block',
            'trim_head',
            'trim_tail',
            'keep_latest_items',
            'summarize',
            'reduce_knowledge_entries'
        )
    ),
    activation_probability_basis_points INTEGER NOT NULL CHECK (
        activation_probability_basis_points BETWEEN 0 AND 10000
    ),
    cacheable INTEGER NOT NULL CHECK (cacheable IN (0, 1)),
    provenance_json TEXT NOT NULL CHECK (
        json_valid(provenance_json)
        AND json_type(provenance_json) = 'object'
        AND length(CAST(provenance_json AS BLOB)) <= 65536
    ),
    document_json TEXT NOT NULL CHECK (
        json_valid(document_json)
        AND json_type(document_json) = 'object'
        AND length(CAST(document_json AS BLOB)) <= 10485760
    ),
    PRIMARY KEY (book_revision_id, entry_id),
    UNIQUE (book_revision_id, ordinal),
    FOREIGN KEY (book_revision_id, parent_entry_id)
        REFERENCES knowledge_entries(book_revision_id, entry_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    CHECK (parent_entry_id IS NULL OR parent_entry_id <> entry_id),
    CHECK (
        min_tokens IS NULL
        OR max_tokens IS NULL
        OR min_tokens <= max_tokens
    )
);

CREATE INDEX knowledge_entries_resolution_order
    ON knowledge_entries(
        book_revision_id,
        enabled,
        priority DESC,
        importance DESC,
        entry_id
    );
CREATE INDEX knowledge_entries_parent
    ON knowledge_entries(book_revision_id, parent_entry_id, entry_id)
    WHERE parent_entry_id IS NOT NULL;

CREATE TABLE knowledge_activation_terms (
    book_revision_id TEXT NOT NULL,
    entry_id TEXT NOT NULL,
    rule_path TEXT NOT NULL CHECK (length(trim(rule_path)) > 0),
    term_ordinal INTEGER NOT NULL CHECK (term_ordinal >= 0),
    term_kind TEXT NOT NULL CHECK (
        term_kind IN (
            'primary_keyword',
            'secondary_keyword',
            'regex',
            'semantic',
            'condition'
        )
    ),
    term_text TEXT,
    normalized_term TEXT,
    term_json TEXT CHECK (
        term_json IS NULL
        OR (
            json_valid(term_json)
            AND length(CAST(term_json AS BLOB)) <= 262144
        )
    ),
    case_sensitive INTEGER NOT NULL CHECK (
        case_sensitive IN (0, 1)
    ),
    whole_word INTEGER NOT NULL CHECK (whole_word IN (0, 1)),
    PRIMARY KEY (
        book_revision_id,
        entry_id,
        rule_path,
        term_ordinal
    ),
    FOREIGN KEY (book_revision_id, entry_id)
        REFERENCES knowledge_entries(book_revision_id, entry_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    CHECK (
        (term_text IS NULL) <> (term_json IS NULL)
    ),
    CHECK (
        term_text IS NULL
        OR length(trim(term_text)) > 0
    )
);

CREATE INDEX knowledge_activation_terms_lookup
    ON knowledge_activation_terms(
        term_kind,
        normalized_term,
        book_revision_id,
        entry_id
    )
    WHERE normalized_term IS NOT NULL;
CREATE INDEX knowledge_activation_terms_entry
    ON knowledge_activation_terms(
        book_revision_id,
        entry_id,
        rule_path,
        term_ordinal
    );

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
        model_route_id,
        dimensions,
        vector_sha256
    )
);

CREATE INDEX knowledge_embeddings_entry
    ON knowledge_embeddings(
        book_revision_id,
        entry_id,
        model_route_id,
        id
    );

CREATE TABLE knowledge_manual_activations (
    book_revision_id TEXT NOT NULL,
    entry_id TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    branch_id TEXT NOT NULL,
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    state_revision INTEGER NOT NULL CHECK (state_revision >= 1),
    updated_at TEXT NOT NULL CHECK (length(trim(updated_at)) > 0),
    PRIMARY KEY (
        book_revision_id,
        entry_id,
        conversation_id,
        branch_id
    ),
    FOREIGN KEY (book_revision_id, entry_id)
        REFERENCES knowledge_entries(book_revision_id, entry_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    FOREIGN KEY (conversation_id, branch_id)
        REFERENCES conversation_branches(conversation_id, id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE
);

CREATE INDEX knowledge_manual_activations_branch
    ON knowledge_manual_activations(
        conversation_id,
        branch_id,
        enabled,
        book_revision_id,
        entry_id
    );

CREATE TRIGGER knowledge_manual_activations_version_guard
BEFORE UPDATE ON knowledge_manual_activations
WHEN
    NEW.book_revision_id != OLD.book_revision_id
    OR NEW.entry_id != OLD.entry_id
    OR NEW.conversation_id != OLD.conversation_id
    OR NEW.branch_id != OLD.branch_id
    OR NEW.state_revision != OLD.state_revision + 1
BEGIN
    SELECT RAISE(ABORT, 'knowledge activation update is not versioned');
END;

CREATE TABLE knowledge_activation_logs (
    plan_id TEXT NOT NULL
        REFERENCES generation_prompt_plans(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    book_revision_id TEXT NOT NULL,
    entry_id TEXT NOT NULL,
    activation_source TEXT NOT NULL CHECK (
        activation_source IN (
            'always',
            'manual',
            'keyword',
            'regex',
            'semantic',
            'condition',
            'recursive'
        )
    ),
    selected INTEGER NOT NULL CHECK (selected IN (0, 1)),
    score_millionths INTEGER CHECK (
        score_millionths IS NULL
        OR score_millionths BETWEEN 0 AND 1000000
    ),
    estimated_tokens INTEGER NOT NULL CHECK (estimated_tokens >= 0),
    reason_json TEXT NOT NULL CHECK (
        json_valid(reason_json)
        AND json_type(reason_json) = 'object'
        AND length(CAST(reason_json AS BLOB)) <= 262144
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    PRIMARY KEY (plan_id, ordinal),
    UNIQUE (
        plan_id,
        book_revision_id,
        entry_id,
        activation_source
    ),
    FOREIGN KEY (book_revision_id, entry_id)
        REFERENCES knowledge_entries(book_revision_id, entry_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

CREATE INDEX knowledge_activation_logs_plan_selection
    ON knowledge_activation_logs(plan_id, selected, ordinal);

CREATE TABLE prompt_preset_knowledge_books (
    prompt_preset_revision_id TEXT NOT NULL
        REFERENCES prompt_preset_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    knowledge_book_revision_id TEXT NOT NULL
        REFERENCES knowledge_book_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    config_json TEXT NOT NULL CHECK (
        json_valid(config_json)
        AND json_type(config_json) = 'object'
        AND length(CAST(config_json AS BLOB)) <= 262144
    ),
    PRIMARY KEY (prompt_preset_revision_id, ordinal),
    UNIQUE (
        prompt_preset_revision_id,
        knowledge_book_revision_id
    )
);

CREATE INDEX prompt_preset_knowledge_books_target
    ON prompt_preset_knowledge_books(
        knowledge_book_revision_id,
        prompt_preset_revision_id
    );

CREATE TABLE generation_prompt_plan_knowledge_selections (
    plan_id TEXT NOT NULL
        REFERENCES generation_prompt_plans(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    book_revision_id TEXT NOT NULL,
    entry_id TEXT NOT NULL,
    selected INTEGER NOT NULL CHECK (selected IN (0, 1)),
    activation_source TEXT NOT NULL CHECK (
        activation_source IN (
            'always',
            'manual',
            'keyword',
            'regex',
            'semantic',
            'condition',
            'recursive'
        )
    ),
    score_millionths INTEGER CHECK (
        score_millionths IS NULL
        OR score_millionths BETWEEN 0 AND 1000000
    ),
    estimated_tokens INTEGER NOT NULL CHECK (estimated_tokens >= 0),
    reason_json TEXT NOT NULL CHECK (
        json_valid(reason_json)
        AND json_type(reason_json) = 'object'
        AND length(CAST(reason_json AS BLOB)) <= 262144
    ),
    PRIMARY KEY (plan_id, ordinal),
    UNIQUE (plan_id, book_revision_id, entry_id),
    FOREIGN KEY (book_revision_id, entry_id)
        REFERENCES knowledge_entries(book_revision_id, entry_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

CREATE INDEX generation_plan_knowledge_selected
    ON generation_prompt_plan_knowledge_selections(
        plan_id,
        selected,
        ordinal
    );

CREATE TRIGGER knowledge_book_revisions_no_update
BEFORE UPDATE ON knowledge_book_revisions
BEGIN
    SELECT RAISE(ABORT, 'knowledge book revisions are immutable');
END;
CREATE TRIGGER knowledge_book_revisions_no_delete
BEFORE DELETE ON knowledge_book_revisions
BEGIN
    SELECT RAISE(ABORT, 'knowledge book revisions are immutable');
END;
CREATE TRIGGER knowledge_entries_no_update
BEFORE UPDATE ON knowledge_entries
BEGIN
    SELECT RAISE(ABORT, 'knowledge entries are immutable');
END;
CREATE TRIGGER knowledge_entries_no_delete
BEFORE DELETE ON knowledge_entries
BEGIN
    SELECT RAISE(ABORT, 'knowledge entries are immutable');
END;
CREATE TRIGGER knowledge_activation_terms_no_update
BEFORE UPDATE ON knowledge_activation_terms
BEGIN
    SELECT RAISE(ABORT, 'knowledge activation terms are immutable');
END;
CREATE TRIGGER knowledge_activation_terms_no_delete
BEFORE DELETE ON knowledge_activation_terms
BEGIN
    SELECT RAISE(ABORT, 'knowledge activation terms are immutable');
END;
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
CREATE TRIGGER knowledge_activation_logs_no_update
BEFORE UPDATE ON knowledge_activation_logs
BEGIN
    SELECT RAISE(ABORT, 'knowledge activation logs are immutable');
END;
CREATE TRIGGER knowledge_activation_logs_seal_guard
BEFORE INSERT ON knowledge_activation_logs
WHEN EXISTS (
    SELECT 1 FROM generation_prompt_plan_seals
    WHERE plan_id = NEW.plan_id
)
BEGIN
    SELECT RAISE(ABORT, 'resolved prompt plan is already sealed');
END;
CREATE TRIGGER knowledge_activation_logs_no_delete
BEFORE DELETE ON knowledge_activation_logs
BEGIN
    SELECT RAISE(ABORT, 'knowledge activation logs are immutable');
END;
CREATE TRIGGER generation_plan_knowledge_no_update
BEFORE UPDATE ON generation_prompt_plan_knowledge_selections
BEGIN
    SELECT RAISE(ABORT, 'resolved knowledge selections are immutable');
END;
CREATE TRIGGER generation_plan_knowledge_seal_guard
BEFORE INSERT ON generation_prompt_plan_knowledge_selections
WHEN EXISTS (
    SELECT 1 FROM generation_prompt_plan_seals
    WHERE plan_id = NEW.plan_id
)
BEGIN
    SELECT RAISE(ABORT, 'resolved prompt plan is already sealed');
END;
CREATE TRIGGER generation_plan_knowledge_no_delete
BEFORE DELETE ON generation_prompt_plan_knowledge_selections
BEGIN
    SELECT RAISE(ABORT, 'resolved knowledge selections are immutable');
END;
