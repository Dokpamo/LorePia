PRAGMA foreign_keys = ON;

-- One selected source component may normalize into several typed documents.
-- Keep the exact create/update decision at document granularity so mixed
-- components never collapse into an implicit overwrite decision.
CREATE TABLE package_import_document_target_reviews (
    import_id TEXT NOT NULL,
    component_ordinal INTEGER NOT NULL CHECK (component_ordinal >= 0),
    document_ordinal INTEGER NOT NULL CHECK (document_ordinal >= 0),
    document_index INTEGER NOT NULL CHECK (document_index >= 0),
    document_kind TEXT NOT NULL CHECK (
        document_kind IN (
            'character_content',
            'prompt_preset',
            'knowledge_book',
            'memory_profile',
            'transform_set',
            'interaction_rule_set',
            'content_module'
        )
    ),
    target_object_id TEXT NOT NULL CHECK (
        length(target_object_id) BETWEEN 1 AND 256
        AND target_object_id = trim(target_object_id)
        AND instr(target_object_id, char(0)) = 0
    ),
    disposition TEXT NOT NULL CHECK (disposition IN ('create', 'update')),
    expected_target_revision_id TEXT,
    expected_target_state_revision INTEGER,
    source_component_sha256 TEXT NOT NULL CHECK (
        length(source_component_sha256) = 64
        AND source_component_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    document_sha256 TEXT NOT NULL CHECK (
        length(document_sha256) = 64
        AND document_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    PRIMARY KEY (import_id, component_ordinal, document_ordinal),
    UNIQUE (import_id, document_index),
    UNIQUE (import_id, target_object_id),
    FOREIGN KEY (import_id, component_ordinal)
        REFERENCES package_import_components(import_id, ordinal)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    FOREIGN KEY (target_object_id, expected_target_revision_id)
        REFERENCES content_revisions(object_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    CHECK (
        (
            disposition = 'create'
            AND expected_target_revision_id IS NULL
            AND expected_target_state_revision IS NULL
        )
        OR (
            disposition = 'update'
            AND expected_target_revision_id IS NOT NULL
            AND expected_target_state_revision >= 1
        )
    )
);

CREATE INDEX package_import_document_target_reviews_target
    ON package_import_document_target_reviews(
        target_object_id,
        expected_target_revision_id
    );

CREATE TRIGGER package_import_document_target_reviews_guard
BEFORE INSERT ON package_import_document_target_reviews
WHEN
    NOT EXISTS (
        SELECT 1
        FROM package_import_components AS component
        JOIN package_imports AS job
          ON job.id = component.import_id
        WHERE component.import_id = NEW.import_id
          AND component.ordinal = NEW.component_ordinal
          AND job.state = 'inspected'
          AND job.selection_json IS NULL
          AND job.selection_sha256 IS NULL
          AND component.selected = 1
          AND component.component_kind = NEW.document_kind
          AND component.disposition IN ('create', 'update', 'conflict')
          AND json_extract(component.review_json, '$.id')
              = component.source_component_key
          AND json_extract(component.review_json, '$.kind')
              = component.component_kind
          AND json_extract(component.review_json, '$.sha256')
              = NEW.source_component_sha256
    )
    OR (
        NEW.disposition = 'create'
        AND EXISTS (
            SELECT 1
            FROM content_objects
            WHERE id = NEW.target_object_id
        )
    )
    OR (
        NEW.disposition = 'update'
        AND NOT EXISTS (
            SELECT 1
            FROM content_objects AS object
            JOIN content_object_state AS state
              ON state.object_id = object.id
            WHERE object.id = NEW.target_object_id
              AND object.object_kind = NEW.document_kind
              AND object.deleted_at IS NULL
              AND state.active_revision_id = NEW.expected_target_revision_id
              AND state.state_version = NEW.expected_target_state_revision
        )
    )
BEGIN
    SELECT RAISE(ABORT, 'package document target review is stale or untyped');
END;

CREATE TRIGGER package_import_document_target_reviews_no_update
BEFORE UPDATE ON package_import_document_target_reviews
BEGIN
    SELECT RAISE(ABORT, 'package document target reviews are immutable');
END;

CREATE TRIGGER package_import_document_target_reviews_no_delete
BEFORE DELETE ON package_import_document_target_reviews
BEGIN
    SELECT RAISE(ABORT, 'package document target reviews are immutable');
END;

DROP TRIGGER package_import_component_commits_guard;

CREATE TRIGGER package_import_component_commits_guard
BEFORE INSERT ON package_import_component_commits
WHEN
    NOT EXISTS (
        SELECT 1
        FROM package_imports AS job
        JOIN package_import_components AS component
          ON component.import_id = job.id
         AND component.ordinal = NEW.component_ordinal
        JOIN package_import_document_target_reviews AS target_review
          ON target_review.import_id = component.import_id
         AND target_review.component_ordinal = component.ordinal
         AND target_review.document_ordinal = NEW.document_ordinal
         AND target_review.target_object_id = NEW.target_object_id
         AND target_review.document_kind = component.component_kind
        JOIN content_objects AS object
          ON object.id = NEW.target_object_id
         AND object.object_kind = component.component_kind
        WHERE job.id = NEW.import_id
          AND job.state = 'committing'
          AND component.selected = 1
          AND component.disposition IN ('create', 'update', 'conflict')
    )
BEGIN
    SELECT RAISE(ABORT, 'package component commit is not approved or typed');
END;
