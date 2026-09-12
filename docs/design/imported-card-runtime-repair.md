# Imported card runtime repair

Task: COMPAT-PERF-20260912. Baseline: 15e559b, branch codex/retire-legacy-ui-20260910. Existing uncommitted import/schema-42/chat/press changes are preserved. The user explicitly authorized resolving both the measured Alternate Hunters latency and missing settings UI.

## Owned work and invariants

Storage package approval reads and module runtime resolution: retain public APIs, schema, canonical hashes, CAS no-follow/same-handle verification, transaction-local DB revalidation and lock order. Memoize pure validation of exact JSON input, never DB approval or capability authority. Inputs changed in any byte miss the bounded cache. A separate four-handle source lease follows ADR 0005's existing verified-open-handle identity boundary; it does not cache approval authority. Collapse duplicate runtime preview/persist validation into one existing immediate transaction. Keep stable asset/component order.

Renderer portable display/runtime: implement bounded CBS pattern expansion and missing condition aliases; keep regex workers/budgets, runtime grant checks, opaque sandbox, message source/nonce checks, and no external network/host markup. Add a contained card UI surface instead of letting imported CSS cover app chrome. Expose approval/retry status in the chat flow; do not auto-grant model or advanced permissions.

Public entry points stay Storage::get_completed_package_authority_by_approval_id, get_applied_module_runtime_plan, Core runtime resolution, WorkspaceApp and PortableMessage. No symbols are moved merely for cleanup. New focused helpers stay internal. Expected delta: bounded helper modules and regression tests, with unchanged public contracts and no baseline increases.

Risks: stale cached data, omitted revalidation, changed ordering, regex budget bypass, imported UI obscuring navigation, permission regression, async stale results. Tests target those risks plus actual provided-card rendering and repeated large-module reads. Governing material: root and owning AGENTS.md, ADRs 0001–0005; SEED Bottom Sheet and Alert Dialog for host UI layout. Before evidence is saved under /tmp/lorepia-compat-audit-20260911/performance-and-settings-diagnosis.md.

## Sequential bounded extraction: COMPAT-VERIFY-EXTRACT

Package memoization tests pass (14) and a second native sample now locates repeated pure ModuleMergeReview/ResolvedModulePlan/ApprovedModuleActivationPlan/AppliedModuleRuntimePlan verification. Before changing that algorithm, move only those four verify impl blocks from modules.rs into modules/verification.rs. Public signatures, all hash functions, constructors and tests remain. Baseline module tests: 21 passed. Expected parent reduction equals the moved bytes/lines minus one module declaration; lower only that measured baseline. This extraction adds no behavior, IO or dependencies. Then COMPAT-VERIFY-MEMO adds bounded, exact-value memoization of successful pure verification; all live DB/CAS authority checks remain in Storage.

## Sequential bounded extraction: COMPAT-FRAME-EXTRACT

Baseline remains 15e559b with the recorded dirty worktree. PortableMessage now exceeds the 600-line default cap. Inventory: portableFrameDocument, portableFrameBridge, BASE_STYLE and escapeHtml move unchanged to portable-renderer-frame.ts; the Svelte component remains the caller, async/race owner, origin/source/runtime-ID validator and asset resolver. Expected reduction: about 115 lines. Governing material: frontend AGENTS.md and the renderer protocol/policy. Run PortableMessage and renderer-policy tests before and after extraction. No public DTO, dependency, security boundary or rendering behavior changes in the extraction. Follow separately with COMPAT-FRAME-CSP: deliver the same trusted bridge as a bundled script under the existing parent CSP, without enabling imported scripts, same-origin access, networking or navigation.

Before/after extraction: 24 component/policy tests passed unchanged. The follow-up bridge uses one external bundled script with an unpredictable per-frame nonce/identity; parent script-src, opaque sandbox and source/origin checks stay unchanged. Only literal CSS-generated room icon text and contained positioning are accepted. Imported scripts, URL/attribute CSS, arbitrary navigation and networking remain rejected.

## Runtime performance and compatibility changes

- Scope-derived Svelte dependencies prevent root-store publications from repeatedly fetching the 5,624-asset profile, discarding grants or restarting Lua. Message synchronization also observes only actual message/generation/stream-state changes.
- Exact verified JSON/plan memoization removes repeated deterministic work. Current DB, capability, CAS identity and durable plan comparisons still run. Prepared asset queries and one runtime materialization transaction avoid repeated query preparation and validation.
- Four reusable regex workers retain the 75ms default execution deadline, bounded queue and input/output limits. Worker startup has a separate 1s bound and cannot permanently disable an unexecuted rule. A killed/failed worker is discarded; idle workers expire after 30s. Superseded display builds stop scheduling further transforms, retain the previous completed frame, and cannot publish stale warnings.
- CBS pattern expansion supports last-message guards, not_equal, width comparisons and numeric single-equals toggles. Disabled conditional patterns are skipped. Original button macros map to the existing validated action channel.

Optimized native audit, same isolated copy of the imported card plus modules, three passes: module review 743/601/569ms; render profile 1710/1529/1566ms; effects 0ms. Earlier debug measurements exceeded 20s for render profile; build-mode and algorithm changes both contribute, so these figures are not claimed as a same-build microbenchmark. UI preview builds now use release by default, with an explicit debug command retained.

Native checks use the existing Alternate Hunters conversation without sending a model request. Raw source, generated HTML and local fixture-specific tests stay outside the repository under /tmp/lorepia-compat-audit-20260911. No original card or attached module is modified.

## Contract correction: persisted character revision

The effective room profile incorrectly replaced `Revisioned<CharacterContentV1>::revision_id` with a derived module-plan hash. The existing runtime-state contract requires an actual revision owned by that character, so SQLite correctly rejected every state read/write with an active module. Keep that field bound to the underlying stored card revision. This corrects the existing field's meaning without adding a DTO, IPC, schema, or relaxed authority check. The merged profile still carries the exact approved module content, and runtime grants hash the entire profile plus selected capabilities, so a module/script/asset change invalidates approval independently of the storage revision.

A new Core vertical test first reproduced the invalid synthetic revision, then verifies a module-enabled settings write, close/reopen restoration, and rejection of a forged revision. Native persistence is checked using the original card's language and session switches. A separate gesture regression ensures the back gesture does not capture a native disclosure summary and suppress its click.

## Verification completed

- Optimized native build: the provided Alternate Hunters conversation opens the original gear/globe settings. Korean/English and Free Scenario ON/OFF update correctly, and the advanced permission disclosure opens normally.
- A read-only database check confirmed durable state writes. After terminating and starting the native process again, enabling the card restored English and Free Scenario ON. The test changes were returned to Korean and Free Scenario OFF; the final stored values are `lang=1` and `scenario=0`.
- `cargo test --workspace`: 1,898 passed, zero failures, 13 ignored. The subsequently added Core revision/persistence regression passed separately. `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all --check` passed after that correction.
- Final `npm run check`: formatting, lint, native TypeScript and Svelte checks passed (zero Svelte errors/warnings). Final `npm run test`: 725 tests passed, followed by the real Lua timeout and regex worker sandbox regressions.
- Actual-card display and Lua fixtures passed against the supplied 5,624-asset merged profile. IPC generation, source architecture, and the 102 Python tooling tests passed. Temporary card-specific fixtures remain outside the repository.
- Native verification did not send an external model request. Model-backed generation is not claimed as a live end-to-end test.

PR #52's CodeQL check identified two single-pass modifier-removal expressions in regex-flag parsing. Their output already passed a flag-character allowlist and never entered an HTML sink. Replace both with direct, depth-aware flag collection so nested or unfinished modifiers cannot accidentally contribute flags such as `s`. Display tests and the real worker test cover ordinary CBS, nested, unfinished and split-tag input. The HTML sanitizer, CSP and worker bounds remain unchanged.

Windows CI exposed a source-lease sharing violation when importing an identical source again: Windows requires a write-capable handle for the existing file's durability flush. Under the existing outer CAS mutation lock, publication now evicts matching source leases before the unchanged hash/no-clobber/fsync sequence. Reader sharing restrictions remain unchanged. A focused duplicate-publication regression complements the Core restart/replay test. Mutation tests accept Windows sharing-violation error 32 as OS-level prevention; replacement of an open destination also reports access-denied error 5 on Windows CI. Both blocked-replacement outcomes require successful verification and unchanged original bytes; actual replacements/mutations must still fail identity/hash verification.
