# Current-design frontend cleanup

- Task ID: `UI-RETIRE-20260910`.
- Baseline / merge-base: `211f1c6f0770485f957beed3d31229211e1db3a3`.
- Worktree before editing: clean `main`; work branch
  `codex/retire-legacy-ui-20260910`.
- Authorization: remove the previous frontend implementation using the current
  design as the reference. This is retirement, not a redesign or a migration of
  backend behavior.
- Governing material: root and frontend `AGENTS.md`, ADR 0006. Completed
  refactoring archives remain immutable.

## Scope and inventory before editing

The static import inventory follows TypeScript/Svelte imports, CSS imports and
literal worker URLs from `main.ts` and the current `preview/ui/main.ts`. It finds
no production dependency on `preview/`. The current native entry reaches
`WorkspaceApp` and `ApplicationFrame`. The earlier `App`, `ChatPane`, provider
panels and stylesheet tree are reachable only from the old demo. `WorkspaceFrame`
and `UiPreview` are not part of either current entry.

- Target files: retired Svelte views, their exclusive presentation helpers and
  styles, preview forwarding files, associated tests, demo entry wiring and
  current developer documentation. Current source-size inventory may retire
  only paths actually removed; no surviving cap or limit is raised.
- Expected size delta: remove 139 unused Svelte/CSS files plus exclusively old
  presentation helpers and tests. Final measured totals are recorded below.
- Remove: `App`, `AppDetailHeader`, `ChatPane` and old chat surfaces; old library,
  conversation, persona, provider and orchestration presentation; old shared
  controls and styles; unused `WorkspaceFrame` and rail/prototype composition.
- Remain: every dependency of the current native and UI demo entries; shared
  root/feature controllers, IPC contracts/client, portable runtime/worker and
  asset security, imported compatibility and prompt-history workflow helpers.
- Public entries remain: `main.ts`, `index.html`, `workspace.html`,
  `preview/main.ts`, `preview.html`, `ui-preview.html`. Both demo URLs will use
  the current UI with an isolated in-memory client.
- No symbol movement, current UI renaming, dependency/API/Cargo/IPC/schema
  changes or current screen/style edits are planned.

## Invariants and validation

- Current native renderer output, navigation, gestures, composer, assets,
  appearance and persisted behavior remain unchanged.
- Demo data remains fixed and isolated from native data and credentials.
- Controller epochs, request authority, stream ordering, generation admission,
  cleanup and portable runtime security remain owned by their existing modules.
- Baseline: frontend tests and both worker post-tests pass; production Vite
  build passes. SHA-256 hashes of all seven production build files are captured
  outside the repository for an exact before/after comparison.
- Validate: complete frontend check/tests; production and UI builds; native
  bundle equality; entry isolation; architecture/IPC/i18n/archive checks and
  relevant Python checker tests.
- Semantic risks: accidentally deleting a shared import or worker, losing a
  regression that applies to the current UI, changing CSS load order, leaving a
  demo URL wired to a retired shell. Static dependency reachability, type/build
  checks, current component tests and output hashes address these risks.

## Test ownership

Two aggregate inventory owners (`ChatPane` and `ProviderSettings`) are retiring
while their shared feature modules remain. Transfer each complete surviving
child-entry list to a current owner, `WorkspaceApp` and `WorkspaceFeatures`.
The architecture checker must accept that transfer only after the old parent
is gone, the replacement is a real production source and the entire surviving
tracking scope remains in one replacement group. Missing children, a live old
parent, stale replacements and raised caps must still fail. This updates
bookkeeping for removal; it does not change a frontend or backend contract.

Retire assertions for retired screen structures together with those screens.
Keep all controller/IPC/runtime tests. Shared composer, press feedback, keyboard,
theme, tooltip and notice assertions must target the current implementation,
not retain the old prototype as their harness. The current workspace suites
cover library/chat navigation, settings, personas, creation, imports and runtime
approvals; pure shared gesture/measurement regressions remain with their owners.

## Result

- Removed 182 non-test frontend files (38,853 lines), including the old shell,
  controls, feature views, stylesheet tree, prototype and forwarding modules.
  Non-test TypeScript/Svelte/CSS totals fall from 96,581 to 57,803 lines; the
  retained test-support additions are included in those totals.
- Retired 52 old test paths; rehomed six complete shared test files and the
  applicable composer, edge-back and feedback cases in three more files. The
  43 other suites targeted retired presentation. Existing controllers, IPC,
  runtime/worker and current workspace suites remain.
- Both demo HTML pages use `preview/main.ts`, which mounts the same current
  `WorkspaceApp` as the native app with isolated fixtures. Browser smoke checks
  on both URLs show the same four-tab home and four demo character cards.
- All 293 files in the native entry's dependency inventory remain byte-for-byte
  unchanged. All seven outputs of the default production build have identical
  SHA-256 hashes before and after cleanup. The UI build also passes.
- The literal inventory retires only 66 deleted-file records (2,438 lines of
  old inline Hangul debt). The remaining 355 lines in 32 files keep their exact
  existing fingerprints. No new literal debt is approved.
- Source aggregate ownership moves with the full surviving child scope. All
  per-source caps and language/kind limits remain unchanged; the retired
  `ChatPane` cap is removed. No archived refactoring evidence is rewritten.
- Full frontend suite: 120 files / 661 tests passed, including Lua and regex
  worker post-tests. Python checker suite: 102 tests passed. IPC, archive,
  context, architecture, workflow-security and i18n checks pass.
- Formatting/lint/typecheck are included in the final frontend gate. Final
  validation is recorded in the task response; there is no UI redesign,
  backend contract change or dependency update in this cleanup.
