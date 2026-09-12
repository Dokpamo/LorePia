use super::{
    CoreResult, MemoryEmbeddingCandidate, MemoryEmbeddingMatch, MemoryEmbeddingQuery,
    MemoryRecordId, corrupted, similarity_millionths, validate_memory_embedding_dimensions,
};
use sha2::{Digest, Sha256};

pub(super) fn score_memory_embedding_candidates(
    query: &MemoryEmbeddingQuery,
    query_norm: f64,
    candidates: Vec<MemoryEmbeddingCandidate>,
) -> CoreResult<Vec<MemoryEmbeddingMatch>> {
    let mut matches = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        validate_encoded_vector(
            query.dimensions,
            &candidate.vector_blob,
            &candidate.vector_sha256,
        )?;
        let mut candidate_norm = 0.0_f64;
        let mut dot = 0.0_f64;
        for (left, chunk) in query
            .values
            .iter()
            .zip(candidate.vector_blob.chunks_exact(4))
        {
            let value = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            if !value.is_finite() {
                return Err(corrupted(
                    "stored memory embedding contains a non-finite value",
                ));
            }
            let value = f64::from(value);
            candidate_norm += value * value;
            dot += f64::from(*left) * value;
        }
        if !candidate_norm.is_finite() || candidate_norm <= f64::EPSILON {
            continue;
        }
        let similarity = (dot / (query_norm * candidate_norm).sqrt()).clamp(-1.0, 1.0);
        matches.push(MemoryEmbeddingMatch {
            embedding_id: candidate.embedding_id,
            memory_record_id: MemoryRecordId::from(candidate.record_id),
            memory_record_revision_id: candidate.revision_id,
            vector_sha256: candidate.vector_sha256,
            similarity_millionths: similarity_millionths(similarity)?,
        });
    }
    Ok(matches)
}

pub(super) fn decode_memory_embedding_vector(
    dimensions: u32,
    bytes: &[u8],
    expected_sha256: &str,
) -> CoreResult<Vec<f32>> {
    validate_encoded_vector(dimensions, bytes, expected_sha256)?;
    let values = bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect::<Vec<_>>();
    if values.iter().any(|value| !value.is_finite()) {
        return Err(corrupted(
            "stored memory embedding contains a non-finite value",
        ));
    }
    Ok(values)
}

fn validate_encoded_vector(dimensions: u32, bytes: &[u8], expected_sha256: &str) -> CoreResult<()> {
    let dimensions = validate_memory_embedding_dimensions(dimensions).map_err(|error| {
        corrupted(format!(
            "stored memory embedding dimensions are invalid: {}",
            error.message
        ))
    })?;
    let expected_len = dimensions
        .checked_mul(4)
        .ok_or_else(|| corrupted("stored memory embedding byte size overflow"))?;
    if bytes.len() != expected_len {
        return Err(corrupted("stored memory embedding byte length is invalid"));
    }
    if hex::encode(Sha256::digest(bytes)) != expected_sha256 {
        return Err(corrupted("stored memory embedding digest is invalid"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
