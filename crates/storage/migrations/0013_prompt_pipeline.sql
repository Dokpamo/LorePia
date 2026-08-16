PRAGMA foreign_keys = ON;

CREATE TABLE prompt_presets (
    id TEXT PRIMARY KEY NOT NULL
        REFERENCES content_objects(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    schema_version INTEGER NOT NULL CHECK (schema_version >= 1),
    revision INTEGER NOT NULL CHECK (revision >= 1),
    default_generation_preset_id TEXT
        REFERENCES generation_presets(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
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
    UNIQUE (id, revision)
);

CREATE TRIGGER prompt_presets_kind_guard
BEFORE INSERT ON prompt_presets
WHEN NOT EXISTS (
    SELECT 1
    FROM content_objects
    WHERE id = NEW.id
      AND object_kind = 'prompt_preset'
)
BEGIN
    SELECT RAISE(ABORT, 'prompt preset object kind is invalid');
END;

CREATE INDEX prompt_presets_active_name
    ON prompt_presets(name COLLATE NOCASE, id)
    WHERE deleted_at IS NULL;

CREATE TABLE prompt_preset_revisions (
    revision_id TEXT PRIMARY KEY NOT NULL,
    prompt_preset_id TEXT NOT NULL
        REFERENCES prompt_presets(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    revision_no INTEGER NOT NULL CHECK (revision_no >= 1),
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    default_generation_preset_id TEXT
        REFERENCES generation_presets(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    metadata_json TEXT NOT NULL CHECK (
        json_valid(metadata_json)
        AND json_type(metadata_json) = 'object'
        AND length(CAST(metadata_json AS BLOB)) <= 262144
    ),
    document_json TEXT NOT NULL CHECK (
        json_valid(document_json)
        AND json_type(document_json) = 'object'
        AND length(CAST(document_json AS BLOB)) <= 16777216
    ),
    UNIQUE (prompt_preset_id, revision_id),
    UNIQUE (prompt_preset_id, revision_no),
    FOREIGN KEY (prompt_preset_id, revision_id)
        REFERENCES content_revisions(object_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

CREATE INDEX prompt_preset_revisions_history
    ON prompt_preset_revisions(
        prompt_preset_id,
        revision_no DESC,
        revision_id
    );

CREATE TRIGGER prompt_preset_revisions_no_update
BEFORE UPDATE ON prompt_preset_revisions
BEGIN
    SELECT RAISE(ABORT, 'prompt preset revisions are immutable');
END;

CREATE TRIGGER prompt_preset_revisions_no_delete
BEFORE DELETE ON prompt_preset_revisions
BEGIN
    SELECT RAISE(ABORT, 'prompt preset revisions are immutable');
END;

CREATE TABLE prompt_variables (
    owner_revision_id TEXT NOT NULL
        REFERENCES prompt_preset_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    variable_key TEXT NOT NULL CHECK (
        length(trim(variable_key)) > 0
        AND instr(variable_key, char(0)) = 0
    ),
    value_type TEXT NOT NULL CHECK (
        value_type IN (
            'bool',
            'integer',
            'decimal',
            'text',
            'enum',
            'string_list'
        )
    ),
    scope TEXT NOT NULL CHECK (
        scope IN (
            'app',
            'user',
            'persona',
            'character',
            'conversation',
            'branch',
            'session',
            'turn',
            'module'
        )
    ),
    namespace TEXT,
    default_value_json TEXT NOT NULL CHECK (
        json_valid(default_value_json)
        AND length(CAST(default_value_json AS BLOB)) <= 65536
    ),
    sensitive INTEGER NOT NULL CHECK (sensitive IN (0, 1)),
    payload_json TEXT NOT NULL CHECK (
        json_valid(payload_json)
        AND json_type(payload_json) = 'object'
        AND length(CAST(payload_json AS BLOB)) <= 262144
    ),
    PRIMARY KEY (owner_revision_id, variable_key),
    CHECK (
        (scope = 'module' AND namespace IS NOT NULL)
        OR (scope <> 'module' AND namespace IS NULL)
    )
);

CREATE TABLE prompt_controls (
    owner_revision_id TEXT NOT NULL
        REFERENCES prompt_preset_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    control_id TEXT NOT NULL CHECK (length(trim(control_id)) > 0),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    kind TEXT NOT NULL CHECK (
        kind IN (
            'toggle',
            'select',
            'multi_select',
            'text',
            'number',
            'slider',
            'section',
            'caption',
            'divider'
        )
    ),
    variable_key TEXT,
    label TEXT NOT NULL,
    description TEXT NOT NULL,
    options_json TEXT NOT NULL CHECK (
        json_valid(options_json)
        AND json_type(options_json) = 'array'
        AND length(CAST(options_json AS BLOB)) <= 262144
    ),
    minimum REAL,
    maximum REAL,
    step REAL,
    visibility_condition_json TEXT CHECK (
        visibility_condition_json IS NULL
        OR (
            json_valid(visibility_condition_json)
            AND json_type(visibility_condition_json) = 'object'
            AND length(CAST(visibility_condition_json AS BLOB)) <= 262144
        )
    ),
    regenerate_required INTEGER NOT NULL CHECK (
        regenerate_required IN (0, 1)
    ),
    document_json TEXT NOT NULL CHECK (
        json_valid(document_json)
        AND json_type(document_json) = 'object'
        AND length(CAST(document_json AS BLOB)) <= 524288
    ),
    PRIMARY KEY (owner_revision_id, control_id),
    UNIQUE (owner_revision_id, ordinal),
    FOREIGN KEY (owner_revision_id, variable_key)
        REFERENCES prompt_variables(owner_revision_id, variable_key)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    CHECK (
        (
            kind IN ('section', 'caption', 'divider')
            AND variable_key IS NULL
        )
        OR (
            kind NOT IN ('section', 'caption', 'divider')
            AND variable_key IS NOT NULL
        )
    ),
    CHECK (
        minimum IS NULL
        OR maximum IS NULL
        OR minimum <= maximum
    ),
    CHECK (step IS NULL OR step > 0)
);

CREATE INDEX prompt_controls_order
    ON prompt_controls(owner_revision_id, ordinal, control_id);

CREATE TABLE prompt_blocks (
    owner_revision_id TEXT NOT NULL
        REFERENCES prompt_preset_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    block_id TEXT NOT NULL CHECK (length(trim(block_id)) > 0),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    kind TEXT NOT NULL CHECK (
        kind IN (
            'static_instruction',
            'character_identity',
            'character_description',
            'character_personality',
            'scenario',
            'user_persona',
            'dialogue_examples',
            'world_knowledge',
            'retrieved_memory',
            'conversation_summary',
            'history_slice',
            'latest_user_turn',
            'author_note',
            'post_history_instruction',
            'assistant_prefill',
            'group_context'
        )
    ),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    authority TEXT NOT NULL CHECK (
        authority IN (
            'application',
            'creator',
            'user',
            'conversation',
            'imported_content'
        )
    ),
    role_hint TEXT NOT NULL CHECK (
        role_hint IN (
            'system',
            'developer',
            'user',
            'assistant',
            'provider_default'
        )
    ),
    template_json TEXT CHECK (
        template_json IS NULL
        OR (
            json_valid(template_json)
            AND json_type(template_json) = 'object'
            AND length(CAST(template_json AS BLOB)) <= 2097152
        )
    ),
    condition_json TEXT CHECK (
        condition_json IS NULL
        OR (
            json_valid(condition_json)
            AND json_type(condition_json) = 'object'
            AND length(CAST(condition_json AS BLOB)) <= 262144
        )
    ),
    source_json TEXT NOT NULL CHECK (
        json_valid(source_json)
        AND json_type(source_json) = 'object'
        AND length(CAST(source_json AS BLOB)) <= 262144
    ),
    placement_zone TEXT NOT NULL CHECK (
        placement_zone IN (
            'application_policy',
            'preset_instruction',
            'character_context',
            'retrieved_context',
            'older_history',
            'recent_enhancement',
            'recent_history',
            'post_history',
            'latest_user',
            'assistant_prefill'
        )
    ),
    history_selector_json TEXT CHECK (
        history_selector_json IS NULL
        OR (
            json_valid(history_selector_json)
            AND json_type(history_selector_json) = 'object'
            AND length(CAST(history_selector_json AS BLOB)) <= 65536
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
    merge_policy TEXT NOT NULL CHECK (
        merge_policy IN (
            'separate_message',
            'merge_with_previous_same_role'
        )
    ),
    provenance_json TEXT NOT NULL CHECK (
        json_valid(provenance_json)
        AND json_type(provenance_json) = 'object'
        AND length(CAST(provenance_json AS BLOB)) <= 65536
    ),
    document_json TEXT NOT NULL CHECK (
        json_valid(document_json)
        AND json_type(document_json) = 'object'
        AND length(CAST(document_json AS BLOB)) <= 4194304
    ),
    PRIMARY KEY (owner_revision_id, block_id),
    UNIQUE (owner_revision_id, ordinal),
    CHECK (
        min_tokens IS NULL
        OR max_tokens IS NULL
        OR min_tokens <= max_tokens
    ),
    CHECK (
        (kind = 'history_slice' AND history_selector_json IS NOT NULL)
        OR (kind <> 'history_slice' AND history_selector_json IS NULL)
    ),
    CHECK (
        kind NOT IN ('latest_user_turn', 'history_slice')
        OR template_json IS NULL
    )
);

CREATE INDEX prompt_blocks_order
    ON prompt_blocks(owner_revision_id, ordinal, block_id);
CREATE UNIQUE INDEX prompt_blocks_one_latest_user
    ON prompt_blocks(owner_revision_id)
    WHERE kind = 'latest_user_turn';
CREATE UNIQUE INDEX prompt_blocks_one_prefill
    ON prompt_blocks(owner_revision_id)
    WHERE kind = 'assistant_prefill';

CREATE TABLE prompt_cache_boundaries (
    owner_revision_id TEXT NOT NULL
        REFERENCES prompt_preset_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    id TEXT NOT NULL CHECK (length(trim(id)) > 0),
    after_block_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    role_filter TEXT NOT NULL CHECK (
        role_filter IN ('all', 'system_like', 'exact_role')
    ),
    exact_role TEXT CHECK (
        exact_role IS NULL
        OR exact_role IN (
            'system',
            'developer',
            'user',
            'assistant'
        )
    ),
    ttl TEXT NOT NULL CHECK (
        ttl IN ('provider_default', 'short', 'long')
    ),
    ttl_seconds INTEGER CHECK (
        ttl_seconds IS NULL OR ttl_seconds > 0
    ),
    mode TEXT NOT NULL CHECK (
        mode IN ('automatic', 'explicit', 'disabled')
    ),
    document_json TEXT NOT NULL CHECK (
        json_valid(document_json)
        AND json_type(document_json) = 'object'
        AND length(CAST(document_json AS BLOB)) <= 262144
    ),
    PRIMARY KEY (owner_revision_id, id),
    UNIQUE (owner_revision_id, after_block_id, id),
    UNIQUE (owner_revision_id, ordinal),
    FOREIGN KEY (owner_revision_id, after_block_id)
        REFERENCES prompt_blocks(owner_revision_id, block_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    CHECK (
        (role_filter = 'exact_role' AND exact_role IS NOT NULL)
        OR (role_filter <> 'exact_role' AND exact_role IS NULL)
    ),
    CHECK (
        (ttl = 'provider_default' AND ttl_seconds IS NULL)
        OR (ttl <> 'provider_default')
    )
);

CREATE INDEX prompt_cache_boundaries_order
    ON prompt_cache_boundaries(owner_revision_id, ordinal, id);

CREATE TABLE task_profiles (
    id TEXT PRIMARY KEY NOT NULL
        REFERENCES content_objects(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    task_kind TEXT NOT NULL CHECK (
        task_kind IN (
            'memory_summary',
            'memory_embedding',
            'translation',
            'emotion_classification',
            'state_extraction',
            'image_prompt',
            'title_generation'
        )
    ),
    schema_version INTEGER NOT NULL CHECK (schema_version >= 1),
    revision INTEGER NOT NULL CHECK (revision >= 1),
    model_route_id TEXT NOT NULL
        REFERENCES provider_models(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    generation_preset_id TEXT NOT NULL
        REFERENCES generation_presets(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    timeout_ms INTEGER NOT NULL CHECK (timeout_ms BETWEEN 1 AND 600000),
    rate_limit_requests INTEGER NOT NULL CHECK (rate_limit_requests > 0),
    rate_limit_per_seconds INTEGER NOT NULL CHECK (
        rate_limit_per_seconds > 0
    ),
    concurrency_limit INTEGER NOT NULL CHECK (
        concurrency_limit BETWEEN 1 AND 64
    ),
    document_json TEXT NOT NULL CHECK (
        json_valid(document_json)
        AND json_type(document_json) = 'object'
        AND length(CAST(document_json AS BLOB)) <= 1048576
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    updated_at TEXT NOT NULL CHECK (length(trim(updated_at)) > 0),
    deleted_at TEXT CHECK (
        deleted_at IS NULL OR length(trim(deleted_at)) > 0
    ),
    UNIQUE (id, revision)
);

CREATE TRIGGER task_profiles_kind_guard
BEFORE INSERT ON task_profiles
WHEN NOT EXISTS (
    SELECT 1
    FROM content_objects
    WHERE id = NEW.id
      AND object_kind = 'task_profile'
)
BEGIN
    SELECT RAISE(ABORT, 'task profile object kind is invalid');
END;

CREATE TRIGGER task_profiles_route_guard
BEFORE INSERT ON task_profiles
WHEN NOT EXISTS (
    SELECT 1
    FROM generation_presets
    WHERE id = NEW.generation_preset_id
      AND model_route_id = NEW.model_route_id
)
BEGIN
    SELECT RAISE(ABORT, 'task profile generation preset route is inconsistent');
END;

CREATE TRIGGER task_profiles_route_update_guard
BEFORE UPDATE OF model_route_id, generation_preset_id ON task_profiles
WHEN NOT EXISTS (
    SELECT 1
    FROM generation_presets
    WHERE id = NEW.generation_preset_id
      AND model_route_id = NEW.model_route_id
)
BEGIN
    SELECT RAISE(ABORT, 'task profile generation preset route is inconsistent');
END;

CREATE INDEX task_profiles_kind_active
    ON task_profiles(task_kind, id)
    WHERE deleted_at IS NULL;

CREATE TABLE task_profile_revisions (
    revision_id TEXT PRIMARY KEY NOT NULL,
    task_profile_id TEXT NOT NULL
        REFERENCES task_profiles(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    revision_no INTEGER NOT NULL CHECK (revision_no >= 1),
    task_kind TEXT NOT NULL CHECK (
        task_kind IN (
            'memory_summary',
            'memory_embedding',
            'translation',
            'emotion_classification',
            'state_extraction',
            'image_prompt',
            'title_generation'
        )
    ),
    model_route_id TEXT NOT NULL
        REFERENCES provider_models(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    generation_preset_id TEXT NOT NULL
        REFERENCES generation_presets(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    timeout_ms INTEGER NOT NULL CHECK (timeout_ms BETWEEN 1 AND 600000),
    rate_limit_requests INTEGER NOT NULL CHECK (rate_limit_requests > 0),
    rate_limit_per_seconds INTEGER NOT NULL CHECK (
        rate_limit_per_seconds > 0
    ),
    concurrency_limit INTEGER NOT NULL CHECK (
        concurrency_limit BETWEEN 1 AND 64
    ),
    payload_json TEXT NOT NULL CHECK (
        json_valid(payload_json)
        AND json_type(payload_json) = 'object'
        AND length(CAST(payload_json AS BLOB)) <= 1048576
    ),
    UNIQUE (task_profile_id, revision_id),
    UNIQUE (task_profile_id, revision_no),
    FOREIGN KEY (task_profile_id, revision_id)
        REFERENCES content_revisions(object_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

CREATE TRIGGER task_profile_revisions_route_guard
BEFORE INSERT ON task_profile_revisions
WHEN NOT EXISTS (
    SELECT 1
    FROM generation_presets
    WHERE id = NEW.generation_preset_id
      AND model_route_id = NEW.model_route_id
)
BEGIN
    SELECT RAISE(ABORT, 'task profile revision route is inconsistent');
END;

CREATE TRIGGER task_profile_revisions_no_update
BEFORE UPDATE ON task_profile_revisions
BEGIN
    SELECT RAISE(ABORT, 'task profile revisions are immutable');
END;

CREATE TRIGGER task_profile_revisions_no_delete
BEFORE DELETE ON task_profile_revisions
BEGIN
    SELECT RAISE(ABORT, 'task profile revisions are immutable');
END;

CREATE TABLE task_profile_fallbacks (
    task_profile_revision_id TEXT NOT NULL
        REFERENCES task_profile_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    model_route_id TEXT NOT NULL
        REFERENCES provider_models(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    generation_preset_id TEXT
        REFERENCES generation_presets(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    timeout_override_ms INTEGER CHECK (
        timeout_override_ms IS NULL
        OR timeout_override_ms BETWEEN 1 AND 600000
    ),
    payload_json TEXT NOT NULL CHECK (
        json_valid(payload_json)
        AND json_type(payload_json) = 'object'
        AND length(CAST(payload_json AS BLOB)) <= 262144
    ),
    PRIMARY KEY (task_profile_revision_id, ordinal),
    UNIQUE (task_profile_revision_id, model_route_id)
);

CREATE TRIGGER task_profile_fallbacks_route_guard
BEFORE INSERT ON task_profile_fallbacks
WHEN
    (
        NEW.generation_preset_id IS NOT NULL
        AND NOT EXISTS (
            SELECT 1
            FROM generation_presets
            WHERE id = NEW.generation_preset_id
              AND model_route_id = NEW.model_route_id
        )
    )
    OR EXISTS (
        SELECT 1
        FROM task_profile_revisions
        WHERE revision_id = NEW.task_profile_revision_id
          AND model_route_id = NEW.model_route_id
    )
BEGIN
    SELECT RAISE(ABORT, 'task profile fallback route is inconsistent');
END;

CREATE TABLE prompt_preset_bindings (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(trim(id)) > 0),
    revision INTEGER NOT NULL CHECK (revision >= 1),
    prompt_preset_id TEXT NOT NULL
        REFERENCES prompt_presets(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    resolution_mode TEXT NOT NULL CHECK (
        resolution_mode IN ('active', 'pinned')
    ),
    pinned_revision_id TEXT,
    scope_kind TEXT NOT NULL CHECK (
        scope_kind IN (
            'app',
            'user',
            'persona',
            'character',
            'conversation',
            'branch'
        )
    ),
    persona_id TEXT
        REFERENCES personas(object_id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE,
    character_id TEXT
        REFERENCES characters(id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE,
    conversation_id TEXT
        REFERENCES conversations(id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE,
    branch_id TEXT,
    generation_preset_override_id TEXT
        REFERENCES generation_presets(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    response_length TEXT NOT NULL DEFAULT 'balanced' CHECK (
        response_length IN ('short', 'balanced', 'long')
    ),
    creativity INTEGER NOT NULL DEFAULT 50 CHECK (
        creativity BETWEEN 0 AND 100
    ),
    reasoning_effort TEXT NOT NULL DEFAULT 'provider_default' CHECK (
        length(trim(reasoning_effort)) > 0
        AND instr(reasoning_effort, char(0)) = 0
    ),
    memory_enabled INTEGER NOT NULL DEFAULT 1 CHECK (
        memory_enabled IN (0, 1)
    ),
    knowledge_enabled INTEGER NOT NULL DEFAULT 1 CHECK (
        knowledge_enabled IN (0, 1)
    ),
    variable_overrides_json TEXT NOT NULL CHECK (
        json_valid(variable_overrides_json)
        AND json_type(variable_overrides_json) = 'object'
        AND length(CAST(variable_overrides_json AS BLOB)) <= 1048576
    ),
    priority INTEGER NOT NULL,
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
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
    UNIQUE (id, revision),
    FOREIGN KEY (prompt_preset_id, pinned_revision_id)
        REFERENCES prompt_preset_revisions(
            prompt_preset_id,
            revision_id
        )
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    FOREIGN KEY (conversation_id, branch_id)
        REFERENCES conversation_branches(conversation_id, id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE,
    CHECK (
        (resolution_mode = 'active' AND pinned_revision_id IS NULL)
        OR (resolution_mode = 'pinned' AND pinned_revision_id IS NOT NULL)
    ),
    CHECK (
        (scope_kind IN ('app', 'user')
            AND persona_id IS NULL
            AND character_id IS NULL
            AND conversation_id IS NULL
            AND branch_id IS NULL)
        OR (scope_kind = 'persona'
            AND persona_id IS NOT NULL
            AND character_id IS NULL
            AND conversation_id IS NULL
            AND branch_id IS NULL)
        OR (scope_kind = 'character'
            AND persona_id IS NULL
            AND character_id IS NOT NULL
            AND conversation_id IS NULL
            AND branch_id IS NULL)
        OR (scope_kind = 'conversation'
            AND persona_id IS NULL
            AND character_id IS NULL
            AND conversation_id IS NOT NULL
            AND branch_id IS NULL)
        OR (scope_kind = 'branch'
            AND persona_id IS NULL
            AND character_id IS NULL
            AND conversation_id IS NOT NULL
            AND branch_id IS NOT NULL)
    )
);

CREATE UNIQUE INDEX prompt_bindings_one_app
    ON prompt_preset_bindings(prompt_preset_id)
    WHERE scope_kind = 'app' AND deleted_at IS NULL;
CREATE UNIQUE INDEX prompt_bindings_one_user
    ON prompt_preset_bindings(prompt_preset_id)
    WHERE scope_kind = 'user' AND deleted_at IS NULL;
CREATE UNIQUE INDEX prompt_bindings_one_persona
    ON prompt_preset_bindings(prompt_preset_id, persona_id)
    WHERE scope_kind = 'persona' AND deleted_at IS NULL;
CREATE UNIQUE INDEX prompt_bindings_one_character
    ON prompt_preset_bindings(prompt_preset_id, character_id)
    WHERE scope_kind = 'character' AND deleted_at IS NULL;
CREATE UNIQUE INDEX prompt_bindings_one_conversation
    ON prompt_preset_bindings(prompt_preset_id, conversation_id)
    WHERE scope_kind = 'conversation' AND deleted_at IS NULL;
CREATE UNIQUE INDEX prompt_bindings_one_branch
    ON prompt_preset_bindings(
        prompt_preset_id,
        conversation_id,
        branch_id
    )
    WHERE scope_kind = 'branch' AND deleted_at IS NULL;
CREATE INDEX prompt_bindings_resolution
    ON prompt_preset_bindings(
        scope_kind,
        enabled,
        priority DESC,
        id
    )
    WHERE deleted_at IS NULL;

CREATE TRIGGER prompt_preset_bindings_revision_guard
BEFORE UPDATE ON prompt_preset_bindings
WHEN
    NEW.id != OLD.id
    OR NEW.revision != OLD.revision + 1
BEGIN
    SELECT RAISE(ABORT, 'prompt preset binding update is not versioned');
END;

CREATE TRIGGER prompt_preset_bindings_import_guard
BEFORE INSERT ON prompt_preset_bindings
WHEN
    NEW.enabled = 1
    AND EXISTS (
        SELECT 1
        FROM content_revisions AS revision
        WHERE revision.object_id = NEW.prompt_preset_id
          AND revision.id = COALESCE(
              NEW.pinned_revision_id,
              (
                  SELECT active_revision_id
                  FROM content_object_state
                  WHERE object_id = NEW.prompt_preset_id
              )
          )
          AND revision.source_kind IN (
              'imported_standard',
              'imported_package'
          )
          AND NOT EXISTS (
              SELECT 1
              FROM package_import_approvals AS approval
              JOIN package_import_components AS component
                ON component.import_id = approval.import_id
               AND component.selected = 1
               AND component.disposition IN ('create', 'update')
              JOIN package_import_component_commits AS committed
                ON committed.import_id = component.import_id
               AND committed.component_ordinal = component.ordinal
               AND committed.target_object_id = NEW.prompt_preset_id
               AND committed.target_revision_id = revision.id
          )
    )
BEGIN
    SELECT RAISE(ABORT, 'imported prompt binding requires exact approval');
END;

CREATE TRIGGER prompt_preset_bindings_import_update_guard
BEFORE UPDATE OF
    prompt_preset_id,
    resolution_mode,
    pinned_revision_id,
    enabled
ON prompt_preset_bindings
WHEN
    NEW.enabled = 1
    AND EXISTS (
        SELECT 1
        FROM content_revisions AS revision
        WHERE revision.object_id = NEW.prompt_preset_id
          AND revision.id = COALESCE(
              NEW.pinned_revision_id,
              (
                  SELECT active_revision_id
                  FROM content_object_state
                  WHERE object_id = NEW.prompt_preset_id
              )
          )
          AND revision.source_kind IN (
              'imported_standard',
              'imported_package'
          )
          AND NOT EXISTS (
              SELECT 1
              FROM package_import_approvals AS approval
              JOIN package_import_components AS component
                ON component.import_id = approval.import_id
               AND component.selected = 1
               AND component.disposition IN ('create', 'update')
              JOIN package_import_component_commits AS committed
                ON committed.import_id = component.import_id
               AND committed.component_ordinal = component.ordinal
               AND committed.target_object_id = NEW.prompt_preset_id
               AND committed.target_revision_id = revision.id
          )
    )
BEGIN
    SELECT RAISE(ABORT, 'imported prompt binding requires exact approval');
END;

CREATE TABLE prompt_mode_defaults (
    mode TEXT PRIMARY KEY NOT NULL CHECK (mode IN ('chat', 'story')),
    prompt_preset_id TEXT NOT NULL
        REFERENCES prompt_presets(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    resolution_mode TEXT NOT NULL CHECK (
        resolution_mode IN ('active', 'pinned')
    ),
    pinned_revision_id TEXT,
    updated_at TEXT NOT NULL CHECK (length(trim(updated_at)) > 0),
    FOREIGN KEY (prompt_preset_id, pinned_revision_id)
        REFERENCES prompt_preset_revisions(
            prompt_preset_id,
            revision_id
        )
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    CHECK (
        (resolution_mode = 'active' AND pinned_revision_id IS NULL)
        OR (resolution_mode = 'pinned' AND pinned_revision_id IS NOT NULL)
    )
);

-- Provider-neutral prompt plans are sealed execution evidence. Canonical JSON
-- and normalized rows are both retained; Rust verifies hash/equivalence before
-- sealing. Credentials, auth headers, and unrestricted URLs have no columns.
CREATE TABLE generation_prompt_plans (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(trim(id)) > 0),
    schema_version INTEGER NOT NULL CHECK (schema_version >= 1),
    plan_sha256 TEXT NOT NULL UNIQUE CHECK (
        length(plan_sha256) = 64
        AND plan_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    input_fingerprint_sha256 TEXT NOT NULL CHECK (
        length(input_fingerprint_sha256) = 64
        AND input_fingerprint_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    conversation_id TEXT NOT NULL
        REFERENCES conversations(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    branch_id TEXT NOT NULL,
    head_message_id TEXT,
    latest_user_message_id TEXT NOT NULL,
    latest_user_included INTEGER NOT NULL CHECK (
        latest_user_included IN (0, 1)
    ),
    prompt_preset_id TEXT NOT NULL
        REFERENCES prompt_presets(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    prompt_preset_revision_id TEXT NOT NULL,
    generation_preset_id TEXT
        REFERENCES generation_presets(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    model_route_id TEXT
        REFERENCES provider_models(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    task_profile_revision_id TEXT
        REFERENCES task_profile_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    random_seed INTEGER,
    tokenizer_id TEXT NOT NULL CHECK (length(trim(tokenizer_id)) > 0),
    tokenizer_version TEXT NOT NULL CHECK (
        length(trim(tokenizer_version)) > 0
    ),
    context_limit_tokens INTEGER NOT NULL CHECK (
        context_limit_tokens > 0
    ),
    reserved_output_tokens INTEGER NOT NULL CHECK (
        reserved_output_tokens >= 0
    ),
    estimated_input_tokens INTEGER NOT NULL CHECK (
        estimated_input_tokens >= 0
    ),
    final_input_tokens INTEGER NOT NULL CHECK (
        final_input_tokens >= 0
    ),
    message_count INTEGER NOT NULL CHECK (message_count >= 0),
    cacheable_prefix_tokens INTEGER NOT NULL DEFAULT 0 CHECK (
        cacheable_prefix_tokens >= 0
    ),
    status TEXT NOT NULL CHECK (
        status IN ('resolved', 'rejected')
    ),
    canonical_plan_json TEXT NOT NULL CHECK (
        json_valid(canonical_plan_json)
        AND json_type(canonical_plan_json) = 'object'
        AND length(CAST(canonical_plan_json AS BLOB)) <= 16777216
    ),
    sealed_at TEXT NOT NULL CHECK (length(trim(sealed_at)) > 0),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    UNIQUE (conversation_id, id),
    FOREIGN KEY (conversation_id, branch_id)
        REFERENCES conversation_branches(conversation_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    FOREIGN KEY (conversation_id, head_message_id)
        REFERENCES messages(conversation_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    FOREIGN KEY (conversation_id, latest_user_message_id)
        REFERENCES messages(conversation_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    FOREIGN KEY (prompt_preset_id, prompt_preset_revision_id)
        REFERENCES prompt_preset_revisions(
            prompt_preset_id,
            revision_id
        )
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    CHECK (
        (generation_preset_id IS NULL AND model_route_id IS NULL)
        OR (
            generation_preset_id IS NOT NULL
            AND model_route_id IS NOT NULL
        )
    ),
    CHECK (
        task_profile_revision_id IS NULL
        OR model_route_id IS NOT NULL
    ),
    CHECK (status = 'rejected' OR latest_user_included = 1),
    CHECK (
        status = 'rejected'
        OR final_input_tokens + reserved_output_tokens
           <= context_limit_tokens
    )
);

CREATE INDEX generation_prompt_plans_conversation
    ON generation_prompt_plans(
        conversation_id,
        branch_id,
        created_at DESC,
        id
    );
CREATE INDEX generation_prompt_plans_preset_revision
    ON generation_prompt_plans(
        prompt_preset_revision_id,
        created_at DESC,
        id
    );

CREATE TRIGGER generation_prompt_plans_route_guard
BEFORE INSERT ON generation_prompt_plans
WHEN
    (
        (NEW.generation_preset_id IS NULL) <> (NEW.model_route_id IS NULL)
    )
    OR (
        NEW.generation_preset_id IS NOT NULL
        AND NOT EXISTS (
            SELECT 1
            FROM generation_presets
            WHERE id = NEW.generation_preset_id
              AND model_route_id = NEW.model_route_id
        )
    )
    OR (
        NEW.task_profile_revision_id IS NOT NULL
        AND NOT EXISTS (
            SELECT 1
            FROM task_profile_revisions
            WHERE revision_id = NEW.task_profile_revision_id
              AND model_route_id = NEW.model_route_id
              AND generation_preset_id = NEW.generation_preset_id
        )
    )
BEGIN
    SELECT RAISE(ABORT, 'resolved prompt plan route is inconsistent');
END;

CREATE TRIGGER generation_prompt_plans_no_update
BEFORE UPDATE ON generation_prompt_plans
BEGIN
    SELECT RAISE(ABORT, 'resolved prompt plans are immutable');
END;

CREATE TRIGGER generation_prompt_plans_no_delete
BEFORE DELETE ON generation_prompt_plans
BEGIN
    SELECT RAISE(ABORT, 'resolved prompt plans are immutable');
END;

CREATE TABLE generation_prompt_plan_blocks (
    plan_id TEXT NOT NULL
        REFERENCES generation_prompt_plans(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    source_owner_revision_id TEXT,
    source_block_id TEXT,
    kind TEXT NOT NULL CHECK (
        kind IN (
            'static_instruction',
            'character_identity',
            'character_description',
            'character_personality',
            'scenario',
            'user_persona',
            'dialogue_examples',
            'world_knowledge',
            'retrieved_memory',
            'conversation_summary',
            'history_slice',
            'latest_user_turn',
            'author_note',
            'post_history_instruction',
            'assistant_prefill',
            'group_context'
        )
    ),
    placement_zone TEXT NOT NULL,
    requested_role TEXT NOT NULL CHECK (
        requested_role IN (
            'system',
            'developer',
            'user',
            'assistant',
            'provider_default'
        )
    ),
    disposition TEXT NOT NULL CHECK (
        disposition IN (
            'included',
            'dropped',
            'trimmed_head',
            'trimmed_tail',
            'summarized'
        )
    ),
    reduction_reason_json TEXT CHECK (
        reduction_reason_json IS NULL
        OR (
            json_valid(reduction_reason_json)
            AND json_type(reduction_reason_json) = 'object'
            AND length(CAST(reduction_reason_json AS BLOB)) <= 262144
        )
    ),
    content TEXT NOT NULL CHECK (
        length(CAST(content AS BLOB)) <= 8388608
    ),
    content_sha256 TEXT NOT NULL CHECK (
        length(content_sha256) = 64
        AND content_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    estimated_tokens INTEGER NOT NULL CHECK (estimated_tokens >= 0),
    final_tokens INTEGER NOT NULL CHECK (final_tokens >= 0),
    provenance_json TEXT NOT NULL CHECK (
        json_valid(provenance_json)
        AND json_type(provenance_json) = 'object'
        AND length(CAST(provenance_json AS BLOB)) <= 262144
    ),
    payload_json TEXT NOT NULL CHECK (
        json_valid(payload_json)
        AND json_type(payload_json) = 'object'
        AND length(CAST(payload_json AS BLOB)) <= 1048576
    ),
    PRIMARY KEY (plan_id, ordinal),
    FOREIGN KEY (source_owner_revision_id, source_block_id)
        REFERENCES prompt_blocks(owner_revision_id, block_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    CHECK (
        (source_owner_revision_id IS NULL AND source_block_id IS NULL)
        OR (
            source_owner_revision_id IS NOT NULL
            AND source_block_id IS NOT NULL
        )
    )
);

CREATE TABLE generation_prompt_plan_messages (
    plan_id TEXT NOT NULL
        REFERENCES generation_prompt_plans(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    role TEXT NOT NULL CHECK (
        role IN (
            'system',
            'developer',
            'user',
            'assistant',
            'provider_default'
        )
    ),
    content TEXT NOT NULL CHECK (
        length(CAST(content AS BLOB)) <= 8388608
    ),
    content_sha256 TEXT NOT NULL CHECK (
        length(content_sha256) = 64
        AND content_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    source_block_ordinals_json TEXT NOT NULL CHECK (
        json_valid(source_block_ordinals_json)
        AND json_type(source_block_ordinals_json) = 'array'
        AND length(CAST(source_block_ordinals_json AS BLOB)) <= 65536
    ),
    source_message_id TEXT,
    estimated_tokens INTEGER NOT NULL CHECK (estimated_tokens >= 0),
    PRIMARY KEY (plan_id, ordinal)
);

CREATE TABLE generation_prompt_plan_directives (
    plan_id TEXT NOT NULL
        REFERENCES generation_prompt_plans(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    directive_kind TEXT NOT NULL CHECK (
        directive_kind IN (
            'cache',
            'reasoning',
            'tool',
            'output_contract',
            'privacy'
        )
    ),
    source_owner_revision_id TEXT,
    source_boundary_id TEXT,
    directive_json TEXT NOT NULL CHECK (
        json_valid(directive_json)
        AND json_type(directive_json) = 'object'
        AND length(CAST(directive_json AS BLOB)) <= 1048576
    ),
    disposition TEXT NOT NULL CHECK (
        disposition IN ('applied', 'ignored', 'rejected')
    ),
    provider_mapping_json TEXT CHECK (
        provider_mapping_json IS NULL
        OR (
            json_valid(provider_mapping_json)
            AND json_type(provider_mapping_json) = 'object'
            AND length(CAST(provider_mapping_json AS BLOB)) <= 1048576
        )
    ),
    warning_code TEXT,
    PRIMARY KEY (plan_id, ordinal),
    FOREIGN KEY (source_owner_revision_id, source_boundary_id)
        REFERENCES prompt_cache_boundaries(owner_revision_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    CHECK (
        (source_owner_revision_id IS NULL AND source_boundary_id IS NULL)
        OR (
            source_owner_revision_id IS NOT NULL
            AND source_boundary_id IS NOT NULL
        )
    )
);

CREATE TABLE generation_prompt_plan_warnings (
    plan_id TEXT NOT NULL
        REFERENCES generation_prompt_plans(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    code TEXT NOT NULL CHECK (length(trim(code)) > 0),
    severity TEXT NOT NULL CHECK (
        severity IN ('info', 'warning', 'error')
    ),
    message_key TEXT NOT NULL CHECK (length(trim(message_key)) > 0),
    details_json TEXT NOT NULL CHECK (
        json_valid(details_json)
        AND json_type(details_json) = 'object'
        AND length(CAST(details_json AS BLOB)) <= 262144
    ),
    PRIMARY KEY (plan_id, ordinal)
);

CREATE TABLE generation_prompt_plan_seals (
    plan_id TEXT PRIMARY KEY NOT NULL
        REFERENCES generation_prompt_plans(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    plan_sha256 TEXT NOT NULL CHECK (
        length(plan_sha256) = 64
        AND plan_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    sealed_at TEXT NOT NULL CHECK (length(trim(sealed_at)) > 0)
);

CREATE TRIGGER generation_prompt_plan_seals_hash_guard
BEFORE INSERT ON generation_prompt_plan_seals
WHEN NOT EXISTS (
    SELECT 1
    FROM generation_prompt_plans
    WHERE id = NEW.plan_id
      AND plan_sha256 = NEW.plan_sha256
)
BEGIN
    SELECT RAISE(ABORT, 'resolved prompt plan seal hash is inconsistent');
END;

CREATE TRIGGER generation_prompt_plan_seals_no_update
BEFORE UPDATE ON generation_prompt_plan_seals
BEGIN
    SELECT RAISE(ABORT, 'resolved prompt plan seals are immutable');
END;

CREATE TRIGGER generation_prompt_plan_seals_no_delete
BEFORE DELETE ON generation_prompt_plan_seals
BEGIN
    SELECT RAISE(ABORT, 'resolved prompt plan seals are immutable');
END;

CREATE TABLE provider_request_snapshots (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(trim(id)) > 0),
    plan_id TEXT NOT NULL UNIQUE
        REFERENCES generation_prompt_plans(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    api_family TEXT NOT NULL CHECK (
        api_family IN (
            'openai_responses',
            'openai_chat_completions',
            'anthropic_messages',
            'gemini_generate_content',
            'ollama_native'
        )
    ),
    request_schema_version INTEGER NOT NULL CHECK (
        request_schema_version >= 1
    ),
    request_json TEXT NOT NULL CHECK (
        json_valid(request_json)
        AND json_type(request_json) = 'object'
        AND length(CAST(request_json AS BLOB)) <= 16777216
    ),
    request_sha256 TEXT NOT NULL CHECK (
        length(request_sha256) = 64
        AND request_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    mapping_diagnostics_json TEXT NOT NULL CHECK (
        json_valid(mapping_diagnostics_json)
        AND json_type(mapping_diagnostics_json) = 'object'
        AND length(CAST(mapping_diagnostics_json AS BLOB)) <= 1048576
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0)
);

CREATE TRIGGER provider_request_snapshots_seal_guard
BEFORE INSERT ON provider_request_snapshots
WHEN NOT EXISTS (
    SELECT 1
    FROM generation_prompt_plan_seals
    WHERE plan_id = NEW.plan_id
)
BEGIN
    SELECT RAISE(ABORT, 'provider request snapshot requires a sealed plan');
END;

CREATE TRIGGER provider_request_snapshots_no_update
BEFORE UPDATE ON provider_request_snapshots
BEGIN
    SELECT RAISE(ABORT, 'provider request snapshots are immutable');
END;

CREATE TRIGGER provider_request_snapshots_no_delete
BEFORE DELETE ON provider_request_snapshots
BEGIN
    SELECT RAISE(ABORT, 'provider request snapshots are immutable');
END;

ALTER TABLE generations
    ADD COLUMN resolved_prompt_plan_id TEXT
        REFERENCES generation_prompt_plans(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT;

ALTER TABLE generations
    ADD COLUMN prompt_plan_sha256 TEXT CHECK (
        prompt_plan_sha256 IS NULL
        OR (
            length(prompt_plan_sha256) = 64
            AND prompt_plan_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    );

ALTER TABLE generations
    ADD COLUMN provider_request_snapshot_id TEXT
        REFERENCES provider_request_snapshots(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT;

CREATE UNIQUE INDEX generations_resolved_prompt_plan
    ON generations(resolved_prompt_plan_id)
    WHERE resolved_prompt_plan_id IS NOT NULL;
CREATE INDEX generations_prompt_plan_hash
    ON generations(prompt_plan_sha256, started_at, id)
    WHERE prompt_plan_sha256 IS NOT NULL;

CREATE TRIGGER generations_prompt_plan_insert_guard
BEFORE INSERT ON generations
WHEN
    (NEW.resolved_prompt_plan_id IS NULL)
        <> (NEW.prompt_plan_sha256 IS NULL)
    OR (NEW.resolved_prompt_plan_id IS NULL)
        <> (NEW.provider_request_snapshot_id IS NULL)
    OR (
        NEW.resolved_prompt_plan_id IS NOT NULL
        AND NOT EXISTS (
            SELECT 1
            FROM generation_prompt_plans AS plan
            JOIN generation_prompt_plan_seals AS seal
              ON seal.plan_id = plan.id
             AND seal.plan_sha256 = plan.plan_sha256
            JOIN provider_request_snapshots AS snapshot
              ON snapshot.id = NEW.provider_request_snapshot_id
             AND snapshot.plan_id = plan.id
            WHERE plan.id = NEW.resolved_prompt_plan_id
              AND plan.plan_sha256 = NEW.prompt_plan_sha256
              AND plan.conversation_id = NEW.conversation_id
              AND plan.branch_id = NEW.branch_id
              AND plan.model_route_id IS NEW.model_route_id
              AND plan.generation_preset_id IS NEW.generation_preset_id
        )
    )
BEGIN
    SELECT RAISE(ABORT, 'generation prompt plan provenance is inconsistent');
END;

CREATE TRIGGER generations_prompt_plan_update_guard
BEFORE UPDATE OF
    resolved_prompt_plan_id,
    prompt_plan_sha256,
    provider_request_snapshot_id,
    conversation_id,
    branch_id,
    model_route_id,
    generation_preset_id
ON generations
WHEN
    (
        OLD.resolved_prompt_plan_id IS NOT NULL
        AND (
            NEW.resolved_prompt_plan_id IS NOT OLD.resolved_prompt_plan_id
            OR NEW.prompt_plan_sha256 IS NOT OLD.prompt_plan_sha256
            OR NEW.provider_request_snapshot_id
               IS NOT OLD.provider_request_snapshot_id
        )
    )
    OR (NEW.resolved_prompt_plan_id IS NULL)
        <> (NEW.prompt_plan_sha256 IS NULL)
    OR (NEW.resolved_prompt_plan_id IS NULL)
        <> (NEW.provider_request_snapshot_id IS NULL)
    OR (
        NEW.resolved_prompt_plan_id IS NOT NULL
        AND NOT EXISTS (
            SELECT 1
            FROM generation_prompt_plans AS plan
            JOIN generation_prompt_plan_seals AS seal
              ON seal.plan_id = plan.id
             AND seal.plan_sha256 = plan.plan_sha256
            JOIN provider_request_snapshots AS snapshot
              ON snapshot.id = NEW.provider_request_snapshot_id
             AND snapshot.plan_id = plan.id
            WHERE plan.id = NEW.resolved_prompt_plan_id
              AND plan.plan_sha256 = NEW.prompt_plan_sha256
              AND plan.conversation_id = NEW.conversation_id
              AND plan.branch_id = NEW.branch_id
              AND plan.model_route_id IS NEW.model_route_id
              AND plan.generation_preset_id IS NEW.generation_preset_id
        )
    )
BEGIN
    SELECT RAISE(ABORT, 'generation prompt plan provenance is inconsistent');
END;

-- Revision-owned normalized payloads form part of the revision hash and may
-- not be edited in place.
CREATE TRIGGER prompt_variables_no_update
BEFORE UPDATE ON prompt_variables
BEGIN
    SELECT RAISE(ABORT, 'prompt revision variables are immutable');
END;
CREATE TRIGGER prompt_variables_no_delete
BEFORE DELETE ON prompt_variables
BEGIN
    SELECT RAISE(ABORT, 'prompt revision variables are immutable');
END;
CREATE TRIGGER prompt_controls_no_update
BEFORE UPDATE ON prompt_controls
BEGIN
    SELECT RAISE(ABORT, 'prompt revision controls are immutable');
END;
CREATE TRIGGER prompt_controls_no_delete
BEFORE DELETE ON prompt_controls
BEGIN
    SELECT RAISE(ABORT, 'prompt revision controls are immutable');
END;
CREATE TRIGGER prompt_blocks_no_update
BEFORE UPDATE ON prompt_blocks
BEGIN
    SELECT RAISE(ABORT, 'prompt revision blocks are immutable');
END;
CREATE TRIGGER prompt_blocks_no_delete
BEFORE DELETE ON prompt_blocks
BEGIN
    SELECT RAISE(ABORT, 'prompt revision blocks are immutable');
END;
CREATE TRIGGER prompt_cache_boundaries_no_update
BEFORE UPDATE ON prompt_cache_boundaries
BEGIN
    SELECT RAISE(ABORT, 'prompt cache boundaries are immutable');
END;
CREATE TRIGGER prompt_cache_boundaries_no_delete
BEFORE DELETE ON prompt_cache_boundaries
BEGIN
    SELECT RAISE(ABORT, 'prompt cache boundaries are immutable');
END;
CREATE TRIGGER generation_prompt_plan_blocks_no_update
BEFORE UPDATE ON generation_prompt_plan_blocks
BEGIN
    SELECT RAISE(ABORT, 'resolved prompt plan blocks are immutable');
END;
CREATE TRIGGER generation_prompt_plan_blocks_seal_guard
BEFORE INSERT ON generation_prompt_plan_blocks
WHEN EXISTS (
    SELECT 1 FROM generation_prompt_plan_seals
    WHERE plan_id = NEW.plan_id
)
BEGIN
    SELECT RAISE(ABORT, 'resolved prompt plan is already sealed');
END;
CREATE TRIGGER generation_prompt_plan_blocks_no_delete
BEFORE DELETE ON generation_prompt_plan_blocks
BEGIN
    SELECT RAISE(ABORT, 'resolved prompt plan blocks are immutable');
END;
CREATE TRIGGER generation_prompt_plan_messages_no_update
BEFORE UPDATE ON generation_prompt_plan_messages
BEGIN
    SELECT RAISE(ABORT, 'resolved prompt plan messages are immutable');
END;
CREATE TRIGGER generation_prompt_plan_messages_seal_guard
BEFORE INSERT ON generation_prompt_plan_messages
WHEN EXISTS (
    SELECT 1 FROM generation_prompt_plan_seals
    WHERE plan_id = NEW.plan_id
)
BEGIN
    SELECT RAISE(ABORT, 'resolved prompt plan is already sealed');
END;
CREATE TRIGGER generation_prompt_plan_messages_no_delete
BEFORE DELETE ON generation_prompt_plan_messages
BEGIN
    SELECT RAISE(ABORT, 'resolved prompt plan messages are immutable');
END;
CREATE TRIGGER generation_prompt_plan_directives_no_update
BEFORE UPDATE ON generation_prompt_plan_directives
BEGIN
    SELECT RAISE(ABORT, 'resolved prompt plan directives are immutable');
END;
CREATE TRIGGER generation_prompt_plan_directives_seal_guard
BEFORE INSERT ON generation_prompt_plan_directives
WHEN EXISTS (
    SELECT 1 FROM generation_prompt_plan_seals
    WHERE plan_id = NEW.plan_id
)
BEGIN
    SELECT RAISE(ABORT, 'resolved prompt plan is already sealed');
END;
CREATE TRIGGER generation_prompt_plan_directives_no_delete
BEFORE DELETE ON generation_prompt_plan_directives
BEGIN
    SELECT RAISE(ABORT, 'resolved prompt plan directives are immutable');
END;
CREATE TRIGGER generation_prompt_plan_warnings_no_update
BEFORE UPDATE ON generation_prompt_plan_warnings
BEGIN
    SELECT RAISE(ABORT, 'resolved prompt plan warnings are immutable');
END;
CREATE TRIGGER generation_prompt_plan_warnings_seal_guard
BEFORE INSERT ON generation_prompt_plan_warnings
WHEN EXISTS (
    SELECT 1 FROM generation_prompt_plan_seals
    WHERE plan_id = NEW.plan_id
)
BEGIN
    SELECT RAISE(ABORT, 'resolved prompt plan is already sealed');
END;
CREATE TRIGGER generation_prompt_plan_warnings_no_delete
BEFORE DELETE ON generation_prompt_plan_warnings
BEGIN
    SELECT RAISE(ABORT, 'resolved prompt plan warnings are immutable');
END;
