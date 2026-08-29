-- Metadata-only audit trail for model calls initiated by imported character
-- runtimes. Prompt text, generated text, credentials, provider payloads, and
-- raw provider usage summaries are deliberately absent from this schema.

CREATE TABLE portable_runtime_model_audit (
    request_id TEXT PRIMARY KEY NOT NULL CHECK (
        length(request_id) = 36
        AND request_id = lower(request_id)
    ),
    character_id TEXT NOT NULL CHECK (
        length(character_id) BETWEEN 1 AND 256
        AND character_id = trim(character_id)
    ),
    character_content_revision_id TEXT CHECK (
        character_content_revision_id IS NULL
        OR (
            length(character_content_revision_id) BETWEEN 1 AND 256
            AND character_content_revision_id = trim(character_content_revision_id)
        )
    ),
    capability TEXT NOT NULL CHECK (
        capability IN ('model:primary', 'model:auxiliary')
    ),
    grant_sha256 TEXT NOT NULL CHECK (
        length(grant_sha256) = 64
        AND grant_sha256 = lower(grant_sha256)
        AND grant_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    provider_connection_id TEXT NOT NULL CHECK (
        length(provider_connection_id) BETWEEN 1 AND 256
        AND provider_connection_id = trim(provider_connection_id)
    ),
    model_route_id TEXT CHECK (
        model_route_id IS NULL
        OR (length(model_route_id) BETWEEN 1 AND 256 AND model_route_id = trim(model_route_id))
    ),
    generation_preset_id TEXT CHECK (
        generation_preset_id IS NULL
        OR (
            length(generation_preset_id) BETWEEN 1 AND 256
            AND generation_preset_id = trim(generation_preset_id)
        )
    ),
    started_at TEXT NOT NULL CHECK (length(trim(started_at)) > 0),
    completed_at TEXT,
    status TEXT NOT NULL CHECK (
        status IN (
            'started', 'succeeded', 'cancelled', 'unknown_outcome', 'failed', 'interrupted'
        )
    ),
    input_tokens INTEGER CHECK (input_tokens IS NULL OR input_tokens >= 0),
    output_tokens INTEGER CHECK (output_tokens IS NULL OR output_tokens >= 0),
    reasoning_tokens INTEGER CHECK (reasoning_tokens IS NULL OR reasoning_tokens >= 0),
    tool_tokens INTEGER CHECK (tool_tokens IS NULL OR tool_tokens >= 0),
    failure_code TEXT CHECK (
        failure_code IS NULL
        OR (length(failure_code) BETWEEN 1 AND 128 AND failure_code = trim(failure_code))
    ),
    CHECK (
        (
            status = 'started'
            AND completed_at IS NULL
            AND input_tokens IS NULL
            AND output_tokens IS NULL
            AND reasoning_tokens IS NULL
            AND tool_tokens IS NULL
            AND failure_code IS NULL
        )
        OR (
            status = 'succeeded'
            AND completed_at IS NOT NULL
            AND failure_code IS NULL
        )
        OR (
            status IN ('cancelled', 'unknown_outcome', 'failed', 'interrupted')
            AND completed_at IS NOT NULL
            AND failure_code IS NOT NULL
        )
    )
);

CREATE INDEX portable_runtime_model_audit_by_character
    ON portable_runtime_model_audit(character_id, started_at DESC, request_id);

CREATE TRIGGER portable_runtime_model_audit_terminal_guard
BEFORE UPDATE ON portable_runtime_model_audit
WHEN
    OLD.status != 'started'
    OR NEW.request_id != OLD.request_id
    OR NEW.character_id != OLD.character_id
    OR NEW.character_content_revision_id IS NOT OLD.character_content_revision_id
    OR NEW.capability != OLD.capability
    OR NEW.grant_sha256 != OLD.grant_sha256
    OR NEW.provider_connection_id != OLD.provider_connection_id
    OR NEW.model_route_id IS NOT OLD.model_route_id
    OR NEW.generation_preset_id IS NOT OLD.generation_preset_id
    OR NEW.started_at != OLD.started_at
    OR NEW.status = 'started'
BEGIN
    SELECT RAISE(ABORT, 'portable runtime model audit transition is invalid');
END;

CREATE TRIGGER portable_runtime_model_audit_no_delete
BEFORE DELETE ON portable_runtime_model_audit
BEGIN
    SELECT RAISE(ABORT, 'portable runtime model audit is durable');
END;
