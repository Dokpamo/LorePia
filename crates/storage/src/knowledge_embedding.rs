//! Exact, immutable semantic vectors for revisioned knowledge entries.
//!
//! Query vectors are produced by Core's durable provider intent. Storage only
//! admits vectors from the same immutable task revision and provider vector
//! space, then computes deterministic cosine scores in Rust.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use lorepia_domain::{
    AuxiliaryTaskKind, CoreError, CoreErrorCode, CoreResult, KnowledgeEntryId, ModelRouteId,
    TaskProfile, ValidateOrchestration,
};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Storage, database::storage_db_error};

const MAX_KNOWLEDGE_EMBEDDING_DIMENSIONS: usize = 32_768;
const MAX_KNOWLEDGE_EMBEDDINGS_PER_BOOK: usize = 10_000;
const MAX_KNOWLEDGE_EMBEDDING_QUERY_ROWS: i64 = 10_001;

type StoredKnowledgeEmbeddingRow = (String, String, String, Vec<u8>);
type StoredKnowledgeEmbeddingReplay = (String, String, Vec<u8>, String);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeEmbeddingWrite {
    pub id: String,
    pub book_revision_id: String,
    pub entry_id: KnowledgeEntryId,
    pub task_profile_revision_id: String,
    pub model_route_id: ModelRouteId,
    pub dimensions: u32,
    pub vector_space_sha256: String,
    pub values: Vec<f32>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeEmbeddingQuery {
    pub book_revision_id: String,
    pub task_profile_revision_id: String,
    pub model_route_id: ModelRouteId,
    pub dimensions: u32,
    pub vector_space_sha256: String,
    pub values: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeEmbeddingCoverageQuery {
    pub book_revision_id: String,
    pub task_profile_revision_id: String,
    pub model_route_id: ModelRouteId,
    pub dimensions: u32,
    pub vector_space_sha256: String,
    pub required_entry_ids: Vec<KnowledgeEntryId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeEmbeddingMatch {
    pub embedding_id: String,
    pub entry_id: KnowledgeEntryId,
    pub vector_sha256: String,
    pub similarity_millionths: u32,
}

impl Storage {
    /// Inserts one immutable entry vector. Replaying the exact same write is
    /// idempotent; a second value for the same exact vector space fails closed.
    pub fn save_knowledge_embedding(&self, write: &KnowledgeEmbeddingWrite) -> CoreResult<()> {
        validate_knowledge_embedding_write(write)?;
        let (bytes, vector_sha256) = encode_vector(write.dimensions, &write.values)?;
        let connection = self.connection()?;
        validate_embedding_task_space(
            &connection,
            &write.task_profile_revision_id,
            &write.model_route_id,
            write.dimensions,
        )?;
        let entry_exists = connection
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM knowledge_entries
                     WHERE book_revision_id = ?1 AND entry_id = ?2
                 )",
                params![write.book_revision_id, write.entry_id.as_str()],
                |row| row.get::<_, bool>(0),
            )
            .map_err(storage_db_error)?;
        if !entry_exists {
            return Err(CoreError::new(
                CoreErrorCode::NotFound,
                "knowledge entry revision was not found",
                false,
            ));
        }

        let existing = load_existing_knowledge_embedding(&connection, write)?;
        if let Some((id, stored_sha256, stored_bytes, created_at)) = existing {
            if id == write.id
                && stored_sha256 == vector_sha256
                && stored_bytes == bytes
                && created_at == write.created_at.to_rfc3339()
            {
                return Ok(());
            }
            return Err(CoreError::invalid(
                "knowledge entry already has a different embedding in the exact vector space",
            ));
        }

        connection
            .execute(
                "INSERT INTO knowledge_embeddings
                 (id, book_revision_id, entry_id, task_profile_revision_id,
                  model_route_id, dimensions, vector_space_sha256, encoding,
                  vector_blob, vector_sha256, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'f32le', ?8, ?9, ?10)",
                params![
                    write.id,
                    write.book_revision_id,
                    write.entry_id.as_str(),
                    write.task_profile_revision_id,
                    write.model_route_id.as_str(),
                    write.dimensions,
                    write.vector_space_sha256,
                    bytes,
                    vector_sha256,
                    write.created_at.to_rfc3339(),
                ],
            )
            .map_err(storage_db_error)?;
        Ok(())
    }

    /// Scores every exact-space vector for one immutable book revision.
    pub fn query_knowledge_embeddings_cosine(
        &self,
        query: &KnowledgeEmbeddingQuery,
    ) -> CoreResult<Vec<KnowledgeEmbeddingMatch>> {
        validate_knowledge_embedding_query(query)?;
        let query_norm = vector_squared_norm(&query.values);
        if !query_norm.is_finite() || query_norm <= f64::EPSILON {
            return Err(CoreError::invalid(
                "knowledge embedding query vector must have a non-zero finite norm",
            ));
        }

        let connection = self.connection()?;
        validate_embedding_task_space(
            &connection,
            &query.task_profile_revision_id,
            &query.model_route_id,
            query.dimensions,
        )?;
        let rows = load_knowledge_embedding_rows(&connection, query)?;
        if rows.len() > MAX_KNOWLEDGE_EMBEDDINGS_PER_BOOK {
            return Err(corrupted(
                "stored knowledge embeddings exceed the per-book safety limit",
            ));
        }

        let mut matches = score_knowledge_embedding_rows(query, query_norm, &rows)?;
        matches.sort_by(|left, right| {
            right
                .similarity_millionths
                .cmp(&left.similarity_millionths)
                .then_with(|| left.entry_id.cmp(&right.entry_id))
                .then_with(|| left.embedding_id.cmp(&right.embedding_id))
        });
        Ok(matches)
    }

    /// Returns whether every requested entry has one vector in the exact
    /// immutable task/provider space. This preflight never reads vector bytes
    /// and lets Core avoid dispatching a provider query that could only fall
    /// back to lexical selection.
    pub fn knowledge_embedding_space_covers_entries(
        &self,
        query: &KnowledgeEmbeddingCoverageQuery,
    ) -> CoreResult<bool> {
        validate_knowledge_embedding_coverage_query(query)?;
        if query.required_entry_ids.is_empty() {
            return Ok(false);
        }
        let required = query
            .required_entry_ids
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        if required.len() != query.required_entry_ids.len() {
            return Err(CoreError::invalid(
                "knowledge embedding coverage entries contain duplicates",
            ));
        }
        let connection = self.connection()?;
        validate_embedding_task_space(
            &connection,
            &query.task_profile_revision_id,
            &query.model_route_id,
            query.dimensions,
        )?;
        let mut statement = connection
            .prepare(
                "SELECT embedding.entry_id
                 FROM knowledge_embeddings AS embedding
                 JOIN knowledge_entries AS entry
                   ON entry.book_revision_id = embedding.book_revision_id
                  AND entry.entry_id = embedding.entry_id
                 WHERE embedding.book_revision_id = ?1
                   AND embedding.task_profile_revision_id = ?2
                   AND embedding.model_route_id = ?3
                   AND embedding.dimensions = ?4
                   AND embedding.vector_space_sha256 = ?5
                   AND embedding.encoding = 'f32le'
                 ORDER BY embedding.entry_id, embedding.id
                 LIMIT ?6",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map(
                params![
                    query.book_revision_id,
                    query.task_profile_revision_id,
                    query.model_route_id.as_str(),
                    query.dimensions,
                    query.vector_space_sha256,
                    MAX_KNOWLEDGE_EMBEDDING_QUERY_ROWS,
                ],
                |row| row.get::<_, String>(0),
            )
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?;
        if rows.len() > MAX_KNOWLEDGE_EMBEDDINGS_PER_BOOK {
            return Err(corrupted(
                "stored knowledge embeddings exceed the per-book safety limit",
            ));
        }
        let available = rows
            .into_iter()
            .map(KnowledgeEntryId::from)
            .collect::<BTreeSet<_>>();
        Ok(required.is_subset(&available))
    }
}

fn validate_knowledge_embedding_write(write: &KnowledgeEmbeddingWrite) -> CoreResult<()> {
    validate_identifier("knowledge embedding", &write.id)?;
    validate_identifier("knowledge book revision", &write.book_revision_id)?;
    validate_identifier("knowledge entry", write.entry_id.as_str())?;
    validate_identifier(
        "knowledge embedding task profile revision",
        &write.task_profile_revision_id,
    )?;
    validate_identifier(
        "knowledge embedding model route",
        write.model_route_id.as_str(),
    )?;
    validate_sha256(
        "knowledge embedding vector space",
        &write.vector_space_sha256,
    )
}

fn validate_knowledge_embedding_query(query: &KnowledgeEmbeddingQuery) -> CoreResult<()> {
    validate_identifier("knowledge book revision", &query.book_revision_id)?;
    validate_identifier(
        "knowledge embedding task profile revision",
        &query.task_profile_revision_id,
    )?;
    validate_identifier(
        "knowledge embedding model route",
        query.model_route_id.as_str(),
    )?;
    validate_sha256(
        "knowledge embedding vector space",
        &query.vector_space_sha256,
    )?;
    validate_vector(query.dimensions, &query.values).map(|_| ())
}

fn validate_knowledge_embedding_coverage_query(
    query: &KnowledgeEmbeddingCoverageQuery,
) -> CoreResult<()> {
    validate_identifier("knowledge book revision", &query.book_revision_id)?;
    validate_identifier(
        "knowledge embedding task profile revision",
        &query.task_profile_revision_id,
    )?;
    validate_identifier(
        "knowledge embedding model route",
        query.model_route_id.as_str(),
    )?;
    validate_sha256(
        "knowledge embedding vector space",
        &query.vector_space_sha256,
    )?;
    if query.required_entry_ids.len() > MAX_KNOWLEDGE_EMBEDDINGS_PER_BOOK {
        return Err(CoreError::invalid(
            "knowledge embedding coverage exceeds the per-book safety limit",
        ));
    }
    for entry_id in &query.required_entry_ids {
        validate_identifier("knowledge entry", entry_id.as_str())?;
    }
    Ok(())
}

fn load_existing_knowledge_embedding(
    connection: &rusqlite::Connection,
    write: &KnowledgeEmbeddingWrite,
) -> CoreResult<Option<StoredKnowledgeEmbeddingReplay>> {
    connection
        .query_row(
            "SELECT id, vector_sha256, vector_blob, created_at
             FROM knowledge_embeddings
             WHERE book_revision_id = ?1
               AND entry_id = ?2
               AND task_profile_revision_id = ?3
               AND model_route_id = ?4
               AND dimensions = ?5
               AND vector_space_sha256 = ?6",
            params![
                write.book_revision_id,
                write.entry_id.as_str(),
                write.task_profile_revision_id,
                write.model_route_id.as_str(),
                write.dimensions,
                write.vector_space_sha256,
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)
}

fn load_knowledge_embedding_rows(
    connection: &rusqlite::Connection,
    query: &KnowledgeEmbeddingQuery,
) -> CoreResult<Vec<StoredKnowledgeEmbeddingRow>> {
    let mut statement = connection
        .prepare(
            "SELECT embedding.id, embedding.entry_id,
                    embedding.vector_sha256, embedding.vector_blob
             FROM knowledge_embeddings AS embedding
             JOIN knowledge_entries AS entry
               ON entry.book_revision_id = embedding.book_revision_id
              AND entry.entry_id = embedding.entry_id
             WHERE embedding.book_revision_id = ?1
               AND embedding.task_profile_revision_id = ?2
               AND embedding.model_route_id = ?3
               AND embedding.dimensions = ?4
               AND embedding.vector_space_sha256 = ?5
               AND embedding.encoding = 'f32le'
             ORDER BY embedding.entry_id, embedding.id
             LIMIT ?6",
        )
        .map_err(storage_db_error)?;
    statement
        .query_map(
            params![
                query.book_revision_id,
                query.task_profile_revision_id,
                query.model_route_id.as_str(),
                query.dimensions,
                query.vector_space_sha256,
                MAX_KNOWLEDGE_EMBEDDING_QUERY_ROWS,
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                ))
            },
        )
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)
}

fn score_knowledge_embedding_rows(
    query: &KnowledgeEmbeddingQuery,
    query_norm: f64,
    rows: &[StoredKnowledgeEmbeddingRow],
) -> CoreResult<Vec<KnowledgeEmbeddingMatch>> {
    let mut matches = Vec::with_capacity(rows.len());
    let mut previous_entry_id: Option<&str> = None;
    for (embedding_id, entry_id, vector_sha256, bytes) in rows {
        if previous_entry_id == Some(entry_id.as_str()) {
            return Err(corrupted(
                "knowledge entry has ambiguous embeddings in one exact vector space",
            ));
        }
        previous_entry_id = Some(entry_id);
        let values = decode_vector(query.dimensions, bytes, vector_sha256)?;
        let candidate_norm = vector_squared_norm(&values);
        if !candidate_norm.is_finite() || candidate_norm <= f64::EPSILON {
            return Err(corrupted(
                "stored knowledge embedding has a zero or non-finite norm",
            ));
        }
        let dot = query
            .values
            .iter()
            .zip(&values)
            .map(|(left, right)| f64::from(*left) * f64::from(*right))
            .sum::<f64>();
        let similarity = (dot / (query_norm * candidate_norm).sqrt()).clamp(-1.0, 1.0);
        matches.push(KnowledgeEmbeddingMatch {
            embedding_id: embedding_id.clone(),
            entry_id: KnowledgeEntryId::from(entry_id.clone()),
            vector_sha256: vector_sha256.clone(),
            similarity_millionths: similarity_millionths(similarity),
        });
    }
    Ok(matches)
}

fn similarity_millionths(similarity: f64) -> u32 {
    debug_assert!(similarity.is_finite() && (-1.0..=1.0).contains(&similarity));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let fixed = (similarity.max(0.0) * 1_000_000.0).round() as u32;
    fixed
}

fn validate_embedding_task_space(
    connection: &rusqlite::Connection,
    task_profile_revision_id: &str,
    model_route_id: &ModelRouteId,
    dimensions: u32,
) -> CoreResult<()> {
    let payload = connection
        .query_row(
            "SELECT payload_json FROM task_profile_revisions
             WHERE revision_id = ?1 AND task_kind = 'memory_embedding'
               AND model_route_id = ?2",
            params![task_profile_revision_id, model_route_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "exact knowledge embedding task profile was not found",
                false,
            )
        })?;
    let task = serde_json::from_str::<TaskProfile>(&payload).map_err(|error| {
        corrupted(format!(
            "stored knowledge embedding task profile is invalid: {error}"
        ))
    })?;
    task.validate().map_err(|error| {
        corrupted(format!(
            "stored knowledge embedding task profile failed validation: {error}"
        ))
    })?;
    if task.kind != AuxiliaryTaskKind::MemoryEmbedding
        || task.route_id != *model_route_id
        || task.embedding_dimensions != Some(dimensions)
    {
        return Err(CoreError::invalid(
            "knowledge embedding route and dimensions do not match the exact task profile",
        ));
    }
    Ok(())
}

fn validate_vector(dimensions: u32, values: &[f32]) -> CoreResult<usize> {
    let dimensions = usize::try_from(dimensions)
        .map_err(|_| CoreError::invalid("knowledge embedding dimensions are invalid"))?;
    if dimensions == 0
        || dimensions > MAX_KNOWLEDGE_EMBEDDING_DIMENSIONS
        || values.len() != dimensions
        || values.iter().any(|value| !value.is_finite())
    {
        return Err(CoreError::invalid(
            "knowledge embedding values do not match finite declared dimensions",
        ));
    }
    Ok(dimensions)
}

fn encode_vector(dimensions: u32, values: &[f32]) -> CoreResult<(Vec<u8>, String)> {
    let dimensions = validate_vector(dimensions, values)?;
    let norm = vector_squared_norm(values);
    if !norm.is_finite() || norm <= f64::EPSILON {
        return Err(CoreError::invalid(
            "knowledge embedding vector must have a non-zero finite norm",
        ));
    }
    let mut bytes = Vec::with_capacity(dimensions.saturating_mul(4));
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    Ok((bytes, sha256))
}

fn decode_vector(dimensions: u32, bytes: &[u8], expected_sha256: &str) -> CoreResult<Vec<f32>> {
    let dimensions = usize::try_from(dimensions)
        .map_err(|_| corrupted("stored knowledge embedding dimensions are invalid"))?;
    let expected_len = dimensions
        .checked_mul(4)
        .ok_or_else(|| corrupted("stored knowledge embedding byte size overflow"))?;
    if bytes.len() != expected_len {
        return Err(corrupted(
            "stored knowledge embedding byte length is invalid",
        ));
    }
    if format!("{:x}", Sha256::digest(bytes)) != expected_sha256 {
        return Err(corrupted("stored knowledge embedding digest is invalid"));
    }
    let values = bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect::<Vec<_>>();
    if values.iter().any(|value| !value.is_finite()) {
        return Err(corrupted(
            "stored knowledge embedding contains a non-finite value",
        ));
    }
    Ok(values)
}

fn vector_squared_norm(values: &[f32]) -> f64 {
    values
        .iter()
        .map(|value| {
            let value = f64::from(*value);
            value * value
        })
        .sum()
}

fn validate_identifier(label: &str, value: &str) -> CoreResult<()> {
    if value.is_empty() || value.len() > 512 || value.trim() != value {
        return Err(CoreError::invalid(format!("{label} is invalid")));
    }
    Ok(())
}

fn validate_sha256(label: &str, value: &str) -> CoreResult<()> {
    if value.len() != 64
        || !value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(CoreError::invalid(format!("{label} is invalid")));
    }
    Ok(())
}

fn corrupted(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::StorageCorrupted, message, false)
}
