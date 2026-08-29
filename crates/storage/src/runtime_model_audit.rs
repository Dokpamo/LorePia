use chrono::{DateTime, Utc};
use lorepia_domain::{CoreError, CoreResult, GenerationUsage, Sha256Digest};
use rusqlite::{OptionalExtension, params};
use uuid::Uuid;

use crate::{Storage, database::storage_db_error};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeModelCapability {
    Primary,
    Auxiliary,
}

impl RuntimeModelCapability {
    fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "model:primary",
            Self::Auxiliary => "model:auxiliary",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeModelAuditStart {
    pub request_id: String,
    pub character_id: String,
    pub character_content_revision_id: Option<String>,
    pub capability: RuntimeModelCapability,
    pub grant_sha256: String,
    pub provider_connection_id: String,
    pub model_route_id: Option<String>,
    pub generation_preset_id: Option<String>,
    pub started_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeModelAuditStatus {
    Succeeded,
    Cancelled,
    UnknownOutcome,
    Failed,
}

impl RuntimeModelAuditStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Cancelled => "cancelled",
            Self::UnknownOutcome => "unknown_outcome",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeModelAuditFinish<'a> {
    pub request_id: &'a str,
    pub status: RuntimeModelAuditStatus,
    pub usage: Option<&'a GenerationUsage>,
    pub failure_code: Option<&'a str>,
    pub completed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRuntimeModelAudit {
    pub request_id: String,
    pub character_id: String,
    pub character_content_revision_id: Option<String>,
    pub capability: String,
    pub grant_sha256: String,
    pub provider_connection_id: String,
    pub model_route_id: Option<String>,
    pub generation_preset_id: Option<String>,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub status: String,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub tool_tokens: Option<u64>,
    pub failure_code: Option<String>,
}

impl Storage {
    pub fn start_runtime_model_audit(&self, start: &RuntimeModelAuditStart) -> CoreResult<()> {
        validate_uuid("runtime model request", &start.request_id)?;
        validate_identifier("runtime model character", &start.character_id)?;
        validate_optional_identifier(
            "runtime model character revision",
            start.character_content_revision_id.as_deref(),
        )?;
        Sha256Digest::parse(start.grant_sha256.clone())
            .map_err(|_| CoreError::invalid("runtime model grant digest is invalid"))?;
        validate_identifier(
            "runtime model provider connection",
            &start.provider_connection_id,
        )?;
        validate_optional_identifier("runtime model route", start.model_route_id.as_deref())?;
        validate_optional_identifier(
            "runtime model generation preset",
            start.generation_preset_id.as_deref(),
        )?;

        let connection = self.connection()?;
        let character_exists = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM characters WHERE id = ?1)",
                [&start.character_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(storage_db_error)?;
        if !character_exists {
            return Err(CoreError::invalid(
                "runtime model audit character does not exist",
            ));
        }
        if let Some(revision_id) = start.character_content_revision_id.as_deref() {
            let revision_matches = connection
                .query_row(
                    "SELECT EXISTS(
                         SELECT 1
                         FROM character_content_revisions AS revision
                         JOIN character_content AS content
                           ON content.object_id = revision.object_id
                         WHERE revision.revision_id = ?1
                           AND content.character_id = ?2
                     )",
                    params![revision_id, start.character_id],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(storage_db_error)?;
            if !revision_matches {
                return Err(CoreError::invalid(
                    "runtime model audit revision does not belong to the character",
                ));
            }
        }
        connection
            .execute(
                "INSERT INTO portable_runtime_model_audit (
                     request_id, character_id, character_content_revision_id,
                     capability, grant_sha256, provider_connection_id,
                     model_route_id, generation_preset_id, started_at, status
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'started')",
                params![
                    start.request_id,
                    start.character_id,
                    start.character_content_revision_id,
                    start.capability.as_str(),
                    start.grant_sha256,
                    start.provider_connection_id,
                    start.model_route_id,
                    start.generation_preset_id,
                    start.started_at.to_rfc3339(),
                ],
            )
            .map_err(storage_db_error)?;
        Ok(())
    }

    pub fn finish_runtime_model_audit(
        &self,
        finish: RuntimeModelAuditFinish<'_>,
    ) -> CoreResult<()> {
        validate_uuid("runtime model request", finish.request_id)?;
        let expected_failure = !matches!(finish.status, RuntimeModelAuditStatus::Succeeded);
        if expected_failure != finish.failure_code.is_some() {
            return Err(CoreError::invalid(
                "runtime model audit terminal status and failure code disagree",
            ));
        }
        if let Some(code) = finish.failure_code {
            validate_failure_code(code)?;
        }
        let usage = finish.usage;
        let changed = self
            .connection()?
            .execute(
                "UPDATE portable_runtime_model_audit
                 SET status = ?2,
                     completed_at = ?3,
                     input_tokens = ?4,
                     output_tokens = ?5,
                     reasoning_tokens = ?6,
                     tool_tokens = ?7,
                     failure_code = ?8
                 WHERE request_id = ?1 AND status = 'started'",
                params![
                    finish.request_id,
                    finish.status.as_str(),
                    finish.completed_at.to_rfc3339(),
                    optional_i64(usage.and_then(|value| value.input_tokens))?,
                    optional_i64(usage.and_then(|value| value.output_tokens))?,
                    optional_i64(usage.and_then(|value| value.reasoning_tokens))?,
                    optional_i64(usage.and_then(|value| value.tool_tokens))?,
                    finish.failure_code,
                ],
            )
            .map_err(storage_db_error)?;
        if changed != 1 {
            return Err(CoreError::invalid(
                "runtime model audit is missing or already terminal",
            ));
        }
        Ok(())
    }

    pub fn runtime_model_audit(
        &self,
        request_id: &str,
    ) -> CoreResult<Option<StoredRuntimeModelAudit>> {
        validate_uuid("runtime model request", request_id)?;
        self.connection()?
            .query_row(
                "SELECT request_id, character_id, character_content_revision_id,
                        capability, grant_sha256, provider_connection_id,
                        model_route_id, generation_preset_id, started_at, completed_at,
                        status, input_tokens, output_tokens, reasoning_tokens,
                        tool_tokens, failure_code
                 FROM portable_runtime_model_audit
                 WHERE request_id = ?1",
                [request_id],
                |row| {
                    Ok(StoredRuntimeModelAudit {
                        request_id: row.get(0)?,
                        character_id: row.get(1)?,
                        character_content_revision_id: row.get(2)?,
                        capability: row.get(3)?,
                        grant_sha256: row.get(4)?,
                        provider_connection_id: row.get(5)?,
                        model_route_id: row.get(6)?,
                        generation_preset_id: row.get(7)?,
                        started_at: row.get(8)?,
                        completed_at: row.get(9)?,
                        status: row.get(10)?,
                        input_tokens: optional_u64(row.get(11)?)?,
                        output_tokens: optional_u64(row.get(12)?)?,
                        reasoning_tokens: optional_u64(row.get(13)?)?,
                        tool_tokens: optional_u64(row.get(14)?)?,
                        failure_code: row.get(15)?,
                    })
                },
            )
            .optional()
            .map_err(storage_db_error)
    }

    pub(crate) fn recover_started_runtime_model_audits(
        &self,
        recovered_at: DateTime<Utc>,
    ) -> CoreResult<u64> {
        let changed = self
            .connection()?
            .execute(
                "UPDATE portable_runtime_model_audit
                 SET status = 'interrupted', completed_at = ?1, failure_code = 'process_restarted'
                 WHERE status = 'started'",
                [recovered_at.to_rfc3339()],
            )
            .map_err(storage_db_error)?;
        u64::try_from(changed)
            .map_err(|_| CoreError::internal("runtime model audit recovery count overflowed"))
    }
}

fn validate_uuid(label: &str, value: &str) -> CoreResult<()> {
    if Uuid::parse_str(value).is_ok_and(|parsed| parsed.to_string() == value) {
        Ok(())
    } else {
        Err(CoreError::invalid(format!("{label} id is invalid")))
    }
}

fn validate_identifier(label: &str, value: &str) -> CoreResult<()> {
    if !value.is_empty() && value.len() <= 256 && value.trim() == value && !value.contains('\0') {
        Ok(())
    } else {
        Err(CoreError::invalid(format!("{label} id is invalid")))
    }
}

fn validate_optional_identifier(label: &str, value: Option<&str>) -> CoreResult<()> {
    value.map_or(Ok(()), |value| validate_identifier(label, value))
}

fn validate_failure_code(code: &str) -> CoreResult<()> {
    if !code.is_empty() && code.len() <= 128 && code.trim() == code && !code.contains('\0') {
        Ok(())
    } else {
        Err(CoreError::invalid(
            "runtime model audit failure code is invalid",
        ))
    }
}

fn optional_i64(value: Option<u64>) -> CoreResult<Option<i64>> {
    value
        .map(|value| {
            i64::try_from(value)
                .map_err(|_| CoreError::invalid("runtime model token usage exceeds storage range"))
        })
        .transpose()
}

fn optional_u64(value: Option<i64>) -> rusqlite::Result<Option<u64>> {
    value
        .map(|value| {
            u64::try_from(value).map_err(|_| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Integer,
                    "negative runtime model token usage".into(),
                )
            })
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use chrono::TimeZone;
    use lorepia_domain::GenerationUsage;
    use rusqlite::params;
    use tempfile::tempdir;

    use super::*;

    const REQUEST_ID: &str = "00000000-0000-4000-8000-000000000101";
    const CHARACTER_ID: &str = "runtime-audit-character";

    fn fixture() -> (tempfile::TempDir, Storage) {
        let root = tempdir().expect("temporary storage root");
        let storage = Storage::open(root.path()).expect("open storage");
        let connection = storage.connection().expect("storage connection");
        connection
            .execute(
                "INSERT INTO content_sources
                 (sha256, relative_path, size_bytes, created_at)
                 VALUES (?1, 'sha256/runtime-audit', 1, ?2)",
                params!["cd".repeat(32), "2026-08-29T00:00:00Z"],
            )
            .expect("insert content source");
        connection
            .execute(
                "INSERT INTO characters
                 (id, name, description, source_hash, avatar_asset_hash, created_at)
                 VALUES (?1, 'Audit Character', '', ?2, NULL, ?3)",
                params![CHARACTER_ID, "cd".repeat(32), "2026-08-29T00:00:00Z"],
            )
            .expect("insert character");
        drop(connection);
        (root, storage)
    }

    fn start(request_id: &str) -> RuntimeModelAuditStart {
        RuntimeModelAuditStart {
            request_id: request_id.to_owned(),
            character_id: CHARACTER_ID.to_owned(),
            character_content_revision_id: None,
            capability: RuntimeModelCapability::Primary,
            grant_sha256: "ab".repeat(32),
            provider_connection_id: "provider-connection".to_owned(),
            model_route_id: Some("model-route".to_owned()),
            generation_preset_id: Some("generation-preset".to_owned()),
            started_at: Utc
                .with_ymd_and_hms(2026, 8, 29, 1, 0, 0)
                .single()
                .expect("start timestamp"),
        }
    }

    #[test]
    fn runtime_model_audit_is_metadata_only_and_terminal_once() {
        let (_root, storage) = fixture();
        storage
            .start_runtime_model_audit(&start(REQUEST_ID))
            .expect("start audit");
        let usage = GenerationUsage {
            input_tokens: Some(12),
            output_tokens: Some(8),
            reasoning_tokens: Some(3),
            tool_tokens: Some(1),
            ..GenerationUsage::default()
        };
        storage
            .finish_runtime_model_audit(RuntimeModelAuditFinish {
                request_id: REQUEST_ID,
                status: RuntimeModelAuditStatus::Succeeded,
                usage: Some(&usage),
                failure_code: None,
                completed_at: Utc
                    .with_ymd_and_hms(2026, 8, 29, 1, 0, 5)
                    .single()
                    .expect("completion timestamp"),
            })
            .expect("finish audit");

        let stored = storage
            .runtime_model_audit(REQUEST_ID)
            .expect("read audit")
            .expect("stored audit");
        assert_eq!(stored.status, "succeeded");
        assert_eq!(stored.input_tokens, Some(12));
        assert_eq!(stored.output_tokens, Some(8));
        assert_eq!(stored.grant_sha256, "ab".repeat(32));
        assert!(
            storage
                .finish_runtime_model_audit(RuntimeModelAuditFinish {
                    request_id: REQUEST_ID,
                    status: RuntimeModelAuditStatus::Failed,
                    usage: None,
                    failure_code: Some("internal"),
                    completed_at: Utc::now(),
                })
                .is_err(),
            "terminal audit must not be rewritten"
        );

        let columns = storage
            .connection()
            .expect("storage connection")
            .prepare("PRAGMA table_info(portable_runtime_model_audit)")
            .expect("prepare column inventory")
            .query_map([], |row| row.get::<_, String>(1))
            .expect("query column inventory")
            .collect::<Result<BTreeSet<_>, _>>()
            .expect("collect column inventory");
        for forbidden in [
            "prompt",
            "credential",
            "result",
            "response",
            "provider_payload",
        ] {
            assert!(!columns.contains(forbidden));
        }
    }

    #[test]
    fn startup_marks_an_unfinished_runtime_model_audit_interrupted() {
        let (root, storage) = fixture();
        storage
            .start_runtime_model_audit(&start(REQUEST_ID))
            .expect("start audit");
        drop(storage);

        let reopened = Storage::open(root.path()).expect("reopen storage");
        let stored = reopened
            .runtime_model_audit(REQUEST_ID)
            .expect("read recovered audit")
            .expect("recovered audit");
        assert_eq!(stored.status, "interrupted");
        assert_eq!(stored.failure_code.as_deref(), Some("process_restarted"));
        assert!(stored.completed_at.is_some());
    }

    #[test]
    fn runtime_model_audit_preserves_unknown_provider_outcome() {
        let (_root, storage) = fixture();
        storage
            .start_runtime_model_audit(&start(REQUEST_ID))
            .expect("start audit");
        storage
            .finish_runtime_model_audit(RuntimeModelAuditFinish {
                request_id: REQUEST_ID,
                status: RuntimeModelAuditStatus::UnknownOutcome,
                usage: None,
                failure_code: Some("cancelled"),
                completed_at: Utc::now(),
            })
            .expect("record outcome-unknown audit");

        let stored = storage
            .runtime_model_audit(REQUEST_ID)
            .expect("read audit")
            .expect("stored audit");
        assert_eq!(stored.status, "unknown_outcome");
        assert_eq!(stored.failure_code.as_deref(), Some("cancelled"));
    }
}
