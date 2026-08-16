-- Durable, secret-free ownership evidence for native provider credential slots.
-- Native credential stores cannot participate in a SQLite transaction, so an
-- immutable preflight attestation and exact plan precede every native effect.
CREATE TABLE provider_credential_operations (
    id TEXT NOT NULL PRIMARY KEY CHECK (length(trim(id)) BETWEEN 1 AND 256),
    connection_id TEXT NOT NULL
        REFERENCES provider_connections(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    credential_ref TEXT NOT NULL CHECK (
        length(trim(credential_ref)) BETWEEN 1 AND 256
        AND credential_ref = connection_id
    ),
    operation_sequence INTEGER NOT NULL CHECK (operation_sequence > 0),
    operation_kind TEXT NOT NULL CHECK (
        operation_kind IN ('install', 'remove_credential', 'remove_for_archive')
    ),
    connection_binding_sha256 TEXT NOT NULL CHECK (
        length(connection_binding_sha256) = 64
        AND connection_binding_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    credential_authority_id TEXT CHECK (
        credential_authority_id IS NULL
        OR length(trim(credential_authority_id)) BETWEEN 1 AND 256
    ),
    credential_authority_binding_sha256 TEXT CHECK (
        credential_authority_binding_sha256 IS NULL
        OR (
            length(credential_authority_binding_sha256) = 64
            AND credential_authority_binding_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    ),
    predecessor_authority_id TEXT CHECK (
        predecessor_authority_id IS NULL
        OR length(trim(predecessor_authority_id)) BETWEEN 1 AND 256
    ),
    predecessor_authority_binding_sha256 TEXT CHECK (
        predecessor_authority_binding_sha256 IS NULL
        OR (
            length(predecessor_authority_binding_sha256) = 64
            AND predecessor_authority_binding_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    ),
    plan_json TEXT NOT NULL CHECK (
        json_valid(plan_json)
        AND json_type(plan_json) = 'object'
        AND length(CAST(plan_json AS BLOB)) BETWEEN 2 AND 16384
    ),
    plan_sha256 TEXT NOT NULL CHECK (
        length(plan_sha256) = 64
        AND plan_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    preflight_status TEXT NOT NULL CHECK (
        preflight_status IN ('missing', 'available', 'unreadable')
        AND (
            (operation_kind = 'install' AND preflight_status = 'missing')
            OR operation_kind IN ('remove_credential', 'remove_for_archive')
        )
    ),
    preflight_evidence_sha256 TEXT NOT NULL CHECK (
        length(preflight_evidence_sha256) = 64
        AND preflight_evidence_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    preflight_attested_at TEXT NOT NULL CHECK (
        length(trim(preflight_attested_at)) > 0
    ),
    native_owner TEXT NOT NULL CHECK (native_owner = 'native_platform'),
    status TEXT NOT NULL CHECK (
        status IN (
            'prepared', 'started', 'succeeded', 'no_effect',
            'cleanup_required', 'outcome_unknown'
        )
    ),
    outcome_code TEXT CHECK (
        outcome_code IS NULL
        OR outcome_code IN (
            'native_effect_confirmed', 'native_effect_absent',
            'native_status_unreadable', 'native_durability_unknown',
            'native_predecessor_durability_unknown', 'connection_changed',
            'archive_commit_failed'
        )
    ),
    outcome_attestation_sequence INTEGER CHECK (
        outcome_attestation_sequence IS NULL OR outcome_attestation_sequence > 0
    ),
    schema_version INTEGER NOT NULL CHECK (schema_version = 1),
    redaction_version INTEGER NOT NULL CHECK (redaction_version = 1),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    started_at TEXT,
    finished_at TEXT,
    updated_at TEXT NOT NULL CHECK (length(trim(updated_at)) > 0),
    UNIQUE (credential_ref, operation_sequence),
    CHECK (
        json_extract(plan_json, '$.schema_version') = schema_version
        AND json_extract(plan_json, '$.redaction_version') = redaction_version
        AND json_extract(plan_json, '$.operation_id') = id
        AND json_extract(plan_json, '$.operation_sequence') = operation_sequence
        AND json_extract(plan_json, '$.operation_kind') = operation_kind
        AND json_extract(plan_json, '$.connection_id') = connection_id
        AND json_extract(plan_json, '$.credential_ref') = credential_ref
        AND json_extract(plan_json, '$.connection_binding_sha256') = connection_binding_sha256
        AND json_extract(plan_json, '$.credential_authority_id')
            IS credential_authority_id
        AND json_extract(plan_json, '$.credential_authority_binding_sha256')
            IS credential_authority_binding_sha256
        AND json_extract(plan_json, '$.predecessor_authority_id')
            IS predecessor_authority_id
        AND json_extract(plan_json, '$.predecessor_authority_binding_sha256')
            IS predecessor_authority_binding_sha256
    ),
    CHECK (
        (credential_authority_id IS NULL
          AND credential_authority_binding_sha256 IS NULL)
        OR (credential_authority_id IS NOT NULL
          AND credential_authority_binding_sha256 IS NOT NULL)
    ),
    CHECK (
        (predecessor_authority_id IS NULL
          AND predecessor_authority_binding_sha256 IS NULL)
        OR (predecessor_authority_id IS NOT NULL
          AND predecessor_authority_binding_sha256 IS NOT NULL)
    ),
    CHECK (
        operation_kind <> 'install'
        OR (
          credential_authority_id = id
          AND credential_authority_binding_sha256 = connection_binding_sha256
        )
    ),
    CHECK (
        (status = 'prepared'
            AND started_at IS NULL AND finished_at IS NULL
            AND outcome_code IS NULL AND outcome_attestation_sequence IS NULL)
        OR (status = 'started'
            AND started_at IS NOT NULL AND finished_at IS NULL
            AND outcome_code IS NULL AND outcome_attestation_sequence IS NULL)
        OR (status IN ('succeeded', 'no_effect', 'cleanup_required', 'outcome_unknown')
            AND finished_at IS NOT NULL AND outcome_code IS NOT NULL)
    )
);

-- One unresolved operation owns a slot at a time. Unknown and cleanup states
-- deliberately retain the reservation so a later connection cannot adopt it.
CREATE UNIQUE INDEX provider_credential_operations_unresolved_slot
    ON provider_credential_operations(credential_ref)
    WHERE status IN ('prepared', 'started', 'cleanup_required', 'outcome_unknown');

CREATE INDEX provider_credential_operations_recovery
    ON provider_credential_operations(status, created_at, id);

CREATE INDEX provider_credential_operations_connection_history
    ON provider_credential_operations(connection_id, operation_sequence);

CREATE TABLE provider_credential_operation_attestations (
    operation_id TEXT NOT NULL
        REFERENCES provider_credential_operations(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    sequence INTEGER NOT NULL CHECK (sequence > 0),
    stage TEXT NOT NULL CHECK (
        stage IN (
            'postflight', 'recovery',
            'cleanup_remove_intent', 'cleanup_archive_intent',
            'durability_repair',
            'operation_durability_required',
            'operation_durability_repaired',
            'predecessor_durability_required',
            'predecessor_durability_repaired',
            'predecessor_delete_intent', 'predecessor_missing'
        )
    ),
    slot_status TEXT NOT NULL CHECK (
        slot_status IN ('missing', 'available', 'unreadable')
    ),
    evidence_sha256 TEXT NOT NULL CHECK (
        length(evidence_sha256) = 64
        AND evidence_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    native_owner TEXT NOT NULL CHECK (native_owner = 'native_platform'),
    schema_version INTEGER NOT NULL CHECK (schema_version = 1),
    redaction_version INTEGER NOT NULL CHECK (redaction_version = 1),
    attested_at TEXT NOT NULL CHECK (length(trim(attested_at)) > 0),
    PRIMARY KEY (operation_id, sequence)
);

CREATE TRIGGER provider_credential_cleanup_intent_insert_guard
BEFORE INSERT ON provider_credential_operation_attestations
WHEN NEW.stage IN ('cleanup_remove_intent', 'cleanup_archive_intent')
  AND (
    NEW.sequence <> (
      SELECT COALESCE(MAX(existing.sequence), 0) + 1
      FROM provider_credential_operation_attestations AS existing
      WHERE existing.operation_id = NEW.operation_id
    )
    OR NOT EXISTS (
      SELECT 1
      FROM provider_credential_operations AS operation
      WHERE operation.id = NEW.operation_id
        AND operation.status IN ('started', 'cleanup_required', 'outcome_unknown')
    )
  )
BEGIN
    SELECT RAISE(ABORT, 'credential cleanup intent is detached from unresolved work');
END;

CREATE TRIGGER provider_credential_durability_attestation_insert_guard
BEFORE INSERT ON provider_credential_operation_attestations
WHEN NEW.stage IN (
    'operation_durability_required', 'operation_durability_repaired',
    'predecessor_durability_required', 'predecessor_durability_repaired'
  )
  AND (
    NEW.sequence <> (
      SELECT COALESCE(MAX(existing.sequence), 0) + 1
      FROM provider_credential_operation_attestations AS existing
      WHERE existing.operation_id = NEW.operation_id
    )
    OR NOT EXISTS (
      SELECT 1 FROM provider_credential_operations AS operation
      WHERE operation.id = NEW.operation_id
        AND operation.status IN ('started', 'cleanup_required', 'outcome_unknown')
        AND (
          NEW.stage NOT LIKE 'predecessor_%'
          OR (
            operation.operation_kind = 'install'
            AND operation.predecessor_authority_id IS NOT NULL
          )
        )
    )
    OR (NEW.stage LIKE '%_required' AND NEW.slot_status <> 'unreadable')
    OR (NEW.stage LIKE '%_repaired' AND NEW.slot_status <> 'missing')
    OR (
      NEW.stage LIKE '%_repaired'
      AND NOT EXISTS (
        SELECT 1 FROM provider_credential_operation_attestations AS required
        WHERE required.operation_id = NEW.operation_id
          AND required.stage = replace(NEW.stage, '_repaired', '_required')
          AND required.sequence > COALESCE((
            SELECT MAX(repaired.sequence)
            FROM provider_credential_operation_attestations AS repaired
            WHERE repaired.operation_id = NEW.operation_id
              AND repaired.stage = NEW.stage
          ), 0)
      )
    )
  )
BEGIN
    SELECT RAISE(ABORT, 'credential durability evidence is detached or out of order');
END;

CREATE TRIGGER provider_credential_predecessor_attestation_insert_guard
BEFORE INSERT ON provider_credential_operation_attestations
WHEN NEW.stage IN ('predecessor_delete_intent', 'predecessor_missing')
  AND (
    NEW.sequence <> (
      SELECT COALESCE(MAX(existing.sequence), 0) + 1
      FROM provider_credential_operation_attestations AS existing
      WHERE existing.operation_id = NEW.operation_id
    )
    OR NOT EXISTS (
      SELECT 1
      FROM provider_credential_operations AS operation
      WHERE operation.id = NEW.operation_id
        AND operation.operation_kind = 'install'
        AND operation.predecessor_authority_id IS NOT NULL
        AND operation.status IN ('started', 'cleanup_required', 'outcome_unknown')
    )
    OR EXISTS (
      SELECT 1
      FROM provider_credential_operation_attestations AS existing_stage
      WHERE existing_stage.operation_id = NEW.operation_id
        AND existing_stage.stage = NEW.stage
    )
    OR (NEW.stage = 'predecessor_missing'
      AND (
        NEW.slot_status <> 'missing'
        OR NOT EXISTS (
          SELECT 1
          FROM provider_credential_operation_attestations AS intent
          WHERE intent.operation_id = NEW.operation_id
            AND intent.stage = 'predecessor_delete_intent'
        )
      ))
  )
BEGIN
    SELECT RAISE(ABORT, 'credential predecessor evidence is detached from replacement work');
END;

-- Current ownership is a projection over append-only ordinary/discovery
-- authority. Existing schema-36 bindings are explicitly pending rather than
-- adopted: a native Available value alone cannot prove who installed it.
CREATE TABLE provider_credential_ownership (
    connection_id TEXT NOT NULL PRIMARY KEY
        REFERENCES provider_connections(id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE,
    credential_ref TEXT NOT NULL UNIQUE CHECK (
        length(trim(credential_ref)) BETWEEN 1 AND 256
        AND credential_ref = connection_id
    ),
    ownership_state TEXT NOT NULL CHECK (
        ownership_state IN (
            'legacy_pending', 'unowned', 'ordinary_owned',
            'discovery_owned', 'removed'
        )
    ),
    connection_binding_sha256 TEXT CHECK (
        connection_binding_sha256 IS NULL
        OR (
            length(connection_binding_sha256) = 64
            AND connection_binding_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    ),
    authority_id TEXT,
    authority_sequence INTEGER NOT NULL DEFAULT 0 CHECK (authority_sequence >= 0),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    updated_at TEXT NOT NULL CHECK (length(trim(updated_at)) > 0),
    CHECK (
        (ownership_state = 'unowned' AND authority_id IS NULL)
        OR (ownership_state <> 'unowned' AND length(trim(authority_id)) > 0)
    ),
    CHECK (
        (ownership_state IN ('ordinary_owned', 'discovery_owned')
          AND connection_binding_sha256 IS NOT NULL
          AND authority_sequence > 0)
        OR (ownership_state NOT IN ('ordinary_owned', 'discovery_owned')
          AND connection_binding_sha256 IS NULL
          AND (
            ownership_state = 'removed'
            OR authority_sequence = 0
          ))
    )
);

INSERT INTO provider_credential_ownership
    (connection_id, credential_ref, ownership_state, connection_binding_sha256,
     authority_id, authority_sequence, created_at, updated_at)
SELECT id, credential_ref, 'legacy_pending', NULL, 'schema-36-cutover', 0,
       created_at, created_at
FROM provider_connections
WHERE archived_at IS NULL
  AND credential_ref IS NOT NULL
  AND credential_scope_json IS NOT NULL;

CREATE TRIGGER provider_credential_ownership_new_binding
AFTER INSERT ON provider_connections
WHEN NEW.credential_ref IS NOT NULL AND NEW.credential_scope_json IS NOT NULL
BEGIN
    INSERT INTO provider_credential_ownership
        (connection_id, credential_ref, ownership_state, connection_binding_sha256,
         authority_id, authority_sequence, created_at, updated_at)
    VALUES (
        NEW.id, NEW.credential_ref, 'unowned', NULL, NULL, 0, NEW.created_at, NEW.created_at
    );
END;

CREATE TRIGGER provider_credential_ownership_initial_state_guard
BEFORE INSERT ON provider_credential_ownership
WHEN NOT (
    (NEW.ownership_state = 'unowned'
      AND NEW.authority_id IS NULL
      AND NEW.authority_sequence = 0
      AND NEW.connection_binding_sha256 IS NULL)
    OR (NEW.ownership_state = 'legacy_pending'
      AND NEW.authority_id = 'schema-36-cutover'
      AND NEW.authority_sequence = 0
      AND NEW.connection_binding_sha256 IS NULL)
)
BEGIN
    SELECT RAISE(ABORT, 'provider credential ownership cannot be inserted as owned');
END;

CREATE TRIGGER provider_credential_ownership_no_replace
BEFORE INSERT ON provider_credential_ownership
WHEN EXISTS (
    SELECT 1 FROM provider_credential_ownership AS existing
    WHERE existing.connection_id = NEW.connection_id
       OR existing.credential_ref = NEW.credential_ref
)
BEGIN
    SELECT RAISE(ABORT, 'provider credential ownership cannot replace existing authority');
END;

CREATE TRIGGER provider_credential_ownership_no_detached_delete
BEFORE DELETE ON provider_credential_ownership
WHEN EXISTS (
    SELECT 1 FROM provider_connections WHERE id = OLD.connection_id
)
BEGIN
    SELECT RAISE(ABORT, 'provider credential ownership cannot be detached from its connection');
END;

CREATE TRIGGER provider_credential_ownership_identity_guard
BEFORE UPDATE ON provider_credential_ownership
WHEN NEW.connection_id <> OLD.connection_id
  OR NEW.credential_ref <> OLD.credential_ref
  OR NEW.created_at <> OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'provider credential ownership identity is immutable');
END;

-- Every ownership projection change is backed by one append-only event. The
-- per-connection sequence is shared by discovery and ordinary operations, so
-- an older authority can never be made current again after a later event.
--
-- Version 27 attestations did not identify a physical native execution. Seal
-- the exact rows which existed at the version-37 migration cutpoint before
-- accepting any version-37 write. A row in this snapshot is historical
-- evidence only: it proves that an otherwise valid semantic attestation was
-- present before physical execution authority existed, and therefore cannot
-- authorize recovery or ownership on its own.
CREATE TABLE provider_discovery_native_no_effect_legacy_cutoff_snapshots (
    operation_id TEXT NOT NULL PRIMARY KEY
        REFERENCES provider_discovery_native_no_effect_attestations(operation_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    session_id TEXT NOT NULL
        REFERENCES provider_discovery_sessions(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    commit_attempt_id TEXT NOT NULL
        REFERENCES provider_discovery_commit_attempts(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    commit_plan_sha256 TEXT NOT NULL CHECK (
        length(commit_plan_sha256) = 64
        AND commit_plan_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    connection_id TEXT NOT NULL CHECK (length(trim(connection_id)) > 0),
    attestation_kind TEXT NOT NULL CHECK (
        attestation_kind = 'credential_slot_missing'
    ),
    evidence_sha256 TEXT NOT NULL CHECK (
        length(evidence_sha256) = 64
        AND evidence_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    recovery_owner TEXT NOT NULL CHECK (recovery_owner = 'native_platform'),
    attestation_schema_version INTEGER NOT NULL CHECK (
        attestation_schema_version = 1
    ),
    attestation_redaction_version INTEGER NOT NULL CHECK (
        attestation_redaction_version = 1
    ),
    attested_at TEXT NOT NULL CHECK (length(trim(attested_at)) > 0),
    cutoff_before_schema_version INTEGER NOT NULL CHECK (
        cutoff_before_schema_version = 37
    ),
    snapshot_schema_version INTEGER NOT NULL CHECK (snapshot_schema_version = 1),
    UNIQUE (session_id, commit_attempt_id, operation_id),
    FOREIGN KEY (session_id, commit_attempt_id, operation_id)
        REFERENCES provider_discovery_native_no_effect_attestations(
            session_id, commit_attempt_id, operation_id
        )
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

INSERT INTO provider_discovery_native_no_effect_legacy_cutoff_snapshots (
    operation_id,
    session_id,
    commit_attempt_id,
    commit_plan_sha256,
    connection_id,
    attestation_kind,
    evidence_sha256,
    recovery_owner,
    attestation_schema_version,
    attestation_redaction_version,
    attested_at,
    cutoff_before_schema_version,
    snapshot_schema_version
)
SELECT operation_id,
       session_id,
       commit_attempt_id,
       commit_plan_sha256,
       connection_id,
       attestation_kind,
       evidence_sha256,
       recovery_owner,
       schema_version,
       redaction_version,
       attested_at,
       37,
       1
FROM provider_discovery_native_no_effect_attestations;

-- The preceding INSERT is the only legal population step. These triggers are
-- created immediately afterwards so neither ordinary INSERT nor
-- INSERT OR REPLACE can relabel a post-cutoff attestation as legacy.
CREATE TRIGGER provider_discovery_native_no_effect_legacy_cutoff_no_insert
BEFORE INSERT ON provider_discovery_native_no_effect_legacy_cutoff_snapshots
BEGIN
    SELECT RAISE(ABORT, 'legacy native no-effect cutoff is sealed');
END;

CREATE TRIGGER provider_discovery_native_no_effect_legacy_cutoff_no_update
BEFORE UPDATE ON provider_discovery_native_no_effect_legacy_cutoff_snapshots
BEGIN
    SELECT RAISE(ABORT, 'legacy native no-effect cutoff is immutable');
END;

CREATE TRIGGER provider_discovery_native_no_effect_legacy_cutoff_no_delete
BEFORE DELETE ON provider_discovery_native_no_effect_legacy_cutoff_snapshots
BEGIN
    SELECT RAISE(ABORT, 'legacy native no-effect cutoff is immutable');
END;

--
-- Reserve a physical discovery authority while the semantic operation is
-- still Prepared. A second append-only row records the exact cutpoint at which
-- every fallible native precondition has completed and the store call is
-- about to begin. Neither table records or derives credential material.
CREATE TABLE provider_discovery_native_credential_executions (
    physical_authority_id TEXT NOT NULL PRIMARY KEY CHECK (
        length(physical_authority_id) = 53
        AND substr(physical_authority_id, 1, 17) = 'discovery-native-'
        AND substr(physical_authority_id, 26, 1) = '-'
        AND substr(physical_authority_id, 31, 1) = '-'
        AND substr(physical_authority_id, 36, 1) = '-'
        AND substr(physical_authority_id, 41, 1) = '-'
        AND substr(physical_authority_id, 32, 1) = '4'
        AND substr(physical_authority_id, 37, 1) GLOB '[89ab]'
        AND length(replace(substr(physical_authority_id, 18), '-', '')) = 32
        AND replace(substr(physical_authority_id, 18), '-', '')
            NOT GLOB '*[^0-9a-f]*'
    ),
    operation_id TEXT NOT NULL UNIQUE
        REFERENCES provider_discovery_operations(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    session_id TEXT NOT NULL
        REFERENCES provider_discovery_sessions(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    commit_attempt_id TEXT NOT NULL
        REFERENCES provider_discovery_commit_attempts(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    commit_plan_sha256 TEXT NOT NULL CHECK (
        length(commit_plan_sha256) = 64
        AND commit_plan_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    connection_id TEXT NOT NULL CHECK (
        length(trim(connection_id)) BETWEEN 1 AND 128
    ),
    connection_binding_sha256 TEXT NOT NULL CHECK (
        length(connection_binding_sha256) = 64
        AND connection_binding_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    reserved_at TEXT NOT NULL CHECK (
        length(trim(reserved_at)) > 0
        AND julianday(reserved_at) IS NOT NULL
    ),
    schema_version INTEGER NOT NULL CHECK (schema_version = 1),
    redaction_version INTEGER NOT NULL CHECK (redaction_version = 1),
    UNIQUE (operation_id, session_id, commit_attempt_id),
    UNIQUE (operation_id, physical_authority_id),
    UNIQUE (physical_authority_id, connection_id, connection_binding_sha256)
);

CREATE TABLE provider_discovery_native_credential_store_attempts (
    operation_id TEXT NOT NULL,
    physical_authority_id TEXT NOT NULL,
    started_at TEXT NOT NULL CHECK (
        length(trim(started_at)) > 0
        AND julianday(started_at) IS NOT NULL
    ),
    schema_version INTEGER NOT NULL CHECK (schema_version = 1),
    redaction_version INTEGER NOT NULL CHECK (redaction_version = 1),
    PRIMARY KEY (operation_id),
    UNIQUE (physical_authority_id),
    FOREIGN KEY (operation_id, physical_authority_id)
        REFERENCES provider_discovery_native_credential_executions(
            operation_id, physical_authority_id
        )
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

-- A reserved authority which is interrupted before the native store cutpoint
-- must never disappear into an ambiguous Prepared history. The row is
-- captured automatically by the terminal transition and repeats the exact
-- secret-free reservation identity so recovery can prove both what was
-- abandoned and that no native store attempt was ever authorized for it.
CREATE TABLE provider_discovery_native_credential_abandoned_reservations (
    operation_id TEXT NOT NULL PRIMARY KEY,
    physical_authority_id TEXT NOT NULL UNIQUE,
    session_id TEXT NOT NULL,
    commit_attempt_id TEXT NOT NULL,
    commit_plan_sha256 TEXT NOT NULL CHECK (
        length(commit_plan_sha256) = 64
        AND commit_plan_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    connection_id TEXT NOT NULL CHECK (
        length(trim(connection_id)) BETWEEN 1 AND 128
    ),
    connection_binding_sha256 TEXT NOT NULL CHECK (
        length(connection_binding_sha256) = 64
        AND connection_binding_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    reserved_at TEXT NOT NULL CHECK (
        length(trim(reserved_at)) > 0
        AND julianday(reserved_at) IS NOT NULL
    ),
    abandonment_kind TEXT NOT NULL CHECK (
        abandonment_kind = 'prepared_interrupted_before_native_store'
    ),
    abandoned_at TEXT NOT NULL CHECK (
        length(trim(abandoned_at)) > 0
        AND julianday(abandoned_at) IS NOT NULL
        AND julianday(abandoned_at) >= julianday(reserved_at)
    ),
    schema_version INTEGER NOT NULL CHECK (schema_version = 1),
    redaction_version INTEGER NOT NULL CHECK (redaction_version = 1),
    FOREIGN KEY (operation_id, physical_authority_id)
        REFERENCES provider_discovery_native_credential_executions(
            operation_id, physical_authority_id
        )
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    FOREIGN KEY (operation_id, session_id, commit_attempt_id)
        REFERENCES provider_discovery_native_credential_executions(
            operation_id, session_id, commit_attempt_id
        )
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    FOREIGN KEY (
        physical_authority_id, connection_id, connection_binding_sha256
    )
        REFERENCES provider_discovery_native_credential_executions(
            physical_authority_id, connection_id, connection_binding_sha256
        )
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

-- Schema 27's semantic attestation did not name a physical authority. This
-- append-only companion binds every schema-37 missing observation to the
-- exact reserved execution and store-attempt cutpoint. Legacy attestations
-- without this row remain historical but cannot authorize recovery.
CREATE TABLE provider_discovery_native_no_effect_execution_bindings (
    operation_id TEXT NOT NULL PRIMARY KEY,
    physical_authority_id TEXT NOT NULL UNIQUE,
    session_id TEXT NOT NULL,
    commit_attempt_id TEXT NOT NULL,
    commit_plan_sha256 TEXT NOT NULL CHECK (
        length(commit_plan_sha256) = 64
        AND commit_plan_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    connection_id TEXT NOT NULL CHECK (
        length(trim(connection_id)) BETWEEN 1 AND 128
    ),
    connection_binding_sha256 TEXT NOT NULL CHECK (
        length(connection_binding_sha256) = 64
        AND connection_binding_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    attestation_evidence_sha256 TEXT NOT NULL CHECK (
        length(attestation_evidence_sha256) = 64
        AND attestation_evidence_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    execution_binding_sha256 TEXT NOT NULL CHECK (
        length(execution_binding_sha256) = 64
        AND execution_binding_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    attested_at TEXT NOT NULL CHECK (
        length(trim(attested_at)) > 0
        AND julianday(attested_at) IS NOT NULL
    ),
    schema_version INTEGER NOT NULL CHECK (schema_version = 1),
    redaction_version INTEGER NOT NULL CHECK (redaction_version = 1),
    FOREIGN KEY (operation_id, physical_authority_id)
        REFERENCES provider_discovery_native_credential_executions(
            operation_id, physical_authority_id
        )
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

CREATE TABLE provider_credential_ownership_events (
    connection_id TEXT NOT NULL
        REFERENCES provider_connections(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    authority_sequence INTEGER NOT NULL CHECK (authority_sequence > 0),
    ownership_state TEXT NOT NULL CHECK (
        ownership_state IN ('ordinary_owned', 'discovery_owned', 'removed')
    ),
    connection_binding_sha256 TEXT CHECK (
        connection_binding_sha256 IS NULL
        OR (
            length(connection_binding_sha256) = 64
            AND connection_binding_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    ),
    authority_id TEXT NOT NULL CHECK (length(trim(authority_id)) BETWEEN 1 AND 256),
    source_kind TEXT NOT NULL CHECK (
        source_kind IN ('ordinary_operation', 'discovery_commit')
    ),
    source_id TEXT NOT NULL CHECK (length(trim(source_id)) BETWEEN 1 AND 256),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    PRIMARY KEY (connection_id, authority_sequence),
    UNIQUE (source_kind, source_id),
    CHECK (
        (ownership_state IN ('ordinary_owned', 'discovery_owned')
          AND connection_binding_sha256 IS NOT NULL)
        OR (ownership_state = 'removed' AND connection_binding_sha256 IS NULL)
    ),
    CHECK (
        (source_kind = 'ordinary_operation' AND authority_id = source_id)
        OR source_kind = 'discovery_commit'
    ),
    CHECK (
        (source_kind = 'ordinary_operation'
          AND ownership_state IN ('ordinary_owned', 'removed'))
        OR (source_kind = 'discovery_commit'
          AND ownership_state = 'discovery_owned')
    )
);

CREATE TRIGGER provider_credential_ownership_event_insert_guard
BEFORE INSERT ON provider_credential_ownership_events
WHEN NEW.authority_sequence <> (
        SELECT COALESCE(MAX(existing.authority_sequence), 0) + 1
        FROM provider_credential_ownership_events AS existing
        WHERE existing.connection_id = NEW.connection_id
     )
  OR CASE NEW.source_kind
    WHEN 'ordinary_operation' THEN NOT EXISTS (
        SELECT 1
        FROM provider_credential_operations AS operation
        WHERE operation.id = NEW.source_id
          AND operation.connection_id = NEW.connection_id
          AND operation.credential_ref = NEW.connection_id
          AND (
            (NEW.ownership_state = 'ordinary_owned'
              AND operation.operation_kind = 'install'
              AND operation.status = 'succeeded'
              AND NEW.connection_binding_sha256 = operation.connection_binding_sha256)
            OR (NEW.ownership_state = 'removed'
              AND NEW.connection_binding_sha256 IS NULL
              AND (
                (operation.operation_kind IN ('remove_credential', 'remove_for_archive')
                  AND operation.status = 'succeeded')
                OR (operation.status = 'no_effect'
                  AND (operation.operation_kind <> 'install'
                    OR operation.predecessor_authority_id IS NULL
                    OR (
                      EXISTS (
                        SELECT 1
                        FROM provider_credential_operation_attestations AS cleanup_intent
                        WHERE cleanup_intent.operation_id = operation.id
                          AND cleanup_intent.stage IN (
                            'cleanup_remove_intent', 'cleanup_archive_intent'
                          )
                      )
                      AND EXISTS (
                        SELECT 1
                        FROM provider_credential_operation_attestations AS predecessor_intent
                        JOIN provider_credential_operation_attestations AS predecessor_missing
                          ON predecessor_missing.operation_id = predecessor_intent.operation_id
                         AND predecessor_missing.sequence > predecessor_intent.sequence
                        WHERE predecessor_intent.operation_id = operation.id
                          AND predecessor_intent.stage = 'predecessor_delete_intent'
                          AND predecessor_missing.stage = 'predecessor_missing'
                          AND predecessor_missing.slot_status = 'missing'
                      )
                    ))
                  AND EXISTS (
                    SELECT 1
                    FROM provider_credential_operation_attestations AS outcome
                    WHERE outcome.operation_id = operation.id
                      AND outcome.sequence = operation.outcome_attestation_sequence
                      AND outcome.slot_status = 'missing'
                  ))
              ))
          )
      )
    WHEN 'discovery_commit' THEN NOT EXISTS (
        SELECT 1
        FROM provider_discovery_operations AS operation
        JOIN provider_discovery_native_credential_executions AS execution
          ON execution.operation_id = operation.id
         AND execution.physical_authority_id = NEW.authority_id
         AND execution.connection_id = NEW.connection_id
         AND execution.connection_binding_sha256 = NEW.connection_binding_sha256
        JOIN provider_discovery_native_credential_store_attempts AS store_attempt
          ON store_attempt.operation_id = execution.operation_id
         AND store_attempt.physical_authority_id = execution.physical_authority_id
        JOIN provider_discovery_sessions AS session
          ON session.id = operation.session_id
        JOIN provider_discovery_commit_attempts AS attempt
          ON attempt.id = session.commit_attempt_id
         AND attempt.session_id = session.id
        JOIN provider_discovery_authorized_native_commit_starts AS authorized
          ON authorized.operation_id = operation.id
         AND authorized.session_id = operation.session_id
         AND authorized.commit_attempt_id = attempt.id
         AND authorized.commit_plan_sha256 = attempt.plan_sha256
         AND authorized.operation_expected_revision = operation.expected_revision
        WHERE operation.id = NEW.source_id
          AND operation.operation_kind = 'atomic_commit'
          AND operation.side_effect_class = 'persistent'
          AND operation.finished_at IS NOT NULL
          AND attempt.phase = 'completed'
          AND attempt.completed_at IS NOT NULL
          AND session.state = 'ready'
          AND session.commit_attempt_id = attempt.id
          AND session.commit_plan_sha256 = attempt.plan_sha256
          AND session.committed_connection_id = NEW.connection_id
          AND json_valid(attempt.plan_json)
          AND json_extract(attempt.plan_json, '$.attempt_id') = attempt.id
          AND json_extract(attempt.plan_json, '$.session_id') = session.id
          AND json_extract(attempt.plan_json, '$.connection_id') = NEW.connection_id
          AND json_extract(attempt.plan_json, '$.credential_ref') = NEW.connection_id
          AND (
            (operation.status = 'succeeded'
              AND operation.expected_revision + 1 = session.revision
              AND operation.finished_at = attempt.completed_at)
            OR (operation.status = 'outcome_unknown'
              AND operation.expected_revision + 2 = session.revision
              AND EXISTS (
                SELECT 1
                FROM provider_discovery_authorized_confirmed_commit_completions AS confirmed
                WHERE confirmed.operation_id = operation.id
                  AND confirmed.session_id = operation.session_id
                  AND confirmed.commit_attempt_id = attempt.id
                  AND confirmed.commit_plan_sha256 = attempt.plan_sha256
                  AND confirmed.connection_id = NEW.connection_id
                  AND confirmed.ready_revision = session.revision
                  AND confirmed.completed_at = attempt.completed_at
              ))
          )
      )
    ELSE 1
  END
BEGIN
    SELECT RAISE(ABORT, 'provider credential ownership event lacks durable authority');
END;

CREATE TRIGGER provider_credential_ownership_event_no_replace
BEFORE INSERT ON provider_credential_ownership_events
WHEN EXISTS (
    SELECT 1 FROM provider_credential_ownership_events AS existing
    WHERE (existing.connection_id = NEW.connection_id
       AND existing.authority_sequence = NEW.authority_sequence)
       OR (existing.source_kind = NEW.source_kind AND existing.source_id = NEW.source_id)
)
BEGIN
    SELECT RAISE(ABORT, 'provider credential ownership event cannot replace history');
END;

CREATE TRIGGER provider_credential_ownership_event_no_update
BEFORE UPDATE ON provider_credential_ownership_events
BEGIN
    SELECT RAISE(ABORT, 'provider credential ownership events are immutable');
END;

CREATE TRIGGER provider_credential_ownership_event_no_delete
BEFORE DELETE ON provider_credential_ownership_events
BEGIN
    SELECT RAISE(ABORT, 'provider credential ownership events are append-only');
END;

-- Superseded authority-derived slots are known exactly from ownership history.
-- They are deleted through this bounded, idempotent journal; the logical raw
-- legacy slot is never enumerated or used as a fallback.
CREATE TABLE provider_credential_slot_gc (
    connection_id TEXT NOT NULL,
    authority_sequence INTEGER NOT NULL CHECK (authority_sequence > 0),
    authority_id TEXT NOT NULL CHECK (length(trim(authority_id)) BETWEEN 1 AND 256),
    connection_binding_sha256 TEXT NOT NULL CHECK (
        length(connection_binding_sha256) = 64
        AND connection_binding_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    status TEXT NOT NULL CHECK (status IN ('pending', 'started', 'completed')),
    preflight_status TEXT CHECK (
        preflight_status IS NULL
        OR preflight_status IN ('missing', 'available', 'unreadable')
    ),
    last_observed_status TEXT CHECK (
        last_observed_status IS NULL
        OR last_observed_status IN ('missing', 'available', 'unreadable')
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    delete_started_at TEXT,
    completed_at TEXT,
    updated_at TEXT NOT NULL CHECK (length(trim(updated_at)) > 0),
    PRIMARY KEY (connection_id, authority_sequence),
    FOREIGN KEY (connection_id, authority_sequence)
        REFERENCES provider_credential_ownership_events(connection_id, authority_sequence)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    CHECK (
        (status = 'pending'
          AND preflight_status IS NULL
          AND last_observed_status IS NULL
          AND delete_started_at IS NULL
          AND completed_at IS NULL)
        OR (status = 'started'
          AND preflight_status IN ('available', 'unreadable')
          AND last_observed_status IN ('available', 'unreadable')
          AND delete_started_at IS NOT NULL
          AND completed_at IS NULL)
        OR (status = 'completed'
          AND preflight_status IS NOT NULL
          AND last_observed_status = 'missing'
          AND completed_at IS NOT NULL)
    )
);

CREATE TRIGGER provider_credential_slot_gc_enqueue_superseded
AFTER INSERT ON provider_credential_ownership_events
BEGIN
    INSERT INTO provider_credential_slot_gc
        (connection_id, authority_sequence, authority_id,
         connection_binding_sha256, status, preflight_status,
         last_observed_status, created_at, delete_started_at,
         completed_at, updated_at)
    SELECT prior.connection_id, prior.authority_sequence, prior.authority_id,
           prior.connection_binding_sha256, 'pending', NULL, NULL,
           NEW.created_at, NULL, NULL, NEW.created_at
    FROM provider_credential_ownership_events AS prior
    WHERE prior.connection_id = NEW.connection_id
      AND prior.authority_sequence < NEW.authority_sequence
      AND prior.ownership_state IN ('ordinary_owned', 'discovery_owned')
      AND NOT EXISTS (
        SELECT 1 FROM provider_credential_slot_gc AS existing
        WHERE existing.connection_id = prior.connection_id
          AND existing.authority_sequence = prior.authority_sequence
      );
END;

CREATE TRIGGER provider_credential_slot_gc_insert_guard
BEFORE INSERT ON provider_credential_slot_gc
WHEN NEW.status <> 'pending'
  OR NEW.preflight_status IS NOT NULL
  OR NEW.last_observed_status IS NOT NULL
  OR NEW.delete_started_at IS NOT NULL
  OR NEW.completed_at IS NOT NULL
  OR NOT EXISTS (
    SELECT 1
    FROM provider_credential_ownership_events AS event
    WHERE event.connection_id = NEW.connection_id
      AND event.authority_sequence = NEW.authority_sequence
      AND event.ownership_state IN ('ordinary_owned', 'discovery_owned')
      AND event.authority_id = NEW.authority_id
      AND event.connection_binding_sha256 = NEW.connection_binding_sha256
      AND event.authority_sequence < (
        SELECT MAX(latest.authority_sequence)
        FROM provider_credential_ownership_events AS latest
        WHERE latest.connection_id = NEW.connection_id
      )
  )
BEGIN
    SELECT RAISE(ABORT, 'provider credential slot gc target is not superseded');
END;

CREATE TRIGGER provider_credential_slot_gc_no_replace
BEFORE INSERT ON provider_credential_slot_gc
WHEN EXISTS (
    SELECT 1 FROM provider_credential_slot_gc AS existing
    WHERE existing.connection_id = NEW.connection_id
      AND existing.authority_sequence = NEW.authority_sequence
)
BEGIN
    SELECT RAISE(ABORT, 'provider credential slot gc cannot replace history');
END;

CREATE TRIGGER provider_credential_slot_gc_identity_guard
BEFORE UPDATE ON provider_credential_slot_gc
WHEN NEW.connection_id <> OLD.connection_id
  OR NEW.authority_sequence <> OLD.authority_sequence
  OR NEW.authority_id <> OLD.authority_id
  OR NEW.connection_binding_sha256 <> OLD.connection_binding_sha256
  OR NEW.created_at <> OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'provider credential slot gc identity is immutable');
END;

CREATE TRIGGER provider_credential_slot_gc_legal_transition
BEFORE UPDATE OF status, preflight_status, last_observed_status,
                 delete_started_at, completed_at, updated_at
ON provider_credential_slot_gc
WHEN NOT (
    (OLD.status = 'pending' AND NEW.status = 'started'
      AND NEW.preflight_status IN ('available', 'unreadable')
      AND NEW.last_observed_status = NEW.preflight_status
      AND NEW.delete_started_at IS NOT NULL
      AND NEW.completed_at IS NULL)
    OR (OLD.status = 'pending' AND NEW.status = 'completed'
      AND NEW.preflight_status = 'missing'
      AND NEW.last_observed_status = 'missing'
      AND NEW.delete_started_at IS NULL
      AND NEW.completed_at IS NOT NULL)
    OR (OLD.status = 'started' AND NEW.status = 'started'
      AND NEW.preflight_status = OLD.preflight_status
      AND NEW.last_observed_status IN ('available', 'unreadable')
      AND NEW.delete_started_at = OLD.delete_started_at
      AND NEW.completed_at IS NULL)
    OR (OLD.status = 'started' AND NEW.status = 'completed'
      AND NEW.preflight_status = OLD.preflight_status
      AND NEW.last_observed_status = 'missing'
      AND NEW.delete_started_at = OLD.delete_started_at
      AND NEW.completed_at IS NOT NULL)
)
BEGIN
    SELECT RAISE(ABORT, 'illegal provider credential slot gc transition');
END;

CREATE TRIGGER provider_credential_slot_gc_no_delete
BEFORE DELETE ON provider_credential_slot_gc
BEGIN
    SELECT RAISE(ABORT, 'provider credential slot gc is append-only');
END;

CREATE TRIGGER provider_credential_ownership_authority_guard
BEFORE UPDATE OF ownership_state, connection_binding_sha256, authority_id, authority_sequence
ON provider_credential_ownership
WHEN NOT (
    EXISTS (
      SELECT 1
      FROM provider_credential_ownership_events AS event
      WHERE event.connection_id = OLD.connection_id
        AND event.authority_sequence = NEW.authority_sequence
        AND event.authority_sequence = (
          SELECT MAX(latest.authority_sequence)
          FROM provider_credential_ownership_events AS latest
          WHERE latest.connection_id = OLD.connection_id
        )
        AND event.ownership_state = NEW.ownership_state
        AND event.connection_binding_sha256 IS NEW.connection_binding_sha256
        AND event.authority_id = NEW.authority_id
    )
)
BEGIN
    SELECT RAISE(ABORT, 'provider credential ownership lacks durable authority');
END;

CREATE TRIGGER provider_credential_operation_binding_guard
BEFORE INSERT ON provider_credential_operations
WHEN NOT EXISTS (
    SELECT 1
    FROM provider_connections AS connection
    WHERE connection.id = NEW.connection_id
      AND connection.archived_at IS NULL
      AND connection.credential_ref = NEW.credential_ref
      AND connection.credential_scope_json IS NOT NULL
)
BEGIN
    SELECT RAISE(ABORT, 'credential operation is detached from an active credential binding');
END;

CREATE TRIGGER provider_credential_operation_initial_status_guard
BEFORE INSERT ON provider_credential_operations
WHEN NEW.status <> 'prepared'
  OR NEW.outcome_code IS NOT NULL
  OR NEW.outcome_attestation_sequence IS NOT NULL
  OR NEW.started_at IS NOT NULL
  OR NEW.finished_at IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'credential operation must begin prepared');
END;

CREATE TRIGGER provider_credential_operation_no_replace
BEFORE INSERT ON provider_credential_operations
WHEN EXISTS (
    SELECT 1 FROM provider_credential_operations AS existing
    WHERE existing.id = NEW.id
       OR (existing.credential_ref = NEW.credential_ref
         AND existing.operation_sequence = NEW.operation_sequence)
       OR (existing.credential_ref = NEW.credential_ref
         AND existing.status IN (
           'prepared', 'started', 'cleanup_required', 'outcome_unknown'
         ))
)
BEGIN
    SELECT RAISE(ABORT, 'provider credential operation cannot replace existing history');
END;

CREATE TRIGGER provider_credential_operation_identity_guard
BEFORE UPDATE ON provider_credential_operations
WHEN
    NEW.id <> OLD.id
    OR NEW.connection_id <> OLD.connection_id
    OR NEW.credential_ref <> OLD.credential_ref
    OR NEW.operation_sequence <> OLD.operation_sequence
    OR NEW.operation_kind <> OLD.operation_kind
    OR NEW.connection_binding_sha256 <> OLD.connection_binding_sha256
    OR NEW.plan_json <> OLD.plan_json
    OR NEW.plan_sha256 <> OLD.plan_sha256
    OR NEW.preflight_status <> OLD.preflight_status
    OR NEW.preflight_evidence_sha256 <> OLD.preflight_evidence_sha256
    OR NEW.preflight_attested_at <> OLD.preflight_attested_at
    OR NEW.native_owner <> OLD.native_owner
    OR NEW.schema_version <> OLD.schema_version
    OR NEW.redaction_version <> OLD.redaction_version
    OR NEW.created_at <> OLD.created_at
BEGIN
    SELECT RAISE(ABORT, 'credential operation plan is immutable');
END;

-- Every terminal or uncertain outcome is backed by the exact typed native
-- observation inserted in the same transaction.
CREATE TRIGGER provider_credential_operation_outcome_attestation_guard
BEFORE UPDATE OF status, outcome_code, outcome_attestation_sequence
ON provider_credential_operations
WHEN NEW.status IN ('succeeded', 'no_effect', 'cleanup_required', 'outcome_unknown')
  AND NOT EXISTS (
    SELECT 1
    FROM provider_credential_operation_attestations AS attestation
    WHERE attestation.operation_id = OLD.id
      AND attestation.sequence = NEW.outcome_attestation_sequence
      AND attestation.native_owner = 'native_platform'
      AND attestation.schema_version = OLD.schema_version
      AND attestation.redaction_version = OLD.redaction_version
      AND (
        (NEW.status = 'succeeded'
          AND OLD.operation_kind = 'install'
          AND NEW.outcome_code = 'native_effect_confirmed'
          AND OLD.started_at IS NOT NULL
          AND OLD.preflight_status = 'missing'
          AND attestation.slot_status = 'available'
          AND (OLD.predecessor_authority_id IS NULL
            OR EXISTS (
              SELECT 1
              FROM provider_credential_operation_attestations AS predecessor_intent
              JOIN provider_credential_operation_attestations AS predecessor_missing
                ON predecessor_missing.operation_id = predecessor_intent.operation_id
               AND predecessor_missing.sequence > predecessor_intent.sequence
              WHERE predecessor_intent.operation_id = OLD.id
                AND predecessor_intent.stage = 'predecessor_delete_intent'
                AND predecessor_missing.stage = 'predecessor_missing'
                AND predecessor_missing.slot_status = 'missing'
            )))
        OR (NEW.status = 'succeeded'
          AND OLD.operation_kind IN ('remove_credential', 'remove_for_archive')
          AND NEW.outcome_code = 'native_effect_confirmed'
          AND OLD.started_at IS NOT NULL
          AND attestation.slot_status = 'missing')
        OR (NEW.status = 'no_effect'
          AND (OLD.status <> 'cleanup_required'
            OR OLD.operation_kind <> 'install'
            OR OLD.predecessor_authority_id IS NULL
            OR (
              EXISTS (
                SELECT 1
                FROM provider_credential_operation_attestations AS cleanup_intent
                WHERE cleanup_intent.operation_id = OLD.id
                  AND cleanup_intent.stage IN (
                    'cleanup_remove_intent', 'cleanup_archive_intent'
                  )
              )
              AND EXISTS (
                SELECT 1
                FROM provider_credential_operation_attestations AS predecessor_intent
                JOIN provider_credential_operation_attestations AS predecessor_missing
                  ON predecessor_missing.operation_id = predecessor_intent.operation_id
                 AND predecessor_missing.sequence > predecessor_intent.sequence
                WHERE predecessor_intent.operation_id = OLD.id
                  AND predecessor_intent.stage = 'predecessor_delete_intent'
                  AND predecessor_missing.stage = 'predecessor_missing'
                  AND predecessor_missing.slot_status = 'missing'
              )
            ))
          AND (
            (NEW.outcome_code = 'native_effect_absent'
              AND (
                (OLD.operation_kind = 'install'
                  AND attestation.slot_status = 'missing'
                  AND NOT (OLD.started_at IS NOT NULL
                    AND OLD.predecessor_authority_id IS NOT NULL))
                OR (OLD.operation_kind IN ('remove_credential', 'remove_for_archive')
                  AND OLD.started_at IS NULL
                  AND OLD.preflight_status IN ('missing', 'available')
                  AND attestation.slot_status = OLD.preflight_status)
              ))
            OR (NEW.outcome_code = 'connection_changed'
              AND attestation.slot_status = 'missing')
          ))
        OR (NEW.status = 'cleanup_required'
          AND (
            (NEW.outcome_code = 'connection_changed'
              AND attestation.slot_status IN ('missing', 'available')
              AND (
                OLD.outcome_code IS NULL
                OR OLD.outcome_code NOT IN (
                  'native_durability_unknown',
                  'native_predecessor_durability_unknown'
                )
                OR (
                  attestation.stage = 'durability_repair'
                  AND attestation.slot_status = 'missing'
                )
              ))
            OR (NEW.outcome_code = 'native_status_unreadable'
              AND attestation.slot_status = 'unreadable')
            OR (NEW.outcome_code = 'native_durability_unknown'
              AND attestation.slot_status = 'unreadable')
            OR (NEW.outcome_code = 'native_predecessor_durability_unknown'
              AND attestation.slot_status = 'unreadable')
            OR (NEW.outcome_code = 'archive_commit_failed'
              AND (OLD.operation_kind = 'remove_for_archive'
                OR EXISTS (
                  SELECT 1
                  FROM provider_credential_operation_attestations AS archive_intent
                  WHERE archive_intent.operation_id = OLD.id
                    AND archive_intent.stage = 'cleanup_archive_intent'
                ))
              AND attestation.slot_status = 'missing')
          ))
        OR (NEW.status = 'outcome_unknown'
          AND (
            (attestation.slot_status = 'unreadable'
              AND NEW.outcome_code = 'native_status_unreadable')
            OR (attestation.slot_status = 'available'
              AND NEW.outcome_code IN ('native_effect_confirmed', 'connection_changed'))
            OR (OLD.operation_kind = 'install'
              AND OLD.started_at IS NOT NULL
              AND OLD.predecessor_authority_id IS NOT NULL
              AND attestation.slot_status = 'missing'
              AND NEW.outcome_code = 'connection_changed')
          ))
      )
  )
BEGIN
    SELECT RAISE(ABORT, 'credential operation outcome lacks a matching native attestation');
END;

CREATE TRIGGER provider_credential_operation_active_durability_guard
BEFORE UPDATE OF status ON provider_credential_operations
WHEN NEW.status IN ('succeeded', 'no_effect')
  AND EXISTS (
    SELECT 1
    FROM provider_credential_operation_attestations AS required
    WHERE required.operation_id = OLD.id
      AND required.stage IN (
        'operation_durability_required',
        'predecessor_durability_required'
      )
      AND NOT EXISTS (
        SELECT 1
        FROM provider_credential_operation_attestations AS repaired
        WHERE repaired.operation_id = required.operation_id
          AND repaired.sequence > required.sequence
          AND repaired.stage = replace(required.stage, '_required', '_repaired')
      )
  )
BEGIN
    SELECT RAISE(ABORT, 'credential durability obligation blocks terminal settlement');
END;

CREATE TRIGGER provider_credential_operation_legal_transition
BEFORE UPDATE OF status, outcome_code, outcome_attestation_sequence,
                 started_at, finished_at, updated_at
ON provider_credential_operations
WHEN NOT (
    (OLD.status = 'prepared' AND NEW.status = 'started'
      AND NEW.outcome_code IS NULL AND NEW.outcome_attestation_sequence IS NULL
      AND NEW.started_at IS NOT NULL AND NEW.finished_at IS NULL
      AND NOT (OLD.operation_kind IN ('remove_credential', 'remove_for_archive')
        AND OLD.preflight_status = 'missing'))
    OR (OLD.status = 'prepared' AND NEW.status IN ('no_effect', 'outcome_unknown')
      AND NEW.started_at IS NULL AND NEW.finished_at IS NOT NULL
      AND NEW.outcome_code IS NOT NULL AND NEW.outcome_attestation_sequence IS NOT NULL)
    OR (OLD.status = 'prepared' AND NEW.status = 'cleanup_required'
      AND OLD.operation_kind = 'remove_for_archive'
      AND OLD.preflight_status = 'missing'
      AND NEW.started_at IS NULL AND NEW.finished_at IS NOT NULL
      AND NEW.outcome_code = 'archive_commit_failed'
      AND NEW.outcome_attestation_sequence IS NOT NULL)
    OR (OLD.status = 'started'
      AND NEW.status IN ('succeeded', 'no_effect', 'cleanup_required', 'outcome_unknown')
      AND NEW.started_at = OLD.started_at AND NEW.finished_at IS NOT NULL
      AND NEW.outcome_code IS NOT NULL AND NEW.outcome_attestation_sequence IS NOT NULL)
    OR (OLD.status = 'cleanup_required'
      AND NEW.status IN ('succeeded', 'no_effect', 'outcome_unknown')
      AND NEW.started_at = OLD.started_at AND NEW.finished_at = OLD.finished_at
      AND NEW.outcome_code IS NOT NULL AND NEW.outcome_attestation_sequence IS NOT NULL)
    OR (OLD.status = 'cleanup_required' AND NEW.status = 'cleanup_required'
      AND (NEW.started_at = OLD.started_at
        OR (OLD.started_at IS NULL AND NEW.started_at IS NOT NULL))
      AND NEW.finished_at = OLD.finished_at
      AND (NEW.outcome_code IN (
            'connection_changed', 'native_status_unreadable',
            'native_durability_unknown',
            'native_predecessor_durability_unknown'
          )
        OR (NEW.outcome_code = 'archive_commit_failed'
          AND (OLD.operation_kind = 'remove_for_archive'
            OR EXISTS (
              SELECT 1
              FROM provider_credential_operation_attestations AS archive_intent
              WHERE archive_intent.operation_id = OLD.id
                AND archive_intent.stage = 'cleanup_archive_intent'
            ))))
      AND NEW.outcome_attestation_sequence IS NOT NULL)
    OR (OLD.status = 'outcome_unknown'
      AND NEW.status IN ('succeeded', 'no_effect', 'cleanup_required', 'outcome_unknown')
      AND (NEW.started_at = OLD.started_at
        OR (NEW.status = 'cleanup_required'
          AND OLD.started_at IS NULL AND NEW.started_at IS NOT NULL))
      AND NEW.finished_at = OLD.finished_at
      AND NEW.outcome_code IS NOT NULL AND NEW.outcome_attestation_sequence IS NOT NULL)
)
BEGIN
    SELECT RAISE(ABORT, 'illegal credential operation status transition');
END;

CREATE TRIGGER provider_credential_operation_attestation_no_update
BEFORE UPDATE ON provider_credential_operation_attestations
BEGIN
    SELECT RAISE(ABORT, 'credential operation attestations are immutable');
END;

CREATE TRIGGER provider_credential_operation_attestation_no_replace
BEFORE INSERT ON provider_credential_operation_attestations
WHEN EXISTS (
    SELECT 1
    FROM provider_credential_operation_attestations AS existing
    WHERE existing.operation_id = NEW.operation_id
      AND existing.sequence = NEW.sequence
)
BEGIN
    SELECT RAISE(ABORT, 'credential operation attestation cannot replace existing evidence');
END;

CREATE TRIGGER provider_credential_operation_attestation_no_delete
BEFORE DELETE ON provider_credential_operation_attestations
BEGIN
    SELECT RAISE(ABORT, 'credential operation attestations are append-only');
END;

CREATE TRIGGER provider_credential_operation_no_delete
BEFORE DELETE ON provider_credential_operations
BEGIN
    SELECT RAISE(ABORT, 'credential operation history is append-only');
END;

-- Discovery credential authority reuses the schema-5 commit journal. Close
-- SQLite `INSERT OR REPLACE` delete-and-insert bypasses on every authority
-- bearing identity/unique key before those rows can back ownership release.
CREATE TRIGGER provider_discovery_session_no_replace
BEFORE INSERT ON provider_discovery_sessions
WHEN EXISTS (
    SELECT 1 FROM provider_discovery_sessions AS existing
    WHERE existing.id = NEW.id
)
BEGIN
    SELECT RAISE(ABORT, 'provider discovery session cannot replace history');
END;

CREATE TRIGGER provider_discovery_session_initial_state_guard
BEFORE INSERT ON provider_discovery_sessions
WHEN NEW.state <> 'draft'
  OR NEW.revision <> 0
  OR NEW.next_event_sequence <> 1
  OR NEW.draft_json IS NOT NULL
  OR NEW.review_diff_json IS NOT NULL
  OR NEW.error_json IS NOT NULL
  OR NEW.recovery_json IS NOT NULL
  OR NEW.unknown_operation IS NOT NULL
  OR NEW.manifest_sha256 IS NOT NULL
  OR NEW.commit_plan_sha256 IS NOT NULL
  OR NEW.commit_attempt_id IS NOT NULL
  OR NEW.committed_connection_id IS NOT NULL
  OR NEW.cancellation_pending <> 0
  OR NEW.active_operation_id IS NOT NULL
  OR NEW.active_effect_approval_json IS NOT NULL
  OR NEW.redaction_version <> 1
  OR NEW.created_at <> NEW.updated_at
BEGIN
    SELECT RAISE(ABORT, 'provider discovery session must begin in its initial state');
END;

CREATE TRIGGER provider_discovery_commit_attempt_no_replace
BEFORE INSERT ON provider_discovery_commit_attempts
WHEN EXISTS (
    SELECT 1 FROM provider_discovery_commit_attempts AS existing
    WHERE existing.id = NEW.id
       OR (existing.session_id = NEW.session_id
         AND existing.attempt_number = NEW.attempt_number)
       OR (existing.session_id = NEW.session_id
         AND existing.action_id = NEW.action_id)
       OR (existing.session_id = NEW.session_id
         AND existing.plan_sha256 = NEW.plan_sha256)
)
BEGIN
    SELECT RAISE(ABORT, 'provider discovery commit attempt cannot replace history');
END;

CREATE TRIGGER provider_discovery_commit_attempt_initial_state_guard
BEFORE INSERT ON provider_discovery_commit_attempts
WHEN NEW.phase <> 'prepared'
  OR NEW.completed_at IS NOT NULL
  OR NEW.redaction_version <> 1
  OR NEW.created_at <> NEW.updated_at
BEGIN
    SELECT RAISE(ABORT, 'provider discovery commit attempt must begin prepared');
END;

CREATE TRIGGER provider_discovery_operation_no_replace
BEFORE INSERT ON provider_discovery_operations
WHEN EXISTS (
    SELECT 1 FROM provider_discovery_operations AS existing
    WHERE existing.id = NEW.id
       OR (existing.session_id = NEW.session_id
         AND existing.action_id = NEW.action_id)
)
BEGIN
    SELECT RAISE(ABORT, 'provider discovery operation cannot replace history');
END;

CREATE TRIGGER provider_discovery_operation_initial_state_guard
BEFORE INSERT ON provider_discovery_operations
WHEN NEW.status <> 'prepared'
  OR NEW.started_at IS NOT NULL
  OR NEW.finished_at IS NOT NULL
  OR NEW.created_at <> NEW.updated_at
BEGIN
    SELECT RAISE(ABORT, 'provider discovery operation must begin prepared');
END;

CREATE TRIGGER provider_discovery_audit_no_replace
BEFORE INSERT ON provider_discovery_audit_log
WHEN EXISTS (
    SELECT 1 FROM provider_discovery_audit_log AS existing
    WHERE existing.id = NEW.id
       OR (existing.session_id = NEW.session_id
         AND existing.audit_sequence = NEW.audit_sequence)
)
BEGIN
    SELECT RAISE(ABORT, 'provider discovery audit cannot replace history');
END;

CREATE TRIGGER provider_discovery_evidence_no_replace
BEFORE INSERT ON provider_discovery_evidence
WHEN EXISTS (
    SELECT 1 FROM provider_discovery_evidence AS existing
    WHERE existing.id = NEW.id
)
BEGIN
    SELECT RAISE(ABORT, 'provider discovery evidence cannot replace history');
END;

CREATE TRIGGER provider_discovery_approval_no_replace
BEFORE INSERT ON provider_discovery_approvals
WHEN EXISTS (
    SELECT 1 FROM provider_discovery_approvals AS existing
    WHERE existing.id = NEW.id
)
BEGIN
    SELECT RAISE(ABORT, 'provider discovery approval cannot replace history');
END;

CREATE TRIGGER provider_discovery_outbox_no_replace
BEFORE INSERT ON provider_discovery_event_outbox
WHEN EXISTS (
    SELECT 1 FROM provider_discovery_event_outbox AS existing
    WHERE existing.id = NEW.id
       OR (existing.session_id = NEW.session_id
         AND existing.sequence = NEW.sequence)
       OR (existing.session_id = NEW.session_id
         AND existing.session_revision = NEW.session_revision)
)
BEGIN
    SELECT RAISE(ABORT, 'provider discovery event cannot replace history');
END;

CREATE TRIGGER provider_discovery_receipt_no_replace
BEFORE INSERT ON provider_discovery_action_receipts
WHEN EXISTS (
    SELECT 1 FROM provider_discovery_action_receipts AS existing
    WHERE existing.action_id = NEW.action_id
       OR (existing.session_id = NEW.session_id
         AND existing.action_id = NEW.action_id)
       OR (existing.session_id = NEW.session_id
         AND existing.resulting_revision = NEW.resulting_revision)
       OR (existing.session_id = NEW.session_id
         AND existing.event_sequence = NEW.event_sequence)
)
BEGIN
    SELECT RAISE(ABORT, 'provider discovery action receipt cannot replace history');
END;

CREATE TRIGGER provider_discovery_native_no_effect_attestation_no_replace
BEFORE INSERT ON provider_discovery_native_no_effect_attestations
WHEN EXISTS (
    SELECT 1
    FROM provider_discovery_native_no_effect_attestations AS existing
    WHERE existing.operation_id = NEW.operation_id
       OR (existing.session_id = NEW.session_id
         AND existing.commit_attempt_id = NEW.commit_attempt_id
         AND existing.operation_id = NEW.operation_id)
)
BEGIN
    SELECT RAISE(ABORT, 'provider discovery native attestation cannot replace history');
END;

-- This view proves only that an atomic-commit operation is bound to the exact
-- receipt which created it. Retry authority is granted separately below: a
-- canonical-looking restart is not trusted until its predecessor operation is
-- already reachable from the initial approved review.
CREATE VIEW provider_discovery_native_commit_start_candidates AS
SELECT operation.id AS operation_id,
       operation.session_id AS session_id,
       operation.action_id AS start_action_id,
       receipt.action_kind AS start_action_kind,
       receipt.expected_revision AS start_expected_revision,
       operation.expected_revision AS operation_expected_revision,
       receipt.event_sequence AS start_event_sequence,
       receipt.created_at AS start_created_at,
       attempt.id AS commit_attempt_id,
       attempt.plan_sha256 AS commit_plan_sha256,
       attempt.action_id AS attempt_action_id,
       attempt.expected_revision AS attempt_expected_revision,
       attempt.created_at AS attempt_created_at,
       json_extract(attempt.plan_json, '$.credential_approval_id')
           AS credential_approval_id,
       json_extract(attempt.plan_json, '$.manifest_sha256') AS manifest_sha256,
       json_extract(attempt.plan_json, '$.review_sha256') AS review_sha256,
       json_extract(attempt.plan_json, '$.graph_sha256') AS graph_sha256,
       transition_audit.audit_sequence AS start_transition_audit_sequence,
       commit_audit.audit_sequence AS commit_prepared_audit_sequence
FROM provider_discovery_operations AS operation
JOIN provider_discovery_action_receipts AS receipt
  ON receipt.session_id = operation.session_id
 AND receipt.action_id = operation.action_id
JOIN provider_discovery_event_outbox AS event
  ON event.id = receipt.event_id
 AND event.session_id = receipt.session_id
JOIN provider_discovery_commit_attempts AS attempt
  ON attempt.session_id = operation.session_id
 AND attempt.id = json_extract(receipt.response_json, '$.session.commit_attempt_id')
 AND attempt.plan_sha256 = json_extract(
     receipt.response_json,
     '$.session.commit_plan_sha256'
 )
JOIN provider_discovery_audit_log AS transition_audit
  ON transition_audit.session_id = receipt.session_id
 AND transition_audit.audit_kind = 'transition_applied'
 AND transition_audit.action_id = receipt.action_id
 AND transition_audit.subject_id = event.id
 AND transition_audit.session_revision = receipt.resulting_revision
 AND transition_audit.summary_key = 'discovery.audit.transition_applied'
 AND transition_audit.created_at = receipt.created_at
JOIN provider_discovery_audit_log AS commit_audit
  ON commit_audit.session_id = receipt.session_id
 AND commit_audit.audit_kind = 'commit_prepared'
 AND commit_audit.action_id = receipt.action_id
 AND commit_audit.subject_id = attempt.id
 AND commit_audit.session_revision = receipt.resulting_revision
 AND commit_audit.summary_key = 'discovery.audit.commit_prepared'
 AND commit_audit.created_at = receipt.created_at
WHERE operation.operation_kind = 'atomic_commit'
  AND operation.side_effect_class = 'persistent'
  AND operation.action_id = receipt.action_id
  AND operation.expected_revision = receipt.resulting_revision
  AND operation.request_sha256 = receipt.request_sha256
  AND operation.created_at = receipt.created_at
  AND receipt.action_kind IN ('approve_review', 'restart_interrupted')
  AND receipt.outcome = 'applied'
  AND receipt.resulting_revision = receipt.expected_revision + 1
  AND receipt.redaction_version = 1
  AND event.sequence = receipt.event_sequence
  AND event.event_version = 2
  AND event.session_revision = receipt.resulting_revision
  AND event.state = 'committing'
  AND event.redaction_version = 1
  AND event.created_at = receipt.created_at
  AND attempt.redaction_version = 1
  AND attempt.plan_sha256
      = lorepia_discovery_commit_plan_sha256(attempt.plan_json)
  AND json_extract(attempt.plan_json, '$.attempt_id') = attempt.id
  AND json_extract(attempt.plan_json, '$.session_id') = attempt.session_id
  AND json_extract(
      attempt.plan_json,
      '$.expected_revision'
  ) = attempt.expected_revision
  AND json_extract(
      receipt.response_json,
      '$.previous_revision'
  ) = receipt.expected_revision
  AND json_extract(receipt.response_json, '$.session.id') = receipt.session_id
  AND json_extract(receipt.response_json, '$.session.state') = 'committing'
  AND json_extract(
      receipt.response_json,
      '$.session.revision'
  ) = receipt.resulting_revision
  AND json_extract(
      receipt.response_json,
      '$.session.next_event_sequence'
  ) = receipt.event_sequence + 1
  AND json_extract(
      receipt.response_json,
      '$.session.commit_attempt_id'
  ) = attempt.id
  AND json_extract(
      receipt.response_json,
      '$.session.commit_plan_sha256'
  ) = attempt.plan_sha256
  AND json_extract(
      receipt.response_json,
      '$.session.input.connection_id'
  ) = json_extract(attempt.plan_json, '$.connection_id')
  AND json_extract(
      receipt.response_json,
      '$.session.input.credential_ref'
  ) = json_extract(attempt.plan_json, '$.credential_ref')
  AND json_extract(
      receipt.response_json,
      '$.session.manifest_sha256'
  ) = json_extract(attempt.plan_json, '$.manifest_sha256')
  AND json_extract(
      receipt.response_json,
      '$.session.cancellation_pending'
  ) = 0
  AND json_type(receipt.response_json, '$.session.recovery') = 'null'
  AND json_type(receipt.response_json, '$.session.unknown_operation') = 'null'
  AND json_type(receipt.response_json, '$.session.active_effect_approval') = 'null'
  AND json_type(receipt.response_json, '$.session.failure') = 'null'
  AND json_type(receipt.response_json, '$.session.committed_connection_id') = 'null'
  AND json_extract(receipt.response_json, '$.effect.effect') = 'commit_atomically'
  AND json_extract(
      receipt.response_json,
      '$.effect.commit_attempt_id'
  ) = attempt.id
  AND json_extract(
      receipt.response_json,
      '$.effect.plan_sha256'
  ) = attempt.plan_sha256
  AND json_extract(receipt.response_json, '$.receipt.action_id') = receipt.action_id
  AND json_extract(receipt.response_json, '$.receipt.session_id') = receipt.session_id
  AND json_extract(receipt.response_json, '$.receipt.action_kind') = receipt.action_kind
  AND json_extract(
      receipt.response_json,
      '$.receipt.request_sha256'
  ) = receipt.request_sha256
  AND json_extract(
      receipt.response_json,
      '$.receipt.expected_revision'
  ) = receipt.expected_revision
  AND json_extract(
      receipt.response_json,
      '$.receipt.resulting_revision'
  ) = receipt.resulting_revision
  AND json_extract(
      receipt.response_json,
      '$.receipt.event_sequence'
  ) = receipt.event_sequence
  AND json_extract(receipt.response_json, '$.receipt.outcome') = receipt.outcome
  AND json_extract(receipt.response_json, '$.event.id') = event.id
  AND json_extract(receipt.response_json, '$.event.session_id') = event.session_id
  AND json_extract(receipt.response_json, '$.event.version') = event.event_version
  AND json_extract(receipt.response_json, '$.event.sequence') = event.sequence
  AND json_extract(
      receipt.response_json,
      '$.event.session_revision'
  ) = event.session_revision
  AND json_extract(receipt.response_json, '$.event.state') = event.state
  AND json_extract(receipt.response_json, '$.event.action_id') = receipt.action_id
  AND json_extract(receipt.response_json, '$.event') = event.event_json
  AND json_extract(event.event_json, '$.id') = event.id
  AND json_extract(event.event_json, '$.session_id') = event.session_id
  AND json_extract(event.event_json, '$.version') = event.event_version
  AND json_extract(event.event_json, '$.sequence') = event.sequence
  AND json_extract(
      event.event_json,
      '$.session_revision'
  ) = event.session_revision
  AND json_extract(event.event_json, '$.state') = event.state
  AND json_extract(event.event_json, '$.action_id') = receipt.action_id
  AND json_type(event.event_json, '$.failure') = 'null'
  AND transition_audit.audit_sequence < commit_audit.audit_sequence
  AND 1 = (
      SELECT COUNT(*)
      FROM provider_discovery_audit_log AS exact_transition
      WHERE exact_transition.session_id = receipt.session_id
        AND exact_transition.audit_kind = 'transition_applied'
        AND exact_transition.action_id = receipt.action_id
        AND exact_transition.subject_id = event.id
        AND exact_transition.session_revision = receipt.resulting_revision
        AND exact_transition.summary_key = 'discovery.audit.transition_applied'
        AND exact_transition.created_at = receipt.created_at
  )
  AND 1 = (
      SELECT COUNT(*)
      FROM provider_discovery_audit_log AS exact_commit
      WHERE exact_commit.session_id = receipt.session_id
        AND exact_commit.audit_kind = 'commit_prepared'
        AND exact_commit.action_id = receipt.action_id
        AND exact_commit.subject_id = attempt.id
        AND exact_commit.session_revision = receipt.resulting_revision
        AND exact_commit.summary_key = 'discovery.audit.commit_prepared'
        AND exact_commit.created_at = receipt.created_at
  );

-- A restart receipt can start another native credential operation only when
-- the immediately preceding revision is an immutable, evidence-backed
-- interruption of this same commit attempt. Keep this proof in one view so
-- the attestation-insert and operation-transition triggers cannot drift.
CREATE VIEW provider_discovery_native_retry_start_candidates AS
SELECT restart_start.operation_id AS restart_operation_id,
       restart.action_id AS restart_action_id,
       restart.session_id AS session_id,
       attempt.id AS commit_attempt_id,
       attempt.plan_sha256 AS commit_plan_sha256,
       restart_start.operation_expected_revision AS restart_operation_expected_revision,
       restart_start.start_transition_audit_sequence AS restart_transition_audit_sequence,
       restart_start.commit_prepared_audit_sequence AS restart_commit_audit_sequence,
       predecessor_operation.id AS predecessor_operation_id,
       predecessor_start.start_action_id AS predecessor_start_action_id,
       predecessor_operation.expected_revision AS predecessor_operation_expected_revision,
       predecessor_start.commit_prepared_audit_sequence AS predecessor_commit_audit_sequence
FROM provider_discovery_action_receipts AS restart
JOIN provider_discovery_event_outbox AS restart_event
  ON restart_event.id = restart.event_id
 AND restart_event.session_id = restart.session_id
JOIN provider_discovery_commit_attempts AS attempt
  ON attempt.session_id = restart.session_id
 AND attempt.id = json_extract(
     restart.response_json,
     '$.session.commit_attempt_id'
 )
 AND attempt.plan_sha256 = json_extract(
     restart.response_json,
     '$.session.commit_plan_sha256'
 )
JOIN provider_discovery_action_receipts AS predecessor
  ON predecessor.session_id = restart.session_id
 AND predecessor.resulting_revision = restart.expected_revision
JOIN provider_discovery_event_outbox AS predecessor_event
  ON predecessor_event.id = predecessor.event_id
 AND predecessor_event.session_id = predecessor.session_id
JOIN provider_discovery_native_commit_start_candidates AS restart_start
  ON restart_start.start_action_id = restart.action_id
 AND restart_start.session_id = restart.session_id
 AND restart_start.commit_attempt_id = attempt.id
 AND restart_start.commit_plan_sha256 = attempt.plan_sha256
JOIN provider_discovery_action_receipts AS terminal_receipt
  ON terminal_receipt.session_id = predecessor.session_id
 AND terminal_receipt.resulting_revision = CASE predecessor.action_kind
     WHEN 'interrupt' THEN predecessor.resulting_revision
     WHEN 'resolve_unknown_outcome' THEN predecessor.expected_revision
     ELSE -1
 END
JOIN provider_discovery_event_outbox AS terminal_event
  ON terminal_event.id = terminal_receipt.event_id
 AND terminal_event.session_id = terminal_receipt.session_id
JOIN provider_discovery_operations AS predecessor_operation
  ON predecessor_operation.session_id = predecessor.session_id
 AND predecessor_operation.operation_kind = 'atomic_commit'
 AND predecessor_operation.side_effect_class = 'persistent'
 AND predecessor_operation.expected_revision = terminal_receipt.expected_revision
JOIN provider_discovery_native_commit_start_candidates AS predecessor_start
  ON predecessor_start.operation_id = predecessor_operation.id
 AND predecessor_start.session_id = predecessor_operation.session_id
 AND predecessor_start.commit_attempt_id = attempt.id
 AND predecessor_start.commit_plan_sha256 = attempt.plan_sha256
WHERE restart.action_kind = 'restart_interrupted'
  AND terminal_receipt.event_sequence = predecessor_start.start_event_sequence + 1
  AND terminal_receipt.redaction_version = 1
  AND terminal_event.sequence = terminal_receipt.event_sequence
  AND terminal_event.event_version = 2
  AND terminal_event.session_revision = terminal_receipt.resulting_revision
  AND terminal_event.redaction_version = 1
  AND terminal_event.created_at = terminal_receipt.created_at
  AND json_type(
      terminal_receipt.response_json,
      '$.session.failure'
  ) = 'null'
  AND json_type(terminal_event.event_json, '$.failure') = 'null'
  AND json_extract(
      terminal_event.event_json,
      '$.version'
  ) = terminal_event.event_version
  AND json_extract(
      terminal_receipt.response_json,
      '$.event'
  ) = terminal_event.event_json
  AND restart.outcome = 'applied'
  AND restart.resulting_revision = restart.expected_revision + 1
  AND restart_event.sequence = restart.event_sequence
  AND restart_event.session_revision = restart.resulting_revision
  AND restart_event.state = 'committing'
  AND restart.created_at = restart_event.created_at
  AND json_extract(
      restart.response_json,
      '$.previous_revision'
  ) = restart.expected_revision
  AND json_extract(restart.response_json, '$.session.id') = restart.session_id
  AND json_extract(restart.response_json, '$.session.state') = 'committing'
  AND json_extract(
      restart.response_json,
      '$.session.revision'
  ) = restart.resulting_revision
  AND json_extract(restart.response_json, '$.effect.effect') = 'commit_atomically'
  AND json_extract(
      restart.response_json,
      '$.effect.commit_attempt_id'
  ) = attempt.id
  AND json_extract(
      restart.response_json,
      '$.effect.plan_sha256'
  ) = attempt.plan_sha256
  AND json_extract(
      restart.response_json,
      '$.session.cancellation_pending'
  ) = 0
  AND json_extract(
      restart.response_json,
      '$.session.next_event_sequence'
  ) = restart.event_sequence + 1
  AND json_extract(restart.response_json, '$.receipt.action_id') = restart.action_id
  AND json_extract(restart.response_json, '$.receipt.session_id') = restart.session_id
  AND json_extract(restart.response_json, '$.receipt.action_kind') = restart.action_kind
  AND json_extract(
      restart.response_json,
      '$.receipt.request_sha256'
  ) = restart.request_sha256
  AND json_extract(
      restart.response_json,
      '$.receipt.expected_revision'
  ) = restart.expected_revision
  AND json_extract(
      restart.response_json,
      '$.receipt.resulting_revision'
  ) = restart.resulting_revision
  AND json_extract(
      restart.response_json,
      '$.receipt.event_sequence'
  ) = restart.event_sequence
  AND json_extract(restart.response_json, '$.receipt.outcome') = restart.outcome
  AND json_extract(restart.response_json, '$.event.id') = restart_event.id
  AND json_extract(
      restart.response_json,
      '$.event.session_id'
  ) = restart_event.session_id
  AND json_extract(restart.response_json, '$.event.version') = restart_event.event_version
  AND json_extract(restart.response_json, '$.event.sequence') = restart_event.sequence
  AND json_extract(
      restart.response_json,
      '$.event.session_revision'
  ) = restart_event.session_revision
  AND json_extract(restart.response_json, '$.event.state') = restart_event.state
  AND json_extract(restart.response_json, '$.event.action_id') = restart.action_id
  AND json_extract(restart_event.event_json, '$.id') = restart_event.id
  AND json_extract(
      restart_event.event_json,
      '$.session_id'
  ) = restart_event.session_id
  AND json_extract(
      restart_event.event_json,
      '$.sequence'
  ) = restart_event.sequence
  AND json_extract(
      restart_event.event_json,
      '$.session_revision'
  ) = restart_event.session_revision
  AND json_extract(restart_event.event_json, '$.state') = 'committing'
  AND json_extract(
      restart_event.event_json,
      '$.action_id'
  ) = restart.action_id
  AND 1 = (
      SELECT COUNT(*)
      FROM provider_discovery_audit_log AS restart_transition_audit
      WHERE restart_transition_audit.session_id = restart.session_id
        AND restart_transition_audit.audit_kind = 'transition_applied'
        AND restart_transition_audit.action_id = restart.action_id
        AND restart_transition_audit.subject_id = restart_event.id
        AND restart_transition_audit.session_revision = restart.resulting_revision
        AND restart_transition_audit.summary_key = 'discovery.audit.transition_applied'
        AND restart_transition_audit.created_at = restart.created_at
  )
  AND predecessor.outcome = 'applied'
  AND predecessor.redaction_version = 1
  AND predecessor.resulting_revision = predecessor.expected_revision + 1
  AND predecessor_event.sequence = predecessor.event_sequence
  AND predecessor_event.event_version = 2
  AND predecessor_event.session_revision = predecessor.resulting_revision
  AND predecessor_event.state = 'interrupted'
  AND predecessor_event.redaction_version = 1
  AND predecessor.created_at = predecessor_event.created_at
  AND json_type(predecessor.response_json, '$.session.failure') = 'null'
  AND json_type(predecessor_event.event_json, '$.failure') = 'null'
  AND julianday(predecessor.created_at) IS NOT NULL
  AND julianday(restart.created_at) IS NOT NULL
  AND julianday(predecessor.created_at) <= julianday(restart.created_at)
  AND restart.event_sequence = predecessor.event_sequence + 1
  AND restart.event_sequence = json_extract(
      predecessor.response_json,
      '$.session.next_event_sequence'
  )
  AND json_extract(predecessor.response_json, '$.session.state') = 'interrupted'
  AND json_extract(
      predecessor.response_json,
      '$.previous_revision'
  ) = predecessor.expected_revision
  AND json_extract(
      predecessor.response_json,
      '$.session.id'
  ) = predecessor.session_id
  AND json_extract(
      predecessor.response_json,
      '$.session.revision'
  ) = predecessor.resulting_revision
  AND json_extract(
      predecessor.response_json,
      '$.session.recovery.interrupted_state'
  ) = 'committing'
  AND json_extract(
      predecessor.response_json,
      '$.session.recovery.operation'
  ) = 'atomic_commit'
  AND json_extract(
      predecessor.response_json,
      '$.session.commit_attempt_id'
  ) = attempt.id
  AND json_extract(
      predecessor.response_json,
      '$.session.commit_plan_sha256'
  ) = attempt.plan_sha256
  AND json_extract(
      predecessor.response_json,
      '$.session.unknown_operation'
  ) IS NULL
  AND json_type(
      predecessor.response_json,
      '$.session.unknown_operation'
  ) = 'null'
  AND json_extract(
      predecessor.response_json,
      '$.session.cancellation_pending'
  ) = 0
  AND json_extract(predecessor.response_json, '$.effect.effect') = 'none'
  AND json_extract(
      predecessor.response_json,
      '$.receipt.action_id'
  ) = predecessor.action_id
  AND json_extract(
      predecessor.response_json,
      '$.receipt.session_id'
  ) = predecessor.session_id
  AND json_extract(
      predecessor.response_json,
      '$.receipt.action_kind'
  ) = predecessor.action_kind
  AND json_extract(
      predecessor.response_json,
      '$.receipt.request_sha256'
  ) = predecessor.request_sha256
  AND json_extract(
      predecessor.response_json,
      '$.receipt.expected_revision'
  ) = predecessor.expected_revision
  AND json_extract(
      predecessor.response_json,
      '$.receipt.resulting_revision'
  ) = predecessor.resulting_revision
  AND json_extract(
      predecessor.response_json,
      '$.receipt.event_sequence'
  ) = predecessor.event_sequence
  AND json_extract(
      predecessor.response_json,
      '$.receipt.outcome'
  ) = predecessor.outcome
  AND json_extract(
      predecessor.response_json,
      '$.event.id'
  ) = predecessor_event.id
  AND json_extract(
      predecessor.response_json,
      '$.event.session_id'
  ) = predecessor_event.session_id
  AND json_extract(
      predecessor.response_json,
      '$.event.version'
  ) = predecessor_event.event_version
  AND json_extract(
      predecessor.response_json,
      '$.event.sequence'
  ) = predecessor_event.sequence
  AND json_extract(
      predecessor.response_json,
      '$.event.session_revision'
  ) = predecessor_event.session_revision
  AND json_extract(
      predecessor.response_json,
      '$.event.state'
  ) = predecessor_event.state
  AND json_extract(
      predecessor.response_json,
      '$.event.action_id'
  ) = predecessor.action_id
  AND json_extract(
      predecessor.response_json,
      '$.event'
  ) = predecessor_event.event_json
  AND json_extract(predecessor_event.event_json, '$.id') = predecessor_event.id
  AND json_extract(
      predecessor_event.event_json,
      '$.version'
  ) = predecessor_event.event_version
  AND json_extract(
      predecessor_event.event_json,
      '$.session_id'
  ) = predecessor_event.session_id
  AND json_extract(
      predecessor_event.event_json,
      '$.sequence'
  ) = predecessor_event.sequence
  AND json_extract(
      predecessor_event.event_json,
      '$.session_revision'
  ) = predecessor_event.session_revision
  AND json_extract(
      predecessor_event.event_json,
      '$.state'
  ) = 'interrupted'
  AND json_extract(
      predecessor_event.event_json,
      '$.action_id'
  ) = predecessor.action_id
  AND json_extract(
      predecessor_event.event_json,
      '$.action_required.kind'
  ) = 'restart_interrupted'
  AND json_extract(
      predecessor_event.event_json,
      '$.action_required.operation'
  ) = 'atomic_commit'
  AND 1 = (
      SELECT COUNT(*)
      FROM provider_discovery_audit_log AS predecessor_transition_audit
      WHERE predecessor_transition_audit.session_id = predecessor.session_id
        AND predecessor_transition_audit.audit_kind = CASE predecessor.action_kind
            WHEN 'resolve_unknown_outcome' THEN 'unknown_outcome_reconciled'
            ELSE 'transition_applied'
        END
        AND predecessor_transition_audit.action_id = predecessor.action_id
        AND predecessor_transition_audit.subject_id = predecessor_event.id
        AND predecessor_transition_audit.session_revision = predecessor.resulting_revision
        AND predecessor_transition_audit.summary_key = 'discovery.audit.transition_applied'
        AND predecessor_transition_audit.created_at = predecessor.created_at
  )
  AND (
      (
          predecessor.action_kind = 'interrupt'
          AND 1 = (
              SELECT COUNT(*)
              FROM provider_discovery_operations AS interrupted_operation
              WHERE interrupted_operation.session_id = restart.session_id
                AND interrupted_operation.operation_kind = 'atomic_commit'
                AND interrupted_operation.side_effect_class = 'persistent'
                AND interrupted_operation.expected_revision = predecessor.expected_revision
          )
          AND 1 = (
              SELECT COUNT(*)
              FROM provider_discovery_operations AS interrupted_operation
              WHERE interrupted_operation.session_id = restart.session_id
                AND interrupted_operation.operation_kind = 'atomic_commit'
                AND interrupted_operation.side_effect_class = 'persistent'
                AND interrupted_operation.expected_revision = predecessor.expected_revision
                AND interrupted_operation.status = 'interrupted'
                AND interrupted_operation.started_at IS NOT NULL
                AND interrupted_operation.finished_at = predecessor.created_at
                AND interrupted_operation.updated_at = interrupted_operation.finished_at
                AND julianday(interrupted_operation.created_at) IS NOT NULL
                AND julianday(interrupted_operation.started_at) IS NOT NULL
                AND julianday(interrupted_operation.finished_at) IS NOT NULL
                AND julianday(interrupted_operation.created_at)
                    <= julianday(interrupted_operation.started_at)
                AND julianday(interrupted_operation.started_at)
                    <= julianday(interrupted_operation.finished_at)
                AND 1 = (
                    SELECT COUNT(*)
                    FROM provider_discovery_audit_log AS interrupted_audit
                    WHERE interrupted_audit.session_id = restart.session_id
                      AND interrupted_audit.audit_kind = 'operation_interrupted'
                      AND interrupted_audit.action_id = predecessor.action_id
                      AND interrupted_audit.subject_id = interrupted_operation.id
                      AND interrupted_audit.session_revision = predecessor.resulting_revision
                      AND interrupted_audit.summary_key = 'discovery.audit.operation_interrupted'
                      AND interrupted_audit.created_at = predecessor.created_at
                      AND interrupted_audit.audit_sequence > (
                          SELECT predecessor_transition_audit.audit_sequence
                          FROM provider_discovery_audit_log AS predecessor_transition_audit
                          WHERE predecessor_transition_audit.session_id = predecessor.session_id
                            AND predecessor_transition_audit.audit_kind = 'transition_applied'
                            AND predecessor_transition_audit.action_id = predecessor.action_id
                            AND predecessor_transition_audit.subject_id = predecessor_event.id
                            AND predecessor_transition_audit.session_revision = predecessor.resulting_revision
                            AND predecessor_transition_audit.summary_key = 'discovery.audit.transition_applied'
                            AND predecessor_transition_audit.created_at = predecessor.created_at
                      )
                )
                AND (
                    (
                        interrupted_operation.started_at = interrupted_operation.finished_at
                        AND NOT EXISTS (
                            SELECT 1
                            FROM provider_discovery_audit_log AS start_audit
                            WHERE start_audit.session_id = restart.session_id
                              AND start_audit.audit_kind = 'operation_started'
                              AND start_audit.subject_id = interrupted_operation.id
                        )
                        AND NOT EXISTS (
                            SELECT 1
                            FROM provider_discovery_native_no_effect_attestations AS attestation
                            WHERE attestation.operation_id = interrupted_operation.id
                        )
                    )
                    OR (
                        1 = (
                            SELECT COUNT(*)
                            FROM provider_discovery_audit_log AS start_audit
                            WHERE start_audit.session_id = restart.session_id
                              AND start_audit.audit_kind = 'operation_started'
                              AND start_audit.action_id = interrupted_operation.action_id
                              AND start_audit.subject_id = interrupted_operation.id
                              AND start_audit.session_revision = interrupted_operation.expected_revision
                              AND start_audit.summary_key = 'discovery.audit.operation_started'
                              AND start_audit.created_at = interrupted_operation.started_at
                              AND start_audit.audit_sequence < (
                                  SELECT predecessor_transition_audit.audit_sequence
                                  FROM provider_discovery_audit_log AS predecessor_transition_audit
                                  WHERE predecessor_transition_audit.session_id = predecessor.session_id
                                    AND predecessor_transition_audit.audit_kind = 'transition_applied'
                                    AND predecessor_transition_audit.action_id = predecessor.action_id
                                    AND predecessor_transition_audit.subject_id = predecessor_event.id
                                    AND predecessor_transition_audit.session_revision = predecessor.resulting_revision
                                    AND predecessor_transition_audit.summary_key = 'discovery.audit.transition_applied'
                                    AND predecessor_transition_audit.created_at = predecessor.created_at
                              )
                        )
                        AND 1 = (
                            SELECT COUNT(*)
                            FROM provider_discovery_native_no_effect_attestations AS attestation
                            WHERE attestation.operation_id = interrupted_operation.id
                              AND attestation.session_id = restart.session_id
                              AND attestation.commit_attempt_id = attempt.id
                              AND attestation.commit_plan_sha256 = attempt.plan_sha256
                              AND attestation.connection_id = json_extract(
                                  attempt.plan_json,
                                  '$.connection_id'
                              )
                              AND attestation.connection_id = json_extract(
                                  attempt.plan_json,
                                  '$.credential_ref'
                              )
                              AND attestation.attestation_kind = 'credential_slot_missing'
                              AND attestation.recovery_owner = 'native_platform'
                              AND attestation.schema_version = 1
                              AND attestation.redaction_version = 1
                              AND attestation.evidence_sha256
                                  = lorepia_native_no_effect_evidence_sha256(
                                      attestation.schema_version,
                                      attestation.attestation_kind,
                                      attestation.recovery_owner,
                                      attestation.operation_id,
                                      attestation.session_id,
                                      attestation.commit_attempt_id,
                                      attestation.commit_plan_sha256,
                                      attestation.connection_id
                                  )
                              AND attestation.attested_at = interrupted_operation.finished_at
                        )
                    )
                )
          )
      )
      OR (
          predecessor.action_kind = 'resolve_unknown_outcome'
          AND EXISTS (
              SELECT 1
              FROM provider_discovery_action_receipts AS unknown_receipt
              JOIN provider_discovery_event_outbox AS unknown_event
                ON unknown_event.id = unknown_receipt.event_id
               AND unknown_event.session_id = unknown_receipt.session_id
              WHERE unknown_receipt.session_id = restart.session_id
                AND unknown_receipt.resulting_revision = predecessor.expected_revision
                AND unknown_receipt.action_kind IN (
                    'interrupt',
                    'external_outcome_became_unknown'
                )
                AND unknown_receipt.outcome = 'applied'
                AND unknown_receipt.redaction_version = 1
                AND unknown_receipt.resulting_revision = unknown_receipt.expected_revision + 1
                AND unknown_event.sequence = unknown_receipt.event_sequence
                AND unknown_event.event_version = 2
                AND unknown_event.session_revision = unknown_receipt.resulting_revision
                AND unknown_event.state = 'unknown_outcome'
                AND unknown_event.redaction_version = 1
                AND unknown_receipt.created_at = unknown_event.created_at
                AND julianday(unknown_receipt.created_at) IS NOT NULL
                AND julianday(unknown_receipt.created_at) <= julianday(predecessor.created_at)
                AND predecessor.event_sequence = unknown_receipt.event_sequence + 1
                AND predecessor.event_sequence = json_extract(
                    unknown_receipt.response_json,
                    '$.session.next_event_sequence'
                )
                AND json_extract(
                    unknown_receipt.response_json,
                    '$.session.state'
                ) = 'unknown_outcome'
                AND json_extract(
                    unknown_receipt.response_json,
                    '$.previous_revision'
                ) = unknown_receipt.expected_revision
                AND json_extract(
                    unknown_receipt.response_json,
                    '$.session.id'
                ) = unknown_receipt.session_id
                AND json_extract(
                    unknown_receipt.response_json,
                    '$.session.revision'
                ) = unknown_receipt.resulting_revision
                AND json_extract(
                    unknown_receipt.response_json,
                    '$.session.unknown_operation'
                ) = 'atomic_commit'
                AND json_extract(
                    unknown_receipt.response_json,
                    '$.session.recovery'
                ) IS NULL
                AND json_type(
                    unknown_receipt.response_json,
                    '$.session.recovery'
                ) = 'null'
                AND json_extract(
                    unknown_receipt.response_json,
                    '$.session.commit_attempt_id'
                ) = attempt.id
                AND json_extract(
                    unknown_receipt.response_json,
                    '$.session.commit_plan_sha256'
                ) = attempt.plan_sha256
                AND json_extract(
                    unknown_receipt.response_json,
                    '$.effect.effect'
                ) = 'none'
                AND json_extract(
                    unknown_receipt.response_json,
                    '$.session.cancellation_pending'
                ) = 0
                AND json_extract(
                    unknown_receipt.response_json,
                    '$.receipt.action_id'
                ) = unknown_receipt.action_id
                AND json_extract(
                    unknown_receipt.response_json,
                    '$.receipt.session_id'
                ) = unknown_receipt.session_id
                AND json_extract(
                    unknown_receipt.response_json,
                    '$.receipt.action_kind'
                ) = unknown_receipt.action_kind
                AND json_extract(
                    unknown_receipt.response_json,
                    '$.receipt.request_sha256'
                ) = unknown_receipt.request_sha256
                AND json_extract(
                    unknown_receipt.response_json,
                    '$.receipt.expected_revision'
                ) = unknown_receipt.expected_revision
                AND json_extract(
                    unknown_receipt.response_json,
                    '$.receipt.resulting_revision'
                ) = unknown_receipt.resulting_revision
                AND json_extract(
                    unknown_receipt.response_json,
                    '$.receipt.event_sequence'
                ) = unknown_receipt.event_sequence
                AND json_extract(
                    unknown_receipt.response_json,
                    '$.receipt.outcome'
                ) = unknown_receipt.outcome
                AND json_extract(
                    unknown_receipt.response_json,
                    '$.event.id'
                ) = unknown_event.id
                AND json_extract(
                    unknown_receipt.response_json,
                    '$.event.session_id'
                ) = unknown_event.session_id
                AND json_extract(
                    unknown_receipt.response_json,
                    '$.event.version'
                ) = unknown_event.event_version
                AND json_extract(
                    unknown_receipt.response_json,
                    '$.event.sequence'
                ) = unknown_event.sequence
                AND json_extract(
                    unknown_receipt.response_json,
                    '$.event.session_revision'
                ) = unknown_event.session_revision
                AND json_extract(
                    unknown_receipt.response_json,
                    '$.event.state'
                ) = unknown_event.state
                AND json_extract(
                    unknown_receipt.response_json,
                    '$.event.action_id'
                ) = unknown_receipt.action_id
                AND json_extract(
                    unknown_receipt.response_json,
                    '$.event'
                ) = unknown_event.event_json
                AND json_extract(unknown_event.event_json, '$.id') = unknown_event.id
                AND json_extract(
                    unknown_event.event_json,
                    '$.version'
                ) = unknown_event.event_version
                AND json_extract(
                    unknown_event.event_json,
                    '$.session_id'
                ) = unknown_event.session_id
                AND json_extract(
                    unknown_event.event_json,
                    '$.sequence'
                ) = unknown_event.sequence
                AND json_extract(
                    unknown_event.event_json,
                    '$.session_revision'
                ) = unknown_event.session_revision
                AND json_extract(
                    unknown_event.event_json,
                    '$.state'
                ) = 'unknown_outcome'
                AND json_extract(
                    unknown_event.event_json,
                    '$.action_id'
                ) = unknown_receipt.action_id
                AND json_extract(
                    unknown_event.event_json,
                    '$.action_required.kind'
                ) = 'reconcile_unknown_outcome'
                AND json_extract(
                    unknown_event.event_json,
                    '$.action_required.operation'
                ) = 'atomic_commit'
                AND 1 = (
                    SELECT COUNT(*)
                    FROM provider_discovery_audit_log AS unknown_transition_audit
                    WHERE unknown_transition_audit.session_id = unknown_receipt.session_id
                      AND unknown_transition_audit.audit_kind = 'transition_applied'
                      AND unknown_transition_audit.action_id = unknown_receipt.action_id
                      AND unknown_transition_audit.subject_id = unknown_event.id
                      AND unknown_transition_audit.session_revision = unknown_receipt.resulting_revision
                      AND unknown_transition_audit.summary_key = 'discovery.audit.transition_applied'
                      AND unknown_transition_audit.created_at = unknown_receipt.created_at
                )
                AND 1 = (
                    SELECT COUNT(*)
                    FROM provider_discovery_operations AS unknown_operation
                    WHERE unknown_operation.session_id = restart.session_id
                      AND unknown_operation.operation_kind = 'atomic_commit'
                      AND unknown_operation.side_effect_class = 'persistent'
                      AND unknown_operation.expected_revision = unknown_receipt.expected_revision
                )
                AND 1 = (
                    SELECT COUNT(*)
                    FROM provider_discovery_operations AS unknown_operation
                    WHERE unknown_operation.session_id = restart.session_id
                      AND unknown_operation.operation_kind = 'atomic_commit'
                      AND unknown_operation.side_effect_class = 'persistent'
                      AND unknown_operation.expected_revision = unknown_receipt.expected_revision
                      AND unknown_operation.status = 'outcome_unknown'
                      AND unknown_operation.started_at IS NOT NULL
                      AND unknown_operation.finished_at = unknown_receipt.created_at
                      AND unknown_operation.updated_at = unknown_operation.finished_at
                      AND julianday(unknown_operation.created_at) IS NOT NULL
                      AND julianday(unknown_operation.started_at) IS NOT NULL
                      AND julianday(unknown_operation.finished_at) IS NOT NULL
                      AND julianday(unknown_operation.created_at)
                          <= julianday(unknown_operation.started_at)
                      AND julianday(unknown_operation.started_at)
                          <= julianday(unknown_operation.finished_at)
                      AND 1 = (
                          SELECT COUNT(*)
                          FROM provider_discovery_audit_log AS start_audit
                          WHERE start_audit.session_id = restart.session_id
                            AND start_audit.audit_kind = 'operation_started'
                            AND start_audit.action_id = unknown_operation.action_id
                            AND start_audit.subject_id = unknown_operation.id
                            AND start_audit.session_revision = unknown_operation.expected_revision
                            AND start_audit.summary_key = 'discovery.audit.operation_started'
                            AND start_audit.created_at = unknown_operation.started_at
                            AND start_audit.audit_sequence < (
                                SELECT unknown_transition_audit.audit_sequence
                                FROM provider_discovery_audit_log AS unknown_transition_audit
                                WHERE unknown_transition_audit.session_id = unknown_receipt.session_id
                                  AND unknown_transition_audit.audit_kind = 'transition_applied'
                                  AND unknown_transition_audit.action_id = unknown_receipt.action_id
                                  AND unknown_transition_audit.subject_id = unknown_event.id
                                  AND unknown_transition_audit.session_revision = unknown_receipt.resulting_revision
                                  AND unknown_transition_audit.summary_key = 'discovery.audit.transition_applied'
                                  AND unknown_transition_audit.created_at = unknown_receipt.created_at
                            )
                      )
                      AND 1 = (
                          SELECT COUNT(*)
                          FROM provider_discovery_audit_log AS unknown_audit
                          WHERE unknown_audit.session_id = restart.session_id
                            AND unknown_audit.audit_kind = 'operation_interrupted'
                            AND unknown_audit.action_id = unknown_receipt.action_id
                            AND unknown_audit.subject_id = unknown_operation.id
                            AND unknown_audit.session_revision = unknown_receipt.resulting_revision
                            AND unknown_audit.summary_key = 'discovery.audit.operation_interrupted'
                            AND unknown_audit.created_at = unknown_receipt.created_at
                            AND unknown_audit.audit_sequence > (
                                SELECT unknown_transition_audit.audit_sequence
                                FROM provider_discovery_audit_log AS unknown_transition_audit
                                WHERE unknown_transition_audit.session_id = unknown_receipt.session_id
                                  AND unknown_transition_audit.audit_kind = 'transition_applied'
                                  AND unknown_transition_audit.action_id = unknown_receipt.action_id
                                  AND unknown_transition_audit.subject_id = unknown_event.id
                                  AND unknown_transition_audit.session_revision = unknown_receipt.resulting_revision
                                  AND unknown_transition_audit.summary_key = 'discovery.audit.transition_applied'
                                  AND unknown_transition_audit.created_at = unknown_receipt.created_at
                            )
                      )
                )
                AND 1 = (
                    SELECT COUNT(*)
                    FROM provider_discovery_approvals AS approval
                    WHERE approval.session_id = restart.session_id
                      AND approval.approval_kind = 'unknown_outcome_resolution'
                      AND approval.candidate_id IS NULL
                      AND approval.decision = 'approved'
                      AND approval.session_revision = predecessor.expected_revision
                      AND approval.created_at = predecessor.created_at
                      AND approval.redaction_version = 1
                      AND approval.grant_sha256 = '710ffd1242a5df75c398580e1cec927bd1660bb1e55f035e1857d0924e949766'
                      AND approval.grant_json = '{"kind":"unknown_outcome_resolution","operation":"atomic_commit","resolution":{"resolution":"confirmed_no_effect"}}'
                )
                AND 1 = (
                    SELECT COUNT(*)
                    FROM provider_discovery_approvals AS approval
                    WHERE approval.session_id = restart.session_id
                      AND approval.approval_kind = 'unknown_outcome_resolution'
                      AND approval.candidate_id IS NULL
                      AND approval.decision = 'approved'
                      AND approval.session_revision = predecessor.expected_revision
                      AND approval.created_at = predecessor.created_at
                      AND approval.redaction_version = 1
                      AND approval.grant_sha256 = '710ffd1242a5df75c398580e1cec927bd1660bb1e55f035e1857d0924e949766'
                      AND approval.grant_json = '{"kind":"unknown_outcome_resolution","operation":"atomic_commit","resolution":{"resolution":"confirmed_no_effect"}}'
                      AND json_extract(approval.grant_json, '$.kind') = 'unknown_outcome_resolution'
                      AND json_extract(approval.grant_json, '$.operation') = 'atomic_commit'
                      AND json_extract(
                          approval.grant_json,
                          '$.resolution.resolution'
                      ) = 'confirmed_no_effect'
                      AND 1 = (
                          SELECT COUNT(*)
                          FROM provider_discovery_audit_log AS approval_audit
                          WHERE approval_audit.session_id = restart.session_id
                            AND approval_audit.audit_kind = 'approval_recorded'
                            AND approval_audit.action_id = predecessor.action_id
                            AND approval_audit.subject_id = approval.id
                            AND approval_audit.session_revision = predecessor.resulting_revision
                            AND approval_audit.summary_key = 'discovery.audit.approval_recorded'
                            AND approval_audit.created_at = predecessor.created_at
                            AND approval_audit.audit_sequence > (
                                SELECT predecessor_transition_audit.audit_sequence
                                FROM provider_discovery_audit_log AS predecessor_transition_audit
                                WHERE predecessor_transition_audit.session_id = predecessor.session_id
                                  AND predecessor_transition_audit.audit_kind = 'unknown_outcome_reconciled'
                                  AND predecessor_transition_audit.action_id = predecessor.action_id
                                  AND predecessor_transition_audit.subject_id = predecessor_event.id
                                  AND predecessor_transition_audit.session_revision = predecessor.resulting_revision
                                  AND predecessor_transition_audit.summary_key = 'discovery.audit.transition_applied'
                                  AND predecessor_transition_audit.created_at = predecessor.created_at
                            )
                      )
                )
          )
      )
      AND predecessor_start.commit_prepared_audit_sequence < COALESCE(
          (
              SELECT start_audit.audit_sequence
              FROM provider_discovery_audit_log AS start_audit
              WHERE start_audit.session_id = predecessor_operation.session_id
                AND start_audit.audit_kind = 'operation_started'
                AND start_audit.action_id = predecessor_operation.action_id
                AND start_audit.subject_id = predecessor_operation.id
                AND start_audit.session_revision
                    = predecessor_operation.expected_revision
                AND start_audit.summary_key = 'discovery.audit.operation_started'
                AND start_audit.created_at = predecessor_operation.started_at
          ),
          (
              SELECT terminal_transition_audit.audit_sequence
              FROM provider_discovery_audit_log AS terminal_transition_audit
              WHERE terminal_transition_audit.session_id = terminal_receipt.session_id
                AND terminal_transition_audit.audit_kind = 'transition_applied'
                AND terminal_transition_audit.action_id = terminal_receipt.action_id
                AND terminal_transition_audit.subject_id = terminal_receipt.event_id
                AND terminal_transition_audit.session_revision
                    = terminal_receipt.resulting_revision
                AND terminal_transition_audit.summary_key
                    = 'discovery.audit.transition_applied'
                AND terminal_transition_audit.created_at = terminal_receipt.created_at
          )
      )
      AND (
          SELECT terminal_interrupted_audit.audit_sequence
          FROM provider_discovery_audit_log AS terminal_interrupted_audit
          WHERE terminal_interrupted_audit.session_id = terminal_receipt.session_id
            AND terminal_interrupted_audit.audit_kind = 'operation_interrupted'
            AND terminal_interrupted_audit.action_id = terminal_receipt.action_id
            AND terminal_interrupted_audit.subject_id = predecessor_operation.id
            AND terminal_interrupted_audit.session_revision
                = terminal_receipt.resulting_revision
            AND terminal_interrupted_audit.summary_key
                = 'discovery.audit.operation_interrupted'
            AND terminal_interrupted_audit.created_at = terminal_receipt.created_at
      ) < restart_start.start_transition_audit_sequence
      AND restart_start.start_transition_audit_sequence
          < restart_start.commit_prepared_audit_sequence
      AND (
          predecessor.action_kind = 'interrupt'
          OR (
              (
                  SELECT terminal_interrupted_audit.audit_sequence
                  FROM provider_discovery_audit_log AS terminal_interrupted_audit
                  WHERE terminal_interrupted_audit.session_id = terminal_receipt.session_id
                    AND terminal_interrupted_audit.audit_kind = 'operation_interrupted'
                    AND terminal_interrupted_audit.action_id = terminal_receipt.action_id
                    AND terminal_interrupted_audit.subject_id = predecessor_operation.id
                    AND terminal_interrupted_audit.session_revision
                        = terminal_receipt.resulting_revision
                    AND terminal_interrupted_audit.summary_key
                        = 'discovery.audit.operation_interrupted'
                    AND terminal_interrupted_audit.created_at = terminal_receipt.created_at
              ) < (
                  SELECT resolution_transition_audit.audit_sequence
                  FROM provider_discovery_audit_log AS resolution_transition_audit
                  WHERE resolution_transition_audit.session_id = predecessor.session_id
                    AND resolution_transition_audit.audit_kind
                        = 'unknown_outcome_reconciled'
                    AND resolution_transition_audit.action_id = predecessor.action_id
                    AND resolution_transition_audit.subject_id = predecessor.event_id
                    AND resolution_transition_audit.session_revision
                        = predecessor.resulting_revision
                    AND resolution_transition_audit.summary_key
                        = 'discovery.audit.transition_applied'
                    AND resolution_transition_audit.created_at = predecessor.created_at
              )
              AND (
                  SELECT approval_audit.audit_sequence
                  FROM provider_discovery_approvals AS approval
                  JOIN provider_discovery_audit_log AS approval_audit
                    ON approval_audit.session_id = approval.session_id
                   AND approval_audit.audit_kind = 'approval_recorded'
                   AND approval_audit.action_id = predecessor.action_id
                   AND approval_audit.subject_id = approval.id
                   AND approval_audit.session_revision = predecessor.resulting_revision
                   AND approval_audit.summary_key = 'discovery.audit.approval_recorded'
                   AND approval_audit.created_at = predecessor.created_at
                  WHERE approval.session_id = predecessor.session_id
                    AND approval.approval_kind = 'unknown_outcome_resolution'
                    AND approval.candidate_id IS NULL
                    AND approval.decision = 'approved'
                    AND approval.session_revision = predecessor.expected_revision
                    AND approval.created_at = predecessor.created_at
                    AND approval.grant_sha256
                        = '710ffd1242a5df75c398580e1cec927bd1660bb1e55f035e1857d0924e949766'
                    AND approval.grant_json
                        = '{"kind":"unknown_outcome_resolution","operation":"atomic_commit","resolution":{"resolution":"confirmed_no_effect"}}'
              ) < restart_start.start_transition_audit_sequence
          )
      )
  );

-- Authorize native atomic-commit starts inductively. The root is the exact
-- approved-review operation. Every retry edge must consume an operation whose
-- own start is already in the set, so repeated restarts cannot manufacture a
-- detached intermediate operation. `UNION` plus strictly increasing revisions
-- makes malformed cycles finite and non-authoritative.
CREATE VIEW provider_discovery_authorized_native_commit_starts AS
WITH RECURSIVE authorized_starts(
    operation_id,
    session_id,
    start_action_id,
    start_action_kind,
    operation_expected_revision,
    commit_attempt_id,
    commit_plan_sha256,
    start_transition_audit_sequence,
    commit_prepared_audit_sequence
) AS (
    SELECT candidate.operation_id,
           candidate.session_id,
           candidate.start_action_id,
           candidate.start_action_kind,
           candidate.operation_expected_revision,
           candidate.commit_attempt_id,
           candidate.commit_plan_sha256,
           candidate.start_transition_audit_sequence,
           candidate.commit_prepared_audit_sequence
    FROM provider_discovery_native_commit_start_candidates AS candidate
    WHERE candidate.start_action_kind = 'approve_review'
      AND candidate.start_action_id = candidate.attempt_action_id
      AND candidate.start_expected_revision = candidate.attempt_expected_revision
      AND candidate.start_created_at = candidate.attempt_created_at
      AND candidate.credential_approval_id IS NOT NULL
      AND 1 = (
          SELECT COUNT(*)
          FROM provider_discovery_approvals AS review_approval
          WHERE review_approval.session_id = candidate.session_id
            AND review_approval.approval_kind = 'review'
            AND review_approval.candidate_id IS NULL
            AND review_approval.decision = 'approved'
            AND review_approval.session_revision = candidate.start_expected_revision
            AND review_approval.created_at = candidate.start_created_at
            AND review_approval.redaction_version = 1
            AND review_approval.grant_json = printf(
                '{"kind":"review","review_sha256":"%s","graph_sha256":"%s"}',
                candidate.review_sha256,
                candidate.graph_sha256
            )
            AND review_approval.grant_sha256
                = lorepia_sha256_hex(review_approval.grant_json)
            AND json_extract(
                review_approval.grant_json,
                '$.kind'
            ) = 'review'
            AND json_extract(
                review_approval.grant_json,
                '$.review_sha256'
            ) = candidate.review_sha256
            AND json_extract(
                review_approval.grant_json,
                '$.graph_sha256'
            ) = candidate.graph_sha256
      )
      AND 1 = (
          SELECT COUNT(*)
          FROM provider_discovery_approvals AS review_approval
          JOIN provider_discovery_audit_log AS approval_audit
            ON approval_audit.session_id = review_approval.session_id
           AND approval_audit.audit_kind = 'approval_recorded'
           AND approval_audit.action_id = candidate.start_action_id
           AND approval_audit.subject_id = review_approval.id
           AND approval_audit.session_revision = candidate.operation_expected_revision
           AND approval_audit.summary_key = 'discovery.audit.approval_recorded'
           AND approval_audit.created_at = candidate.start_created_at
          WHERE review_approval.session_id = candidate.session_id
            AND review_approval.approval_kind = 'review'
            AND review_approval.candidate_id IS NULL
            AND review_approval.decision = 'approved'
            AND review_approval.session_revision = candidate.start_expected_revision
            AND review_approval.created_at = candidate.start_created_at
            AND review_approval.redaction_version = 1
            AND review_approval.grant_json = printf(
                '{"kind":"review","review_sha256":"%s","graph_sha256":"%s"}',
                candidate.review_sha256,
                candidate.graph_sha256
            )
            AND review_approval.grant_sha256
                = lorepia_sha256_hex(review_approval.grant_json)
            AND candidate.start_transition_audit_sequence
                < approval_audit.audit_sequence
            AND approval_audit.audit_sequence
                < candidate.commit_prepared_audit_sequence
      )
      AND 1 = (
          SELECT COUNT(*)
          FROM provider_discovery_approvals AS credential_approval
          JOIN provider_discovery_audit_log AS approval_audit
            ON approval_audit.session_id = credential_approval.session_id
           AND approval_audit.audit_kind = 'approval_recorded'
           AND approval_audit.subject_id = credential_approval.id
           AND approval_audit.session_revision
               = credential_approval.session_revision + 1
           AND approval_audit.summary_key = 'discovery.audit.approval_recorded'
           AND approval_audit.created_at = credential_approval.created_at
          JOIN provider_discovery_action_receipts AS approval_receipt
            ON approval_receipt.session_id = credential_approval.session_id
           AND approval_receipt.action_id = approval_audit.action_id
          JOIN provider_discovery_event_outbox AS approval_event
            ON approval_event.id = approval_receipt.event_id
           AND approval_event.session_id = approval_receipt.session_id
          JOIN provider_discovery_audit_log AS approval_transition_audit
            ON approval_transition_audit.session_id = approval_receipt.session_id
           AND approval_transition_audit.audit_kind = 'transition_applied'
           AND approval_transition_audit.action_id = approval_receipt.action_id
           AND approval_transition_audit.subject_id = approval_event.id
           AND approval_transition_audit.session_revision
               = approval_receipt.resulting_revision
           AND approval_transition_audit.summary_key
               = 'discovery.audit.transition_applied'
           AND approval_transition_audit.created_at = approval_receipt.created_at
          WHERE credential_approval.id = candidate.credential_approval_id
            AND credential_approval.session_id = candidate.session_id
            AND credential_approval.approval_kind = 'credential_origin'
            AND credential_approval.candidate_id IS NULL
            AND credential_approval.decision = 'approved'
            AND credential_approval.session_revision
                < candidate.start_expected_revision
            AND credential_approval.created_at <= candidate.start_created_at
            AND credential_approval.redaction_version = 1
            AND json_extract(
                credential_approval.grant_json,
                '$.kind'
            ) = 'credential_origin'
            AND json_type(
                credential_approval.grant_json,
                '$.origin'
            ) = 'text'
            AND json_extract(
                credential_approval.grant_json,
                '$.origin'
            ) = lorepia_canonical_origin(json_extract(
                credential_approval.grant_json,
                '$.origin'
            ))
            AND json_type(
                credential_approval.grant_json,
                '$.auth_binding'
            ) = 'object'
            AND json_extract(
                credential_approval.grant_json,
                '$.auth_binding.kind'
            ) IN ('none', 'bearer_header', 'header_api_key')
            AND CASE json_extract(
                credential_approval.grant_json,
                '$.auth_binding.kind'
            )
                WHEN 'none' THEN 1 = (
                    SELECT COUNT(*)
                    FROM json_each(
                        credential_approval.grant_json,
                        '$.auth_binding'
                    )
                )
                WHEN 'bearer_header' THEN 1 = (
                    SELECT COUNT(*)
                    FROM json_each(
                        credential_approval.grant_json,
                        '$.auth_binding'
                    )
                )
                WHEN 'header_api_key' THEN
                    json_type(
                        credential_approval.grant_json,
                        '$.auth_binding.header_name'
                    ) = 'text'
                    AND json_extract(
                        credential_approval.grant_json,
                        '$.auth_binding.header_name'
                    ) <> ''
                    AND json_extract(
                        credential_approval.grant_json,
                        '$.auth_binding.header_name'
                    ) = lorepia_header_name(json_extract(
                        credential_approval.grant_json,
                        '$.auth_binding.header_name'
                    ))
                    AND json_extract(
                        credential_approval.grant_json,
                        '$.auth_binding.header_name'
                    ) NOT GLOB '*[^!#$%&''*+.^_`|~0-9a-z-]*'
                    AND 2 = (
                        SELECT COUNT(*)
                        FROM json_each(
                            credential_approval.grant_json,
                            '$.auth_binding'
                        )
                    )
                ELSE 0
            END
            AND json_extract(
                credential_approval.grant_json,
                '$.manifest_sha256'
            ) = candidate.manifest_sha256
            AND 4 = (
                SELECT COUNT(*)
                FROM json_each(credential_approval.grant_json)
            )
            AND credential_approval.grant_json = CASE json_extract(
                credential_approval.grant_json,
                '$.auth_binding.kind'
            )
                WHEN 'none' THEN printf(
                    '{"kind":"credential_origin","origin":%s,"auth_binding":{"kind":"none"},"manifest_sha256":%s}',
                    json_quote(json_extract(
                        credential_approval.grant_json,
                        '$.origin'
                    )),
                    json_quote(candidate.manifest_sha256)
                )
                WHEN 'bearer_header' THEN printf(
                    '{"kind":"credential_origin","origin":%s,"auth_binding":{"kind":"bearer_header"},"manifest_sha256":%s}',
                    json_quote(json_extract(
                        credential_approval.grant_json,
                        '$.origin'
                    )),
                    json_quote(candidate.manifest_sha256)
                )
                WHEN 'header_api_key' THEN printf(
                    '{"kind":"credential_origin","origin":%s,"auth_binding":{"kind":"header_api_key","header_name":%s},"manifest_sha256":%s}',
                    json_quote(json_extract(
                        credential_approval.grant_json,
                        '$.origin'
                    )),
                    json_quote(json_extract(
                        credential_approval.grant_json,
                        '$.auth_binding.header_name'
                    )),
                    json_quote(candidate.manifest_sha256)
                )
                ELSE NULL
            END
            AND length(credential_approval.grant_sha256) = 64
            AND credential_approval.grant_sha256
                NOT GLOB '*[^0-9a-f]*'
            AND credential_approval.grant_sha256
                = lorepia_sha256_hex(credential_approval.grant_json)
            AND approval_receipt.action_kind = 'approve_credential_origin'
            AND approval_receipt.outcome = 'applied'
            AND approval_receipt.redaction_version = 1
            AND approval_receipt.expected_revision
                = credential_approval.session_revision
            AND approval_receipt.resulting_revision
                = credential_approval.session_revision + 1
            AND approval_receipt.created_at = credential_approval.created_at
            AND approval_event.sequence = approval_receipt.event_sequence
            AND approval_event.event_version = 2
            AND approval_event.session_revision
                = approval_receipt.resulting_revision
            AND approval_event.state = 'listing_models'
            AND approval_event.redaction_version = 1
            AND approval_event.created_at = approval_receipt.created_at
            AND json_type(
                approval_receipt.response_json,
                '$.session.failure'
            ) = 'null'
            AND json_type(
                approval_event.event_json,
                '$.failure'
            ) = 'null'
            AND json_extract(
                approval_receipt.response_json,
                '$.previous_revision'
            ) = approval_receipt.expected_revision
            AND json_extract(
                approval_receipt.response_json,
                '$.session.id'
            ) = candidate.session_id
            AND json_extract(
                approval_receipt.response_json,
                '$.session.state'
            ) = 'listing_models'
            AND json_extract(
                approval_receipt.response_json,
                '$.session.revision'
            ) = approval_receipt.resulting_revision
            AND json_extract(
                approval_receipt.response_json,
                '$.session.next_event_sequence'
            ) = approval_receipt.event_sequence + 1
            AND json_extract(
                approval_receipt.response_json,
                '$.effect.effect'
            ) = 'list_models'
            AND json_extract(
                approval_receipt.response_json,
                '$.receipt.action_id'
            ) = approval_receipt.action_id
            AND json_extract(
                approval_receipt.response_json,
                '$.receipt.session_id'
            ) = approval_receipt.session_id
            AND json_extract(
                approval_receipt.response_json,
                '$.receipt.action_kind'
            ) = approval_receipt.action_kind
            AND json_extract(
                approval_receipt.response_json,
                '$.receipt.request_sha256'
            ) = approval_receipt.request_sha256
            AND json_extract(
                approval_receipt.response_json,
                '$.receipt.expected_revision'
            ) = approval_receipt.expected_revision
            AND json_extract(
                approval_receipt.response_json,
                '$.receipt.resulting_revision'
            ) = approval_receipt.resulting_revision
            AND json_extract(
                approval_receipt.response_json,
                '$.receipt.event_sequence'
            ) = approval_receipt.event_sequence
            AND json_extract(
                approval_receipt.response_json,
                '$.receipt.outcome'
            ) = approval_receipt.outcome
            AND json_extract(
                approval_receipt.response_json,
                '$.event.id'
            ) = approval_event.id
            AND json_extract(
                approval_receipt.response_json,
                '$.event.session_id'
            ) = approval_event.session_id
            AND json_extract(
                approval_receipt.response_json,
                '$.event.version'
            ) = approval_event.event_version
            AND json_extract(
                approval_receipt.response_json,
                '$.event.sequence'
            ) = approval_event.sequence
            AND json_extract(
                approval_receipt.response_json,
                '$.event.session_revision'
            ) = approval_event.session_revision
            AND json_extract(
                approval_receipt.response_json,
                '$.event.state'
            ) = approval_event.state
            AND json_extract(
                approval_receipt.response_json,
                '$.event.action_id'
            ) = approval_receipt.action_id
            AND json_extract(
                approval_receipt.response_json,
                '$.event'
            ) = approval_event.event_json
            AND json_extract(
                approval_event.event_json,
                '$.id'
            ) = approval_event.id
            AND json_extract(
                approval_event.event_json,
                '$.session_id'
            ) = approval_event.session_id
            AND json_extract(
                approval_event.event_json,
                '$.version'
            ) = approval_event.event_version
            AND json_extract(
                approval_event.event_json,
                '$.sequence'
            ) = approval_event.sequence
            AND json_extract(
                approval_event.event_json,
                '$.session_revision'
            ) = approval_event.session_revision
            AND json_extract(
                approval_event.event_json,
                '$.state'
            ) = approval_event.state
            AND json_extract(
                approval_event.event_json,
                '$.action_id'
            ) = approval_receipt.action_id
            AND approval_transition_audit.audit_sequence
                < approval_audit.audit_sequence
            AND approval_audit.audit_sequence
                < candidate.start_transition_audit_sequence
      )
    UNION
    SELECT restart_start.operation_id,
           restart_start.session_id,
           restart_start.start_action_id,
           restart_start.start_action_kind,
           restart_start.operation_expected_revision,
           restart_start.commit_attempt_id,
           restart_start.commit_plan_sha256,
           restart_start.start_transition_audit_sequence,
           restart_start.commit_prepared_audit_sequence
    FROM authorized_starts AS prior_start
    JOIN provider_discovery_native_retry_start_candidates AS retry_edge
      ON retry_edge.predecessor_operation_id = prior_start.operation_id
     AND retry_edge.predecessor_start_action_id = prior_start.start_action_id
     AND retry_edge.predecessor_operation_expected_revision
         = prior_start.operation_expected_revision
     AND retry_edge.predecessor_commit_audit_sequence
         = prior_start.commit_prepared_audit_sequence
     AND retry_edge.session_id = prior_start.session_id
     AND retry_edge.commit_attempt_id = prior_start.commit_attempt_id
     AND retry_edge.commit_plan_sha256 = prior_start.commit_plan_sha256
    JOIN provider_discovery_native_commit_start_candidates AS restart_start
      ON restart_start.operation_id = retry_edge.restart_operation_id
     AND restart_start.start_action_id = retry_edge.restart_action_id
     AND restart_start.operation_expected_revision
         = retry_edge.restart_operation_expected_revision
     AND restart_start.session_id = retry_edge.session_id
     AND restart_start.commit_attempt_id = retry_edge.commit_attempt_id
     AND restart_start.commit_plan_sha256 = retry_edge.commit_plan_sha256
    WHERE restart_start.start_action_kind = 'restart_interrupted'
      AND restart_start.operation_expected_revision
          > prior_start.operation_expected_revision
)
SELECT operation_id,
       session_id,
       start_action_id,
       start_action_kind,
       operation_expected_revision,
       commit_attempt_id,
       commit_plan_sha256,
       start_transition_audit_sequence,
       commit_prepared_audit_sequence
FROM authorized_starts;

-- A version-36 database can legitimately reach the native store cutpoint
-- before it knows how to persist a physical execution authority. Snapshot
-- only the exact, still-active Started lineage present at this migration
-- cutpoint. These rows authorize one conservative transition to
-- OutcomeUnknown; they never authorize completion, credential ownership, or
-- adoption, and deliberately contain no synthetic physical authority.
CREATE TABLE provider_discovery_native_credential_legacy_started_cutoff_snapshots (
    operation_id TEXT NOT NULL PRIMARY KEY
        REFERENCES provider_discovery_operations(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    session_id TEXT NOT NULL
        REFERENCES provider_discovery_sessions(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    commit_attempt_id TEXT NOT NULL
        REFERENCES provider_discovery_commit_attempts(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    commit_plan_sha256 TEXT NOT NULL CHECK (
        length(commit_plan_sha256) = 64
        AND commit_plan_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    connection_id TEXT NOT NULL CHECK (
        length(trim(connection_id)) BETWEEN 1 AND 128
    ),
    session_cancellation_pending INTEGER NOT NULL CHECK (
        session_cancellation_pending IN (0, 1)
    ),
    session_revision_at_cutoff INTEGER NOT NULL CHECK (
        session_revision_at_cutoff >= 0
    ),
    session_next_event_sequence_at_cutoff INTEGER NOT NULL CHECK (
        session_next_event_sequence_at_cutoff > 0
    ),
    start_action_id TEXT NOT NULL CHECK (length(trim(start_action_id)) > 0),
    start_action_kind TEXT NOT NULL CHECK (
        start_action_kind IN ('approve_review', 'restart_interrupted')
    ),
    request_sha256 TEXT NOT NULL CHECK (
        length(request_sha256) = 64
        AND request_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    operation_expected_revision INTEGER NOT NULL CHECK (
        operation_expected_revision >= 0
    ),
    start_transition_audit_sequence INTEGER NOT NULL CHECK (
        start_transition_audit_sequence > 0
    ),
    commit_prepared_audit_sequence INTEGER NOT NULL CHECK (
        commit_prepared_audit_sequence > 0
    ),
    operation_created_at TEXT NOT NULL CHECK (
        length(trim(operation_created_at)) > 0
    ),
    operation_started_at TEXT NOT NULL CHECK (
        length(trim(operation_started_at)) > 0
    ),
    cutoff_before_schema_version INTEGER NOT NULL CHECK (
        cutoff_before_schema_version = 37
    ),
    snapshot_schema_version INTEGER NOT NULL CHECK (snapshot_schema_version = 1),
    redaction_version INTEGER NOT NULL CHECK (redaction_version = 1),
    UNIQUE (session_id, start_action_id),
    UNIQUE (session_id, commit_attempt_id, operation_id),
    CHECK (
        (session_cancellation_pending = 0
          AND session_revision_at_cutoff = operation_expected_revision)
        OR (session_cancellation_pending = 1
          AND session_revision_at_cutoff > operation_expected_revision)
    )
);

INSERT INTO provider_discovery_native_credential_legacy_started_cutoff_snapshots (
    operation_id,
    session_id,
    commit_attempt_id,
    commit_plan_sha256,
    connection_id,
    session_cancellation_pending,
    session_revision_at_cutoff,
    session_next_event_sequence_at_cutoff,
    start_action_id,
    start_action_kind,
    request_sha256,
    operation_expected_revision,
    start_transition_audit_sequence,
    commit_prepared_audit_sequence,
    operation_created_at,
    operation_started_at,
    cutoff_before_schema_version,
    snapshot_schema_version,
    redaction_version
)
SELECT operation.id,
       operation.session_id,
       attempt.id,
       attempt.plan_sha256,
       json_extract(attempt.plan_json, '$.connection_id'),
       session.cancellation_pending,
       session.revision,
       session.next_event_sequence,
       receipt.action_id,
       receipt.action_kind,
       operation.request_sha256,
       operation.expected_revision,
       transition_audit.audit_sequence,
       commit_audit.audit_sequence,
       operation.created_at,
       operation.started_at,
       37,
       1,
       1
FROM provider_discovery_operations AS operation
JOIN provider_discovery_sessions AS session
  ON session.id = operation.session_id
JOIN provider_discovery_action_receipts AS receipt
  ON receipt.session_id = operation.session_id
 AND receipt.action_id = operation.action_id
JOIN provider_discovery_event_outbox AS event
  ON event.id = receipt.event_id
 AND event.session_id = receipt.session_id
JOIN provider_discovery_commit_attempts AS attempt
  ON attempt.id = session.commit_attempt_id
 AND attempt.session_id = session.id
 AND attempt.plan_sha256 = session.commit_plan_sha256
 AND attempt.id = json_extract(
     receipt.response_json,
     '$.session.commit_attempt_id'
 )
 AND attempt.plan_sha256 = json_extract(
     receipt.response_json,
     '$.session.commit_plan_sha256'
 )
JOIN provider_discovery_audit_log AS transition_audit
  ON transition_audit.session_id = receipt.session_id
 AND transition_audit.audit_kind = 'transition_applied'
 AND transition_audit.action_id = receipt.action_id
 AND transition_audit.subject_id = event.id
 AND transition_audit.session_revision = receipt.resulting_revision
 AND transition_audit.summary_key = 'discovery.audit.transition_applied'
 AND transition_audit.created_at = receipt.created_at
JOIN provider_discovery_audit_log AS commit_audit
  ON commit_audit.session_id = receipt.session_id
 AND commit_audit.audit_kind = 'commit_prepared'
 AND commit_audit.action_id = receipt.action_id
 AND commit_audit.subject_id = attempt.id
 AND commit_audit.session_revision = receipt.resulting_revision
 AND commit_audit.summary_key = 'discovery.audit.commit_prepared'
 AND commit_audit.created_at = receipt.created_at
JOIN provider_discovery_audit_log AS started_audit
  ON started_audit.session_id = operation.session_id
 AND started_audit.audit_kind = 'operation_started'
 AND started_audit.action_id = operation.action_id
 AND started_audit.subject_id = operation.id
 AND started_audit.session_revision = operation.expected_revision
 AND started_audit.summary_key = 'discovery.audit.operation_started'
 AND started_audit.created_at = operation.started_at
WHERE operation.operation_kind = 'atomic_commit'
  AND operation.side_effect_class = 'persistent'
  AND operation.status = 'started'
  AND operation.started_at IS NOT NULL
  AND operation.finished_at IS NULL
  AND session.state = 'committing'
  AND session.active_operation_id = operation.id
  AND session.commit_attempt_id = attempt.id
  AND session.commit_plan_sha256 = attempt.plan_sha256
  AND (
      (session.cancellation_pending = 0
       AND session.revision = operation.expected_revision)
      OR (
          session.cancellation_pending = 1
          AND session.revision > operation.expected_revision
          AND (
              SELECT COUNT(*)
              FROM provider_discovery_action_receipts AS cancellation
              WHERE cancellation.session_id = operation.session_id
                AND cancellation.resulting_revision
                    > operation.expected_revision
                AND cancellation.resulting_revision <= session.revision
          ) = session.revision - operation.expected_revision
          AND session.next_event_sequence = (
              SELECT last_cancellation.event_sequence + 1
              FROM provider_discovery_action_receipts AS last_cancellation
              WHERE last_cancellation.session_id = operation.session_id
                AND last_cancellation.resulting_revision = session.revision
          )
          AND session.updated_at = (
              SELECT last_cancellation.created_at
              FROM provider_discovery_action_receipts AS last_cancellation
              WHERE last_cancellation.session_id = operation.session_id
                AND last_cancellation.resulting_revision = session.revision
          )
          AND NOT EXISTS (
              SELECT 1
              FROM provider_discovery_action_receipts AS cancellation
              WHERE cancellation.session_id = operation.session_id
                AND cancellation.resulting_revision
                    > operation.expected_revision
                AND cancellation.resulting_revision <= session.revision
                AND NOT COALESCE(
                    cancellation.action_kind = 'cancel'
                    AND cancellation.outcome = 'applied'
                    AND cancellation.resulting_revision
                        = cancellation.expected_revision + 1
                    AND cancellation.redaction_version = 1
                    AND json_extract(
                        cancellation.response_json,
                        '$.previous_revision'
                    ) = cancellation.expected_revision
                    AND json_extract(
                        cancellation.response_json,
                        '$.session.id'
                    ) = operation.session_id
                    AND json_extract(
                        cancellation.response_json,
                        '$.session.state'
                    ) = 'committing'
                    AND json_extract(
                        cancellation.response_json,
                        '$.session.revision'
                    ) = cancellation.resulting_revision
                    AND json_extract(
                        cancellation.response_json,
                        '$.session.next_event_sequence'
                    ) = cancellation.event_sequence + 1
                    AND json_extract(
                        cancellation.response_json,
                        '$.session.commit_attempt_id'
                    ) = attempt.id
                    AND json_extract(
                        cancellation.response_json,
                        '$.session.commit_plan_sha256'
                    ) = attempt.plan_sha256
                    AND json_extract(
                        cancellation.response_json,
                        '$.session.input.connection_id'
                    ) = json_extract(attempt.plan_json, '$.connection_id')
                    AND json_extract(
                        cancellation.response_json,
                        '$.session.input.credential_ref'
                    ) = json_extract(attempt.plan_json, '$.credential_ref')
                    AND json_extract(
                        cancellation.response_json,
                        '$.session.cancellation_pending'
                    ) = 1
                    AND json_type(
                        cancellation.response_json,
                        '$.session.recovery'
                    ) = 'null'
                    AND json_type(
                        cancellation.response_json,
                        '$.session.unknown_operation'
                    ) = 'null'
                    AND json_type(
                        cancellation.response_json,
                        '$.session.failure'
                    ) = 'null'
                    AND (
                        (
                            cancellation.expected_revision
                                = operation.expected_revision
                            AND json_extract(
                                cancellation.response_json,
                                '$.effect.effect'
                            ) = 'request_cancellation'
                            AND json_extract(
                                cancellation.response_json,
                                '$.effect.operation'
                            ) = 'atomic_commit'
                        )
                        OR (
                            cancellation.expected_revision
                                > operation.expected_revision
                            AND json_extract(
                                cancellation.response_json,
                                '$.effect.effect'
                            ) = 'none'
                        )
                    )
                    AND EXISTS (
                        SELECT 1
                        FROM provider_discovery_action_receipts AS predecessor
                        WHERE predecessor.session_id = cancellation.session_id
                          AND predecessor.resulting_revision
                              = cancellation.expected_revision
                          AND predecessor.event_sequence + 1
                              = cancellation.event_sequence
                          AND julianday(predecessor.created_at)
                              <= julianday(cancellation.created_at)
                    )
                    AND EXISTS (
                        SELECT 1
                        FROM provider_discovery_event_outbox AS cancellation_event
                        WHERE cancellation_event.id = cancellation.event_id
                          AND cancellation_event.session_id
                              = cancellation.session_id
                          AND cancellation_event.sequence
                              = cancellation.event_sequence
                          AND cancellation_event.event_version = 2
                          AND cancellation_event.session_revision
                              = cancellation.resulting_revision
                          AND cancellation_event.state = 'committing'
                          AND cancellation_event.redaction_version = 1
                          AND cancellation_event.created_at
                              = cancellation.created_at
                          AND json_extract(
                              cancellation.response_json,
                              '$.event'
                          ) = cancellation_event.event_json
                    )
                    AND EXISTS (
                        SELECT 1
                        FROM provider_discovery_audit_log AS cancellation_audit
                        WHERE cancellation_audit.session_id
                              = cancellation.session_id
                          AND cancellation_audit.audit_kind = 'transition_applied'
                          AND cancellation_audit.action_id
                              = cancellation.action_id
                          AND cancellation_audit.subject_id
                              = cancellation.event_id
                          AND cancellation_audit.session_revision
                              = cancellation.resulting_revision
                          AND cancellation_audit.summary_key
                              = 'discovery.audit.transition_applied'
                          AND cancellation_audit.created_at
                              = cancellation.created_at
                    ),
                    0
                )
          )
      )
  )
  AND attempt.phase = 'prepared'
  AND attempt.redaction_version = 1
  AND operation.action_id = receipt.action_id
  AND operation.expected_revision = receipt.resulting_revision
  AND operation.request_sha256 = receipt.request_sha256
  AND operation.created_at = receipt.created_at
  AND receipt.action_kind IN ('approve_review', 'restart_interrupted')
  AND receipt.outcome = 'applied'
  AND receipt.resulting_revision = receipt.expected_revision + 1
  AND receipt.redaction_version = 1
  AND event.sequence = receipt.event_sequence
  AND event.event_version = 2
  AND event.session_revision = receipt.resulting_revision
  AND event.state = 'committing'
  AND event.redaction_version = 1
  AND event.created_at = receipt.created_at
  AND transition_audit.audit_sequence < commit_audit.audit_sequence
  AND commit_audit.audit_sequence < started_audit.audit_sequence
  AND json_extract(receipt.response_json, '$.session.id') = session.id
  AND json_extract(receipt.response_json, '$.session.state') = 'committing'
  AND json_extract(receipt.response_json, '$.session.revision')
      = operation.expected_revision
  AND json_extract(
      receipt.response_json,
      '$.session.cancellation_pending'
  ) = 0
  AND json_extract(attempt.plan_json, '$.connection_id')
      = json_extract(attempt.plan_json, '$.credential_ref')
  AND length(trim(json_extract(attempt.plan_json, '$.connection_id')))
      BETWEEN 1 AND 128;

CREATE TRIGGER provider_discovery_native_credential_legacy_started_cutoff_no_insert
BEFORE INSERT ON provider_discovery_native_credential_legacy_started_cutoff_snapshots
BEGIN
    SELECT RAISE(ABORT, 'legacy native Started cutoff is sealed');
END;

CREATE TRIGGER provider_discovery_native_credential_legacy_started_cutoff_no_update
BEFORE UPDATE ON provider_discovery_native_credential_legacy_started_cutoff_snapshots
BEGIN
    SELECT RAISE(ABORT, 'legacy native Started cutoff is immutable');
END;

CREATE TRIGGER provider_discovery_native_credential_legacy_started_cutoff_no_delete
BEFORE DELETE ON provider_discovery_native_credential_legacy_started_cutoff_snapshots
BEGIN
    SELECT RAISE(ABORT, 'legacy native Started cutoff is immutable');
END;

-- A native atomic commit whose external outcome became unknown may become a
-- credential authority only when an exact approved reconciliation confirms
-- that very operation completed. The reusable attempt id is never the
-- physical authority: this view deliberately returns the original operation
-- id and seals its start, unknown terminal, approval, graph, and Ready receipt.
CREATE VIEW provider_discovery_authorized_confirmed_commit_completions AS
WITH exact_applied_receipts AS (
    SELECT receipt.action_id,
           receipt.session_id,
           receipt.action_kind,
           receipt.request_sha256,
           receipt.expected_revision,
           receipt.resulting_revision,
           receipt.event_sequence,
           receipt.created_at,
           receipt.response_json,
           event.id AS event_id,
           event.state AS event_state,
           event.event_json
    FROM provider_discovery_action_receipts AS receipt
    JOIN provider_discovery_event_outbox AS event
      ON event.id = receipt.event_id
     AND event.session_id = receipt.session_id
    WHERE receipt.outcome = 'applied'
      AND receipt.resulting_revision = receipt.expected_revision + 1
      AND receipt.redaction_version = 1
      AND event.sequence = receipt.event_sequence
      AND event.event_version = 2
      AND event.session_revision = receipt.resulting_revision
      AND event.redaction_version = 1
      AND event.created_at = receipt.created_at
      AND json_extract(receipt.response_json, '$.previous_revision') = receipt.expected_revision
      AND json_extract(receipt.response_json, '$.session.id') = receipt.session_id
      AND json_extract(receipt.response_json, '$.session.revision') = receipt.resulting_revision
      AND json_extract(receipt.response_json, '$.session.next_event_sequence')
          = receipt.event_sequence + 1
      AND json_extract(receipt.response_json, '$.session.state') = event.state
      AND json_extract(receipt.response_json, '$.receipt.action_id') = receipt.action_id
      AND json_extract(receipt.response_json, '$.receipt.session_id') = receipt.session_id
      AND json_extract(receipt.response_json, '$.receipt.action_kind') = receipt.action_kind
      AND json_extract(receipt.response_json, '$.receipt.request_sha256') = receipt.request_sha256
      AND json_extract(receipt.response_json, '$.receipt.expected_revision')
          = receipt.expected_revision
      AND json_extract(receipt.response_json, '$.receipt.resulting_revision')
          = receipt.resulting_revision
      AND json_extract(receipt.response_json, '$.receipt.event_sequence')
          = receipt.event_sequence
      AND json_extract(receipt.response_json, '$.receipt.outcome') = receipt.outcome
      AND json_extract(receipt.response_json, '$.event') = event.event_json
      AND json_extract(event.event_json, '$.id') = event.id
      AND json_extract(event.event_json, '$.session_id') = event.session_id
      AND json_extract(event.event_json, '$.version') = event.event_version
      AND json_extract(event.event_json, '$.sequence') = event.sequence
      AND json_extract(event.event_json, '$.session_revision') = event.session_revision
      AND json_extract(event.event_json, '$.state') = event.state
      AND json_extract(event.event_json, '$.action_id') = receipt.action_id
      AND json_type(receipt.response_json, '$.session.failure') = 'null'
      AND json_type(event.event_json, '$.failure') = 'null'
)
SELECT operation.id AS operation_id,
       operation.session_id AS session_id,
       attempt.id AS commit_attempt_id,
       attempt.plan_sha256 AS commit_plan_sha256,
       json_extract(attempt.plan_json, '$.connection_id') AS connection_id,
       session.revision AS ready_revision,
       attempt.completed_at AS completed_at
FROM provider_discovery_operations AS operation
JOIN provider_discovery_authorized_native_commit_starts AS authorized
  ON authorized.operation_id = operation.id
 AND authorized.session_id = operation.session_id
 AND authorized.operation_expected_revision = operation.expected_revision
JOIN provider_discovery_native_commit_start_candidates AS start_candidate
  ON start_candidate.operation_id = authorized.operation_id
 AND start_candidate.session_id = authorized.session_id
 AND start_candidate.start_action_id = authorized.start_action_id
 AND start_candidate.commit_attempt_id = authorized.commit_attempt_id
 AND start_candidate.commit_plan_sha256 = authorized.commit_plan_sha256
JOIN provider_discovery_sessions AS session
  ON session.id = operation.session_id
JOIN provider_discovery_commit_attempts AS attempt
  ON attempt.id = session.commit_attempt_id
 AND attempt.session_id = session.id
 AND attempt.id = authorized.commit_attempt_id
 AND attempt.plan_sha256 = authorized.commit_plan_sha256
JOIN exact_applied_receipts AS unknown_receipt
  ON unknown_receipt.session_id = operation.session_id
 AND unknown_receipt.expected_revision = operation.expected_revision
 AND unknown_receipt.resulting_revision = operation.expected_revision + 1
JOIN exact_applied_receipts AS resolution_receipt
  ON resolution_receipt.session_id = unknown_receipt.session_id
 AND resolution_receipt.expected_revision = unknown_receipt.resulting_revision
 AND resolution_receipt.resulting_revision = unknown_receipt.resulting_revision + 1
JOIN provider_discovery_approvals AS resolution_approval
  ON resolution_approval.session_id = resolution_receipt.session_id
 AND resolution_approval.approval_kind = 'unknown_outcome_resolution'
 AND resolution_approval.candidate_id IS NULL
 AND resolution_approval.decision = 'approved'
 AND resolution_approval.session_revision = resolution_receipt.expected_revision
 AND resolution_approval.created_at = resolution_receipt.created_at
 AND resolution_approval.redaction_version = 1
JOIN provider_discovery_audit_log AS operation_started
  ON operation_started.session_id = operation.session_id
 AND operation_started.audit_kind = 'operation_started'
 AND operation_started.action_id = operation.action_id
 AND operation_started.subject_id = operation.id
 AND operation_started.session_revision = operation.expected_revision
 AND operation_started.summary_key = 'discovery.audit.operation_started'
 AND operation_started.created_at = operation.started_at
JOIN provider_discovery_audit_log AS graph_applied
  ON graph_applied.session_id = operation.session_id
 AND graph_applied.audit_kind = 'transition_applied'
 AND graph_applied.action_id IS NULL
 AND graph_applied.subject_id = json_extract(attempt.plan_json, '$.graph_sha256')
 AND graph_applied.session_revision = operation.expected_revision
 AND graph_applied.summary_key = 'discovery.audit.provider_graph_applied'
JOIN provider_discovery_audit_log AS template_owned
  ON template_owned.session_id = operation.session_id
 AND template_owned.audit_kind = 'transition_applied'
 AND template_owned.action_id IS NULL
 AND template_owned.subject_id IN ('created', 'reused')
 AND template_owned.session_revision = graph_applied.session_revision
 AND template_owned.summary_key = 'discovery.audit.provider_template_ownership'
 AND template_owned.created_at = graph_applied.created_at
JOIN provider_discovery_audit_log AS unknown_transition
  ON unknown_transition.session_id = unknown_receipt.session_id
 AND unknown_transition.audit_kind = 'transition_applied'
 AND unknown_transition.action_id = unknown_receipt.action_id
 AND unknown_transition.subject_id = unknown_receipt.event_id
 AND unknown_transition.session_revision = unknown_receipt.resulting_revision
 AND unknown_transition.summary_key = 'discovery.audit.transition_applied'
 AND unknown_transition.created_at = unknown_receipt.created_at
JOIN provider_discovery_audit_log AS operation_interrupted
  ON operation_interrupted.session_id = unknown_receipt.session_id
 AND operation_interrupted.audit_kind = 'operation_interrupted'
 AND operation_interrupted.action_id = unknown_receipt.action_id
 AND operation_interrupted.subject_id = operation.id
 AND operation_interrupted.session_revision = unknown_receipt.resulting_revision
 AND operation_interrupted.summary_key = 'discovery.audit.operation_interrupted'
 AND operation_interrupted.created_at = unknown_receipt.created_at
JOIN provider_discovery_audit_log AS resolution_transition
  ON resolution_transition.session_id = resolution_receipt.session_id
 AND resolution_transition.audit_kind = 'unknown_outcome_reconciled'
 AND resolution_transition.action_id = resolution_receipt.action_id
 AND resolution_transition.subject_id = resolution_receipt.event_id
 AND resolution_transition.session_revision = resolution_receipt.resulting_revision
 AND resolution_transition.summary_key = 'discovery.audit.transition_applied'
 AND resolution_transition.created_at = resolution_receipt.created_at
JOIN provider_discovery_audit_log AS approval_recorded
  ON approval_recorded.session_id = resolution_approval.session_id
 AND approval_recorded.audit_kind = 'approval_recorded'
 AND approval_recorded.action_id = resolution_receipt.action_id
 AND approval_recorded.subject_id = resolution_approval.id
 AND approval_recorded.session_revision = resolution_receipt.resulting_revision
 AND approval_recorded.summary_key = 'discovery.audit.approval_recorded'
 AND approval_recorded.created_at = resolution_receipt.created_at
WHERE operation.operation_kind = 'atomic_commit'
  AND operation.side_effect_class = 'persistent'
  AND operation.status = 'outcome_unknown'
  AND operation.approval_id IS NULL
  AND operation.approval_grant_sha256 IS NULL
  AND operation.started_at IS NOT NULL
  AND operation.finished_at IS NOT NULL
  AND operation.updated_at = operation.finished_at
  AND julianday(operation.created_at) IS NOT NULL
  AND julianday(operation.started_at) IS NOT NULL
  AND julianday(operation.finished_at) IS NOT NULL
  AND julianday(operation.created_at) <= julianday(operation.started_at)
  AND julianday(operation.started_at) <= julianday(operation.finished_at)
  AND operation.finished_at = unknown_receipt.created_at
  AND unknown_receipt.action_kind IN ('interrupt', 'external_outcome_became_unknown')
  AND unknown_receipt.event_sequence = start_candidate.start_event_sequence + 1
  AND unknown_receipt.event_state = 'unknown_outcome'
  AND json_extract(unknown_receipt.response_json, '$.session.input')
      = session.sanitized_input_json
  AND json_extract(unknown_receipt.response_json, '$.session.commit_attempt_id') = attempt.id
  AND json_extract(unknown_receipt.response_json, '$.session.commit_plan_sha256')
      = attempt.plan_sha256
  AND json_extract(unknown_receipt.response_json, '$.session.manifest_sha256')
      = json_extract(attempt.plan_json, '$.manifest_sha256')
  AND json_extract(unknown_receipt.response_json, '$.session.unknown_operation')
      = 'atomic_commit'
  AND json_type(unknown_receipt.response_json, '$.session.recovery') = 'null'
  AND json_type(unknown_receipt.response_json, '$.session.committed_connection_id') = 'null'
  AND json_extract(unknown_receipt.response_json, '$.session.cancellation_pending') = 0
  AND json_type(unknown_receipt.response_json, '$.session.active_effect_approval') = 'null'
  AND json_extract(unknown_receipt.response_json, '$.effect.effect') = 'none'
  AND json_extract(unknown_receipt.event_json, '$.action_required.kind')
      = 'reconcile_unknown_outcome'
  AND json_extract(unknown_receipt.event_json, '$.action_required.operation')
      = 'atomic_commit'
  AND json_extract(unknown_receipt.event_json, '$.warning') = 'unknown_external_outcome'
  AND resolution_receipt.action_kind = 'resolve_unknown_outcome'
  AND resolution_receipt.event_sequence = unknown_receipt.event_sequence + 1
  AND resolution_receipt.event_state = 'ready'
  AND resolution_receipt.created_at = attempt.completed_at
  AND json_extract(resolution_receipt.response_json, '$.session.input')
      = session.sanitized_input_json
  AND json_extract(resolution_receipt.response_json, '$.session.commit_attempt_id') = attempt.id
  AND json_extract(resolution_receipt.response_json, '$.session.commit_plan_sha256')
      = attempt.plan_sha256
  AND json_extract(resolution_receipt.response_json, '$.session.manifest_sha256')
      = session.manifest_sha256
  AND json_extract(resolution_receipt.response_json, '$.session.committed_connection_id')
      = json_extract(attempt.plan_json, '$.connection_id')
  AND json_type(resolution_receipt.response_json, '$.session.unknown_operation') = 'null'
  AND json_type(resolution_receipt.response_json, '$.session.recovery') = 'null'
  AND json_type(resolution_receipt.response_json, '$.session.active_effect_approval') = 'null'
  AND json_extract(resolution_receipt.response_json, '$.session.cancellation_pending') = 0
  AND json_extract(resolution_receipt.response_json, '$.effect.effect') = 'none'
  AND json_type(resolution_receipt.event_json, '$.action_required') = 'null'
  AND json_type(resolution_receipt.event_json, '$.progress') = 'null'
  AND json_type(resolution_receipt.event_json, '$.warning') = 'null'
  AND session.state = 'ready'
  AND session.revision = resolution_receipt.resulting_revision
  AND session.next_event_sequence = resolution_receipt.event_sequence + 1
  AND session.commit_attempt_id = attempt.id
  AND session.commit_plan_sha256 = attempt.plan_sha256
  AND session.manifest_sha256 = json_extract(attempt.plan_json, '$.manifest_sha256')
  AND session.committed_connection_id = json_extract(attempt.plan_json, '$.connection_id')
  AND session.active_operation_id IS NULL
  AND session.recovery_json IS NULL
  AND session.unknown_operation IS NULL
  AND session.error_json IS NULL
  AND session.active_effect_approval_json IS NULL
  AND session.cancellation_pending = 0
  AND session.redaction_version = 1
  AND session.updated_at = resolution_receipt.created_at
  AND attempt.phase = 'completed'
  AND attempt.completed_at IS NOT NULL
  AND attempt.updated_at = attempt.completed_at
  AND attempt.redaction_version = 1
  AND attempt.plan_sha256 = lorepia_discovery_commit_plan_sha256(attempt.plan_json)
  AND json_extract(attempt.plan_json, '$.attempt_id') = attempt.id
  AND json_extract(attempt.plan_json, '$.session_id') = attempt.session_id
  AND json_extract(attempt.plan_json, '$.expected_revision') = attempt.expected_revision
  AND json_extract(attempt.plan_json, '$.credential_approval_id') IS NOT NULL
  AND json_extract(attempt.plan_json, '$.credential_ref')
      = json_extract(attempt.plan_json, '$.connection_id')
  AND julianday(operation.finished_at) <= julianday(attempt.completed_at)
  AND resolution_approval.grant_json = printf(
      '{"kind":"unknown_outcome_resolution","operation":"atomic_commit","resolution":{"resolution":"confirmed_commit_completed","connection_id":%s}}',
      json_quote(json_extract(attempt.plan_json, '$.connection_id'))
  )
  AND resolution_approval.grant_sha256 = lorepia_sha256_hex(resolution_approval.grant_json)
  AND 1 = (
      SELECT COUNT(*)
      FROM provider_discovery_approvals AS exact_approval
      WHERE exact_approval.session_id = resolution_receipt.session_id
        AND exact_approval.approval_kind = 'unknown_outcome_resolution'
        AND exact_approval.candidate_id IS NULL
        AND exact_approval.decision = 'approved'
        AND exact_approval.session_revision = resolution_receipt.expected_revision
        AND exact_approval.created_at = resolution_receipt.created_at
        AND exact_approval.redaction_version = 1
        AND exact_approval.grant_json = resolution_approval.grant_json
        AND exact_approval.grant_sha256 = resolution_approval.grant_sha256
  )
  AND 2 = (
      SELECT COUNT(*)
      FROM provider_discovery_audit_log AS graph_audit
      WHERE graph_audit.session_id = operation.session_id
        AND graph_audit.summary_key IN (
          'discovery.audit.provider_graph_applied',
          'discovery.audit.provider_template_ownership'
        )
  )
  AND authorized.commit_prepared_audit_sequence < operation_started.audit_sequence
  AND operation_started.audit_sequence < graph_applied.audit_sequence
  AND graph_applied.audit_sequence < template_owned.audit_sequence
  AND template_owned.audit_sequence < unknown_transition.audit_sequence
  AND unknown_transition.audit_sequence < operation_interrupted.audit_sequence
  AND operation_interrupted.audit_sequence < resolution_transition.audit_sequence
  AND resolution_transition.audit_sequence < approval_recorded.audit_sequence
  AND julianday(graph_applied.created_at) <= julianday(operation.finished_at)
  AND 1 = (
      SELECT COUNT(*) FROM provider_discovery_audit_log AS exact_started
      WHERE exact_started.session_id = operation.session_id
        AND exact_started.audit_kind = 'operation_started'
        AND exact_started.action_id = operation.action_id
        AND exact_started.subject_id = operation.id
  )
  AND 1 = (
      SELECT COUNT(*) FROM provider_discovery_audit_log AS exact_unknown_transition
      WHERE exact_unknown_transition.session_id = unknown_receipt.session_id
        AND exact_unknown_transition.audit_kind = 'transition_applied'
        AND exact_unknown_transition.action_id = unknown_receipt.action_id
        AND exact_unknown_transition.subject_id = unknown_receipt.event_id
  )
  AND 1 = (
      SELECT COUNT(*) FROM provider_discovery_audit_log AS exact_interrupted
      WHERE exact_interrupted.session_id = unknown_receipt.session_id
        AND exact_interrupted.audit_kind = 'operation_interrupted'
        AND exact_interrupted.action_id = unknown_receipt.action_id
        AND exact_interrupted.subject_id = operation.id
  )
  AND 1 = (
      SELECT COUNT(*) FROM provider_discovery_audit_log AS exact_resolution
      WHERE exact_resolution.session_id = resolution_receipt.session_id
        AND exact_resolution.audit_kind = 'unknown_outcome_reconciled'
        AND exact_resolution.action_id = resolution_receipt.action_id
        AND exact_resolution.subject_id = resolution_receipt.event_id
  )
  AND 1 = (
      SELECT COUNT(*) FROM provider_discovery_audit_log AS exact_approval_audit
      WHERE exact_approval_audit.session_id = resolution_receipt.session_id
        AND exact_approval_audit.audit_kind = 'approval_recorded'
        AND exact_approval_audit.action_id = resolution_receipt.action_id
  );

-- Compatibility projection for the two version-27 trigger replacements below.
-- The triggers additionally match the concrete operation id, so an authorized
-- action cannot be replayed onto a different physical operation.
CREATE VIEW provider_discovery_authorized_native_retry_starts AS
SELECT operation_id,
       start_action_id AS restart_action_id,
       session_id,
       commit_attempt_id,
       commit_plan_sha256,
       operation_expected_revision
FROM provider_discovery_authorized_native_commit_starts
WHERE start_action_kind = 'restart_interrupted';

-- Version 27 bound a native no-effect attestation to the action that first
-- prepared the commit attempt. A legitimate restart reuses that attempt but
-- starts a new atomic-commit operation from a `restart_interrupted` receipt.
-- Rebind both the insert and terminal-transition guards to the immutable
-- receipt/event that actually started the active operation.
DROP TRIGGER provider_discovery_native_no_effect_attestation_binding;

CREATE TRIGGER provider_discovery_native_no_effect_attestation_binding
BEFORE INSERT ON provider_discovery_native_no_effect_attestations
WHEN NOT EXISTS (
    SELECT 1
    FROM provider_discovery_operations AS operation
    JOIN provider_discovery_native_no_effect_execution_bindings AS binding
      ON binding.operation_id = operation.id
     AND binding.session_id = operation.session_id
    JOIN provider_discovery_native_credential_executions AS execution
      ON execution.operation_id = binding.operation_id
     AND execution.physical_authority_id = binding.physical_authority_id
    JOIN provider_discovery_native_credential_store_attempts AS store_attempt
      ON store_attempt.operation_id = execution.operation_id
     AND store_attempt.physical_authority_id = execution.physical_authority_id
    JOIN provider_discovery_sessions AS session
      ON session.id = operation.session_id
    JOIN provider_discovery_commit_attempts AS attempt
      ON attempt.id = NEW.commit_attempt_id
     AND attempt.session_id = session.id
    JOIN provider_discovery_action_receipts AS receipt
      ON receipt.session_id = operation.session_id
     AND receipt.action_id = operation.action_id
    JOIN provider_discovery_event_outbox AS event
      ON event.id = receipt.event_id
     AND event.session_id = receipt.session_id
    WHERE operation.id = NEW.operation_id
      AND operation.session_id = NEW.session_id
      AND operation.operation_kind = 'atomic_commit'
      AND operation.side_effect_class = 'persistent'
      AND operation.status = 'started'
      AND operation.started_at IS NOT NULL
      AND operation.started_at = store_attempt.started_at
      AND julianday(operation.created_at) IS NOT NULL
      AND julianday(operation.started_at) IS NOT NULL
      AND julianday(operation.started_at) >= julianday(operation.created_at)
      AND julianday(NEW.attested_at) IS NOT NULL
      AND julianday(NEW.attested_at) >= julianday(operation.started_at)
      AND operation.expected_revision = session.revision
      AND session.state = 'committing'
      AND session.active_operation_id = operation.id
      AND session.commit_attempt_id = attempt.id
      AND session.commit_plan_sha256 = NEW.commit_plan_sha256
      AND attempt.plan_sha256 = NEW.commit_plan_sha256
      AND attempt.phase = 'prepared'
      AND json_extract(attempt.plan_json, '$.attempt_id') = attempt.id
      AND json_extract(attempt.plan_json, '$.session_id') = session.id
      AND json_extract(attempt.plan_json, '$.connection_id') = NEW.connection_id
      AND json_extract(attempt.plan_json, '$.credential_ref') = NEW.connection_id
      AND binding.commit_attempt_id = NEW.commit_attempt_id
      AND binding.commit_plan_sha256 = NEW.commit_plan_sha256
      AND binding.connection_id = NEW.connection_id
      AND binding.connection_binding_sha256 = execution.connection_binding_sha256
      AND binding.attestation_evidence_sha256 = NEW.evidence_sha256
      AND binding.attested_at = NEW.attested_at
      AND NEW.evidence_sha256 = lorepia_native_no_effect_evidence_sha256(
          NEW.schema_version,
          NEW.attestation_kind,
          NEW.recovery_owner,
          NEW.operation_id,
          NEW.session_id,
          NEW.commit_attempt_id,
          NEW.commit_plan_sha256,
          NEW.connection_id
      )
      AND receipt.action_kind IN ('approve_review', 'restart_interrupted')
      AND receipt.outcome = 'applied'
      AND receipt.resulting_revision = operation.expected_revision
      AND receipt.resulting_revision = receipt.expected_revision + 1
      AND receipt.request_sha256 = operation.request_sha256
      AND receipt.created_at = operation.created_at
      AND event.sequence = receipt.event_sequence
      AND event.session_revision = receipt.resulting_revision
      AND event.state = 'committing'
      AND json_extract(
          receipt.response_json,
          '$.session.commit_attempt_id'
      ) = attempt.id
      AND json_extract(
          receipt.response_json,
          '$.session.commit_plan_sha256'
      ) = attempt.plan_sha256
      AND EXISTS (
          SELECT 1
          FROM provider_discovery_authorized_native_commit_starts AS authorized
          WHERE authorized.operation_id = operation.id
            AND authorized.start_action_id = receipt.action_id
            AND authorized.session_id = receipt.session_id
            AND authorized.commit_attempt_id = attempt.id
            AND authorized.commit_plan_sha256 = attempt.plan_sha256
            AND authorized.operation_expected_revision = operation.expected_revision
      )
)
BEGIN
    SELECT RAISE(ABORT, 'native no-effect attestation is detached from the active credential commit');
END;

DROP TRIGGER provider_discovery_operation_legal_transition;

CREATE TRIGGER provider_discovery_operation_legal_transition
BEFORE UPDATE OF status, started_at, finished_at, updated_at
ON provider_discovery_operations
WHEN NOT (
    (
        OLD.status = 'prepared'
        AND NEW.status = 'started'
        AND NEW.started_at IS NOT NULL
        AND NEW.finished_at IS NULL
        AND julianday(OLD.created_at) IS NOT NULL
        AND julianday(NEW.started_at) IS NOT NULL
        AND julianday(NEW.started_at) >= julianday(OLD.created_at)
    )
    OR (
        OLD.status = 'prepared'
        AND NEW.status = 'interrupted'
        AND NEW.started_at IS NOT NULL
        AND NEW.finished_at IS NOT NULL
        AND julianday(OLD.created_at) IS NOT NULL
        AND julianday(NEW.started_at) IS NOT NULL
        AND julianday(NEW.finished_at) IS NOT NULL
        AND julianday(NEW.started_at) >= julianday(OLD.created_at)
        AND julianday(NEW.finished_at) >= julianday(NEW.started_at)
        AND (
            NOT EXISTS (
                SELECT 1
                FROM provider_discovery_native_credential_executions AS execution
                WHERE execution.operation_id = OLD.id
            )
            OR (
                NEW.started_at = NEW.finished_at
                AND NEW.updated_at = NEW.finished_at
                AND NOT EXISTS (
                    SELECT 1
                    FROM provider_discovery_native_credential_store_attempts AS store_attempt
                    WHERE store_attempt.operation_id = OLD.id
                )
            )
        )
    )
    OR (
        OLD.status = 'started'
        AND NEW.status IN ('succeeded', 'failed')
        AND NEW.started_at = OLD.started_at
        AND NEW.finished_at IS NOT NULL
        AND julianday(OLD.started_at) IS NOT NULL
        AND julianday(NEW.finished_at) IS NOT NULL
        AND julianday(NEW.finished_at) >= julianday(OLD.started_at)
    )
    OR (
        OLD.status = 'started'
        AND OLD.side_effect_class IN ('local_deterministic', 'read_only')
        AND NEW.status = 'interrupted'
        AND NEW.started_at = OLD.started_at
        AND NEW.finished_at IS NOT NULL
        AND julianday(OLD.started_at) IS NOT NULL
        AND julianday(NEW.finished_at) IS NOT NULL
        AND julianday(NEW.finished_at) >= julianday(OLD.started_at)
    )
    OR (
        OLD.status = 'started'
        AND OLD.operation_kind = 'atomic_commit'
        AND OLD.side_effect_class = 'persistent'
        AND NEW.status = 'interrupted'
        AND NEW.started_at = OLD.started_at
        AND NEW.finished_at IS NOT NULL
        AND julianday(OLD.started_at) IS NOT NULL
        AND julianday(NEW.finished_at) IS NOT NULL
        AND julianday(NEW.finished_at) >= julianday(OLD.started_at)
        AND EXISTS (
            SELECT 1
            FROM provider_discovery_native_no_effect_attestations AS attestation
            JOIN provider_discovery_native_no_effect_execution_bindings AS binding
              ON binding.operation_id = attestation.operation_id
             AND binding.session_id = attestation.session_id
             AND binding.commit_attempt_id = attestation.commit_attempt_id
             AND binding.commit_plan_sha256 = attestation.commit_plan_sha256
             AND binding.connection_id = attestation.connection_id
             AND binding.attestation_evidence_sha256 = attestation.evidence_sha256
             AND binding.attested_at = attestation.attested_at
            JOIN provider_discovery_native_credential_executions AS execution
              ON execution.operation_id = binding.operation_id
             AND execution.physical_authority_id = binding.physical_authority_id
             AND execution.connection_id = binding.connection_id
             AND execution.connection_binding_sha256 = binding.connection_binding_sha256
            JOIN provider_discovery_native_credential_store_attempts AS store_attempt
              ON store_attempt.operation_id = execution.operation_id
             AND store_attempt.physical_authority_id = execution.physical_authority_id
            JOIN provider_discovery_sessions AS session
              ON session.id = attestation.session_id
            JOIN provider_discovery_commit_attempts AS attempt
              ON attempt.id = attestation.commit_attempt_id
             AND attempt.session_id = session.id
            JOIN provider_discovery_action_receipts AS receipt
              ON receipt.session_id = OLD.session_id
             AND receipt.action_id = OLD.action_id
            JOIN provider_discovery_event_outbox AS event
              ON event.id = receipt.event_id
             AND event.session_id = receipt.session_id
            WHERE attestation.operation_id = OLD.id
              AND attestation.session_id = OLD.session_id
              AND session.state = 'committing'
              AND session.active_operation_id = OLD.id
              AND session.commit_attempt_id = attempt.id
              AND session.commit_plan_sha256 = attestation.commit_plan_sha256
              AND OLD.started_at = store_attempt.started_at
              AND OLD.expected_revision = session.revision
              AND attempt.plan_sha256 = attestation.commit_plan_sha256
              AND attempt.phase = 'prepared'
              AND json_extract(attempt.plan_json, '$.attempt_id') = attempt.id
              AND json_extract(attempt.plan_json, '$.session_id') = session.id
              AND json_extract(attempt.plan_json, '$.connection_id') = attestation.connection_id
              AND json_extract(attempt.plan_json, '$.credential_ref') = attestation.connection_id
              AND receipt.action_kind IN ('approve_review', 'restart_interrupted')
              AND receipt.outcome = 'applied'
              AND receipt.resulting_revision = OLD.expected_revision
              AND receipt.resulting_revision = receipt.expected_revision + 1
              AND receipt.request_sha256 = OLD.request_sha256
              AND receipt.created_at = OLD.created_at
              AND event.sequence = receipt.event_sequence
              AND event.session_revision = receipt.resulting_revision
              AND event.state = 'committing'
              AND json_extract(
                  receipt.response_json,
                  '$.session.commit_attempt_id'
              ) = attempt.id
              AND json_extract(
                  receipt.response_json,
                  '$.session.commit_plan_sha256'
              ) = attempt.plan_sha256
              AND EXISTS (
                  SELECT 1
                  FROM provider_discovery_authorized_native_commit_starts AS authorized
                  WHERE authorized.operation_id = OLD.id
                    AND authorized.start_action_id = receipt.action_id
                    AND authorized.session_id = receipt.session_id
                    AND authorized.commit_attempt_id = attempt.id
                    AND authorized.commit_plan_sha256 = attempt.plan_sha256
                    AND authorized.operation_expected_revision = OLD.expected_revision
              )
              AND attestation.attestation_kind = 'credential_slot_missing'
              AND attestation.recovery_owner = 'native_platform'
              AND attestation.schema_version = 1
              AND attestation.redaction_version = 1
              AND attestation.evidence_sha256
                  = lorepia_native_no_effect_evidence_sha256(
                      attestation.schema_version,
                      attestation.attestation_kind,
                      attestation.recovery_owner,
                      attestation.operation_id,
                      attestation.session_id,
                      attestation.commit_attempt_id,
                      attestation.commit_plan_sha256,
                      attestation.connection_id
                  )
              AND attestation.attested_at = NEW.finished_at
        )
    )
    OR (
        OLD.status = 'started'
        AND OLD.side_effect_class IN ('billable_external', 'persistent')
        AND NEW.status = 'outcome_unknown'
        AND NEW.started_at = OLD.started_at
        AND NEW.finished_at IS NOT NULL
        AND julianday(OLD.started_at) IS NOT NULL
        AND julianday(NEW.finished_at) IS NOT NULL
        AND julianday(NEW.finished_at) >= julianday(OLD.started_at)
    )
)
BEGIN
    SELECT RAISE(ABORT, 'illegal discovery operation status transition');
END;

CREATE INDEX provider_discovery_native_credential_executions_attempt
    ON provider_discovery_native_credential_executions(
        commit_attempt_id, reserved_at, operation_id
    );

CREATE INDEX provider_discovery_native_credential_executions_connection
    ON provider_discovery_native_credential_executions(
        connection_id, reserved_at, physical_authority_id
    );

CREATE TRIGGER provider_discovery_native_credential_execution_insert_guard
BEFORE INSERT ON provider_discovery_native_credential_executions
WHEN NOT EXISTS (
    SELECT 1
    FROM provider_discovery_operations AS operation
    JOIN provider_discovery_sessions AS session
      ON session.id = operation.session_id
    JOIN provider_discovery_commit_attempts AS attempt
      ON attempt.id = NEW.commit_attempt_id
     AND attempt.session_id = session.id
    JOIN provider_discovery_authorized_native_commit_starts AS authorized
      ON authorized.operation_id = operation.id
     AND authorized.session_id = session.id
     AND authorized.commit_attempt_id = attempt.id
     AND authorized.commit_plan_sha256 = attempt.plan_sha256
     AND authorized.operation_expected_revision = operation.expected_revision
    WHERE operation.id = NEW.operation_id
      AND operation.session_id = NEW.session_id
      AND operation.operation_kind = 'atomic_commit'
      AND operation.side_effect_class = 'persistent'
      AND operation.status = 'prepared'
      AND operation.started_at IS NULL
      AND operation.finished_at IS NULL
      AND session.state = 'committing'
      AND session.active_operation_id = operation.id
      AND session.revision = operation.expected_revision
      AND session.commit_attempt_id = attempt.id
      AND session.commit_plan_sha256 = attempt.plan_sha256
      AND session.cancellation_pending = 0
      AND attempt.phase = 'prepared'
      AND attempt.plan_sha256 = NEW.commit_plan_sha256
      AND json_extract(attempt.plan_json, '$.attempt_id') = attempt.id
      AND json_extract(attempt.plan_json, '$.session_id') = session.id
      AND json_extract(attempt.plan_json, '$.connection_id') = NEW.connection_id
      AND json_extract(attempt.plan_json, '$.credential_ref') = NEW.connection_id
      AND julianday(operation.created_at) IS NOT NULL
      AND julianday(NEW.reserved_at) >= julianday(operation.created_at)
)
BEGIN
    SELECT RAISE(ABORT, 'native credential execution is detached from its prepared discovery commit');
END;

CREATE TRIGGER provider_discovery_native_credential_execution_no_replace
BEFORE INSERT ON provider_discovery_native_credential_executions
WHEN EXISTS (
    SELECT 1
    FROM provider_discovery_native_credential_executions AS existing
    WHERE existing.physical_authority_id = NEW.physical_authority_id
       OR existing.operation_id = NEW.operation_id
)
BEGIN
    SELECT RAISE(ABORT, 'native credential execution cannot replace history');
END;

CREATE TRIGGER provider_discovery_native_credential_execution_no_update
BEFORE UPDATE ON provider_discovery_native_credential_executions
BEGIN
    SELECT RAISE(ABORT, 'native credential executions are immutable');
END;

CREATE TRIGGER provider_discovery_native_credential_execution_no_delete
BEFORE DELETE ON provider_discovery_native_credential_executions
BEGIN
    SELECT RAISE(ABORT, 'native credential executions are immutable');
END;

CREATE TRIGGER provider_discovery_native_credential_store_attempt_insert_guard
BEFORE INSERT ON provider_discovery_native_credential_store_attempts
WHEN NOT EXISTS (
    SELECT 1
    FROM provider_discovery_native_credential_executions AS execution
    JOIN provider_discovery_operations AS operation
      ON operation.id = execution.operation_id
    JOIN provider_discovery_sessions AS session
      ON session.id = execution.session_id
    JOIN provider_discovery_commit_attempts AS attempt
      ON attempt.id = execution.commit_attempt_id
     AND attempt.session_id = execution.session_id
    JOIN provider_discovery_authorized_native_commit_starts AS authorized
      ON authorized.operation_id = execution.operation_id
     AND authorized.session_id = execution.session_id
     AND authorized.commit_attempt_id = execution.commit_attempt_id
     AND authorized.commit_plan_sha256 = execution.commit_plan_sha256
     AND authorized.operation_expected_revision = operation.expected_revision
    WHERE execution.operation_id = NEW.operation_id
      AND execution.physical_authority_id = NEW.physical_authority_id
      AND operation.status = 'prepared'
      AND operation.started_at IS NULL
      AND operation.finished_at IS NULL
      AND session.state = 'committing'
      AND session.active_operation_id = operation.id
      AND session.revision = operation.expected_revision
      AND session.commit_attempt_id = attempt.id
      AND session.commit_plan_sha256 = attempt.plan_sha256
      AND session.cancellation_pending = 0
      AND attempt.phase = 'prepared'
      AND julianday(NEW.started_at) >= julianday(execution.reserved_at)
)
BEGIN
    SELECT RAISE(ABORT, 'native credential store attempt is detached from its reservation');
END;

CREATE TRIGGER provider_discovery_native_credential_store_attempt_no_replace
BEFORE INSERT ON provider_discovery_native_credential_store_attempts
WHEN EXISTS (
    SELECT 1
    FROM provider_discovery_native_credential_store_attempts AS existing
    WHERE existing.operation_id = NEW.operation_id
       OR existing.physical_authority_id = NEW.physical_authority_id
)
BEGIN
    SELECT RAISE(ABORT, 'native credential store attempt cannot replace history');
END;

CREATE TRIGGER provider_discovery_native_credential_store_attempt_no_update
BEFORE UPDATE ON provider_discovery_native_credential_store_attempts
BEGIN
    SELECT RAISE(ABORT, 'native credential store attempts are immutable');
END;

CREATE TRIGGER provider_discovery_native_credential_store_attempt_no_delete
BEFORE DELETE ON provider_discovery_native_credential_store_attempts
BEGIN
    SELECT RAISE(ABORT, 'native credential store attempts are immutable');
END;

-- This guard is also used by the automatic capture trigger below. Requiring
-- the operation's already-written interrupted state makes a standalone row
-- impossible, while the operation transition remains atomic with the capture.
CREATE TRIGGER provider_discovery_native_credential_abandonment_insert_guard
BEFORE INSERT ON provider_discovery_native_credential_abandoned_reservations
WHEN NOT EXISTS (
    SELECT 1
    FROM provider_discovery_native_credential_executions AS execution
    JOIN provider_discovery_operations AS operation
      ON operation.id = execution.operation_id
    WHERE execution.operation_id = NEW.operation_id
      AND execution.physical_authority_id = NEW.physical_authority_id
      AND execution.session_id = NEW.session_id
      AND execution.commit_attempt_id = NEW.commit_attempt_id
      AND execution.commit_plan_sha256 = NEW.commit_plan_sha256
      AND execution.connection_id = NEW.connection_id
      AND execution.connection_binding_sha256 = NEW.connection_binding_sha256
      AND execution.reserved_at = NEW.reserved_at
      AND operation.session_id = execution.session_id
      AND operation.operation_kind = 'atomic_commit'
      AND operation.side_effect_class = 'persistent'
      AND operation.status = 'interrupted'
      AND operation.started_at = NEW.abandoned_at
      AND operation.finished_at = NEW.abandoned_at
      AND operation.updated_at = NEW.abandoned_at
      AND NOT EXISTS (
          SELECT 1
          FROM provider_discovery_native_credential_store_attempts AS store_attempt
          WHERE store_attempt.operation_id = execution.operation_id
             OR store_attempt.physical_authority_id = execution.physical_authority_id
      )
      AND NOT EXISTS (
          SELECT 1
          FROM provider_discovery_audit_log AS started_audit
          WHERE started_audit.session_id = execution.session_id
            AND started_audit.audit_kind = 'operation_started'
            AND started_audit.subject_id = execution.operation_id
      )
)
BEGIN
    SELECT RAISE(ABORT, 'native credential abandonment is detached from an interrupted reservation');
END;

CREATE TRIGGER provider_discovery_native_credential_abandonment_capture
AFTER UPDATE OF status, started_at, finished_at, updated_at
ON provider_discovery_operations
WHEN OLD.status = 'prepared'
 AND NEW.status = 'interrupted'
 AND NEW.operation_kind = 'atomic_commit'
 AND NEW.side_effect_class = 'persistent'
 AND NEW.started_at = NEW.finished_at
 AND NEW.finished_at = NEW.updated_at
 AND EXISTS (
     SELECT 1
     FROM provider_discovery_native_credential_executions AS execution
     WHERE execution.operation_id = NEW.id
       AND execution.session_id = NEW.session_id
 )
 AND NOT EXISTS (
     SELECT 1
     FROM provider_discovery_native_credential_store_attempts AS store_attempt
     WHERE store_attempt.operation_id = NEW.id
 )
BEGIN
    INSERT INTO provider_discovery_native_credential_abandoned_reservations (
        operation_id,
        physical_authority_id,
        session_id,
        commit_attempt_id,
        commit_plan_sha256,
        connection_id,
        connection_binding_sha256,
        reserved_at,
        abandonment_kind,
        abandoned_at,
        schema_version,
        redaction_version
    )
    SELECT execution.operation_id,
           execution.physical_authority_id,
           execution.session_id,
           execution.commit_attempt_id,
           execution.commit_plan_sha256,
           execution.connection_id,
           execution.connection_binding_sha256,
           execution.reserved_at,
           'prepared_interrupted_before_native_store',
           NEW.finished_at,
           1,
           1
    FROM provider_discovery_native_credential_executions AS execution
    WHERE execution.operation_id = NEW.id;
END;

CREATE TRIGGER provider_discovery_native_credential_abandonment_no_replace
BEFORE INSERT ON provider_discovery_native_credential_abandoned_reservations
WHEN EXISTS (
    SELECT 1
    FROM provider_discovery_native_credential_abandoned_reservations AS existing
    WHERE existing.operation_id = NEW.operation_id
       OR existing.physical_authority_id = NEW.physical_authority_id
)
BEGIN
    SELECT RAISE(ABORT, 'native credential abandonment cannot replace history');
END;

CREATE TRIGGER provider_discovery_native_credential_abandonment_no_update
BEFORE UPDATE ON provider_discovery_native_credential_abandoned_reservations
BEGIN
    SELECT RAISE(ABORT, 'native credential abandonments are immutable');
END;

CREATE TRIGGER provider_discovery_native_credential_abandonment_no_delete
BEFORE DELETE ON provider_discovery_native_credential_abandoned_reservations
BEGIN
    SELECT RAISE(ABORT, 'native credential abandonments are immutable');
END;

CREATE TRIGGER provider_discovery_native_no_effect_execution_binding_insert_guard
BEFORE INSERT ON provider_discovery_native_no_effect_execution_bindings
WHEN NOT EXISTS (
    SELECT 1
    FROM provider_discovery_native_credential_executions AS execution
    JOIN provider_discovery_native_credential_store_attempts AS store_attempt
      ON store_attempt.operation_id = execution.operation_id
     AND store_attempt.physical_authority_id = execution.physical_authority_id
    JOIN provider_discovery_operations AS operation
      ON operation.id = execution.operation_id
    JOIN provider_discovery_sessions AS session
      ON session.id = execution.session_id
    JOIN provider_discovery_commit_attempts AS attempt
      ON attempt.id = execution.commit_attempt_id
     AND attempt.session_id = execution.session_id
    WHERE execution.operation_id = NEW.operation_id
      AND execution.physical_authority_id = NEW.physical_authority_id
      AND execution.session_id = NEW.session_id
      AND execution.commit_attempt_id = NEW.commit_attempt_id
      AND execution.commit_plan_sha256 = NEW.commit_plan_sha256
      AND execution.connection_id = NEW.connection_id
      AND execution.connection_binding_sha256 = NEW.connection_binding_sha256
      AND operation.status = 'started'
      AND operation.started_at = store_attempt.started_at
      AND session.state = 'committing'
      AND session.active_operation_id = operation.id
      AND session.commit_attempt_id = attempt.id
      AND session.commit_plan_sha256 = attempt.plan_sha256
      AND attempt.phase = 'prepared'
      AND julianday(NEW.attested_at) >= julianday(store_attempt.started_at)
      AND NEW.execution_binding_sha256 = lorepia_sha256_hex(printf(
          '{"attestation_evidence_sha256":%s,"attested_at":%s,"commit_attempt_id":%s,"commit_plan_sha256":%s,"connection_binding_sha256":%s,"connection_id":%s,"operation_id":%s,"physical_authority_id":%s,"redaction_version":1,"schema_version":1,"session_id":%s}',
          json_quote(NEW.attestation_evidence_sha256),
          json_quote(NEW.attested_at),
          json_quote(NEW.commit_attempt_id),
          json_quote(NEW.commit_plan_sha256),
          json_quote(NEW.connection_binding_sha256),
          json_quote(NEW.connection_id),
          json_quote(NEW.operation_id),
          json_quote(NEW.physical_authority_id),
          json_quote(NEW.session_id)
      ))
)
BEGIN
    SELECT RAISE(ABORT, 'native no-effect execution binding is detached from its store attempt');
END;

CREATE TRIGGER provider_discovery_native_no_effect_execution_binding_no_replace
BEFORE INSERT ON provider_discovery_native_no_effect_execution_bindings
WHEN EXISTS (
    SELECT 1
    FROM provider_discovery_native_no_effect_execution_bindings AS existing
    WHERE existing.operation_id = NEW.operation_id
       OR existing.physical_authority_id = NEW.physical_authority_id
)
BEGIN
    SELECT RAISE(ABORT, 'native no-effect execution binding cannot replace history');
END;

CREATE TRIGGER provider_discovery_native_no_effect_execution_binding_no_update
BEFORE UPDATE ON provider_discovery_native_no_effect_execution_bindings
BEGIN
    SELECT RAISE(ABORT, 'native no-effect execution bindings are immutable');
END;

CREATE TRIGGER provider_discovery_native_no_effect_execution_binding_no_delete
BEFORE DELETE ON provider_discovery_native_no_effect_execution_bindings
BEGIN
    SELECT RAISE(ABORT, 'native no-effect execution bindings are immutable');
END;

-- Every attestation inserted after the sealed version-37 cutpoint must already
-- have the exact physical companion in the same transaction. Rows copied into
-- the legacy snapshot above are intentionally exempt because they predate
-- this trigger and remain non-authorizing.
CREATE TRIGGER provider_discovery_native_no_effect_schema37_companion_required
BEFORE INSERT ON provider_discovery_native_no_effect_attestations
WHEN NOT EXISTS (
    SELECT 1
    FROM provider_discovery_native_no_effect_execution_bindings AS binding
    WHERE binding.operation_id = NEW.operation_id
      AND binding.session_id = NEW.session_id
      AND binding.commit_attempt_id = NEW.commit_attempt_id
      AND binding.commit_plan_sha256 = NEW.commit_plan_sha256
      AND binding.connection_id = NEW.connection_id
      AND binding.attestation_evidence_sha256 = NEW.evidence_sha256
      AND binding.attested_at = NEW.attested_at
      AND binding.schema_version = NEW.schema_version
      AND binding.redaction_version = NEW.redaction_version
)
BEGIN
    SELECT RAISE(ABORT, 'schema-37 native no-effect attestation requires physical execution authority');
END;

-- A reservation is visible while Prepared. The store-attempt row is inserted
-- immediately before Prepared -> Started in one IMMEDIATE transaction. Every
-- started or terminal credential operation retains both immutable edges.
CREATE TRIGGER provider_discovery_native_credential_execution_required
BEFORE UPDATE OF status, started_at, finished_at, updated_at
ON provider_discovery_operations
WHEN OLD.operation_kind = 'atomic_commit'
 AND OLD.side_effect_class = 'persistent'
 AND EXISTS (
     SELECT 1
     FROM provider_discovery_authorized_native_commit_starts AS authorized
     WHERE authorized.operation_id = OLD.id
 )
 AND NEW.status IN ('started', 'succeeded', 'failed', 'outcome_unknown')
 AND NOT EXISTS (
     SELECT 1
     FROM provider_discovery_native_credential_executions AS execution
     JOIN provider_discovery_native_credential_store_attempts AS store_attempt
       ON store_attempt.operation_id = execution.operation_id
      AND store_attempt.physical_authority_id = execution.physical_authority_id
     JOIN provider_discovery_authorized_native_commit_starts AS authorized
       ON authorized.operation_id = execution.operation_id
      AND authorized.session_id = execution.session_id
      AND authorized.commit_attempt_id = execution.commit_attempt_id
      AND authorized.commit_plan_sha256 = execution.commit_plan_sha256
     WHERE execution.operation_id = OLD.id
       AND execution.session_id = OLD.session_id
       AND store_attempt.started_at = NEW.started_at
 )
 AND NOT (
     OLD.status = 'started'
     AND NEW.status = 'outcome_unknown'
     AND NEW.started_at = OLD.started_at
     AND NEW.finished_at IS NOT NULL
     AND EXISTS (
         SELECT 1
         FROM provider_discovery_native_credential_legacy_started_cutoff_snapshots
              AS legacy
         JOIN provider_discovery_authorized_native_commit_starts AS authorized
           ON authorized.operation_id = legacy.operation_id
          AND authorized.session_id = legacy.session_id
          AND authorized.start_action_id = legacy.start_action_id
          AND authorized.start_action_kind = legacy.start_action_kind
          AND authorized.operation_expected_revision
              = legacy.operation_expected_revision
          AND authorized.commit_attempt_id = legacy.commit_attempt_id
          AND authorized.commit_plan_sha256 = legacy.commit_plan_sha256
          AND authorized.start_transition_audit_sequence
              = legacy.start_transition_audit_sequence
          AND authorized.commit_prepared_audit_sequence
              = legacy.commit_prepared_audit_sequence
         JOIN provider_discovery_commit_attempts AS attempt
           ON attempt.id = legacy.commit_attempt_id
          AND attempt.session_id = legacy.session_id
          AND attempt.plan_sha256 = legacy.commit_plan_sha256
         JOIN provider_discovery_sessions AS session
           ON session.id = legacy.session_id
         WHERE legacy.operation_id = OLD.id
           AND legacy.session_id = OLD.session_id
           AND legacy.session_cancellation_pending
               = session.cancellation_pending
           AND session.state = 'committing'
           AND session.active_operation_id = OLD.id
           AND session.revision = legacy.session_revision_at_cutoff
           AND session.next_event_sequence
               = legacy.session_next_event_sequence_at_cutoff
           AND session.commit_attempt_id = legacy.commit_attempt_id
           AND session.commit_plan_sha256 = legacy.commit_plan_sha256
           AND legacy.start_action_id = OLD.action_id
           AND legacy.request_sha256 = OLD.request_sha256
           AND legacy.operation_expected_revision = OLD.expected_revision
           AND legacy.operation_created_at = OLD.created_at
           AND legacy.operation_started_at = OLD.started_at
           AND legacy.cutoff_before_schema_version = 37
           AND legacy.snapshot_schema_version = 1
           AND legacy.redaction_version = 1
           AND json_extract(attempt.plan_json, '$.connection_id')
               = legacy.connection_id
           AND json_extract(attempt.plan_json, '$.credential_ref')
               = legacy.connection_id
     )
 )
BEGIN
    SELECT RAISE(ABORT, 'native discovery credential operation has no immutable execution authority');
END;

-- Provider selection is a single durable register. Discovery compensation may
-- clear a selection in one transaction and restore the pre-commit value in a
-- later transaction, so the value JSON alone cannot distinguish that internal
-- clear from a newer explicit user clear. Every selection intent advances this
-- revision; restoration is authorized only by the exact revision produced by
-- graph removal.
CREATE TABLE provider_selection_state (
    singleton_key TEXT NOT NULL PRIMARY KEY CHECK (singleton_key = 'application'),
    revision INTEGER NOT NULL CHECK (revision >= 0)
);

INSERT INTO provider_selection_state (singleton_key, revision)
VALUES ('application', 0);

CREATE TRIGGER provider_selection_state_singleton_no_insert
BEFORE INSERT ON provider_selection_state
BEGIN
    SELECT RAISE(ABORT, 'provider selection state is a singleton');
END;

CREATE TRIGGER provider_selection_state_revision_guard
BEFORE UPDATE ON provider_selection_state
WHEN NEW.singleton_key != OLD.singleton_key
  OR NEW.revision != OLD.revision + 1
BEGIN
    SELECT RAISE(ABORT, 'provider selection revision must advance exactly once');
END;

CREATE TRIGGER provider_selection_state_no_delete
BEFORE DELETE ON provider_selection_state
BEGIN
    SELECT RAISE(ABORT, 'provider selection state cannot be deleted');
END;

-- The authority is append-only evidence that one graph-removal transaction
-- actually changed the selected graph to None at this exact revision. A
-- selection restore without this row is a no-op, which keeps pre-schema-37 or
-- user-cleared states conservative instead of resurrecting stale settings.
CREATE TABLE provider_discovery_selection_restore_authorities (
    commit_attempt_id TEXT NOT NULL PRIMARY KEY
        REFERENCES provider_discovery_commit_attempts(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    restore_step_id TEXT NOT NULL UNIQUE
        REFERENCES provider_discovery_compensation_steps(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    selection_revision_after_graph_removal INTEGER NOT NULL CHECK (
        selection_revision_after_graph_removal > 0
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0)
);

CREATE TRIGGER provider_discovery_selection_restore_authority_insert_guard
BEFORE INSERT ON provider_discovery_selection_restore_authorities
WHEN NOT EXISTS (
    SELECT 1
    FROM provider_selection_state AS selection_state
    JOIN provider_discovery_compensation_steps AS restore_step
      ON restore_step.id = NEW.restore_step_id
     AND restore_step.commit_attempt_id = NEW.commit_attempt_id
     AND restore_step.step_kind = 'restore_previous_selection'
     AND restore_step.status = 'pending'
    JOIN provider_discovery_compensation_steps AS graph_step
      ON graph_step.commit_attempt_id = NEW.commit_attempt_id
     AND graph_step.step_kind = 'remove_connection_graph'
     AND graph_step.status = 'in_progress'
    JOIN app_settings AS settings
      ON settings.key = 'application'
    WHERE selection_state.singleton_key = 'application'
      AND selection_state.revision = NEW.selection_revision_after_graph_removal
      AND json_extract(settings.value_json, '$.selected_provider_profile_id') IS NULL
      AND json_extract(settings.value_json, '$.selected_model_route_id') IS NULL
      AND json_extract(settings.value_json, '$.selected_generation_preset_id') IS NULL
)
BEGIN
    SELECT RAISE(ABORT, 'discovery selection restore authority is detached from graph removal');
END;

CREATE TRIGGER provider_discovery_selection_restore_authority_no_update
BEFORE UPDATE ON provider_discovery_selection_restore_authorities
BEGIN
    SELECT RAISE(ABORT, 'discovery selection restore authority is immutable');
END;

CREATE TRIGGER provider_discovery_selection_restore_authority_no_delete
BEFORE DELETE ON provider_discovery_selection_restore_authorities
BEGIN
    SELECT RAISE(ABORT, 'discovery selection restore authority is immutable');
END;
