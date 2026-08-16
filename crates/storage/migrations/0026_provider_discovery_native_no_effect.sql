-- A trusted native credential-vault owner can prove that a started credential
-- installation left no external effect when the exact vault slot is absent.
-- Keep the database exception narrower than the persistent side-effect class:
-- only an atomic commit may project that typed attestation to `interrupted`.
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
