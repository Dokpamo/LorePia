# Core Guide

## Scope

- This file applies to `crates/core/**` and supplements the repository guide.
- `lorepia-core` is the headless application/use-case facade between pure
  engines/adapters and `lorepia-shell-api`.
- Keep Tauri commands, renderer DTOs, native vault implementation, production
  SQL, and provider wire encoding outside this crate. Existing test-only SQL
  uses the `rusqlite` dev-dependency.
- Read ADRs 0001-0003, plus ADR 0004 for IPC-facing work and ADR 0005 for asset
  delivery, then `src/lib.rs`, the owning module, its Shell API callers, and
  related tests before changing a boundary.

## Public Entry Points

- Consumers import through `lorepia_core`; modules under `src/` are private
  implementation details.
- `src/lib.rs` owns the compatibility surface, `CORE_API_VERSION`, and
  `core_version()`. Existing legacy wildcard exports are not precedent; add new
  exports explicitly.
- Construct the facade with `CoreConfig::new(...)` and `Core::open(...)`.
- `Core::open_with_discovery_recovery_owner(...)` is for a host that keeps Core
  private until native credential reconciliation finishes.
- Public `Core` methods live across `app.rs` and feature modules. Inventory them
  with `rg -n '^impl Core|^\s*pub (async )?fn ' crates/core/src`.
- The production caller is `lorepia-shell-api`. `test-support` remains
  feature-gated and test-only.

## State and Transaction Owner

- `Core` is a cloneable facade over one shared `Arc<CoreInner>`.
- `CoreInner` owns process-local runtime state: pending reviews, discovery
  reservations, active generation/model-sync registries, event buses, and the
  owned Tokio runtime.
- Storage owns SQLite/CAS transactions, durable phases, cutover, and recovery.
  Core coordinates semantic use cases and must not rebuild atomic work from
  unrelated low-level Storage calls.
- The platform adapter owns credential storage/deletion. Core may carry a
  request-scoped credential lease but never persist, serialize, or log its value.
- Final `Core` drop cancels and bounds active generation shutdown, cancels model
  sync, and then shuts down the owned runtime. Preserve that lifecycle.

## Security and Determinism Invariants

- Preserve public signatures, error codes, serialized fields, API versions, and
  Shell/IPC behavior during refactors.
- Do not re-export a new `lorepia_storage::Stored*` row. Define a Core-owned
  view/projection and use a meaning-based repository operation.
- `ConnectionBoundCredential` remains non-`Clone`, non-`Serialize`,
  non-`Display`, redacted in `Debug`, and zeroized before its dispatch lease is
  released.
- Preserve exact credential binding/authority epoch checks. Locked vault,
  binding drift, and unknown durability fail closed.
- Seal the durable ordinary-chat generation attempt before provider dispatch.
  Preserve nonce/resume exclusivity, admission, cancellation, checkpoint,
  side-effect certainty, and terminal-persistence-before-publication behavior.
  Runtime/auxiliary generation and discovery retain their separate durable
  audit/WAL boundaries.
- Preserve route identity, strictly increasing event sequences,
  watermark/prefix atomicity, reattachment, and shutdown watchdog behavior.
- Discovery persists operation/audit/outbox state before network work. Raw
  credentials never enter drafts, evidence, events, or errors.
- Keep deterministic discovery evidence distinct from assistant-derived
  evidence; do not silently raise its trust level.
- Preserve stable sorting, canonical serialization, hash inputs, revision/CAS
  checks, and golden outputs.
- Renderer-facing assets remain digest-based and path-free. Storage retains
  verified bounded-read ownership.

## Module Map

- `lib.rs`: public facade, compatibility re-exports, and version constants.
- `config.rs`: data-root configuration and discovery recovery ownership.
- `app.rs`: `Core`/`CoreInner` lifecycle and the remaining compatibility/
  delegation facade; feature-grouped unit tests live under `app/tests/`.
- `app/generation/`, `generation_events.rs`, `generation_workflow.rs`:
  operation identity, admission, credential lifetime, target/prompt/send flow,
  ordered subscriptions, checkpointing, persistence, and events.
- `app/model_sync.rs`, `runtime_control.rs`, `runtime_generation.rs`,
  `portable_runtime_state.rs`: model jobs, owned runtime, auxiliary generation,
  and scoped portable state.
- `provider_discovery*.rs`, `provider_discovery/`: durable discovery,
  credential-free deterministic work, views, and compatibility fixtures.
- `app/providers/`, `provider_credential.rs`, `catalog.rs`: provider facade,
  native-vault coordination, reconciliation, and signed catalog lifecycle.
- `app/imports/`, `content_package.rs`, `content_package/`: staged content
  import, inspection, review, commit, and lifecycle coordination.
- `content_export.rs`: Rust-only, path-private native export handoff.
- `asset_delivery.rs`: path-free renderer descriptor and bounded range facade.
- `orchestration.rs`, `orchestration/`, `orchestration_runtime.rs`,
  `orchestration_runtime/`, `module_orchestration.rs`,
  `transform_documents.rs`: orchestration use cases and durable runtime work.
- `interaction_*`, `message_display_projection.rs`: interaction outbox,
  Core-owned views, and display projections.
- `persona.rs`, `revision.rs`: persona use cases and Core-owned revisions.
- `tests/*.rs`: vertical, restart, security, and determinism contracts.

## Common Change Recipes

- Public API: inspect `src/lib.rs`, the owning `impl Core`, Shell API callers,
  and external Core tests; keep existing facade paths.
- Extraction: run a baseline, move symbols unchanged into a context-named child
  module, keep facade delegation, use minimum visibility, then lower the
  measured source-size baseline.
- Ordinary chat generation: use the owning context-map entry to select the
  focused `app/generation/` modules, then read Storage attempt APIs and chat
  vertical tests together.
- Storage-facing change: use an existing semantic repository operation. If a
  new Storage operation is required, stop unless the task authorizes
  `crates/storage/**`, then read its guide. Never pass a
  `rusqlite::Connection`, leak a Storage/CAS path across the public boundary,
  or split one transaction across Core. Preserve intentional `CoreConfig`,
  staged-import, and Rust-only export path APIs.
- Orchestration: pure rules belong in `lorepia-orchestration`; Core owns state
  lookup, authority/revision checks, and durable coordination.
- A new dependency, schema change, golden/hash drift, unclear secret lifetime,
  or required out-of-task edit is a stop condition.

## Targeted Tests

```bash
cargo test -p lorepia-core
cargo clippy -p lorepia-core --all-targets -- -D warnings
cargo fmt --all --check
python3 scripts/check_source_architecture.py --base-ref "$(git merge-base HEAD main)"
```

- Lifecycle/open: `cargo test -p lorepia-core --test core_smoke`
- App child tests: `cargo test -p lorepia-core --lib app::tests`
- Chat/generation: `chat_vertical_slice`,
  `ordinary_same_branch_sealed_retry`, `generation_attempt_derived_closure`
- Import/export: `import_vertical_slice`, `orchestration_content_import`,
  `content_source_export_round_trip`
- Provider/discovery: `provider_discovery_integration`,
  `built_in_connection_defaults`, `model_refresh_concurrency`,
  `semantic_provider_vertical`
- Orchestration/interaction: `orchestration_core_contract`,
  `prompt_preset_rollback`, `memory_auxiliary_fallback`,
  `interaction_derived_supervisor`, `ordinary_interaction_sealed_replay`
- Persona/recovery: `persona_contract`, `schema11_release_cutover`
- The default `schema11_release_cutover` run includes an ignored rollback
  harness; do not present the default suite as proof that ignored harness ran.
- One-binary example:
  `cargo test -p lorepia-core --test chat_vertical_slice`. Substitute a listed
  test file stem as needed. Also run the affected Storage, Providers,
  Orchestration, Chat, or Content crate suite.

## Forbidden Dependencies and Changes

- No dependency on Shell API, Tauri, frontend code, or native platform plugins.
- Do not move `rusqlite` from test-only use into production Core code.
- No production SQL, DB connection, migration, CAS transaction/recovery, OS
  vault, or provider HTTP/wire implementation.
- In extraction work, add no dependency, wildcard re-export, unnecessary
  visibility, public persistence row, or new size-baseline exception.
- Do not change schema, IPC, DTO/API version, error code, deterministic hash,
  admission limit, lock scope, or secret lifetime unless the task explicitly
  authorizes that semantic change.
