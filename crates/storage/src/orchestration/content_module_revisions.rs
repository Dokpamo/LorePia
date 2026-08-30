//! Exact content-module revision and component retrieval.

use super::{
    ActiveContentModuleRevision, Connection, ContentModule, ContentModuleId, ContentModuleRevision,
    ContentModuleRevisionDiff, CoreError, CoreResult, DeserializeOwned, DocumentTable,
    InteractionRuleSet, InteractionRuleSetId, KnowledgeBook, KnowledgeBookId,
    ModuleRevisionComponentSnapshot, ModuleRevisionId, ObjectRevision, OptionalExtension,
    PromptBlockId, Storage, StoredRevision, TransformSet, TransformSetId, decode_document,
    diff_content_object_revisions, list_object_revisions, load_exact_content_revision,
    module_component_storage_key, not_found, params, parse_datetime, rollback_content_object,
    sha256_hex, storage_corrupted, storage_db_error, u64_revision,
};

impl Storage {
    pub fn get_active_content_module_revision(
        &self,
        module_id: &ContentModuleId,
    ) -> CoreResult<ActiveContentModuleRevision> {
        let connection = self.connection()?;
        let revision_id = connection
            .query_row(
                "SELECT state.active_revision_id
                 FROM content_objects AS object
                 JOIN content_object_state AS state ON state.object_id = object.id
                 WHERE object.id = ?1
                   AND object.object_kind = 'content_module'
                   AND object.deleted_at IS NULL",
                [module_id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("content module"))?;
        load_content_module_revision(&connection, module_id, &revision_id)
    }

    pub fn get_content_module_revision(
        &self,
        module_id: &ContentModuleId,
        revision_id: &ModuleRevisionId,
    ) -> CoreResult<ActiveContentModuleRevision> {
        let connection = self.connection()?;
        load_content_module_revision(&connection, module_id, revision_id.as_str())
    }

    pub fn get_module_revision_component(
        &self,
        source: &lorepia_orchestration::ModuleCandidateSource,
        component: &lorepia_domain::ModuleComponentRef,
        expected_component_sha256: &lorepia_domain::Sha256Digest,
    ) -> CoreResult<ModuleRevisionComponentSnapshot> {
        get_module_revision_component(self, source, component, expected_component_sha256)
    }

    pub fn list_content_module_revisions(
        &self,
        id: &ContentModuleId,
    ) -> CoreResult<Vec<ObjectRevision<ContentModule>>> {
        list_object_revisions(self, DocumentTable::ContentModules, id.as_str())
    }

    pub fn diff_content_module_revisions(
        &self,
        id: &ContentModuleId,
        from_revision: u64,
        to_revision: u64,
    ) -> CoreResult<ContentModuleRevisionDiff> {
        diff_content_object_revisions(
            self,
            DocumentTable::ContentModules,
            id.as_str(),
            from_revision,
            to_revision,
        )
    }

    pub fn rollback_content_module(
        &self,
        id: &ContentModuleId,
        target_revision: u64,
        expected_revision: u64,
    ) -> CoreResult<StoredRevision<ContentModule>> {
        rollback_content_object(
            self,
            DocumentTable::ContentModules,
            id.as_str(),
            target_revision,
            expected_revision,
        )
    }
}

pub(super) fn load_content_module_revision(
    connection: &Connection,
    module_id: &ContentModuleId,
    revision_id: &str,
) -> CoreResult<ActiveContentModuleRevision> {
    let row = load_content_module_revision_row(connection, module_id, revision_id)?;
    let module = decode_document::<ContentModule>("content module", &row.document_json)?;
    let component_hashes = load_content_module_component_rows(connection, revision_id)?
        .into_iter()
        .map(|component| resolve_content_module_component(connection, component))
        .collect::<CoreResult<Vec<_>>>()?;
    let source_hash = lorepia_domain::Sha256Digest::parse(row.source_hash).map_err(|error| {
        storage_corrupted(format!("stored module source hash is invalid: {error}"))
    })?;
    Ok(ActiveContentModuleRevision {
        object: ObjectRevision {
            revision_id: revision_id.to_owned(),
            object_kind: "content_module".to_owned(),
            object_id: module_id.as_str().to_owned(),
            revision: u64_revision(row.revision_no)?,
            value: module.clone(),
            sha256: row.document_sha256,
            created_at: parse_datetime(
                "content module revision created_at",
                &row.content_created_at,
            )?,
        },
        module_revision: ContentModuleRevision {
            id: ModuleRevisionId::from(revision_id),
            module_id: module_id.clone(),
            version: row.version,
            source_hash,
            previous_revision_id: row.previous_revision_id.map(ModuleRevisionId::from),
            component_hashes,
            created_at: parse_datetime("module projection created_at", &row.module_created_at)?,
        },
    })
}

struct StoredContentModuleRevisionRow {
    revision_no: i64,
    document_json: String,
    document_sha256: String,
    content_created_at: String,
    version: String,
    previous_revision_id: Option<String>,
    source_hash: String,
    module_created_at: String,
}

fn load_content_module_revision_row(
    connection: &Connection,
    module_id: &ContentModuleId,
    revision_id: &str,
) -> CoreResult<StoredContentModuleRevisionRow> {
    connection
        .query_row(
            "SELECT content.revision_no, content.document_json,
                    content.document_sha256, content.created_at,
                    module.version, module.previous_revision_id,
                    module.source_hash, module.created_at
             FROM content_revisions AS content
             JOIN content_module_revisions AS module
               ON module.revision_id = content.id
              AND module.module_id = content.object_id
             WHERE content.object_id = ?1 AND content.id = ?2
               AND content.object_kind = 'content_module'",
            params![module_id.as_str(), revision_id],
            |row| {
                Ok(StoredContentModuleRevisionRow {
                    revision_no: row.get(0)?,
                    document_json: row.get(1)?,
                    document_sha256: row.get(2)?,
                    content_created_at: row.get(3)?,
                    version: row.get(4)?,
                    previous_revision_id: row.get(5)?,
                    source_hash: row.get(6)?,
                    module_created_at: row.get(7)?,
                })
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("content module revision"))
}

struct StoredContentModuleComponentRow {
    kind: String,
    prompt_block_id: Option<String>,
    control_id: Option<String>,
    knowledge_book_revision_id: Option<String>,
    transform_set_revision_id: Option<String>,
    interaction_rule_set_revision_id: Option<String>,
    asset_descriptor_id: Option<String>,
    sha256: String,
}

fn load_content_module_component_rows(
    connection: &Connection,
    revision_id: &str,
) -> CoreResult<Vec<StoredContentModuleComponentRow>> {
    let mut statement = connection
        .prepare(
            "SELECT component_kind, prompt_block_id, control_id,
                    knowledge_book_revision_id, transform_set_revision_id,
                    interaction_rule_set_revision_id, asset_descriptor_id,
                    component_sha256
             FROM content_module_components
             WHERE module_revision_id = ?1
             ORDER BY ordinal",
        )
        .map_err(storage_db_error)?;
    statement
        .query_map([revision_id], |row| {
            Ok(StoredContentModuleComponentRow {
                kind: row.get(0)?,
                prompt_block_id: row.get(1)?,
                control_id: row.get(2)?,
                knowledge_book_revision_id: row.get(3)?,
                transform_set_revision_id: row.get(4)?,
                interaction_rule_set_revision_id: row.get(5)?,
                asset_descriptor_id: row.get(6)?,
                sha256: row.get(7)?,
            })
        })
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)
}

fn resolve_content_module_component(
    connection: &Connection,
    row: StoredContentModuleComponentRow,
) -> CoreResult<lorepia_domain::ComponentHash> {
    let reference = match row.kind.as_str() {
        "prompt_block" => {
            lorepia_domain::ModuleComponentRef::PromptBlock {
                id: PromptBlockId::from(row.prompt_block_id.ok_or_else(|| {
                    storage_corrupted("module prompt block component is incomplete")
                })?),
            }
        }
        "control" => lorepia_domain::ModuleComponentRef::Control {
            id: lorepia_domain::ControlId::from(
                row.control_id
                    .ok_or_else(|| storage_corrupted("module control component is incomplete"))?,
            ),
        },
        "knowledge_book" => lorepia_domain::ModuleComponentRef::KnowledgeBook {
            id: KnowledgeBookId::from(content_object_id_for_revision(
                connection,
                row.knowledge_book_revision_id
                    .as_deref()
                    .ok_or_else(|| storage_corrupted("module knowledge component is incomplete"))?,
            )?),
        },
        "transform_set" => lorepia_domain::ModuleComponentRef::TransformSet {
            id: TransformSetId::from(content_object_id_for_revision(
                connection,
                row.transform_set_revision_id
                    .as_deref()
                    .ok_or_else(|| storage_corrupted("module transform component is incomplete"))?,
            )?),
        },
        "interaction_rule_set" => lorepia_domain::ModuleComponentRef::InteractionRuleSet {
            id: InteractionRuleSetId::from(content_object_id_for_revision(
                connection,
                row.interaction_rule_set_revision_id
                    .as_deref()
                    .ok_or_else(|| {
                        storage_corrupted("module interaction component is incomplete")
                    })?,
            )?),
        },
        "asset" => lorepia_domain::ModuleComponentRef::Asset {
            id: lorepia_domain::AssetId::from(
                row.asset_descriptor_id
                    .ok_or_else(|| storage_corrupted("module asset component is incomplete"))?,
            ),
        },
        other => {
            return Err(storage_corrupted(format!(
                "stored module component kind is invalid: {other}"
            )));
        }
    };
    Ok(lorepia_domain::ComponentHash {
        component: reference,
        sha256: lorepia_domain::Sha256Digest::parse(row.sha256).map_err(|error| {
            storage_corrupted(format!("stored module component hash is invalid: {error}"))
        })?,
    })
}

type StoredModuleRevisionComponentRow = (String, String, Option<String>);

const MODULE_PROMPT_BLOCK_COMPONENT_SQL: &str =
    "SELECT component.component_sha256, block.document_json, NULL
     FROM content_module_components AS component
     JOIN content_module_prompt_blocks AS block
       ON block.module_revision_id = component.module_revision_id
      AND block.block_id = component.prompt_block_id
     WHERE component.module_revision_id = ?1
       AND component.component_kind = 'prompt_block'
       AND component.prompt_block_id = ?2";
const MODULE_CONTROL_COMPONENT_SQL: &str =
    "SELECT component.component_sha256, control.document_json, NULL
     FROM content_module_components AS component
     JOIN content_module_controls AS control
       ON control.module_revision_id = component.module_revision_id
      AND control.control_id = component.control_id
     WHERE component.module_revision_id = ?1
       AND component.component_kind = 'control'
       AND component.control_id = ?2";
const MODULE_KNOWLEDGE_COMPONENT_SQL: &str =
    "SELECT component.component_sha256, revision.revision_id,
            revision.revision_id
     FROM content_module_components AS component
     JOIN knowledge_book_revisions AS revision
       ON revision.revision_id = component.knowledge_book_revision_id
     WHERE component.module_revision_id = ?1
       AND component.component_kind = 'knowledge_book'
       AND revision.knowledge_book_id = ?2";
const MODULE_TRANSFORM_COMPONENT_SQL: &str =
    "SELECT component.component_sha256, revision.revision_id,
            revision.revision_id
     FROM content_module_components AS component
     JOIN transform_set_revisions AS revision
       ON revision.revision_id = component.transform_set_revision_id
     WHERE component.module_revision_id = ?1
       AND component.component_kind = 'transform_set'
       AND revision.transform_set_id = ?2";
const MODULE_INTERACTION_COMPONENT_SQL: &str =
    "SELECT component.component_sha256, revision.revision_id,
            revision.revision_id
     FROM content_module_components AS component
     JOIN interaction_rule_set_revisions AS revision
       ON revision.revision_id = component.interaction_rule_set_revision_id
     WHERE component.module_revision_id = ?1
       AND component.component_kind = 'interaction_rule_set'
       AND revision.interaction_rule_set_id = ?2";
const MODULE_ASSET_COMPONENT_SQL: &str =
    "SELECT component.component_sha256, descriptor.payload_json, NULL
     FROM content_module_components AS component
     JOIN asset_descriptors AS descriptor
       ON descriptor.id = component.asset_descriptor_id
     WHERE component.module_revision_id = ?1
       AND component.component_kind = 'asset'
       AND component.asset_descriptor_id = ?2";

fn query_module_revision_component_row(
    connection: &Connection,
    sql: &str,
    revision_id: &str,
    component_id: &str,
) -> rusqlite::Result<Option<StoredModuleRevisionComponentRow>> {
    connection
        .query_row(sql, params![revision_id, component_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .optional()
}

fn load_module_revision_component_row(
    connection: &Connection,
    revision_id: &str,
    component: &lorepia_domain::ModuleComponentRef,
) -> CoreResult<StoredModuleRevisionComponentRow> {
    let row = match component {
        lorepia_domain::ModuleComponentRef::PromptBlock { id } => {
            query_module_revision_component_row(
                connection,
                MODULE_PROMPT_BLOCK_COMPONENT_SQL,
                revision_id,
                id.as_str(),
            )
        }
        lorepia_domain::ModuleComponentRef::Control { id } => query_module_revision_component_row(
            connection,
            MODULE_CONTROL_COMPONENT_SQL,
            revision_id,
            id.as_str(),
        ),
        lorepia_domain::ModuleComponentRef::KnowledgeBook { id } => {
            query_module_revision_component_row(
                connection,
                MODULE_KNOWLEDGE_COMPONENT_SQL,
                revision_id,
                id.as_str(),
            )
        }
        lorepia_domain::ModuleComponentRef::TransformSet { id } => {
            query_module_revision_component_row(
                connection,
                MODULE_TRANSFORM_COMPONENT_SQL,
                revision_id,
                id.as_str(),
            )
        }
        lorepia_domain::ModuleComponentRef::InteractionRuleSet { id } => {
            query_module_revision_component_row(
                connection,
                MODULE_INTERACTION_COMPONENT_SQL,
                revision_id,
                id.as_str(),
            )
        }
        lorepia_domain::ModuleComponentRef::Asset { id } => query_module_revision_component_row(
            connection,
            MODULE_ASSET_COMPONENT_SQL,
            revision_id,
            id.as_str(),
        ),
    }
    .map_err(storage_db_error)?
    .ok_or_else(|| not_found("exact module revision component"))?;
    Ok(row)
}

fn decode_module_component_payload<T>(
    kind: &str,
    payload_json: &str,
    expected_sha256: &str,
    invalid_hash_message: &str,
) -> CoreResult<T>
where
    T: DeserializeOwned,
{
    if sha256_hex(payload_json.as_bytes()) != expected_sha256 {
        return Err(storage_corrupted(invalid_hash_message));
    }
    decode_document(kind, payload_json)
}

fn load_linked_module_component_revision<T>(
    connection: &Connection,
    revision_id: Option<&str>,
    expected_kind: &str,
    expected_sha256: &str,
    missing_link_message: &str,
    invalid_hash_message: &str,
) -> CoreResult<ObjectRevision<T>>
where
    T: DeserializeOwned,
{
    let revision = load_exact_content_revision::<T>(
        connection,
        revision_id.ok_or_else(|| storage_corrupted(missing_link_message))?,
        expected_kind,
    )?;
    if revision.sha256 != expected_sha256 {
        return Err(storage_corrupted(invalid_hash_message));
    }
    Ok(revision)
}

fn get_module_revision_component(
    storage: &Storage,
    source: &lorepia_orchestration::ModuleCandidateSource,
    component: &lorepia_domain::ModuleComponentRef,
    expected_component_sha256: &lorepia_domain::Sha256Digest,
) -> CoreResult<ModuleRevisionComponentSnapshot> {
    let connection = storage.connection()?;
    let parent =
        load_content_module_revision(&connection, &source.module_id, source.revision_id.as_str())?;
    if parent.module_revision.source_hash != source.revision_source_sha256 {
        return Err(CoreError::invalid("module candidate source hash is stale"));
    }
    let parent_component = parent
        .module_revision
        .component_hashes
        .iter()
        .find(|hash| &hash.component == component)
        .ok_or_else(|| not_found("module revision component"))?;
    if &parent_component.sha256 != expected_component_sha256 {
        return Err(CoreError::invalid(
            "module component hash does not match the approved plan",
        ));
    }
    let expected_kind = module_component_storage_key(component).0;
    let row =
        load_module_revision_component_row(&connection, source.revision_id.as_str(), component)?;
    if row.0 != expected_component_sha256.as_str() {
        return Err(storage_corrupted(format!(
            "stored {expected_kind} component hash differs from its parent revision"
        )));
    }
    match component {
        lorepia_domain::ModuleComponentRef::PromptBlock { .. } => decode_module_component_payload(
            "module prompt block",
            &row.1,
            &row.0,
            "module prompt block payload hash is invalid",
        )
        .map(ModuleRevisionComponentSnapshot::PromptBlock),
        lorepia_domain::ModuleComponentRef::Control { .. } => decode_module_component_payload(
            "module control",
            &row.1,
            &row.0,
            "module control payload hash is invalid",
        )
        .map(ModuleRevisionComponentSnapshot::Control),
        lorepia_domain::ModuleComponentRef::KnowledgeBook { .. } => {
            load_linked_module_component_revision::<KnowledgeBook>(
                &connection,
                row.2.as_deref(),
                "knowledge_book",
                &row.0,
                "module knowledge revision link is missing",
                "module knowledge revision hash is invalid",
            )
            .map(ModuleRevisionComponentSnapshot::KnowledgeBook)
        }
        lorepia_domain::ModuleComponentRef::TransformSet { .. } => {
            load_linked_module_component_revision::<TransformSet>(
                &connection,
                row.2.as_deref(),
                "transform_set",
                &row.0,
                "module transform revision link is missing",
                "module transform revision hash is invalid",
            )
            .map(ModuleRevisionComponentSnapshot::TransformSet)
        }
        lorepia_domain::ModuleComponentRef::InteractionRuleSet { .. } => {
            load_linked_module_component_revision::<InteractionRuleSet>(
                &connection,
                row.2.as_deref(),
                "interaction_rule_set",
                &row.0,
                "module interaction revision link is missing",
                "module interaction revision hash is invalid",
            )
            .map(ModuleRevisionComponentSnapshot::InteractionRuleSet)
        }
        lorepia_domain::ModuleComponentRef::Asset { .. } => decode_module_component_payload(
            "module asset descriptor",
            &row.1,
            &row.0,
            "module asset descriptor payload hash is invalid",
        )
        .map(ModuleRevisionComponentSnapshot::Asset),
    }
}

fn content_object_id_for_revision(
    connection: &Connection,
    revision_id: &str,
) -> CoreResult<String> {
    connection
        .query_row(
            "SELECT object_id FROM content_revisions WHERE id = ?1",
            [revision_id],
            |row| row.get(0),
        )
        .map_err(storage_db_error)
}
