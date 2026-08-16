PRAGMA foreign_keys = ON;

CREATE TABLE transform_sets (
    id TEXT PRIMARY KEY NOT NULL
        REFERENCES content_objects(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    schema_version INTEGER NOT NULL CHECK (schema_version >= 1),
    revision INTEGER NOT NULL CHECK (revision >= 1),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    max_rules_per_phase INTEGER NOT NULL CHECK (
        max_rules_per_phase BETWEEN 1 AND 1024
    ),
    max_output_chars INTEGER NOT NULL CHECK (
        max_output_chars BETWEEN 1 AND 16777216
    ),
    document_json TEXT NOT NULL CHECK (
        json_valid(document_json)
        AND json_type(document_json) = 'object'
        AND length(CAST(document_json AS BLOB)) <= 8388608
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
        source_kind NOT IN ('imported_standard', 'imported_package')
        OR enabled = 0
    )
);

CREATE TRIGGER transform_sets_kind_guard
BEFORE INSERT ON transform_sets
WHEN NOT EXISTS (
    SELECT 1
    FROM content_objects
    WHERE id = NEW.id
      AND object_kind = 'transform_set'
)
BEGIN
    SELECT RAISE(ABORT, 'transform set object kind is invalid');
END;

CREATE INDEX transform_sets_active_name
    ON transform_sets(name COLLATE NOCASE, id)
    WHERE deleted_at IS NULL;

CREATE TABLE transform_set_revisions (
    revision_id TEXT PRIMARY KEY NOT NULL,
    transform_set_id TEXT NOT NULL
        REFERENCES transform_sets(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    revision_no INTEGER NOT NULL CHECK (revision_no >= 1),
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    max_rules_per_phase INTEGER NOT NULL CHECK (
        max_rules_per_phase BETWEEN 1 AND 1024
    ),
    max_output_chars INTEGER NOT NULL CHECK (
        max_output_chars BETWEEN 1 AND 16777216
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
    document_json TEXT NOT NULL CHECK (
        json_valid(document_json)
        AND json_type(document_json) = 'object'
        AND length(CAST(document_json AS BLOB)) <= 8388608
    ),
    UNIQUE (transform_set_id, revision_id),
    UNIQUE (transform_set_id, revision_no),
    FOREIGN KEY (transform_set_id, revision_id)
        REFERENCES content_revisions(object_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    CHECK (
        source_kind NOT IN ('imported_standard', 'imported_package')
        OR enabled = 0
    )
);

CREATE TABLE transform_rules (
    set_revision_id TEXT NOT NULL
        REFERENCES transform_set_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    rule_id TEXT NOT NULL CHECK (length(trim(rule_id)) > 0),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    imported_enabled INTEGER NOT NULL CHECK (
        imported_enabled IN (0, 1)
    ),
    phase TEXT NOT NULL CHECK (
        phase IN (
            'user_input_for_request',
            'resolved_prompt',
            'provider_output_canonical',
            'display_only',
            'memory_input'
        )
    ),
    engine TEXT NOT NULL CHECK (engine = 'rust_regex_v1'),
    pattern TEXT NOT NULL CHECK (
        length(CAST(pattern AS BLOB)) BETWEEN 1 AND 65536
    ),
    case_insensitive INTEGER NOT NULL CHECK (
        case_insensitive IN (0, 1)
    ),
    replacement TEXT NOT NULL CHECK (
        length(CAST(replacement AS BLOB)) <= 1048576
    ),
    condition_json TEXT CHECK (
        condition_json IS NULL
        OR (
            json_valid(condition_json)
            AND json_type(condition_json) = 'object'
            AND length(CAST(condition_json AS BLOB)) <= 262144
        )
    ),
    max_replacements INTEGER NOT NULL CHECK (
        max_replacements BETWEEN 1 AND 100000
    ),
    input_limit INTEGER NOT NULL CHECK (
        input_limit BETWEEN 1 AND 16777216
    ),
    output_limit INTEGER NOT NULL CHECK (
        output_limit BETWEEN 1 AND 16777216
    ),
    max_applications INTEGER NOT NULL DEFAULT 1 CHECK (
        max_applications = 1
    ),
    provenance_json TEXT NOT NULL CHECK (
        json_valid(provenance_json)
        AND json_type(provenance_json) = 'object'
        AND length(CAST(provenance_json AS BLOB)) <= 65536
    ),
    document_json TEXT NOT NULL CHECK (
        json_valid(document_json)
        AND json_type(document_json) = 'object'
        AND length(CAST(document_json AS BLOB)) <= 2097152
    ),
    PRIMARY KEY (set_revision_id, rule_id),
    UNIQUE (set_revision_id, ordinal),
    CHECK (output_limit <= 16777216)
);

CREATE INDEX transform_rules_phase_order
    ON transform_rules(
        set_revision_id,
        phase,
        enabled,
        ordinal,
        rule_id
    );

CREATE TRIGGER transform_rules_import_guard
BEFORE INSERT ON transform_rules
WHEN
    EXISTS (
        SELECT 1
        FROM transform_set_revisions
        WHERE revision_id = NEW.set_revision_id
          AND source_kind IN ('imported_standard', 'imported_package')
    )
    AND (NEW.enabled != 0 OR NEW.imported_enabled != 0)
BEGIN
    SELECT RAISE(
        ABORT,
        'imported transform rules must remain disabled'
    );
END;

CREATE TABLE transform_rule_tests (
    set_revision_id TEXT NOT NULL,
    rule_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    input_text TEXT NOT NULL CHECK (
        length(CAST(input_text AS BLOB)) <= 1048576
    ),
    expected_text TEXT NOT NULL CHECK (
        length(CAST(expected_text AS BLOB)) <= 1048576
    ),
    expected_status TEXT NOT NULL CHECK (
        expected_status IN ('applied', 'no_match', 'limit_rejected')
    ),
    payload_json TEXT NOT NULL CHECK (
        json_valid(payload_json)
        AND json_type(payload_json) = 'object'
        AND length(CAST(payload_json AS BLOB)) <= 262144
    ),
    PRIMARY KEY (set_revision_id, rule_id, ordinal),
    FOREIGN KEY (set_revision_id, rule_id)
        REFERENCES transform_rules(set_revision_id, rule_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

CREATE TABLE transform_application_logs (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(trim(id)) > 0),
    plan_id TEXT
        REFERENCES generation_prompt_plans(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    generation_id TEXT
        REFERENCES generations(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    message_id TEXT
        REFERENCES messages(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    set_revision_id TEXT NOT NULL,
    rule_id TEXT NOT NULL,
    phase TEXT NOT NULL CHECK (
        phase IN (
            'user_input_for_request',
            'resolved_prompt',
            'provider_output_canonical',
            'display_only',
            'memory_input'
        )
    ),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    status TEXT NOT NULL CHECK (
        status IN ('applied', 'no_match', 'failed', 'limit_rejected')
    ),
    before_sha256 TEXT NOT NULL CHECK (
        length(before_sha256) = 64
        AND before_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    after_sha256 TEXT CHECK (
        after_sha256 IS NULL
        OR (
            length(after_sha256) = 64
            AND after_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    ),
    replacement_count INTEGER NOT NULL CHECK (
        replacement_count >= 0
    ),
    input_chars INTEGER NOT NULL CHECK (input_chars >= 0),
    output_chars INTEGER NOT NULL CHECK (output_chars >= 0),
    error_code TEXT,
    diagnostics_json TEXT NOT NULL CHECK (
        json_valid(diagnostics_json)
        AND json_type(diagnostics_json) = 'object'
        AND length(CAST(diagnostics_json AS BLOB)) <= 262144
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    FOREIGN KEY (set_revision_id, rule_id)
        REFERENCES transform_rules(set_revision_id, rule_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    CHECK (
        plan_id IS NOT NULL
        OR generation_id IS NOT NULL
        OR message_id IS NOT NULL
    ),
    CHECK (
        (status = 'failed' AND error_code IS NOT NULL)
        OR (status <> 'failed' AND error_code IS NULL)
    ),
    CHECK (
        status NOT IN ('failed', 'limit_rejected')
        OR after_sha256 IS NULL
    )
);

CREATE INDEX transform_application_logs_plan
    ON transform_application_logs(plan_id, phase, ordinal, id)
    WHERE plan_id IS NOT NULL;
CREATE INDEX transform_application_logs_generation
    ON transform_application_logs(generation_id, phase, ordinal, id)
    WHERE generation_id IS NOT NULL;
CREATE INDEX transform_application_logs_failures
    ON transform_application_logs(status, created_at, id)
    WHERE status IN ('failed', 'limit_rejected');

CREATE TABLE prompt_preset_transform_sets (
    prompt_preset_revision_id TEXT NOT NULL
        REFERENCES prompt_preset_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    transform_set_revision_id TEXT NOT NULL
        REFERENCES transform_set_revisions(revision_id)
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
        transform_set_revision_id
    )
);

CREATE INDEX prompt_preset_transform_sets_target
    ON prompt_preset_transform_sets(
        transform_set_revision_id,
        prompt_preset_revision_id
    );

CREATE TRIGGER transform_set_revisions_no_update
BEFORE UPDATE ON transform_set_revisions
BEGIN
    SELECT RAISE(ABORT, 'transform set revisions are immutable');
END;
CREATE TRIGGER transform_set_revisions_no_delete
BEFORE DELETE ON transform_set_revisions
BEGIN
    SELECT RAISE(ABORT, 'transform set revisions are immutable');
END;
CREATE TRIGGER transform_rules_no_update
BEFORE UPDATE ON transform_rules
BEGIN
    SELECT RAISE(ABORT, 'transform rules are immutable');
END;
CREATE TRIGGER transform_rules_no_delete
BEFORE DELETE ON transform_rules
BEGIN
    SELECT RAISE(ABORT, 'transform rules are immutable');
END;
CREATE TRIGGER transform_rule_tests_no_update
BEFORE UPDATE ON transform_rule_tests
BEGIN
    SELECT RAISE(ABORT, 'transform rule tests are immutable');
END;
CREATE TRIGGER transform_rule_tests_no_delete
BEFORE DELETE ON transform_rule_tests
BEGIN
    SELECT RAISE(ABORT, 'transform rule tests are immutable');
END;
CREATE TRIGGER transform_application_logs_no_update
BEFORE UPDATE ON transform_application_logs
BEGIN
    SELECT RAISE(ABORT, 'transform application logs are immutable');
END;
CREATE TRIGGER transform_application_logs_no_delete
BEFORE DELETE ON transform_application_logs
BEGIN
    SELECT RAISE(ABORT, 'transform application logs are immutable');
END;
