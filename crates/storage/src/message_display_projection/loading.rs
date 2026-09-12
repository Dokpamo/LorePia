use std::collections::BTreeMap;

use super::{
    CoreError, CoreResult, DIAGNOSTIC_SCHEMA_VERSION, GenerationId,
    MAX_MESSAGE_TRANSFORM_APPLICATIONS, Message, MessageTransformDiagnostic,
    MessageTransformDisposition, Storage, StoredMessageDisplayProjection,
    StoredPipelineDiagnostics, StoredRuleDiagnosticRow, decode_rule_diagnostic, diagnostics_sha256,
    parse_datetime, parse_sha256, sha256_digest, sort_diagnostics, storage_corrupted,
    storage_db_error, validate_display_content, validate_pipeline_failures,
    validate_terminal_projection_owner,
};

const MESSAGE_BATCH_SIZE: usize = 64;
type ProjectionRow = (String, String, String, String, String, String, String);

impl Storage {
    /// Loads one verified sidecar; absence is a legitimate identity projection.
    pub fn get_message_display_projection(
        &self,
        message: &Message,
    ) -> CoreResult<Option<StoredMessageDisplayProjection>> {
        Ok(self.get_message_display_projections(&[message])?.pop())
    }

    /// Loads sidecars with at most two queries per bounded batch, releasing the database
    /// lock before decoding and hashing. All single-message integrity checks remain.
    pub fn get_message_display_projections(
        &self,
        messages: &[&Message],
    ) -> CoreResult<Vec<StoredMessageDisplayProjection>> {
        if messages.is_empty() {
            return Ok(Vec::new());
        }
        let mut projections = Vec::new();
        for batch in messages.chunks(MESSAGE_BATCH_SIZE) {
            let rows = {
                let connection = self.connection()?;
                read_batch(&connection, batch)?
            };
            projections.extend(verify_batch(rows)?);
        }
        Ok(projections)
    }
}

pub(crate) struct LoadedProjection<'a> {
    message: &'a Message,
    row: ProjectionRow,
    diagnostics: Vec<StoredRuleDiagnosticRow>,
}

/// Reads page sidecars from the caller's established canonical-message snapshot.
/// Verification consumes these borrowed rows only after the caller releases `SQLite`.
pub(crate) fn read_page_projections<'a>(
    connection: &rusqlite::Connection,
    messages: impl Iterator<Item = &'a Message>,
) -> CoreResult<Vec<LoadedProjection<'a>>> {
    let eligible = messages
        .filter(|message| {
            message.role == lorepia_domain::MessageRole::Assistant
                && message.status != lorepia_domain::MessageStatus::Pending
                && message
                    .generation_id
                    .as_ref()
                    .is_some_and(|id| !id.is_character_greeting())
        })
        .collect::<Vec<_>>();
    let mut rows = Vec::new();
    for batch in eligible.chunks(MESSAGE_BATCH_SIZE) {
        rows.extend(read_batch(connection, batch)?);
    }
    Ok(rows)
}

fn read_batch<'a>(
    connection: &rusqlite::Connection,
    messages: &[&'a Message],
) -> CoreResult<Vec<LoadedProjection<'a>>> {
    let ids = serde_json::to_string(&messages.iter().map(|m| &m.id.0).collect::<Vec<_>>())
        .map_err(|_| CoreError::internal("cannot encode projection lookup identities"))?;
    let mut statement = connection
        .prepare_cached(
            "SELECT message_id, generation_id, canonical_content_sha256, display_content,
                display_content_sha256, pipeline_diagnostics_json, diagnostics_sha256, created_at
         FROM message_display_projections WHERE message_id IN (SELECT value FROM json_each(?1))",
        )
        .map_err(storage_db_error)?;
    let mut projections = statement
        .query_map([&ids], |row| {
            Ok((
                row.get::<_, String>(0)?,
                (
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                ),
            ))
        })
        .map_err(storage_db_error)?
        .collect::<Result<BTreeMap<_, _>, _>>()
        .map_err(storage_db_error)?;
    // The diagnostic join below cannot produce rows without a projection.
    if projections.is_empty() {
        return Ok(Vec::new());
    }
    let mut statement = connection
        .prepare_cached(
            "SELECT log.message_id, log.set_revision_id, revision.transform_set_id,
                log.rule_id, log.phase, log.status, log.before_sha256, log.after_sha256,
                log.error_code, log.diagnostics_json, log.created_at
         FROM transform_application_logs AS log
         JOIN transform_set_revisions AS revision ON revision.revision_id = log.set_revision_id
         JOIN message_display_projections AS projection ON projection.message_id = log.message_id
            AND projection.generation_id = log.generation_id
         WHERE log.message_id IN (SELECT value FROM json_each(?1))
            AND log.phase IN ('provider_output_canonical', 'display_only')
         ORDER BY log.message_id, CASE log.phase WHEN 'provider_output_canonical' THEN 0 ELSE 1 END,
            log.ordinal, log.id",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map([&ids], |row| {
            Ok((
                row.get::<_, String>(0)?,
                StoredRuleDiagnosticRow {
                    set_revision_id: row.get(1)?,
                    set_id: row.get(2)?,
                    rule_id: row.get(3)?,
                    phase: row.get(4)?,
                    status: row.get(5)?,
                    before_sha256: row.get(6)?,
                    after_sha256: row.get(7)?,
                    error_code: row.get(8)?,
                    diagnostics_json: row.get(9)?,
                    created_at: row.get(10)?,
                },
            ))
        })
        .map_err(storage_db_error)?;
    let mut diagnostics: BTreeMap<String, Vec<StoredRuleDiagnosticRow>> = BTreeMap::new();
    for row in rows {
        let (id, row) = row.map_err(storage_db_error)?;
        let entries = diagnostics.entry(id).or_default();
        if entries.len() == MAX_MESSAGE_TRANSFORM_APPLICATIONS {
            return Err(storage_corrupted(
                "stored message transform application count exceeds its bound",
            ));
        }
        entries.push(row);
    }
    Ok(messages
        .iter()
        .filter_map(|message| {
            projections
                .remove(&message.id.0)
                .map(|row| LoadedProjection {
                    message,
                    row,
                    diagnostics: diagnostics.remove(&message.id.0).unwrap_or_default(),
                })
        })
        .collect())
}

pub(crate) fn verify_batch(
    rows: Vec<LoadedProjection<'_>>,
) -> CoreResult<Vec<StoredMessageDisplayProjection>> {
    rows.into_iter()
        .map(|row| verify_projection(row.message, row.row, row.diagnostics))
        .collect()
}

fn verify_projection(
    message: &Message,
    row: ProjectionRow,
    rows: Vec<StoredRuleDiagnosticRow>,
) -> CoreResult<StoredMessageDisplayProjection> {
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
    validate_pipeline_failures(&pipeline.failures)
        .map_err(|_| storage_corrupted("stored transform pipeline diagnostics violate bounds"))?;
    if pipeline.schema_version != DIAGNOSTIC_SCHEMA_VERSION {
        return Err(storage_corrupted(
            "stored transform pipeline diagnostics schema is unsupported",
        ));
    }
    let created_at = parse_datetime("display projection created_at", &row.6)?;
    let mut diagnostics = rows
        .into_iter()
        .map(|row| decode_rule_diagnostic(row, created_at))
        .collect::<CoreResult<Vec<_>>>()?;
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
    Ok(StoredMessageDisplayProjection {
        message_id: message.id.clone(),
        generation_id,
        display_content: row.2,
        canonical_content_sha256,
        display_content_sha256,
        diagnostics_sha256: diagnostics_sha256_stored,
        diagnostics,
        created_at,
    })
}

#[cfg(test)]
mod tests;
