use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use lorepia_domain::{CoreError, CoreResult, GenerationId, Message, Sha256Digest};
use rusqlite::{Transaction, params};

use crate::database::storage_db_error;

use super::{
    DIAGNOSTIC_SCHEMA_VERSION, MAX_MESSAGE_TRANSFORM_APPLICATIONS,
    MAX_MESSAGE_TRANSFORM_PIPELINE_FAILURES, MessageDisplayProjectionWrite,
    MessageTransformApplicationWrite, MessageTransformDiagnostic, MessageTransformDisposition,
    MessageTransformPipelineFailureWrite, StoredPipelineDiagnostics, StoredRuleDiagnostics,
    diagnostics_sha256, sha256_digest, sha256_hex, sort_diagnostics, storage_corrupted,
    validate_identifier, validate_projection_write, validate_terminal_projection_owner,
    validate_transform_hash_chain,
};

#[derive(Debug)]
struct ResolvedApplication<'a> {
    write: &'a MessageTransformApplicationWrite,
    set_revision_id: String,
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
