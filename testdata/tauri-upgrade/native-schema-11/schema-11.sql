PRAGMA foreign_keys=OFF;
BEGIN TRANSACTION;
CREATE TABLE schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL
);
INSERT INTO schema_migrations VALUES(1,'2026-08-11T02:50:24.933207+00:00');
INSERT INTO schema_migrations VALUES(2,'2026-08-11T02:50:24.933506+00:00');
INSERT INTO schema_migrations VALUES(3,'2026-08-11T02:50:24.935001+00:00');
INSERT INTO schema_migrations VALUES(4,'2026-08-11T02:50:24.937131+00:00');
INSERT INTO schema_migrations VALUES(5,'2026-08-11T02:50:24.943390+00:00');
INSERT INTO schema_migrations VALUES(6,'2026-08-11T02:50:24.946686+00:00');
INSERT INTO schema_migrations VALUES(7,'2026-08-11T02:50:24.948458+00:00');
INSERT INTO schema_migrations VALUES(8,'2026-08-11T02:50:24.958292+00:00');
INSERT INTO schema_migrations VALUES(9,'2026-08-11T02:50:24.966193+00:00');
INSERT INTO schema_migrations VALUES(10,'2026-08-11T02:50:24.968070+00:00');
INSERT INTO schema_migrations VALUES(11,'2026-08-11T02:50:24.968651+00:00');
CREATE TABLE content_sources (
    sha256 TEXT PRIMARY KEY,
    relative_path TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    created_at TEXT NOT NULL
);
INSERT INTO content_sources VALUES('2c528a64fbf36a011e29c1a692cd13568b83f76e764ea03487393c28a2e666de','sources/sha256/2c/528a64fbf36a011e29c1a692cd13568b83f76e764ea03487393c28a2e666de',383,'2026-08-11T02:50:25.025013+00:00');
CREATE TABLE characters (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    source_hash TEXT NOT NULL REFERENCES content_sources(sha256),
    avatar_asset_hash TEXT,
    created_at TEXT NOT NULL
);
INSERT INTO characters VALUES('3c0ffb8c-e24b-48a0-ad31-d0455665c290','Synthetic Avatar','Asset persistence fixture','2c528a64fbf36a011e29c1a692cd13568b83f76e764ea03487393c28a2e666de','aa7bb0431aaeb198a77c26a14fe6dd714a75e4d7db94e3e1238a1fdcbfe1f8d4','2026-08-11T02:50:24.992914+00:00');
CREATE TABLE assets (
    sha256 TEXT PRIMARY KEY,
    relative_path TEXT NOT NULL,
    media_type TEXT,
    size_bytes INTEGER NOT NULL,
    created_at TEXT NOT NULL
);
INSERT INTO assets VALUES('aa7bb0431aaeb198a77c26a14fe6dd714a75e4d7db94e3e1238a1fdcbfe1f8d4','assets/sha256/aa/7bb0431aaeb198a77c26a14fe6dd714a75e4d7db94e3e1238a1fdcbfe1f8d4','image/png',70,'2026-08-11T02:50:25.025035+00:00');
CREATE TABLE character_assets (
    character_id TEXT NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
    asset_hash TEXT NOT NULL REFERENCES assets(sha256),
    role TEXT NOT NULL,
    PRIMARY KEY (character_id, asset_hash, role)
);
INSERT INTO character_assets VALUES('3c0ffb8c-e24b-48a0-ad31-d0455665c290','aa7bb0431aaeb198a77c26a14fe6dd714a75e4d7db94e3e1238a1fdcbfe1f8d4','avatar');
CREATE TABLE conversations (
    id TEXT PRIMARY KEY,
    character_id TEXT NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
INSERT INTO conversations VALUES('2faca127-ff70-4acb-b26f-60f1042b8d11','3c0ffb8c-e24b-48a0-ad31-d0455665c290','Synthetic continuity conversation','2026-08-11T02:50:25.095639+00:00','2026-08-11T02:50:25.114899+00:00');
CREATE TABLE provider_profiles (
    id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    base_url TEXT NOT NULL,
    model TEXT NOT NULL,
    timeout_seconds INTEGER NOT NULL
);
CREATE TABLE app_settings (
    key TEXT PRIMARY KEY,
    value_json TEXT NOT NULL
);
INSERT INTO app_settings VALUES('application','{"preserve_partial_generations":true,"selected_provider_profile_id":null,"selected_model_route_id":"fixture-openai-route","selected_generation_preset_id":"fixture-openai-preset"}');
CREATE TABLE import_jobs (
    id TEXT PRIMARY KEY,
    source_hash TEXT NOT NULL,
    staging_path TEXT NOT NULL,
    state TEXT NOT NULL,
    updated_at TEXT NOT NULL
, asset_hashes_json TEXT NOT NULL DEFAULT '[]');
CREATE TABLE messages (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    parent_id TEXT,
    role TEXT NOT NULL CHECK (role IN ('system', 'user', 'assistant')),
    content TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'complete', 'cancelled', 'failed')),
    generation_id TEXT,
    created_at TEXT NOT NULL,
    UNIQUE (conversation_id, id),
    FOREIGN KEY (conversation_id, parent_id)
        REFERENCES messages(conversation_id, id),
    CHECK (parent_id IS NULL OR parent_id <> id),
    CHECK (role = 'assistant' OR status = 'complete'),
    CHECK (
        (role = 'assistant' AND generation_id IS NOT NULL)
        OR (role <> 'assistant' AND generation_id IS NULL)
    )
);
INSERT INTO messages VALUES('ba449e07-4475-4ade-9a61-1547212b9536','2faca127-ff70-4acb-b26f-60f1042b8d11',NULL,'user','Synthetic user message.','complete',NULL,'2026-08-11T02:50:25.112587+00:00');
INSERT INTO messages VALUES('30ef489c-24c5-436e-9e77-5de4e6358e48','2faca127-ff70-4acb-b26f-60f1042b8d11','ba449e07-4475-4ade-9a61-1547212b9536','assistant','Synthetic assistant reply.','complete','b2c74070-4ef3-4645-a530-a15062d3921a','2026-08-11T02:50:25.112859+00:00');
CREATE TABLE conversation_branches (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    title TEXT,
    fork_message_id TEXT,
    head_message_id TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (conversation_id, id),
    FOREIGN KEY (conversation_id, fork_message_id)
        REFERENCES messages(conversation_id, id),
    FOREIGN KEY (conversation_id, head_message_id)
        REFERENCES messages(conversation_id, id),
    CHECK (fork_message_id IS NULL OR fork_message_id <> '')
);
INSERT INTO conversation_branches VALUES('e1a0da4e-ee02-4709-8b0d-f6d3f213f1bf','2faca127-ff70-4acb-b26f-60f1042b8d11',NULL,NULL,'30ef489c-24c5-436e-9e77-5de4e6358e48','2026-08-11T02:50:25.095647+00:00','2026-08-11T02:50:25.113043+00:00');
CREATE TABLE conversation_state (
    conversation_id TEXT PRIMARY KEY REFERENCES conversations(id) ON DELETE CASCADE,
    active_branch_id TEXT NOT NULL,
    selected_mode TEXT NOT NULL CHECK (selected_mode IN ('chat', 'story')),
    updated_at TEXT NOT NULL,
    FOREIGN KEY (conversation_id, active_branch_id)
        REFERENCES conversation_branches(conversation_id, id)
);
INSERT INTO conversation_state VALUES('2faca127-ff70-4acb-b26f-60f1042b8d11','e1a0da4e-ee02-4709-8b0d-f6d3f213f1bf','chat','2026-08-11T02:50:25.095639+00:00');
CREATE TABLE generations (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    branch_id TEXT NOT NULL,
    user_message_id TEXT NOT NULL REFERENCES messages(id),
    assistant_message_id TEXT UNIQUE REFERENCES messages(id) ON DELETE SET NULL,
    mode TEXT NOT NULL CHECK (mode IN ('chat', 'story')),
    model TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('running', 'complete', 'cancelled', 'failed')),
    input_tokens INTEGER,
    output_tokens INTEGER,
    error_code TEXT,
    started_at TEXT NOT NULL,
    finished_at TEXT, model_route_id TEXT
        REFERENCES provider_models(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT, generation_preset_id TEXT
        REFERENCES generation_presets(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT, provider_family TEXT
    CHECK (
        provider_family IS NULL
        OR provider_family IN (
            'openai_responses',
            'openai_chat_completions',
            'anthropic_messages',
            'gemini_generate_content',
            'ollama_native'
        )
    ), cached_read_tokens INTEGER
    CHECK (cached_read_tokens IS NULL OR cached_read_tokens >= 0), cached_write_tokens INTEGER
    CHECK (cached_write_tokens IS NULL OR cached_write_tokens >= 0), reasoning_tokens INTEGER
    CHECK (reasoning_tokens IS NULL OR reasoning_tokens >= 0), tool_tokens INTEGER
    CHECK (tool_tokens IS NULL OR tool_tokens >= 0), provider_raw_summary_json TEXT
    CHECK (
        provider_raw_summary_json IS NULL
        OR (
            json_valid(provider_raw_summary_json)
            AND json_type(provider_raw_summary_json) = 'object'
            AND length(CAST(provider_raw_summary_json AS BLOB)) <= 4096
        )
    ), opaque_reasoning_state_json TEXT
    CHECK (
        opaque_reasoning_state_json IS NULL
        OR (
            json_valid(opaque_reasoning_state_json)
            AND json_type(opaque_reasoning_state_json) = 'array'
            -- Keep this durable v8 envelope in sync with
            -- MAX_OPAQUE_REASONING_SERIALIZED_BYTES (264 KiB).
            AND length(CAST(opaque_reasoning_state_json AS BLOB)) <= 270336
        )
    ),
    FOREIGN KEY (conversation_id, branch_id)
        REFERENCES conversation_branches(conversation_id, id),
    CHECK (input_tokens IS NULL OR input_tokens >= 0),
    CHECK (output_tokens IS NULL OR output_tokens >= 0),
    CHECK (
        (status = 'running' AND finished_at IS NULL)
        OR (status <> 'running' AND finished_at IS NOT NULL)
    )
);
INSERT INTO generations VALUES('b2c74070-4ef3-4645-a530-a15062d3921a','2faca127-ff70-4acb-b26f-60f1042b8d11','e1a0da4e-ee02-4709-8b0d-f6d3f213f1bf','ba449e07-4475-4ade-9a61-1547212b9536','30ef489c-24c5-436e-9e77-5de4e6358e48','chat','fixture-model','complete',7,3,NULL,'2026-08-11T02:50:25.112859+00:00','2026-08-11T02:50:25.114899+00:00','fixture-openai-route','fixture-openai-preset','openai_chat_completions',NULL,NULL,NULL,NULL,NULL,NULL);
CREATE TABLE provider_templates (
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
INSERT INTO provider_templates VALUES('custom-openai-chat-v1',1,'Custom OpenAI-compatible Chat','built_in','{"id":"custom-openai-chat-v1","display_name":"Custom OpenAI-compatible Chat","manifest_version":1,"source":"built_in","api_family":"openai_chat_completions","connection_fields":[{"key":"api_base_url","label_key":"provider.connection.api_base_url","description_key":"provider.connection.api_base_url.description","value_type":"text","required":true},{"key":"api_key","label_key":"provider.connection.api_key","description_key":"provider.connection.api_key.description","value_type":"credential","required":false}],"default_manifest":{"schema_version":1,"api_family":"openai_chat_completions","sources":[],"default_api_origin":null,"auth":{"kind":"bearer_header"},"endpoints":{"models":{"method":"GET","path":"/models"},"generate":{"method":"POST","path":"/chat/completions"}},"decoders":{"response":"open_ai_json_v1","streaming":"open_ai_sse_v1"},"parameters":[{"id":"temperature","label_key":"provider.parameter.temperature","description_key":"provider.parameter.temperature.description","value_type":"number","allowed_values":[],"minimum":0.0,"maximum":2.0,"step":0.1,"default_mode":"provider_default","visibility":null,"conflicts":[],"provider_mapping":{"target":"request_body","field_name":"temperature"},"level":"basic"},{"id":"max_output_tokens","label_key":"provider.parameter.max_output_tokens","description_key":"provider.parameter.max_output_tokens.description","value_type":"integer","allowed_values":[],"minimum":1.0,"maximum":4294967295.0,"step":1.0,"default_mode":"provider_default","visibility":null,"conflicts":[],"provider_mapping":{"target":"request_body","field_name":"max_tokens"},"level":"basic"}]}}','a9d7b683bfcdadafc6d0f45d7ebe3d2af287ae71ec79fbeed733a083eb024356','2026-08-11T02:50:24.936861+00:00');
INSERT INTO provider_templates VALUES('openai-responses-v1',2,'OpenAI','built_in','{"id":"openai-responses-v1","display_name":"OpenAI","manifest_version":2,"source":"built_in","api_family":"openai_responses","connection_fields":[{"key":"api_key","label_key":"provider.connection.api_key","description_key":"provider.connection.api_key.description","value_type":"credential","required":true}],"default_manifest":{"schema_version":1,"api_family":"openai_responses","sources":[{"kind":"official_site","url":"https://platform.openai.com/docs","content_sha256":null},{"kind":"official_documentation","url":"https://platform.openai.com/docs/api-reference/models","content_sha256":null}],"default_api_origin":"https://api.openai.com","auth":{"kind":"bearer_header"},"endpoints":{"models":{"method":"GET","path":"/models"},"generate":{"method":"POST","path":"/responses"}},"decoders":{"response":"open_ai_json_v1","streaming":"open_ai_sse_v1"},"parameters":[{"id":"temperature","label_key":"provider.parameter.temperature","description_key":"provider.parameter.temperature.description","value_type":"number","allowed_values":[],"minimum":0.0,"maximum":2.0,"step":null,"default_mode":"provider_default","visibility":null,"conflicts":[],"provider_mapping":{"target":"request_body","field_name":"temperature"},"level":"basic"},{"id":"max_output_tokens","label_key":"provider.parameter.max_output_tokens","description_key":"provider.parameter.max_output_tokens.description","value_type":"integer","allowed_values":[],"minimum":1.0,"maximum":4294967295.0,"step":1.0,"default_mode":"provider_default","visibility":null,"conflicts":[],"provider_mapping":{"target":"request_body","field_name":"max_output_tokens"},"level":"basic"},{"id":"top_p","label_key":"provider.parameter.top_p","description_key":"provider.parameter.top_p.description","value_type":"number","allowed_values":[],"minimum":0.0,"maximum":1.0,"step":null,"default_mode":"provider_default","visibility":null,"conflicts":[],"provider_mapping":{"target":"request_body","field_name":"top_p"},"level":"advanced"}]}}','bc472d82f97d7ef9a6efb51eee389b4774cc3ed6e301e71058565b5095a16d08','2026-08-11T02:50:24.971958+00:00');
INSERT INTO provider_templates VALUES('openai-chat-compatible-v1',2,'Custom OpenAI-compatible Chat','built_in','{"id":"openai-chat-compatible-v1","display_name":"Custom OpenAI-compatible Chat","manifest_version":2,"source":"built_in","api_family":"openai_chat_completions","connection_fields":[{"key":"api_base_url","label_key":"provider.connection.api_base_url","description_key":"provider.connection.api_base_url.description","value_type":"text","required":true},{"key":"api_key","label_key":"provider.connection.api_key","description_key":"provider.connection.api_key.description","value_type":"credential","required":true}],"default_manifest":{"schema_version":1,"api_family":"openai_chat_completions","sources":[],"default_api_origin":null,"auth":{"kind":"bearer_header"},"endpoints":{"models":{"method":"GET","path":"/models"},"generate":{"method":"POST","path":"/chat/completions"}},"decoders":{"response":"open_ai_json_v1","streaming":"open_ai_sse_v1"},"parameters":[{"id":"temperature","label_key":"provider.parameter.temperature","description_key":"provider.parameter.temperature.description","value_type":"number","allowed_values":[],"minimum":0.0,"maximum":2.0,"step":null,"default_mode":"provider_default","visibility":null,"conflicts":[],"provider_mapping":{"target":"request_body","field_name":"temperature"},"level":"basic"},{"id":"max_output_tokens","label_key":"provider.parameter.max_output_tokens","description_key":"provider.parameter.max_output_tokens.description","value_type":"integer","allowed_values":[],"minimum":1.0,"maximum":4294967295.0,"step":1.0,"default_mode":"provider_default","visibility":null,"conflicts":[],"provider_mapping":{"target":"request_body","field_name":"max_tokens"},"level":"basic"},{"id":"top_p","label_key":"provider.parameter.top_p","description_key":"provider.parameter.top_p.description","value_type":"number","allowed_values":[],"minimum":0.0,"maximum":1.0,"step":null,"default_mode":"provider_default","visibility":null,"conflicts":[],"provider_mapping":{"target":"request_body","field_name":"top_p"},"level":"advanced"}]}}','50e2f905db39e53b88097671b8f44fd7a6a7e302fb7d9bcbd7e8f64fe52d61ad','2026-08-11T02:50:24.972311+00:00');
INSERT INTO provider_templates VALUES('anthropic-messages-v1',2,'Anthropic','built_in','{"id":"anthropic-messages-v1","display_name":"Anthropic","manifest_version":2,"source":"built_in","api_family":"anthropic_messages","connection_fields":[{"key":"api_key","label_key":"provider.connection.api_key","description_key":"provider.connection.api_key.description","value_type":"credential","required":true}],"default_manifest":{"schema_version":1,"api_family":"anthropic_messages","sources":[{"kind":"official_site","url":"https://platform.claude.com/docs","content_sha256":null},{"kind":"official_documentation","url":"https://platform.claude.com/docs/en/api/models/list","content_sha256":null}],"default_api_origin":"https://api.anthropic.com","auth":{"kind":"header_api_key","header_name":"x-api-key"},"endpoints":{"models":{"method":"GET","path":"/models"},"generate":{"method":"POST","path":"/messages"}},"decoders":{"response":"anthropic_json_v1","streaming":"anthropic_sse_v1"},"parameters":[{"id":"temperature","label_key":"provider.parameter.temperature","description_key":"provider.parameter.temperature.description","value_type":"number","allowed_values":[],"minimum":0.0,"maximum":1.0,"step":null,"default_mode":"provider_default","visibility":null,"conflicts":[],"provider_mapping":{"target":"request_body","field_name":"temperature"},"level":"basic"},{"id":"max_output_tokens","label_key":"provider.parameter.max_output_tokens","description_key":"provider.parameter.max_output_tokens.description","value_type":"integer","allowed_values":[],"minimum":1.0,"maximum":4294967295.0,"step":1.0,"default_mode":"explicit_required","visibility":null,"conflicts":[],"provider_mapping":{"target":"request_body","field_name":"max_tokens"},"level":"basic"}]}}','7089e3271b4f6019c36d233e0bd6fdb9484e03417d8e3aa1b182c1d47a46ac99','2026-08-11T02:50:24.972633+00:00');
INSERT INTO provider_templates VALUES('gemini-generate-content-v1',2,'Google Gemini','built_in','{"id":"gemini-generate-content-v1","display_name":"Google Gemini","manifest_version":2,"source":"built_in","api_family":"gemini_generate_content","connection_fields":[{"key":"api_key","label_key":"provider.connection.api_key","description_key":"provider.connection.api_key.description","value_type":"credential","required":true}],"default_manifest":{"schema_version":1,"api_family":"gemini_generate_content","sources":[{"kind":"official_site","url":"https://ai.google.dev/gemini-api/docs","content_sha256":null},{"kind":"official_documentation","url":"https://ai.google.dev/api/models","content_sha256":null}],"default_api_origin":"https://generativelanguage.googleapis.com","auth":{"kind":"header_api_key","header_name":"x-goog-api-key"},"endpoints":{"models":{"method":"GET","path":"/models"},"generate":{"method":"POST","path":"/models"}},"decoders":{"response":"gemini_json_v1","streaming":"gemini_sse_v1"},"parameters":[{"id":"temperature","label_key":"provider.parameter.temperature","description_key":"provider.parameter.temperature.description","value_type":"number","allowed_values":[],"minimum":0.0,"maximum":null,"step":null,"default_mode":"provider_default","visibility":null,"conflicts":[],"provider_mapping":{"target":"request_body","field_name":"generationConfig.temperature"},"level":"basic"},{"id":"max_output_tokens","label_key":"provider.parameter.max_output_tokens","description_key":"provider.parameter.max_output_tokens.description","value_type":"integer","allowed_values":[],"minimum":1.0,"maximum":4294967295.0,"step":1.0,"default_mode":"provider_default","visibility":null,"conflicts":[],"provider_mapping":{"target":"request_body","field_name":"generationConfig.maxOutputTokens"},"level":"basic"},{"id":"top_p","label_key":"provider.parameter.top_p","description_key":"provider.parameter.top_p.description","value_type":"number","allowed_values":[],"minimum":0.0,"maximum":1.0,"step":null,"default_mode":"provider_default","visibility":null,"conflicts":[],"provider_mapping":{"target":"request_body","field_name":"generationConfig.topP"},"level":"advanced"}]}}','f78e89de37973c8fcbef5fd0e217b6a00052fdba65f8b3ad9c82c9f49238c72c','2026-08-11T02:50:24.973035+00:00');
INSERT INTO provider_templates VALUES('openrouter-v1',2,'OpenRouter','built_in','{"id":"openrouter-v1","display_name":"OpenRouter","manifest_version":2,"source":"built_in","api_family":"openai_chat_completions","connection_fields":[{"key":"api_key","label_key":"provider.connection.api_key","description_key":"provider.connection.api_key.description","value_type":"credential","required":true}],"default_manifest":{"schema_version":1,"api_family":"openai_chat_completions","sources":[{"kind":"official_site","url":"https://openrouter.ai/docs","content_sha256":null},{"kind":"official_documentation","url":"https://openrouter.ai/docs/api/api-reference/models/get-models","content_sha256":null},{"kind":"official_documentation","url":"https://openrouter.ai/docs/api/api-reference/chat/send-chat-completion-request","content_sha256":null}],"default_api_origin":"https://openrouter.ai","auth":{"kind":"bearer_header"},"endpoints":{"models":{"method":"GET","path":"/models"},"generate":{"method":"POST","path":"/chat/completions"}},"decoders":{"response":"open_ai_json_v1","streaming":"open_ai_sse_v1"},"parameters":[{"id":"temperature","label_key":"provider.parameter.temperature","description_key":"provider.parameter.temperature.description","value_type":"number","allowed_values":[],"minimum":0.0,"maximum":2.0,"step":null,"default_mode":"provider_default","visibility":null,"conflicts":[],"provider_mapping":{"target":"request_body","field_name":"temperature"},"level":"basic"},{"id":"max_output_tokens","label_key":"provider.parameter.max_output_tokens","description_key":"provider.parameter.max_output_tokens.description","value_type":"integer","allowed_values":[],"minimum":1.0,"maximum":4294967295.0,"step":1.0,"default_mode":"provider_default","visibility":null,"conflicts":[],"provider_mapping":{"target":"request_body","field_name":"max_tokens"},"level":"basic"},{"id":"top_p","label_key":"provider.parameter.top_p","description_key":"provider.parameter.top_p.description","value_type":"number","allowed_values":[],"minimum":0.0,"maximum":1.0,"step":null,"default_mode":"provider_default","visibility":null,"conflicts":[],"provider_mapping":{"target":"request_body","field_name":"top_p"},"level":"advanced"}]}}','015e68d83ddb4d7c1a4456de801c68210450f3afcc8a1fa113d99de6b62080ec','2026-08-11T02:50:24.973536+00:00');
INSERT INTO provider_templates VALUES('ollama-native-v1',2,'Ollama','built_in','{"id":"ollama-native-v1","display_name":"Ollama","manifest_version":2,"source":"built_in","api_family":"ollama_native","connection_fields":[{"key":"api_base_url","label_key":"provider.connection.api_base_url","description_key":"provider.connection.api_base_url.description","value_type":"text","required":false}],"default_manifest":{"schema_version":1,"api_family":"ollama_native","sources":[{"kind":"official_site","url":"https://docs.ollama.com/","content_sha256":null},{"kind":"official_documentation","url":"https://docs.ollama.com/api/tags","content_sha256":null}],"default_api_origin":"http://localhost:11434","auth":{"kind":"none"},"endpoints":{"models":{"method":"GET","path":"/tags"},"generate":{"method":"POST","path":"/chat"}},"decoders":{"response":"ollama_json_v1","streaming":"ollama_jsonl_v1"},"parameters":[{"id":"temperature","label_key":"provider.parameter.temperature","description_key":"provider.parameter.temperature.description","value_type":"number","allowed_values":[],"minimum":0.0,"maximum":null,"step":null,"default_mode":"provider_default","visibility":null,"conflicts":[],"provider_mapping":{"target":"request_body","field_name":"options.temperature"},"level":"basic"},{"id":"max_output_tokens","label_key":"provider.parameter.max_output_tokens","description_key":"provider.parameter.max_output_tokens.description","value_type":"integer","allowed_values":[],"minimum":1.0,"maximum":4294967295.0,"step":1.0,"default_mode":"provider_default","visibility":null,"conflicts":[],"provider_mapping":{"target":"request_body","field_name":"options.num_predict"},"level":"basic"},{"id":"top_p","label_key":"provider.parameter.top_p","description_key":"provider.parameter.top_p.description","value_type":"number","allowed_values":[],"minimum":0.0,"maximum":1.0,"step":null,"default_mode":"provider_default","visibility":null,"conflicts":[],"provider_mapping":{"target":"request_body","field_name":"options.top_p"},"level":"advanced"}]}}','53d3822ff360c7c3d510ffe3e9b1c8f067c45634c0f2e91b3406465bc731e7e2','2026-08-11T02:50:24.973927+00:00');
CREATE TABLE provider_connections (
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
    updated_at TEXT NOT NULL CHECK (length(trim(updated_at)) > 0), archived_at TEXT
    CHECK (
        archived_at IS NULL
        OR length(trim(archived_at)) > 0
    ),
    FOREIGN KEY (template_id, template_version)
        REFERENCES provider_templates(id, version)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);
INSERT INTO provider_connections VALUES('fixture-openai-loopback','openai-chat-compatible-v1',2,'Synthetic loopback fixture','http://127.0.0.1:38081','{"api_base_path":"/v1","network_mode":"local_loopback","local_network_approval":null,"values":[{"key":"api_base_url","value":{"type":"text","value":"http://127.0.0.1:38081/v1"}}]}','fixture-openai-loopback','{"allowed_origins":["http://127.0.0.1:38081"],"auth_binding":{"kind":"bearer_header"},"redirect_policy":"deny"}',5,'untested','2026-08-11T02:50:25.060425+00:00','2026-08-11T02:50:25.060425+00:00',NULL);
CREATE TABLE provider_models (
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
    ), miss_count INTEGER NOT NULL DEFAULT 0
    CHECK (miss_count >= 0 AND miss_count <= 4294967295), metadata_source_kind TEXT NOT NULL DEFAULT 'legacy'
    CHECK (
        metadata_source_kind IN (
            'legacy',
            'provider_api',
            'official_documentation',
            'signed_catalog',
            'capability_probe',
            'user_override'
        )
    ), metadata_observed_at TEXT, last_reconciled_sync_job_id TEXT
    REFERENCES model_sync_jobs(id) ON DELETE SET NULL, metadata_sync_job_id TEXT
    REFERENCES model_sync_jobs(id) ON DELETE SET NULL,
    UNIQUE (connection_id, api_family, model_id, route_json)
);
INSERT INTO provider_models VALUES('fixture-openai-route','fixture-openai-loopback','openai_chat_completions','fixture-model','Synthetic fixture model','{"deployment_id":null,"region":null,"endpoint_path":null,"values":[]}','available',NULL,'2026-08-02T00:00:00+00:00','2026-08-02T00:00:00+00:00',0,'user_override',NULL,NULL,NULL);
CREATE TABLE model_capability_observations (
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
CREATE TABLE generation_presets (
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
INSERT INTO generation_presets VALUES('fixture-openai-preset','fixture-openai-route','Synthetic fixture preset','{"prompt_cache":{"context_reference":null,"mode":"provider_default","ttl":{"kind":"provider_default"}},"reasoning":{"budget_tokens":null,"effort":null,"mode":"provider_default","preserve_opaque_state":false,"summary":"provider_default"},"schema_version":1,"values":[]}','2026-08-02T00:00:00+00:00','2026-08-02T00:00:00+00:00');
CREATE TABLE provider_discovery_sessions (
    id TEXT NOT NULL PRIMARY KEY CHECK (
        length(id) BETWEEN 1 AND 128
        AND id = trim(id)
        AND instr(id, char(0)) = 0
    ),
    state TEXT NOT NULL CHECK (
        state IN (
            'draft',
            'resolving_known_provider',
            'awaiting_template_selection',
            'fetching_documents',
            'extracting_evidence',
            'awaiting_more_evidence',
            'awaiting_assistant_consent',
            'building_deterministic_manifest_draft',
            'building_assistant_manifest_draft',
            'validating_manifest',
            'awaiting_credential_origin_approval',
            'listing_models',
            'awaiting_probe_consent',
            'probing_capabilities',
            'awaiting_review',
            'committing',
            'compensating',
            'ready',
            'failed',
            'cancelled',
            'interrupted',
            'unknown_outcome'
        )
    ),
    revision INTEGER NOT NULL DEFAULT 0 CHECK (revision >= 0),
    next_event_sequence INTEGER NOT NULL DEFAULT 1 CHECK (
        next_event_sequence > 0
    ),
    -- Produced only from SanitizedDiscoveryInput. There is intentionally no
    -- raw request, pasted cURL, header, cookie, or credential-value column.
    sanitized_input_json TEXT NOT NULL CHECK (
        json_valid(sanitized_input_json)
        AND json_type(sanitized_input_json) = 'object'
        AND json_type(
            sanitized_input_json,
            '$.connection_id'
        ) = 'text'
        AND length(trim(json_extract(
            sanitized_input_json,
            '$.connection_id'
        ))) BETWEEN 1 AND 128
        AND json_type(
            sanitized_input_json,
            '$.display_name'
        ) = 'text'
        AND length(trim(json_extract(
            sanitized_input_json,
            '$.display_name'
        ))) BETWEEN 1 AND 120
        AND (
            json_type(sanitized_input_json, '$.credential_ref') IS NULL
            OR json_type(sanitized_input_json, '$.credential_ref') = 'null'
            OR (
                json_type(
                    sanitized_input_json,
                    '$.credential_ref'
                ) = 'text'
                AND json_extract(
                    sanitized_input_json,
                    '$.credential_ref'
                ) = json_extract(
                    sanitized_input_json,
                    '$.connection_id'
                )
            )
        )
        AND (
            json_type(sanitized_input_json, '$.site_url') IS NULL
            OR (
                json_type(sanitized_input_json, '$.site_url') = 'text'
                AND instr(
                    json_extract(sanitized_input_json, '$.site_url'),
                    '?'
                ) = 0
                AND instr(
                    json_extract(sanitized_input_json, '$.site_url'),
                    '#'
                ) = 0
            )
        )
        AND (
            json_type(sanitized_input_json, '$.docs_url') IS NULL
            OR json_type(sanitized_input_json, '$.docs_url') = 'null'
            OR (
                json_type(sanitized_input_json, '$.docs_url') = 'text'
                AND instr(
                    json_extract(sanitized_input_json, '$.docs_url'),
                    '?'
                ) = 0
                AND instr(
                    json_extract(sanitized_input_json, '$.docs_url'),
                    '#'
                ) = 0
            )
        )
    ),
    draft_json TEXT CHECK (
        draft_json IS NULL
        OR (
            json_valid(draft_json)
            AND json_type(draft_json) = 'object'
        )
    ),
    review_diff_json TEXT CHECK (
        review_diff_json IS NULL
        OR (
            json_valid(review_diff_json)
            AND json_type(review_diff_json) = 'object'
        )
    ),
    error_json TEXT CHECK (
        error_json IS NULL
        OR (
            json_valid(error_json)
            AND json_type(error_json) = 'object'
            AND json_type(error_json, '$.code') = 'text'
            AND json_type(error_json, '$.message_key') = 'text'
            AND json_type(error_json, '$.recoverable') IN ('true', 'false')
        )
    ),
    recovery_json TEXT CHECK (
        recovery_json IS NULL
        OR (
            json_valid(recovery_json)
            AND json_type(recovery_json) = 'object'
        )
    ),
    unknown_operation TEXT CHECK (
        unknown_operation IS NULL
        OR unknown_operation IN (
            'resolve_known_provider',
            'fetch_documents',
            'extract_evidence',
            'build_deterministic_manifest_draft',
            'build_assistant_manifest_draft',
            'validate_manifest',
            'list_models',
            'probe_capabilities',
            'atomic_commit',
            'compensation'
        )
    ),
    manifest_sha256 TEXT CHECK (
        manifest_sha256 IS NULL
        OR (
            length(manifest_sha256) = 64
            AND manifest_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    ),
    commit_plan_sha256 TEXT CHECK (
        commit_plan_sha256 IS NULL
        OR (
            length(commit_plan_sha256) = 64
            AND commit_plan_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    ),
    commit_attempt_id TEXT CHECK (
        commit_attempt_id IS NULL
        OR (
            length(commit_attempt_id) BETWEEN 1 AND 128
            AND commit_attempt_id = trim(commit_attempt_id)
        )
    ),
    committed_connection_id TEXT
        REFERENCES provider_connections(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    cancellation_pending INTEGER NOT NULL DEFAULT 0 CHECK (
        cancellation_pending IN (0, 1)
    ),
    -- Cross-table binding is validated by the transactional writer because
    -- operations also reference sessions, forming an intentional cycle.
    active_operation_id TEXT CHECK (
        active_operation_id IS NULL
        OR (
            length(active_operation_id) BETWEEN 1 AND 128
            AND active_operation_id = trim(active_operation_id)
        )
    ),
    active_effect_approval_json TEXT CHECK (
        active_effect_approval_json IS NULL
        OR (
            json_valid(active_effect_approval_json)
            AND json_type(active_effect_approval_json) = 'object'
            AND json_type(
                active_effect_approval_json,
                '$.approval_id'
            ) = 'text'
            AND json_type(
                active_effect_approval_json,
                '$.grant_sha256'
            ) = 'text'
            AND length(json_extract(
                active_effect_approval_json,
                '$.grant_sha256'
            )) = 64
            AND json_extract(
                active_effect_approval_json,
                '$.grant_sha256'
            ) NOT GLOB '*[^0-9a-f]*'
        )
    ),
    redaction_version INTEGER NOT NULL DEFAULT 1 CHECK (
        redaction_version > 0
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    updated_at TEXT NOT NULL CHECK (length(trim(updated_at)) > 0),
    CHECK (
        (state = 'interrupted' AND recovery_json IS NOT NULL)
        OR (state <> 'interrupted' AND recovery_json IS NULL)
    ),
    CHECK (
        (state = 'unknown_outcome' AND unknown_operation IS NOT NULL)
        OR (state <> 'unknown_outcome' AND unknown_operation IS NULL)
    ),
    CHECK (
        state NOT IN ('committing', 'compensating')
        OR (
            commit_plan_sha256 IS NOT NULL
            AND commit_attempt_id IS NOT NULL
        )
    )
);
CREATE TABLE provider_discovery_evidence (
    id TEXT NOT NULL PRIMARY KEY CHECK (length(trim(id)) > 0),
    session_id TEXT NOT NULL
        REFERENCES provider_discovery_sessions(id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (length(trim(kind)) > 0),
    source_url TEXT NOT NULL CHECK (
        length(trim(source_url)) > 0
        AND instr(source_url, '?') = 0
        AND instr(source_url, '#') = 0
    ),
    content_sha256 TEXT NOT NULL CHECK (
        length(content_sha256) = 64
        AND content_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    -- Structured, redacted extraction only. Full document bodies are not stored.
    extracted_json TEXT NOT NULL CHECK (
        json_valid(extracted_json)
        AND json_type(extracted_json) = 'object'
    ),
    redaction_version INTEGER NOT NULL DEFAULT 1 CHECK (
        redaction_version > 0
    ),
    fetched_at TEXT NOT NULL CHECK (length(trim(fetched_at)) > 0)
);
CREATE TABLE provider_discovery_candidates (
    id TEXT NOT NULL PRIMARY KEY CHECK (length(trim(id)) > 0),
    session_id TEXT NOT NULL
        REFERENCES provider_discovery_sessions(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    candidate_kind TEXT NOT NULL CHECK (
        candidate_kind IN (
            'provider_template',
            'api_origin',
            'official_document',
            'model_route',
            'manifest_draft'
        )
    ),
    summary_json TEXT NOT NULL CHECK (
        json_valid(summary_json)
        AND json_type(summary_json) = 'object'
    ),
    evidence_ids_json TEXT NOT NULL DEFAULT '[]' CHECK (
        json_valid(evidence_ids_json)
        AND json_type(evidence_ids_json) = 'array'
    ),
    proposed_revision INTEGER NOT NULL CHECK (proposed_revision >= 0),
    redaction_version INTEGER NOT NULL DEFAULT 1 CHECK (
        redaction_version > 0
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    UNIQUE (session_id, id)
);
CREATE TABLE provider_discovery_approvals (
    id TEXT NOT NULL PRIMARY KEY CHECK (length(trim(id)) > 0),
    session_id TEXT NOT NULL
        REFERENCES provider_discovery_sessions(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    approval_kind TEXT NOT NULL CHECK (
        approval_kind IN (
            'template_selection',
            'assistant_consent',
            'credential_origin',
            'capability_probe',
            'review',
            'unknown_outcome_resolution'
        )
    ),
    candidate_id TEXT,
    decision TEXT NOT NULL CHECK (decision IN ('approved', 'rejected')),
    grant_json TEXT NOT NULL CHECK (
        json_valid(grant_json)
        AND json_type(grant_json) = 'object'
    ),
    session_revision INTEGER NOT NULL CHECK (session_revision >= 0),
    grant_sha256 TEXT NOT NULL CHECK (
        length(grant_sha256) = 64
        AND grant_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    redaction_version INTEGER NOT NULL DEFAULT 1 CHECK (
        redaction_version > 0
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    FOREIGN KEY (session_id, candidate_id)
        REFERENCES provider_discovery_candidates(session_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);
CREATE TABLE provider_discovery_operations (
    id TEXT NOT NULL PRIMARY KEY CHECK (length(trim(id)) > 0),
    session_id TEXT NOT NULL
        REFERENCES provider_discovery_sessions(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    operation_kind TEXT NOT NULL CHECK (
        operation_kind IN (
            'resolve_known_provider',
            'fetch_documents',
            'extract_evidence',
            'build_deterministic_manifest_draft',
            'build_assistant_manifest_draft',
            'validate_manifest',
            'list_models',
            'probe_capabilities',
            'atomic_commit',
            'compensation'
        )
    ),
    side_effect_class TEXT NOT NULL CHECK (
        side_effect_class IN (
            'local_deterministic',
            'read_only',
            'billable_external',
            'persistent'
        )
    ),
    status TEXT NOT NULL CHECK (
        status IN (
            'prepared',
            'started',
            'succeeded',
            'failed',
            'interrupted',
            'outcome_unknown'
        )
    ),
    action_id TEXT NOT NULL,
    expected_revision INTEGER NOT NULL CHECK (expected_revision >= 0),
    request_sha256 TEXT NOT NULL CHECK (
        length(request_sha256) = 64
        AND request_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    approval_id TEXT,
    approval_grant_sha256 TEXT CHECK (
        approval_grant_sha256 IS NULL
        OR (
            length(approval_grant_sha256) = 64
            AND approval_grant_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    ),
    started_at TEXT,
    finished_at TEXT,
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    updated_at TEXT NOT NULL CHECK (length(trim(updated_at)) > 0),
    CHECK (
        (status = 'prepared' AND started_at IS NULL)
        OR (status <> 'prepared' AND started_at IS NOT NULL)
    ),
    CHECK (
        (status IN ('succeeded', 'failed', 'interrupted', 'outcome_unknown')
            AND finished_at IS NOT NULL)
        OR (status IN ('prepared', 'started') AND finished_at IS NULL)
    ),
    CHECK (
        (approval_id IS NULL AND approval_grant_sha256 IS NULL)
        OR (approval_id IS NOT NULL AND approval_grant_sha256 IS NOT NULL)
    ),
    FOREIGN KEY (approval_id)
        REFERENCES provider_discovery_approvals(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    UNIQUE (session_id, action_id)
);
CREATE TABLE provider_discovery_event_outbox (
    id TEXT NOT NULL PRIMARY KEY CHECK (length(trim(id)) > 0),
    session_id TEXT NOT NULL
        REFERENCES provider_discovery_sessions(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    sequence INTEGER NOT NULL CHECK (sequence > 0),
    event_version INTEGER NOT NULL CHECK (event_version > 0),
    session_revision INTEGER NOT NULL CHECK (session_revision > 0),
    state TEXT NOT NULL CHECK (
        state IN (
            'draft',
            'resolving_known_provider',
            'awaiting_template_selection',
            'fetching_documents',
            'extracting_evidence',
            'awaiting_more_evidence',
            'awaiting_assistant_consent',
            'building_deterministic_manifest_draft',
            'building_assistant_manifest_draft',
            'validating_manifest',
            'awaiting_credential_origin_approval',
            'listing_models',
            'awaiting_probe_consent',
            'probing_capabilities',
            'awaiting_review',
            'committing',
            'compensating',
            'ready',
            'failed',
            'cancelled',
            'interrupted',
            'unknown_outcome'
        )
    ),
    event_json TEXT NOT NULL CHECK (
        json_valid(event_json)
        AND json_type(event_json) = 'object'
    ),
    redaction_version INTEGER NOT NULL DEFAULT 1 CHECK (
        redaction_version > 0
    ),
    delivery_attempts INTEGER NOT NULL DEFAULT 0 CHECK (
        delivery_attempts >= 0
    ),
    available_at TEXT NOT NULL CHECK (length(trim(available_at)) > 0),
    delivered_at TEXT,
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    UNIQUE (session_id, sequence),
    UNIQUE (session_id, session_revision)
);
CREATE TABLE provider_discovery_action_receipts (
    action_id TEXT PRIMARY KEY CHECK (length(trim(action_id)) > 0),
    session_id TEXT NOT NULL
        REFERENCES provider_discovery_sessions(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    action_kind TEXT NOT NULL CHECK (length(trim(action_kind)) > 0),
    request_sha256 TEXT NOT NULL CHECK (
        length(request_sha256) = 64
        AND request_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    expected_revision INTEGER NOT NULL CHECK (expected_revision >= 0),
    resulting_revision INTEGER NOT NULL CHECK (
        resulting_revision = expected_revision + 1
    ),
    event_id TEXT NOT NULL
        REFERENCES provider_discovery_event_outbox(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    event_sequence INTEGER NOT NULL CHECK (event_sequence > 0),
    outcome TEXT NOT NULL CHECK (
        outcome IN ('applied', 'rejected', 'outcome_unknown')
    ),
    response_json TEXT NOT NULL CHECK (
        json_valid(response_json)
        AND json_type(response_json) = 'object'
    ),
    redaction_version INTEGER NOT NULL DEFAULT 1 CHECK (
        redaction_version > 0
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    UNIQUE (session_id, action_id),
    UNIQUE (session_id, resulting_revision),
    UNIQUE (session_id, event_sequence)
);
CREATE TABLE provider_discovery_commit_attempts (
    id TEXT NOT NULL PRIMARY KEY CHECK (length(trim(id)) > 0),
    session_id TEXT NOT NULL
        REFERENCES provider_discovery_sessions(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    attempt_number INTEGER NOT NULL CHECK (attempt_number > 0),
    action_id TEXT NOT NULL CHECK (length(trim(action_id)) > 0),
    expected_revision INTEGER NOT NULL CHECK (expected_revision >= 0),
    plan_sha256 TEXT NOT NULL CHECK (
        length(plan_sha256) = 64
        AND plan_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    plan_json TEXT NOT NULL CHECK (
        json_valid(plan_json)
        AND json_type(plan_json) = 'object'
    ),
    phase TEXT NOT NULL CHECK (
        phase IN (
            'prepared',
            'database_applied',
            'credential_reference_applied',
            'completed',
            'compensation_required',
            'compensating',
            'compensated',
            'outcome_unknown'
        )
    ),
    redaction_version INTEGER NOT NULL DEFAULT 1 CHECK (
        redaction_version > 0
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    updated_at TEXT NOT NULL CHECK (length(trim(updated_at)) > 0),
    completed_at TEXT,
    CHECK (
        (
            phase IN ('completed', 'compensated')
            AND completed_at IS NOT NULL
        )
        OR (
            phase NOT IN ('completed', 'compensated')
            AND completed_at IS NULL
        )
    ),
    UNIQUE (session_id, attempt_number),
    UNIQUE (session_id, action_id),
    UNIQUE (session_id, plan_sha256)
);
CREATE TABLE provider_discovery_compensation_steps (
    id TEXT NOT NULL PRIMARY KEY CHECK (length(trim(id)) > 0),
    commit_attempt_id TEXT NOT NULL
        REFERENCES provider_discovery_commit_attempts(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    action_id TEXT NOT NULL CHECK (length(trim(action_id)) > 0),
    step_kind TEXT NOT NULL CHECK (
        step_kind IN (
            'remove_credential_slot',
            'remove_connection_graph',
            'restore_previous_selection'
        )
    ),
    step_json TEXT NOT NULL CHECK (
        json_valid(step_json)
        AND json_type(step_json) = 'object'
    ),
    status TEXT NOT NULL CHECK (
        status IN (
            'pending',
            'in_progress',
            'completed',
            'failed',
            'outcome_unknown'
        )
    ),
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    last_failure_json TEXT CHECK (
        last_failure_json IS NULL
        OR (
            json_valid(last_failure_json)
            AND json_type(last_failure_json) = 'object'
        )
    ),
    redaction_version INTEGER NOT NULL DEFAULT 1 CHECK (
        redaction_version > 0
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    updated_at TEXT NOT NULL CHECK (length(trim(updated_at)) > 0),
    completed_at TEXT,
    CHECK (
        (status = 'completed' AND completed_at IS NOT NULL)
        OR (status <> 'completed' AND completed_at IS NULL)
    ),
    CHECK (
        (status = 'failed' AND last_failure_json IS NOT NULL)
        OR (status <> 'failed' AND last_failure_json IS NULL)
    ),
    UNIQUE (commit_attempt_id, ordinal),
    UNIQUE (commit_attempt_id, action_id)
);
CREATE TABLE provider_discovery_audit_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL
        REFERENCES provider_discovery_sessions(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    audit_sequence INTEGER NOT NULL CHECK (audit_sequence > 0),
    session_revision INTEGER NOT NULL CHECK (session_revision >= 0),
    audit_kind TEXT NOT NULL CHECK (
        audit_kind IN (
            'session_created',
            'transition_applied',
            'candidate_recorded',
            'approval_recorded',
            'operation_started',
            'operation_interrupted',
            'commit_prepared',
            'compensation_started',
            'unknown_outcome_reconciled'
        )
    ),
    action_id TEXT,
    subject_id TEXT,
    summary_key TEXT NOT NULL CHECK (
        length(summary_key) BETWEEN 1 AND 128
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    UNIQUE (session_id, audit_sequence)
);
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
INSERT INTO provider_catalog_state VALUES(1,0,NULL,NULL,0,NULL,'1970-01-01T00:00:00Z');
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
CREATE TABLE model_sync_jobs (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(id) > 0),
    connection_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK (
        state IN (
            'created',
            'fetching',
            'interrupted',
            'diff-ready-awaiting-review',
            'committing',
            'completed',
            'failed',
            'cancelled'
        )
    ),
    revision INTEGER NOT NULL CHECK (revision >= 1),
    next_event_sequence INTEGER NOT NULL CHECK (next_event_sequence >= 2),
    expected_connection_json TEXT NOT NULL
        CHECK (
            json_valid(expected_connection_json)
            AND json_type(expected_connection_json) = 'object'
            AND length(CAST(expected_connection_json AS BLOB)) <= 65536
        ),
    expected_connection_sha256 TEXT NOT NULL
        CHECK (
            length(expected_connection_sha256) = 64
            AND expected_connection_sha256 NOT GLOB '*[^0-9a-f]*'
        ),
    base_graph_sha256 TEXT NOT NULL
        CHECK (
            length(base_graph_sha256) = 64
            AND base_graph_sha256 NOT GLOB '*[^0-9a-f]*'
        ),
    review_json TEXT
        CHECK (
            review_json IS NULL
            OR (
                json_valid(review_json)
                AND json_type(review_json) = 'object'
                AND length(CAST(review_json AS BLOB)) <= 8388608
            )
        ),
    review_sha256 TEXT
        CHECK (
            review_sha256 IS NULL
            OR (
                length(review_sha256) = 64
                AND review_sha256 NOT GLOB '*[^0-9a-f]*'
            )
        ),
    approved_review_sha256 TEXT
        CHECK (
            approved_review_sha256 IS NULL
            OR (
                length(approved_review_sha256) = 64
                AND approved_review_sha256 NOT GLOB '*[^0-9a-f]*'
            )
        ),
    approved_at TEXT,
    failure_json TEXT
        CHECK (
            failure_json IS NULL
            OR (
                json_valid(failure_json)
                AND json_type(failure_json) = 'object'
                AND json_type(failure_json, '$.code') = 'text'
                AND json_type(failure_json, '$.message_key') = 'text'
                AND json_extract(failure_json, '$.message_key') = 'model_sync.failed'
                AND json_type(failure_json, '$.recoverable') IN ('true', 'false')
                AND length(CAST(failure_json AS BLOB)) <= 1024
            )
        ),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY (connection_id)
        REFERENCES provider_connections(id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE,
    CHECK (
        (review_json IS NULL AND review_sha256 IS NULL)
        OR (review_json IS NOT NULL AND review_sha256 IS NOT NULL)
    ),
    CHECK (
        (approved_review_sha256 IS NULL AND approved_at IS NULL)
        OR (approved_review_sha256 IS NOT NULL AND approved_at IS NOT NULL)
    ),
    CHECK (
        state NOT IN (
            'diff-ready-awaiting-review',
            'committing',
            'completed'
        )
        OR review_json IS NOT NULL
    ),
    CHECK (
        state <> 'committing'
        OR approved_review_sha256 = review_sha256
    ),
    CHECK (
        state <> 'completed'
        OR (
            approved_review_sha256 = review_sha256
            AND failure_json IS NULL
        )
    ),
    CHECK (
        state <> 'failed' OR failure_json IS NOT NULL
    ),
    CHECK (
        state = 'failed' OR failure_json IS NULL
    )
);
CREATE TABLE model_sync_event_outbox (
    job_id TEXT NOT NULL,
    sequence INTEGER NOT NULL CHECK (sequence >= 1),
    event_version INTEGER NOT NULL CHECK (event_version >= 1),
    job_revision INTEGER NOT NULL CHECK (job_revision >= 1),
    state TEXT NOT NULL CHECK (
        state IN (
            'created',
            'fetching',
            'interrupted',
            'diff-ready-awaiting-review',
            'committing',
            'completed',
            'failed',
            'cancelled'
        )
    ),
    redaction_version INTEGER NOT NULL CHECK (redaction_version >= 1),
    event_json TEXT NOT NULL
        CHECK (
            json_valid(event_json)
            AND json_type(event_json) = 'object'
            AND length(CAST(event_json AS BLOB)) <= 16384
        ),
    created_at TEXT NOT NULL,
    available_at TEXT NOT NULL,
    delivery_attempts INTEGER NOT NULL DEFAULT 0
        CHECK (delivery_attempts >= 0),
    delivered_at TEXT,
    PRIMARY KEY (job_id, sequence),
    UNIQUE (job_id, job_revision),
    FOREIGN KEY (job_id)
        REFERENCES model_sync_jobs(id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE
);
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
INSERT INTO sqlite_sequence VALUES('provider_discovery_audit_log',0);
CREATE TRIGGER provider_discovery_session_revision_guard
BEFORE UPDATE OF
    state,
    revision,
    next_event_sequence,
    sanitized_input_json,
    draft_json,
    review_diff_json,
    error_json,
    recovery_json,
    unknown_operation,
    manifest_sha256,
    commit_plan_sha256,
    commit_attempt_id,
    committed_connection_id,
    cancellation_pending,
    active_operation_id,
    active_effect_approval_json
ON provider_discovery_sessions
FOR EACH ROW
WHEN
    NEW.revision <> OLD.revision + 1
    OR NEW.next_event_sequence <> OLD.next_event_sequence + 1
BEGIN
    SELECT RAISE(
        ABORT,
        'discovery transition must increment revision and event sequence exactly once'
    );
END;
CREATE TRIGGER provider_discovery_receipt_no_update
BEFORE UPDATE ON provider_discovery_action_receipts
BEGIN
    SELECT RAISE(ABORT, 'discovery action receipts are immutable');
END;
CREATE TRIGGER provider_discovery_receipt_no_delete
BEFORE DELETE ON provider_discovery_action_receipts
BEGIN
    SELECT RAISE(ABORT, 'discovery action receipts are immutable');
END;
CREATE TRIGGER provider_discovery_evidence_no_update
BEFORE UPDATE ON provider_discovery_evidence
BEGIN
    SELECT RAISE(ABORT, 'discovery evidence is immutable');
END;
CREATE TRIGGER provider_discovery_evidence_no_delete
BEFORE DELETE ON provider_discovery_evidence
BEGIN
    SELECT RAISE(ABORT, 'discovery evidence is immutable');
END;
CREATE TRIGGER provider_discovery_operation_identity_no_update
BEFORE UPDATE OF
    id,
    session_id,
    operation_kind,
    side_effect_class,
    action_id,
    expected_revision,
    request_sha256,
    approval_id,
    approval_grant_sha256,
    redaction_version,
    created_at
ON provider_discovery_operations
BEGIN
    SELECT RAISE(ABORT, 'discovery operation identity is immutable');
END;
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
        AND OLD.side_effect_class IN ('billable_external', 'persistent')
        AND NEW.status = 'outcome_unknown'
        AND NEW.started_at = OLD.started_at
        AND NEW.finished_at IS NOT NULL
    )
)
BEGIN
    SELECT RAISE(ABORT, 'illegal discovery operation status transition');
END;
CREATE TRIGGER provider_discovery_operation_no_delete
BEFORE DELETE ON provider_discovery_operations
BEGIN
    SELECT RAISE(ABORT, 'discovery operations are immutable');
END;
CREATE TRIGGER provider_discovery_commit_identity_no_update
BEFORE UPDATE OF
    id,
    session_id,
    attempt_number,
    action_id,
    expected_revision,
    plan_sha256,
    plan_json,
    redaction_version,
    created_at
ON provider_discovery_commit_attempts
BEGIN
    SELECT RAISE(ABORT, 'discovery commit identity is immutable');
END;
CREATE TRIGGER provider_discovery_commit_legal_transition
BEFORE UPDATE OF phase, updated_at, completed_at
ON provider_discovery_commit_attempts
WHEN NOT (
    (
        OLD.phase = 'prepared'
        AND NEW.phase IN (
            'database_applied',
            'compensation_required',
            'compensated',
            'outcome_unknown'
        )
    )
    OR (
        OLD.phase = 'database_applied'
        AND NEW.phase IN (
            'credential_reference_applied',
            'completed',
            'compensation_required',
            'outcome_unknown'
        )
    )
    OR (
        OLD.phase = 'credential_reference_applied'
        AND NEW.phase IN (
            'completed',
            'compensation_required',
            'outcome_unknown'
        )
    )
    OR (
        OLD.phase = 'compensation_required'
        AND NEW.phase = 'compensating'
    )
    OR (
        OLD.phase = 'compensating'
        AND NEW.phase IN (
            'compensating',
            'compensated',
            'outcome_unknown'
        )
    )
    OR (
        OLD.phase = 'outcome_unknown'
        AND NEW.phase IN (
            'prepared',
            'database_applied',
            'credential_reference_applied',
            'compensation_required',
            'compensating',
            'compensated'
        )
    )
)
BEGIN
    SELECT RAISE(ABORT, 'illegal discovery commit phase transition');
END;
CREATE TRIGGER provider_discovery_commit_no_delete
BEFORE DELETE ON provider_discovery_commit_attempts
BEGIN
    SELECT RAISE(ABORT, 'discovery commit attempts are immutable');
END;
CREATE TRIGGER provider_discovery_compensation_identity_no_update
BEFORE UPDATE OF
    id,
    commit_attempt_id,
    ordinal,
    action_id,
    step_kind,
    step_json,
    redaction_version,
    created_at
ON provider_discovery_compensation_steps
BEGIN
    SELECT RAISE(ABORT, 'discovery compensation identity is immutable');
END;
CREATE TRIGGER provider_discovery_compensation_legal_transition
BEFORE UPDATE OF
    status,
    attempt_count,
    last_failure_json,
    updated_at,
    completed_at
ON provider_discovery_compensation_steps
WHEN NOT (
    (
        OLD.status = 'pending'
        AND NEW.status = 'in_progress'
        AND NEW.attempt_count = OLD.attempt_count + 1
    )
    OR (
        OLD.status = 'in_progress'
        AND NEW.status IN ('completed', 'failed', 'outcome_unknown')
        AND NEW.attempt_count = OLD.attempt_count
    )
    OR (
        OLD.status = 'failed'
        AND NEW.status = 'pending'
        AND NEW.attempt_count = OLD.attempt_count
    )
    OR (
        OLD.status = 'outcome_unknown'
        AND NEW.status = 'pending'
        AND NEW.attempt_count = OLD.attempt_count
    )
    OR (
        OLD.status IN ('pending', 'failed', 'outcome_unknown')
        AND NEW.status = 'completed'
        AND NEW.attempt_count = OLD.attempt_count
    )
)
BEGIN
    SELECT RAISE(ABORT, 'illegal discovery compensation status transition');
END;
CREATE TRIGGER provider_discovery_compensation_no_delete
BEFORE DELETE ON provider_discovery_compensation_steps
BEGIN
    SELECT RAISE(ABORT, 'discovery compensation steps are immutable');
END;
CREATE TRIGGER provider_discovery_event_identity_no_update
BEFORE UPDATE OF
    id,
    session_id,
    session_revision,
    sequence,
    event_version,
    event_json,
    redaction_version,
    created_at
ON provider_discovery_event_outbox
BEGIN
    SELECT RAISE(ABORT, 'discovery event identity is immutable');
END;
CREATE TRIGGER provider_discovery_event_no_delete
BEFORE DELETE ON provider_discovery_event_outbox
BEGIN
    SELECT RAISE(ABORT, 'discovery events are immutable');
END;
CREATE TRIGGER provider_discovery_candidate_no_update
BEFORE UPDATE ON provider_discovery_candidates
BEGIN
    SELECT RAISE(ABORT, 'discovery candidates are immutable');
END;
CREATE TRIGGER provider_discovery_candidate_no_delete
BEFORE DELETE ON provider_discovery_candidates
BEGIN
    SELECT RAISE(ABORT, 'discovery candidates are immutable');
END;
CREATE TRIGGER provider_discovery_approval_no_update
BEFORE UPDATE ON provider_discovery_approvals
BEGIN
    SELECT RAISE(ABORT, 'discovery approvals are immutable');
END;
CREATE TRIGGER provider_discovery_approval_no_delete
BEFORE DELETE ON provider_discovery_approvals
BEGIN
    SELECT RAISE(ABORT, 'discovery approvals are immutable');
END;
CREATE TRIGGER provider_discovery_audit_no_update
BEFORE UPDATE ON provider_discovery_audit_log
BEGIN
    SELECT RAISE(ABORT, 'discovery audit entries are immutable');
END;
CREATE TRIGGER provider_discovery_audit_no_delete
BEFORE DELETE ON provider_discovery_audit_log
BEGIN
    SELECT RAISE(ABORT, 'discovery audit entries are immutable');
END;
CREATE TRIGGER generations_provider_target_insert_guard
BEFORE INSERT ON generations
WHEN
    (NEW.model_route_id IS NULL) <> (NEW.generation_preset_id IS NULL)
    OR (
        NEW.model_route_id IS NOT NULL
        AND NOT EXISTS (
            SELECT 1
            FROM generation_presets AS preset
            WHERE preset.id = NEW.generation_preset_id
              AND preset.model_route_id = NEW.model_route_id
        )
    )
BEGIN
    SELECT RAISE(ABORT, 'generation provider target is inconsistent');
END;
CREATE TRIGGER generations_provider_target_update_guard
BEFORE UPDATE OF model_route_id, generation_preset_id ON generations
WHEN
    (NEW.model_route_id IS NULL) <> (NEW.generation_preset_id IS NULL)
    OR (
        NEW.model_route_id IS NOT NULL
        AND NOT EXISTS (
            SELECT 1
            FROM generation_presets AS preset
            WHERE preset.id = NEW.generation_preset_id
              AND preset.model_route_id = NEW.model_route_id
        )
    )
BEGIN
    SELECT RAISE(ABORT, 'generation provider target is inconsistent');
END;
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
CREATE INDEX messages_conversation_created
    ON messages(conversation_id, created_at, id);
CREATE INDEX messages_conversation_parent
    ON messages(conversation_id, parent_id, created_at, id);
CREATE UNIQUE INDEX messages_generation_unique
    ON messages(generation_id)
    WHERE generation_id IS NOT NULL;
CREATE INDEX conversation_branches_conversation_updated
    ON conversation_branches(conversation_id, updated_at DESC, id);
CREATE INDEX generations_conversation_branch_started
    ON generations(conversation_id, branch_id, started_at, id);
CREATE INDEX provider_templates_source_created
    ON provider_templates(source_kind, created_at, id, version);
CREATE INDEX provider_connections_template
    ON provider_connections(template_id, template_version, id);
CREATE INDEX provider_connections_status_updated
    ON provider_connections(status, updated_at, id);
CREATE INDEX provider_models_connection_availability
    ON provider_models(connection_id, availability, model_id, id);
CREATE INDEX provider_models_last_seen
    ON provider_models(connection_id, last_seen_at, id);
CREATE INDEX capability_observations_route_key_observed
    ON model_capability_observations(
        model_route_id,
        capability_key,
        observed_at DESC,
        id
    );
CREATE INDEX capability_observations_expiry
    ON model_capability_observations(expires_at, id)
    WHERE expires_at IS NOT NULL;
CREATE INDEX generation_presets_model_route
    ON generation_presets(model_route_id, updated_at DESC, id);
CREATE INDEX provider_discovery_sessions_state_updated
    ON provider_discovery_sessions(state, updated_at, id);
CREATE INDEX provider_discovery_sessions_action_required
    ON provider_discovery_sessions(state, updated_at, id)
    WHERE state IN (
        'awaiting_template_selection',
        'awaiting_more_evidence',
        'awaiting_assistant_consent',
        'awaiting_credential_origin_approval',
        'awaiting_probe_consent',
        'awaiting_review',
        'interrupted',
        'unknown_outcome'
    );
CREATE INDEX provider_discovery_evidence_session_fetched
    ON provider_discovery_evidence(session_id, fetched_at, id);
CREATE INDEX provider_discovery_evidence_source_hash
    ON provider_discovery_evidence(source_url, content_sha256, id);
CREATE INDEX provider_discovery_candidates_session_kind
    ON provider_discovery_candidates(
        session_id,
        candidate_kind,
        proposed_revision,
        id
    );
CREATE INDEX provider_discovery_approvals_session_kind
    ON provider_discovery_approvals(
        session_id,
        approval_kind,
        session_revision,
        id
    );
CREATE INDEX provider_discovery_operations_recovery
    ON provider_discovery_operations(status, side_effect_class, updated_at, id)
    WHERE status IN ('prepared', 'started');
CREATE INDEX provider_discovery_outbox_pending
    ON provider_discovery_event_outbox(
        delivered_at,
        available_at,
        session_id,
        sequence
    )
    WHERE delivered_at IS NULL;
CREATE INDEX provider_discovery_receipts_session_created
    ON provider_discovery_action_receipts(session_id, created_at, action_id);
CREATE INDEX provider_discovery_commit_attempts_recovery
    ON provider_discovery_commit_attempts(phase, updated_at, session_id, id)
    WHERE phase NOT IN ('completed', 'compensated');
CREATE INDEX provider_discovery_compensation_pending
    ON provider_discovery_compensation_steps(
        status,
        commit_attempt_id,
        ordinal DESC
    )
    WHERE status IN ('pending', 'in_progress', 'failed', 'outcome_unknown');
CREATE INDEX provider_discovery_audit_session_revision
    ON provider_discovery_audit_log(
        session_id,
        session_revision,
        audit_sequence
    );
CREATE INDEX generations_model_route_started
    ON generations(model_route_id, started_at, id)
    WHERE model_route_id IS NOT NULL;
CREATE INDEX generations_preset_started
    ON generations(generation_preset_id, started_at, id)
    WHERE generation_preset_id IS NOT NULL;
CREATE INDEX provider_catalog_signed_envelopes_imported
    ON provider_catalog_signed_envelopes(accepted_at DESC, catalog_revision DESC);
CREATE INDEX provider_catalog_snapshots_source
    ON provider_catalog_snapshots(source_kind, catalog_revision, local_revision);
CREATE INDEX provider_catalog_snapshot_envelopes_revision
    ON provider_catalog_snapshot_envelopes(
        catalog_revision,
        local_revision
    );
CREATE INDEX provider_catalog_activation_audit_target
    ON provider_catalog_activation_audit(
        to_local_revision,
        state_version DESC
    );
CREATE UNIQUE INDEX model_sync_one_active_job_per_connection
    ON model_sync_jobs(connection_id)
    WHERE state IN (
        'created',
        'fetching',
        'diff-ready-awaiting-review',
        'committing'
    );
CREATE INDEX model_sync_jobs_connection_history
    ON model_sync_jobs(connection_id, created_at DESC, id);
CREATE INDEX model_sync_outbox_undelivered
    ON model_sync_event_outbox(available_at, job_id, sequence)
    WHERE delivered_at IS NULL;
CREATE INDEX provider_connections_active_display_name
    ON provider_connections(display_name COLLATE NOCASE, id)
    WHERE archived_at IS NULL;
CREATE INDEX provider_connection_local_network_approvals_origin
    ON provider_connection_local_network_approvals(origin, connection_id);
COMMIT;
