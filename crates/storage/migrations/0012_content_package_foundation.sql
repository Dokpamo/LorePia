PRAGMA foreign_keys = ON;

-- Revision registry shared by every user-authored/imported orchestration
-- object. Logical identity is stable; immutable revisions hold canonical
-- documents, and content_object_state is the only mutable active pointer.
CREATE TABLE content_objects (
    id TEXT PRIMARY KEY NOT NULL CHECK (
        length(id) BETWEEN 1 AND 256
        AND id = trim(id)
        AND instr(id, char(0)) = 0
    ),
    object_kind TEXT NOT NULL CHECK (
        object_kind IN (
            'prompt_preset',
            'task_profile',
            'knowledge_book',
            'memory_profile',
            'memory_summary_schema',
            'transform_set',
            'interaction_rule_set',
            'content_module',
            'character_content',
            'persona'
        )
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    deleted_at TEXT CHECK (
        deleted_at IS NULL OR length(trim(deleted_at)) > 0
    ),
    UNIQUE (id, object_kind)
);

CREATE INDEX content_objects_kind_active
    ON content_objects(object_kind, id)
    WHERE deleted_at IS NULL;

CREATE TABLE content_revisions (
    id TEXT PRIMARY KEY NOT NULL CHECK (
        length(id) BETWEEN 1 AND 256
        AND id = trim(id)
        AND instr(id, char(0)) = 0
    ),
    object_id TEXT NOT NULL,
    object_kind TEXT NOT NULL,
    revision_no INTEGER NOT NULL CHECK (revision_no >= 1),
    parent_revision_id TEXT,
    schema_version INTEGER NOT NULL CHECK (schema_version >= 1),
    document_json TEXT NOT NULL CHECK (
        CASE
            WHEN json_valid(document_json)
            THEN json_type(document_json) = 'object'
                 AND length(CAST(document_json AS BLOB)) <= 16777216
            ELSE 0
        END
    ),
    document_sha256 TEXT NOT NULL CHECK (
        length(document_sha256) = 64
        AND document_sha256 NOT GLOB '*[^0-9a-f]*'
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
    provenance_json TEXT NOT NULL CHECK (
        CASE
            WHEN json_valid(provenance_json)
            THEN json_type(provenance_json) = 'object'
                 AND length(CAST(provenance_json AS BLOB)) <= 65536
            ELSE 0
        END
    ),
    local_override_of_revision_id TEXT,
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    UNIQUE (object_id, revision_no),
    UNIQUE (object_id, id),
    UNIQUE (id, object_kind),
    FOREIGN KEY (object_id, object_kind)
        REFERENCES content_objects(id, object_kind)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    FOREIGN KEY (object_id, parent_revision_id)
        REFERENCES content_revisions(object_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    FOREIGN KEY (local_override_of_revision_id, object_kind)
        REFERENCES content_revisions(id, object_kind)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    CHECK (
        (revision_no = 1 AND parent_revision_id IS NULL)
        OR (revision_no > 1 AND parent_revision_id IS NOT NULL)
    ),
    CHECK (
        local_override_of_revision_id IS NULL
        OR source_kind = 'local_override'
    )
);

CREATE INDEX content_revisions_object_history
    ON content_revisions(object_id, revision_no DESC, id);
CREATE INDEX content_revisions_source_hash
    ON content_revisions(source_hash, object_id)
    WHERE source_hash IS NOT NULL;

CREATE TRIGGER content_revisions_append_guard
BEFORE INSERT ON content_revisions
WHEN
    NEW.revision_no != (
        SELECT COALESCE(MAX(revision_no), 0) + 1
        FROM content_revisions
        WHERE object_id = NEW.object_id
    )
    OR (
        NEW.parent_revision_id IS NOT NULL
        AND NOT EXISTS (
            SELECT 1
            FROM content_revisions
            WHERE object_id = NEW.object_id
              AND id = NEW.parent_revision_id
              AND revision_no = NEW.revision_no - 1
        )
    )
BEGIN
    SELECT RAISE(ABORT, 'content revision is not the next revision');
END;

CREATE TRIGGER content_revisions_no_update
BEFORE UPDATE ON content_revisions
BEGIN
    SELECT RAISE(ABORT, 'content revisions are immutable');
END;

CREATE TRIGGER content_revisions_no_delete
BEFORE DELETE ON content_revisions
BEGIN
    SELECT RAISE(ABORT, 'content revisions are immutable');
END;

CREATE TABLE content_object_state (
    object_id TEXT PRIMARY KEY NOT NULL
        REFERENCES content_objects(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    active_revision_id TEXT NOT NULL,
    state_version INTEGER NOT NULL CHECK (state_version >= 1),
    updated_at TEXT NOT NULL CHECK (length(trim(updated_at)) > 0),
    FOREIGN KEY (object_id, active_revision_id)
        REFERENCES content_revisions(object_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

CREATE INDEX content_object_state_active_revision
    ON content_object_state(active_revision_id, object_id);

CREATE TRIGGER content_object_state_version_guard
BEFORE UPDATE ON content_object_state
WHEN
    NEW.object_id != OLD.object_id
    OR NEW.state_version != OLD.state_version + 1
    OR NEW.active_revision_id = OLD.active_revision_id
BEGIN
    SELECT RAISE(ABORT, 'content object state update is not a revision switch');
END;

CREATE TABLE content_revision_events (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(trim(id)) > 0),
    object_id TEXT NOT NULL
        REFERENCES content_objects(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    event_kind TEXT NOT NULL CHECK (
        event_kind IN (
            'create',
            'update',
            'import',
            'override',
            'activate',
            'rollback',
            'soft_delete',
            'restore'
        )
    ),
    from_revision_id TEXT,
    to_revision_id TEXT,
    diff_json TEXT CHECK (
        diff_json IS NULL
        OR (
            json_valid(diff_json)
            AND json_type(diff_json) = 'object'
            AND length(CAST(diff_json AS BLOB)) <= 8388608
        )
    ),
    diff_sha256 TEXT CHECK (
        diff_sha256 IS NULL
        OR (
            length(diff_sha256) = 64
            AND diff_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    ),
    plan_sha256 TEXT CHECK (
        plan_sha256 IS NULL
        OR (
            length(plan_sha256) = 64
            AND plan_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    ),
    idempotency_key TEXT NOT NULL UNIQUE CHECK (
        length(trim(idempotency_key)) > 0
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    FOREIGN KEY (object_id, from_revision_id)
        REFERENCES content_revisions(object_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    FOREIGN KEY (object_id, to_revision_id)
        REFERENCES content_revisions(object_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    CHECK (
        (diff_json IS NULL AND diff_sha256 IS NULL)
        OR (diff_json IS NOT NULL AND diff_sha256 IS NOT NULL)
    ),
    CHECK (from_revision_id IS NOT NULL OR to_revision_id IS NOT NULL)
);

CREATE INDEX content_revision_events_object_created
    ON content_revision_events(object_id, created_at DESC, id);

CREATE TRIGGER content_revision_events_no_update
BEFORE UPDATE ON content_revision_events
BEGIN
    SELECT RAISE(ABORT, 'content revision events are immutable');
END;

CREATE TRIGGER content_revision_events_no_delete
BEFORE DELETE ON content_revision_events
BEGIN
    SELECT RAISE(ABORT, 'content revision events are immutable');
END;

CREATE TABLE content_rollback_plans (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(trim(id)) > 0),
    object_id TEXT NOT NULL
        REFERENCES content_objects(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    expected_active_revision_id TEXT NOT NULL,
    target_revision_id TEXT NOT NULL,
    diff_json TEXT NOT NULL CHECK (
        json_valid(diff_json)
        AND json_type(diff_json) = 'object'
        AND length(CAST(diff_json AS BLOB)) <= 8388608
    ),
    diff_sha256 TEXT NOT NULL CHECK (
        length(diff_sha256) = 64
        AND diff_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    plan_sha256 TEXT NOT NULL UNIQUE CHECK (
        length(plan_sha256) = 64
        AND plan_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    state TEXT NOT NULL CHECK (
        state IN ('prepared', 'applied', 'discarded', 'stale')
    ),
    prepared_at TEXT NOT NULL CHECK (length(trim(prepared_at)) > 0),
    approved_at TEXT,
    applied_at TEXT,
    FOREIGN KEY (object_id, expected_active_revision_id)
        REFERENCES content_revisions(object_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    FOREIGN KEY (object_id, target_revision_id)
        REFERENCES content_revisions(object_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    CHECK (expected_active_revision_id <> target_revision_id),
    CHECK (
        (state = 'prepared' AND approved_at IS NULL AND applied_at IS NULL)
        OR (
            state = 'applied'
            AND approved_at IS NOT NULL
            AND applied_at IS NOT NULL
        )
        OR (
            state IN ('discarded', 'stale')
            AND applied_at IS NULL
        )
    )
);

CREATE UNIQUE INDEX content_rollback_one_prepared_per_object
    ON content_rollback_plans(object_id)
    WHERE state = 'prepared';
CREATE INDEX content_rollback_plans_object_prepared
    ON content_rollback_plans(object_id, prepared_at DESC, id);

CREATE TRIGGER content_rollback_plans_initial_state_guard
BEFORE INSERT ON content_rollback_plans
WHEN NEW.state != 'prepared'
BEGIN
    SELECT RAISE(ABORT, 'content rollback plan must begin prepared');
END;

CREATE TRIGGER content_rollback_plans_transition_guard
BEFORE UPDATE ON content_rollback_plans
WHEN
    NEW.id != OLD.id
    OR NEW.object_id != OLD.object_id
    OR NEW.expected_active_revision_id != OLD.expected_active_revision_id
    OR NEW.target_revision_id != OLD.target_revision_id
    OR NEW.diff_json != OLD.diff_json
    OR NEW.diff_sha256 != OLD.diff_sha256
    OR NEW.plan_sha256 != OLD.plan_sha256
    OR NEW.prepared_at != OLD.prepared_at
    OR OLD.state != 'prepared'
    OR NEW.state NOT IN ('applied', 'discarded', 'stale')
    OR (
        NEW.state = 'applied'
        AND NOT EXISTS (
            SELECT 1
            FROM content_revision_events
            WHERE object_id = NEW.object_id
              AND event_kind = 'rollback'
              AND from_revision_id = NEW.expected_active_revision_id
              AND to_revision_id = NEW.target_revision_id
              AND plan_sha256 = NEW.plan_sha256
        )
    )
BEGIN
    SELECT RAISE(ABORT, 'content rollback plan transition is invalid');
END;

CREATE TRIGGER content_rollback_plans_no_delete
BEFORE DELETE ON content_rollback_plans
BEGIN
    SELECT RAISE(ABORT, 'content rollback plans are durable');
END;

-- Character-card fields are an optional sidecar. Legacy character rows remain
-- valid and keep their original identifiers, schema, and CAS source hash.
CREATE TABLE character_content (
    object_id TEXT PRIMARY KEY NOT NULL
        REFERENCES content_objects(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    character_id TEXT NOT NULL UNIQUE
        REFERENCES characters(id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE
);

CREATE TRIGGER character_content_kind_guard
BEFORE INSERT ON character_content
WHEN NOT EXISTS (
    SELECT 1
    FROM content_objects
    WHERE id = NEW.object_id
      AND object_kind = 'character_content'
)
BEGIN
    SELECT RAISE(ABORT, 'character content object kind is invalid');
END;

CREATE TABLE character_content_revisions (
    revision_id TEXT PRIMARY KEY NOT NULL
        REFERENCES content_revisions(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    object_id TEXT NOT NULL
        REFERENCES character_content(object_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    personality TEXT NOT NULL DEFAULT '',
    scenario TEXT NOT NULL DEFAULT '',
    first_message TEXT NOT NULL DEFAULT '',
    system_instruction TEXT NOT NULL DEFAULT '',
    post_history_instruction TEXT NOT NULL DEFAULT '',
    creator_notes TEXT NOT NULL DEFAULT '',
    unknown_extensions_json TEXT NOT NULL CHECK (
        json_valid(unknown_extensions_json)
        AND json_type(unknown_extensions_json) = 'object'
        AND length(CAST(unknown_extensions_json AS BLOB)) <= 4194304
    ),
    metadata_json TEXT NOT NULL CHECK (
        json_valid(metadata_json)
        AND json_type(metadata_json) = 'object'
        AND length(CAST(metadata_json AS BLOB)) <= 262144
    ),
    payload_json TEXT NOT NULL CHECK (
        json_valid(payload_json)
        AND json_type(payload_json) = 'object'
        AND length(CAST(payload_json AS BLOB)) <= 8388608
    ),
    UNIQUE (object_id, revision_id),
    FOREIGN KEY (object_id, revision_id)
        REFERENCES content_revisions(object_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

CREATE TABLE character_greetings (
    character_content_revision_id TEXT NOT NULL
        REFERENCES character_content_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    greeting_id TEXT NOT NULL CHECK (length(trim(greeting_id)) > 0),
    kind TEXT NOT NULL CHECK (kind IN ('default', 'alternate')),
    content TEXT NOT NULL CHECK (length(content) > 0),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    payload_json TEXT NOT NULL CHECK (
        json_valid(payload_json)
        AND json_type(payload_json) = 'object'
        AND length(CAST(payload_json AS BLOB)) <= 1048576
    ),
    PRIMARY KEY (character_content_revision_id, greeting_id),
    UNIQUE (character_content_revision_id, ordinal)
);

CREATE TABLE character_dialogue_examples (
    character_content_revision_id TEXT NOT NULL
        REFERENCES character_content_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    example_id TEXT NOT NULL CHECK (length(trim(example_id)) > 0),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    name TEXT NOT NULL DEFAULT '',
    payload_json TEXT NOT NULL CHECK (
        json_valid(payload_json)
        AND json_type(payload_json) = 'object'
        AND length(CAST(payload_json AS BLOB)) <= 2097152
    ),
    PRIMARY KEY (character_content_revision_id, example_id),
    UNIQUE (character_content_revision_id, ordinal)
);

CREATE TABLE character_dialogue_example_messages (
    character_content_revision_id TEXT NOT NULL,
    example_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    role TEXT NOT NULL CHECK (role IN ('system', 'user', 'assistant')),
    content TEXT NOT NULL CHECK (length(content) > 0),
    PRIMARY KEY (
        character_content_revision_id,
        example_id,
        ordinal
    ),
    FOREIGN KEY (character_content_revision_id, example_id)
        REFERENCES character_dialogue_examples(
            character_content_revision_id,
            example_id
        )
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

CREATE TABLE personas (
    object_id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    schema_version INTEGER NOT NULL DEFAULT 1 CHECK (schema_version >= 1),
    revision INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
    description TEXT NOT NULL DEFAULT '',
    payload_json TEXT NOT NULL CHECK (
        json_valid(payload_json)
        AND json_type(payload_json) = 'object'
        AND length(CAST(payload_json AS BLOB)) <= 1048576
    ),
    created_at TEXT NOT NULL DEFAULT '1970-01-01T00:00:00Z' CHECK (
        length(trim(created_at)) > 0
    ),
    updated_at TEXT NOT NULL DEFAULT '1970-01-01T00:00:00Z' CHECK (
        length(trim(updated_at)) > 0
    ),
    deleted_at TEXT CHECK (
        deleted_at IS NULL OR length(trim(deleted_at)) > 0
    ),
    UNIQUE (object_id, revision),
    FOREIGN KEY (object_id)
        REFERENCES content_objects(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

CREATE TRIGGER personas_kind_guard
BEFORE INSERT ON personas
WHEN NOT EXISTS (
    SELECT 1 FROM content_objects
    WHERE id = NEW.object_id AND object_kind = 'persona'
)
BEGIN
    SELECT RAISE(ABORT, 'persona object kind is invalid');
END;

CREATE TABLE persona_revisions (
    revision_id TEXT PRIMARY KEY NOT NULL,
    persona_id TEXT NOT NULL
        REFERENCES personas(object_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    revision_no INTEGER NOT NULL CHECK (revision_no >= 1),
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    description TEXT NOT NULL DEFAULT '',
    document_json TEXT NOT NULL CHECK (
        json_valid(document_json)
        AND json_type(document_json) = 'object'
        AND length(CAST(document_json AS BLOB)) <= 1048576
    ),
    UNIQUE (persona_id, revision_id),
    UNIQUE (persona_id, revision_no),
    FOREIGN KEY (persona_id, revision_id)
        REFERENCES content_revisions(object_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

-- CAS remains keyed by assets.sha256. Descriptors add semantic identity and
-- reviewed metadata without duplicating or weakening byte ownership.
CREATE TABLE asset_descriptors (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(trim(id)) > 0),
    asset_hash TEXT NOT NULL
        REFERENCES assets(sha256)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    role TEXT NOT NULL CHECK (
        role IN (
            'avatar',
            'icon',
            'background',
            'user_icon',
            'emotion',
            'expression',
            'illustration',
            'audio',
            'voice',
            'video',
            'status_panel',
            'attachment',
            'other'
        )
    ),
    media_type TEXT NOT NULL CHECK (length(trim(media_type)) > 0),
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
    width INTEGER CHECK (width IS NULL OR width > 0),
    height INTEGER CHECK (height IS NULL OR height > 0),
    duration_ms INTEGER CHECK (duration_ms IS NULL OR duration_ms >= 0),
    risk_class TEXT NOT NULL CHECK (
        risk_class IN ('normal', 'high_risk')
    ),
    source_revision_id TEXT
        REFERENCES content_revisions(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    source_kind TEXT NOT NULL CHECK (
        source_kind IN (
            'character_card',
            'charx_package',
            'lorepia_package',
            'content_module',
            'user_selected',
            'generated',
            'unknown'
        )
    ),
    source_hash TEXT CHECK (
        source_hash IS NULL
        OR (
            length(source_hash) = 64
            AND source_hash NOT GLOB '*[^0-9a-f]*'
        )
    ),
    logical_path TEXT CHECK (
        logical_path IS NULL
        OR (
            length(trim(logical_path)) > 0
            AND instr(logical_path, char(0)) = 0
            AND substr(logical_path, 1, 1) <> '/'
            AND instr('/' || logical_path || '/', '/../') = 0
        )
    ),
    payload_json TEXT NOT NULL CHECK (
        json_valid(payload_json)
        AND json_type(payload_json) = 'object'
        AND length(CAST(payload_json AS BLOB)) <= 262144
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0)
);

CREATE INDEX asset_descriptors_hash_role
    ON asset_descriptors(asset_hash, role, id);

CREATE TRIGGER asset_descriptors_cas_guard
BEFORE INSERT ON asset_descriptors
WHEN NOT EXISTS (
    SELECT 1
    FROM assets
    WHERE sha256 = NEW.asset_hash
      AND size_bytes = NEW.size_bytes
      AND (media_type IS NULL OR media_type = NEW.media_type)
)
BEGIN
    SELECT RAISE(ABORT, 'asset descriptor does not match CAS metadata');
END;

CREATE TABLE asset_links (
    owner_revision_id TEXT NOT NULL
        REFERENCES content_revisions(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    asset_descriptor_id TEXT NOT NULL
        REFERENCES asset_descriptors(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    role TEXT NOT NULL CHECK (length(trim(role)) > 0),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    payload_json TEXT NOT NULL CHECK (
        json_valid(payload_json)
        AND json_type(payload_json) = 'object'
        AND length(CAST(payload_json AS BLOB)) <= 65536
    ),
    PRIMARY KEY (owner_revision_id, asset_descriptor_id, role),
    UNIQUE (owner_revision_id, role, ordinal)
);

CREATE INDEX asset_links_descriptor
    ON asset_links(asset_descriptor_id, owner_revision_id);

-- An inspected source is immutable. Selection, approval, commit and rollback
-- are separate durable records so reopening cannot manufacture fake success.
CREATE TABLE package_sources (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(trim(id)) > 0),
    source_hash TEXT NOT NULL UNIQUE
        REFERENCES content_sources(sha256)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    format TEXT NOT NULL CHECK (
        format IN (
            'lorepia_content_package',
            'public_character_card',
            'compat_import'
        )
    ),
    format_version INTEGER NOT NULL CHECK (format_version >= 1),
    package_id TEXT NOT NULL CHECK (length(trim(package_id)) > 0),
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    version TEXT NOT NULL CHECK (length(trim(version)) > 0),
    author TEXT,
    manifest_json TEXT NOT NULL CHECK (
        json_valid(manifest_json)
        AND json_type(manifest_json) = 'object'
        AND length(CAST(manifest_json AS BLOB)) <= 4194304
    ),
    manifest_sha256 TEXT NOT NULL CHECK (
        length(manifest_sha256) = 64
        AND manifest_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    license_expression TEXT,
    license_status TEXT NOT NULL CHECK (
        license_status IN ('declared', 'missing', 'unknown', 'invalid')
    ),
    redistribution_status TEXT NOT NULL CHECK (
        redistribution_status IN ('allowed', 'denied', 'unknown')
    ),
    required_app_version TEXT,
    signature_json TEXT CHECK (
        signature_json IS NULL
        OR (
            json_valid(signature_json)
            AND json_type(signature_json) = 'object'
            AND length(CAST(signature_json AS BLOB)) <= 262144
        )
    ),
    signature_status TEXT NOT NULL CHECK (
        signature_status IN ('unsigned', 'valid', 'invalid', 'untrusted')
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    CHECK (
        (license_status = 'declared' AND license_expression IS NOT NULL)
        OR license_status <> 'declared'
    )
);

CREATE INDEX package_sources_identity
    ON package_sources(package_id, version, id);
CREATE INDEX package_sources_share_policy
    ON package_sources(
        license_status,
        redistribution_status,
        package_id,
        version
    );

CREATE TRIGGER package_sources_no_update
BEFORE UPDATE ON package_sources
BEGIN
    SELECT RAISE(ABORT, 'package sources are immutable');
END;

CREATE TRIGGER package_sources_no_delete
BEFORE DELETE ON package_sources
BEGIN
    SELECT RAISE(ABORT, 'package sources are immutable');
END;

CREATE TABLE package_imports (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(trim(id)) > 0),
    package_source_id TEXT NOT NULL
        REFERENCES package_sources(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    inspection_schema_version INTEGER NOT NULL CHECK (
        inspection_schema_version >= 1
    ),
    state TEXT NOT NULL CHECK (
        state IN (
            'inspected',
            'awaiting_review',
            'approved',
            'committing',
            'completed',
            'failed',
            'discarded',
            'rolled_back'
        )
    ),
    revision INTEGER NOT NULL CHECK (revision >= 1),
    inspection_json TEXT NOT NULL CHECK (
        json_valid(inspection_json)
        AND json_type(inspection_json) = 'object'
        AND length(CAST(inspection_json AS BLOB)) <= 16777216
    ),
    inspection_sha256 TEXT NOT NULL CHECK (
        length(inspection_sha256) = 64
        AND inspection_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    selection_json TEXT CHECK (
        selection_json IS NULL
        OR (
            json_valid(selection_json)
            AND json_type(selection_json) = 'object'
            AND length(CAST(selection_json AS BLOB)) <= 8388608
        )
    ),
    selection_sha256 TEXT CHECK (
        selection_sha256 IS NULL
        OR (
            length(selection_sha256) = 64
            AND selection_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    ),
    capability_review_sha256 TEXT NOT NULL CHECK (
        length(capability_review_sha256) = 64
        AND capability_review_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    approved_selection_sha256 TEXT CHECK (
        approved_selection_sha256 IS NULL
        OR (
            length(approved_selection_sha256) = 64
            AND approved_selection_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    ),
    approved_at TEXT,
    failure_json TEXT CHECK (
        failure_json IS NULL
        OR (
            json_valid(failure_json)
            AND json_type(failure_json) = 'object'
            AND length(CAST(failure_json AS BLOB)) <= 65536
        )
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    updated_at TEXT NOT NULL CHECK (length(trim(updated_at)) > 0),
    completed_at TEXT,
    CHECK (
        (selection_json IS NULL AND selection_sha256 IS NULL)
        OR (selection_json IS NOT NULL AND selection_sha256 IS NOT NULL)
    ),
    CHECK (
        (
            approved_selection_sha256 IS NULL
            AND approved_at IS NULL
        )
        OR (
            selection_sha256 IS NOT NULL
            AND
            approved_selection_sha256 = selection_sha256
            AND approved_at IS NOT NULL
        )
    ),
    CHECK (
        approved_selection_sha256 IS NULL
        OR state IN (
            'approved',
            'committing',
            'completed',
            'failed',
            'discarded',
            'rolled_back'
        )
    ),
    CHECK (
        state NOT IN ('approved', 'committing', 'completed', 'rolled_back')
        OR approved_selection_sha256 IS NOT NULL
    ),
    CHECK (
        state <> 'failed' OR failure_json IS NOT NULL
    ),
    CHECK (
        state = 'failed' OR failure_json IS NULL
    ),
    CHECK (
        state IN ('completed', 'failed', 'discarded', 'rolled_back')
        OR completed_at IS NULL
    ),
    CHECK (
        state NOT IN ('completed', 'rolled_back')
        OR completed_at IS NOT NULL
    )
);

CREATE UNIQUE INDEX package_imports_one_live_per_source
    ON package_imports(package_source_id)
    WHERE state IN (
        'inspected',
        'awaiting_review',
        'approved',
        'committing'
    );
CREATE INDEX package_imports_state_updated
    ON package_imports(state, updated_at, id);
CREATE INDEX package_imports_source_history
    ON package_imports(package_source_id, created_at DESC, id);

CREATE TRIGGER package_imports_initial_state_guard
BEFORE INSERT ON package_imports
WHEN NEW.state NOT IN ('inspected', 'awaiting_review')
BEGIN
    SELECT RAISE(ABORT, 'package import must begin in a reviewable state');
END;

CREATE TRIGGER package_imports_transition_guard
BEFORE UPDATE ON package_imports
WHEN
    NEW.id != OLD.id
    OR NEW.package_source_id != OLD.package_source_id
    OR NEW.inspection_schema_version != OLD.inspection_schema_version
    OR NEW.inspection_json != OLD.inspection_json
    OR NEW.inspection_sha256 != OLD.inspection_sha256
    OR NEW.capability_review_sha256 != OLD.capability_review_sha256
    OR NEW.created_at != OLD.created_at
    OR NEW.revision != OLD.revision + 1
    OR (
        OLD.state = 'inspected'
        AND NEW.state NOT IN (
            'awaiting_review',
            'failed',
            'discarded'
        )
    )
    OR (
        OLD.state = 'awaiting_review'
        AND NEW.state NOT IN (
            'awaiting_review',
            'approved',
            'failed',
            'discarded'
        )
    )
    OR (
        OLD.state = 'approved'
        AND NEW.state NOT IN ('committing', 'failed', 'discarded')
    )
    OR (
        OLD.state = 'committing'
        AND NEW.state NOT IN ('completed', 'failed')
    )
    OR (
        OLD.state = 'completed'
        AND NEW.state != 'rolled_back'
    )
    OR OLD.state IN ('failed', 'discarded', 'rolled_back')
    OR (
        OLD.state IN (
            'approved',
            'committing',
            'completed',
            'failed',
            'discarded',
            'rolled_back'
        )
        AND (
            NEW.selection_json IS NOT OLD.selection_json
            OR NEW.selection_sha256 IS NOT OLD.selection_sha256
            OR NEW.approved_selection_sha256
               IS NOT OLD.approved_selection_sha256
            OR NEW.approved_at IS NOT OLD.approved_at
        )
    )
BEGIN
    SELECT RAISE(ABORT, 'package import transition is invalid');
END;

CREATE TRIGGER package_imports_no_delete
BEFORE DELETE ON package_imports
BEGIN
    SELECT RAISE(ABORT, 'package import history is durable');
END;

CREATE TABLE package_import_components (
    import_id TEXT NOT NULL
        REFERENCES package_imports(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    source_component_key TEXT NOT NULL CHECK (
        length(trim(source_component_key)) > 0
    ),
    component_kind TEXT NOT NULL CHECK (
        component_kind IN (
            'character_content',
            'prompt_preset',
            'knowledge_book',
            'memory_profile',
            'transform_set',
            'interaction_rule_set',
            'content_module',
            'asset',
            'raw_extension'
        )
    ),
    disposition TEXT NOT NULL CHECK (
        disposition IN (
            'create',
            'update',
            'skip',
            'quarantine',
            'unsupported',
            'conflict'
        )
    ),
    selected INTEGER NOT NULL CHECK (selected IN (0, 1)),
    target_object_id TEXT
        REFERENCES content_objects(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    target_revision_id TEXT,
    review_json TEXT NOT NULL CHECK (
        json_valid(review_json)
        AND json_type(review_json) = 'object'
        AND length(CAST(review_json AS BLOB)) <= 2097152
    ),
    review_sha256 TEXT NOT NULL CHECK (
        length(review_sha256) = 64
        AND review_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    PRIMARY KEY (import_id, ordinal),
    UNIQUE (import_id, source_component_key),
    FOREIGN KEY (target_object_id, target_revision_id)
        REFERENCES content_revisions(object_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    CHECK (
        (target_object_id IS NULL AND target_revision_id IS NULL)
        OR (
            target_object_id IS NOT NULL
            AND target_revision_id IS NOT NULL
        )
    )
);

CREATE INDEX package_import_components_target
    ON package_import_components(target_object_id, target_revision_id)
    WHERE target_object_id IS NOT NULL;

CREATE TRIGGER package_import_components_target_kind_guard
BEFORE INSERT ON package_import_components
WHEN
    NEW.target_object_id IS NOT NULL
    AND NOT EXISTS (
        SELECT 1
        FROM content_objects
        WHERE id = NEW.target_object_id
          AND object_kind = NEW.component_kind
    )
BEGIN
    SELECT RAISE(ABORT, 'package component target kind is invalid');
END;

CREATE TABLE package_import_component_commits (
    import_id TEXT NOT NULL,
    component_ordinal INTEGER NOT NULL CHECK (component_ordinal >= 0),
    document_ordinal INTEGER NOT NULL CHECK (document_ordinal >= 0),
    target_object_id TEXT NOT NULL,
    target_revision_id TEXT NOT NULL,
    result_json TEXT NOT NULL CHECK (
        json_valid(result_json)
        AND json_type(result_json) = 'object'
        AND length(CAST(result_json AS BLOB)) <= 1048576
    ),
    result_sha256 TEXT NOT NULL CHECK (
        length(result_sha256) = 64
        AND result_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    committed_at TEXT NOT NULL CHECK (length(trim(committed_at)) > 0),
    PRIMARY KEY (import_id, component_ordinal, document_ordinal),
    UNIQUE (target_object_id, target_revision_id),
    FOREIGN KEY (import_id, component_ordinal)
        REFERENCES package_import_components(import_id, ordinal)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    FOREIGN KEY (target_object_id, target_revision_id)
        REFERENCES content_revisions(object_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

CREATE TRIGGER package_import_component_commits_guard
BEFORE INSERT ON package_import_component_commits
WHEN
    NOT EXISTS (
        SELECT 1
        FROM package_imports AS job
        JOIN package_import_components AS component
          ON component.import_id = job.id
         AND component.ordinal = NEW.component_ordinal
        JOIN content_objects AS object
          ON object.id = NEW.target_object_id
         AND object.object_kind = component.component_kind
        WHERE job.id = NEW.import_id
          AND job.state = 'committing'
          AND component.selected = 1
          AND component.disposition IN ('create', 'update')
    )
BEGIN
    SELECT RAISE(ABORT, 'package component commit is not approved or typed');
END;

CREATE TRIGGER package_import_component_commits_no_update
BEFORE UPDATE ON package_import_component_commits
BEGIN
    SELECT RAISE(ABORT, 'package component commit results are immutable');
END;

CREATE TRIGGER package_import_component_commits_no_delete
BEFORE DELETE ON package_import_component_commits
BEGIN
    SELECT RAISE(ABORT, 'package component commit results are immutable');
END;

CREATE TABLE package_capability_requests (
    import_id TEXT NOT NULL
        REFERENCES package_imports(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    capability TEXT NOT NULL CHECK (
        capability IN (
            'prompt_fragments',
            'knowledge',
            'variables',
            'transforms',
            'declarative_interactions',
            'image_assets',
            'audio_assets',
            'video_assets',
            'attachment_assets',
            'high_risk_assets',
            'external_urls',
            'html',
            'script',
            'native_code',
            'network',
            'filesystem',
            'shell',
            'credentials'
        )
    ),
    support_status TEXT NOT NULL CHECK (
        support_status IN (
            'supported',
            'unsupported',
            'approval_required'
        )
    ),
    approved INTEGER NOT NULL CHECK (approved IN (0, 1)),
    executable INTEGER NOT NULL DEFAULT 0 CHECK (executable = 0),
    reason TEXT NOT NULL CHECK (length(trim(reason)) > 0),
    PRIMARY KEY (import_id, capability),
    CHECK (
        capability NOT IN (
            'external_urls',
            'html',
            'script',
            'native_code',
            'network',
            'filesystem',
            'shell',
            'credentials'
        )
        OR approved = 0
    )
);

CREATE TABLE package_raw_extensions (
    package_source_id TEXT NOT NULL
        REFERENCES package_sources(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    logical_path TEXT NOT NULL CHECK (
        length(trim(logical_path)) > 0
        AND instr(logical_path, char(0)) = 0
        AND substr(logical_path, 1, 1) <> '/'
        AND instr('/' || logical_path || '/', '/../') = 0
    ),
    namespace TEXT,
    kind TEXT NOT NULL CHECK (
        kind IN (
            'unknown',
            'script',
            'html',
            'style',
            'code',
            'external_url',
            'binary'
        )
    ),
    content_sha256 TEXT NOT NULL CHECK (
        length(content_sha256) = 64
        AND content_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    raw_json TEXT CHECK (
        raw_json IS NULL
        OR (
            json_valid(raw_json)
            AND length(CAST(raw_json AS BLOB)) <= 4194304
        )
    ),
    asset_hash TEXT
        REFERENCES assets(sha256)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    disposition TEXT NOT NULL CHECK (
        disposition IN ('preserved_inert', 'excluded')
    ),
    PRIMARY KEY (package_source_id, ordinal),
    UNIQUE (package_source_id, logical_path),
    CHECK (
        (raw_json IS NULL) <> (asset_hash IS NULL)
    )
);

CREATE TRIGGER package_raw_extensions_no_update
BEFORE UPDATE ON package_raw_extensions
BEGIN
    SELECT RAISE(ABORT, 'package raw extensions are immutable');
END;

CREATE TRIGGER package_raw_extensions_no_delete
BEFORE DELETE ON package_raw_extensions
BEGIN
    SELECT RAISE(ABORT, 'package raw extensions are immutable');
END;

CREATE TABLE package_import_approvals (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(trim(id)) > 0),
    import_id TEXT NOT NULL
        REFERENCES package_imports(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    inspection_sha256 TEXT NOT NULL CHECK (
        length(inspection_sha256) = 64
        AND inspection_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    selection_sha256 TEXT NOT NULL CHECK (
        length(selection_sha256) = 64
        AND selection_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    capability_review_sha256 TEXT NOT NULL CHECK (
        length(capability_review_sha256) = 64
        AND capability_review_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    approval_payload_json TEXT NOT NULL CHECK (
        json_valid(approval_payload_json)
        AND json_type(approval_payload_json) = 'object'
        AND length(CAST(approval_payload_json AS BLOB)) <= 262144
    ),
    approved_at TEXT NOT NULL CHECK (length(trim(approved_at)) > 0),
    UNIQUE (import_id, selection_sha256)
);

CREATE TRIGGER package_import_approvals_snapshot_guard
BEFORE INSERT ON package_import_approvals
WHEN NOT EXISTS (
    SELECT 1
    FROM package_imports
    WHERE id = NEW.import_id
      AND state IN ('awaiting_review', 'approved')
      AND inspection_sha256 = NEW.inspection_sha256
      AND selection_sha256 = NEW.selection_sha256
      AND capability_review_sha256 = NEW.capability_review_sha256
)
BEGIN
    SELECT RAISE(ABORT, 'package approval does not match reviewed snapshots');
END;

CREATE TRIGGER package_import_approvals_no_update
BEFORE UPDATE ON package_import_approvals
BEGIN
    SELECT RAISE(ABORT, 'package import approvals are immutable');
END;

CREATE TRIGGER package_import_approvals_no_delete
BEFORE DELETE ON package_import_approvals
BEGIN
    SELECT RAISE(ABORT, 'package import approvals are immutable');
END;

CREATE TRIGGER character_content_revisions_no_update
BEFORE UPDATE ON character_content_revisions
BEGIN
    SELECT RAISE(ABORT, 'character content revisions are immutable');
END;
CREATE TRIGGER character_content_revisions_no_delete
BEFORE DELETE ON character_content_revisions
BEGIN
    SELECT RAISE(ABORT, 'character content revisions are immutable');
END;
CREATE TRIGGER character_greetings_no_update
BEFORE UPDATE ON character_greetings
BEGIN
    SELECT RAISE(ABORT, 'character greetings are immutable');
END;
CREATE TRIGGER character_greetings_no_delete
BEFORE DELETE ON character_greetings
BEGIN
    SELECT RAISE(ABORT, 'character greetings are immutable');
END;
CREATE TRIGGER character_dialogue_examples_no_update
BEFORE UPDATE ON character_dialogue_examples
BEGIN
    SELECT RAISE(ABORT, 'character dialogue examples are immutable');
END;
CREATE TRIGGER character_dialogue_examples_no_delete
BEFORE DELETE ON character_dialogue_examples
BEGIN
    SELECT RAISE(ABORT, 'character dialogue examples are immutable');
END;
CREATE TRIGGER character_dialogue_messages_no_update
BEFORE UPDATE ON character_dialogue_example_messages
BEGIN
    SELECT RAISE(ABORT, 'character dialogue messages are immutable');
END;
CREATE TRIGGER character_dialogue_messages_no_delete
BEFORE DELETE ON character_dialogue_example_messages
BEGIN
    SELECT RAISE(ABORT, 'character dialogue messages are immutable');
END;
CREATE TRIGGER persona_revisions_no_update
BEFORE UPDATE ON persona_revisions
BEGIN
    SELECT RAISE(ABORT, 'persona revisions are immutable');
END;
CREATE TRIGGER persona_revisions_no_delete
BEFORE DELETE ON persona_revisions
BEGIN
    SELECT RAISE(ABORT, 'persona revisions are immutable');
END;
CREATE TRIGGER asset_descriptors_no_update
BEFORE UPDATE ON asset_descriptors
BEGIN
    SELECT RAISE(ABORT, 'asset descriptors are immutable');
END;
CREATE TRIGGER asset_descriptors_no_delete
BEFORE DELETE ON asset_descriptors
BEGIN
    SELECT RAISE(ABORT, 'asset descriptors are immutable');
END;
CREATE TRIGGER asset_links_no_update
BEFORE UPDATE ON asset_links
BEGIN
    SELECT RAISE(ABORT, 'asset links are immutable');
END;
CREATE TRIGGER asset_links_no_delete
BEFORE DELETE ON asset_links
BEGIN
    SELECT RAISE(ABORT, 'asset links are immutable');
END;
CREATE TRIGGER package_import_components_no_update
BEFORE UPDATE ON package_import_components
BEGIN
    SELECT RAISE(ABORT, 'package import components are immutable');
END;
CREATE TRIGGER package_import_components_no_delete
BEFORE DELETE ON package_import_components
BEGIN
    SELECT RAISE(ABORT, 'package import components are immutable');
END;
CREATE TRIGGER package_capability_requests_no_update
BEFORE UPDATE ON package_capability_requests
BEGIN
    SELECT RAISE(ABORT, 'package capability review is immutable');
END;
CREATE TRIGGER package_capability_requests_no_delete
BEFORE DELETE ON package_capability_requests
BEGIN
    SELECT RAISE(ABORT, 'package capability review is immutable');
END;

CREATE TABLE package_import_audit_events (
    import_id TEXT NOT NULL
        REFERENCES package_imports(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    sequence INTEGER NOT NULL CHECK (sequence >= 1),
    import_revision INTEGER NOT NULL CHECK (import_revision >= 1),
    event_kind TEXT NOT NULL CHECK (
        event_kind IN (
            'inspected',
            'review_requested',
            'selection_changed',
            'approved',
            'commit_started',
            'component_committed',
            'commit_completed',
            'failed',
            'discarded',
            'rollback_started',
            'rolled_back'
        )
    ),
    payload_json TEXT NOT NULL CHECK (
        json_valid(payload_json)
        AND json_type(payload_json) = 'object'
        AND length(CAST(payload_json AS BLOB)) <= 1048576
    ),
    payload_sha256 TEXT NOT NULL CHECK (
        length(payload_sha256) = 64
        AND payload_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    PRIMARY KEY (import_id, sequence),
    UNIQUE (import_id, import_revision)
);

CREATE TRIGGER package_imports_approved_snapshot_guard
BEFORE UPDATE OF state ON package_imports
WHEN
    NEW.state IN ('approved', 'committing', 'completed', 'rolled_back')
    AND NOT EXISTS (
        SELECT 1
        FROM package_import_approvals
        WHERE import_id = NEW.id
          AND inspection_sha256 = NEW.inspection_sha256
          AND selection_sha256 = NEW.selection_sha256
          AND capability_review_sha256 = NEW.capability_review_sha256
    )
BEGIN
    SELECT RAISE(ABORT, 'package import has no matching approval');
END;

CREATE TRIGGER package_imports_completion_audit_guard
BEFORE UPDATE OF state ON package_imports
WHEN
    NEW.state = 'completed'
    AND OLD.state <> 'completed'
    AND NOT EXISTS (
        SELECT 1
        FROM package_import_audit_events
        WHERE import_id = NEW.id
          AND event_kind = 'commit_started'
    )
BEGIN
    SELECT RAISE(ABORT, 'package import completion lacks commit audit');
END;

CREATE INDEX package_import_audit_kind
    ON package_import_audit_events(event_kind, created_at, import_id);

CREATE TRIGGER package_import_audit_append_guard
BEFORE INSERT ON package_import_audit_events
WHEN NEW.sequence != (
    SELECT COALESCE(MAX(sequence), 0) + 1
    FROM package_import_audit_events
    WHERE import_id = NEW.import_id
)
BEGIN
    SELECT RAISE(ABORT, 'package import audit sequence is not append-only');
END;

CREATE TRIGGER package_import_audit_no_update
BEFORE UPDATE ON package_import_audit_events
BEGIN
    SELECT RAISE(ABORT, 'package import audit events are immutable');
END;

CREATE TRIGGER package_import_audit_no_delete
BEFORE DELETE ON package_import_audit_events
BEGIN
    SELECT RAISE(ABORT, 'package import audit events are immutable');
END;

-- Sharing is evaluated against an exact immutable content revision. Missing or
-- unknown licensing can never produce an eligible public-share decision.
CREATE TABLE share_eligibility_reviews (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(trim(id)) > 0),
    content_revision_id TEXT NOT NULL
        REFERENCES content_revisions(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    share_scope TEXT NOT NULL CHECK (
        share_scope IN ('local_export', 'public_share')
    ),
    policy_version INTEGER NOT NULL CHECK (policy_version >= 1),
    decision TEXT NOT NULL CHECK (
        decision IN ('eligible', 'warning', 'blocked')
    ),
    license_status TEXT NOT NULL CHECK (
        license_status IN ('declared', 'missing', 'unknown', 'invalid')
    ),
    redistribution_status TEXT NOT NULL CHECK (
        redistribution_status IN ('allowed', 'denied', 'unknown')
    ),
    blockers_json TEXT NOT NULL CHECK (
        json_valid(blockers_json)
        AND json_type(blockers_json) = 'array'
        AND length(CAST(blockers_json AS BLOB)) <= 262144
    ),
    evidence_json TEXT NOT NULL CHECK (
        json_valid(evidence_json)
        AND json_type(evidence_json) = 'object'
        AND length(CAST(evidence_json AS BLOB)) <= 1048576
    ),
    evaluated_at TEXT NOT NULL CHECK (length(trim(evaluated_at)) > 0),
    UNIQUE (content_revision_id, share_scope, policy_version),
    UNIQUE (content_revision_id, share_scope, id),
    CHECK (
        share_scope <> 'public_share'
        OR decision <> 'eligible'
        OR (
            license_status = 'declared'
            AND redistribution_status = 'allowed'
            AND json_array_length(blockers_json) = 0
        )
    )
);

CREATE INDEX share_eligibility_decision
    ON share_eligibility_reviews(
        decision,
        share_scope,
        evaluated_at,
        content_revision_id
    );

CREATE TRIGGER share_eligibility_reviews_no_update
BEFORE UPDATE ON share_eligibility_reviews
BEGIN
    SELECT RAISE(ABORT, 'share eligibility reviews are immutable');
END;

CREATE TRIGGER share_eligibility_reviews_no_delete
BEFORE DELETE ON share_eligibility_reviews
BEGIN
    SELECT RAISE(ABORT, 'share eligibility reviews are immutable');
END;

CREATE TABLE share_eligibility_state (
    content_revision_id TEXT NOT NULL
        REFERENCES content_revisions(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    share_scope TEXT NOT NULL CHECK (
        share_scope IN ('local_export', 'public_share')
    ),
    active_review_id TEXT NOT NULL,
    state_version INTEGER NOT NULL CHECK (state_version >= 1),
    updated_at TEXT NOT NULL CHECK (length(trim(updated_at)) > 0),
    PRIMARY KEY (content_revision_id, share_scope),
    FOREIGN KEY (
        content_revision_id,
        share_scope,
        active_review_id
    )
        REFERENCES share_eligibility_reviews(
            content_revision_id,
            share_scope,
            id
        )
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

CREATE TRIGGER share_eligibility_state_version_guard
BEFORE UPDATE ON share_eligibility_state
WHEN
    NEW.content_revision_id != OLD.content_revision_id
    OR NEW.share_scope != OLD.share_scope
    OR NEW.state_version != OLD.state_version + 1
    OR NEW.active_review_id = OLD.active_review_id
BEGIN
    SELECT RAISE(ABORT, 'share eligibility state update is not versioned');
END;
