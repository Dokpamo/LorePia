# LorePia Repository Guide

## Scope

- This file applies to the entire repository. A nearer `AGENTS.md` may add
  local constraints, but it must not weaken the boundaries below.
- Use the Rust and Node versions pinned by `rust-toolchain.toml` and
  `.node-version`.
- For refactoring work, perform one Task ID at a time. Before editing, record:
  Task ID; baseline commit/merge-base; worktree state; target files and public
  entry points; owned invariants; relevant tests; symbols to move and remain;
  expected size delta; semantic risks; and governing task/ADR material.
- Inventory symbols and tests with `rg` before opening a giant source file.
  Treat `config/source-size-baseline.json` as the current cap, and remeasure
  current sizes with the architecture checker.
- If a task requires a public contract, schema, security boundary, dependency,
  or unrelated-path change that it did not authorize, stop and report it.

## Public Entry Points

- Each Rust crate exposes its supported API through `crates/*/src/lib.rs`.
- `crates/core/src/lib.rs` is the headless application facade.
- `crates/shell-api/src/lib.rs` is the typed shell/DTO boundary.
- `apps/lorepia/src-tauri/src/main.rs` calls
  `lorepia_tauri_lib::run()` to start the native shell.
- `apps/lorepia/src/main.ts` is the live renderer entry; the isolated demo uses
  `apps/lorepia/src/preview/main.ts`.
- `apps/lorepia/src/lib/ipc/client.ts` is the renderer IPC facade.
- `config/ipc-commands.json` is the source of truth for mechanical command
  names. Do not hand-edit generated registries or permissions.

## State and Transaction Owner

- Core owns use-case coordination and its headless runtime lifecycle. Tauri
  `AppState` owns native process-local lifecycle and supervisors.
- Storage owns SQLite connections and transactions, CAS publication/fsync,
  durable intent and phase, migrations, cutover, and default startup recovery.
  A delegated discovery recovery remains hidden until its Core/native owner
  reconciles it.
- Providers own URL policy, network transport, and provider wire protocols.
- The platform plugin owns OS credential material and its native
  staging/platform operations. Storage owns data-root/CAS permission
  enforcement and durable publication.
- Tauri state composes Shell API and native services; it must not reconstruct a
  Storage transaction from multiple commands.
- Frontend controllers own shared async/race authority and backend-listener
  lifetimes. Components may own view-local subscriptions, teardown,
  interaction, and presentation lifecycle.

## Security and Determinism Invariants

- Domain is the innermost workspace layer. Orchestration depends on Domain;
  Core coordinates Domain/Orchestration with Content, Storage, Providers, and
  Chat adapters; Shell API, the native shell, and UI consume outward-facing
  Core contracts.
- `domain` has no workspace dependency. `orchestration` depends only on
  `domain` inside the workspace and performs no I/O, network, SQLite, Tauri, or
  wall-clock access; callers inject time and seeds.
- Never expose DB rows, absolute host paths, raw handles, or raw credentials to
  renderer DTOs.
- Credential values remain in the native vault and short dispatch leases.
  Secret-bearing carriers preserve redacted `Debug`, non-`Clone`/
  non-`Serialize`, zeroization, binding checks, and drop order; secret-free
  plans and views may remain ordinary value types.
- Ordinary chat provider dispatch starts only after a durable generation
  attempt is recorded. Preserve nonce/resume exclusivity, admission limits,
  cancellation, checkpointing, stream order, and unknown-side-effect
  classification. Auxiliary runtime and discovery flows keep their own durable
  audit/WAL boundaries.
- Preserve stable sorting, tie-breaks, canonical serialization, hash inputs,
  and golden outputs. Unordered iteration must not affect durable or visible
  results.
- Preserve Storage transaction scope, lock order, fsync order, journal phases,
  and fail-closed recovery. Incomplete state is not successful data.
- Assets cross the renderer boundary only by approved descriptor and canonical
  `lorepia-asset://sha256/<lowercase-digest>` identity. Keep no-follow open,
  same-handle verification/read, identity checks, and bounds.
- Keep `config/ipc-commands.json`, generated registries, `generate_handler!`,
  both capabilities, and generated permissions in exact agreement.
- Migrations `0001` through `0011` are frozen. Add a new migration for an
  explicitly approved schema change.
- Do not add a new public Core re-export of a Storage `Stored*` type.

## Module Map

| Path | Owner |
| --- | --- |
| `crates/domain` | Platform-independent types and errors |
| `crates/orchestration` | Pure deterministic prompt, memory, knowledge, and module planning |
| `crates/content` | Untrusted card and CHARX inspection/normalization |
| `crates/storage` | SQLite, CAS, migrations, durable recovery |
| `crates/providers` | Network policy and provider adapters |
| `crates/chat` | Generation streams and event projection |
| `crates/core` | Application use cases and public facade |
| `crates/shell-api` | Renderer-safe DTO and validation boundary |
| `plugins/lorepia-platform` | Native credential, staging, and platform operations |
| `apps/lorepia/src-tauri` | Tauri composition, commands, and asset protocol |
| `apps/lorepia/src` | Svelte UI, controllers, and IPC client |
| `config`, `scripts` | Contract generation and architecture/security ratchets |
| `docs/adr` | Approved architecture decisions |

## Common Change Recipes

### Refactor or Extract

1. Run the existing targeted tests and record the symbol/caller map.
2. Move code without renaming, behavior changes, or unrelated cleanup.
3. Preserve the old public entry through delegation or explicit re-export.
4. Use the narrowest visibility: private, then `pub(super)`, then `pub(crate)`.
5. Run targeted checks, lint/format, then the full gate.
6. Lower an existing size baseline only by the measured parent reduction.
   Never raise it or add a giant-file exception for an extraction.

### Add or Change an IPC Command

1. Define the Shell API DTO and method.
2. Add a strict Tauri command handler.
3. Update `config/ipc-commands.json` and run the generator.
4. Add the handler to `generate_handler!`, grant it in both development and
   release capabilities, then review and Git-track the generated permission.
5. Update the frontend contract/client and registry tests.

### Change the Schema

1. Add the next numbered migration; never rewrite an applied migration.
   Migrations `0001`-`0011` are additionally frozen compatibility fixtures.
2. Update `SCHEMA_VERSION`, the registry, and latest-schema/cutpoint
   expectations. Preserve frozen schema-11 constants and artifacts.
3. Verify fresh install, upgrade, reopen idempotence, and failure recovery.

## Targeted Tests

- Crate example: `cargo test -p lorepia-core`; substitute only a package name
  confirmed in its `Cargo.toml`.
- Core: `cargo test -p lorepia-core`
- Storage: `cargo test -p lorepia-storage` plus the affected Core vertical slice.
- Orchestration golden: `cargo test -p lorepia-orchestration --test cross_platform_golden`
- Shell API: `cargo test -p lorepia-shell-api`
- Tauri shell: `cargo test -p lorepia-tauri --lib`
- Frontend: `npm run check --prefix apps/lorepia` and, for example,
  `npm run test --prefix apps/lorepia -- src/features/chat/ChatPane.test.ts`.
- IPC/architecture:
  `python3 scripts/generate_ipc_commands.py --check` and
  `python3 scripts/check_source_architecture.py --base-ref "$(git merge-base HEAD main)"`.

Before merge, run:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm run check --prefix apps/lorepia
npm run test --prefix apps/lorepia
python3 -m unittest scripts/test_generate_ipc_commands.py scripts/test_check_source_architecture.py
python3 scripts/generate_ipc_commands.py --check
python3 scripts/check_source_architecture.py --base-ref "$(git merge-base HEAD main)"
```

Local checks do not replace the cross-platform Rust, Frontend, Android
dependency integrity, iOS simulator, Dependency Review, RustSec, Cargo deny,
and CodeQL GitHub checks.

## Forbidden Dependencies and Changes

- Do not add a workspace dependency to `domain` or any I/O boundary dependency
  to `orchestration`.
- In a refactoring-only task, do not add or upgrade dependencies or change
  public API, schema, serialized DTOs, error codes, IPC names, or behavior.
- Do not add wildcard re-exports, unnecessary `pub`, baseline increases, or new
  giant-source exceptions.
- Do not weaken credential, URL, archive, path, asset, transaction, recovery,
  capability, portability, or deterministic-ordering checks.
- Do not mix code movement with renames, algorithm cleanup, UI redesign, or an
  unrelated formatting sweep.
- Do not regenerate a golden fixture unless an explicitly approved semantic
  change requires it and the resulting diff is reviewed.
