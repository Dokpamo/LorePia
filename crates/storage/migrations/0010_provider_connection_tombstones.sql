PRAGMA foreign_keys = ON;

-- Provider connection identities are durable provenance. Archiving hides a
-- connection from active product flows without cascading away model routes,
-- generation provenance, model-sync history, or discovery audit references.
ALTER TABLE provider_connections
    ADD COLUMN archived_at TEXT
    CHECK (
        archived_at IS NULL
        OR length(trim(archived_at)) > 0
    );

CREATE INDEX provider_connections_active_display_name
    ON provider_connections(display_name COLLATE NOCASE, id)
    WHERE archived_at IS NULL;
