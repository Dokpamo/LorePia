PRAGMA foreign_keys = ON;

ALTER TABLE generations ADD COLUMN provider_family TEXT
    CHECK (
        provider_family IS NULL
        OR provider_family IN (
            'openai_responses',
            'openai_chat_completions',
            'anthropic_messages',
            'gemini_generate_content',
            'ollama_native'
        )
    );

ALTER TABLE generations ADD COLUMN cached_read_tokens INTEGER
    CHECK (cached_read_tokens IS NULL OR cached_read_tokens >= 0);
ALTER TABLE generations ADD COLUMN cached_write_tokens INTEGER
    CHECK (cached_write_tokens IS NULL OR cached_write_tokens >= 0);
ALTER TABLE generations ADD COLUMN reasoning_tokens INTEGER
    CHECK (reasoning_tokens IS NULL OR reasoning_tokens >= 0);
ALTER TABLE generations ADD COLUMN tool_tokens INTEGER
    CHECK (tool_tokens IS NULL OR tool_tokens >= 0);

ALTER TABLE generations ADD COLUMN provider_raw_summary_json TEXT
    CHECK (
        provider_raw_summary_json IS NULL
        OR (
            json_valid(provider_raw_summary_json)
            AND json_type(provider_raw_summary_json) = 'object'
            AND length(CAST(provider_raw_summary_json AS BLOB)) <= 4096
        )
    );

ALTER TABLE generations ADD COLUMN opaque_reasoning_state_json TEXT
    CHECK (
        opaque_reasoning_state_json IS NULL
        OR (
            json_valid(opaque_reasoning_state_json)
            AND json_type(opaque_reasoning_state_json) = 'array'
            -- Keep this durable v8 envelope in sync with
            -- MAX_OPAQUE_REASONING_SERIALIZED_BYTES (264 KiB).
            AND length(CAST(opaque_reasoning_state_json AS BLOB)) <= 270336
        )
    );

-- Rows written between the provenance and protocol-state migrations can be
-- backfilled exactly from their immutable model route.
UPDATE generations
SET provider_family = (
    SELECT api_family
    FROM provider_models
    WHERE provider_models.id = generations.model_route_id
)
WHERE model_route_id IS NOT NULL;

CREATE TRIGGER generations_protocol_state_insert_guard
BEFORE INSERT ON generations
WHEN
    (NEW.model_route_id IS NULL) <> (NEW.provider_family IS NULL)
    OR (
        NEW.model_route_id IS NOT NULL
        AND NOT EXISTS (
            SELECT 1
            FROM provider_models
            WHERE provider_models.id = NEW.model_route_id
              AND provider_models.api_family = NEW.provider_family
        )
    )
    OR (
        NEW.opaque_reasoning_state_json IS NOT NULL
        AND (
            NEW.status <> 'complete'
            OR
            NEW.model_route_id IS NULL
            OR NEW.generation_preset_id IS NULL
            OR NEW.provider_family IS NULL
        )
    )
BEGIN
    SELECT RAISE(ABORT, 'generation protocol-state provenance is inconsistent');
END;

CREATE TRIGGER generations_protocol_state_update_guard
BEFORE UPDATE OF
    model_route_id,
    generation_preset_id,
    provider_family,
    status,
    opaque_reasoning_state_json
ON generations
WHEN
    (NEW.model_route_id IS NULL) <> (NEW.provider_family IS NULL)
    OR (
        NEW.model_route_id IS NOT NULL
        AND NOT EXISTS (
            SELECT 1
            FROM provider_models
            WHERE provider_models.id = NEW.model_route_id
              AND provider_models.api_family = NEW.provider_family
        )
    )
    OR (
        NEW.opaque_reasoning_state_json IS NOT NULL
        AND (
            NEW.status <> 'complete'
            OR
            NEW.model_route_id IS NULL
            OR NEW.generation_preset_id IS NULL
            OR NEW.provider_family IS NULL
        )
    )
BEGIN
    SELECT RAISE(ABORT, 'generation protocol-state provenance is inconsistent');
END;
