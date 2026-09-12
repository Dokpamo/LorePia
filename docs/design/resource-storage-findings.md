# Storage resource optimization slice

Task: RESOURCE-OPTIMIZATION-20260912, parallel Storage slice.
Baseline HEAD and merge-base: aac3b16e0697c710b1209f498caba71821746140.
Worktree: codex/ux-responsiveness-20260912, existing concurrent dirty changes;
only this slice's files are owned here, with no checkout, reset, or commit.

Targets: memory_queue.rs query_memory_embeddings_cosine,
score_memory_embedding_candidates, decode_memory_embedding_vector, and private
memory_queue/embedding_scoring.rs plus its tests. Public entry point remains
Storage::query_memory_embeddings_cosine, exported through the existing facade.
Core caller: orchestration_runtime/semantic.rs provider semantic resolution.

Invariants: unchanged SQL and candidate snapshot, limits (2048 candidates,
16 MiB vector bytes), SHA256 and byte-length checks, finite checks and error
codes/messages, f64 conversion and accumulation order, fixed-point rounding,
score tie-breaks, public API/schema/dependencies and durable transactions.
Only the read connection guard ends after owned candidates have been loaded;
no transaction or later database read participates in scoring.

Move the private scoring and decoder implementation into a child; retain
query validation, query orchestration, norm and rounding helpers in the parent.
Expected parent size reduction: roughly 60 lines, no baseline increases.
Risks: changed floating arithmetic or corruption precedence; validate with an
independent reference implementation and malformed vectors before acceptance.

Governing material: root and Storage AGENTS.md, ADR 0001/0002/0003/0005,
and storage-public-api-audit.md. This is authorized resource optimization,
not a behavior-changing cleanup or schema/pruning project.

Baseline targeted test: embedding_and_terminal_job_commit_once_and_cosine_is_exactly_scoped.
Validation: existing scoped embedding test, encoded-scoring differential tests,
Storage targeted clippy where available; full workspace gates are parent-owned.

## Implemented result

The connection guard is explicitly released immediately after candidate loading.
Private scoring and decoding now live in memory_queue/embedding_scoring.rs.
Scoring verifies dimensions, byte length and SHA256 before walking f32le bytes
once, checking finite values and independently accumulating norm and dot in the
same original left-to-right f64 order. Existing decoder callers still receive
Vec<f32>; query scoring no longer allocates a candidate float vector. Candidate
blob loading, result allocation, scoring order and sort/truncate are unchanged.

Parent file measured delta: 5 inserted / 66 removed, net reduction 61 lines.
New private implementation: 89 lines; dedicated tests: 116 lines. No config,
public API, schema, dependency, fixture or other owner's file was modified.

Checks passed:
- Baseline scoped embedding completion/cosine test.
- cargo test -p lorepia-storage --lib memory_queue:: (14 passed).
- cargo clippy -p lorepia-storage --all-targets -- -D warnings.
- rustfmt --edition 2024 --check crates/storage/src/memory_queue.rs.

Differential coverage compares scores with the existing decoded-vector
calculation at 1, 3, 127, 1536 and 32768 dimensions, multiple deterministic
vectors, equal/opposite vectors, extreme finite values and zero norms. Corruption
checks preserve exact error codes/messages for byte length, SHA256, NaN and
both infinities. Existing scoped queue tests cover ownership, branch rewind,
immutable completion, replay and query intent behavior. Full workspace and
architecture gates remain parent-owned.

## Deferred findings

The current Core caller requests all bounded candidates as results; top-k
selection does not help that path. Portable-state eviction collects up to 1024
metadata rows even when fewer are needed, but early stopping can alter corrupt
row detection. Embedding retention is enforced by no-delete triggers, and
rollback generations are recovery evidence; no automatic pruning was added.
