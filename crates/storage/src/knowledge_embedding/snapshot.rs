use super::{
    CoreResult, KnowledgeEmbeddingMatch, KnowledgeEmbeddingQuery, KnowledgeEmbeddingWorkMeter,
    KnowledgeEntryId, MAX_KNOWLEDGE_EMBEDDING_QUERY_ROWS, MAX_KNOWLEDGE_EMBEDDINGS_PER_BOOK,
    bind_exact_space_query, bind_required_entries, charge_stored_embedding_row, corrupted,
    required_entry_filter, score_encoded_vector, similarity_millionths, storage_db_error,
    stored_length, validate_stored_row_lengths,
};

pub(super) struct Candidate {
    embedding_id: String,
    entry_id: String,
    vector_sha256: String,
    vector: Vec<u8>,
}

// Errors are retained in row order: an earlier corrupt vector must still win
// over a later malformed row or budget failure, exactly as streaming scoring did.
pub(super) fn load(
    connection: &rusqlite::Connection,
    query: &KnowledgeEmbeddingQuery,
    required_entry_ids: &[KnowledgeEntryId],
    work: &mut KnowledgeEmbeddingWorkMeter,
) -> CoreResult<Vec<CoreResult<Candidate>>> {
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
    let mut candidates = Vec::new();
    loop {
        let row = match rows.next().map_err(storage_db_error) {
            Ok(Some(row)) => row,
            Ok(None) => break,
            Err(error) => {
                candidates.push(Err(error));
                break;
            }
        };
        let candidate = (|| {
            if candidates.len() >= MAX_KNOWLEDGE_EMBEDDINGS_PER_BOOK {
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
            let vector = row.get_ref(7).map_err(storage_db_error)?;
            let type_error =
                || rusqlite::Error::InvalidColumnType(7, "vector_blob".into(), vector.data_type());
            let bytes = vector
                .as_blob()
                .map_err(|_| storage_db_error(type_error()))?;
            Ok(Candidate {
                embedding_id,
                entry_id,
                vector_sha256,
                vector: bytes.to_vec(),
            })
        })();
        let failed = candidate.is_err();
        candidates.push(candidate);
        if failed {
            break;
        }
    }
    Ok(candidates)
}

pub(super) fn score(
    candidates: Vec<CoreResult<Candidate>>,
    query: &KnowledgeEmbeddingQuery,
    query_norm: f64,
) -> CoreResult<Vec<KnowledgeEmbeddingMatch>> {
    let mut matches = Vec::with_capacity(candidates.len());
    let mut previous_entry_id: Option<String> = None;
    for candidate in candidates {
        let Candidate {
            embedding_id,
            entry_id,
            vector_sha256,
            vector,
        } = candidate?;
        if previous_entry_id.as_deref() == Some(entry_id.as_str()) {
            return Err(corrupted(
                "knowledge entry has ambiguous embeddings in one exact vector space",
            ));
        }
        previous_entry_id = Some(entry_id.clone());
        let similarity = score_encoded_vector(query, query_norm, &vector, &vector_sha256)?;
        matches.push(KnowledgeEmbeddingMatch {
            embedding_id,
            entry_id: KnowledgeEntryId::from(entry_id),
            vector_sha256,
            similarity_millionths: similarity_millionths(similarity),
        });
    }
    Ok(matches)
}

#[cfg(test)]
mod tests;
