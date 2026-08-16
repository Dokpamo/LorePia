use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use lorepia_domain::{
    CoreError, CoreErrorCode, CoreResult, GenerationId, Message, MessageId, MessageRole,
    MessageStatus, Sha256Digest,
};
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::database::{Storage, storage_db_error};

/// Display transforms share the pure engine's 256 Ki-character ceiling. UTF-8
/// storage is additionally capped at four bytes per scalar value.
pub const MAX_MESSAGE_DISPLAY_PROJECTION_CHARS: usize = 256 * 1_024;
pub const MAX_MESSAGE_DISPLAY_PROJECTION_BYTES: usize = MAX_MESSAGE_DISPLAY_PROJECTION_CHARS * 4;
pub const MAX_MESSAGE_TRANSFORM_APPLICATIONS: usize = 256;
pub const MAX_MESSAGE_TRANSFORM_PIPELINE_FAILURES: usize = 2;

const MAX_DIAGNOSTIC_IDENTIFIER_BYTES: usize = 512;
const MAX_DIAGNOSTIC_IDENTIFIER_CHARS: usize = 256;
const DIAGNOSTIC_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageTransformStage {
    ProviderOutputCanonical,
    DisplayOnly,
}

impl MessageTransformStage {
    const fn as_str(self) -> &'static str {
        match self {
            Self::ProviderOutputCanonical => "provider_output_canonical",
            Self::DisplayOnly => "display_only",
        }
    }

    fn parse(value: &str) -> CoreResult<Self> {
        match value {
            "provider_output_canonical" => Ok(Self::ProviderOutputCanonical),
            "display_only" => Ok(Self::DisplayOnly),
            _ => Err(storage_corrupted("stored transform stage is invalid")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageTransformDisposition {
    Applied,
    NoMatch,
    Disabled,
    PendingImportApproval,
    ResolvedPromptDisabled,
    ConditionFalse,
    Failed,
    LimitRejected,
    PipelineRejected,
}

impl MessageTransformDisposition {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::NoMatch => "no_match",
            Self::Disabled => "disabled",
            Self::PendingImportApproval => "pending_import_approval",
            Self::ResolvedPromptDisabled => "resolved_prompt_disabled",
            Self::ConditionFalse => "condition_false",
            Self::Failed => "failed",
            Self::LimitRejected => "limit_rejected",
            Self::PipelineRejected => "pipeline_rejected",
        }
    }

    fn stored_status(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Failed | Self::PipelineRejected => "failed",
            Self::LimitRejected => "limit_rejected",
            Self::NoMatch
            | Self::Disabled
            | Self::PendingImportApproval
            | Self::ResolvedPromptDisabled
            | Self::ConditionFalse => "no_match",
        }
    }

    fn parse(value: &str) -> CoreResult<Self> {
        match value {
            "applied" => Ok(Self::Applied),
            "no_match" => Ok(Self::NoMatch),
            "disabled" => Ok(Self::Disabled),
            "pending_import_approval" => Ok(Self::PendingImportApproval),
            "resolved_prompt_disabled" => Ok(Self::ResolvedPromptDisabled),
            "condition_false" => Ok(Self::ConditionFalse),
            "failed" => Ok(Self::Failed),
            "limit_rejected" => Ok(Self::LimitRejected),
            "pipeline_rejected" => Ok(Self::PipelineRejected),
            _ => Err(storage_corrupted(
                "stored transform diagnostic disposition is invalid",
            )),
        }
    }
}

/// Trusted Core-to-storage write for one rule report. Text and free-form error
/// messages are intentionally absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageTransformApplicationWrite {
    pub set_id: String,
    pub rule_id: String,
    pub stage: MessageTransformStage,
    pub disposition: MessageTransformDisposition,
    pub code: Option<String>,
    pub before_sha256: Sha256Digest,
    pub after_sha256: Option<Sha256Digest>,
    pub replacement_count: u32,
    pub input_chars: u32,
    pub output_chars: u32,
}

/// Content-free failure evidence for a phase-level rejection that produced no
/// per-rule report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageTransformPipelineFailureWrite {
    pub stage: MessageTransformStage,
    pub code: String,
    pub before_sha256: Sha256Digest,
}

/// Bounded display text and content-free diagnostics prepared by Core for the
/// same atomic transaction as the terminal assistant message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageDisplayProjectionWrite {
    pub display_content: String,
    pub applications: Vec<MessageTransformApplicationWrite>,
    pub pipeline_failures: Vec<MessageTransformPipelineFailureWrite>,
}

/// Public diagnostic projection. It deliberately omits counts, snippets,
/// patterns, replacements, variables, capabilities, and free-form errors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageTransformDiagnostic {
    pub set_revision_id: Option<String>,
    pub rule_id: Option<String>,
    pub stage: MessageTransformStage,
    pub disposition: MessageTransformDisposition,
    pub code: Option<String>,
    pub before_sha256: Sha256Digest,
    pub after_sha256: Option<Sha256Digest>,
    pub recorded_at: DateTime<Utc>,
}

/// Hash-verified display representation loaded alongside an unchanged
/// canonical [`Message::content`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredMessageDisplayProjection {
    pub message_id: MessageId,
    pub generation_id: GenerationId,
    pub display_content: String,
    pub canonical_content_sha256: Sha256Digest,
    pub display_content_sha256: Sha256Digest,
    pub diagnostics_sha256: Sha256Digest,
    pub diagnostics: Vec<MessageTransformDiagnostic>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredPipelineDiagnostics {
    schema_version: u32,
    failures: Vec<MessageTransformPipelineFailureWrite>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredRuleDiagnostics {
    schema_version: u32,
    set_id: String,
    disposition: String,
    code: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct DiagnosticDigestDocument<'a> {
    schema_version: u32,
    diagnostics: &'a [MessageTransformDiagnostic],
}

#[derive(Debug)]
struct ResolvedApplication<'a> {
    write: &'a MessageTransformApplicationWrite,
    set_revision_id: String,
}

#[derive(Debug)]
struct StoredRuleDiagnosticRow {
    set_revision_id: String,
    set_id: String,
    rule_id: String,
    phase: String,
    status: String,
    before_sha256: String,
    after_sha256: Option<String>,
    error_code: Option<String>,
    diagnostics_json: String,
    created_at: String,
}

/// Persists the display sidecar and every generation-linked rule report in the
/// caller's terminal transaction. The canonical assistant content is only
/// hashed and verified; it is never replaced here.
pub(crate) fn persist_terminal_message_display_projection(
    transaction: &Transaction<'_>,
    assistant: &Message,
    write: Option<&MessageDisplayProjectionWrite>,
    created_at: DateTime<Utc>,
) -> CoreResult<()> {
    let Some(write) = write else {
        return Ok(());
    };
    validate_terminal_projection_owner(assistant)?;
    validate_projection_write(write)?;
    let generation_id = assistant
        .generation_id
        .as_ref()
        .ok_or_else(|| CoreError::invalid("display projection requires a generation"))?;
    let revisions = load_generation_transform_revisions(transaction, generation_id)?;
    let mut resolved = Vec::with_capacity(write.applications.len());
    for application in &write.applications {
        let set_revision_id = revisions.get(&application.set_id).ok_or_else(|| {
            storage_corrupted("generation transform report references an unsealed transform set")
        })?;
        validate_exact_transform_rule(
            transaction,
            &application.set_id,
            set_revision_id,
            &application.rule_id,
        )?;
        resolved.push(ResolvedApplication {
            write: application,
            set_revision_id: set_revision_id.clone(),
        });
    }

    let canonical_content_sha256 = sha256_digest(assistant.content.as_bytes())?;
    let display_content_sha256 = sha256_digest(write.display_content.as_bytes())?;
    validate_transform_hash_chain(write, &canonical_content_sha256, &display_content_sha256)?;
    let pipeline = StoredPipelineDiagnostics {
        schema_version: DIAGNOSTIC_SCHEMA_VERSION,
        failures: write.pipeline_failures.clone(),
    };
    let pipeline_json = serde_json::to_string(&pipeline).map_err(|error| {
        CoreError::internal(format!(
            "cannot encode transform pipeline diagnostics: {error}"
        ))
    })?;
    let diagnostics = materialize_diagnostics(&resolved, &write.pipeline_failures, created_at)?;
    let diagnostics_sha256 = diagnostics_sha256(&diagnostics)?;

    transaction
        .execute(
            "INSERT INTO message_display_projections
             (message_id, generation_id, canonical_content_sha256,
              display_content, display_content_sha256,
              pipeline_diagnostics_json, diagnostics_sha256, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                assistant.id.0,
                generation_id.0,
                canonical_content_sha256.as_str(),
                write.display_content,
                display_content_sha256.as_str(),
                pipeline_json,
                diagnostics_sha256.as_str(),
                created_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;

    for (ordinal, application) in resolved.iter().enumerate() {
        insert_transform_application_log(
            transaction,
            assistant,
            generation_id,
            ordinal,
            application,
            created_at,
        )?;
    }
    Ok(())
}

impl Storage {
    /// Loads and verifies a sidecar for one already-loaded canonical message.
    /// A missing sidecar is a legitimate identity projection.
    pub fn get_message_display_projection(
        &self,
        message: &Message,
    ) -> CoreResult<Option<StoredMessageDisplayProjection>> {
        let connection = self.connection()?;
        let row = connection
            .query_row(
                "SELECT generation_id, canonical_content_sha256, display_content,
                        display_content_sha256, pipeline_diagnostics_json,
                        diagnostics_sha256, created_at
                 FROM message_display_projections
                 WHERE message_id = ?1",
                [&message.id.0],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_db_error)?;
        let Some(row) = row else {
            return Ok(None);
        };
        validate_terminal_projection_owner(message).map_err(|_| {
            storage_corrupted("display projection belongs to a nonterminal assistant message")
        })?;
        let generation_id = GenerationId(row.0);
        if message.generation_id.as_ref() != Some(&generation_id) {
            return Err(storage_corrupted(
                "display projection generation ownership is inconsistent",
            ));
        }
        validate_display_content(&row.2)
            .map_err(|_| storage_corrupted("stored display projection violates content bounds"))?;
        let canonical_content_sha256 = parse_sha256("canonical content", row.1)?;
        let display_content_sha256 = parse_sha256("display content", row.3)?;
        let diagnostics_sha256_stored = parse_sha256("transform diagnostics", row.5)?;
        if sha256_digest(message.content.as_bytes())? != canonical_content_sha256
            || sha256_digest(row.2.as_bytes())? != display_content_sha256
        {
            return Err(storage_corrupted(
                "stored display projection content hash is inconsistent",
            ));
        }
        let pipeline: StoredPipelineDiagnostics = serde_json::from_str(&row.4)
            .map_err(|_| storage_corrupted("stored transform pipeline diagnostics are invalid"))?;
        validate_pipeline_failures(&pipeline.failures).map_err(|_| {
            storage_corrupted("stored transform pipeline diagnostics violate bounds")
        })?;
        if pipeline.schema_version != DIAGNOSTIC_SCHEMA_VERSION {
            return Err(storage_corrupted(
                "stored transform pipeline diagnostics schema is unsupported",
            ));
        }
        let created_at = parse_datetime("display projection created_at", &row.6)?;
        let mut diagnostics =
            load_rule_diagnostics(&connection, &message.id, &generation_id, created_at)?;
        diagnostics.extend(
            pipeline
                .failures
                .iter()
                .map(|failure| MessageTransformDiagnostic {
                    set_revision_id: None,
                    rule_id: None,
                    stage: failure.stage,
                    disposition: MessageTransformDisposition::PipelineRejected,
                    code: Some(failure.code.clone()),
                    before_sha256: failure.before_sha256.clone(),
                    after_sha256: None,
                    recorded_at: created_at,
                }),
        );
        sort_diagnostics(&mut diagnostics);
        if diagnostics_sha256(&diagnostics)? != diagnostics_sha256_stored {
            return Err(storage_corrupted(
                "stored transform diagnostics hash is inconsistent",
            ));
        }
        Ok(Some(StoredMessageDisplayProjection {
            message_id: message.id.clone(),
            generation_id,
            display_content: row.2,
            canonical_content_sha256,
            display_content_sha256,
            diagnostics_sha256: diagnostics_sha256_stored,
            diagnostics,
            created_at,
        }))
    }
}

fn validate_terminal_projection_owner(assistant: &Message) -> CoreResult<()> {
    if assistant.role != MessageRole::Assistant || assistant.status == MessageStatus::Pending {
        return Err(CoreError::invalid(
            "display projection requires a terminal assistant message",
        ));
    }
    if assistant.generation_id.is_none() {
        return Err(CoreError::invalid(
            "display projection requires generation ownership",
        ));
    }
    Ok(())
}

fn validate_projection_write(write: &MessageDisplayProjectionWrite) -> CoreResult<()> {
    validate_display_content(&write.display_content)?;
    if write.applications.len() > MAX_MESSAGE_TRANSFORM_APPLICATIONS {
        return Err(CoreError::invalid(
            "display transform application count exceeds its bound",
        ));
    }
    validate_pipeline_failures(&write.pipeline_failures)?;
    for application in &write.applications {
        validate_identifier("transform set", &application.set_id)?;
        validate_identifier("transform rule", &application.rule_id)?;
        validate_code(application.code.as_deref())?;
        let expected_status = application.disposition.stored_status();
        if application.disposition == MessageTransformDisposition::PipelineRejected
            || (expected_status == "failed" && application.code.is_none())
            || (expected_status != "failed"
                && expected_status != "limit_rejected"
                && application.code.is_some())
            || (matches!(expected_status, "failed" | "limit_rejected")
                && application.after_sha256.is_some())
            || (!matches!(expected_status, "failed" | "limit_rejected")
                && application.after_sha256.is_none())
        {
            return Err(CoreError::invalid(
                "display transform application diagnostic is inconsistent",
            ));
        }
    }
    Ok(())
}

fn validate_pipeline_failures(failures: &[MessageTransformPipelineFailureWrite]) -> CoreResult<()> {
    if failures.len() > MAX_MESSAGE_TRANSFORM_PIPELINE_FAILURES {
        return Err(CoreError::invalid(
            "display transform pipeline failure count exceeds its bound",
        ));
    }
    let mut stages = std::collections::BTreeSet::new();
    for failure in failures {
        validate_code(Some(&failure.code))?;
        if !stages.insert(failure.stage.as_str()) {
            return Err(CoreError::invalid(
                "display transform pipeline failure stage is duplicated",
            ));
        }
    }
    Ok(())
}

fn validate_transform_hash_chain(
    write: &MessageDisplayProjectionWrite,
    canonical_content_sha256: &Sha256Digest,
    display_content_sha256: &Sha256Digest,
) -> CoreResult<()> {
    let mut saw_display_stage = false;
    let mut canonical_cursor: Option<&Sha256Digest> = None;
    let mut display_cursor = canonical_content_sha256;
    for application in &write.applications {
        match application.stage {
            MessageTransformStage::ProviderOutputCanonical => {
                if saw_display_stage {
                    return Err(CoreError::invalid(
                        "canonical transform diagnostics cannot follow DisplayOnly diagnostics",
                    ));
                }
                if canonical_cursor.is_some_and(|cursor| cursor != &application.before_sha256) {
                    return Err(CoreError::invalid(
                        "canonical transform diagnostic hash chain is inconsistent",
                    ));
                }
                canonical_cursor = Some(
                    application
                        .after_sha256
                        .as_ref()
                        .unwrap_or(&application.before_sha256),
                );
            }
            MessageTransformStage::DisplayOnly => {
                saw_display_stage = true;
                if display_cursor != &application.before_sha256 {
                    return Err(CoreError::invalid(
                        "DisplayOnly transform diagnostic hash chain is inconsistent",
                    ));
                }
                display_cursor = application
                    .after_sha256
                    .as_ref()
                    .unwrap_or(&application.before_sha256);
            }
        }
    }
    if canonical_cursor.is_some_and(|cursor| cursor != canonical_content_sha256)
        || display_cursor != display_content_sha256
    {
        return Err(CoreError::invalid(
            "transform diagnostic hash chain does not match terminal content",
        ));
    }
    for failure in &write.pipeline_failures {
        let expected = match failure.stage {
            MessageTransformStage::ProviderOutputCanonical => canonical_content_sha256,
            MessageTransformStage::DisplayOnly => {
                if display_content_sha256 != canonical_content_sha256 {
                    return Err(CoreError::invalid(
                        "a rejected DisplayOnly pipeline cannot change display content",
                    ));
                }
                canonical_content_sha256
            }
        };
        if &failure.before_sha256 != expected
            || write
                .applications
                .iter()
                .any(|application| application.stage == failure.stage)
        {
            return Err(CoreError::invalid(
                "transform pipeline failure evidence is inconsistent",
            ));
        }
    }
    Ok(())
}

fn validate_display_content(value: &str) -> CoreResult<()> {
    if value.len() > MAX_MESSAGE_DISPLAY_PROJECTION_BYTES
        || value.chars().count() > MAX_MESSAGE_DISPLAY_PROJECTION_CHARS
    {
        return Err(CoreError::invalid(
            "display projection exceeds its byte or character bound",
        ));
    }
    Ok(())
}

fn validate_identifier(label: &str, value: &str) -> CoreResult<()> {
    if value.is_empty()
        || value.len() > MAX_DIAGNOSTIC_IDENTIFIER_BYTES
        || value.chars().count() > MAX_DIAGNOSTIC_IDENTIFIER_CHARS
    {
        return Err(CoreError::invalid(format!(
            "{label} diagnostic identifier is invalid"
        )));
    }
    Ok(())
}

fn validate_code(value: Option<&str>) -> CoreResult<()> {
    if let Some(value) = value {
        validate_identifier("transform code", value)?;
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(CoreError::invalid("transform diagnostic code is invalid"));
        }
    }
    Ok(())
}

fn load_generation_transform_revisions(
    transaction: &Transaction<'_>,
    generation_id: &GenerationId,
) -> CoreResult<BTreeMap<String, String>> {
    let raw = transaction
        .query_row(
            "SELECT snapshot.mapping_diagnostics_json
             FROM generations AS generation
             JOIN provider_request_snapshots AS snapshot
               ON snapshot.id = generation.provider_request_snapshot_id
             WHERE generation.id = ?1",
            [&generation_id.0],
            |row| row.get::<_, String>(0),
        )
        .map_err(storage_db_error)?;
    let document: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|_| storage_corrupted("generation provider mapping diagnostics are invalid"))?;
    let revisions = document
        .get("value")
        .and_then(|value| value.get("transform_set_revisions"))
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            storage_corrupted("generation lacks exact transform revision diagnostics")
        })?;
    if revisions.len() > 64 {
        return Err(storage_corrupted(
            "generation transform revision count exceeds its bound",
        ));
    }
    let mut result = BTreeMap::new();
    for revision in revisions {
        let set_id = revision
            .get("set_id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| storage_corrupted("generation transform set identity is invalid"))?;
        let revision_id = revision
            .get("revision_id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                storage_corrupted("generation transform revision identity is invalid")
            })?;
        validate_identifier("transform set", set_id)
            .map_err(|_| storage_corrupted("generation transform set identity violates bounds"))?;
        validate_identifier("transform revision", revision_id).map_err(|_| {
            storage_corrupted("generation transform revision identity violates bounds")
        })?;
        if result
            .insert(set_id.to_owned(), revision_id.to_owned())
            .is_some()
        {
            return Err(storage_corrupted(
                "generation transform revision identity is duplicated",
            ));
        }
    }
    Ok(result)
}

fn validate_exact_transform_rule(
    transaction: &Transaction<'_>,
    set_id: &str,
    revision_id: &str,
    rule_id: &str,
) -> CoreResult<()> {
    let exists = transaction
        .query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM transform_set_revisions AS set_revision
                JOIN transform_rules AS rule
                  ON rule.set_revision_id = set_revision.revision_id
                WHERE set_revision.transform_set_id = ?1
                  AND set_revision.revision_id = ?2
                  AND rule.rule_id = ?3
             )",
            params![set_id, revision_id, rule_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)?;
    if exists {
        Ok(())
    } else {
        Err(storage_corrupted(
            "generation transform report does not match its exact stored revision",
        ))
    }
}

fn insert_transform_application_log(
    transaction: &Transaction<'_>,
    assistant: &Message,
    generation_id: &GenerationId,
    ordinal: usize,
    application: &ResolvedApplication<'_>,
    created_at: DateTime<Utc>,
) -> CoreResult<()> {
    let ordinal = u32::try_from(ordinal)
        .map_err(|_| CoreError::invalid("transform diagnostic ordinal exceeds its bound"))?;
    let diagnostics = StoredRuleDiagnostics {
        schema_version: DIAGNOSTIC_SCHEMA_VERSION,
        set_id: application.write.set_id.clone(),
        disposition: application.write.disposition.as_str().to_owned(),
        code: application.write.code.clone(),
    };
    let diagnostics_json = serde_json::to_string(&diagnostics).map_err(|error| {
        CoreError::internal(format!("cannot encode transform rule diagnostics: {error}"))
    })?;
    let id_material = format!(
        "{}\0{}\0{}\0{}\0{}\0{}",
        generation_id.0,
        assistant.id.0,
        application.write.stage.as_str(),
        ordinal,
        application.set_revision_id,
        application.write.rule_id,
    );
    let id = format!(
        "transform-application:{}",
        sha256_hex(id_material.as_bytes())
    );
    let stored_status = application.write.disposition.stored_status();
    let stored_error_code = (stored_status == "failed")
        .then_some(application.write.code.as_deref())
        .flatten();
    transaction
        .execute(
            "INSERT INTO transform_application_logs
             (id, plan_id, generation_id, message_id, set_revision_id,
              rule_id, phase, ordinal, status, before_sha256,
              after_sha256, replacement_count, input_chars, output_chars,
              error_code, diagnostics_json, created_at)
             VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                     ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                id,
                generation_id.0,
                assistant.id.0,
                application.set_revision_id,
                application.write.rule_id,
                application.write.stage.as_str(),
                ordinal,
                stored_status,
                application.write.before_sha256.as_str(),
                application
                    .write
                    .after_sha256
                    .as_ref()
                    .map(Sha256Digest::as_str),
                application.write.replacement_count,
                application.write.input_chars,
                application.write.output_chars,
                stored_error_code,
                diagnostics_json,
                created_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn materialize_diagnostics(
    applications: &[ResolvedApplication<'_>],
    pipeline_failures: &[MessageTransformPipelineFailureWrite],
    recorded_at: DateTime<Utc>,
) -> CoreResult<Vec<MessageTransformDiagnostic>> {
    let mut diagnostics = applications
        .iter()
        .map(|application| MessageTransformDiagnostic {
            set_revision_id: Some(application.set_revision_id.clone()),
            rule_id: Some(application.write.rule_id.clone()),
            stage: application.write.stage,
            disposition: application.write.disposition,
            code: application.write.code.clone(),
            before_sha256: application.write.before_sha256.clone(),
            after_sha256: application.write.after_sha256.clone(),
            recorded_at,
        })
        .collect::<Vec<_>>();
    diagnostics.extend(
        pipeline_failures
            .iter()
            .map(|failure| MessageTransformDiagnostic {
                set_revision_id: None,
                rule_id: None,
                stage: failure.stage,
                disposition: MessageTransformDisposition::PipelineRejected,
                code: Some(failure.code.clone()),
                before_sha256: failure.before_sha256.clone(),
                after_sha256: None,
                recorded_at,
            }),
    );
    sort_diagnostics(&mut diagnostics);
    if diagnostics.len()
        > MAX_MESSAGE_TRANSFORM_APPLICATIONS + MAX_MESSAGE_TRANSFORM_PIPELINE_FAILURES
    {
        return Err(CoreError::invalid(
            "message transform diagnostic count exceeds its bound",
        ));
    }
    Ok(diagnostics)
}

fn load_rule_diagnostics(
    connection: &rusqlite::Connection,
    message_id: &MessageId,
    generation_id: &GenerationId,
    projection_created_at: DateTime<Utc>,
) -> CoreResult<Vec<MessageTransformDiagnostic>> {
    let mut statement = connection
        .prepare(
            "SELECT log.set_revision_id, set_revision.transform_set_id,
                    log.rule_id, log.phase, log.status, log.before_sha256,
                    log.after_sha256, log.error_code, log.diagnostics_json,
                    log.created_at
             FROM transform_application_logs AS log
             JOIN transform_set_revisions AS set_revision
               ON set_revision.revision_id = log.set_revision_id
             WHERE log.message_id = ?1 AND log.generation_id = ?2
               AND log.phase IN ('provider_output_canonical', 'display_only')
             ORDER BY CASE log.phase
                        WHEN 'provider_output_canonical' THEN 0 ELSE 1 END,
                      log.ordinal, log.id",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map(params![message_id.0, generation_id.0], |row| {
            Ok(StoredRuleDiagnosticRow {
                set_revision_id: row.get(0)?,
                set_id: row.get(1)?,
                rule_id: row.get(2)?,
                phase: row.get(3)?,
                status: row.get(4)?,
                before_sha256: row.get(5)?,
                after_sha256: row.get(6)?,
                error_code: row.get(7)?,
                diagnostics_json: row.get(8)?,
                created_at: row.get(9)?,
            })
        })
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    if rows.len() > MAX_MESSAGE_TRANSFORM_APPLICATIONS {
        return Err(storage_corrupted(
            "stored message transform application count exceeds its bound",
        ));
    }
    rows.into_iter()
        .map(|row| decode_rule_diagnostic(row, projection_created_at))
        .collect()
}

fn decode_rule_diagnostic(
    row: StoredRuleDiagnosticRow,
    projection_created_at: DateTime<Utc>,
) -> CoreResult<MessageTransformDiagnostic> {
    let metadata: StoredRuleDiagnostics = serde_json::from_str(&row.diagnostics_json)
        .map_err(|_| storage_corrupted("stored transform rule diagnostics are invalid"))?;
    if metadata.schema_version != DIAGNOSTIC_SCHEMA_VERSION {
        return Err(storage_corrupted(
            "stored transform rule diagnostics schema is unsupported",
        ));
    }
    let disposition = MessageTransformDisposition::parse(&metadata.disposition)?;
    validate_identifier("transform revision", &row.set_revision_id)
        .map_err(|_| storage_corrupted("stored transform revision identity violates bounds"))?;
    validate_identifier("transform set", &row.set_id)
        .map_err(|_| storage_corrupted("stored transform set identity violates bounds"))?;
    validate_identifier("transform rule", &row.rule_id)
        .map_err(|_| storage_corrupted("stored transform rule identity violates bounds"))?;
    validate_code(metadata.code.as_deref())
        .map_err(|_| storage_corrupted("stored transform diagnostic code is invalid"))?;
    let error_code_is_consistent = if row.status == "failed" {
        metadata.code.as_deref() == row.error_code.as_deref()
    } else {
        row.error_code.is_none()
    };
    let diagnostic_code_is_consistent = matches!(
        disposition,
        MessageTransformDisposition::Failed | MessageTransformDisposition::LimitRejected
    ) == metadata.code.is_some();
    let after_hash_is_consistent = if matches!(row.status.as_str(), "failed" | "limit_rejected") {
        row.after_sha256.is_none()
    } else {
        row.after_sha256.is_some()
    };
    if metadata.set_id != row.set_id
        || disposition == MessageTransformDisposition::PipelineRejected
        || disposition.stored_status() != row.status
        || !error_code_is_consistent
        || !diagnostic_code_is_consistent
        || !after_hash_is_consistent
    {
        return Err(storage_corrupted(
            "stored transform rule diagnostic metadata is inconsistent",
        ));
    }
    let recorded_at = parse_datetime("transform diagnostic created_at", &row.created_at)?;
    if recorded_at != projection_created_at {
        return Err(storage_corrupted(
            "transform diagnostic timestamp diverges from its display projection",
        ));
    }
    Ok(MessageTransformDiagnostic {
        set_revision_id: Some(row.set_revision_id),
        rule_id: Some(row.rule_id),
        stage: MessageTransformStage::parse(&row.phase)?,
        disposition,
        code: metadata.code,
        before_sha256: parse_sha256("transform before content", row.before_sha256)?,
        after_sha256: row
            .after_sha256
            .map(|value| parse_sha256("transform after content", value))
            .transpose()?,
        recorded_at,
    })
}

fn sort_diagnostics(diagnostics: &mut [MessageTransformDiagnostic]) {
    diagnostics.sort_by(|left, right| {
        stage_rank(left.stage)
            .cmp(&stage_rank(right.stage))
            .then_with(|| left.set_revision_id.cmp(&right.set_revision_id))
            .then_with(|| left.rule_id.cmp(&right.rule_id))
            .then_with(|| left.disposition.as_str().cmp(right.disposition.as_str()))
            .then_with(|| left.code.cmp(&right.code))
    });
}

const fn stage_rank(stage: MessageTransformStage) -> u8 {
    match stage {
        MessageTransformStage::ProviderOutputCanonical => 0,
        MessageTransformStage::DisplayOnly => 1,
    }
}

fn diagnostics_sha256(diagnostics: &[MessageTransformDiagnostic]) -> CoreResult<Sha256Digest> {
    let canonical = serde_json::to_vec(&DiagnosticDigestDocument {
        schema_version: DIAGNOSTIC_SCHEMA_VERSION,
        diagnostics,
    })
    .map_err(|error| {
        CoreError::internal(format!(
            "cannot encode transform diagnostic digest: {error}"
        ))
    })?;
    sha256_digest(&canonical)
}

fn sha256_digest(bytes: &[u8]) -> CoreResult<Sha256Digest> {
    Sha256Digest::parse(sha256_hex(bytes))
        .map_err(|error| CoreError::internal(format!("cannot construct SHA-256 digest: {error}")))
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn parse_sha256(label: &str, value: String) -> CoreResult<Sha256Digest> {
    Sha256Digest::parse(value)
        .map_err(|_| storage_corrupted(format!("stored {label} hash is invalid")))
}

fn parse_datetime(label: &str, value: &str) -> CoreResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| storage_corrupted(format!("stored {label} is invalid")))
}

fn storage_corrupted(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::StorageCorrupted, message, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_hash_chain_must_start_at_canonical_and_end_at_projection() {
        let canonical = sha256_digest(b"canonical").expect("canonical digest");
        let display = sha256_digest(b"display").expect("display digest");
        let write = MessageDisplayProjectionWrite {
            display_content: "display".to_owned(),
            applications: vec![MessageTransformApplicationWrite {
                set_id: "set-1".to_owned(),
                rule_id: "rule-1".to_owned(),
                stage: MessageTransformStage::DisplayOnly,
                disposition: MessageTransformDisposition::Applied,
                code: None,
                before_sha256: canonical.clone(),
                after_sha256: Some(display.clone()),
                replacement_count: 1,
                input_chars: 9,
                output_chars: 7,
            }],
            pipeline_failures: Vec::new(),
        };
        validate_transform_hash_chain(&write, &canonical, &display)
            .expect("exact DisplayOnly hash chain");

        let mut tampered = write;
        tampered.applications[0].before_sha256 = sha256_digest(b"other").expect("tampered digest");
        assert!(validate_transform_hash_chain(&tampered, &canonical, &display).is_err());
    }

    #[test]
    fn pipeline_failures_are_unique_content_free_fail_open_evidence() {
        let canonical = sha256_digest(b"canonical").expect("canonical digest");
        let failure = MessageTransformPipelineFailureWrite {
            stage: MessageTransformStage::DisplayOnly,
            code: "pipeline_invalid".to_owned(),
            before_sha256: canonical.clone(),
        };
        let write = MessageDisplayProjectionWrite {
            display_content: "canonical".to_owned(),
            applications: Vec::new(),
            pipeline_failures: vec![failure.clone()],
        };
        validate_projection_write(&write).expect("bounded pipeline diagnostic");
        validate_transform_hash_chain(&write, &canonical, &canonical)
            .expect("pipeline rejection remains byte-identical");

        let duplicated = MessageDisplayProjectionWrite {
            display_content: "canonical".to_owned(),
            applications: Vec::new(),
            pipeline_failures: vec![failure.clone(), failure],
        };
        assert!(validate_projection_write(&duplicated).is_err());
        let serialized =
            serde_json::to_string(&write.pipeline_failures).expect("serialize pipeline evidence");
        assert!(!serialized.contains("canonical"));
    }
}
