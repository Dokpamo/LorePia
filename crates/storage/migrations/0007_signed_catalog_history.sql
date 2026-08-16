PRAGMA foreign_keys = ON;

-- Exact, already-verified signed input. These rows are deliberately separate
-- from the mutable active pointer so a reload can re-run signature validation
-- against the original envelope.
CREATE TABLE provider_catalog_signed_envelopes (
    id TEXT PRIMARY KEY CHECK (
        length(id) BETWEEN 1 AND 160
        AND id = trim(id)
        AND instr(id, char(0)) = 0
    ),
    catalog_id TEXT NOT NULL CHECK (
        length(catalog_id) BETWEEN 1 AND 160
        AND catalog_id = trim(catalog_id)
        AND instr(catalog_id, char(0)) = 0
    ),
    catalog_schema_version INTEGER NOT NULL CHECK (
        catalog_schema_version > 0
    ),
    catalog_revision INTEGER NOT NULL CHECK (catalog_revision > 0),
    envelope_version INTEGER NOT NULL CHECK (envelope_version > 0),
    signing_key_id TEXT NOT NULL CHECK (
        length(signing_key_id) BETWEEN 1 AND 64
        AND signing_key_id = trim(signing_key_id)
        AND signing_key_id NOT GLOB '*[^a-z0-9-]*'
    ),
    envelope_bytes BLOB NOT NULL CHECK (
        json_valid(CAST(envelope_bytes AS TEXT))
        AND json_type(CAST(envelope_bytes AS TEXT)) = 'object'
        AND length(envelope_bytes) <= 2097152
    ),
    envelope_sha256 TEXT NOT NULL CHECK (
        length(envelope_sha256) = 64
        AND envelope_sha256 NOT GLOB '*[^0-9a-f]*'
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
    issued_at TEXT NOT NULL CHECK (
        length(issued_at) BETWEEN 20 AND 35
        AND issued_at = trim(issued_at)
    ),
    effective_at TEXT NOT NULL CHECK (
        length(effective_at) BETWEEN 20 AND 35
        AND effective_at = trim(effective_at)
        AND effective_at >= issued_at
    ),
    expires_at TEXT NOT NULL CHECK (
        length(expires_at) BETWEEN 20 AND 35
        AND expires_at = trim(expires_at)
        AND expires_at > effective_at
    ),
    accepted_at TEXT NOT NULL CHECK (
        length(accepted_at) BETWEEN 20 AND 35
        AND accepted_at = trim(accepted_at)
    ),
    UNIQUE (catalog_id, catalog_revision),
    UNIQUE (catalog_revision),
    UNIQUE (envelope_sha256),
    UNIQUE (payload_sha256),
    UNIQUE (id, catalog_revision),
    UNIQUE (id, catalog_revision, payload_sha256)
);

CREATE INDEX provider_catalog_signed_envelopes_imported
    ON provider_catalog_signed_envelopes(accepted_at DESC, catalog_revision DESC);

CREATE TRIGGER provider_catalog_signed_envelopes_no_update
BEFORE UPDATE ON provider_catalog_signed_envelopes
BEGIN
    SELECT RAISE(ABORT, 'signed catalog envelopes are immutable');
END;

CREATE TRIGGER provider_catalog_signed_envelopes_no_delete
BEFORE DELETE ON provider_catalog_signed_envelopes
BEGIN
    SELECT RAISE(ABORT, 'signed catalog envelopes are immutable');
END;

-- Every effective merged catalog is retained as an immutable local snapshot.
-- Local revisions form the history ordering and need not equal the signed
-- catalog revision (the first entry can be the bundled baseline).
CREATE TABLE provider_catalog_snapshots (
    local_revision INTEGER PRIMARY KEY CHECK (local_revision > 0),
    snapshot_schema_version INTEGER NOT NULL CHECK (
        snapshot_schema_version > 0
    ),
    snapshot_json TEXT NOT NULL CHECK (
        json_valid(snapshot_json)
        AND json_type(snapshot_json) = 'object'
        AND length(CAST(snapshot_json AS BLOB)) <= 2097152
    ),
    snapshot_sha256 TEXT NOT NULL CHECK (
        length(snapshot_sha256) = 64
        AND snapshot_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    bundled_revision INTEGER NOT NULL CHECK (bundled_revision > 0),
    bundled_sha256 TEXT NOT NULL CHECK (
        length(bundled_sha256) = 64
        AND bundled_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    signed_revision_chain_json TEXT NOT NULL CHECK (
        json_valid(signed_revision_chain_json)
        AND json_type(signed_revision_chain_json) = 'array'
        AND length(CAST(signed_revision_chain_json AS BLOB)) <= 131072
    ),
    source_kind TEXT NOT NULL CHECK (
        source_kind IN ('bundled_baseline', 'signed_import')
    ),
    source_envelope_id TEXT,
    catalog_revision INTEGER,
    captured_at TEXT NOT NULL CHECK (
        length(captured_at) BETWEEN 20 AND 35
        AND captured_at = trim(captured_at)
    ),
    CHECK (
        (
            source_kind = 'bundled_baseline'
            AND source_envelope_id IS NULL
            AND catalog_revision IS NULL
        )
        OR (
            source_kind = 'signed_import'
            AND source_envelope_id IS NOT NULL
            AND catalog_revision > 0
        )
    ),
    UNIQUE (snapshot_sha256),
    UNIQUE (local_revision, snapshot_sha256),
    FOREIGN KEY (source_envelope_id, catalog_revision)
        REFERENCES provider_catalog_signed_envelopes(id, catalog_revision)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

CREATE INDEX provider_catalog_snapshots_source
    ON provider_catalog_snapshots(source_kind, catalog_revision, local_revision);

CREATE TRIGGER provider_catalog_snapshots_no_update
BEFORE UPDATE ON provider_catalog_snapshots
BEGIN
    SELECT RAISE(ABORT, 'provider catalog snapshots are immutable');
END;

CREATE TRIGGER provider_catalog_snapshots_no_delete
BEFORE DELETE ON provider_catalog_snapshots
BEGIN
    SELECT RAISE(ABORT, 'provider catalog snapshots are immutable');
END;

-- A relational copy of the signed-revision chain binds every history entry to
-- the exact accepted envelopes. The JSON copy remains convenient for typed
-- round trips; Rust verifies both representations agree.
CREATE TABLE provider_catalog_snapshot_envelopes (
    local_revision INTEGER NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    envelope_id TEXT NOT NULL,
    catalog_revision INTEGER NOT NULL CHECK (catalog_revision > 0),
    payload_sha256 TEXT NOT NULL CHECK (
        length(payload_sha256) = 64
        AND payload_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    PRIMARY KEY (local_revision, ordinal),
    UNIQUE (local_revision, catalog_revision),
    FOREIGN KEY (local_revision)
        REFERENCES provider_catalog_snapshots(local_revision)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    FOREIGN KEY (envelope_id, catalog_revision, payload_sha256)
        REFERENCES provider_catalog_signed_envelopes(
            id,
            catalog_revision,
            payload_sha256
        )
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

CREATE INDEX provider_catalog_snapshot_envelopes_revision
    ON provider_catalog_snapshot_envelopes(
        catalog_revision,
        local_revision
    );

CREATE TRIGGER provider_catalog_snapshot_envelopes_no_update
BEFORE UPDATE ON provider_catalog_snapshot_envelopes
BEGIN
    SELECT RAISE(ABORT, 'provider catalog snapshot chains are immutable');
END;

CREATE TRIGGER provider_catalog_snapshot_envelopes_no_delete
BEFORE DELETE ON provider_catalog_snapshot_envelopes
BEGIN
    SELECT RAISE(ABORT, 'provider catalog snapshot chains are immutable');
END;

CREATE TRIGGER provider_catalog_snapshot_envelopes_append_only
BEFORE INSERT ON provider_catalog_snapshot_envelopes
WHEN
    NEW.ordinal != (
        SELECT COUNT(*)
        FROM provider_catalog_snapshot_envelopes
        WHERE local_revision = NEW.local_revision
    )
    OR (
        NEW.ordinal > 0
        AND NEW.catalog_revision <= (
            SELECT catalog_revision
            FROM provider_catalog_snapshot_envelopes
            WHERE local_revision = NEW.local_revision
            ORDER BY ordinal DESC
            LIMIT 1
        )
    )
BEGIN
    SELECT RAISE(ABORT, 'provider catalog snapshot chain is not ordered');
END;

-- Singleton mutable pointer plus the monotonic signed-revision guard.
CREATE TABLE provider_catalog_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    state_version INTEGER NOT NULL CHECK (state_version >= 0),
    active_local_revision INTEGER,
    active_snapshot_sha256 TEXT,
    highest_accepted_revision INTEGER NOT NULL CHECK (
        highest_accepted_revision >= 0
    ),
    latest_issued_at TEXT,
    updated_at TEXT NOT NULL CHECK (
        length(updated_at) BETWEEN 20 AND 35
        AND updated_at = trim(updated_at)
    ),
    CHECK (
        (active_local_revision IS NULL AND active_snapshot_sha256 IS NULL)
        OR (
            active_local_revision > 0
            AND length(active_snapshot_sha256) = 64
            AND active_snapshot_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    ),
    CHECK (
        (
            highest_accepted_revision = 0
            AND latest_issued_at IS NULL
        )
        OR (
            highest_accepted_revision > 0
            AND length(latest_issued_at) BETWEEN 20 AND 35
            AND latest_issued_at = trim(latest_issued_at)
        )
    ),
    FOREIGN KEY (active_local_revision, active_snapshot_sha256)
        REFERENCES provider_catalog_snapshots(
            local_revision,
            snapshot_sha256
        )
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

INSERT INTO provider_catalog_state (
    singleton,
    state_version,
    active_local_revision,
    active_snapshot_sha256,
    highest_accepted_revision,
    latest_issued_at,
    updated_at
) VALUES (
    1,
    0,
    NULL,
    NULL,
    0,
    NULL,
    '1970-01-01T00:00:00Z'
);

CREATE TRIGGER provider_catalog_signed_envelopes_revision_guard
BEFORE INSERT ON provider_catalog_signed_envelopes
WHEN NEW.catalog_revision <= (
    SELECT highest_accepted_revision
    FROM provider_catalog_state
    WHERE singleton = 1
)
BEGIN
    SELECT RAISE(ABORT, 'signed catalog revision was already passed');
END;

CREATE TRIGGER provider_catalog_state_no_delete
BEFORE DELETE ON provider_catalog_state
BEGIN
    SELECT RAISE(ABORT, 'provider catalog state cannot be deleted');
END;

CREATE TRIGGER provider_catalog_state_guard_monotonic
BEFORE UPDATE OF highest_accepted_revision, latest_issued_at
ON provider_catalog_state
WHEN
    NEW.highest_accepted_revision < OLD.highest_accepted_revision
    OR (
        OLD.latest_issued_at IS NOT NULL
        AND (
            NEW.latest_issued_at IS NULL
            OR NEW.latest_issued_at < OLD.latest_issued_at
        )
    )
BEGIN
    SELECT RAISE(ABORT, 'provider catalog revision guard cannot decrease');
END;

CREATE TRIGGER provider_catalog_state_guard_matches_history
BEFORE UPDATE OF highest_accepted_revision, latest_issued_at
ON provider_catalog_state
WHEN
    NEW.highest_accepted_revision != COALESCE(
        (
            SELECT MAX(catalog_revision)
            FROM provider_catalog_signed_envelopes
        ),
        0
    )
    OR (
        NEW.highest_accepted_revision > 0
        AND NEW.latest_issued_at != (
            SELECT issued_at
            FROM provider_catalog_signed_envelopes
            WHERE catalog_revision = NEW.highest_accepted_revision
        )
    )
BEGIN
    SELECT RAISE(ABORT, 'provider catalog guard does not match accepted history');
END;

-- Append-only evidence for both signed imports and user-approved local
-- rollbacks. The state_version uniqueness makes each pointer transition
-- accountable exactly once.
CREATE TABLE provider_catalog_activation_audit (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    action_id TEXT NOT NULL UNIQUE CHECK (
        length(action_id) BETWEEN 1 AND 160
        AND action_id = trim(action_id)
        AND instr(action_id, char(0)) = 0
    ),
    state_version INTEGER NOT NULL UNIQUE CHECK (state_version > 0),
    activation_kind TEXT NOT NULL CHECK (
        activation_kind IN ('import', 'rollback')
    ),
    from_local_revision INTEGER,
    from_snapshot_sha256 TEXT,
    to_local_revision INTEGER NOT NULL CHECK (to_local_revision > 0),
    to_snapshot_sha256 TEXT NOT NULL CHECK (
        length(to_snapshot_sha256) = 64
        AND to_snapshot_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    source_envelope_id TEXT,
    signed_catalog_revision INTEGER,
    signing_key_id TEXT,
    diff_json TEXT NOT NULL CHECK (
        json_valid(diff_json)
        AND json_type(diff_json) = 'object'
        AND length(CAST(diff_json AS BLOB)) <= 2097152
    ),
    diff_sha256 TEXT NOT NULL CHECK (
        length(diff_sha256) = 64
        AND diff_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    rollback_plan_json TEXT CHECK (
        rollback_plan_json IS NULL
        OR (
            json_valid(rollback_plan_json)
            AND json_type(rollback_plan_json) = 'object'
            AND length(CAST(rollback_plan_json AS BLOB)) <= 1048576
        )
    ),
    plan_sha256 TEXT CHECK (
        plan_sha256 IS NULL
        OR (
            length(plan_sha256) = 64
            AND plan_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    ),
    activated_at TEXT NOT NULL CHECK (
        length(activated_at) BETWEEN 20 AND 35
        AND activated_at = trim(activated_at)
    ),
    CHECK (
        (from_local_revision IS NULL AND from_snapshot_sha256 IS NULL)
        OR (
            from_local_revision > 0
            AND length(from_snapshot_sha256) = 64
            AND from_snapshot_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    ),
    CHECK (
        (
            activation_kind = 'import'
            AND source_envelope_id IS NOT NULL
            AND signed_catalog_revision > 0
            AND length(signing_key_id) BETWEEN 1 AND 64
            AND rollback_plan_json IS NULL
            AND plan_sha256 IS NULL
        )
        OR (
            activation_kind = 'rollback'
            AND source_envelope_id IS NULL
            AND signed_catalog_revision IS NULL
            AND signing_key_id IS NULL
            AND rollback_plan_json IS NOT NULL
            AND plan_sha256 IS NOT NULL
        )
    ),
    FOREIGN KEY (from_local_revision, from_snapshot_sha256)
        REFERENCES provider_catalog_snapshots(
            local_revision,
            snapshot_sha256
        )
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    FOREIGN KEY (to_local_revision, to_snapshot_sha256)
        REFERENCES provider_catalog_snapshots(
            local_revision,
            snapshot_sha256
        )
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    FOREIGN KEY (source_envelope_id, signed_catalog_revision)
        REFERENCES provider_catalog_signed_envelopes(id, catalog_revision)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

CREATE INDEX provider_catalog_activation_audit_target
    ON provider_catalog_activation_audit(
        to_local_revision,
        state_version DESC
    );

CREATE TRIGGER provider_catalog_activation_audit_no_update
BEFORE UPDATE ON provider_catalog_activation_audit
BEGIN
    SELECT RAISE(ABORT, 'provider catalog activation audit is immutable');
END;

CREATE TRIGGER provider_catalog_activation_audit_no_delete
BEFORE DELETE ON provider_catalog_activation_audit
BEGIN
    SELECT RAISE(ABORT, 'provider catalog activation audit is immutable');
END;

CREATE TRIGGER provider_catalog_snapshot_envelopes_history_sealed
BEFORE INSERT ON provider_catalog_snapshot_envelopes
WHEN
    EXISTS (
        SELECT 1
        FROM provider_catalog_state
        WHERE active_local_revision = NEW.local_revision
    )
    OR EXISTS (
        SELECT 1
        FROM provider_catalog_activation_audit
        WHERE from_local_revision = NEW.local_revision
           OR to_local_revision = NEW.local_revision
    )
BEGIN
    SELECT RAISE(ABORT, 'activated provider catalog snapshot chain is sealed');
END;
