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
use lorepia_orchestration::MAX_GENERATION_KNOWLEDGE_WORK_BYTES;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Storage, database::storage_db_error};

const MAX_KNOWLEDGE_EMBEDDING_DIMENSIONS: usize = 32_768;
const MAX_KNOWLEDGE_EMBEDDINGS_PER_BOOK: usize = 10_000;
const MAX_KNOWLEDGE_EMBEDDING_QUERY_ROWS: i64 = 10_001;
const VECTOR_ELEMENT_BYTES: usize = std::mem::size_of::<f32>();
const QUERY_VECTOR_PASSES: usize = 2;
const STORED_VECTOR_PASSES: usize = 4;

type StoredKnowledgeEmbeddingReplay = (String, String, Vec<u8>, String);

struct KnowledgeEmbeddingWorkMeter {
    limit: usize,
    used: usize,
}

impl KnowledgeEmbeddingWorkMeter {
    fn new(limit: usize) -> Self {
        Self {
            limit: limit.min(MAX_GENERATION_KNOWLEDGE_WORK_BYTES),
            used: 0,
        }
    }

    fn charge(&mut self, work_bytes: usize) -> CoreResult<()> {
        let next = self.used.checked_add(work_bytes).ok_or_else(|| {
            CoreError::invalid("knowledge embedding query work budget overflowed")
        })?;
        if next > self.limit {
            return Err(CoreError::invalid(
                "knowledge embedding query exceeds the remaining generation work budget",
            ));
        }
        self.used = next;
        Ok(())
    }

    const fn used(&self) -> usize {
        self.used
    }
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnowledgeEmbeddingQueryResult {
    pub matches: Vec<KnowledgeEmbeddingMatch>,
    pub work_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnowledgeEmbeddingCoverageResult {
    pub covered: bool,
    pub work_bytes: usize,
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
        self.query_knowledge_embeddings_cosine_internal(
            query,
            &[],
            MAX_GENERATION_KNOWLEDGE_WORK_BYTES,
        )
        .map(|result| result.matches)
    }

    /// Scores only requested entries while respecting a caller-owned work
    /// allowance. The returned work is charged by Core to the generation-wide
    /// budget.
    pub fn query_required_knowledge_embeddings_cosine_bounded(
        &self,
        query: &KnowledgeEmbeddingQuery,
        required_entry_ids: &[KnowledgeEntryId],
        max_work_bytes: usize,
    ) -> CoreResult<KnowledgeEmbeddingQueryResult> {
        if required_entry_ids.is_empty() {
            return Err(CoreError::invalid(
                "knowledge embedding query requires at least one entry",
            ));
        }
        self.query_knowledge_embeddings_cosine_internal(query, required_entry_ids, max_work_bytes)
    }

    /// Returns whether every requested entry has one vector in the exact
    /// immutable task/provider space. This preflight never reads vector bytes
    /// and lets Core avoid dispatching a provider query that could only fall
    /// back to lexical selection.
    pub fn knowledge_embedding_space_covers_entries(
        &self,
        query: &KnowledgeEmbeddingCoverageQuery,
    ) -> CoreResult<bool> {
        self.knowledge_embedding_space_covers_entries_bounded(
            query,
            MAX_GENERATION_KNOWLEDGE_WORK_BYTES,
        )
        .map(|result| result.covered)
    }

    /// Bounded coverage preflight for a caller-owned generation work budget.
    pub fn knowledge_embedding_space_covers_entries_bounded(
        &self,
        query: &KnowledgeEmbeddingCoverageQuery,
        max_work_bytes: usize,
    ) -> CoreResult<KnowledgeEmbeddingCoverageResult> {
        let mut work = KnowledgeEmbeddingWorkMeter::new(max_work_bytes);
        validate_knowledge_embedding_coverage_query(query, &mut work)?;
        if query.required_entry_ids.is_empty() {
            return Ok(KnowledgeEmbeddingCoverageResult {
                covered: false,
                work_bytes: work.used(),
            });
        }
        let connection = self.connection()?;
        validate_embedding_task_space(
            &connection,
            &query.task_profile_revision_id,
            &query.model_route_id,
            query.dimensions,
        )?;
        let entry_filter = required_entry_filter(query.required_entry_ids.len(), &mut work)?;
        let sql = format!(
            "SELECT COUNT(*)
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
               {entry_filter}"
        );
        let mut statement = connection.prepare(&sql).map_err(storage_db_error)?;
        bind_exact_space_coverage_query(&mut statement, query)?;
        bind_required_entries(&mut statement, &query.required_entry_ids)?;
        let available = statement
            .raw_query()
            .next()
            .map_err(storage_db_error)?
            .ok_or_else(|| corrupted("knowledge embedding coverage count disappeared"))?
            .get::<_, i64>(0)
            .map_err(storage_db_error)?;
        let available = usize::try_from(available)
            .map_err(|_| corrupted("knowledge embedding coverage count is invalid"))?;
        if available > query.required_entry_ids.len() {
            return Err(corrupted(
                "knowledge embedding coverage exceeds requested entries",
            ));
        }
        Ok(KnowledgeEmbeddingCoverageResult {
            covered: available == query.required_entry_ids.len(),
            work_bytes: work.used(),
        })
    }

    fn query_knowledge_embeddings_cosine_internal(
        &self,
        query: &KnowledgeEmbeddingQuery,
        required_entry_ids: &[KnowledgeEntryId],
        max_work_bytes: usize,
    ) -> CoreResult<KnowledgeEmbeddingQueryResult> {
        let mut work = KnowledgeEmbeddingWorkMeter::new(max_work_bytes);
        let query_norm = validate_knowledge_embedding_query(query, &mut work)?;
        validate_required_entries(required_entry_ids, &mut work)?;
        let connection = self.connection()?;
        validate_embedding_task_space(
            &connection,
            &query.task_profile_revision_id,
            &query.model_route_id,
            query.dimensions,
        )?;
        let mut matches = score_knowledge_embedding_rows(
            &connection,
            query,
            query_norm,
            required_entry_ids,
            &mut work,
        )?;
        charge_embedding_sort(&matches, &mut work)?;
        matches.sort_unstable_by(|left, right| {
            right
                .similarity_millionths
                .cmp(&left.similarity_millionths)
                .then_with(|| left.entry_id.cmp(&right.entry_id))
                .then_with(|| left.embedding_id.cmp(&right.embedding_id))
        });
        Ok(KnowledgeEmbeddingQueryResult {
            matches,
            work_bytes: work.used(),
        })
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

fn validate_knowledge_embedding_query(
    query: &KnowledgeEmbeddingQuery,
    work: &mut KnowledgeEmbeddingWorkMeter,
) -> CoreResult<f64> {
    charge_exact_space_identifiers(
        &query.book_revision_id,
        &query.task_profile_revision_id,
        query.model_route_id.as_str(),
        &query.vector_space_sha256,
        work,
    )?;
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
    let dimensions = validate_vector_shape(query.dimensions, query.values.len())?;
    charge_vector_passes(dimensions, QUERY_VECTOR_PASSES, work)?;
    if query.values.iter().any(|value| !value.is_finite()) {
        return Err(CoreError::invalid(
            "knowledge embedding values do not match finite declared dimensions",
        ));
    }
    let query_norm = vector_squared_norm(&query.values);
    if !query_norm.is_finite() || query_norm <= f64::EPSILON {
        return Err(CoreError::invalid(
            "knowledge embedding query vector must have a non-zero finite norm",
        ));
    }
    Ok(query_norm)
}

fn validate_knowledge_embedding_coverage_query(
    query: &KnowledgeEmbeddingCoverageQuery,
    work: &mut KnowledgeEmbeddingWorkMeter,
) -> CoreResult<()> {
    charge_exact_space_identifiers(
        &query.book_revision_id,
        &query.task_profile_revision_id,
        query.model_route_id.as_str(),
        &query.vector_space_sha256,
        work,
    )?;
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
    validate_dimensions(query.dimensions)?;
    validate_required_entries(&query.required_entry_ids, work)
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

fn score_knowledge_embedding_rows(
    connection: &rusqlite::Connection,
    query: &KnowledgeEmbeddingQuery,
    query_norm: f64,
    required_entry_ids: &[KnowledgeEntryId],
    work: &mut KnowledgeEmbeddingWorkMeter,
) -> CoreResult<Vec<KnowledgeEmbeddingMatch>> {
    let entry_filter = required_entry_filter(required_entry_ids.len(), work)?;
    let sql = format!(
        "SELECT length(embedding.id), length(embedding.entry_id),
                length(embedding.vector_sha256), length(embedding.vector_blob),
                embedding.id, embedding.entry_id,
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
           {entry_filter}
         ORDER BY embedding.entry_id, embedding.id
         LIMIT {MAX_KNOWLEDGE_EMBEDDING_QUERY_ROWS}"
    );
    let mut statement = connection.prepare(&sql).map_err(storage_db_error)?;
    bind_exact_space_query(&mut statement, query)?;
    bind_required_entries(&mut statement, required_entry_ids)?;
    let mut rows = statement.raw_query();
    let mut matches = Vec::new();
    let mut previous_entry_id: Option<String> = None;
    while let Some(row) = rows.next().map_err(storage_db_error)? {
        if matches.len() >= MAX_KNOWLEDGE_EMBEDDINGS_PER_BOOK {
            return Err(corrupted(
                "stored knowledge embeddings exceed the per-book safety limit",
            ));
        }
        let embedding_id_len = stored_length(row, 0, "knowledge embedding id")?;
        let entry_id_len = stored_length(row, 1, "knowledge entry id")?;
        let vector_sha256_len = stored_length(row, 2, "knowledge embedding digest")?;
        let vector_blob_len = stored_length(row, 3, "knowledge embedding vector")?;
        validate_stored_row_lengths(
            query.dimensions,
            embedding_id_len,
            entry_id_len,
            vector_sha256_len,
            vector_blob_len,
        )?;
        charge_stored_embedding_row(
            embedding_id_len,
            entry_id_len,
            vector_sha256_len,
            vector_blob_len,
            work,
        )?;
        let embedding_id = row.get::<_, String>(4).map_err(storage_db_error)?;
        let entry_id = row.get::<_, String>(5).map_err(storage_db_error)?;
        let vector_sha256 = row.get::<_, String>(6).map_err(storage_db_error)?;
        let bytes = row.get::<_, Vec<u8>>(7).map_err(storage_db_error)?;
        if previous_entry_id.as_deref() == Some(entry_id.as_str()) {
            return Err(corrupted(
                "knowledge entry has ambiguous embeddings in one exact vector space",
            ));
        }
        previous_entry_id = Some(entry_id.clone());
        let similarity = score_encoded_vector(query, query_norm, &bytes, &vector_sha256)?;
        matches.push(KnowledgeEmbeddingMatch {
            embedding_id,
            entry_id: KnowledgeEntryId::from(entry_id),
            vector_sha256,
            similarity_millionths: similarity_millionths(similarity),
        });
    }
    Ok(matches)
}

fn score_encoded_vector(
    query: &KnowledgeEmbeddingQuery,
    query_norm: f64,
    bytes: &[u8],
    expected_sha256: &str,
) -> CoreResult<f64> {
    if format!("{:x}", Sha256::digest(bytes)) != expected_sha256 {
        return Err(corrupted("stored knowledge embedding digest is invalid"));
    }
    let mut candidate_norm = 0.0_f64;
    let mut dot = 0.0_f64;
    for (query_value, chunk) in query
        .values
        .iter()
        .zip(bytes.chunks_exact(VECTOR_ELEMENT_BYTES))
    {
        let candidate = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        if !candidate.is_finite() {
            return Err(corrupted(
                "stored knowledge embedding contains a non-finite value",
            ));
        }
        let candidate = f64::from(candidate);
        candidate_norm += candidate * candidate;
        dot += f64::from(*query_value) * candidate;
    }
    if !candidate_norm.is_finite() || candidate_norm <= f64::EPSILON {
        return Err(corrupted(
            "stored knowledge embedding has a zero or non-finite norm",
        ));
    }
    Ok((dot / (query_norm * candidate_norm).sqrt()).clamp(-1.0, 1.0))
}

fn stored_length(row: &rusqlite::Row<'_>, index: usize, label: &str) -> CoreResult<usize> {
    let length = row.get::<_, i64>(index).map_err(storage_db_error)?;
    usize::try_from(length).map_err(|_| corrupted(format!("stored {label} length is invalid")))
}

fn validate_stored_row_lengths(
    dimensions: u32,
    embedding_id_len: usize,
    entry_id_len: usize,
    vector_sha256_len: usize,
    vector_blob_len: usize,
) -> CoreResult<()> {
    if embedding_id_len == 0 || embedding_id_len > 512 || entry_id_len == 0 || entry_id_len > 512 {
        return Err(corrupted(
            "stored knowledge embedding identifier is invalid",
        ));
    }
    if vector_sha256_len != 64 {
        return Err(corrupted("stored knowledge embedding digest is invalid"));
    }
    let expected_blob_len = validate_dimensions(dimensions)?
        .checked_mul(VECTOR_ELEMENT_BYTES)
        .ok_or_else(|| corrupted("stored knowledge embedding byte size overflow"))?;
    if vector_blob_len != expected_blob_len {
        return Err(corrupted(
            "stored knowledge embedding byte length is invalid",
        ));
    }
    Ok(())
}

fn charge_stored_embedding_row(
    embedding_id_len: usize,
    entry_id_len: usize,
    vector_sha256_len: usize,
    vector_blob_len: usize,
    work: &mut KnowledgeEmbeddingWorkMeter,
) -> CoreResult<()> {
    let metadata_work = embedding_id_len
        .checked_add(entry_id_len)
        .and_then(|value| value.checked_add(vector_sha256_len))
        .and_then(|value| value.checked_mul(2))
        .ok_or_else(|| CoreError::invalid("knowledge embedding row work overflowed"))?;
    let vector_work = vector_blob_len
        .checked_mul(STORED_VECTOR_PASSES)
        .ok_or_else(|| CoreError::invalid("knowledge embedding vector work overflowed"))?;
    work.charge(
        metadata_work
            .checked_add(vector_work)
            .and_then(|value| value.checked_add(std::mem::size_of::<KnowledgeEmbeddingMatch>()))
            .ok_or_else(|| CoreError::invalid("knowledge embedding row work overflowed"))?,
    )
}

fn validate_required_entries(
    required_entry_ids: &[KnowledgeEntryId],
    work: &mut KnowledgeEmbeddingWorkMeter,
) -> CoreResult<()> {
    if required_entry_ids.len() > MAX_KNOWLEDGE_EMBEDDINGS_PER_BOOK {
        return Err(CoreError::invalid(
            "knowledge embedding entries exceed the per-book safety limit",
        ));
    }
    let mut seen = BTreeSet::new();
    for entry_id in required_entry_ids {
        let identifier_work = entry_id
            .as_str()
            .len()
            .checked_add(std::mem::size_of::<&str>())
            .ok_or_else(|| CoreError::invalid("knowledge embedding entry work overflowed"))?;
        work.charge(identifier_work)?;
        validate_identifier("knowledge entry", entry_id.as_str())?;
        if !seen.insert(entry_id.as_str()) {
            return Err(CoreError::invalid(
                "knowledge embedding entries contain duplicates",
            ));
        }
    }
    Ok(())
}

fn required_entry_filter(
    required_entry_count: usize,
    work: &mut KnowledgeEmbeddingWorkMeter,
) -> CoreResult<String> {
    if required_entry_count == 0 {
        return Ok(String::new());
    }
    let construction_work = required_entry_count
        .checked_mul(8)
        .and_then(|value| value.checked_add(64))
        .ok_or_else(|| CoreError::invalid("knowledge embedding query work overflowed"))?;
    work.charge(construction_work)?;
    let mut filter = String::from("AND embedding.entry_id IN (");
    for index in 0..required_entry_count {
        if index > 0 {
            filter.push(',');
        }
        filter.push('?');
        filter.push_str(&(index + 6).to_string());
    }
    filter.push(')');
    Ok(filter)
}

fn bind_exact_space_query(
    statement: &mut rusqlite::Statement<'_>,
    query: &KnowledgeEmbeddingQuery,
) -> CoreResult<()> {
    statement
        .raw_bind_parameter(1, query.book_revision_id.as_str())
        .and_then(|()| statement.raw_bind_parameter(2, query.task_profile_revision_id.as_str()))
        .and_then(|()| statement.raw_bind_parameter(3, query.model_route_id.as_str()))
        .and_then(|()| statement.raw_bind_parameter(4, query.dimensions))
        .and_then(|()| statement.raw_bind_parameter(5, query.vector_space_sha256.as_str()))
        .map_err(storage_db_error)
}

fn bind_exact_space_coverage_query(
    statement: &mut rusqlite::Statement<'_>,
    query: &KnowledgeEmbeddingCoverageQuery,
) -> CoreResult<()> {
    statement
        .raw_bind_parameter(1, query.book_revision_id.as_str())
        .and_then(|()| statement.raw_bind_parameter(2, query.task_profile_revision_id.as_str()))
        .and_then(|()| statement.raw_bind_parameter(3, query.model_route_id.as_str()))
        .and_then(|()| statement.raw_bind_parameter(4, query.dimensions))
        .and_then(|()| statement.raw_bind_parameter(5, query.vector_space_sha256.as_str()))
        .map_err(storage_db_error)
}

fn bind_required_entries(
    statement: &mut rusqlite::Statement<'_>,
    required_entry_ids: &[KnowledgeEntryId],
) -> CoreResult<()> {
    for (index, entry_id) in required_entry_ids.iter().enumerate() {
        statement
            .raw_bind_parameter(index + 6, entry_id.as_str())
            .map_err(storage_db_error)?;
    }
    Ok(())
}

fn charge_exact_space_identifiers(
    book_revision_id: &str,
    task_profile_revision_id: &str,
    model_route_id: &str,
    vector_space_sha256: &str,
    work: &mut KnowledgeEmbeddingWorkMeter,
) -> CoreResult<()> {
    let identifier_work = book_revision_id
        .len()
        .checked_add(task_profile_revision_id.len())
        .and_then(|value| value.checked_add(model_route_id.len()))
        .and_then(|value| value.checked_add(vector_space_sha256.len()))
        .ok_or_else(|| CoreError::invalid("knowledge embedding identifier work overflowed"))?;
    work.charge(identifier_work)
}

fn charge_vector_passes(
    dimensions: usize,
    passes: usize,
    work: &mut KnowledgeEmbeddingWorkMeter,
) -> CoreResult<()> {
    let bytes = dimensions
        .checked_mul(VECTOR_ELEMENT_BYTES)
        .and_then(|value| value.checked_mul(passes))
        .ok_or_else(|| CoreError::invalid("knowledge embedding vector work overflowed"))?;
    work.charge(bytes)
}

fn charge_embedding_sort(
    matches: &[KnowledgeEmbeddingMatch],
    work: &mut KnowledgeEmbeddingWorkMeter,
) -> CoreResult<()> {
    if matches.len() < 2 {
        return Ok(());
    }
    let max_entry_id_len = matches
        .iter()
        .map(|candidate| candidate.entry_id.as_str().len())
        .max()
        .unwrap_or(0);
    let max_embedding_id_len = matches
        .iter()
        .map(|candidate| candidate.embedding_id.len())
        .max()
        .unwrap_or(0);
    let comparison_work = std::mem::size_of::<u32>()
        .checked_add(max_entry_id_len.saturating_mul(2))
        .and_then(|value| value.checked_add(max_embedding_id_len.saturating_mul(2)))
        .ok_or_else(|| CoreError::invalid("knowledge embedding sort work overflowed"))?;
    let sort_levels = usize::try_from(usize::BITS - (matches.len() - 1).leading_zeros())
        .map_err(|_| CoreError::invalid("knowledge embedding sort work overflowed"))?;
    let comparisons = matches
        .len()
        .checked_mul(sort_levels)
        .and_then(|value| value.checked_mul(8))
        .ok_or_else(|| CoreError::invalid("knowledge embedding sort work overflowed"))?;
    work.charge(
        comparisons
            .checked_mul(comparison_work)
            .ok_or_else(|| CoreError::invalid("knowledge embedding sort work overflowed"))?,
    )
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
    let dimensions = validate_vector_shape(dimensions, values.len())?;
    if values.iter().any(|value| !value.is_finite()) {
        return Err(CoreError::invalid(
            "knowledge embedding values do not match finite declared dimensions",
        ));
    }
    Ok(dimensions)
}

fn validate_vector_shape(dimensions: u32, value_count: usize) -> CoreResult<usize> {
    let dimensions = validate_dimensions(dimensions)?;
    if value_count != dimensions {
        return Err(CoreError::invalid(
            "knowledge embedding values do not match finite declared dimensions",
        ));
    }
    Ok(dimensions)
}

fn validate_dimensions(dimensions: u32) -> CoreResult<usize> {
    let dimensions = usize::try_from(dimensions)
        .map_err(|_| CoreError::invalid("knowledge embedding dimensions are invalid"))?;
    if dimensions == 0 || dimensions > MAX_KNOWLEDGE_EMBEDDING_DIMENSIONS {
        return Err(CoreError::invalid(
            "knowledge embedding dimensions are invalid",
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
