use super::*;
use crate::memory_queue::vector_squared_norm;

fn query(values: Vec<f32>) -> MemoryEmbeddingQuery {
    MemoryEmbeddingQuery {
        conversation_id: lorepia_domain::ConversationId("conversation".into()),
        branch_id: lorepia_domain::ConversationBranchId("branch".into()),
        context_head_message_id: lorepia_domain::MessageId("head".into()),
        task_profile_revision_id: "task".into(),
        model_route_id: "route".into(),
        dimensions: u32::try_from(values.len()).unwrap(),
        vector_space_sha256: "a".repeat(64),
        values,
        candidate_limit: 1,
        result_limit: 1,
    }
}

fn candidate(values: &[f32]) -> MemoryEmbeddingCandidate {
    let bytes = values
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect::<Vec<_>>();
    MemoryEmbeddingCandidate {
        embedding_id: "embedding".into(),
        record_id: "record".into(),
        revision_id: "revision".into(),
        vector_sha256: hex::encode(Sha256::digest(&bytes)),
        vector_blob: bytes,
    }
}

#[test]
fn encoded_scores_match_decoded_reference() {
    for dimensions in [1, 3, 127, 1_536, 32_768] {
        for seed in 0..8 {
            let values = (0..dimensions)
                .map(|i| f32::from(u16::try_from((i * 17 + seed * 31) % 103).unwrap()) - 51.0)
                .collect::<Vec<_>>();
            let query = query(values);
            for vector in [
                query.values.clone(),
                query.values.iter().map(|value| -*value).collect(),
                (0..dimensions)
                    .map(|i| {
                        if i % 2 == 0 {
                            f32::MAX
                        } else {
                            f32::MIN_POSITIVE
                        }
                    })
                    .collect(),
                vec![-0.0; dimensions],
            ] {
                let candidate = candidate(&vector);
                let decoded = decode_memory_embedding_vector(
                    query.dimensions,
                    &candidate.vector_blob,
                    &candidate.vector_sha256,
                )
                .unwrap();
                let norm = vector_squared_norm(&decoded);
                let query_norm = vector_squared_norm(&query.values);
                let actual =
                    score_memory_embedding_candidates(&query, query_norm, vec![candidate]).unwrap();
                if norm <= f64::EPSILON {
                    assert!(actual.is_empty());
                } else {
                    let dot = query
                        .values
                        .iter()
                        .zip(&decoded)
                        .map(|(left, right)| f64::from(*left) * f64::from(*right))
                        .sum::<f64>();
                    let expected =
                        similarity_millionths((dot / (query_norm * norm).sqrt()).clamp(-1.0, 1.0))
                            .unwrap();
                    assert_eq!(actual[0].similarity_millionths, expected);
                }
            }
        }
    }
}

#[test]
fn encoded_scores_preserve_corruption_errors() {
    let query = query(vec![1.0, 2.0, 3.0]);
    for fault in 0..5 {
        let mut candidate = candidate(&[1.0, 2.0, 3.0]);
        match fault {
            0 => {
                candidate.vector_blob.pop();
            }
            1 => candidate.vector_sha256 = "0".repeat(64),
            _ => {
                let value = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY][fault - 2];
                candidate.vector_blob[8..12].copy_from_slice(&value.to_le_bytes());
                candidate.vector_sha256 = hex::encode(Sha256::digest(&candidate.vector_blob));
            }
        }
        let expected = decode_memory_embedding_vector(
            query.dimensions,
            &candidate.vector_blob,
            &candidate.vector_sha256,
        )
        .unwrap_err();
        let actual = score_memory_embedding_candidates(
            &query,
            vector_squared_norm(&query.values),
            vec![candidate],
        )
        .unwrap_err();
        assert_eq!(actual.code, expected.code);
        assert_eq!(actual.message, expected.message);
    }
}
