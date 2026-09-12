use super::*;
use lorepia_domain::{CoreError, ModelRouteId};

#[test]
fn earlier_vector_corruption_precedes_later_snapshot_failure() {
    let query = KnowledgeEmbeddingQuery {
        book_revision_id: "book".into(),
        task_profile_revision_id: "task".into(),
        model_route_id: ModelRouteId::from("route"),
        dimensions: 1,
        vector_space_sha256: "a".repeat(64),
        values: vec![1.0],
    };
    let bytes = 1.0f32.to_le_bytes().to_vec();
    let digest = "0".repeat(64);
    let expected = score_encoded_vector(&query, 1.0, &bytes, &digest).unwrap_err();
    let candidates = vec![
        Ok(Candidate {
            embedding_id: "embedding".into(),
            entry_id: "entry".into(),
            vector_sha256: digest,
            vector: bytes,
        }),
        Err(CoreError::invalid("later budget failure")),
    ];
    let actual = score(candidates, &query, 1.0).unwrap_err();
    assert_eq!(actual.code, expected.code);
    assert_eq!(actual.message, expected.message);
    assert_eq!(actual.recoverable, expected.recoverable);
}
