PRAGMA foreign_keys = ON;

-- A LAN exception is an explicit, reviewable grant for one exact origin and
-- one finite address set. The typed config remains the canonical application
-- value; this relational mirror makes the grant independently auditable and
-- prevents an approved connection from being silently rebound.
CREATE TABLE provider_connection_local_network_approvals (
    connection_id TEXT PRIMARY KEY
        REFERENCES provider_connections(id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE,
    origin TEXT NOT NULL CHECK (length(trim(origin)) > 0),
    addresses_json TEXT NOT NULL CHECK (
        json_valid(addresses_json)
        AND json_type(addresses_json) = 'array'
        AND json_array_length(addresses_json) BETWEEN 1 AND 16
    ),
    approved_at TEXT NOT NULL CHECK (length(trim(approved_at)) > 0)
);

CREATE INDEX provider_connection_local_network_approvals_origin
    ON provider_connection_local_network_approvals(origin, connection_id);

CREATE TRIGGER provider_local_network_approval_insert_guard
BEFORE INSERT ON provider_connection_local_network_approvals
BEGIN
    SELECT CASE
        WHEN NOT EXISTS (
            SELECT 1
            FROM provider_connections
            WHERE id = NEW.connection_id
              AND api_origin = NEW.origin
              AND json_extract(config_json, '$.network_mode') =
                  'approved_local_network'
              AND json_extract(
                    config_json,
                    '$.local_network_approval.origin'
                  ) = NEW.origin
              AND json(
                    json_extract(
                      config_json,
                      '$.local_network_approval.addresses'
                    )
                  ) = json(NEW.addresses_json)
        )
        THEN RAISE(
            ABORT,
            'local-network approval does not match provider connection config'
        )
    END;
    SELECT CASE
        WHEN EXISTS (
            SELECT 1
            FROM json_each(NEW.addresses_json)
            WHERE type != 'text' OR length(trim(value)) = 0
        )
        THEN RAISE(
            ABORT,
            'local-network approval addresses must be non-empty strings'
        )
    END;
END;

CREATE TRIGGER provider_local_network_approval_immutable
BEFORE UPDATE ON provider_connection_local_network_approvals
BEGIN
    SELECT RAISE(
        ABORT,
        'local-network approval is immutable; create a new connection'
    );
END;

CREATE TRIGGER provider_connection_local_network_approval_guard
BEFORE UPDATE OF api_origin, config_json ON provider_connections
WHEN EXISTS (
    SELECT 1
    FROM provider_connection_local_network_approvals
    WHERE connection_id = OLD.id
)
BEGIN
    SELECT CASE
        WHEN NOT EXISTS (
            SELECT 1
            FROM provider_connection_local_network_approvals
            WHERE connection_id = OLD.id
              AND origin = NEW.api_origin
              AND json_extract(NEW.config_json, '$.network_mode') =
                  'approved_local_network'
              AND json_extract(
                    NEW.config_json,
                    '$.local_network_approval.origin'
                  ) = origin
              AND json(
                    json_extract(
                      NEW.config_json,
                      '$.local_network_approval.addresses'
                    )
                  ) = json(addresses_json)
        )
        THEN RAISE(
            ABORT,
            'provider connection local-network approval is immutable'
        )
    END;
END;

-- Schema 10 could be opened briefly by a newer application binary while this
-- migration was being developed. Preserve any already-valid typed grants.
INSERT INTO provider_connection_local_network_approvals (
    connection_id,
    origin,
    addresses_json,
    approved_at
)
SELECT
    id,
    api_origin,
    json(json_extract(config_json, '$.local_network_approval.addresses')),
    created_at
FROM provider_connections
WHERE json_extract(config_json, '$.network_mode') =
      'approved_local_network';
