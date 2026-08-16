PRAGMA foreign_keys = ON;

CREATE TABLE interaction_rule_sets (
    id TEXT PRIMARY KEY NOT NULL
        REFERENCES content_objects(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    schema_version INTEGER NOT NULL CHECK (schema_version >= 1),
    revision INTEGER NOT NULL CHECK (revision >= 1),
    max_actions_per_event INTEGER NOT NULL CHECK (
        max_actions_per_event BETWEEN 1 AND 1024
    ),
    document_json TEXT NOT NULL CHECK (
        json_valid(document_json)
        AND json_type(document_json) = 'object'
        AND length(CAST(document_json AS BLOB)) <= 8388608
    ),
    provenance_json TEXT NOT NULL CHECK (
        json_valid(provenance_json)
        AND json_type(provenance_json) = 'object'
        AND length(CAST(provenance_json AS BLOB)) <= 65536
    ),
    source_kind TEXT NOT NULL CHECK (
        source_kind IN (
            'application_built_in',
            'user_created',
            'imported_standard',
            'imported_package',
            'generated',
            'local_override',
            'migrated'
        )
    ),
    source_hash TEXT CHECK (
        source_hash IS NULL
        OR (
            length(source_hash) = 64
            AND source_hash NOT GLOB '*[^0-9a-f]*'
        )
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    updated_at TEXT NOT NULL CHECK (length(trim(updated_at)) > 0),
    deleted_at TEXT CHECK (
        deleted_at IS NULL OR length(trim(deleted_at)) > 0
    ),
    UNIQUE (id, revision)
);

CREATE TRIGGER interaction_rule_sets_kind_guard
BEFORE INSERT ON interaction_rule_sets
WHEN NOT EXISTS (
    SELECT 1
    FROM content_objects
    WHERE id = NEW.id
      AND object_kind = 'interaction_rule_set'
)
BEGIN
    SELECT RAISE(ABORT, 'interaction rule set object kind is invalid');
END;

CREATE INDEX interaction_rule_sets_active_name
    ON interaction_rule_sets(name COLLATE NOCASE, id)
    WHERE deleted_at IS NULL;

CREATE TABLE interaction_rule_set_revisions (
    revision_id TEXT PRIMARY KEY NOT NULL,
    interaction_rule_set_id TEXT NOT NULL
        REFERENCES interaction_rule_sets(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    revision_no INTEGER NOT NULL CHECK (revision_no >= 1),
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    max_actions_per_event INTEGER NOT NULL CHECK (
        max_actions_per_event BETWEEN 1 AND 1024
    ),
    source_kind TEXT NOT NULL CHECK (
        source_kind IN (
            'application_built_in',
            'user_created',
            'imported_standard',
            'imported_package',
            'generated',
            'local_override',
            'migrated'
        )
    ),
    document_json TEXT NOT NULL CHECK (
        json_valid(document_json)
        AND json_type(document_json) = 'object'
        AND length(CAST(document_json AS BLOB)) <= 8388608
    ),
    UNIQUE (interaction_rule_set_id, revision_id),
    UNIQUE (interaction_rule_set_id, revision_no),
    FOREIGN KEY (interaction_rule_set_id, revision_id)
        REFERENCES content_revisions(object_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

CREATE TABLE interaction_rules (
    set_revision_id TEXT NOT NULL
        REFERENCES interaction_rule_set_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    rule_id TEXT NOT NULL CHECK (length(trim(rule_id)) > 0),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    event_kind TEXT NOT NULL CHECK (
        event_kind IN (
            'conversation_opened',
            'conversation_started',
            'before_generation',
            'after_generation',
            'message_committed',
            'user_action',
            'variable_changed',
            'knowledge_activated'
        )
    ),
    event_argument_json TEXT CHECK (
        event_argument_json IS NULL
        OR (
            json_valid(event_argument_json)
            AND json_type(event_argument_json) = 'object'
            AND length(CAST(event_argument_json AS BLOB)) <= 262144
        )
    ),
    condition_json TEXT CHECK (
        condition_json IS NULL
        OR (
            json_valid(condition_json)
            AND json_type(condition_json) = 'object'
            AND length(CAST(condition_json AS BLOB)) <= 262144
        )
    ),
    priority INTEGER NOT NULL,
    stop_after_match INTEGER NOT NULL CHECK (
        stop_after_match IN (0, 1)
    ),
    provenance_json TEXT NOT NULL CHECK (
        json_valid(provenance_json)
        AND json_type(provenance_json) = 'object'
        AND length(CAST(provenance_json AS BLOB)) <= 65536
    ),
    document_json TEXT NOT NULL CHECK (
        json_valid(document_json)
        AND json_type(document_json) = 'object'
        AND length(CAST(document_json AS BLOB)) <= 2097152
    ),
    PRIMARY KEY (set_revision_id, rule_id),
    UNIQUE (set_revision_id, ordinal),
    CHECK (
        event_kind IN (
            'user_action',
            'variable_changed',
            'knowledge_activated'
        )
        OR event_argument_json IS NULL
    )
);

CREATE INDEX interaction_rules_event_order
    ON interaction_rules(
        set_revision_id,
        event_kind,
        enabled,
        priority DESC,
        ordinal,
        rule_id
    );

CREATE TABLE interaction_actions (
    set_revision_id TEXT NOT NULL,
    rule_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    action_kind TEXT NOT NULL CHECK (
        action_kind IN (
            'set_variable',
            'increment_variable',
            'activate_knowledge',
            'show_asset',
            'play_audio',
            'present_choices',
            'append_visible_system_event',
            'roll_dice',
            'request_user_approval'
        )
    ),
    payload_json TEXT NOT NULL CHECK (
        json_valid(payload_json)
        AND json_type(payload_json) = 'object'
        AND length(CAST(payload_json AS BLOB)) <= 1048576
    ),
    knowledge_book_revision_id TEXT,
    knowledge_entry_id TEXT,
    asset_descriptor_id TEXT
        REFERENCES asset_descriptors(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    requires_approval INTEGER NOT NULL CHECK (
        requires_approval IN (0, 1)
    ),
    PRIMARY KEY (set_revision_id, rule_id, ordinal),
    FOREIGN KEY (set_revision_id, rule_id)
        REFERENCES interaction_rules(set_revision_id, rule_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    FOREIGN KEY (knowledge_book_revision_id, knowledge_entry_id)
        REFERENCES knowledge_entries(book_revision_id, entry_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    CHECK (
        (
            action_kind = 'activate_knowledge'
            AND knowledge_book_revision_id IS NOT NULL
            AND knowledge_entry_id IS NOT NULL
            AND asset_descriptor_id IS NULL
        )
        OR (
            action_kind IN ('show_asset', 'play_audio')
            AND knowledge_book_revision_id IS NULL
            AND knowledge_entry_id IS NULL
            AND asset_descriptor_id IS NOT NULL
        )
        OR (
            action_kind NOT IN (
                'activate_knowledge',
                'show_asset',
                'play_audio'
            )
            AND knowledge_book_revision_id IS NULL
            AND knowledge_entry_id IS NULL
            AND asset_descriptor_id IS NULL
        )
    ),
    CHECK (
        action_kind <> 'request_user_approval'
        OR requires_approval = 1
    )
);

CREATE TABLE interaction_state (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(trim(id)) > 0),
    conversation_id TEXT NOT NULL
        REFERENCES conversations(id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE,
    branch_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision >= 0),
    document_json TEXT NOT NULL CHECK (
        json_valid(document_json)
        AND json_type(document_json) = 'object'
        AND length(CAST(document_json AS BLOB)) <= 8388608
    ),
    updated_at TEXT NOT NULL CHECK (length(trim(updated_at)) > 0),
    UNIQUE (conversation_id, branch_id),
    UNIQUE (id, revision),
    FOREIGN KEY (conversation_id, branch_id)
        REFERENCES conversation_branches(conversation_id, id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE
);

CREATE INDEX interaction_state_branch
    ON interaction_state(conversation_id, branch_id, id);

CREATE TRIGGER interaction_state_revision_guard
BEFORE UPDATE ON interaction_state
WHEN
    NEW.id != OLD.id
    OR NEW.conversation_id != OLD.conversation_id
    OR NEW.branch_id != OLD.branch_id
    OR NEW.revision != OLD.revision + 1
BEGIN
    SELECT RAISE(ABORT, 'interaction state update is not versioned');
END;

CREATE TABLE interaction_state_variables (
    interaction_state_id TEXT NOT NULL
        REFERENCES interaction_state(id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE,
    scope TEXT NOT NULL CHECK (
        scope IN (
            'app',
            'user',
            'persona',
            'character',
            'conversation',
            'branch',
            'module'
        )
    ),
    namespace TEXT NOT NULL DEFAULT '',
    variable_id TEXT NOT NULL CHECK (
        length(trim(variable_id)) > 0
    ),
    value_type TEXT NOT NULL CHECK (
        value_type IN (
            'bool',
            'integer',
            'decimal',
            'text',
            'enum',
            'string_list'
        )
    ),
    value_json TEXT NOT NULL CHECK (
        json_valid(value_json)
        AND length(CAST(value_json AS BLOB)) <= 262144
    ),
    state_revision INTEGER NOT NULL CHECK (state_revision >= 0),
    updated_at TEXT NOT NULL CHECK (length(trim(updated_at)) > 0),
    PRIMARY KEY (
        interaction_state_id,
        scope,
        namespace,
        variable_id
    ),
    CHECK (
        (scope = 'module' AND length(trim(namespace)) > 0)
        OR (scope <> 'module' AND namespace = '')
    ),
    CHECK (
        scope <> 'module'
        OR instr(variable_id, namespace || '.') = 1
    )
);

CREATE INDEX interaction_state_variables_lookup
    ON interaction_state_variables(
        interaction_state_id,
        scope,
        namespace,
        variable_id
    );

CREATE TABLE interaction_state_knowledge (
    interaction_state_id TEXT NOT NULL
        REFERENCES interaction_state(id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE,
    book_revision_id TEXT NOT NULL,
    entry_id TEXT NOT NULL,
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    state_revision INTEGER NOT NULL CHECK (state_revision >= 0),
    PRIMARY KEY (
        interaction_state_id,
        book_revision_id,
        entry_id
    ),
    FOREIGN KEY (book_revision_id, entry_id)
        REFERENCES knowledge_entries(book_revision_id, entry_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

CREATE TABLE interaction_events (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(trim(id)) > 0),
    idempotency_key TEXT NOT NULL UNIQUE CHECK (
        length(trim(idempotency_key)) > 0
    ),
    interaction_state_id TEXT NOT NULL
        REFERENCES interaction_state(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    expected_state_revision INTEGER NOT NULL CHECK (
        expected_state_revision >= 0
    ),
    resulting_state_revision INTEGER NOT NULL CHECK (
        resulting_state_revision = expected_state_revision + 1
    ),
    conversation_id TEXT NOT NULL,
    branch_id TEXT NOT NULL,
    event_kind TEXT NOT NULL CHECK (
        event_kind IN (
            'conversation_opened',
            'conversation_started',
            'before_generation',
            'after_generation',
            'message_committed',
            'user_action',
            'variable_changed',
            'knowledge_activated'
        )
    ),
    event_argument_json TEXT CHECK (
        event_argument_json IS NULL
        OR (
            json_valid(event_argument_json)
            AND json_type(event_argument_json) = 'object'
            AND length(CAST(event_argument_json AS BLOB)) <= 262144
        )
    ),
    module_plan_sha256 TEXT NOT NULL CHECK (
        length(module_plan_sha256) = 64
        AND module_plan_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    policy_json TEXT NOT NULL CHECK (
        json_valid(policy_json)
        AND json_type(policy_json) = 'object'
        AND length(CAST(policy_json AS BLOB)) <= 1048576
    ),
    policy_sha256 TEXT NOT NULL CHECK (
        length(policy_sha256) = 64
        AND policy_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    payload_json TEXT NOT NULL CHECK (
        json_valid(payload_json)
        AND json_type(payload_json) = 'object'
        AND length(CAST(payload_json AS BLOB)) <= 1048576
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    UNIQUE (interaction_state_id, resulting_state_revision),
    FOREIGN KEY (conversation_id, branch_id)
        REFERENCES conversation_branches(conversation_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

CREATE TABLE interaction_event_policy_rule_sets (
    event_id TEXT NOT NULL
        REFERENCES interaction_events(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    rule_set_id TEXT NOT NULL,
    rule_set_revision_id TEXT NOT NULL,
    revision_sha256 TEXT NOT NULL CHECK (
        length(revision_sha256) = 64
        AND revision_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    PRIMARY KEY (event_id, ordinal),
    UNIQUE (event_id, rule_set_id),
    FOREIGN KEY (rule_set_id, rule_set_revision_id)
        REFERENCES interaction_rule_set_revisions(
            interaction_rule_set_id,
            revision_id
        )
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

CREATE INDEX interaction_events_branch
    ON interaction_events(
        conversation_id,
        branch_id,
        created_at,
        id
    );

CREATE TABLE interaction_action_results (
    event_id TEXT NOT NULL
        REFERENCES interaction_events(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    set_revision_id TEXT NOT NULL,
    rule_id TEXT NOT NULL,
    action_ordinal INTEGER NOT NULL CHECK (action_ordinal >= 0),
    result_ordinal INTEGER NOT NULL CHECK (result_ordinal >= 0),
    status TEXT NOT NULL CHECK (
        status IN ('proposed', 'applied', 'skipped', 'failed')
    ),
    result_json TEXT NOT NULL CHECK (
        json_valid(result_json)
        AND json_type(result_json) = 'object'
        AND length(CAST(result_json AS BLOB)) <= 1048576
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    PRIMARY KEY (event_id, result_ordinal),
    FOREIGN KEY (set_revision_id, rule_id, action_ordinal)
        REFERENCES interaction_actions(
            set_revision_id,
            rule_id,
            ordinal
        )
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

CREATE TABLE interaction_effect_outbox (
    event_id TEXT NOT NULL
        REFERENCES interaction_events(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    sequence INTEGER NOT NULL CHECK (sequence >= 1),
    effect_id TEXT NOT NULL UNIQUE CHECK (length(trim(effect_id)) > 0),
    effect_kind TEXT NOT NULL CHECK (
        effect_kind IN (
            'asset_shown',
            'audio_requested',
            'choices_presented',
            'visible_system_event',
            'dice_rolled',
            'approval_requested'
        )
    ),
    effect_json TEXT NOT NULL CHECK (
        json_valid(effect_json)
        AND json_type(effect_json) = 'object'
        AND length(CAST(effect_json AS BLOB)) <= 1048576
    ),
    available_at TEXT NOT NULL CHECK (length(trim(available_at)) > 0),
    delivery_attempts INTEGER NOT NULL DEFAULT 0 CHECK (
        delivery_attempts >= 0
    ),
    delivered_at TEXT,
    choice_status TEXT CHECK (
        choice_status IN ('pending', 'consumed', 'expired')
    ),
    choice_id TEXT,
    choice_decided_at_epoch_seconds INTEGER,
    PRIMARY KEY (event_id, sequence),
    CHECK (
        (
            effect_kind = 'choices_presented'
            AND choice_status IS NOT NULL
        )
        OR (
            effect_kind != 'choices_presented'
            AND choice_status IS NULL
            AND choice_id IS NULL
            AND choice_decided_at_epoch_seconds IS NULL
        )
    ),
    CHECK (
        choice_status IS NULL
        OR (
            choice_status = 'pending'
            AND choice_id IS NULL
            AND choice_decided_at_epoch_seconds IS NULL
        )
        OR (
            choice_status = 'consumed'
            AND length(trim(choice_id)) > 0
            AND choice_decided_at_epoch_seconds IS NOT NULL
        )
        OR (
            choice_status = 'expired'
            AND choice_id IS NULL
            AND choice_decided_at_epoch_seconds IS NOT NULL
        )
    )
);

CREATE INDEX interaction_effect_outbox_pending
    ON interaction_effect_outbox(available_at, event_id, sequence)
    WHERE delivered_at IS NULL;

-- Approval records contain only validated proposal data. Approval is a CAS
-- transition from pending and dispatches the proposal id as a UserAction; no
-- arbitrary action name or arguments can be injected at approval time.
CREATE TABLE interaction_proposals (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(trim(id)) > 0),
    interaction_state_id TEXT NOT NULL
        REFERENCES interaction_state(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    rule_set_revision_id TEXT NOT NULL,
    rule_id TEXT NOT NULL,
    action_ordinal INTEGER NOT NULL CHECK (action_ordinal >= 0),
    proposal_id TEXT NOT NULL CHECK (length(trim(proposal_id)) > 0),
    title TEXT NOT NULL CHECK (length(trim(title)) > 0),
    body TEXT NOT NULL CHECK (
        length(CAST(body AS BLOB)) <= 1048576
    ),
    status TEXT NOT NULL CHECK (
        status IN ('pending', 'approved', 'rejected', 'expired')
    ),
    source_interaction_state_revision INTEGER NOT NULL CHECK (
        source_interaction_state_revision >= 0
    ),
    proposal_revision INTEGER NOT NULL CHECK (proposal_revision >= 1),
    payload_json TEXT NOT NULL CHECK (
        json_valid(payload_json)
        AND json_type(payload_json) = 'object'
        AND length(CAST(payload_json AS BLOB)) <= 1048576
    ),
    payload_sha256 TEXT NOT NULL CHECK (
        length(payload_sha256) = 64
        AND payload_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    requested_at_epoch_seconds INTEGER NOT NULL,
    expires_at_epoch_seconds INTEGER,
    decided_at_epoch_seconds INTEGER,
    dispatched_at_epoch_seconds INTEGER,
    FOREIGN KEY (
        rule_set_revision_id,
        rule_id,
        action_ordinal
    )
        REFERENCES interaction_actions(
            set_revision_id,
            rule_id,
            ordinal
        )
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    CHECK (
        expires_at_epoch_seconds IS NULL
        OR expires_at_epoch_seconds > requested_at_epoch_seconds
    ),
    CHECK (
        (status = 'pending'
            AND decided_at_epoch_seconds IS NULL
            AND dispatched_at_epoch_seconds IS NULL)
        OR (status = 'approved'
            AND decided_at_epoch_seconds IS NOT NULL)
        OR (status = 'rejected'
            AND decided_at_epoch_seconds IS NOT NULL
            AND dispatched_at_epoch_seconds IS NULL)
        OR (status = 'expired'
            AND decided_at_epoch_seconds IS NOT NULL
            AND dispatched_at_epoch_seconds IS NULL)
    ),
    CHECK (
        dispatched_at_epoch_seconds IS NULL OR status = 'approved'
    )
);

CREATE INDEX interaction_proposals_pending
    ON interaction_proposals(
        interaction_state_id,
        expires_at_epoch_seconds,
        requested_at_epoch_seconds,
        id
    )
    WHERE status = 'pending';

CREATE UNIQUE INDEX interaction_proposals_one_pending_id
    ON interaction_proposals(interaction_state_id, proposal_id)
    WHERE status = 'pending';

CREATE TRIGGER interaction_proposals_initial_state_guard
BEFORE INSERT ON interaction_proposals
WHEN NEW.status != 'pending' OR NEW.proposal_revision != 1
BEGIN
    SELECT RAISE(ABORT, 'interaction proposal must begin pending');
END;

CREATE TRIGGER interaction_proposals_transition_guard
BEFORE UPDATE ON interaction_proposals
WHEN
    NEW.id != OLD.id
    OR NEW.interaction_state_id != OLD.interaction_state_id
    OR NEW.rule_set_revision_id != OLD.rule_set_revision_id
    OR NEW.rule_id != OLD.rule_id
    OR NEW.action_ordinal != OLD.action_ordinal
    OR NEW.proposal_id != OLD.proposal_id
    OR NEW.title != OLD.title
    OR NEW.body != OLD.body
    OR NEW.source_interaction_state_revision
       != OLD.source_interaction_state_revision
    OR NEW.payload_json != OLD.payload_json
    OR NEW.payload_sha256 != OLD.payload_sha256
    OR NEW.requested_at_epoch_seconds != OLD.requested_at_epoch_seconds
    OR NEW.expires_at_epoch_seconds IS NOT OLD.expires_at_epoch_seconds
    OR NEW.proposal_revision != OLD.proposal_revision + 1
    OR (
        OLD.status = 'pending'
        AND NEW.status NOT IN ('approved', 'rejected', 'expired')
    )
    OR (OLD.status != 'pending' AND NEW.status != OLD.status)
    OR (
        OLD.status != 'pending'
        AND NEW.decided_at_epoch_seconds
            IS NOT OLD.decided_at_epoch_seconds
    )
    OR (
        OLD.status IN ('rejected', 'expired')
        OR (
            OLD.status = 'approved'
            AND OLD.dispatched_at_epoch_seconds IS NOT NULL
        )
        OR (
            OLD.status = 'approved'
            AND OLD.dispatched_at_epoch_seconds IS NULL
            AND NEW.dispatched_at_epoch_seconds IS NULL
        )
    )
    OR (
        OLD.dispatched_at_epoch_seconds IS NOT NULL
        AND NEW.dispatched_at_epoch_seconds
            IS NOT OLD.dispatched_at_epoch_seconds
    )
BEGIN
    SELECT RAISE(ABORT, 'interaction proposal transition is invalid');
END;

CREATE TABLE interaction_proposal_audit (
    proposal_id TEXT NOT NULL
        REFERENCES interaction_proposals(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    sequence INTEGER NOT NULL CHECK (sequence >= 1),
    proposal_revision INTEGER NOT NULL CHECK (proposal_revision >= 1),
    event_kind TEXT NOT NULL CHECK (
        event_kind IN (
            'requested',
            'approved',
            'rejected',
            'expired',
            'dispatched'
        )
    ),
    payload_json TEXT NOT NULL CHECK (
        json_valid(payload_json)
        AND json_type(payload_json) = 'object'
        AND length(CAST(payload_json AS BLOB)) <= 262144
    ),
    created_at_epoch_seconds INTEGER NOT NULL,
    PRIMARY KEY (proposal_id, sequence),
    UNIQUE (proposal_id, proposal_revision)
);

CREATE TRIGGER interaction_proposal_audit_append_guard
BEFORE INSERT ON interaction_proposal_audit
WHEN NEW.sequence != (
    SELECT COALESCE(MAX(sequence), 0) + 1
    FROM interaction_proposal_audit
    WHERE proposal_id = NEW.proposal_id
)
BEGIN
    SELECT RAISE(ABORT, 'interaction proposal audit is not append-only');
END;

CREATE TABLE prompt_preset_interaction_rule_sets (
    prompt_preset_revision_id TEXT NOT NULL
        REFERENCES prompt_preset_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    interaction_rule_set_revision_id TEXT NOT NULL
        REFERENCES interaction_rule_set_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    config_json TEXT NOT NULL CHECK (
        json_valid(config_json)
        AND json_type(config_json) = 'object'
        AND length(CAST(config_json AS BLOB)) <= 262144
    ),
    PRIMARY KEY (prompt_preset_revision_id, ordinal),
    UNIQUE (
        prompt_preset_revision_id,
        interaction_rule_set_revision_id
    )
);

CREATE TABLE content_modules (
    id TEXT PRIMARY KEY NOT NULL
        REFERENCES content_objects(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    version TEXT NOT NULL CHECK (length(trim(version)) > 0),
    schema_version INTEGER NOT NULL CHECK (schema_version >= 1),
    revision INTEGER NOT NULL CHECK (revision >= 1),
    document_json TEXT NOT NULL CHECK (
        json_valid(document_json)
        AND json_type(document_json) = 'object'
        AND length(CAST(document_json AS BLOB)) <= 16777216
    ),
    metadata_json TEXT NOT NULL CHECK (
        json_valid(metadata_json)
        AND json_type(metadata_json) = 'object'
        AND length(CAST(metadata_json AS BLOB)) <= 1048576
    ),
    source_kind TEXT NOT NULL CHECK (
        source_kind IN (
            'application_built_in',
            'user_created',
            'imported_standard',
            'imported_package',
            'generated',
            'local_override',
            'migrated'
        )
    ),
    source_hash TEXT CHECK (
        source_hash IS NULL
        OR (
            length(source_hash) = 64
            AND source_hash NOT GLOB '*[^0-9a-f]*'
        )
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    updated_at TEXT NOT NULL CHECK (length(trim(updated_at)) > 0),
    deleted_at TEXT CHECK (
        deleted_at IS NULL OR length(trim(deleted_at)) > 0
    ),
    UNIQUE (id, revision)
);

CREATE TRIGGER content_modules_kind_guard
BEFORE INSERT ON content_modules
WHEN NOT EXISTS (
    SELECT 1
    FROM content_objects
    WHERE id = NEW.id
      AND object_kind = 'content_module'
)
BEGIN
    SELECT RAISE(ABORT, 'content module object kind is invalid');
END;

CREATE INDEX content_modules_active_name
    ON content_modules(name COLLATE NOCASE, version, id)
    WHERE deleted_at IS NULL;

CREATE TABLE content_module_revisions (
    revision_id TEXT PRIMARY KEY NOT NULL,
    module_id TEXT NOT NULL
        REFERENCES content_modules(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    revision_no INTEGER NOT NULL CHECK (revision_no >= 1),
    version TEXT NOT NULL CHECK (length(trim(version)) > 0),
    previous_revision_id TEXT,
    source_kind TEXT NOT NULL CHECK (
        source_kind IN (
            'application_built_in',
            'user_created',
            'imported_standard',
            'imported_package',
            'generated',
            'local_override',
            'migrated'
        )
    ),
    source_hash TEXT NOT NULL CHECK (
        length(source_hash) = 64
        AND source_hash NOT GLOB '*[^0-9a-f]*'
    ),
    metadata_json TEXT NOT NULL CHECK (
        json_valid(metadata_json)
        AND json_type(metadata_json) = 'object'
        AND length(CAST(metadata_json AS BLOB)) <= 1048576
    ),
    document_json TEXT NOT NULL CHECK (
        json_valid(document_json)
        AND json_type(document_json) = 'object'
        AND length(CAST(document_json AS BLOB)) <= 16777216
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    UNIQUE (module_id, revision_id),
    UNIQUE (module_id, revision_no),
    FOREIGN KEY (module_id, revision_id)
        REFERENCES content_revisions(object_id, id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    FOREIGN KEY (module_id, previous_revision_id)
        REFERENCES content_module_revisions(module_id, revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    CHECK (
        (revision_no = 1 AND previous_revision_id IS NULL)
        OR (revision_no > 1 AND previous_revision_id IS NOT NULL)
    )
);

CREATE TABLE content_module_variables (
    module_revision_id TEXT NOT NULL
        REFERENCES content_module_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    variable_id TEXT NOT NULL CHECK (
        length(trim(variable_id)) > 0
        AND instr(variable_id, '.') > 1
    ),
    value_type TEXT NOT NULL CHECK (
        value_type IN (
            'bool',
            'integer',
            'decimal',
            'text',
            'enum',
            'string_list'
        )
    ),
    default_value_json TEXT NOT NULL CHECK (
        json_valid(default_value_json)
        AND length(CAST(default_value_json AS BLOB)) <= 65536
    ),
    sensitive INTEGER NOT NULL CHECK (sensitive IN (0, 1)),
    document_json TEXT NOT NULL CHECK (
        json_valid(document_json)
        AND json_type(document_json) = 'object'
        AND length(CAST(document_json AS BLOB)) <= 262144
    ),
    PRIMARY KEY (module_revision_id, variable_id)
);

CREATE TABLE content_module_controls (
    module_revision_id TEXT NOT NULL
        REFERENCES content_module_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    control_id TEXT NOT NULL CHECK (length(trim(control_id)) > 0),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    kind TEXT NOT NULL CHECK (
        kind IN (
            'toggle',
            'select',
            'multi_select',
            'text',
            'number',
            'slider',
            'section',
            'caption',
            'divider'
        )
    ),
    variable_id TEXT,
    label TEXT NOT NULL,
    document_json TEXT NOT NULL CHECK (
        json_valid(document_json)
        AND json_type(document_json) = 'object'
        AND length(CAST(document_json AS BLOB)) <= 524288
    ),
    PRIMARY KEY (module_revision_id, control_id),
    UNIQUE (module_revision_id, ordinal),
    FOREIGN KEY (module_revision_id, variable_id)
        REFERENCES content_module_variables(module_revision_id, variable_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    CHECK (
        (
            kind IN ('section', 'caption', 'divider')
            AND variable_id IS NULL
        )
        OR (
            kind NOT IN ('section', 'caption', 'divider')
            AND variable_id IS NOT NULL
        )
    )
);

CREATE TABLE content_module_prompt_blocks (
    module_revision_id TEXT NOT NULL
        REFERENCES content_module_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    block_id TEXT NOT NULL CHECK (length(trim(block_id)) > 0),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    name TEXT NOT NULL CHECK (length(trim(name)) > 0),
    kind TEXT NOT NULL CHECK (
        kind IN (
            'static_instruction',
            'character_identity',
            'character_description',
            'character_personality',
            'scenario',
            'user_persona',
            'dialogue_examples',
            'world_knowledge',
            'retrieved_memory',
            'conversation_summary',
            'history_slice',
            'latest_user_turn',
            'author_note',
            'post_history_instruction',
            'assistant_prefill',
            'group_context'
        )
    ),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    authority TEXT NOT NULL CHECK (
        authority IN (
            'application',
            'creator',
            'user',
            'conversation',
            'imported_content'
        )
    ),
    role_hint TEXT NOT NULL CHECK (
        role_hint IN (
            'system',
            'developer',
            'user',
            'assistant',
            'provider_default'
        )
    ),
    placement_zone TEXT NOT NULL CHECK (
        placement_zone IN (
            'application_policy',
            'preset_instruction',
            'character_context',
            'retrieved_context',
            'older_history',
            'recent_enhancement',
            'recent_history',
            'post_history',
            'latest_user',
            'assistant_prefill'
        )
    ),
    document_json TEXT NOT NULL CHECK (
        json_valid(document_json)
        AND json_type(document_json) = 'object'
        AND length(CAST(document_json AS BLOB)) <= 4194304
    ),
    PRIMARY KEY (module_revision_id, block_id),
    UNIQUE (module_revision_id, ordinal)
);

CREATE TABLE content_module_required_capabilities (
    module_revision_id TEXT NOT NULL
        REFERENCES content_module_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    capability TEXT NOT NULL CHECK (
        capability IN (
            'prompt_fragments',
            'knowledge',
            'variables',
            'transforms',
            'declarative_interactions',
            'image_assets',
            'audio_assets',
            'video_assets',
            'attachment_assets',
            'high_risk_assets'
        )
    ),
    support_status TEXT NOT NULL CHECK (
        support_status IN (
            'supported',
            'unsupported',
            'approval_required'
        )
    ),
    approved INTEGER NOT NULL CHECK (approved IN (0, 1)),
    reason TEXT NOT NULL CHECK (length(trim(reason)) > 0),
    PRIMARY KEY (module_revision_id, capability),
    CHECK (
        support_status = 'supported'
        OR approved = 0
    )
);

CREATE TABLE content_module_components (
    module_revision_id TEXT NOT NULL
        REFERENCES content_module_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    component_kind TEXT NOT NULL CHECK (
        component_kind IN (
            'prompt_block',
            'control',
            'knowledge_book',
            'memory_profile',
            'transform_set',
            'interaction_rule_set',
            'asset'
        )
    ),
    prompt_block_id TEXT,
    control_id TEXT,
    knowledge_book_revision_id TEXT
        REFERENCES knowledge_book_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    memory_profile_revision_id TEXT
        REFERENCES memory_profile_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    transform_set_revision_id TEXT
        REFERENCES transform_set_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    interaction_rule_set_revision_id TEXT
        REFERENCES interaction_rule_set_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    asset_descriptor_id TEXT
        REFERENCES asset_descriptors(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    merge_policy TEXT NOT NULL CHECK (
        merge_policy IN (
            'append',
            'prepend',
            'replace',
            'merge',
            'require_resolution'
        )
    ),
    component_sha256 TEXT NOT NULL CHECK (
        length(component_sha256) = 64
        AND component_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    config_json TEXT NOT NULL CHECK (
        json_valid(config_json)
        AND json_type(config_json) = 'object'
        AND length(CAST(config_json AS BLOB)) <= 1048576
    ),
    PRIMARY KEY (module_revision_id, ordinal),
    FOREIGN KEY (module_revision_id, prompt_block_id)
        REFERENCES content_module_prompt_blocks(
            module_revision_id,
            block_id
        )
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    FOREIGN KEY (module_revision_id, control_id)
        REFERENCES content_module_controls(
            module_revision_id,
            control_id
        )
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    CHECK (
        (component_kind = 'prompt_block'
            AND prompt_block_id IS NOT NULL
            AND control_id IS NULL
            AND knowledge_book_revision_id IS NULL
            AND memory_profile_revision_id IS NULL
            AND transform_set_revision_id IS NULL
            AND interaction_rule_set_revision_id IS NULL
            AND asset_descriptor_id IS NULL)
        OR (component_kind = 'control'
            AND prompt_block_id IS NULL
            AND control_id IS NOT NULL
            AND knowledge_book_revision_id IS NULL
            AND memory_profile_revision_id IS NULL
            AND transform_set_revision_id IS NULL
            AND interaction_rule_set_revision_id IS NULL
            AND asset_descriptor_id IS NULL)
        OR (component_kind = 'knowledge_book'
            AND prompt_block_id IS NULL
            AND control_id IS NULL
            AND knowledge_book_revision_id IS NOT NULL
            AND memory_profile_revision_id IS NULL
            AND transform_set_revision_id IS NULL
            AND interaction_rule_set_revision_id IS NULL
            AND asset_descriptor_id IS NULL)
        OR (component_kind = 'memory_profile'
            AND prompt_block_id IS NULL
            AND control_id IS NULL
            AND knowledge_book_revision_id IS NULL
            AND memory_profile_revision_id IS NOT NULL
            AND transform_set_revision_id IS NULL
            AND interaction_rule_set_revision_id IS NULL
            AND asset_descriptor_id IS NULL)
        OR (component_kind = 'transform_set'
            AND prompt_block_id IS NULL
            AND control_id IS NULL
            AND knowledge_book_revision_id IS NULL
            AND memory_profile_revision_id IS NULL
            AND transform_set_revision_id IS NOT NULL
            AND interaction_rule_set_revision_id IS NULL
            AND asset_descriptor_id IS NULL)
        OR (component_kind = 'interaction_rule_set'
            AND prompt_block_id IS NULL
            AND control_id IS NULL
            AND knowledge_book_revision_id IS NULL
            AND memory_profile_revision_id IS NULL
            AND transform_set_revision_id IS NULL
            AND interaction_rule_set_revision_id IS NOT NULL
            AND asset_descriptor_id IS NULL)
        OR (component_kind = 'asset'
            AND prompt_block_id IS NULL
            AND control_id IS NULL
            AND knowledge_book_revision_id IS NULL
            AND memory_profile_revision_id IS NULL
            AND transform_set_revision_id IS NULL
            AND interaction_rule_set_revision_id IS NULL
            AND asset_descriptor_id IS NOT NULL)
    )
);

CREATE INDEX content_module_components_knowledge
    ON content_module_components(
        knowledge_book_revision_id,
        module_revision_id
    )
    WHERE knowledge_book_revision_id IS NOT NULL;
CREATE INDEX content_module_components_transform
    ON content_module_components(
        transform_set_revision_id,
        module_revision_id
    )
    WHERE transform_set_revision_id IS NOT NULL;
CREATE INDEX content_module_components_interaction
    ON content_module_components(
        interaction_rule_set_revision_id,
        module_revision_id
    )
    WHERE interaction_rule_set_revision_id IS NOT NULL;
CREATE INDEX content_module_components_asset
    ON content_module_components(asset_descriptor_id, module_revision_id)
    WHERE asset_descriptor_id IS NOT NULL;

CREATE TABLE content_module_bindings (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(trim(id)) > 0),
    revision INTEGER NOT NULL CHECK (revision >= 1),
    module_id TEXT NOT NULL
        REFERENCES content_modules(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    resolution_mode TEXT NOT NULL CHECK (
        resolution_mode IN ('active', 'pinned')
    ),
    pinned_revision_id TEXT,
    scope_kind TEXT NOT NULL CHECK (
        scope_kind IN (
            'app',
            'user',
            'persona',
            'character',
            'conversation',
            'branch'
        )
    ),
    persona_id TEXT
        REFERENCES personas(object_id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE,
    character_id TEXT
        REFERENCES characters(id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE,
    conversation_id TEXT
        REFERENCES conversations(id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE,
    branch_id TEXT,
    priority INTEGER NOT NULL,
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    approved INTEGER NOT NULL CHECK (approved IN (0, 1)),
    activation_approval_id TEXT CHECK (
        activation_approval_id IS NULL
        OR length(trim(activation_approval_id)) > 0
    ),
    activation_review_sha256 TEXT CHECK (
        activation_review_sha256 IS NULL
        OR (
            length(activation_review_sha256) = 64
            AND activation_review_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    ),
    activation_plan_sha256 TEXT CHECK (
        activation_plan_sha256 IS NULL
        OR (
            length(activation_plan_sha256) = 64
            AND activation_plan_sha256 NOT GLOB '*[^0-9a-f]*'
        )
    ),
    package_import_approval_id TEXT
        REFERENCES package_import_approvals(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    variable_overrides_json TEXT NOT NULL CHECK (
        json_valid(variable_overrides_json)
        AND json_type(variable_overrides_json) = 'object'
        AND length(CAST(variable_overrides_json AS BLOB)) <= 1048576
    ),
    document_json TEXT NOT NULL CHECK (
        json_valid(document_json)
        AND json_type(document_json) = 'object'
        AND length(CAST(document_json AS BLOB)) <= 2097152
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    updated_at TEXT NOT NULL CHECK (length(trim(updated_at)) > 0),
    deleted_at TEXT CHECK (
        deleted_at IS NULL OR length(trim(deleted_at)) > 0
    ),
    UNIQUE (id, revision),
    FOREIGN KEY (module_id, pinned_revision_id)
        REFERENCES content_module_revisions(module_id, revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    FOREIGN KEY (conversation_id, branch_id)
        REFERENCES conversation_branches(conversation_id, id)
        ON UPDATE RESTRICT
        ON DELETE CASCADE,
    CHECK (
        (resolution_mode = 'active' AND pinned_revision_id IS NULL)
        OR (resolution_mode = 'pinned' AND pinned_revision_id IS NOT NULL)
    ),
    CHECK (
        (approved = 0
            AND activation_approval_id IS NULL
            AND activation_review_sha256 IS NULL
            AND activation_plan_sha256 IS NULL)
        OR (approved = 1
            AND activation_approval_id IS NOT NULL
            AND activation_review_sha256 IS NOT NULL
            AND activation_plan_sha256 IS NOT NULL)
    ),
    CHECK (enabled = 0 OR approved = 1),
    CHECK (
        (scope_kind IN ('app', 'user')
            AND persona_id IS NULL
            AND character_id IS NULL
            AND conversation_id IS NULL
            AND branch_id IS NULL)
        OR (scope_kind = 'persona'
            AND persona_id IS NOT NULL
            AND character_id IS NULL
            AND conversation_id IS NULL
            AND branch_id IS NULL)
        OR (scope_kind = 'character'
            AND persona_id IS NULL
            AND character_id IS NOT NULL
            AND conversation_id IS NULL
            AND branch_id IS NULL)
        OR (scope_kind = 'conversation'
            AND persona_id IS NULL
            AND character_id IS NULL
            AND conversation_id IS NOT NULL
            AND branch_id IS NULL)
        OR (scope_kind = 'branch'
            AND persona_id IS NULL
            AND character_id IS NULL
            AND conversation_id IS NOT NULL
            AND branch_id IS NOT NULL)
    )
);

CREATE UNIQUE INDEX module_bindings_one_app
    ON content_module_bindings(module_id)
    WHERE scope_kind = 'app' AND deleted_at IS NULL;
CREATE UNIQUE INDEX module_bindings_one_user
    ON content_module_bindings(module_id)
    WHERE scope_kind = 'user' AND deleted_at IS NULL;
CREATE UNIQUE INDEX module_bindings_one_persona
    ON content_module_bindings(module_id, persona_id)
    WHERE scope_kind = 'persona' AND deleted_at IS NULL;
CREATE UNIQUE INDEX module_bindings_one_character
    ON content_module_bindings(module_id, character_id)
    WHERE scope_kind = 'character' AND deleted_at IS NULL;
CREATE UNIQUE INDEX module_bindings_one_conversation
    ON content_module_bindings(module_id, conversation_id)
    WHERE scope_kind = 'conversation' AND deleted_at IS NULL;
CREATE UNIQUE INDEX module_bindings_one_branch
    ON content_module_bindings(module_id, conversation_id, branch_id)
    WHERE scope_kind = 'branch' AND deleted_at IS NULL;
CREATE INDEX module_bindings_resolution
    ON content_module_bindings(
        scope_kind,
        enabled,
        priority,
        id
    )
    WHERE deleted_at IS NULL;

CREATE TRIGGER content_module_bindings_revision_guard
BEFORE UPDATE ON content_module_bindings
WHEN
    NEW.id != OLD.id
    OR NEW.revision != OLD.revision + 1
BEGIN
    SELECT RAISE(ABORT, 'content module binding update is not versioned');
END;

CREATE TRIGGER content_module_bindings_import_guard
BEFORE INSERT ON content_module_bindings
WHEN
    NEW.enabled = 1
    AND EXISTS (
        SELECT 1
        FROM content_module_revisions AS revision
        WHERE revision.module_id = NEW.module_id
          AND revision.revision_id = COALESCE(
              NEW.pinned_revision_id,
              (
                  SELECT active_revision_id
                  FROM content_object_state
                  WHERE object_id = NEW.module_id
              )
          )
          AND revision.source_kind IN (
              'imported_standard',
              'imported_package'
          )
          AND NOT EXISTS (
              SELECT 1
              FROM package_import_approvals AS approval
              JOIN package_import_components AS component
                ON component.import_id = approval.import_id
               AND component.selected = 1
               AND component.disposition IN ('create', 'update')
              JOIN package_import_component_commits AS committed
                ON committed.import_id = component.import_id
               AND committed.component_ordinal = component.ordinal
               AND committed.target_object_id = NEW.module_id
               AND committed.target_revision_id = revision.revision_id
              WHERE approval.id = NEW.package_import_approval_id
          )
    )
BEGIN
    SELECT RAISE(ABORT, 'imported module binding requires exact approval');
END;

CREATE TRIGGER content_module_bindings_import_update_guard
BEFORE UPDATE OF
    module_id,
    resolution_mode,
    pinned_revision_id,
    enabled,
    approved,
    activation_approval_id,
    activation_review_sha256,
    activation_plan_sha256,
    package_import_approval_id
ON content_module_bindings
WHEN
    NEW.enabled = 1
    AND EXISTS (
        SELECT 1
        FROM content_module_revisions AS revision
        WHERE revision.module_id = NEW.module_id
          AND revision.revision_id = COALESCE(
              NEW.pinned_revision_id,
              (
                  SELECT active_revision_id
                  FROM content_object_state
                  WHERE object_id = NEW.module_id
              )
          )
          AND revision.source_kind IN (
              'imported_standard',
              'imported_package'
          )
          AND NOT EXISTS (
              SELECT 1
              FROM package_import_approvals AS approval
              JOIN package_import_components AS component
                ON component.import_id = approval.import_id
               AND component.selected = 1
               AND component.disposition IN ('create', 'update')
              JOIN package_import_component_commits AS committed
                ON committed.import_id = component.import_id
               AND committed.component_ordinal = component.ordinal
               AND committed.target_object_id = NEW.module_id
               AND committed.target_revision_id = revision.revision_id
              WHERE approval.id = NEW.package_import_approval_id
          )
    )
BEGIN
    SELECT RAISE(ABORT, 'imported module binding requires exact approval');
END;

CREATE TABLE prompt_preset_modules (
    prompt_preset_revision_id TEXT NOT NULL
        REFERENCES prompt_preset_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    module_id TEXT NOT NULL
        REFERENCES content_modules(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    module_revision_id TEXT NOT NULL
        REFERENCES content_module_revisions(revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    source_sha256 TEXT NOT NULL CHECK (
        length(source_sha256) = 64
        AND source_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    config_json TEXT NOT NULL CHECK (
        json_valid(config_json)
        AND json_type(config_json) = 'object'
        AND length(CAST(config_json AS BLOB)) <= 262144
    ),
    PRIMARY KEY (prompt_preset_revision_id, ordinal),
    UNIQUE (prompt_preset_revision_id, module_id),
    FOREIGN KEY (module_id, module_revision_id)
        REFERENCES content_module_revisions(module_id, revision_id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT
);

CREATE TABLE module_activation_plans (
    id TEXT PRIMARY KEY NOT NULL CHECK (length(trim(id)) > 0),
    scope_kind TEXT NOT NULL CHECK (
        scope_kind IN (
            'app',
            'user',
            'persona',
            'character',
            'conversation',
            'branch'
        )
    ),
    expected_bindings_revision_sha256 TEXT NOT NULL CHECK (
        length(expected_bindings_revision_sha256) = 64
        AND expected_bindings_revision_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    input_module_revisions_json TEXT NOT NULL CHECK (
        json_valid(input_module_revisions_json)
        AND json_type(input_module_revisions_json) = 'array'
        AND length(CAST(input_module_revisions_json AS BLOB)) <= 1048576
    ),
    conflicts_json TEXT NOT NULL CHECK (
        json_valid(conflicts_json)
        AND json_type(conflicts_json) = 'array'
        AND length(CAST(conflicts_json AS BLOB)) <= 4194304
    ),
    resolutions_json TEXT NOT NULL CHECK (
        json_valid(resolutions_json)
        AND json_type(resolutions_json) = 'array'
        AND length(CAST(resolutions_json AS BLOB)) <= 4194304
    ),
    merge_sha256 TEXT NOT NULL CHECK (
        length(merge_sha256) = 64
        AND merge_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    plan_sha256 TEXT NOT NULL UNIQUE CHECK (
        length(plan_sha256) = 64
        AND plan_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    activation_binding_id TEXT NOT NULL CHECK (
        length(trim(activation_binding_id)) > 0
    ),
    review_json TEXT NOT NULL CHECK (
        json_valid(review_json)
        AND json_type(review_json) = 'object'
        AND length(CAST(review_json AS BLOB)) <= 4194304
    ),
    approved_plan_json TEXT NOT NULL CHECK (
        json_valid(approved_plan_json)
        AND json_type(approved_plan_json) = 'object'
        AND length(CAST(approved_plan_json AS BLOB)) <= 4194304
    ),
    approval_id TEXT NOT NULL UNIQUE CHECK (
        length(trim(approval_id)) > 0
    ),
    approval_sha256 TEXT NOT NULL UNIQUE CHECK (
        length(approval_sha256) = 64
        AND approval_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    state TEXT NOT NULL CHECK (
        state IN (
            'prepared',
            'approved',
            'applied',
            'stale',
            'discarded'
        )
    ),
    revision INTEGER NOT NULL CHECK (revision >= 1),
    prepared_at TEXT NOT NULL CHECK (length(trim(prepared_at)) > 0),
    approved_at TEXT,
    applied_at TEXT,
    CHECK (
        (state = 'prepared'
            AND approved_at IS NULL
            AND applied_at IS NULL)
        OR (state = 'approved'
            AND approved_at IS NOT NULL
            AND applied_at IS NULL)
        OR (state = 'applied'
            AND approved_at IS NOT NULL
            AND applied_at IS NOT NULL)
        OR (state = 'stale'
            AND (applied_at IS NULL OR approved_at IS NOT NULL))
        OR (state = 'discarded'
            AND applied_at IS NULL)
    )
);

CREATE TRIGGER module_activation_plans_initial_state_guard
BEFORE INSERT ON module_activation_plans
WHEN NEW.state != 'prepared' OR NEW.revision != 1
BEGIN
    SELECT RAISE(ABORT, 'module activation plan must begin prepared');
END;

CREATE TABLE module_conflict_resolutions (
    activation_plan_id TEXT NOT NULL
        REFERENCES module_activation_plans(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    component_kind TEXT NOT NULL CHECK (
        component_kind IN (
            'prompt_block',
            'control',
            'knowledge_book',
            'transform_set',
            'interaction_rule_set',
            'asset'
        )
    ),
    component_key TEXT NOT NULL CHECK (length(trim(component_key)) > 0),
    expected_candidates_json TEXT NOT NULL CHECK (
        json_valid(expected_candidates_json)
        AND json_type(expected_candidates_json) = 'array'
        AND length(CAST(expected_candidates_json AS BLOB)) <= 1048576
    ),
    selected_candidate_json TEXT CHECK (
        selected_candidate_json IS NULL
        OR (
            json_valid(selected_candidate_json)
            AND json_type(selected_candidate_json) = 'object'
            AND length(CAST(selected_candidate_json AS BLOB)) <= 262144
        )
    ),
    resolution_sha256 TEXT NOT NULL CHECK (
        length(resolution_sha256) = 64
        AND resolution_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    PRIMARY KEY (activation_plan_id, ordinal),
    UNIQUE (
        activation_plan_id,
        component_kind,
        component_key
    )
);

CREATE TABLE module_activation_audit (
    activation_plan_id TEXT NOT NULL
        REFERENCES module_activation_plans(id)
        ON UPDATE RESTRICT
        ON DELETE RESTRICT,
    sequence INTEGER NOT NULL CHECK (sequence >= 1),
    plan_revision INTEGER NOT NULL CHECK (plan_revision >= 1),
    event_kind TEXT NOT NULL CHECK (
        event_kind IN (
            'prepared',
            'approved',
            'applied',
            'stale',
            'discarded'
        )
    ),
    payload_json TEXT NOT NULL CHECK (
        json_valid(payload_json)
        AND json_type(payload_json) = 'object'
        AND length(CAST(payload_json AS BLOB)) <= 1048576
    ),
    created_at TEXT NOT NULL CHECK (length(trim(created_at)) > 0),
    PRIMARY KEY (activation_plan_id, sequence),
    UNIQUE (activation_plan_id, plan_revision)
);

CREATE TRIGGER module_activation_audit_append_guard
BEFORE INSERT ON module_activation_audit
WHEN NEW.sequence != (
    SELECT COALESCE(MAX(sequence), 0) + 1
    FROM module_activation_audit
    WHERE activation_plan_id = NEW.activation_plan_id
)
BEGIN
    SELECT RAISE(ABORT, 'module activation audit is not append-only');
END;

CREATE TRIGGER module_activation_plans_transition_guard
BEFORE UPDATE ON module_activation_plans
WHEN
    NEW.id != OLD.id
    OR NEW.scope_kind != OLD.scope_kind
    OR NEW.expected_bindings_revision_sha256
       != OLD.expected_bindings_revision_sha256
    OR NEW.input_module_revisions_json != OLD.input_module_revisions_json
    OR NEW.conflicts_json != OLD.conflicts_json
    OR NEW.resolutions_json != OLD.resolutions_json
    OR NEW.merge_sha256 != OLD.merge_sha256
    OR NEW.plan_sha256 != OLD.plan_sha256
    OR NEW.activation_binding_id != OLD.activation_binding_id
    OR NEW.review_json != OLD.review_json
    OR NEW.approved_plan_json != OLD.approved_plan_json
    OR NEW.approval_id != OLD.approval_id
    OR NEW.approval_sha256 != OLD.approval_sha256
    OR NEW.prepared_at != OLD.prepared_at
    OR (
        OLD.approved_at IS NOT NULL
        AND NEW.approved_at IS NOT OLD.approved_at
    )
    OR (
        OLD.applied_at IS NOT NULL
        AND NEW.applied_at IS NOT OLD.applied_at
    )
    OR NEW.revision != OLD.revision + 1
    OR (
        (OLD.state = 'prepared'
            AND NEW.state NOT IN ('approved', 'stale', 'discarded'))
        OR (OLD.state = 'approved'
            AND NEW.state NOT IN ('applied', 'stale', 'discarded'))
        OR (OLD.state = 'applied'
            AND NEW.state != 'stale')
        OR OLD.state IN ('stale', 'discarded')
    )
BEGIN
    SELECT RAISE(ABORT, 'module activation plan transition is invalid');
END;

-- Immutable revision children and execution audit records.
CREATE TRIGGER interaction_rule_set_revisions_no_update
BEFORE UPDATE ON interaction_rule_set_revisions
BEGIN
    SELECT RAISE(ABORT, 'interaction rule set revisions are immutable');
END;
CREATE TRIGGER interaction_rule_set_revisions_no_delete
BEFORE DELETE ON interaction_rule_set_revisions
BEGIN
    SELECT RAISE(ABORT, 'interaction rule set revisions are immutable');
END;
CREATE TRIGGER interaction_rules_no_update
BEFORE UPDATE ON interaction_rules
BEGIN
    SELECT RAISE(ABORT, 'interaction rules are immutable');
END;
CREATE TRIGGER interaction_rules_no_delete
BEFORE DELETE ON interaction_rules
BEGIN
    SELECT RAISE(ABORT, 'interaction rules are immutable');
END;
CREATE TRIGGER interaction_actions_no_update
BEFORE UPDATE ON interaction_actions
BEGIN
    SELECT RAISE(ABORT, 'interaction actions are immutable');
END;
CREATE TRIGGER interaction_actions_no_delete
BEFORE DELETE ON interaction_actions
BEGIN
    SELECT RAISE(ABORT, 'interaction actions are immutable');
END;
CREATE TRIGGER interaction_events_no_update
BEFORE UPDATE ON interaction_events
BEGIN
    SELECT RAISE(ABORT, 'interaction events are immutable');
END;
CREATE TRIGGER interaction_events_no_delete
BEFORE DELETE ON interaction_events
BEGIN
    SELECT RAISE(ABORT, 'interaction events are immutable');
END;
CREATE TRIGGER interaction_action_results_no_update
BEFORE UPDATE ON interaction_action_results
BEGIN
    SELECT RAISE(ABORT, 'interaction action results are immutable');
END;
CREATE TRIGGER interaction_action_results_no_delete
BEFORE DELETE ON interaction_action_results
BEGIN
    SELECT RAISE(ABORT, 'interaction action results are immutable');
END;
CREATE TRIGGER interaction_proposals_no_delete
BEFORE DELETE ON interaction_proposals
BEGIN
    SELECT RAISE(ABORT, 'interaction proposals are durable');
END;
CREATE TRIGGER interaction_proposal_audit_no_update
BEFORE UPDATE ON interaction_proposal_audit
BEGIN
    SELECT RAISE(ABORT, 'interaction proposal audit is immutable');
END;
CREATE TRIGGER interaction_proposal_audit_no_delete
BEFORE DELETE ON interaction_proposal_audit
BEGIN
    SELECT RAISE(ABORT, 'interaction proposal audit is immutable');
END;
CREATE TRIGGER content_module_revisions_no_update
BEFORE UPDATE ON content_module_revisions
BEGIN
    SELECT RAISE(ABORT, 'content module revisions are immutable');
END;
CREATE TRIGGER content_module_revisions_no_delete
BEFORE DELETE ON content_module_revisions
BEGIN
    SELECT RAISE(ABORT, 'content module revisions are immutable');
END;
CREATE TRIGGER content_module_variables_no_update
BEFORE UPDATE ON content_module_variables
BEGIN
    SELECT RAISE(ABORT, 'content module variables are immutable');
END;
CREATE TRIGGER content_module_variables_no_delete
BEFORE DELETE ON content_module_variables
BEGIN
    SELECT RAISE(ABORT, 'content module variables are immutable');
END;
CREATE TRIGGER content_module_controls_no_update
BEFORE UPDATE ON content_module_controls
BEGIN
    SELECT RAISE(ABORT, 'content module controls are immutable');
END;
CREATE TRIGGER content_module_controls_no_delete
BEFORE DELETE ON content_module_controls
BEGIN
    SELECT RAISE(ABORT, 'content module controls are immutable');
END;
CREATE TRIGGER content_module_prompt_blocks_no_update
BEFORE UPDATE ON content_module_prompt_blocks
BEGIN
    SELECT RAISE(ABORT, 'content module prompt blocks are immutable');
END;
CREATE TRIGGER content_module_prompt_blocks_no_delete
BEFORE DELETE ON content_module_prompt_blocks
BEGIN
    SELECT RAISE(ABORT, 'content module prompt blocks are immutable');
END;
CREATE TRIGGER content_module_components_no_update
BEFORE UPDATE ON content_module_components
BEGIN
    SELECT RAISE(ABORT, 'content module components are immutable');
END;
CREATE TRIGGER content_module_components_no_delete
BEFORE DELETE ON content_module_components
BEGIN
    SELECT RAISE(ABORT, 'content module components are immutable');
END;
CREATE TRIGGER module_conflict_resolutions_no_update
BEFORE UPDATE ON module_conflict_resolutions
BEGIN
    SELECT RAISE(ABORT, 'module conflict resolutions are immutable');
END;
CREATE TRIGGER module_conflict_resolutions_no_delete
BEFORE DELETE ON module_conflict_resolutions
BEGIN
    SELECT RAISE(ABORT, 'module conflict resolutions are immutable');
END;
CREATE TRIGGER module_activation_audit_no_update
BEFORE UPDATE ON module_activation_audit
BEGIN
    SELECT RAISE(ABORT, 'module activation audit is immutable');
END;
CREATE TRIGGER module_activation_audit_no_delete
BEFORE DELETE ON module_activation_audit
BEGIN
    SELECT RAISE(ABORT, 'module activation audit is immutable');
END;
