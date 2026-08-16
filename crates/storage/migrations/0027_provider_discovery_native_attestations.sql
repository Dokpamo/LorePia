-- Persist the exact native-vault statement that authorizes the one narrow
-- exception to conservative persistent-effect recovery. Version 26 allowed an
-- atomic commit to become `interrupted`, but the database could not distinguish
-- a native missing-slot attestation from an ordinary status update.
CREATE TABLE provider_discovery_native_no_effect_attestations (
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
    connection_id TEXT NOT NULL CHECK (length(trim(connection_id)) > 0),
    attestation_kind TEXT NOT NULL CHECK (
        attestation_kind = 'credential_slot_missing'
    ),
    evidence_sha256 TEXT NOT NULL CHECK (
        length(evidence_sha256) = 64
        AND evidence_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    recovery_owner TEXT NOT NULL CHECK (recovery_owner = 'native_platform'),
    schema_version INTEGER NOT NULL CHECK (schema_version = 1),
    redaction_version INTEGER NOT NULL CHECK (redaction_version = 1),
    attested_at TEXT NOT NULL CHECK (length(trim(attested_at)) > 0),
    UNIQUE (session_id, commit_attempt_id, operation_id)
);

-- An attestation can be inserted only for the exact active, started,
-- credential-bearing atomic commit that the native recovery owner inspected.
CREATE TRIGGER provider_discovery_native_no_effect_attestation_binding
BEFORE INSERT ON provider_discovery_native_no_effect_attestations
WHEN NOT EXISTS (
    SELECT 1
    FROM provider_discovery_operations AS operation
    JOIN provider_discovery_sessions AS session
      ON session.id = operation.session_id
    JOIN provider_discovery_commit_attempts AS attempt
      ON attempt.id = NEW.commit_attempt_id
     AND attempt.session_id = session.id
    WHERE operation.id = NEW.operation_id
      AND operation.session_id = NEW.session_id
      AND operation.operation_kind = 'atomic_commit'
      AND operation.side_effect_class = 'persistent'
      AND operation.status = 'started'
      AND operation.expected_revision = session.revision
      AND operation.action_id = attempt.action_id
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
)
BEGIN
    SELECT RAISE(ABORT, 'native no-effect attestation is detached from the active credential commit');
END;

CREATE TRIGGER provider_discovery_native_no_effect_attestation_no_update
BEFORE UPDATE ON provider_discovery_native_no_effect_attestations
BEGIN
    SELECT RAISE(ABORT, 'native no-effect attestations are immutable');
END;

CREATE TRIGGER provider_discovery_native_no_effect_attestation_no_delete
BEFORE DELETE ON provider_discovery_native_no_effect_attestations
BEGIN
    SELECT RAISE(ABORT, 'native no-effect attestations are immutable');
END;

-- Replace the version-26 exception. A started persistent operation can become
-- `interrupted` only when the exact immutable native attestation already exists
-- in the same transaction. The ordinary Core recovery path remains
-- `outcome_unknown` for every started persistent operation.
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
    )
    OR (
        OLD.status = 'prepared'
        AND NEW.status = 'interrupted'
        AND NEW.started_at IS NOT NULL
        AND NEW.finished_at IS NOT NULL
    )
    OR (
        OLD.status = 'started'
        AND NEW.status IN ('succeeded', 'failed')
        AND NEW.started_at = OLD.started_at
        AND NEW.finished_at IS NOT NULL
    )
    OR (
        OLD.status = 'started'
        AND OLD.side_effect_class IN ('local_deterministic', 'read_only')
        AND NEW.status = 'interrupted'
        AND NEW.started_at = OLD.started_at
        AND NEW.finished_at IS NOT NULL
    )
    OR (
        OLD.status = 'started'
        AND OLD.operation_kind = 'atomic_commit'
        AND OLD.side_effect_class = 'persistent'
        AND NEW.status = 'interrupted'
        AND NEW.started_at = OLD.started_at
        AND NEW.finished_at IS NOT NULL
        AND EXISTS (
            SELECT 1
            FROM provider_discovery_native_no_effect_attestations AS attestation
            JOIN provider_discovery_sessions AS session
              ON session.id = attestation.session_id
            JOIN provider_discovery_commit_attempts AS attempt
              ON attempt.id = attestation.commit_attempt_id
             AND attempt.session_id = session.id
            WHERE attestation.operation_id = OLD.id
              AND attestation.session_id = OLD.session_id
              AND session.state = 'committing'
              AND session.active_operation_id = OLD.id
              AND session.commit_attempt_id = attempt.id
              AND session.commit_plan_sha256 = attestation.commit_plan_sha256
              AND OLD.expected_revision = session.revision
              AND OLD.action_id = attempt.action_id
              AND attempt.plan_sha256 = attestation.commit_plan_sha256
              AND attempt.phase = 'prepared'
              AND json_extract(attempt.plan_json, '$.attempt_id') = attempt.id
              AND json_extract(attempt.plan_json, '$.session_id') = session.id
              AND json_extract(attempt.plan_json, '$.connection_id') = attestation.connection_id
              AND json_extract(attempt.plan_json, '$.credential_ref') = attestation.connection_id
              AND attestation.attestation_kind = 'credential_slot_missing'
              AND attestation.recovery_owner = 'native_platform'
              AND attestation.schema_version = 1
              AND attestation.redaction_version = 1
              AND attestation.attested_at = NEW.finished_at
        )
    )
    OR (
        OLD.status = 'started'
        AND OLD.side_effect_class IN ('billable_external', 'persistent')
        AND NEW.status = 'outcome_unknown'
        AND NEW.started_at = OLD.started_at
        AND NEW.finished_at IS NOT NULL
    )
)
BEGIN
    SELECT RAISE(ABORT, 'illegal discovery operation status transition');
END;
