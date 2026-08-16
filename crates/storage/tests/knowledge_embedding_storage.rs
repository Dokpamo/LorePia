//! Public Storage API contracts for exact knowledge-vector spaces.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use lorepia_domain::{
    ActivationRule, AuxiliaryTaskKind, CoreErrorCode, GenerationPresetId, KnowledgeBook,
    KnowledgeBookId, KnowledgeEntry, KnowledgeEntryId, KnowledgePlacement, ModelRouteId,
    Provenance, RateLimit, SourceKind, TaskProfile, TaskProfileId, TokenBudget, TokenPolicy,
};
use lorepia_storage::{
    KnowledgeEmbeddingCoverageQuery, KnowledgeEmbeddingQuery, KnowledgeEmbeddingWrite, Storage,
};
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use tempfile::{TempDir, tempdir};

const NOW: &str = "2026-08-09T00:00:00Z";
const MODEL_ROUTE_ID: &str = "route:knowledge-storage";
const GENERATION_PRESET_ID: &str = "preset:knowledge-storage";
const ENTRY_ID: &str = "entry:knowledge-storage";
const VECTOR_SPACE_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const VECTOR_SPACE_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn active_database_path(root: &Path) -> PathBuf {
    let cutover = root.join("db/schema-cutover");
    let (_, relative) = std::fs::read_dir(cutover)
        .expect("read committed database generations")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join("generation-committed.json").is_file())
        .map(|entry| {
            let manifest = serde_json::from_slice::<serde_json::Value>(
                &std::fs::read(entry.path().join("generation-manifest.json"))
                    .expect("read generation manifest"),
            )
            .expect("parse generation manifest");
            let sequence = manifest["activation_sequence"]
                .as_u64()
                .expect("generation activation sequence");
            let relative = manifest["active_database_relative_path"]
                .as_str()
                .expect("active database relative path")
                .to_owned();
            (sequence, relative)
        })
        .max_by_key(|(sequence, _)| *sequence)
        .expect("at least one committed database generation");
    root.join(relative)
}

struct Fixture {
    root: TempDir,
    storage: Storage,
    task_profile_revision_id: String,
    summary_task_profile_revision_id: String,
    book_revision_id: String,
}

#[test]
fn exact_knowledge_embedding_storage_contract_fails_closed_across_spaces_and_tasks() {
    let fixture = fixture();
    let created_at = timestamp();
    let write = KnowledgeEmbeddingWrite {
        id: "embedding:knowledge-storage".to_owned(),
        book_revision_id: fixture.book_revision_id.clone(),
        entry_id: KnowledgeEntryId::from(ENTRY_ID),
        task_profile_revision_id: fixture.task_profile_revision_id.clone(),
        model_route_id: ModelRouteId::from(MODEL_ROUTE_ID),
        dimensions: 3,
        vector_space_sha256: VECTOR_SPACE_A.to_owned(),
        values: vec![1.0, 0.0, 0.0],
        created_at,
    };

    fixture
        .storage
        .save_knowledge_embedding(&write)
        .expect("save exact knowledge embedding");
    fixture
        .storage
        .save_knowledge_embedding(&write)
        .expect("exact replay is idempotent");

    assert_exact_embedding_and_coverage(&fixture, &write);

    let mut conflicting = write.clone();
    conflicting.id = "embedding:knowledge-storage-conflict".to_owned();
    conflicting.values = vec![0.0, 1.0, 0.0];
    let conflict = fixture
        .storage
        .save_knowledge_embedding(&conflicting)
        .expect_err("a different vector in the same exact space must fail closed");
    assert_eq!(conflict.code, CoreErrorCode::InvalidInput);

    let other_space = fixture
        .storage
        .query_knowledge_embeddings_cosine(&query(
            &fixture,
            &fixture.task_profile_revision_id,
            3,
            VECTOR_SPACE_B,
            vec![1.0, 0.0, 0.0],
        ))
        .expect("query a different exact vector space");
    assert!(
        other_space.is_empty(),
        "vectors from one exact provider space must not match another"
    );

    let wrong_dimensions = fixture
        .storage
        .query_knowledge_embeddings_cosine(&query(
            &fixture,
            &fixture.task_profile_revision_id,
            2,
            VECTOR_SPACE_A,
            vec![1.0, 0.0],
        ))
        .expect_err("dimensions that differ from the exact task must be rejected");
    assert_eq!(wrong_dimensions.code, CoreErrorCode::InvalidInput);

    let wrong_task = fixture
        .storage
        .query_knowledge_embeddings_cosine(&query(
            &fixture,
            &fixture.summary_task_profile_revision_id,
            3,
            VECTOR_SPACE_A,
            vec![1.0, 0.0, 0.0],
        ))
        .expect_err("a non-embedding task revision must be rejected");
    assert_eq!(wrong_task.code, CoreErrorCode::NotFound);
}

#[test]
fn bounded_embedding_query_rejects_insufficient_remaining_work() {
    let fixture = fixture();
    let write = required_embedding_write(&fixture);
    fixture
        .storage
        .save_knowledge_embedding(&write)
        .expect("save required embedding");

    let error = fixture
        .storage
        .query_required_knowledge_embeddings_cosine_bounded(
            &query(
                &fixture,
                &fixture.task_profile_revision_id,
                3,
                VECTOR_SPACE_A,
                vec![1.0, 0.0, 0.0],
            ),
            std::slice::from_ref(&write.entry_id),
            1,
        )
        .expect_err("one remaining work byte must fail closed before vector work");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
}

#[test]
fn required_entry_queries_ignore_large_unrelated_exact_space() {
    const ENTRY_COUNT: usize = 1_025;

    let fixture = fixture_with_entry_count(ENTRY_COUNT);
    let write = required_embedding_write(&fixture);
    fixture
        .storage
        .save_knowledge_embedding(&write)
        .expect("save required embedding");
    let required = [write.entry_id.clone()];
    let score_query = query(
        &fixture,
        &fixture.task_profile_revision_id,
        3,
        VECTOR_SPACE_A,
        vec![1.0, 0.0, 0.0],
    );
    let coverage = coverage_query(&fixture, VECTOR_SPACE_A, required.to_vec());
    let baseline_scores = fixture
        .storage
        .query_required_knowledge_embeddings_cosine_bounded(&score_query, &required, 8 * 1_024)
        .expect("score the required entry before unrelated rows");
    let baseline_coverage = fixture
        .storage
        .knowledge_embedding_space_covers_entries_bounded(&coverage, 8 * 1_024)
        .expect("cover the required entry before unrelated rows");

    seed_unrelated_embeddings(&fixture, ENTRY_COUNT);

    let scores = fixture
        .storage
        .query_required_knowledge_embeddings_cosine_bounded(&score_query, &required, 8 * 1_024)
        .expect("unrelated vectors must not consume required-entry query work");
    let covered = fixture
        .storage
        .knowledge_embedding_space_covers_entries_bounded(&coverage, 8 * 1_024)
        .expect("unrelated vectors must not consume coverage work");
    assert_eq!(scores, baseline_scores);
    assert_eq!(covered, baseline_coverage);
    assert!(scores.work_bytes > 0, "provider scoring must be charged");
    assert!(covered.work_bytes > 0, "coverage preflight must be charged");
    assert!(covered.covered);
    assert_eq!(scores.matches.len(), 1);
    assert_eq!(scores.matches[0].entry_id, write.entry_id);
}

fn assert_exact_embedding_and_coverage(fixture: &Fixture, write: &KnowledgeEmbeddingWrite) {
    let exact = fixture
        .storage
        .query_knowledge_embeddings_cosine(&query(
            fixture,
            &fixture.task_profile_revision_id,
            3,
            VECTOR_SPACE_A,
            vec![1.0, 0.0, 0.0],
        ))
        .expect("query exact knowledge vector space");
    assert_eq!(exact.len(), 1, "exact replay must not duplicate the row");
    assert_eq!(exact[0].embedding_id, write.id);
    assert_eq!(exact[0].entry_id, write.entry_id);
    assert_eq!(exact[0].similarity_millionths, 1_000_000);
    assert_eq!(
        exact[0].vector_sha256,
        format!("{:x}", Sha256::digest(encode_vector(&write.values)))
    );
    assert!(
        fixture
            .storage
            .knowledge_embedding_space_covers_entries(&coverage_query(
                fixture,
                VECTOR_SPACE_A,
                vec![KnowledgeEntryId::from(ENTRY_ID)],
            ))
            .expect("check complete exact coverage")
    );
    assert!(
        !fixture
            .storage
            .knowledge_embedding_space_covers_entries(&coverage_query(
                fixture,
                VECTOR_SPACE_A,
                vec![
                    KnowledgeEntryId::from(ENTRY_ID),
                    KnowledgeEntryId::from("entry:knowledge-storage-missing"),
                ],
            ))
            .expect("check incomplete exact coverage")
    );
}

fn fixture() -> Fixture {
    fixture_with_entry_count(1)
}

#[allow(clippy::too_many_lines)]
fn fixture_with_entry_count(entry_count: usize) -> Fixture {
    assert!(entry_count > 0);
    let root = tempdir().expect("temporary storage root");
    let storage = Storage::open(root.path()).expect("open storage");
    seed_provider_graph(root.path());

    let embedding_task = TaskProfile {
        id: TaskProfileId::from("task:knowledge-storage-embedding"),
        kind: AuxiliaryTaskKind::MemoryEmbedding,
        route_id: ModelRouteId::from(MODEL_ROUTE_ID),
        generation_preset_id: GenerationPresetId::from(GENERATION_PRESET_ID),
        fallback_route_ids: Vec::new(),
        embedding_dimensions: Some(3),
        timeout_ms: 30_000,
        rate_limit: RateLimit {
            requests: 2,
            per_seconds: 60,
        },
        concurrency_limit: 1,
    };
    let task_profile_revision_id = storage
        .save_task_profile(&embedding_task, None)
        .expect("embedding task profile")
        .revision_id
        .expect("embedding task revision id");

    let summary_task = TaskProfile {
        id: TaskProfileId::from("task:knowledge-storage-summary"),
        kind: AuxiliaryTaskKind::MemorySummary,
        route_id: ModelRouteId::from(MODEL_ROUTE_ID),
        generation_preset_id: GenerationPresetId::from(GENERATION_PRESET_ID),
        fallback_route_ids: Vec::new(),
        embedding_dimensions: None,
        timeout_ms: 30_000,
        rate_limit: RateLimit {
            requests: 2,
            per_seconds: 60,
        },
        concurrency_limit: 1,
    };
    let summary_task_profile_revision_id = storage
        .save_task_profile(&summary_task, None)
        .expect("summary task profile")
        .revision_id
        .expect("summary task revision id");

    let book_id = KnowledgeBookId::from("book:knowledge-storage");
    let provenance = fixture_provenance();
    let entries = (0..entry_count)
        .map(|index| KnowledgeEntry {
            id: if index == 0 {
                KnowledgeEntryId::from(ENTRY_ID)
            } else {
                KnowledgeEntryId::from(format!("entry:knowledge-storage-unrelated-{index:04}"))
            },
            book_id: book_id.clone(),
            name: format!("Exact vector entry {index}"),
            content: format!("Project-owned synthetic exact-vector content {index}."),
            enabled: true,
            activation: ActivationRule::Always,
            priority: 1,
            importance: 50,
            placement: KnowledgePlacement::RetrievedContext,
            token_policy: TokenPolicy {
                priority: 100,
                min_tokens: None,
                max_tokens: Some(64),
                reserve_tokens: None,
            },
            parent_id: None,
            activation_probability_basis_points: 10_000,
            provenance: provenance.clone(),
        })
        .collect();
    let book = KnowledgeBook {
        id: book_id.clone(),
        name: "Knowledge embedding storage fixture".to_owned(),
        schema_version: 1,
        entries,
        scan_depth: 8,
        token_budget: TokenBudget { max_tokens: 256 },
        recursive: false,
        max_recursion_depth: 0,
        provenance,
    };
    let book_revision_id = storage
        .save_knowledge_book(&book, None)
        .expect("knowledge book")
        .revision_id
        .expect("knowledge book revision id");

    Fixture {
        root,
        storage,
        task_profile_revision_id,
        summary_task_profile_revision_id,
        book_revision_id,
    }
}

fn required_embedding_write(fixture: &Fixture) -> KnowledgeEmbeddingWrite {
    KnowledgeEmbeddingWrite {
        id: "embedding:knowledge-storage-required".to_owned(),
        book_revision_id: fixture.book_revision_id.clone(),
        entry_id: KnowledgeEntryId::from(ENTRY_ID),
        task_profile_revision_id: fixture.task_profile_revision_id.clone(),
        model_route_id: ModelRouteId::from(MODEL_ROUTE_ID),
        dimensions: 3,
        vector_space_sha256: VECTOR_SPACE_A.to_owned(),
        values: vec![1.0, 0.0, 0.0],
        created_at: timestamp(),
    }
}

fn seed_unrelated_embeddings(fixture: &Fixture, entry_count: usize) {
    let mut connection = Connection::open(active_database_path(fixture.root.path()))
        .expect("open unrelated embedding fixture database");
    connection
        .pragma_update(None, "foreign_keys", true)
        .expect("enable foreign keys");
    let transaction = connection.transaction().expect("begin embedding fixture");
    let bytes = encode_vector(&[0.0, 1.0, 0.0]);
    let vector_sha256 = format!("{:x}", Sha256::digest(&bytes));
    {
        let mut statement = transaction
            .prepare(
                "INSERT INTO knowledge_embeddings
                 (id, book_revision_id, entry_id, task_profile_revision_id,
                  model_route_id, dimensions, vector_space_sha256, encoding,
                  vector_blob, vector_sha256, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 3, ?6, 'f32le', ?7, ?8, ?9)",
            )
            .expect("prepare unrelated embedding insert");
        for index in 1..entry_count {
            statement
                .execute(params![
                    format!("embedding:knowledge-storage-unrelated-{index:04}"),
                    fixture.book_revision_id,
                    format!("entry:knowledge-storage-unrelated-{index:04}"),
                    fixture.task_profile_revision_id,
                    MODEL_ROUTE_ID,
                    VECTOR_SPACE_A,
                    bytes,
                    vector_sha256,
                    NOW,
                ])
                .expect("insert unrelated exact-space embedding");
        }
    }
    transaction
        .commit()
        .expect("commit unrelated embedding fixture");
}

fn seed_provider_graph(root: &Path) {
    let database_path = active_database_path(root);
    let connection = Connection::open(database_path).expect("open provider fixture database");
    connection
        .pragma_update(None, "foreign_keys", true)
        .expect("enable foreign keys");
    let manifest_json = "{}";
    let manifest_sha256 = format!("{:x}", Sha256::digest(manifest_json.as_bytes()));
    connection
        .execute(
            "INSERT INTO provider_templates
             (id, version, display_name, source_kind, manifest_json,
              manifest_sha256, created_at)
             VALUES ('template:knowledge-storage', 1, 'Knowledge storage fixture',
                     'built_in', ?1, ?2, ?3)",
            params![manifest_json, manifest_sha256, NOW],
        )
        .expect("provider template");
    connection
        .execute(
            "INSERT INTO provider_connections
             (id, template_id, template_version, display_name, api_origin,
              config_json, credential_ref, credential_scope_json,
              timeout_seconds, status, created_at, updated_at)
             VALUES ('connection:knowledge-storage', 'template:knowledge-storage', 1,
                     'Knowledge storage fixture', 'https://example.invalid', '{}',
                     NULL, NULL, 30, 'connected', ?1, ?1)",
            [NOW],
        )
        .expect("provider connection");
    connection
        .execute(
            "INSERT INTO provider_models
             (id, connection_id, api_family, model_id, display_name,
              route_json, availability, raw_metadata_json,
              first_seen_at, last_seen_at)
             VALUES (?1, 'connection:knowledge-storage', 'openai_chat_completions',
                     'knowledge-storage-model', 'Knowledge storage model', '{}',
                     'available', NULL, ?2, ?2)",
            params![MODEL_ROUTE_ID, NOW],
        )
        .expect("provider model");
    connection
        .execute(
            "INSERT INTO generation_presets
             (id, model_route_id, display_name, values_json, created_at, updated_at)
             VALUES (?1, ?2, 'Knowledge storage preset', '{}', ?3, ?3)",
            params![GENERATION_PRESET_ID, MODEL_ROUTE_ID, NOW],
        )
        .expect("generation preset");
}

fn query(
    fixture: &Fixture,
    task_profile_revision_id: &str,
    dimensions: u32,
    vector_space_sha256: &str,
    values: Vec<f32>,
) -> KnowledgeEmbeddingQuery {
    KnowledgeEmbeddingQuery {
        book_revision_id: fixture.book_revision_id.clone(),
        task_profile_revision_id: task_profile_revision_id.to_owned(),
        model_route_id: ModelRouteId::from(MODEL_ROUTE_ID),
        dimensions,
        vector_space_sha256: vector_space_sha256.to_owned(),
        values,
    }
}

fn coverage_query(
    fixture: &Fixture,
    vector_space_sha256: &str,
    required_entry_ids: Vec<KnowledgeEntryId>,
) -> KnowledgeEmbeddingCoverageQuery {
    KnowledgeEmbeddingCoverageQuery {
        book_revision_id: fixture.book_revision_id.clone(),
        task_profile_revision_id: fixture.task_profile_revision_id.clone(),
        model_route_id: ModelRouteId::from(MODEL_ROUTE_ID),
        dimensions: 3,
        vector_space_sha256: vector_space_sha256.to_owned(),
        required_entry_ids,
    }
}

fn encode_vector(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn timestamp() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(NOW)
        .expect("fixture timestamp")
        .with_timezone(&Utc)
}

fn fixture_provenance() -> Provenance {
    Provenance {
        source_kind: SourceKind::UserCreated,
        source_id: Some("synthetic.knowledge-embedding-storage".to_owned()),
        source_hash: None,
        author: None,
        license: None,
        imported_at: None,
    }
}
