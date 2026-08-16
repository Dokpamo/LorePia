PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS content_sources (
    sha256 TEXT PRIMARY KEY,
    relative_path TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS characters (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    source_hash TEXT NOT NULL REFERENCES content_sources(sha256),
    avatar_asset_hash TEXT,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS assets (
    sha256 TEXT PRIMARY KEY,
    relative_path TEXT NOT NULL,
    media_type TEXT,
    size_bytes INTEGER NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS character_assets (
    character_id TEXT NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
    asset_hash TEXT NOT NULL REFERENCES assets(sha256),
    role TEXT NOT NULL,
    PRIMARY KEY (character_id, asset_hash, role)
);

CREATE TABLE IF NOT EXISTS conversations (
    id TEXT PRIMARY KEY,
    character_id TEXT NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS messages (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    parent_id TEXT REFERENCES messages(id),
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    status TEXT NOT NULL,
    generation_id TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS messages_conversation_created
    ON messages(conversation_id, created_at, id);

CREATE TABLE IF NOT EXISTS provider_profiles (
    id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    base_url TEXT NOT NULL,
    model TEXT NOT NULL,
    timeout_seconds INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS app_settings (
    key TEXT PRIMARY KEY,
    value_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS import_jobs (
    id TEXT PRIMARY KEY,
    source_hash TEXT NOT NULL,
    staging_path TEXT NOT NULL,
    state TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
