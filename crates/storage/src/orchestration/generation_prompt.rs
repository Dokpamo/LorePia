//! Immutable generation prompt-plan persistence and provenance.

use super::{
    ApiFamily, ConversationBranchId, ConversationId, CoreError, CoreResult, Deserialize,
    GenerationId, GenerationPresetId, GenerationPromptPlanRecord, KnowledgeActivationLog,
    MessageId, ModelRouteId, OptionalExtension, PromptPresetId, ProviderMessageRole,
    ProviderRequestSnapshotRecord, ResolvedPromptPlan, Serialize, Storage, Transaction, Value,
    VersionedJson, decode_document, enum_wire, load_generation_module_plan_evidence, not_found,
    params, parse_datetime, sha256_hex, storage_corrupted, storage_db_error, u64_revision,
    validate_identifier, validate_json_bounds, validate_optional_sha256,
    write_generation_knowledge_logs,
};

/// Storage-only counters used to prove orchestration transaction atomicity
/// without exposing raw `SQLite` access through app or FFI surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrchestrationDatabaseStats {
    pub generations: u64,
    pub generation_prompt_plans: u64,
    pub knowledge_activation_logs: u64,
}

impl Storage {
    /// Loads the immutable prompt provenance attached to one generation.
    pub fn get_generation_prompt_plan_by_generation(
        &self,
        generation_id: &GenerationId,
    ) -> CoreResult<GenerationPromptPlanRecord> {
        let connection = self.connection()?;
        let raw = connection
            .query_row(
                "SELECT plan.id, generation.id, plan.conversation_id, plan.branch_id,
                        plan.head_message_id, plan.latest_user_message_id,
                        plan.prompt_preset_id, plan.prompt_preset_revision_id,
                        plan.model_route_id, plan.generation_preset_id,
                        plan.task_profile_revision_id, plan.random_seed,
                        plan.tokenizer_id, plan.tokenizer_version,
                        plan.schema_version, plan.canonical_plan_json,
                        plan.plan_sha256, plan.input_fingerprint_sha256,
                        plan.context_limit_tokens, plan.estimated_input_tokens,
                        plan.reserved_output_tokens, plan.final_input_tokens,
                        plan.cacheable_prefix_tokens, plan.created_at,
                        snapshot.id, snapshot.api_family,
                        snapshot.request_schema_version, snapshot.request_json,
                        snapshot.request_sha256,
                        snapshot.mapping_diagnostics_json, snapshot.created_at
                 FROM generations AS generation
                 JOIN generation_prompt_plans AS plan
                   ON plan.id = generation.resolved_prompt_plan_id
                 JOIN generation_prompt_plan_seals AS seal
                   ON seal.plan_id = plan.id
                  AND seal.plan_sha256 = plan.plan_sha256
                 JOIN provider_request_snapshots AS snapshot
                   ON snapshot.id = generation.provider_request_snapshot_id
                  AND snapshot.plan_id = plan.id
                 WHERE generation.id = ?1
                   AND generation.prompt_plan_sha256 = plan.plan_sha256",
                [&generation_id.0],
                raw_generation_prompt_plan,
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("generation prompt plan"))?;
        decode_generation_prompt_plan_record(raw)
    }

    /// Returns bounded orchestration counters for atomicity assertions.
    pub fn orchestration_stats(&self) -> CoreResult<OrchestrationDatabaseStats> {
        let connection = self.connection()?;
        let count = |table: &str| -> CoreResult<u64> {
            let sql = format!("SELECT COUNT(*) FROM {table}");
            connection
                .query_row(&sql, [], |row| row.get::<_, i64>(0))
                .map_err(storage_db_error)
                .and_then(u64_revision)
        };
        Ok(OrchestrationDatabaseStats {
            generations: count("generations")?,
            generation_prompt_plans: count("generation_prompt_plans")?,
            knowledge_activation_logs: count("knowledge_activation_logs")?,
        })
    }
}

struct RawGenerationPromptPlan {
    plan_id: String,
    generation_id: String,
    conversation_id: String,
    branch_id: String,
    head_message_id: Option<String>,
    latest_user_message_id: String,
    prompt_preset_id: String,
    prompt_preset_revision_id: String,
    model_route_id: Option<String>,
    generation_preset_id: Option<String>,
    task_profile_revision_id: Option<String>,
    random_seed: Option<i64>,
    tokenizer_id: String,
    tokenizer_version: String,
    plan_schema_version: i64,
    canonical_plan_json: String,
    plan_sha256: String,
    input_fingerprint_sha256: String,
    context_limit_tokens: i64,
    estimated_input_tokens: i64,
    reserved_output_tokens: i64,
    final_input_tokens: i64,
    cacheable_prefix_tokens: i64,
    plan_created_at: String,
    request_id: String,
    api_family: String,
    request_schema_version: i64,
    request_json: String,
    request_sha256: String,
    mapping_diagnostics_json: String,
    request_created_at: String,
}

fn raw_generation_prompt_plan(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<RawGenerationPromptPlan> {
    Ok(RawGenerationPromptPlan {
        plan_id: row.get(0)?,
        generation_id: row.get(1)?,
        conversation_id: row.get(2)?,
        branch_id: row.get(3)?,
        head_message_id: row.get(4)?,
        latest_user_message_id: row.get(5)?,
        prompt_preset_id: row.get(6)?,
        prompt_preset_revision_id: row.get(7)?,
        model_route_id: row.get(8)?,
        generation_preset_id: row.get(9)?,
        task_profile_revision_id: row.get(10)?,
        random_seed: row.get(11)?,
        tokenizer_id: row.get(12)?,
        tokenizer_version: row.get(13)?,
        plan_schema_version: row.get(14)?,
        canonical_plan_json: row.get(15)?,
        plan_sha256: row.get(16)?,
        input_fingerprint_sha256: row.get(17)?,
        context_limit_tokens: row.get(18)?,
        estimated_input_tokens: row.get(19)?,
        reserved_output_tokens: row.get(20)?,
        final_input_tokens: row.get(21)?,
        cacheable_prefix_tokens: row.get(22)?,
        plan_created_at: row.get(23)?,
        request_id: row.get(24)?,
        api_family: row.get(25)?,
        request_schema_version: row.get(26)?,
        request_json: row.get(27)?,
        request_sha256: row.get(28)?,
        mapping_diagnostics_json: row.get(29)?,
        request_created_at: row.get(30)?,
    })
}

struct DecodedGenerationPromptPayload {
    plan_schema_version: u32,
    request_schema_version: u32,
    plan_value: Value,
    request_value: Value,
    resolved: ResolvedPromptPlan,
}

fn decode_generation_prompt_payload(
    raw: &RawGenerationPromptPlan,
) -> CoreResult<DecodedGenerationPromptPayload> {
    let plan_schema_version = u32::try_from(raw.plan_schema_version)
        .map_err(|_| storage_corrupted("stored prompt plan schema version is invalid"))?;
    let request_schema_version = u32::try_from(raw.request_schema_version)
        .map_err(|_| storage_corrupted("stored request schema version is invalid"))?;
    validate_stored_json("resolved prompt plan", &raw.canonical_plan_json)?;
    validate_stored_json("provider request snapshot", &raw.request_json)?;
    validate_stored_sha256("prompt plan", &raw.plan_sha256)?;
    validate_stored_sha256("request", &raw.request_sha256)?;
    if sha256_hex(raw.request_json.as_bytes()) != raw.request_sha256 {
        return Err(storage_corrupted(
            "stored provider request snapshot hash does not match its canonical JSON",
        ));
    }
    let plan_value = serde_json::from_str::<Value>(&raw.canonical_plan_json)
        .map_err(|error| storage_corrupted(format!("stored prompt plan is invalid: {error}")))?;
    let request_value = serde_json::from_str::<Value>(&raw.request_json).map_err(|error| {
        storage_corrupted(format!(
            "stored provider request snapshot is invalid: {error}"
        ))
    })?;
    let resolved =
        serde_json::from_value::<ResolvedPromptPlan>(plan_value.clone()).map_err(|error| {
            storage_corrupted(format!(
                "stored resolved prompt plan cannot be decoded: {error}"
            ))
        })?;
    if resolved.schema_version != plan_schema_version
        || resolved.plan_hash != raw.plan_sha256
        || resolved_prompt_plan_hash(&resolved).map_err(|error| {
            storage_corrupted(format!(
                "stored resolved prompt plan cannot be rehashed: {}",
                error.message
            ))
        })? != raw.plan_sha256
    {
        return Err(storage_corrupted(
            "stored resolved prompt plan hash or schema version is invalid",
        ));
    }
    Ok(DecodedGenerationPromptPayload {
        plan_schema_version,
        request_schema_version,
        plan_value,
        request_value,
        resolved,
    })
}

fn validate_stored_json(label: &str, value: &str) -> CoreResult<()> {
    validate_json_bounds(&format!("stored {label}"), value).map_err(|error| {
        storage_corrupted(format!(
            "stored {label} violates storage bounds: {}",
            error.message
        ))
    })
}

fn validate_stored_sha256(label: &str, value: &str) -> CoreResult<()> {
    validate_optional_sha256(&format!("stored {label} hash"), Some(value)).map_err(|error| {
        storage_corrupted(format!("stored {label} hash is invalid: {}", error.message))
    })
}

fn decode_generation_prompt_plan_record(
    raw: RawGenerationPromptPlan,
) -> CoreResult<GenerationPromptPlanRecord> {
    let decoded = decode_generation_prompt_payload(&raw)?;
    let record = GenerationPromptPlanRecord {
        id: raw.plan_id,
        generation_id: GenerationId(raw.generation_id),
        conversation_id: ConversationId(raw.conversation_id),
        branch_id: ConversationBranchId(raw.branch_id),
        head_message_id: raw.head_message_id.map(MessageId),
        latest_user_message_id: MessageId(raw.latest_user_message_id),
        prompt_preset_id: PromptPresetId::from(raw.prompt_preset_id),
        prompt_preset_revision_id: raw.prompt_preset_revision_id,
        model_route_id: raw.model_route_id.map(ModelRouteId::from),
        generation_preset_id: raw.generation_preset_id.map(GenerationPresetId::from),
        task_profile_revision_id: raw.task_profile_revision_id,
        random_seed: raw
            .random_seed
            .map(|value| {
                u64::try_from(value)
                    .map_err(|_| storage_corrupted("stored prompt random seed is invalid"))
            })
            .transpose()?,
        tokenizer_id: raw.tokenizer_id,
        tokenizer_version: raw.tokenizer_version,
        plan: VersionedJson {
            schema_version: decoded.plan_schema_version,
            value: decoded.plan_value,
        },
        plan_sha256: raw.plan_sha256,
        input_fingerprint_sha256: raw.input_fingerprint_sha256,
        context_limit_tokens: positive_u32("context limit", raw.context_limit_tokens)?,
        estimated_input_tokens: nonnegative_u32(
            "estimated input tokens",
            raw.estimated_input_tokens,
        )?,
        reserved_output_tokens: nonnegative_u32(
            "reserved output tokens",
            raw.reserved_output_tokens,
        )?,
        final_input_tokens: nonnegative_u32("final input tokens", raw.final_input_tokens)?,
        cacheable_prefix_tokens: nonnegative_u32(
            "cacheable prefix tokens",
            raw.cacheable_prefix_tokens,
        )?,
        provider_request: ProviderRequestSnapshotRecord {
            id: raw.request_id,
            api_family: parse_api_family(&raw.api_family)?,
            request_schema_version: decoded.request_schema_version,
            request: VersionedJson {
                schema_version: decoded.request_schema_version,
                value: decoded.request_value,
            },
            mapping_diagnostics: decode_document(
                "provider mapping diagnostics",
                &raw.mapping_diagnostics_json,
            )?,
            created_at: parse_datetime("provider request created_at", &raw.request_created_at)?,
        },
        created_at: parse_datetime("prompt plan created_at", &raw.plan_created_at)?,
    };
    validate_generation_prompt_plan_metadata(&record, &decoded.resolved)?;
    Ok(record)
}

fn validate_generation_prompt_plan_metadata(
    record: &GenerationPromptPlanRecord,
    resolved: &ResolvedPromptPlan,
) -> CoreResult<()> {
    let latest_user_included = resolved.effective_messages.iter().any(|message| {
        message.effective_role == ProviderMessageRole::User
            && message
                .source_message_ids
                .iter()
                .any(|id| id == &record.latest_user_message_id)
    });
    if resolved.preset_id != record.prompt_preset_id
        || resolved.generation_preset_id != record.generation_preset_id
        || resolved.trace.max_context_tokens != record.context_limit_tokens
        || resolved.trace.reserved_output_tokens != record.reserved_output_tokens
        || resolved.trace.estimated_input_tokens != record.estimated_input_tokens
        || record.final_input_tokens != resolved.trace.estimated_input_tokens
        || !latest_user_included
    {
        return Err(storage_corrupted(
            "stored resolved prompt plan metadata does not match its canonical body",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub(crate) struct GenerationPromptPlanLink {
    pub plan_id: String,
    pub plan_sha256: String,
    pub provider_request_snapshot_id: String,
}

struct PreparedGenerationPromptPlan {
    resolved: ResolvedPromptPlan,
    canonical_plan_json: String,
    request_json: String,
    mapping_diagnostics_json: String,
    request_sha256: String,
    random_seed: Option<i64>,
}

fn prepare_generation_prompt_plan(
    transaction: &Transaction<'_>,
    record: &GenerationPromptPlanRecord,
) -> CoreResult<PreparedGenerationPromptPlan> {
    validate_identifier("generation prompt plan", &record.id)?;
    validate_identifier("provider request snapshot", &record.provider_request.id)?;
    validate_optional_sha256("prompt plan hash", Some(&record.plan_sha256))?;
    validate_optional_sha256(
        "prompt input fingerprint",
        Some(&record.input_fingerprint_sha256),
    )?;
    if record.context_limit_tokens == 0
        || record
            .final_input_tokens
            .saturating_add(record.reserved_output_tokens)
            > record.context_limit_tokens
    {
        return Err(CoreError::invalid(
            "resolved prompt plan exceeds its context token limit",
        ));
    }
    let resolved: ResolvedPromptPlan = serde_json::from_value(record.plan.value.clone())
        .map_err(|error| CoreError::invalid(format!("resolved prompt plan is invalid: {error}")))?;
    validate_prepared_generation_prompt_plan_metadata(record, &resolved)?;
    let canonical_plan_json = serde_json::to_string(&record.plan.value).map_err(|error| {
        CoreError::invalid(format!("cannot encode resolved prompt plan: {error}"))
    })?;
    validate_json_bounds("resolved prompt plan", &canonical_plan_json)?;
    let request_json =
        serde_json::to_string(&record.provider_request.request.value).map_err(|error| {
            CoreError::invalid(format!("cannot encode provider request snapshot: {error}"))
        })?;
    validate_json_bounds("provider request snapshot", &request_json)?;
    let mapping_diagnostics_json =
        serde_json::to_string(&record.provider_request.mapping_diagnostics).map_err(|error| {
            CoreError::invalid(format!(
                "cannot encode provider mapping diagnostics: {error}"
            ))
        })?;
    validate_json_bounds("provider mapping diagnostics", &mapping_diagnostics_json)?;
    // Prompt-only and transform-only module overlays must also pass the exact
    // append-time module-plan identity check.
    let _ = load_generation_module_plan_evidence(transaction, record)?;
    let request_sha256 = sha256_hex(request_json.as_bytes());
    let random_seed = record
        .random_seed
        .map(|value| {
            i64::try_from(value)
                .map_err(|_| CoreError::invalid("prompt random seed exceeds SQLite range"))
        })
        .transpose()?;
    Ok(PreparedGenerationPromptPlan {
        resolved,
        canonical_plan_json,
        request_json,
        mapping_diagnostics_json,
        request_sha256,
        random_seed,
    })
}

fn validate_prepared_generation_prompt_plan_metadata(
    record: &GenerationPromptPlanRecord,
    resolved: &ResolvedPromptPlan,
) -> CoreResult<()> {
    if record.plan.schema_version != resolved.schema_version
        || resolved.plan_hash != record.plan_sha256
        || resolved_prompt_plan_hash(resolved)? != record.plan_sha256
    {
        return Err(CoreError::invalid(
            "resolved prompt plan hash or schema version does not match",
        ));
    }
    if resolved.preset_id != record.prompt_preset_id
        || resolved.generation_preset_id != record.generation_preset_id
        || resolved.trace.max_context_tokens != record.context_limit_tokens
        || resolved.trace.reserved_output_tokens != record.reserved_output_tokens
        || resolved.trace.estimated_input_tokens != record.estimated_input_tokens
    {
        return Err(CoreError::invalid(
            "resolved prompt plan metadata does not match its canonical body",
        ));
    }
    if record.final_input_tokens != resolved.trace.estimated_input_tokens {
        return Err(CoreError::invalid(
            "final input token count does not match the resolved prompt plan",
        ));
    }
    let latest_user_included = resolved.effective_messages.iter().any(|message| {
        message.effective_role == ProviderMessageRole::User
            && message
                .source_message_ids
                .iter()
                .any(|id| id == &record.latest_user_message_id)
    });
    if !latest_user_included {
        return Err(CoreError::invalid(
            "resolved prompt plan does not include the latest user message",
        ));
    }
    if record.model_route_id.is_some() != record.generation_preset_id.is_some() {
        return Err(CoreError::invalid(
            "prompt plan route and generation preset must be present together",
        ));
    }
    Ok(())
}

fn insert_generation_prompt_plan(
    transaction: &Transaction<'_>,
    record: &GenerationPromptPlanRecord,
    prepared: &PreparedGenerationPromptPlan,
) -> CoreResult<()> {
    transaction
        .execute(
            "INSERT INTO generation_prompt_plans
             (id, schema_version, plan_sha256, input_fingerprint_sha256,
              conversation_id, branch_id, head_message_id,
              latest_user_message_id, latest_user_included, prompt_preset_id,
              prompt_preset_revision_id, generation_preset_id, model_route_id,
              task_profile_revision_id, random_seed, tokenizer_id,
              tokenizer_version, context_limit_tokens, reserved_output_tokens,
              estimated_input_tokens, final_input_tokens, message_count,
              cacheable_prefix_tokens, status, canonical_plan_json, sealed_at,
              created_at)
             VALUES (
                 ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?10, ?11, ?12,
                 ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22,
                 'resolved', ?23, ?24, ?24
             )",
            params![
                record.id,
                record.plan.schema_version,
                record.plan_sha256,
                record.input_fingerprint_sha256,
                record.conversation_id.0,
                record.branch_id.0,
                record.head_message_id.as_ref().map(|id| id.0.as_str()),
                record.latest_user_message_id.0,
                record.prompt_preset_id.as_str(),
                record.prompt_preset_revision_id,
                record
                    .generation_preset_id
                    .as_ref()
                    .map(GenerationPresetId::as_str),
                record.model_route_id.as_ref().map(ModelRouteId::as_str),
                record.task_profile_revision_id,
                prepared.random_seed,
                record.tokenizer_id,
                record.tokenizer_version,
                record.context_limit_tokens,
                record.reserved_output_tokens,
                record.estimated_input_tokens,
                record.final_input_tokens,
                i64::try_from(prepared.resolved.effective_messages.len())
                    .map_err(|_| CoreError::invalid("too many prompt messages"))?,
                record.cacheable_prefix_tokens,
                prepared.canonical_plan_json,
                record.created_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn seal_generation_prompt_plan(
    transaction: &Transaction<'_>,
    record: &GenerationPromptPlanRecord,
    prepared: &PreparedGenerationPromptPlan,
) -> CoreResult<()> {
    transaction
        .execute(
            "INSERT INTO generation_prompt_plan_seals
             (plan_id, plan_sha256, sealed_at) VALUES (?1, ?2, ?3)",
            params![
                record.id,
                record.plan_sha256,
                record.created_at.to_rfc3339()
            ],
        )
        .map_err(storage_db_error)?;
    transaction
        .execute(
            "INSERT INTO provider_request_snapshots
             (id, plan_id, api_family, request_schema_version, request_json,
              request_sha256, mapping_diagnostics_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                record.provider_request.id,
                record.id,
                api_family_str(record.provider_request.api_family),
                record.provider_request.request_schema_version,
                prepared.request_json,
                prepared.request_sha256,
                prepared.mapping_diagnostics_json,
                record.provider_request.created_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

pub(crate) fn write_generation_prompt_plan(
    transaction: &Transaction<'_>,
    record: &GenerationPromptPlanRecord,
    knowledge_logs: &[KnowledgeActivationLog],
) -> CoreResult<GenerationPromptPlanLink> {
    let prepared = prepare_generation_prompt_plan(transaction, record)?;
    insert_generation_prompt_plan(transaction, record, &prepared)?;
    write_resolved_prompt_children(transaction, &record.id, &prepared.resolved)?;
    write_generation_knowledge_logs(transaction, record, knowledge_logs)?;
    seal_generation_prompt_plan(transaction, record, &prepared)?;
    Ok(GenerationPromptPlanLink {
        plan_id: record.id.clone(),
        plan_sha256: record.plan_sha256.clone(),
        provider_request_snapshot_id: record.provider_request.id.clone(),
    })
}

fn resolved_prompt_plan_hash(plan: &ResolvedPromptPlan) -> CoreResult<String> {
    #[derive(Serialize)]
    struct HashMaterial<'a> {
        schema_version: u32,
        preset_id: &'a PromptPresetId,
        generation_preset_id: &'a Option<GenerationPresetId>,
        effective_messages: &'a [lorepia_domain::ResolvedPromptMessage],
        cache_directives: &'a [lorepia_domain::ResolvedCacheDirective],
        trace: &'a lorepia_domain::PromptResolutionTrace,
        preview: &'a lorepia_domain::PromptPreview,
    }
    let material = HashMaterial {
        schema_version: plan.schema_version,
        preset_id: &plan.preset_id,
        generation_preset_id: &plan.generation_preset_id,
        effective_messages: &plan.effective_messages,
        cache_directives: &plan.cache_directives,
        trace: &plan.trace,
        preview: &plan.preview,
    };
    serde_json::to_vec(&material)
        .map(|bytes| sha256_hex(&bytes))
        .map_err(|error| CoreError::invalid(format!("cannot hash resolved prompt plan: {error}")))
}

fn write_resolved_prompt_children(
    transaction: &Transaction<'_>,
    plan_id: &str,
    resolved: &ResolvedPromptPlan,
) -> CoreResult<()> {
    write_resolved_prompt_messages(transaction, plan_id, resolved)?;
    write_resolved_cache_directives(transaction, plan_id, resolved)?;
    write_resolved_prompt_warnings(transaction, plan_id, resolved)
}

fn write_resolved_prompt_messages(
    transaction: &Transaction<'_>,
    plan_id: &str,
    resolved: &ResolvedPromptPlan,
) -> CoreResult<()> {
    for (ordinal, message) in resolved.effective_messages.iter().enumerate() {
        let ordinal = i64::try_from(ordinal)
            .map_err(|_| CoreError::invalid("too many resolved prompt messages"))?;
        let trace = resolved
            .trace
            .blocks
            .iter()
            .find(|trace| trace.block_id == message.block_id);
        let provenance_json = serde_json::to_string(&message.provenance).map_err(|error| {
            CoreError::invalid(format!("cannot encode prompt provenance: {error}"))
        })?;
        let payload_json = serde_json::to_string(message).map_err(|error| {
            CoreError::invalid(format!("cannot encode resolved prompt message: {error}"))
        })?;
        transaction
            .execute(
                "INSERT INTO generation_prompt_plan_blocks
                 (plan_id, ordinal, source_owner_revision_id, source_block_id,
                  kind, placement_zone, requested_role, disposition,
                  reduction_reason_json, content, content_sha256,
                  estimated_tokens, final_tokens, provenance_json, payload_json)
                 VALUES (
                     ?1, ?2, NULL, NULL, ?3, 'resolved', ?4, ?5, NULL,
                     ?6, ?7, ?8, ?8, ?9, ?10
                 )",
                params![
                    plan_id,
                    ordinal,
                    enum_wire(&message.block_kind)?,
                    enum_wire(&message.requested_role)?,
                    trace.map_or("included", |trace| {
                        block_resolution_disposition(trace.status)
                    }),
                    message.content,
                    sha256_hex(message.content.as_bytes()),
                    message.estimated_tokens,
                    provenance_json,
                    payload_json,
                ],
            )
            .map_err(storage_db_error)?;
        transaction
            .execute(
                "INSERT INTO generation_prompt_plan_messages
                 (plan_id, ordinal, role, content, content_sha256,
                  source_block_ordinals_json, source_message_id,
                  estimated_tokens)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    plan_id,
                    ordinal,
                    enum_wire(&message.effective_role)?,
                    message.content,
                    sha256_hex(message.content.as_bytes()),
                    format!("[{ordinal}]"),
                    message.source_message_ids.first().map(|id| id.0.as_str()),
                    message.estimated_tokens,
                ],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

fn write_resolved_cache_directives(
    transaction: &Transaction<'_>,
    plan_id: &str,
    resolved: &ResolvedPromptPlan,
) -> CoreResult<()> {
    for (ordinal, directive) in resolved.cache_directives.iter().enumerate() {
        let directive_json = serde_json::to_string(directive).map_err(|error| {
            CoreError::invalid(format!("cannot encode cache directive: {error}"))
        })?;
        let (disposition, warning_code) = match directive.status {
            lorepia_domain::CacheDirectiveStatus::Applied => ("applied", None),
            lorepia_domain::CacheDirectiveStatus::IgnoredUnsupported => {
                ("ignored", Some("unsupported"))
            }
            lorepia_domain::CacheDirectiveStatus::IgnoredLimit => ("ignored", Some("limit")),
            lorepia_domain::CacheDirectiveStatus::RemovedWithBlock => {
                ("ignored", Some("removed_with_block"))
            }
        };
        transaction
            .execute(
                "INSERT INTO generation_prompt_plan_directives
                 (plan_id, ordinal, directive_kind, source_owner_revision_id,
                  source_boundary_id, directive_json, disposition,
                  provider_mapping_json, warning_code)
                 VALUES (?1, ?2, 'cache', NULL, NULL, ?3, ?4, NULL, ?5)",
                params![
                    plan_id,
                    i64::try_from(ordinal)
                        .map_err(|_| CoreError::invalid("too many cache directives"))?,
                    directive_json,
                    disposition,
                    warning_code,
                ],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

fn write_resolved_prompt_warnings(
    transaction: &Transaction<'_>,
    plan_id: &str,
    resolved: &ResolvedPromptPlan,
) -> CoreResult<()> {
    for (ordinal, warning) in resolved.trace.warnings.iter().enumerate() {
        transaction
            .execute(
                "INSERT INTO generation_prompt_plan_warnings
                 (plan_id, ordinal, code, severity, message_key, details_json)
                 VALUES (?1, ?2, 'resolver_warning', 'warning', ?3, '{}')",
                params![
                    plan_id,
                    i64::try_from(ordinal)
                        .map_err(|_| CoreError::invalid("too many prompt warnings"))?,
                    warning
                ],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

const fn block_resolution_disposition(
    status: lorepia_domain::BlockResolutionStatus,
) -> &'static str {
    match status {
        lorepia_domain::BlockResolutionStatus::Included => "included",
        lorepia_domain::BlockResolutionStatus::TrimmedHead => "trimmed_head",
        lorepia_domain::BlockResolutionStatus::TrimmedTail
        | lorepia_domain::BlockResolutionStatus::ReducedItems => "trimmed_tail",
        lorepia_domain::BlockResolutionStatus::Summarized => "summarized",
        lorepia_domain::BlockResolutionStatus::ConditionFalse
        | lorepia_domain::BlockResolutionStatus::Disabled
        | lorepia_domain::BlockResolutionStatus::Empty
        | lorepia_domain::BlockResolutionStatus::DroppedForBudget => "dropped",
    }
}

fn parse_api_family(value: &str) -> CoreResult<ApiFamily> {
    match value {
        "open_ai_responses" | "openai_responses" => Ok(ApiFamily::OpenAiResponses),
        "open_ai_chat_completions" | "openai_chat_completions" => {
            Ok(ApiFamily::OpenAiChatCompletions)
        }
        "anthropic_messages" => Ok(ApiFamily::AnthropicMessages),
        "gemini_generate_content" => Ok(ApiFamily::GeminiGenerateContent),
        "ollama_native" => Ok(ApiFamily::OllamaNative),
        _ => Err(storage_corrupted(format!(
            "stored provider API family is invalid: {value}"
        ))),
    }
}

const fn api_family_str(value: ApiFamily) -> &'static str {
    match value {
        ApiFamily::OpenAiResponses => "openai_responses",
        ApiFamily::OpenAiChatCompletions => "openai_chat_completions",
        ApiFamily::AnthropicMessages => "anthropic_messages",
        ApiFamily::GeminiGenerateContent => "gemini_generate_content",
        ApiFamily::OllamaNative => "ollama_native",
    }
}

pub(super) fn nonnegative_u32(label: &str, value: i64) -> CoreResult<u32> {
    u32::try_from(value).map_err(|_| storage_corrupted(format!("stored {label} is invalid")))
}

fn positive_u32(label: &str, value: i64) -> CoreResult<u32> {
    let value = nonnegative_u32(label, value)?;
    if value == 0 {
        Err(storage_corrupted(format!("stored {label} is zero")))
    } else {
        Ok(value)
    }
}
