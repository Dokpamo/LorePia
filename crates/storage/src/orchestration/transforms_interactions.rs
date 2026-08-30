//! Transform-set and interaction-rule-set document stores and projections.

use super::{
    CoreError, CoreResult, DocumentTable, InteractionRuleSet, InteractionRuleSetId,
    KnowledgeEntryId, RevisionEventKind, SourceKind, Storage, StoredRevision, Transaction,
    TransformSet, TransformSetId, Utc, ValidateOrchestration, content_revision_no, encode_document,
    enum_wire, get_document, i64_revision, list_documents, not_found, params, save_content_object,
    soft_delete_content_object, source_kind_str, storage_db_error,
};

impl Storage {
    pub fn get_transform_set(
        &self,
        id: &TransformSetId,
    ) -> CoreResult<StoredRevision<TransformSet>> {
        get_document(self, DocumentTable::TransformSets, id.as_str(), false)
    }

    pub fn list_transform_sets(&self) -> CoreResult<Vec<StoredRevision<TransformSet>>> {
        list_documents(self, DocumentTable::TransformSets)
    }

    pub fn save_transform_set(
        &self,
        transform_set: &TransformSet,
        expected_revision: Option<u64>,
    ) -> CoreResult<StoredRevision<TransformSet>> {
        save_content_object(
            self,
            DocumentTable::TransformSets,
            transform_set.id.as_str(),
            transform_set.schema_version,
            transform_set,
            &transform_set.provenance,
            expected_revision,
            if expected_revision.is_some() {
                RevisionEventKind::Update
            } else {
                RevisionEventKind::Create
            },
            |transaction, revision_id, document_json| {
                write_transform_set_projection(
                    transaction,
                    revision_id,
                    transform_set,
                    document_json,
                    expected_revision,
                )
            },
            false,
        )
    }

    pub fn soft_delete_transform_set(
        &self,
        id: &TransformSetId,
        expected_revision: u64,
    ) -> CoreResult<StoredRevision<TransformSet>> {
        let current = self.get_transform_set(id)?;
        soft_delete_content_object(
            self,
            DocumentTable::TransformSets,
            id.as_str(),
            expected_revision,
            |transaction, revision_id, document_json| {
                write_transform_set_projection(
                    transaction,
                    revision_id,
                    &current.value,
                    document_json,
                    Some(expected_revision),
                )
            },
        )
    }

    pub fn get_interaction_rule_set(
        &self,
        id: &InteractionRuleSetId,
    ) -> CoreResult<StoredRevision<InteractionRuleSet>> {
        get_document(self, DocumentTable::InteractionRuleSets, id.as_str(), false)
    }

    pub fn list_interaction_rule_sets(
        &self,
    ) -> CoreResult<Vec<StoredRevision<InteractionRuleSet>>> {
        list_documents(self, DocumentTable::InteractionRuleSets)
    }

    pub fn save_interaction_rule_set(
        &self,
        rule_set: &InteractionRuleSet,
        expected_revision: Option<u64>,
    ) -> CoreResult<StoredRevision<InteractionRuleSet>> {
        save_content_object(
            self,
            DocumentTable::InteractionRuleSets,
            rule_set.id.as_str(),
            rule_set.schema_version,
            rule_set,
            &rule_set.provenance,
            expected_revision,
            if expected_revision.is_some() {
                RevisionEventKind::Update
            } else {
                RevisionEventKind::Create
            },
            |transaction, revision_id, document_json| {
                write_interaction_rule_set_projection(
                    transaction,
                    revision_id,
                    rule_set,
                    document_json,
                    expected_revision,
                )
            },
            false,
        )
    }

    pub fn soft_delete_interaction_rule_set(
        &self,
        id: &InteractionRuleSetId,
        expected_revision: u64,
    ) -> CoreResult<StoredRevision<InteractionRuleSet>> {
        let current = self.get_interaction_rule_set(id)?;
        soft_delete_content_object(
            self,
            DocumentTable::InteractionRuleSets,
            id.as_str(),
            expected_revision,
            |transaction, revision_id, document_json| {
                write_interaction_rule_set_projection(
                    transaction,
                    revision_id,
                    &current.value,
                    document_json,
                    Some(expected_revision),
                )
            },
        )
    }
}

fn validate_transform_set_projection(transform_set: &TransformSet) -> CoreResult<()> {
    transform_set
        .validate()
        .map_err(|error| CoreError::invalid(error.to_string()))?;
    let imported = matches!(
        transform_set.provenance.source_kind,
        SourceKind::ImportedStandard | SourceKind::ImportedPackage
    );
    if imported
        && (transform_set.enabled
            || transform_set
                .rules
                .iter()
                .any(|rule| rule.enabled || rule.imported_enabled))
    {
        return Err(CoreError::invalid(
            "imported transform sets and rules must remain disabled until reviewed",
        ));
    }
    Ok(())
}

struct TransformProjectionMetadata<'a> {
    revision_id: &'a str,
    document_json: &'a str,
    revision_no: u64,
    state_version: u64,
    source_kind: &'a str,
    provenance_json: &'a str,
    now: &'a str,
    expected_revision: Option<u64>,
}

fn write_transform_set_projection_header(
    transaction: &Transaction<'_>,
    transform_set: &TransformSet,
    metadata: &TransformProjectionMetadata<'_>,
) -> CoreResult<()> {
    transaction
        .execute(
            "INSERT INTO transform_sets
             (id, name, schema_version, revision, enabled, max_rules_per_phase,
              max_output_chars, document_json, provenance_json, source_kind,
              source_hash, created_at, updated_at, deleted_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                     ?12, NULL)
             ON CONFLICT(id) DO UPDATE SET
                 name = excluded.name,
                 schema_version = excluded.schema_version,
                 revision = excluded.revision,
                 enabled = excluded.enabled,
                 max_rules_per_phase = excluded.max_rules_per_phase,
                 max_output_chars = excluded.max_output_chars,
                 document_json = excluded.document_json,
                 provenance_json = excluded.provenance_json,
                 source_kind = excluded.source_kind,
                 source_hash = excluded.source_hash,
                 updated_at = excluded.updated_at
             WHERE transform_sets.revision = ?13
               AND transform_sets.deleted_at IS NULL",
            params![
                transform_set.id.as_str(),
                transform_set.name,
                transform_set.schema_version,
                i64_revision(metadata.state_version)?,
                transform_set.enabled,
                transform_set.max_rules_per_phase,
                transform_set.max_output_chars,
                metadata.document_json,
                metadata.provenance_json,
                metadata.source_kind,
                transform_set.provenance.source_hash,
                metadata.now,
                metadata
                    .expected_revision
                    .map(i64_revision)
                    .transpose()?
                    .unwrap_or(0),
            ],
        )
        .map_err(storage_db_error)?;
    transaction
        .execute(
            "INSERT INTO transform_set_revisions
             (revision_id, transform_set_id, revision_no, name, enabled,
              max_rules_per_phase, max_output_chars, source_kind, document_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                metadata.revision_id,
                transform_set.id.as_str(),
                i64_revision(metadata.revision_no)?,
                transform_set.name,
                transform_set.enabled,
                transform_set.max_rules_per_phase,
                transform_set.max_output_chars,
                metadata.source_kind,
                metadata.document_json,
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn write_transform_rules_projection(
    transaction: &Transaction<'_>,
    revision_id: &str,
    transform_set: &TransformSet,
) -> CoreResult<()> {
    for (ordinal, rule) in transform_set.rules.iter().enumerate() {
        let condition_json = rule
            .condition
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| {
                CoreError::invalid(format!("cannot encode transform condition: {error}"))
            })?;
        let (rule_provenance_json, _) =
            encode_document("transform rule provenance", &rule.provenance)?;
        let (rule_json, _) = encode_document("transform rule", rule)?;
        transaction
            .execute(
                "INSERT INTO transform_rules
                 (set_revision_id, rule_id, ordinal, name, enabled,
                  imported_enabled, phase, engine, pattern, case_insensitive,
                  replacement, condition_json, max_replacements, input_limit,
                  output_limit, max_applications, provenance_json, document_json)
                 VALUES (
                     ?1, ?2, ?3, ?4, ?5, ?6, ?7, 'rust_regex_v1', ?8, ?9,
                     ?10, ?11, ?12, ?13, ?14, 1, ?15, ?16
                 )",
                params![
                    revision_id,
                    rule.id.as_str(),
                    i64::try_from(ordinal)
                        .map_err(|_| CoreError::invalid("too many transform rules"))?,
                    rule.name,
                    rule.enabled,
                    rule.imported_enabled,
                    enum_wire(&rule.phase)?,
                    rule.pattern.pattern,
                    rule.pattern.case_insensitive,
                    rule.replacement,
                    condition_json,
                    rule.max_replacements,
                    rule.input_limit,
                    rule.output_limit,
                    rule_provenance_json,
                    rule_json,
                ],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

pub(super) fn write_transform_set_projection(
    transaction: &Transaction<'_>,
    revision_id: &str,
    transform_set: &TransformSet,
    document_json: &str,
    expected_revision: Option<u64>,
) -> CoreResult<()> {
    validate_transform_set_projection(transform_set)?;
    let source_kind = source_kind_str(&transform_set.provenance.source_kind);
    let revision_no = content_revision_no(transaction, revision_id)?;
    let state_version = expected_revision.map_or(1, |value| value.saturating_add(1));
    let (provenance_json, _) =
        encode_document("transform set provenance", &transform_set.provenance)?;
    let now = Utc::now().to_rfc3339();
    let metadata = TransformProjectionMetadata {
        revision_id,
        document_json,
        revision_no,
        state_version,
        source_kind,
        provenance_json: &provenance_json,
        now: &now,
        expected_revision,
    };
    write_transform_set_projection_header(transaction, transform_set, &metadata)?;
    write_transform_rules_projection(transaction, revision_id, transform_set)
}

fn interaction_event_kind(event: &lorepia_domain::InteractionEvent) -> &'static str {
    match event {
        lorepia_domain::InteractionEvent::ConversationOpened => "conversation_opened",
        lorepia_domain::InteractionEvent::ConversationStarted => "conversation_started",
        lorepia_domain::InteractionEvent::BeforeGeneration => "before_generation",
        lorepia_domain::InteractionEvent::AfterGeneration => "after_generation",
        lorepia_domain::InteractionEvent::MessageCommitted => "message_committed",
        lorepia_domain::InteractionEvent::UserAction { .. } => "user_action",
        lorepia_domain::InteractionEvent::VariableChanged { .. } => "variable_changed",
        lorepia_domain::InteractionEvent::KnowledgeActivated { .. } => "knowledge_activated",
    }
}

fn interaction_action_kind(action: &lorepia_domain::InteractionAction) -> &'static str {
    match action {
        lorepia_domain::InteractionAction::SetVariable { .. } => "set_variable",
        lorepia_domain::InteractionAction::IncrementVariable { .. } => "increment_variable",
        lorepia_domain::InteractionAction::ActivateKnowledge { .. } => "activate_knowledge",
        lorepia_domain::InteractionAction::ShowAsset { .. } => "show_asset",
        lorepia_domain::InteractionAction::PlayAudio { .. } => "play_audio",
        lorepia_domain::InteractionAction::PresentChoices { .. } => "present_choices",
        lorepia_domain::InteractionAction::AppendVisibleSystemEvent { .. } => {
            "append_visible_system_event"
        }
        lorepia_domain::InteractionAction::RollDice { .. } => "roll_dice",
        lorepia_domain::InteractionAction::RequestUserApproval { .. } => "request_user_approval",
    }
}

fn active_knowledge_entry_revision(
    transaction: &Transaction<'_>,
    entry_id: &KnowledgeEntryId,
) -> CoreResult<String> {
    let mut statement = transaction
        .prepare(
            "SELECT entry.book_revision_id
             FROM knowledge_entries AS entry
             JOIN content_object_state AS state
               ON state.active_revision_id = entry.book_revision_id
             JOIN content_objects AS object
               ON object.id = state.object_id
              AND object.object_kind = 'knowledge_book'
              AND object.deleted_at IS NULL
             WHERE entry.entry_id = ?1
             ORDER BY entry.book_revision_id",
        )
        .map_err(storage_db_error)?;
    let revisions = statement
        .query_map([entry_id.as_str()], |row| row.get::<_, String>(0))
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    match revisions.as_slice() {
        [revision] => Ok(revision.clone()),
        [] => Err(not_found("interaction knowledge entry")),
        _ => Err(CoreError::invalid(
            "interaction knowledge entry id is ambiguous across active books",
        )),
    }
}

struct InteractionProjectionMetadata<'a> {
    revision_id: &'a str,
    document_json: &'a str,
    revision_no: u64,
    state_version: u64,
    source_kind: &'a str,
    provenance_json: &'a str,
    now: &'a str,
    expected_revision: Option<u64>,
}

fn write_interaction_rule_set_projection_header(
    transaction: &Transaction<'_>,
    rule_set: &InteractionRuleSet,
    metadata: &InteractionProjectionMetadata<'_>,
) -> CoreResult<()> {
    transaction
        .execute(
            "INSERT INTO interaction_rule_sets
             (id, name, schema_version, revision, max_actions_per_event,
              document_json, provenance_json, source_kind, source_hash,
              created_at, updated_at, deleted_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, NULL)
             ON CONFLICT(id) DO UPDATE SET
                 name = excluded.name,
                 schema_version = excluded.schema_version,
                 revision = excluded.revision,
                 max_actions_per_event = excluded.max_actions_per_event,
                 document_json = excluded.document_json,
                 provenance_json = excluded.provenance_json,
                 source_kind = excluded.source_kind,
                 source_hash = excluded.source_hash,
                 updated_at = excluded.updated_at
             WHERE interaction_rule_sets.revision = ?11
               AND interaction_rule_sets.deleted_at IS NULL",
            params![
                rule_set.id.as_str(),
                rule_set.name,
                rule_set.schema_version,
                i64_revision(metadata.state_version)?,
                rule_set.max_actions_per_event,
                metadata.document_json,
                metadata.provenance_json,
                metadata.source_kind,
                rule_set.provenance.source_hash,
                metadata.now,
                metadata
                    .expected_revision
                    .map(i64_revision)
                    .transpose()?
                    .unwrap_or(0),
            ],
        )
        .map_err(storage_db_error)?;
    transaction
        .execute(
            "INSERT INTO interaction_rule_set_revisions
             (revision_id, interaction_rule_set_id, revision_no, name,
              max_actions_per_event, source_kind, document_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                metadata.revision_id,
                rule_set.id.as_str(),
                i64_revision(metadata.revision_no)?,
                rule_set.name,
                rule_set.max_actions_per_event,
                metadata.source_kind,
                metadata.document_json,
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn write_interaction_action_projection(
    transaction: &Transaction<'_>,
    revision_id: &str,
    rule_id: &str,
    action_ordinal: usize,
    action: &lorepia_domain::InteractionAction,
) -> CoreResult<()> {
    let (knowledge_revision, knowledge_entry, asset_descriptor) = match action {
        lorepia_domain::InteractionAction::ActivateKnowledge { entry_id } => (
            Some(active_knowledge_entry_revision(transaction, entry_id)?),
            Some(entry_id.as_str()),
            None,
        ),
        lorepia_domain::InteractionAction::ShowAsset { asset_id, .. }
        | lorepia_domain::InteractionAction::PlayAudio { asset_id } => {
            (None, None, Some(asset_id.as_str()))
        }
        _ => (None, None, None),
    };
    let payload_json = serde_json::to_string(action).map_err(|error| {
        CoreError::invalid(format!("cannot encode interaction action: {error}"))
    })?;
    transaction
        .execute(
            "INSERT INTO interaction_actions
             (set_revision_id, rule_id, ordinal, action_kind,
              payload_json, knowledge_book_revision_id,
              knowledge_entry_id, asset_descriptor_id,
              requires_approval)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                revision_id,
                rule_id,
                i64::try_from(action_ordinal)
                    .map_err(|_| CoreError::invalid("too many interaction actions"))?,
                interaction_action_kind(action),
                payload_json,
                knowledge_revision,
                knowledge_entry,
                asset_descriptor,
                matches!(
                    action,
                    lorepia_domain::InteractionAction::RequestUserApproval { .. }
                ),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn write_interaction_rules_projection(
    transaction: &Transaction<'_>,
    revision_id: &str,
    rule_set: &InteractionRuleSet,
) -> CoreResult<()> {
    for (rule_ordinal, rule) in rule_set.rules.iter().enumerate() {
        let event_argument_json = match rule.event {
            lorepia_domain::InteractionEvent::UserAction { .. }
            | lorepia_domain::InteractionEvent::VariableChanged { .. }
            | lorepia_domain::InteractionEvent::KnowledgeActivated { .. } => {
                Some(serde_json::to_string(&rule.event).map_err(|error| {
                    CoreError::invalid(format!("cannot encode interaction event: {error}"))
                })?)
            }
            _ => None,
        };
        let condition_json = rule
            .condition
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| {
                CoreError::invalid(format!("cannot encode interaction condition: {error}"))
            })?;
        let (rule_provenance_json, _) =
            encode_document("interaction rule provenance", &rule.provenance)?;
        let (rule_json, _) = encode_document("interaction rule", rule)?;
        transaction
            .execute(
                "INSERT INTO interaction_rules
                 (set_revision_id, rule_id, ordinal, name, enabled, event_kind,
                  event_argument_json, condition_json, priority,
                  stop_after_match, provenance_json, document_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    revision_id,
                    rule.id.as_str(),
                    i64::try_from(rule_ordinal)
                        .map_err(|_| CoreError::invalid("too many interaction rules"))?,
                    rule.name,
                    rule.enabled,
                    interaction_event_kind(&rule.event),
                    event_argument_json,
                    condition_json,
                    rule.priority,
                    rule.stop_after_match,
                    rule_provenance_json,
                    rule_json,
                ],
            )
            .map_err(storage_db_error)?;
        for (action_ordinal, action) in rule.actions.iter().enumerate() {
            write_interaction_action_projection(
                transaction,
                revision_id,
                rule.id.as_str(),
                action_ordinal,
                action,
            )?;
        }
    }
    Ok(())
}

pub(super) fn write_interaction_rule_set_projection(
    transaction: &Transaction<'_>,
    revision_id: &str,
    rule_set: &InteractionRuleSet,
    document_json: &str,
    expected_revision: Option<u64>,
) -> CoreResult<()> {
    rule_set
        .validate()
        .map_err(|error| CoreError::invalid(error.to_string()))?;
    let revision_no = content_revision_no(transaction, revision_id)?;
    let state_version = expected_revision.map_or(1, |value| value.saturating_add(1));
    let source_kind = source_kind_str(&rule_set.provenance.source_kind);
    let (provenance_json, _) =
        encode_document("interaction rule set provenance", &rule_set.provenance)?;
    let now = Utc::now().to_rfc3339();
    let metadata = InteractionProjectionMetadata {
        revision_id,
        document_json,
        revision_no,
        state_version,
        source_kind,
        provenance_json: &provenance_json,
        now: &now,
        expected_revision,
    };
    write_interaction_rule_set_projection_header(transaction, rule_set, &metadata)?;
    write_interaction_rules_projection(transaction, revision_id, rule_set)
}
