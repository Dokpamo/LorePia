PRAGMA foreign_keys = ON;
-- Legacy discovery JSON had no redaction contract. Ensure dropped legacy
-- payload pages are overwritten on this connection during the rebuild.
PRAGMA secure_delete = ON;

-- Rebuild the PR4 discovery tables so the durable state vocabulary and
-- revision metadata are enforced by SQLite. Both tables are rebuilt together
-- because provider_discovery_evidence references provider_discovery_sessions.
ALTER TABLE provider_discovery_evidence
    RENAME TO provider_discovery_evidence_v4;
ALTER TABLE provider_discovery_sessions
    RENAME TO provider_discovery_sessions_v4;

DROP INDEX IF EXISTS provider_discovery_evidence_session_fetched;
DROP INDEX IF EXISTS provider_discovery_evidence_source_hash;
DROP INDEX IF EXISTS provider_discovery_sessions_state_updated;

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

-- Version 4 persisted neither the typed recovery context nor the immutable
-- ledgers required to prove a safe restart or reconciliation. Archive legacy
-- work fail-closed; no operation is retried by this migration.
INSERT INTO provider_discovery_sessions (
    id,
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
    active_effect_approval_json,
    redaction_version,
    created_at,
    updated_at
)
WITH sanitized_v4 AS (
    SELECT
        provider_discovery_sessions_v4.*,
        provider_discovery_sessions_v4.rowid AS legacy_rowid,
        json_extract(
            sanitized_input_json,
            '$.site_url'
        ) AS legacy_site_url,
        json_extract(
            sanitized_input_json,
            '$.docs_url'
        ) AS legacy_docs_url
    FROM provider_discovery_sessions_v4
),
stripped_v4 AS (
    SELECT
        sanitized_v4.*,
        CASE
            WHEN legacy_site_url IS NULL THEN NULL
            WHEN instr(legacy_site_url, '?') = 0 THEN
                CASE
                    WHEN instr(legacy_site_url, '#') = 0
                        THEN legacy_site_url
                    ELSE substr(
                        legacy_site_url,
                        1,
                        instr(legacy_site_url, '#') - 1
                    )
                END
            WHEN instr(legacy_site_url, '#') = 0
                OR instr(legacy_site_url, '?') < instr(legacy_site_url, '#')
                THEN substr(
                    legacy_site_url,
                    1,
                    instr(legacy_site_url, '?') - 1
                )
            ELSE substr(
                legacy_site_url,
                1,
                instr(legacy_site_url, '#') - 1
            )
        END AS sanitized_site_url,
        CASE
            WHEN legacy_docs_url IS NULL THEN NULL
            WHEN instr(legacy_docs_url, '?') = 0 THEN
                CASE
                    WHEN instr(legacy_docs_url, '#') = 0
                        THEN legacy_docs_url
                    ELSE substr(
                        legacy_docs_url,
                        1,
                        instr(legacy_docs_url, '#') - 1
                    )
                END
            WHEN instr(legacy_docs_url, '#') = 0
                OR instr(legacy_docs_url, '?') < instr(legacy_docs_url, '#')
                THEN substr(
                    legacy_docs_url,
                    1,
                    instr(legacy_docs_url, '?') - 1
                )
            ELSE substr(
                legacy_docs_url,
                1,
                instr(legacy_docs_url, '#') - 1
            )
        END AS sanitized_docs_url
    FROM sanitized_v4
)
SELECT
    -- Version 4 did not classify session identifiers as non-secret metadata.
    -- Rekey every row rather than carrying an apparently well-formed identifier
    -- into the redacted schema: a valid-length legacy ID may itself be an API
    -- key or other credential. The legacy rowid is local, deterministic, and
    -- unique within this one-time table rebuild.
    printf('legacy-v5-session-%016x', legacy_rowid),
    -- Version 4 has neither a typed/redacted working draft nor approval,
    -- operation, commit-plan, and compensation ledgers. Guessing any recovery
    -- operation would risk replaying an external effect. Preserve only an
    -- already-cancelled terminal state; archive every other legacy session as
    -- an explicit non-recoverable failure.
    CASE
        WHEN state = 'cancelled' THEN 'cancelled'
        ELSE 'failed'
    END,
    0,
    1,
    -- Version 4 accepted arbitrary URLs and JSON. Even a path, userinfo, or
    -- opaque identifier can contain credential material, so do not relabel
    -- any legacy input as redacted. Keep a non-routable placeholder only so
    -- unknown-outcome sessions remain loadable for explicit reconciliation.
    json_object(
        'connection_id',
            'legacy-redacted-provider',
        'display_name', 'Redacted legacy provider',
        'site_url', 'https://redacted.invalid/',
        'docs_url', 'https://redacted.invalid/',
        'credential_ref', NULL,
        'preferred_assistant', NULL,
        'local_network_mode', json('false'),
        'supplied_evidence_ids', json('[]')
    ),
    -- Legacy drafts had no redaction contract. Discard rather than relabel.
    NULL,
    NULL,
    CASE
        WHEN state = 'cancelled' AND error_json IS NULL THEN NULL
        -- Preserve only the fact that a legacy state cannot be recovered,
        -- never arbitrary version-4 error strings.
        ELSE json_object(
            'code', 'legacy.discovery_failure',
            'message_key', 'discovery.error.legacy_failure',
            'recoverable', json('false')
        )
    END,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    0,
    NULL,
    NULL,
    1,
    '1970-01-01T00:00:00Z',
    '1970-01-01T00:00:00Z'
FROM stripped_v4;

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

-- Every migrated v4 session is archived and cannot resume. Version 4 placed no
-- bounds or closed vocabulary on evidence IDs, kinds, or timestamps, so even
-- those metadata fields may contain credential material and may not hydrate as
-- v5 records. Purge the complete legacy evidence set instead of relabelling any
-- untrusted field as safe.

CREATE INDEX provider_discovery_evidence_session_fetched
    ON provider_discovery_evidence(session_id, fetched_at, id);
CREATE INDEX provider_discovery_evidence_source_hash
    ON provider_discovery_evidence(source_url, content_sha256, id);

DROP TABLE provider_discovery_evidence_v4;
DROP TABLE provider_discovery_sessions_v4;

-- Candidate summaries are typed, redacted DTOs. A candidate is immutable;
-- selection and rejection are append-only approval records.
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

CREATE INDEX provider_discovery_candidates_session_kind
    ON provider_discovery_candidates(
        session_id,
        candidate_kind,
        proposed_revision,
        id
    );

-- User decisions are immutable evidence of consent. Grant JSON may contain
-- origins, hashes, budgets, and opaque logical references, never secret values.
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

CREATE INDEX provider_discovery_approvals_session_kind
    ON provider_discovery_approvals(
        session_id,
        approval_kind,
        session_revision,
        id
    );

-- Operations are first prepared, then marked started immediately before work.
-- On startup, local_deterministic/read_only + started becomes interrupted;
-- billable or persistent + started becomes unknown_outcome. Recovery never
-- replays a row.
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

CREATE INDEX provider_discovery_operations_recovery
    ON provider_discovery_operations(status, side_effect_class, updated_at, id)
    WHERE status IN ('prepared', 'started');

-- The outbox row is inserted in the same transaction as the CAS state update
-- and action receipt. Delivery is at-least-once; consumers deduplicate by
-- (session_id, sequence) or event id.
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

CREATE INDEX provider_discovery_outbox_pending
    ON provider_discovery_event_outbox(
        delivered_at,
        available_at,
        session_id,
        sequence
    )
    WHERE delivered_at IS NULL;

-- Publish the upgrade-time interruption classification. Version 4 had no
-- outbox, so this is the first recoverable event for those active sessions.
INSERT INTO provider_discovery_event_outbox (
    id,
    session_id,
    sequence,
    event_version,
    session_revision,
    state,
    event_json,
    redaction_version,
    delivery_attempts,
    available_at,
    delivered_at,
    created_at
)
SELECT
    printf(
        'migration-v5-event-%016x-%016x',
        length(id),
        rowid
    ),
    id,
    1,
    1,
    1,
    state,
    json_object(
        'version', 1,
        'id', printf(
            'migration-v5-event-%016x-%016x',
            length(id),
            rowid
        ),
        'session_id', id,
        'sequence', 1,
        'session_revision', 1,
        'state', state,
        'progress', NULL,
        'action_required', CASE state
            WHEN 'interrupted' THEN json_object(
                'kind', 'restart_interrupted',
                'operation', json_extract(recovery_json, '$.operation')
            )
            ELSE json_object(
                'kind', 'reconcile_unknown_outcome',
                'operation', unknown_operation
            )
        END,
        'warning', CASE state
            WHEN 'interrupted' THEN 'explicit_restart_required'
            ELSE 'unknown_external_outcome'
        END,
        'failure', CASE
            WHEN error_json IS NULL THEN NULL
            ELSE json(error_json)
        END,
        'action_id', printf(
            'migration-v5-action-%016x-%016x',
            length(id),
            rowid
        )
    ),
    1,
    0,
    updated_at,
    NULL,
    updated_at
FROM provider_discovery_sessions
WHERE revision = 1
  AND state IN ('interrupted', 'unknown_outcome');

-- An applied action receipt is immutable. Replaying the same action_id with the
-- same hash returns the recorded result; a different hash is rejected.
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

CREATE INDEX provider_discovery_receipts_session_created
    ON provider_discovery_action_receipts(session_id, created_at, action_id);

-- Commit attempts bridge SQLite and the native credential store. The plan
-- contains only graph IDs, hashes, an opaque credential slot, and approval IDs.
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

CREATE INDEX provider_discovery_commit_attempts_recovery
    ON provider_discovery_commit_attempts(phase, updated_at, session_id, id)
    WHERE phase NOT IN ('completed', 'compensated');

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

CREATE INDEX provider_discovery_compensation_pending
    ON provider_discovery_compensation_steps(
        status,
        commit_attempt_id,
        ordinal DESC
    )
    WHERE status IN ('pending', 'in_progress', 'failed', 'outcome_unknown');

-- Audit entries are append-only and deliberately contain no arbitrary provider
-- response or request fields.
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

CREATE INDEX provider_discovery_audit_session_revision
    ON provider_discovery_audit_log(
        session_id,
        session_revision,
        audit_sequence
    );

INSERT INTO provider_discovery_audit_log (
    session_id,
    audit_sequence,
    session_revision,
    audit_kind,
    action_id,
    subject_id,
    summary_key,
    created_at
)
SELECT
    id,
    1,
    0,
    'session_created',
    NULL,
    id,
    'discovery.audit.session_created_before_v5',
    created_at
FROM provider_discovery_sessions;

INSERT INTO provider_discovery_audit_log (
    session_id,
    audit_sequence,
    session_revision,
    audit_kind,
    action_id,
    subject_id,
    summary_key,
    created_at
)
SELECT
    id,
    2,
    1,
    'operation_interrupted',
    printf(
        'migration-v5-action-%016x-%016x',
        length(id),
        rowid
    ),
    printf(
        'migration-v5-event-%016x-%016x',
        length(id),
        rowid
    ),
    CASE state
        WHEN 'interrupted' THEN 'discovery.audit.upgrade_interrupted'
        ELSE 'discovery.audit.upgrade_unknown_outcome'
    END,
    updated_at
FROM provider_discovery_sessions
WHERE revision = 1
  AND state IN ('interrupted', 'unknown_outcome');

-- State changes, revisions, and outbox sequence allocation are inseparable.
-- The application still performs UPDATE ... WHERE revision = ? for CAS; this
-- trigger additionally prevents malformed writers from skipping a revision or
-- event sequence.
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
