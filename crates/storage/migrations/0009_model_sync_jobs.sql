-- Durable, review-gated provider model synchronization.
--
-- Provider credentials are intentionally absent. expected_connection_json
-- contains only the public ProviderConnection snapshot and opaque vault
-- reference already present in provider_connections.

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

-- Only one live network/review/commit lineage may exist per connection. An
-- interrupted job is immutable history; an explicit start creates a new job.
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

CREATE INDEX model_sync_outbox_undelivered
    ON model_sync_event_outbox(available_at, job_id, sequence)
    WHERE delivered_at IS NULL;

ALTER TABLE provider_models
    ADD COLUMN miss_count INTEGER NOT NULL DEFAULT 0
    CHECK (miss_count >= 0 AND miss_count <= 4294967295);

ALTER TABLE provider_models
    ADD COLUMN metadata_source_kind TEXT NOT NULL DEFAULT 'legacy'
    CHECK (
        metadata_source_kind IN (
            'legacy',
            'provider_api',
            'official_documentation',
            'signed_catalog',
            'capability_probe',
            'user_override'
        )
    );

ALTER TABLE provider_models
    ADD COLUMN metadata_observed_at TEXT;

ALTER TABLE provider_models
    ADD COLUMN last_reconciled_sync_job_id TEXT
    REFERENCES model_sync_jobs(id) ON DELETE SET NULL;

ALTER TABLE provider_models
    ADD COLUMN metadata_sync_job_id TEXT
    REFERENCES model_sync_jobs(id) ON DELETE SET NULL;

-- Preserve the fact that a pre-v9 route had already been omitted at least
-- once, while allowing subsequent successful omissions to count normally.
UPDATE provider_models
SET miss_count = 1
WHERE availability = 'missing_temporarily';
