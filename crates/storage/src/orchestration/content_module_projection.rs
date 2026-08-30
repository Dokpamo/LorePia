//! Content-module document store and ordered component projection.

use super::{
    BTreeSet, ContentModule, ContentModuleId, ControlSpec, CoreError, CoreResult, DocumentTable,
    OptionalExtension, RevisionEventKind, Storage, StoredRevision, Transaction, Utc,
    ValidateOrchestration, active_content_revision_id, content_revision_no, encode_document,
    enum_wire, get_document, i64_revision, list_documents, not_found, params, save_content_object,
    sha256_hex, soft_delete_content_object, source_kind_str, storage_db_error,
    validate_optional_sha256,
};

impl Storage {
    pub fn get_content_module(
        &self,
        id: &ContentModuleId,
    ) -> CoreResult<StoredRevision<ContentModule>> {
        get_document(self, DocumentTable::ContentModules, id.as_str(), false)
    }

    pub fn list_content_modules(&self) -> CoreResult<Vec<StoredRevision<ContentModule>>> {
        list_documents(self, DocumentTable::ContentModules)
    }

    pub fn save_content_module(
        &self,
        module: &ContentModule,
        expected_revision: Option<u64>,
    ) -> CoreResult<StoredRevision<ContentModule>> {
        save_content_object(
            self,
            DocumentTable::ContentModules,
            module.id.as_str(),
            module.schema_version,
            module,
            &module.metadata.provenance,
            expected_revision,
            if expected_revision.is_some() {
                RevisionEventKind::Update
            } else {
                RevisionEventKind::Create
            },
            |transaction, revision_id, document_json| {
                write_content_module_projection(
                    transaction,
                    revision_id,
                    module,
                    document_json,
                    expected_revision,
                )
            },
            false,
        )
    }

    pub fn soft_delete_content_module(
        &self,
        id: &ContentModuleId,
        expected_revision: u64,
    ) -> CoreResult<StoredRevision<ContentModule>> {
        let current = self.get_content_module(id)?;
        soft_delete_content_object(
            self,
            DocumentTable::ContentModules,
            id.as_str(),
            expected_revision,
            |transaction, revision_id, document_json| {
                write_content_module_projection(
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

fn variable_type_for_control(control: &ControlSpec) -> CoreResult<lorepia_domain::VariableType> {
    if let Some(value_type) = control.value_type {
        return Ok(value_type);
    }
    match control.default_value {
        Some(lorepia_domain::VariableValue::Bool(_)) => Ok(lorepia_domain::VariableType::Bool),
        Some(lorepia_domain::VariableValue::Integer(_)) => {
            Ok(lorepia_domain::VariableType::Integer)
        }
        Some(lorepia_domain::VariableValue::Decimal(_)) => {
            Ok(lorepia_domain::VariableType::Decimal)
        }
        Some(lorepia_domain::VariableValue::Text(_)) => Ok(lorepia_domain::VariableType::Text),
        Some(lorepia_domain::VariableValue::Enum(_)) => Ok(lorepia_domain::VariableType::Enum),
        Some(lorepia_domain::VariableValue::StringList(_)) => {
            Ok(lorepia_domain::VariableType::StringList)
        }
        None => Err(CoreError::invalid(
            "module control variable requires a value type",
        )),
    }
}

struct ContentModuleProjectionMetadata<'a> {
    revision_id: &'a str,
    document_json: &'a str,
    revision_no: u64,
    state_version: u64,
    source_kind: &'a str,
    source_hash: &'a str,
    metadata_json: &'a str,
    now: &'a str,
    previous_revision_id: Option<&'a str>,
    expected_revision: Option<u64>,
}

fn write_content_module_projection_header(
    transaction: &Transaction<'_>,
    module: &ContentModule,
    metadata: &ContentModuleProjectionMetadata<'_>,
) -> CoreResult<()> {
    transaction
        .execute(
            "INSERT INTO content_modules
             (id, name, version, schema_version, revision, document_json,
              metadata_json, source_kind, source_hash, created_at, updated_at,
              deleted_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, NULL)
             ON CONFLICT(id) DO UPDATE SET
                 name = excluded.name,
                 version = excluded.version,
                 schema_version = excluded.schema_version,
                 revision = excluded.revision,
                 document_json = excluded.document_json,
                 metadata_json = excluded.metadata_json,
                 source_kind = excluded.source_kind,
                 source_hash = excluded.source_hash,
                 updated_at = excluded.updated_at
             WHERE content_modules.revision = ?11
               AND content_modules.deleted_at IS NULL",
            params![
                module.id.as_str(),
                module.name,
                module.version,
                module.schema_version,
                i64_revision(metadata.state_version)?,
                metadata.document_json,
                metadata.metadata_json,
                metadata.source_kind,
                metadata.source_hash,
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
            "INSERT INTO content_module_revisions
             (revision_id, module_id, revision_no, version,
              previous_revision_id, source_kind, source_hash, metadata_json,
              document_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                metadata.revision_id,
                module.id.as_str(),
                i64_revision(metadata.revision_no)?,
                module.version,
                metadata.previous_revision_id,
                metadata.source_kind,
                metadata.source_hash,
                metadata.metadata_json,
                metadata.document_json,
                metadata.now,
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn write_content_module_variables(
    transaction: &Transaction<'_>,
    revision_id: &str,
    module: &ContentModule,
) -> CoreResult<()> {
    let mut inserted_variables = BTreeSet::new();
    for control in &module.control_specs {
        let Some(variable) = control.variable.as_ref() else {
            continue;
        };
        if !inserted_variables.insert(variable.id.as_str().to_owned()) {
            continue;
        }
        let value_type = variable_type_for_control(control)?;
        let default_value_json =
            serde_json::to_string(&control.default_value).map_err(|error| {
                CoreError::invalid(format!("cannot encode module variable default: {error}"))
            })?;
        let variable_json = serde_json::to_string(&serde_json::json!({
            "variable": variable,
            "value_type": value_type,
            "default_value": control.default_value,
            "sensitive": control.sensitive,
        }))
        .map_err(|error| CoreError::invalid(format!("cannot encode module variable: {error}")))?;
        transaction
            .execute(
                "INSERT INTO content_module_variables
                 (module_revision_id, variable_id, value_type,
                  default_value_json, sensitive, document_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    revision_id,
                    variable.id.as_str(),
                    enum_wire(&value_type)?,
                    default_value_json,
                    control.sensitive,
                    variable_json,
                ],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

fn write_content_module_controls(
    transaction: &Transaction<'_>,
    revision_id: &str,
    module: &ContentModule,
    mut component_ordinal: i64,
) -> CoreResult<i64> {
    for (ordinal, control) in module.control_specs.iter().enumerate() {
        let (control_json, control_hash) = encode_document("module control", control)?;
        transaction
            .execute(
                "INSERT INTO content_module_controls
                 (module_revision_id, control_id, ordinal, kind, variable_id,
                  label, document_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    revision_id,
                    control.id.as_str(),
                    i64::try_from(ordinal)
                        .map_err(|_| CoreError::invalid("too many module controls"))?,
                    enum_wire(&control.kind)?,
                    control.variable.as_ref().map(|value| value.id.as_str()),
                    control.label,
                    control_json,
                ],
            )
            .map_err(storage_db_error)?;
        transaction
            .execute(
                "INSERT INTO content_module_components
                 (module_revision_id, ordinal, component_kind,
                  prompt_block_id, control_id, knowledge_book_revision_id,
                  memory_profile_revision_id, transform_set_revision_id,
                  interaction_rule_set_revision_id, asset_descriptor_id,
                  merge_policy, component_sha256, config_json)
                 VALUES (
                     ?1, ?2, 'control', NULL, ?3, NULL, NULL, NULL, NULL,
                     NULL, 'append', ?4, '{}'
                 )",
                params![
                    revision_id,
                    component_ordinal,
                    control.id.as_str(),
                    control_hash
                ],
            )
            .map_err(storage_db_error)?;
        component_ordinal += 1;
    }
    Ok(component_ordinal)
}

fn write_content_module_prompt_blocks(
    transaction: &Transaction<'_>,
    revision_id: &str,
    module: &ContentModule,
    mut component_ordinal: i64,
) -> CoreResult<i64> {
    for (ordinal, block) in module.prompt_fragments.iter().enumerate() {
        let (block_json, block_hash) = encode_document("module prompt block", block)?;
        transaction
            .execute(
                "INSERT INTO content_module_prompt_blocks
                 (module_revision_id, block_id, ordinal, name, kind, enabled,
                  authority, role_hint, placement_zone, document_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    revision_id,
                    block.id.as_str(),
                    i64::try_from(ordinal)
                        .map_err(|_| CoreError::invalid("too many module prompt blocks"))?,
                    block.name,
                    enum_wire(&block.kind)?,
                    block.enabled,
                    enum_wire(&block.authority)?,
                    enum_wire(&block.role_hint)?,
                    enum_wire(&block.placement_zone)?,
                    block_json,
                ],
            )
            .map_err(storage_db_error)?;
        transaction
            .execute(
                "INSERT INTO content_module_components
                 (module_revision_id, ordinal, component_kind,
                  prompt_block_id, control_id, knowledge_book_revision_id,
                  memory_profile_revision_id, transform_set_revision_id,
                  interaction_rule_set_revision_id, asset_descriptor_id,
                  merge_policy, component_sha256, config_json)
                 VALUES (
                     ?1, ?2, 'prompt_block', ?3, NULL, NULL, NULL, NULL, NULL,
                     NULL, 'append', ?4, '{}'
                 )",
                params![
                    revision_id,
                    component_ordinal,
                    block.id.as_str(),
                    block_hash
                ],
            )
            .map_err(storage_db_error)?;
        component_ordinal += 1;
    }
    Ok(component_ordinal)
}

fn write_content_module_linked_components(
    transaction: &Transaction<'_>,
    revision_id: &str,
    module: &ContentModule,
    mut component_ordinal: i64,
) -> CoreResult<i64> {
    for book_id in &module.knowledge_book_ids {
        let target_revision =
            active_content_revision_id(transaction, book_id.as_str(), "knowledge_book")?;
        let component_hash = active_content_revision_sha256(transaction, &target_revision)?;
        insert_linked_module_component(
            transaction,
            revision_id,
            component_ordinal,
            "knowledge_book",
            &target_revision,
            &component_hash,
        )?;
        component_ordinal += 1;
    }
    for transform_id in &module.transform_set_ids {
        let target_revision =
            active_content_revision_id(transaction, transform_id.as_str(), "transform_set")?;
        let component_hash = active_content_revision_sha256(transaction, &target_revision)?;
        insert_linked_module_component(
            transaction,
            revision_id,
            component_ordinal,
            "transform_set",
            &target_revision,
            &component_hash,
        )?;
        component_ordinal += 1;
    }
    for interaction_id in &module.interaction_rule_set_ids {
        let target_revision = active_content_revision_id(
            transaction,
            interaction_id.as_str(),
            "interaction_rule_set",
        )?;
        let component_hash = active_content_revision_sha256(transaction, &target_revision)?;
        insert_linked_module_component(
            transaction,
            revision_id,
            component_ordinal,
            "interaction_rule_set",
            &target_revision,
            &component_hash,
        )?;
        component_ordinal += 1;
    }
    Ok(component_ordinal)
}

fn write_content_module_assets(
    transaction: &Transaction<'_>,
    revision_id: &str,
    module: &ContentModule,
    mut component_ordinal: i64,
) -> CoreResult<i64> {
    for asset_id in &module.asset_ids {
        let payload = transaction
            .query_row(
                "SELECT payload_json FROM asset_descriptors WHERE id = ?1",
                [asset_id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("module asset descriptor"))?;
        transaction
            .execute(
                "INSERT INTO content_module_components
                 (module_revision_id, ordinal, component_kind,
                  prompt_block_id, control_id, knowledge_book_revision_id,
                  memory_profile_revision_id, transform_set_revision_id,
                  interaction_rule_set_revision_id, asset_descriptor_id,
                  merge_policy, component_sha256, config_json)
                 VALUES (
                     ?1, ?2, 'asset', NULL, NULL, NULL, NULL, NULL, NULL,
                     ?3, 'append', ?4, '{}'
                 )",
                params![
                    revision_id,
                    component_ordinal,
                    asset_id.as_str(),
                    sha256_hex(payload.as_bytes())
                ],
            )
            .map_err(storage_db_error)?;
        component_ordinal += 1;
    }
    Ok(component_ordinal)
}

fn write_content_module_capabilities(
    transaction: &Transaction<'_>,
    revision_id: &str,
    module: &ContentModule,
) -> CoreResult<()> {
    for capability in &module.required_capabilities {
        let approval_required = matches!(
            capability,
            lorepia_domain::ContentCapability::HighRiskAssets
        );
        transaction
            .execute(
                "INSERT INTO content_module_required_capabilities
                 (module_revision_id, capability, support_status, approved, reason)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    revision_id,
                    enum_wire(capability)?,
                    if approval_required {
                        "approval_required"
                    } else {
                        "supported"
                    },
                    !approval_required,
                    if approval_required {
                        "high-risk assets require explicit local approval"
                    } else {
                        "supported by the declarative module runtime"
                    },
                ],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

pub(super) fn write_content_module_projection(
    transaction: &Transaction<'_>,
    revision_id: &str,
    module: &ContentModule,
    document_json: &str,
    expected_revision: Option<u64>,
) -> CoreResult<()> {
    module
        .validate()
        .map_err(|error| CoreError::invalid(error.to_string()))?;
    let revision_no = content_revision_no(transaction, revision_id)?;
    let state_version = expected_revision.map_or(1, |value| value.saturating_add(1));
    let source_kind = source_kind_str(&module.metadata.provenance.source_kind);
    let source_hash = module
        .metadata
        .provenance
        .source_hash
        .clone()
        .unwrap_or_else(|| sha256_hex(document_json.as_bytes()));
    validate_optional_sha256("content module source hash", Some(&source_hash))?;
    let (metadata_json, _) = encode_document("content module metadata", &module.metadata)?;
    let now = Utc::now().to_rfc3339();
    let previous_revision_id = transaction
        .query_row(
            "SELECT revision_id
             FROM content_module_revisions
             WHERE module_id = ?1
             ORDER BY revision_no DESC
             LIMIT 1",
            [module.id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?;
    let metadata = ContentModuleProjectionMetadata {
        revision_id,
        document_json,
        revision_no,
        state_version,
        source_kind,
        source_hash: &source_hash,
        metadata_json: &metadata_json,
        now: &now,
        previous_revision_id: previous_revision_id.as_deref(),
        expected_revision,
    };
    write_content_module_projection_header(transaction, module, &metadata)?;
    write_content_module_variables(transaction, revision_id, module)?;
    let component_ordinal = write_content_module_controls(transaction, revision_id, module, 0)?;
    let component_ordinal =
        write_content_module_prompt_blocks(transaction, revision_id, module, component_ordinal)?;
    let component_ordinal = write_content_module_linked_components(
        transaction,
        revision_id,
        module,
        component_ordinal,
    )?;
    write_content_module_assets(transaction, revision_id, module, component_ordinal)?;
    write_content_module_capabilities(transaction, revision_id, module)
}

fn active_content_revision_sha256(
    transaction: &Transaction<'_>,
    revision_id: &str,
) -> CoreResult<String> {
    transaction
        .query_row(
            "SELECT document_sha256 FROM content_revisions WHERE id = ?1",
            [revision_id],
            |row| row.get(0),
        )
        .map_err(storage_db_error)
}

fn insert_linked_module_component(
    transaction: &Transaction<'_>,
    module_revision_id: &str,
    ordinal: i64,
    kind: &str,
    target_revision_id: &str,
    component_hash: &str,
) -> CoreResult<()> {
    let (knowledge, transform, interaction) = match kind {
        "knowledge_book" => (Some(target_revision_id), None, None),
        "transform_set" => (None, Some(target_revision_id), None),
        "interaction_rule_set" => (None, None, Some(target_revision_id)),
        _ => {
            return Err(CoreError::internal(
                "unsupported linked module component kind",
            ));
        }
    };
    transaction
        .execute(
            "INSERT INTO content_module_components
             (module_revision_id, ordinal, component_kind,
              prompt_block_id, control_id, knowledge_book_revision_id,
              memory_profile_revision_id, transform_set_revision_id,
              interaction_rule_set_revision_id, asset_descriptor_id,
              merge_policy, component_sha256, config_json)
             VALUES (
                 ?1, ?2, ?3, NULL, NULL, ?4, NULL, ?5, ?6, NULL,
                 'append', ?7, '{}'
             )",
            params![
                module_revision_id,
                ordinal,
                kind,
                knowledge,
                transform,
                interaction,
                component_hash,
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}
