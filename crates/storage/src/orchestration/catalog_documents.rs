//! Persona, task-profile, and prompt-preset revisioned document facade.

use super::{
    BTreeSet, Connection, ContentModuleId, CoreError, CoreResult, DateTime, DocumentTable,
    KnowledgeBook, KnowledgeBookId, MemoryProfile, ModuleRevisionId, ObjectRevision,
    OptionalExtension, Persona, PersonaCatalogPage, PersonaId, PromptPreset, PromptPresetId,
    PromptPresetModuleDependency, PromptPresetRevisionDiff, PromptPresetRollbackApproval,
    PromptPresetRollbackCommit, PromptPresetRollbackReview, RevisionEventKind, Sha256Digest,
    Storage, StoredRevision, TaskProfile, TaskProfileId, Transaction, TransactionBehavior,
    TransformSet, TransformSetId, Utc, ValidateOrchestration, apply_prompt_preset_rollback,
    content_revision_number, decode_document, diff_prompt_preset_revision_documents,
    document_provenance, enum_wire, get_document, i64_revision, list_documents,
    list_documents_page, list_object_revisions, load_exact_content_revision, not_found, params,
    parse_datetime, persona_catalog_revision, prompt_preset_rollback_approval_sha256,
    review_prompt_preset_rollback, rollback_content_object, save_content_object,
    soft_delete_content_object, storage_corrupted, storage_db_error, usize_to_i64,
    validate_identifier, write_prompt_preset_projection,
};

impl Storage {
    pub fn save_persona(
        &self,
        persona: &Persona,
        expected_revision: Option<u64>,
    ) -> CoreResult<StoredRevision<Persona>> {
        persona
            .validate()
            .map_err(|error| CoreError::invalid(error.to_string()))?;
        save_content_object(
            self,
            DocumentTable::Personas,
            persona.id.as_str(),
            persona.schema_version,
            persona,
            &persona.provenance,
            expected_revision,
            if expected_revision.is_some() {
                RevisionEventKind::Update
            } else {
                RevisionEventKind::Create
            },
            |transaction, revision_id, document_json| {
                write_persona_projection(
                    transaction,
                    revision_id,
                    persona,
                    document_json,
                    expected_revision,
                )
            },
            false,
        )
    }

    pub fn get_persona(&self, id: &PersonaId) -> CoreResult<StoredRevision<Persona>> {
        get_document(self, DocumentTable::Personas, id.as_str(), false)
    }

    pub fn list_personas(&self, limit: u32) -> CoreResult<Vec<StoredRevision<Persona>>> {
        match self.list_personas_page(None, None, limit)? {
            PersonaCatalogPage::Page { items, .. } => Ok(items),
            PersonaCatalogPage::RestartRequired { .. } => Err(CoreError::internal(
                "an initial persona catalog page unexpectedly required restart",
            )),
        }
    }

    /// Lists active personas after one exact `(updated_at DESC, id ASC)`
    /// boundary. The timestamp is normalized back to the repository's RFC
    /// 3339 representation before comparison so a serialized `Z` cursor and
    /// the durable `+00:00` value retain equal-timestamp semantics.
    pub fn list_personas_page(
        &self,
        expected_catalog_revision: Option<&Sha256Digest>,
        after: Option<(&DateTime<Utc>, &PersonaId)>,
        limit: u32,
    ) -> CoreResult<PersonaCatalogPage> {
        if expected_catalog_revision.is_some() != after.is_some() {
            return Err(CoreError::invalid(
                "persona page cursor revision and boundary must be provided together",
            ));
        }
        if let Some((_, persona_id)) = after {
            validate_identifier("persona page cursor", persona_id.as_str())?;
        }
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(storage_db_error)?;
        let catalog_revision = persona_catalog_revision(&transaction)?;
        if expected_catalog_revision.is_some_and(|expected| expected != &catalog_revision) {
            transaction.commit().map_err(storage_db_error)?;
            return Ok(PersonaCatalogPage::RestartRequired {
                current_catalog_revision: catalog_revision,
            });
        }
        let items = list_documents_page(
            &transaction,
            DocumentTable::Personas,
            after.map(|(updated_at, persona_id)| (updated_at, persona_id.as_str())),
            limit,
        )?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(PersonaCatalogPage::Page {
            catalog_revision,
            items,
        })
    }

    pub fn list_persona_revisions(
        &self,
        id: &PersonaId,
    ) -> CoreResult<Vec<ObjectRevision<Persona>>> {
        list_object_revisions(self, DocumentTable::Personas, id.as_str())
    }

    pub fn get_persona_revision(
        &self,
        id: &PersonaId,
        revision_id: &str,
    ) -> CoreResult<ObjectRevision<Persona>> {
        let connection = self.connection()?;
        let revision = load_exact_content_revision::<Persona>(&connection, revision_id, "persona")?;
        if revision.object_id != id.as_str() || revision.value.id != *id {
            return Err(storage_corrupted(
                "exact persona revision identity does not match its owner",
            ));
        }
        Ok(revision)
    }

    pub fn rollback_persona(
        &self,
        id: &PersonaId,
        target_revision: u64,
        expected_revision: u64,
    ) -> CoreResult<StoredRevision<Persona>> {
        rollback_content_object(
            self,
            DocumentTable::Personas,
            id.as_str(),
            target_revision,
            expected_revision,
        )
    }

    pub fn soft_delete_persona(
        &self,
        id: &PersonaId,
        expected_revision: u64,
    ) -> CoreResult<StoredRevision<Persona>> {
        let current = self.get_persona(id)?;
        soft_delete_content_object(
            self,
            DocumentTable::Personas,
            id.as_str(),
            expected_revision,
            |transaction, revision_id, document_json| {
                write_persona_projection(
                    transaction,
                    revision_id,
                    &current.value,
                    document_json,
                    Some(expected_revision),
                )?;
                crate::persona_repository::clear_persona_selections_in_transaction(
                    transaction,
                    id,
                    Utc::now(),
                )
            },
        )
    }

    pub fn get_task_profile(&self, id: &TaskProfileId) -> CoreResult<StoredRevision<TaskProfile>> {
        get_document(self, DocumentTable::TaskProfiles, id.as_str(), false)
    }

    pub fn list_task_profiles(&self) -> CoreResult<Vec<StoredRevision<TaskProfile>>> {
        list_documents(self, DocumentTable::TaskProfiles)
    }

    pub fn save_task_profile(
        &self,
        profile: &TaskProfile,
        expected_revision: Option<u64>,
    ) -> CoreResult<StoredRevision<TaskProfile>> {
        let provenance = document_provenance(DocumentTable::TaskProfiles, profile)?;
        save_content_object(
            self,
            DocumentTable::TaskProfiles,
            profile.id.as_str(),
            1,
            profile,
            &provenance,
            expected_revision,
            if expected_revision.is_some() {
                RevisionEventKind::Update
            } else {
                RevisionEventKind::Create
            },
            |transaction, revision_id, document_json| {
                write_task_profile_projection(
                    transaction,
                    revision_id,
                    profile,
                    document_json,
                    expected_revision,
                )
            },
            false,
        )
    }

    pub fn soft_delete_task_profile(
        &self,
        id: &TaskProfileId,
        expected_revision: u64,
    ) -> CoreResult<StoredRevision<TaskProfile>> {
        let current = self.get_task_profile(id)?;
        soft_delete_content_object(
            self,
            DocumentTable::TaskProfiles,
            id.as_str(),
            expected_revision,
            |transaction, revision_id, document_json| {
                write_task_profile_projection(
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

struct TaskProfileProjection<'a> {
    revision_id: &'a str,
    profile: &'a TaskProfile,
    document_json: &'a str,
    expected_revision: Option<u64>,
    task_kind: String,
    display_name: String,
    timeout_ms: i64,
}

fn prepare_task_profile_projection<'a>(
    revision_id: &'a str,
    profile: &'a TaskProfile,
    document_json: &'a str,
    expected_revision: Option<u64>,
) -> CoreResult<TaskProfileProjection<'a>> {
    validate_identifier("task profile", profile.id.as_str())?;
    if profile.timeout_ms == 0
        || profile.timeout_ms > 600_000
        || profile.rate_limit.requests == 0
        || profile.rate_limit.per_seconds == 0
        || profile.concurrency_limit == 0
        || profile.concurrency_limit > 64
    {
        return Err(CoreError::invalid(
            "task profile timeout, rate limit, or concurrency limit is invalid",
        ));
    }
    let task_kind = enum_wire(&profile.kind)?;
    let display_name = task_kind
        .split('_')
        .map(|part| {
            let mut characters = part.chars();
            characters.next().map_or_else(String::new, |first| {
                first.to_uppercase().collect::<String>() + characters.as_str()
            })
        })
        .collect::<Vec<_>>()
        .join(" ");
    let timeout_ms = i64::try_from(profile.timeout_ms)
        .map_err(|_| CoreError::invalid("task timeout exceeds SQLite range"))?;
    Ok(TaskProfileProjection {
        revision_id,
        profile,
        document_json,
        expected_revision,
        task_kind,
        display_name,
        timeout_ms,
    })
}

fn write_task_profile_rows(
    transaction: &Transaction<'_>,
    projection: &TaskProfileProjection<'_>,
) -> CoreResult<()> {
    let profile = projection.profile;
    let state_version = projection
        .expected_revision
        .map_or(1, |value| value.saturating_add(1));
    let now = Utc::now().to_rfc3339();
    transaction
        .execute(
            "INSERT INTO task_profiles
             (id, name, task_kind, schema_version, revision, model_route_id,
              generation_preset_id, timeout_ms, rate_limit_requests,
              rate_limit_per_seconds, concurrency_limit, document_json,
              created_at, updated_at, deleted_at)
             VALUES (?1, ?2, ?3, 1, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                     ?12, ?12, NULL)
             ON CONFLICT(id) DO UPDATE SET
                 name = excluded.name,
                 task_kind = excluded.task_kind,
                 schema_version = excluded.schema_version,
                 revision = excluded.revision,
                 model_route_id = excluded.model_route_id,
                 generation_preset_id = excluded.generation_preset_id,
                 timeout_ms = excluded.timeout_ms,
                 rate_limit_requests = excluded.rate_limit_requests,
                 rate_limit_per_seconds = excluded.rate_limit_per_seconds,
                 concurrency_limit = excluded.concurrency_limit,
                 document_json = excluded.document_json,
                 updated_at = excluded.updated_at",
            params![
                profile.id.as_str(),
                projection.display_name.as_str(),
                projection.task_kind.as_str(),
                i64_revision(state_version)?,
                profile.route_id.as_str(),
                profile.generation_preset_id.as_str(),
                projection.timeout_ms,
                profile.rate_limit.requests,
                profile.rate_limit.per_seconds,
                profile.concurrency_limit,
                projection.document_json,
                now,
            ],
        )
        .map_err(storage_db_error)?;
    let revision_no = content_revision_number(transaction, projection.revision_id)?;
    transaction
        .execute(
            "INSERT INTO task_profile_revisions
             (revision_id, task_profile_id, revision_no, task_kind,
              model_route_id, generation_preset_id, timeout_ms,
              rate_limit_requests, rate_limit_per_seconds, concurrency_limit,
              payload_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                projection.revision_id,
                profile.id.as_str(),
                i64_revision(revision_no)?,
                projection.task_kind.as_str(),
                profile.route_id.as_str(),
                profile.generation_preset_id.as_str(),
                projection.timeout_ms,
                profile.rate_limit.requests,
                profile.rate_limit.per_seconds,
                profile.concurrency_limit,
                projection.document_json,
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn write_task_profile_fallbacks(
    transaction: &Transaction<'_>,
    revision_id: &str,
    profile: &TaskProfile,
) -> CoreResult<()> {
    let mut fallback_ids = BTreeSet::new();
    for (ordinal, route_id) in profile.fallback_route_ids.iter().enumerate() {
        if route_id == &profile.route_id || !fallback_ids.insert(route_id.as_str()) {
            return Err(CoreError::invalid(
                "task fallback routes must be unique and differ from the primary route",
            ));
        }
        transaction
            .execute(
                "INSERT INTO task_profile_fallbacks
                 (task_profile_revision_id, ordinal, model_route_id,
                  generation_preset_id, timeout_override_ms, payload_json)
                 VALUES (?1, ?2, ?3, NULL, NULL, ?4)",
                params![
                    revision_id,
                    usize_to_i64(ordinal, "task fallback ordinal")?,
                    route_id.as_str(),
                    serde_json::json!({"model_route_id": route_id}).to_string(),
                ],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

fn write_task_profile_projection(
    transaction: &Transaction<'_>,
    revision_id: &str,
    profile: &TaskProfile,
    document_json: &str,
    expected_revision: Option<u64>,
) -> CoreResult<()> {
    let projection =
        prepare_task_profile_projection(revision_id, profile, document_json, expected_revision)?;
    write_task_profile_rows(transaction, &projection)?;
    write_task_profile_fallbacks(transaction, revision_id, profile)
}

fn write_persona_projection(
    transaction: &Transaction<'_>,
    revision_id: &str,
    persona: &Persona,
    document_json: &str,
    expected_revision: Option<u64>,
) -> CoreResult<()> {
    persona
        .validate()
        .map_err(|error| CoreError::invalid(error.to_string()))?;
    let state_version = expected_revision.map_or(1, |value| value.saturating_add(1));
    transaction
        .execute(
            "INSERT INTO personas
             (object_id, name, schema_version, revision, description,
              payload_json, created_at, updated_at, deleted_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL)
             ON CONFLICT(object_id) DO UPDATE SET
                 name = excluded.name,
                 schema_version = excluded.schema_version,
                 revision = excluded.revision,
                 description = excluded.description,
                 payload_json = excluded.payload_json,
                 updated_at = excluded.updated_at,
                 deleted_at = NULL",
            params![
                persona.id.as_str(),
                persona.name,
                persona.schema_version,
                i64_revision(state_version)?,
                persona.description,
                document_json,
                persona.created_at.to_rfc3339(),
                persona.updated_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    let revision_no = content_revision_number(transaction, revision_id)?;
    transaction
        .execute(
            "INSERT INTO persona_revisions
             (revision_id, persona_id, revision_no, name, description,
              document_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                revision_id,
                persona.id.as_str(),
                i64_revision(revision_no)?,
                persona.name,
                persona.description,
                document_json,
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn get_prompt_preset_rollback_approval(
    storage: &Storage,
    approval_id: &str,
) -> CoreResult<PromptPresetRollbackApproval> {
    validate_identifier("prompt preset rollback approval", approval_id)?;
    storage
        .connection()?
        .query_row(
            "SELECT approval_sha256, review_sha256, approved_at
             FROM prompt_preset_rollback_approvals
             WHERE approval_id = ?1",
            [approval_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("prompt preset rollback approval"))
        .and_then(|row| {
            let approval = PromptPresetRollbackApproval {
                approval_id: approval_id.to_owned(),
                expected_review_sha256: row.1,
                approval_sha256: row.0,
                approved_at: parse_datetime("prompt preset rollback approved_at", &row.2)?,
            };
            let expected = prompt_preset_rollback_approval_sha256(
                &approval.approval_id,
                &approval.expected_review_sha256,
            )?;
            if approval.approval_sha256 != expected {
                return Err(storage_corrupted(
                    "stored prompt preset rollback approval hash is invalid",
                ));
            }
            Ok(approval)
        })
}

impl Storage {
    pub fn save_prompt_preset(
        &self,
        preset: &PromptPreset,
        expected_revision: Option<u64>,
    ) -> CoreResult<StoredRevision<PromptPreset>> {
        let provenance = preset.metadata.provenance.clone();
        save_content_object(
            self,
            DocumentTable::PromptPresets,
            preset.id.as_str(),
            preset.schema_version,
            preset,
            &provenance,
            expected_revision,
            if expected_revision.is_some() {
                RevisionEventKind::Update
            } else {
                RevisionEventKind::Create
            },
            |transaction, revision_id, document_json| {
                write_prompt_preset_projection(
                    transaction,
                    revision_id,
                    preset,
                    document_json,
                    expected_revision,
                )
            },
            false,
        )
    }

    pub fn get_prompt_preset(
        &self,
        id: &PromptPresetId,
    ) -> CoreResult<StoredRevision<PromptPreset>> {
        get_document(self, DocumentTable::PromptPresets, id.as_str(), false)
    }

    pub fn list_prompt_preset_revisions(
        &self,
        id: &PromptPresetId,
    ) -> CoreResult<Vec<ObjectRevision<PromptPreset>>> {
        list_object_revisions(self, DocumentTable::PromptPresets, id.as_str())
    }

    pub fn diff_prompt_preset_revisions(
        &self,
        id: &PromptPresetId,
        from_revision: u64,
        to_revision: u64,
    ) -> CoreResult<PromptPresetRevisionDiff> {
        diff_prompt_preset_revision_documents(self, id, from_revision, to_revision)
    }

    pub fn review_prompt_preset_rollback(
        &self,
        id: &PromptPresetId,
        expected_current_state_revision: u64,
        target_revision: u64,
        reviewed_at: DateTime<Utc>,
    ) -> CoreResult<PromptPresetRollbackReview> {
        review_prompt_preset_rollback(
            self,
            id,
            expected_current_state_revision,
            target_revision,
            reviewed_at,
        )
    }

    pub fn apply_prompt_preset_rollback(
        &self,
        commit: &PromptPresetRollbackCommit,
    ) -> CoreResult<StoredRevision<PromptPreset>> {
        apply_prompt_preset_rollback(self, commit)
    }

    /// Returns the exact durable approval, including its original trusted
    /// timestamp, so a response-loss retry can reproduce the first receipt.
    pub fn get_prompt_preset_rollback_approval(
        &self,
        approval_id: &str,
    ) -> CoreResult<PromptPresetRollbackApproval> {
        get_prompt_preset_rollback_approval(self, approval_id)
    }

    /// Returns the exact immutable module revisions captured when a prompt
    /// preset revision was written.
    ///
    /// These rows are dependency evidence only. They neither activate a module
    /// nor substitute for an approved module activation plan.
    pub fn get_prompt_preset_module_dependencies(
        &self,
        prompt_preset_revision_id: &str,
    ) -> CoreResult<Vec<PromptPresetModuleDependency>> {
        validate_identifier("prompt preset revision", prompt_preset_revision_id)?;
        let connection = self
            .connection
            .lock()
            .map_err(|_| CoreError::internal("storage connection lock is poisoned"))?;
        let preset_json = connection
            .query_row(
                "SELECT document_json
                 FROM prompt_preset_revisions
                 WHERE revision_id = ?1",
                [prompt_preset_revision_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("prompt preset revision"))?;
        let preset: PromptPreset =
            decode_document("prompt preset revision dependency owner", &preset_json)?;

        let mut statement = connection
            .prepare(
                "SELECT dependency.ordinal, dependency.module_id,
                        dependency.module_revision_id, dependency.source_sha256,
                        revision.source_hash
                 FROM prompt_preset_modules AS dependency
                 JOIN content_module_revisions AS revision
                   ON revision.module_id = dependency.module_id
                  AND revision.revision_id = dependency.module_revision_id
                 WHERE dependency.prompt_preset_revision_id = ?1
                 ORDER BY dependency.ordinal",
            )
            .map_err(storage_db_error)?;
        let raw_rows = statement
            .query_map([prompt_preset_revision_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?;

        if raw_rows.len() != preset.module_ids.len() {
            return Err(storage_corrupted(
                "prompt preset module dependency count does not match its immutable document",
            ));
        }
        let mut dependencies = Vec::with_capacity(raw_rows.len());
        for (expected_ordinal, raw) in raw_rows.into_iter().enumerate() {
            let ordinal = u32::try_from(raw.0)
                .map_err(|_| storage_corrupted("prompt preset module ordinal is invalid"))?;
            if ordinal as usize != expected_ordinal {
                return Err(storage_corrupted(
                    "prompt preset module dependency ordinals are not contiguous",
                ));
            }
            let expected_module_id = preset.module_ids.get(expected_ordinal).ok_or_else(|| {
                storage_corrupted("prompt preset module dependency ordinal is out of range")
            })?;
            if raw.1 != expected_module_id.as_str() || raw.3 != raw.4 {
                return Err(storage_corrupted(
                    "prompt preset module dependency does not match its immutable module revision",
                ));
            }
            dependencies.push(PromptPresetModuleDependency {
                ordinal,
                prompt_preset_revision_id: prompt_preset_revision_id.to_owned(),
                module_id: ContentModuleId::from(raw.1),
                module_revision_id: ModuleRevisionId::from(raw.2),
                source_sha256: lorepia_domain::Sha256Digest::parse(raw.3).map_err(|error| {
                    storage_corrupted(format!(
                        "prompt preset module dependency has invalid source hash: {error}"
                    ))
                })?,
            });
        }
        Ok(dependencies)
    }

    /// Loads the exact knowledge-book revisions captured by a prompt-preset
    /// revision. Active pointers are deliberately ignored.
    pub fn get_prompt_preset_knowledge_book_revisions(
        &self,
        prompt_preset_revision_id: &str,
    ) -> CoreResult<Vec<ObjectRevision<KnowledgeBook>>> {
        let connection = self.connection()?;
        let preset = load_prompt_preset_revision_owner(&connection, prompt_preset_revision_id)?;
        let mut statement = connection
            .prepare(
                "SELECT ordinal, knowledge_book_revision_id
                 FROM prompt_preset_knowledge_books
                 WHERE prompt_preset_revision_id = ?1 AND enabled = 1
                 ORDER BY ordinal",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map([prompt_preset_revision_id], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?;
        drop(statement);
        if rows.len() != preset.knowledge_book_ids.len() {
            return Err(storage_corrupted(
                "prompt preset knowledge dependency count diverges from its document",
            ));
        }
        rows.into_iter()
            .enumerate()
            .map(|(expected_ordinal, (ordinal, revision_id))| {
                validate_dependency_ordinal("knowledge book", expected_ordinal, ordinal)?;
                let revision = load_exact_content_revision::<KnowledgeBook>(
                    &connection,
                    &revision_id,
                    "knowledge_book",
                )?;
                if preset
                    .knowledge_book_ids
                    .get(expected_ordinal)
                    .map(KnowledgeBookId::as_str)
                    != Some(revision.object_id.as_str())
                    || revision.value.id.as_str() != revision.object_id
                {
                    return Err(storage_corrupted(
                        "prompt preset knowledge dependency identity is invalid",
                    ));
                }
                Ok(revision)
            })
            .collect()
    }

    /// Loads the exact transform-set revisions captured by a prompt-preset
    /// revision. This never substitutes a newer active transform revision.
    pub fn get_prompt_preset_transform_set_revisions(
        &self,
        prompt_preset_revision_id: &str,
    ) -> CoreResult<Vec<ObjectRevision<TransformSet>>> {
        let connection = self.connection()?;
        let preset = load_prompt_preset_revision_owner(&connection, prompt_preset_revision_id)?;
        let mut statement = connection
            .prepare(
                "SELECT ordinal, transform_set_revision_id
                 FROM prompt_preset_transform_sets
                 WHERE prompt_preset_revision_id = ?1 AND enabled = 1
                 ORDER BY ordinal",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map([prompt_preset_revision_id], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?;
        drop(statement);
        if rows.len() != preset.transform_set_ids.len() {
            return Err(storage_corrupted(
                "prompt preset transform dependency count diverges from its document",
            ));
        }
        rows.into_iter()
            .enumerate()
            .map(|(expected_ordinal, (ordinal, revision_id))| {
                validate_dependency_ordinal("transform set", expected_ordinal, ordinal)?;
                let revision = load_exact_content_revision::<TransformSet>(
                    &connection,
                    &revision_id,
                    "transform_set",
                )?;
                if preset
                    .transform_set_ids
                    .get(expected_ordinal)
                    .map(TransformSetId::as_str)
                    != Some(revision.object_id.as_str())
                    || revision.value.id.as_str() != revision.object_id
                {
                    return Err(storage_corrupted(
                        "prompt preset transform dependency identity is invalid",
                    ));
                }
                Ok(revision)
            })
            .collect()
    }

    /// Loads the optional exact memory-profile revision captured by a
    /// prompt-preset revision.
    pub fn get_prompt_preset_memory_profile_revision(
        &self,
        prompt_preset_revision_id: &str,
    ) -> CoreResult<Option<ObjectRevision<MemoryProfile>>> {
        let connection = self.connection()?;
        let preset = load_prompt_preset_revision_owner(&connection, prompt_preset_revision_id)?;
        let revision_id = connection
            .query_row(
                "SELECT memory_profile_revision_id
                 FROM prompt_preset_memory_profiles
                 WHERE prompt_preset_revision_id = ?1 AND enabled = 1",
                [prompt_preset_revision_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_db_error)?;
        match (preset.memory_profile_id.as_ref(), revision_id) {
            (None, None) => Ok(None),
            (Some(expected), Some(revision_id)) => {
                let revision = load_exact_content_revision::<MemoryProfile>(
                    &connection,
                    &revision_id,
                    "memory_profile",
                )?;
                if expected.as_str() != revision.object_id
                    || revision.value.id.as_str() != revision.object_id
                {
                    return Err(storage_corrupted(
                        "prompt preset memory dependency identity is invalid",
                    ));
                }
                Ok(Some(revision))
            }
            _ => Err(storage_corrupted(
                "prompt preset memory dependency diverges from its document",
            )),
        }
    }

    pub fn list_prompt_presets(&self) -> CoreResult<Vec<StoredRevision<PromptPreset>>> {
        list_documents(self, DocumentTable::PromptPresets)
    }

    pub fn soft_delete_prompt_preset(
        &self,
        id: &PromptPresetId,
        expected_revision: u64,
    ) -> CoreResult<StoredRevision<PromptPreset>> {
        let current = self.get_prompt_preset(id)?;
        soft_delete_content_object(
            self,
            DocumentTable::PromptPresets,
            id.as_str(),
            expected_revision,
            |transaction, revision_id, document_json| {
                write_prompt_preset_projection(
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

fn load_prompt_preset_revision_owner(
    connection: &Connection,
    revision_id: &str,
) -> CoreResult<PromptPreset> {
    validate_identifier("prompt preset revision", revision_id)?;
    load_exact_content_revision::<PromptPreset>(connection, revision_id, "prompt_preset")
        .map(|revision| revision.value)
}

fn validate_dependency_ordinal(
    kind: &str,
    expected_ordinal: usize,
    stored_ordinal: i64,
) -> CoreResult<()> {
    let stored_ordinal = usize::try_from(stored_ordinal)
        .map_err(|_| storage_corrupted(format!("prompt preset {kind} ordinal is invalid")))?;
    if stored_ordinal != expected_ordinal {
        return Err(storage_corrupted(format!(
            "prompt preset {kind} ordinals are not contiguous"
        )));
    }
    Ok(())
}
