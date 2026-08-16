PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS provider_templates (
    id TEXT NOT NULL CHECK (length(trim(id)) > 0),
    version INTEGER NOT NULL CHECK (version > 0),
    display_name TEXT NOT NULL CHECK (length(trim(display_name)) > 0),
    source_kind TEXT NOT NULL CHECK (
        source_kind IN ('built_in', 'user_discovered', 'signed_catalog')
    ),
    manifest_json TEXT NOT NULL CHECK (json_valid(manifest_json)),
    manifest_sha256 TEXT NOT NULL CHECK (
        length(manifest_sha256) = 64
        AND manifest_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    PRIMARY KEY (id, version)
);

CREATE INDEX IF NOT EXISTS provider_templates_source_created
    ON provider_templates(source_kind, created_at, id, version);

CREATE TABLE IF NOT EXISTS provider_connections (
    id TEXT PRIMARY KEY CHECK (length(trim(id)) > 0),
    template_id TEXT NOT NULL CHECK (length(trim(template_id)) > 0),
    template_version INTEGER NOT NULL CHECK (template_version > 0),
    display_name TEXT NOT NULL CHECK (length(trim(display_name)) > 0),
    api_origin TEXT NOT NULL CHECK (length(trim(api_origin)) > 0),
    config_json TEXT NOT NULL CHECK (json_valid(config_json)),
    credential_ref TEXT CHECK (
        credential_ref IS NULL OR length(trim(credential_ref)) > 0
    ),
    credential_scope_json TEXT CHECK (
        credential_scope_json IS NULL OR json_valid(credential_scope_json)
    ),
    timeout_seconds INTEGER NOT NULL CHECK (
        timeout_seconds BETWEEN 1 AND 600
    ),
    status TEXT NOT NULL CHECK (
        status IN ('untested', 'connected', 'auth_failed', 'unavailable')
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    updated_at TEXT NOT NULL CHECK (length(trim(updated_at)) > 0),
    FOREIGN KEY (template_id, template_version)
        REFERENCES provider_templates(id, version)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

CREATE INDEX IF NOT EXISTS provider_connections_template
    ON provider_connections(template_id, template_version, id);
CREATE INDEX IF NOT EXISTS provider_connections_status_updated
    ON provider_connections(status, updated_at, id);

CREATE TABLE IF NOT EXISTS provider_models (
    id TEXT PRIMARY KEY CHECK (length(trim(id)) > 0),
    connection_id TEXT NOT NULL
        REFERENCES provider_connections(id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE,
    api_family TEXT NOT NULL CHECK (
        api_family IN (
            'openai_responses',
            'openai_chat_completions',
            'anthropic_messages',
            'gemini_generate_content',
            'ollama_native'
        )
    ),
    model_id TEXT NOT NULL CHECK (length(trim(model_id)) > 0),
    display_name TEXT CHECK (
        display_name IS NULL OR length(trim(display_name)) > 0
    ),
    route_json TEXT NOT NULL CHECK (json_valid(route_json)),
    availability TEXT NOT NULL CHECK (
        availability IN (
            'available',
            'missing_temporarily',
            'documented_only',
            'access_denied',
            'deprecated',
            'retired',
            'unknown'
        )
    ),
    raw_metadata_json TEXT CHECK (
        raw_metadata_json IS NULL OR json_valid(raw_metadata_json)
    ),
    first_seen_at TEXT NOT NULL CHECK (length(trim(first_seen_at)) > 0),
    last_seen_at TEXT CHECK (
        last_seen_at IS NULL OR length(trim(last_seen_at)) > 0
    ),
    UNIQUE (connection_id, api_family, model_id, route_json)
);

CREATE INDEX IF NOT EXISTS provider_models_connection_availability
    ON provider_models(connection_id, availability, model_id, id);
CREATE INDEX IF NOT EXISTS provider_models_last_seen
    ON provider_models(connection_id, last_seen_at, id);

CREATE TABLE IF NOT EXISTS model_capability_observations (
    id TEXT PRIMARY KEY CHECK (length(trim(id)) > 0),
    model_route_id TEXT NOT NULL
        REFERENCES provider_models(id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE,
    capability_key TEXT NOT NULL CHECK (
        capability_key IN (
            'streaming',
            'reasoning',
            'prompt_caching',
            'tool_calling',
            'parallel_tool_calling',
            'structured_output',
            'json_mode',
            'image_input',
            'audio_input',
            'audio_output',
            'logprobs',
            'seed',
            'batch',
            'background',
            'context_window',
            'max_output_tokens'
        )
    ),
    value_json TEXT NOT NULL CHECK (json_valid(value_json)),
    support_status TEXT NOT NULL CHECK (
        support_status IN (
            'verified',
            'documented',
            'inferred',
            'unsupported',
            'unknown',
            'conditional'
        )
    ),
    source_kind TEXT NOT NULL CHECK (
        source_kind IN (
            'provider_api',
            'official_documentation',
            'signed_lorepia_catalog',
            'capability_probe',
            'user_override',
            'llm_inference'
        )
    ),
    confidence TEXT NOT NULL CHECK (
        confidence IN ('high', 'medium', 'low')
    ),
    evidence_ref TEXT CHECK (
        evidence_ref IS NULL OR length(trim(evidence_ref)) > 0
    ),
    observed_at TEXT NOT NULL CHECK (length(trim(observed_at)) > 0),
    expires_at TEXT CHECK (
        expires_at IS NULL OR length(trim(expires_at)) > 0
    )
);

CREATE INDEX IF NOT EXISTS capability_observations_route_key_observed
    ON model_capability_observations(
        model_route_id,
        capability_key,
        observed_at DESC,
        id
    );
CREATE INDEX IF NOT EXISTS capability_observations_expiry
    ON model_capability_observations(expires_at, id)
    WHERE expires_at IS NOT NULL;

CREATE TABLE IF NOT EXISTS generation_presets (
    id TEXT PRIMARY KEY CHECK (length(trim(id)) > 0),
    model_route_id TEXT NOT NULL
        REFERENCES provider_models(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    display_name TEXT NOT NULL CHECK (length(trim(display_name)) > 0),
    values_json TEXT NOT NULL CHECK (json_valid(values_json)),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    updated_at TEXT NOT NULL CHECK (length(trim(updated_at)) > 0)
);

CREATE INDEX IF NOT EXISTS generation_presets_model_route
    ON generation_presets(model_route_id, updated_at DESC, id);

CREATE TABLE IF NOT EXISTS provider_discovery_sessions (
    id TEXT PRIMARY KEY CHECK (length(trim(id)) > 0),
    state TEXT NOT NULL CHECK (
        state IN (
            'draft',
            'resolving_known_provider',
            'fetching_documents',
            'extracting_evidence',
            'awaiting_assistant_consent',
            'building_manifest_draft',
            'validating_manifest',
            'awaiting_credential_origin_approval',
            'listing_models',
            'awaiting_probe_consent',
            'probing_capabilities',
            'awaiting_review',
            'committing',
            'ready',
            'failed',
            'cancelled'
        )
    ),
    sanitized_input_json TEXT NOT NULL CHECK (
        json_valid(sanitized_input_json)
    ),
    draft_json TEXT CHECK (draft_json IS NULL OR json_valid(draft_json)),
    error_json TEXT CHECK (error_json IS NULL OR json_valid(error_json)),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    updated_at TEXT NOT NULL CHECK (length(trim(updated_at)) > 0)
);

CREATE INDEX IF NOT EXISTS provider_discovery_sessions_state_updated
    ON provider_discovery_sessions(state, updated_at, id);

CREATE TABLE IF NOT EXISTS provider_discovery_evidence (
    id TEXT PRIMARY KEY CHECK (length(trim(id)) > 0),
    session_id TEXT NOT NULL
        REFERENCES provider_discovery_sessions(id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (length(trim(kind)) > 0),
    source_url TEXT NOT NULL CHECK (length(trim(source_url)) > 0),
    content_sha256 TEXT NOT NULL CHECK (
        length(content_sha256) = 64
        AND content_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    extracted_json TEXT NOT NULL CHECK (json_valid(extracted_json)),
    fetched_at TEXT NOT NULL CHECK (length(trim(fetched_at)) > 0)
);

CREATE INDEX IF NOT EXISTS provider_discovery_evidence_session_fetched
    ON provider_discovery_evidence(session_id, fetched_at, id);
CREATE INDEX IF NOT EXISTS provider_discovery_evidence_source_hash
    ON provider_discovery_evidence(source_url, content_sha256, id);
