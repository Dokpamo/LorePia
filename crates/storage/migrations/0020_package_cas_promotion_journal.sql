CREATE TABLE package_cas_promotion_journal (
    import_id TEXT NOT NULL CHECK (
        length(trim(import_id)) BETWEEN 1 AND 256
    ),
    namespace TEXT NOT NULL CHECK (
        namespace IN ('source', 'asset')
    ),
    sha256 TEXT NOT NULL CHECK (
        length(sha256) = 64
        AND sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
    media_type TEXT,
    relative_path TEXT NOT NULL CHECK (
        relative_path = CASE namespace
            WHEN 'source' THEN
                'sources/sha256/' || substr(sha256, 1, 2) || '/' ||
                substr(sha256, 3)
            WHEN 'asset' THEN
                'assets/sha256/' || substr(sha256, 1, 2) || '/' ||
                substr(sha256, 3)
        END
    ),
    phase TEXT NOT NULL CHECK (
        phase IN (
            'intent',
            'file_durable',
            'row_registered',
            'cleanup_pending'
        )
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    updated_at TEXT NOT NULL CHECK (length(trim(updated_at)) > 0),
    PRIMARY KEY (import_id, namespace, sha256),
    CHECK (
        (namespace = 'source' AND media_type IS NULL)
        OR (
            namespace = 'asset'
            AND media_type IS NOT NULL
            AND length(trim(media_type)) > 0
        )
    )
);

CREATE INDEX package_cas_promotion_journal_artifact
    ON package_cas_promotion_journal(namespace, sha256, import_id);

CREATE INDEX package_cas_promotion_journal_phase
    ON package_cas_promotion_journal(phase, updated_at, import_id);

CREATE TRIGGER package_cas_promotion_journal_identity_guard
BEFORE UPDATE ON package_cas_promotion_journal
WHEN
    NEW.import_id <> OLD.import_id
    OR NEW.namespace <> OLD.namespace
    OR NEW.sha256 <> OLD.sha256
    OR NEW.size_bytes <> OLD.size_bytes
    OR NEW.media_type IS NOT OLD.media_type
    OR NEW.relative_path <> OLD.relative_path
    OR NEW.created_at <> OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'package CAS promotion identity is immutable');
END;
