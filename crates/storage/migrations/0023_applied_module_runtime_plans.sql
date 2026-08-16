PRAGMA foreign_keys = ON;

CREATE TABLE applied_module_runtime_plans (
    applied_plan_sha256 TEXT PRIMARY KEY NOT NULL CHECK (
        length(applied_plan_sha256) = 64
        AND applied_plan_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    source_activation_plan_sha256 TEXT NOT NULL
        REFERENCES module_activation_plans(plan_sha256)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    source_approval_sha256 TEXT NOT NULL CHECK (
        length(source_approval_sha256) = 64
        AND source_approval_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    derived_from_plan_sha256 TEXT
        REFERENCES applied_module_runtime_plans(applied_plan_sha256)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    conversation_id TEXT,
    branch_id TEXT,
    review_sha256 TEXT NOT NULL CHECK (
        length(review_sha256) = 64
        AND review_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    context_json TEXT NOT NULL CHECK (
        json_valid(context_json)
        AND json_type(context_json) = 'object'
        AND length(CAST(context_json AS BLOB)) <= 1048576
    ),
    runtime_plan_json TEXT NOT NULL CHECK (
        json_valid(runtime_plan_json)
        AND json_type(runtime_plan_json) = 'object'
        AND length(CAST(runtime_plan_json AS BLOB)) <= 8388608
    ),
    state TEXT NOT NULL CHECK (state IN ('applied', 'stale')),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    stale_at TEXT,
    FOREIGN KEY (conversation_id, branch_id)
        REFERENCES conversation_branches(conversation_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    CHECK (
        (conversation_id IS NULL AND branch_id IS NULL)
        OR (conversation_id IS NOT NULL AND branch_id IS NOT NULL)
    ),
    CHECK (
        (state = 'applied' AND stale_at IS NULL)
        OR (state = 'stale' AND stale_at IS NOT NULL)
    )
);

CREATE INDEX applied_module_runtime_plans_context
    ON applied_module_runtime_plans(
        conversation_id,
        branch_id,
        state,
        created_at,
        applied_plan_sha256
    );

CREATE TRIGGER applied_module_runtime_plans_identity_guard
BEFORE UPDATE ON applied_module_runtime_plans
WHEN
    NEW.applied_plan_sha256 != OLD.applied_plan_sha256
    OR NEW.source_activation_plan_sha256
        != OLD.source_activation_plan_sha256
    OR NEW.source_approval_sha256 != OLD.source_approval_sha256
    OR NEW.derived_from_plan_sha256 IS NOT OLD.derived_from_plan_sha256
    OR NEW.conversation_id IS NOT OLD.conversation_id
    OR NEW.branch_id IS NOT OLD.branch_id
    OR NEW.review_sha256 != OLD.review_sha256
    OR NEW.context_json != OLD.context_json
    OR NEW.runtime_plan_json != OLD.runtime_plan_json
    OR NEW.created_at != OLD.created_at
    OR OLD.state != 'applied'
    OR NEW.state != 'stale'
    OR NEW.stale_at IS NULL
BEGIN
    SELECT RAISE(ABORT, 'applied module runtime plan is immutable');
END;

CREATE TRIGGER applied_module_runtime_plans_no_delete
BEFORE DELETE ON applied_module_runtime_plans
BEGIN
    SELECT RAISE(ABORT, 'applied module runtime plans are immutable audit records');
END;
