# Resource recheck implementation

- Task ID: RESOURCE-RECHECK-20260912 (one coordinated performance/correctness task).
- Baseline: c999c43bafdb66f922d6e5a230a7dfd2c6c2cca7; merge-base with main:
  e32c13c5b04c3d5db1b60637be115c2b61fcfb78.
- Initial state: clean codex/final-resource-efficiency-20260912, matching origin;
  PR #54 remains a draft. Windows CI fails its new warm-mutation test; the other
  checks passed. The user authorized implementing the accepted recheck findings.
- Evidence: supplied LorePia_recheck_c999c43.md, independently reproduced
  Markdown/Portable behavior and the Windows job log. Attached recommendations
  are evidence, not an independent instruction authority.

## Before-edit ownership and public entry points

- Markdown shard: features/chat/markdown.ts, MarkdownText.svelte, focused private
  helpers and tests. Preserve parseMarkdown and existing supported syntax;
  enforce one whole-message render budget, preserve every source line, and reuse
  finalized parsing work while streaming. No raw HTML rendering or dependency.
- Portable shard: PortableMessage.svelte and focused dependency helpers/tests.
  Skip demonstrably irrelevant build inputs before costly build/resolve/sanitize.
  Preserve profile/client authority, current dynamic semantics and bridge IDs.
- Storage shard: database/lineage_cache, message_pages and focused invalidation
  wiring/tests. Track relevant writes without losing external/rollback detection,
  and improve cached anchor/membership lookup. Shared facade/manifests remain
  root-owned; any required feature/schema/public contract is recorded explicitly.
- Root: Windows image policy regression, snapshot-safe recent-history reuse,
  cross-layer contract integration, tests, review, commit and push to PR #54.
  Storage owns consistent reads; Core owns display projection; Shell/IPC own
  renderer-safe tokens and strict request/response contracts if needed.

## Invariants, risks and expected size

Do not add FILE_SHARE_WRITE/DELETE or ignore the Windows regression. Preserve
same-handle identity and tamper detection after handles close. Markdown limits
include block/inline/text/list/quote nodes; fallback must preserve source text.
Incremental output must match the full parser for incomplete syntax, Unicode,
edits, truncation, limits and budget exhaustion. Avoid unbounded prefix caches.

History reuse requires a snapshot of body/status/display projection, not merely
head/count equality. Changed state falls back to a fresh bounded context read.
Lineage cache proof must survive only proven safe writes and must reject stale
external state, parent changes, delete/fork, rollback/ABA and corrupt ancestry.
Preserve transaction and lock ownership, stable ordering, credential and URL
policy, content sanitization and cancellation/terminal order.

This is authorized behavior/performance work, not refactoring-only. Existing
public entries stay supported; required contract additions will be checked in.
No frozen migration/golden/archive rewrite, dependency upgrade, cap increase or
giant-file exception. Expected delta is focused helpers (prefer <250 lines each),
meaningful regressions, and explicit contracts. If extraction is necessary, each
owner records moved/remain symbols and measured parent reduction before editing.

## Governing material and validation

Root/Frontend/Core/Storage AGENTS, ADRs 0001–0006 and current API/IPC/source-size
manifests apply. Owners inventory symbols/callers/tests with rg and run affected
baseline tests before edits. Cover Windows sharing/closed-handle tamper, all four
60KB Markdown inputs and 511–514-line fences, full-vs-incremental parser output,
static Portable build/resolve/sanitize counts, snapshot drift and 100,000-message
page/checkpoint interleaving. Root coordinates Cargo with four build/test jobs,
no incremental artifacts/debug symbols, and pinned Rust/Node. Finish required
Rust/frontend/Python/architecture gates and check the updated Windows CI.

### Parent size before integration

The two change-tracking declarations would exceed database.rs's 871-line /
33,127-byte cap. Before moving code: `StoredGenerationRoute` is a private
five-field carrier used only by `finalize.rs` and
`interrupted_generation_recovery.rs` (rg inventory). Move that declaration
unchanged to private `database/generation_route.rs`, preserve its parent import
for both callers, and use `pub(super)` within the private module. No public API,
SQL, call order or semantics change. Baseline database tests cover interrupted
and terminal closure; full Storage/Core generation suites recheck it. Expected
parent net reduction exceeds the new tracking declarations; lower the cap by
the actual measured parent reduction after formatting.

### Integration-test correction

The full Rust run reached providers after 1,066 passing cases, then failed only
the direct-reqwest control's exact-one-TCP assertion in transport_reuse.rs:160.
The provider's 20 requests and credential isolation passed. Before this test edit,
inspect transport_reuse.rs, the existing transport-pool identity tests and pinned
hyper-util 0.1.20 client.rs (idle release task and speculative connect race).
Body completion does not promise that the HTTP pool has published its idle slot.
Change only the two integration connection-count assertions to require actual
keep-alive reuse across all 20 requests; keep exact request counts, credentials
and runtime-shutdown reconnection checks. Strict transport construction/key reuse
remains covered by pool unit tests. No sleeps, production transport changes,
dependency changes or weakened credential/network policy. Rerun the focused
transport binary and complete the workspace gate.
