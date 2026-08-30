# Storage Guide

## Scope

- This file applies to `crates/storage/**` and supplements the repository guide.
- `lorepia-storage` is the SQLite and content-addressed-storage adapter used by
  Core. It owns the database root, persistence, migrations/cutover, durable
  workflow state, CAS publication, and startup recovery.
- It does not own provider HTTP, UI/IPC contracts, or native credential values.
- Read ADRs 0001-0003 and 0005,
  `docs/architecture/storage-public-api-audit.md`, `src/lib.rs`, the target
  module, its Core callers, and its tests before editing.

## Public Entry Points

- `src/lib.rs` is the explicit crate facade.
- `Storage` is defined in `database.rs`; repository modules add inherent
  `Storage` methods.
- `Storage::open()` performs its owned startup recovery before return.
  `open_with_deferred_discovery_recovery()` delegates provider-discovery
  reconciliation and must remain unexposed until the selected Core/native owner
  completes it.
- Production dependency direction is `storage -> domain/orchestration`, then
  `core -> storage`.
- The current API still contains repository results, use-case inputs,
  persistence rows, transaction commands, and hash helpers for compatibility;
  its reduction is unfinished. Do not hide them before migrating and verifying
  callers.
- Keep existing signatures and explicit re-exports during extraction. Prefer a
  Core-owned view and meaning-based repository method over a new public row.

## State and Transaction Owner

`Storage` owns:

- the canonical data root and exclusive owner lock;
- the shared SQLite connection and connection metrics;
- the CAS mutation lock and verified-asset handle cache;
- migrations, immutable database generations, cutover, and startup recovery;
- durable intents, phases, outboxes, leases, retries, quarantines, and recovery
  rows.

Core coordinates use cases but must not reconstruct an atomic use case from
unrelated low-level methods. Native adapters own OS keychain effects; Storage
owns only their secret-free plan, authority, evidence, and status.

The lock order is `cas_mutation -> connection`. Never acquire the CAS mutation
lock while holding the connection lock.

## Security and Determinism Invariants

- Keep all rows for one atomic transition in one SQLite transaction. Extracted
  helpers receive the same `&Transaction`; they do not reacquire a connection.
- Preserve `TransactionBehavior`, compare-and-swap predicates, lock order and
  lifetime, filesystem sync order, and journal phase order.
- Record durable intent/phase before filesystem or native effects. Preserve CAS
  no-clobber publication, hash/size validation, and file/directory fsync.
- Startup recovery remains idempotent and completes or quarantines incomplete
  work before Storage becomes observable. When
  `open_with_deferred_discovery_recovery` delegates discovery ownership, the
  owning Core/native host must finish reconciliation before application
  exposure.
- Incomplete, unresolved, corrupt, or unknown-outcome state never becomes
  successful data. Preserve fail-closed/review-required classification.
- Credential repositories accept no credential bytes or credential-derived
  digest. Persist only references, binding hashes, authority, redacted evidence,
  and operation status.
- Do not add new public Core/Shell/renderer exposure of connections, persistence
  rows, raw handles, absolute/staging paths, or credentials. Existing internal
  Core adapter calls are migrated deliberately, not removed opportunistically.
- Preserve owner-lock, private-path, traversal, symlink/reparse-point,
  regular-file, link-count, MIME/signature, size, range, and identity checks.
- Preserve canonical JSON, hashes, schema/status strings, serde names, operation
  order, and error classification.
- Results used by hashing, ranking, replay, or visible ordering require explicit
  SQL ordering or stable canonical sorting.
- Do not update durable fixtures or golden hashes to conceal semantic drift.

## Module Map

- `database.rs`, `database/`: `Storage`, open/bootstrap, connection/root
  ownership, migrations, base repositories, CAS promotion, asset delivery.
- `cutover.rs`: immutable DB generations, fingerprints, publication, pins, and
  crash recovery.
- `catalog.rs`, `model_sync.rs`: provider catalog history and model-sync jobs.
- `discovery.rs`: low-level discovery state-machine primitives.
- `discovery_repository.rs`, `discovery_repository/`: typed discovery aggregate,
  validation/codec, transitions, approval, commit, credentials, and recovery.
- `generation_attempt.rs`: durable generation identity, approval, dispatch seal,
  retry, and credential authority.
- `interaction_repository.rs`, `interaction_repository/`: interaction state,
  proposals, effects, checkpoints, derived outbox, quarantine, and recovery.
- `orchestration.rs`, `orchestration/`: revisioned prompt, memory, knowledge,
  transform, interaction, content-module, and runtime-plan persistence.
- `package_repository.rs`, `package_repository/`: persisted package inspection
  results/expectations, review, approval, transactional commit, and completed
  CAS authority; Content/Core own source inspection.
- `provider_credential_repository.rs`: secret-free native credential journal and
  recovery barriers.
- `memory_*`, `knowledge_embedding.rs`, `lifecycle_outbox.rs`,
  `runtime_model_audit.rs`: durable worker/query/lifecycle/audit state.
- `persona_repository.rs`, `portable_runtime_state.rs`,
  `message_display_projection.rs`: bounded feature repositories.
- `content_export.rs`, `verified_asset_cache.rs`: trusted export and short-lived
  verified file-handle leases.
- `migrations/`: append-only schema evolution; `0001`-`0011` are frozen.

## Common Change Recipes

### Repository Extraction

1. Inventory methods, exported types, Core callers, transaction helpers, SQL,
   and tests; run the targeted baseline.
2. Move code without renaming or modifying SQL.
3. Keep the facade and pass the existing transaction through helpers.
4. Use private or `pub(super)` before `pub(crate)`.
5. Run targeted tests, architecture ratchets, then full Rust checks.
6. Lower the parent baseline by the measured reduction; never raise it.

### Schema Change

1. Never rewrite an applied migration; `0001`-`0011` are additionally frozen
   compatibility fixtures.
2. Add the next numbered migration and increment `SCHEMA_VERSION`.
3. Update the registry and latest-schema/cutpoint expectations. Preserve frozen
   schema-11 constants and artifacts.
4. Verify fresh install, each supported cutpoint, reopen idempotence,
   rollback-on-failure, and cutover recovery.

### Durable External Effect

1. Persist immutable intent and authority first.
2. Release DB locks before slow work where the existing flow does.
3. Record the next phase only after its durability barrier.
4. Classify lost responses conservatively and verify recovery after reopen.

## Targeted Tests

```bash
cargo test -p lorepia-storage
cargo clippy -p lorepia-storage --all-targets -- -D warnings
cargo fmt --all --check
```

- Schema/cutover: `schema_registry_integrity`, `migration_cutpoint_matrix`,
  `schema11_release_cutover`, `schema11_fixture_provenance`
- Discovery: `discovery_state_storage`, `discovery_wal_purge`, and
  `cargo test -p lorepia-storage --lib discovery_repository::tests`
- Credentials: `provider_credential_operations`
- Interaction: `interaction_derived_event_storage` and
  `cargo test -p lorepia-storage --lib interaction_repository::tests`
- Orchestration: `orchestration_migrations` and
  `cargo test -p lorepia-storage --lib orchestration::tests`
- Package/assets: `cargo test -p lorepia-storage --lib database::tests`,
  `cargo test -p lorepia-storage --lib package_repository::tests`,
  `cargo test -p lorepia-storage --lib verified_asset_cache::tests`, and
  `cargo test -p lorepia-storage --test private_storage_permissions`
- One-binary example:
  `cargo test -p lorepia-storage --test schema_registry_integrity`. Substitute
  a listed test file stem as needed. If Core consumes the changed contract, run
  `cargo test -p lorepia-core` and the architecture checker.

## Forbidden Dependencies and Changes

- No dependency on Core, Shell API, Providers, the platform plugin, Tauri, or UI
  crates.
- No ORM, network transport, native vault access, or dependency upgrade in a
  Storage refactor.
- Do not disguise public API, durable schema/wire, error-code, hash, ordering,
  recovery-policy, or transaction-boundary changes as extraction.
- Do not add wildcard exports, new public persistence rows, raw-secret
  persistence/logging, renderer-visible paths/rows/handles, baseline increases,
  or new giant-source exceptions.
