-- Expand both durable content-module capability contracts so imported
-- portable runtimes can be represented and explicitly approved. SQLite cannot
-- alter a CHECK constraint in place, so rebuild the tables inside the
-- migration transaction while preserving every reviewed decision byte-for-byte.

-- These tables are not rename targets for any surviving foreign key, trigger,
-- or view. Legacy rename mode therefore avoids reparsing unrelated integrity
-- triggers that depend on application-registered SQLite functions, without
-- changing any schema reference.
PRAGMA legacy_alter_table = ON;

CREATE TABLE content_module_required_capabilities_v41 (
    module_revision_id TEXT NOT NULL
        REFERENCES content_module_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    capability TEXT NOT NULL CHECK (
        capability IN (
            'prompt_fragments',
            'knowledge',
            'variables',
            'transforms',
            'declarative_interactions',
            'portable_runtime',
            'image_assets',
            'audio_assets',
            'video_assets',
            'attachment_assets',
            'high_risk_assets'
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
    reason TEXT NOT NULL CHECK (length(trim(reason)) > 0),
    PRIMARY KEY (module_revision_id, capability),
    CHECK (
        support_status = 'supported'
        OR approved = 0
    )
);

INSERT INTO content_module_required_capabilities_v41 (
    module_revision_id,
    capability,
    support_status,
    approved,
    reason
)
SELECT
    module_revision_id,
    capability,
    support_status,
    approved,
    reason
FROM content_module_required_capabilities;

DROP TABLE content_module_required_capabilities;
ALTER TABLE content_module_required_capabilities_v41
    RENAME TO content_module_required_capabilities;

DROP TRIGGER package_capability_requests_no_update;
DROP TRIGGER package_capability_requests_no_delete;

CREATE TABLE package_capability_requests_v41 (
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
            'portable_runtime',
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

INSERT INTO package_capability_requests_v41 (
    import_id,
    capability,
    support_status,
    approved,
    executable,
    reason
)
SELECT
    import_id,
    capability,
    support_status,
    approved,
    executable,
    reason
FROM package_capability_requests;

DROP TABLE package_capability_requests;
ALTER TABLE package_capability_requests_v41 RENAME TO package_capability_requests;

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

PRAGMA legacy_alter_table = OFF;
