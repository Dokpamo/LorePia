-- Terminal fail-closed evidence for a claimed derived occurrence whose sealed
-- policy authority cannot be reconstructed. The parent outbox row remains
-- immutable; existence of this one-to-one record is its terminal state.

CREATE TABLE interaction_derived_event_quarantines (
    occurrence_id TEXT PRIMARY KEY NOT NULL
        REFERENCES interaction_derived_event_outbox(occurrence_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    reason_kind TEXT NOT NULL CHECK (
        reason_kind = 'sealed_policy_recovery_failed'
    ),
    delivery_attempts INTEGER NOT NULL CHECK (delivery_attempts >= 1),
    sealed_policy_sha256 TEXT NOT NULL CHECK (
        length(sealed_policy_sha256) = 64
        AND sealed_policy_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    active_policy_sha256 TEXT CHECK (
        active_policy_sha256 IS NULL
        OR (
            length(active_policy_sha256) = 64
            AND active_policy_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    ),
    source_effect_sha256 TEXT NOT NULL CHECK (
        length(source_effect_sha256) = 64
        AND source_effect_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    source_action_sha256 TEXT NOT NULL CHECK (
        length(source_action_sha256) = 64
        AND source_action_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    evidence_json TEXT NOT NULL CHECK (
        json_valid(evidence_json)
        AND json_type(evidence_json) = 'object'
        AND length(CAST(evidence_json AS BLOB)) <= 262144
    ),
    evidence_sha256 TEXT NOT NULL CHECK (
        length(evidence_sha256) = 64
        AND evidence_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    quarantined_at TEXT NOT NULL CHECK (length(trim(quarantined_at)) > 0)
);

CREATE TRIGGER interaction_derived_event_quarantine_claim_guard
BEFORE INSERT ON interaction_derived_event_quarantines
WHEN NOT EXISTS (
    SELECT 1
    FROM interaction_derived_event_outbox AS occurrence
    WHERE occurrence.occurrence_id = NEW.occurrence_id
      AND occurrence.status = 'claimed'
      AND occurrence.delivery_attempts = NEW.delivery_attempts
)
BEGIN
    SELECT RAISE(ABORT, 'derived interaction quarantine requires its exact claim');
END;

CREATE TRIGGER interaction_derived_event_quarantine_no_update
BEFORE UPDATE ON interaction_derived_event_quarantines
BEGIN
    SELECT RAISE(ABORT, 'derived interaction quarantine is immutable');
END;

CREATE TRIGGER interaction_derived_event_quarantine_no_delete
BEFORE DELETE ON interaction_derived_event_quarantines
BEGIN
    SELECT RAISE(ABORT, 'derived interaction quarantine is immutable');
END;
