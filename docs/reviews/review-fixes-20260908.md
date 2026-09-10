# REVIEW-FIX-20260908

User authorization: fix all fifteen findings from the 2026-09-08 review, including the necessary bounded API/IPC and development-policy changes. This is one correction task with parallel owned implementation areas, not a refactoring-only extraction campaign.

Baseline: original HEAD/main merge-base 6f22761a4305443541452e7409639114f4c10dfd. Isolated snapshot 3c39090df2951673d4cc2c1ce044830b8287c3c5 includes 221 existing changed/untracked user files. Original working directory remains untouched while implementing; merge only the correction delta at handoff.

Owned invariants: renderer-safe DTOs; credential lifetime/redaction/zeroization/binding; durable generation admission/checkpoint/terminal order; Storage transactions, CAS identity/no-follow/same-handle reads, lock/fsync/recovery order; deterministic ordering and golden outputs; frozen migrations; generated IPC/capability exact agreement. No dependency upgrade or schema change is planned.

Areas: runtime agent owns stream event framing, asset admission and token trimming; pagination agent owns creator/memory page repositories, Core/Shell/native/client contracts and UI; frontend agent owns preset race, scroll complexity, provider loading and behavioral UI checks. Root owns architecture policy, completed refactoring archives, facade retirement, validation and integration; message history batching/incremental reconciliation will be assigned after runtime work.

Public entry points remain each crate lib.rs, Core methods, ShellApi methods, registered Tauri commands and renderer IPC client. Existing entry points remain compatible where practical; new bounded page endpoints are authorized. No new Core Stored* re-export is allowed.

Root target files: scripts/check_source_architecture.py, scripts/check_ai_context_map.py, scripts/report_refactoring_baseline.py and focused new helpers/tests; config/core-storage-public-api-baseline.json after measured API changes; a completed-refactoring archive descriptor; .github/workflows/ci.yml only as needed; AGENTS/ADR documentation. Source-size caps may be lowered by measured parent reductions and never increased.

Root symbol inventory: /tmp/lorepia-review-fixes-policy-symbols.txt. Preserve existing validation and parser APIs; no parser rename/extraction needed. Adjust policy-comparison functions to permit explicitly tracked contract updates while independently enforcing layer/Stored*/wildcard invariants. Completed historical context/report artifacts will be verified against a fixed ancestor, not unrelated future source growth. Facade retirement will require the old source to be absent rather than allow a live file to evade its classification.

Expected size delta: bounded new implementation/test modules add code; corresponding giant parent changes must remain within existing caps. Exact before/after measurements are recorded at validation; no giant-file exceptions. Root policy changes should remove permanent-growth loops without replacing them with another large approval mechanism.

Risks: pagination stability under edits, oversized individual documents, stale frontend requests, stream flood handling, response body ownership, terminal display projection authority, user edits advancing while isolated work proceeds. Regression tests must cover these behaviors rather than mirror implementation strings.

Governing material: root/core/storage/frontend AGENTS.md; ADR0001 through0005; docs/architecture/storage-public-api-audit.md; review report /Users/codexer/.codex/reviews/lorepia-20260908-review.md. ADR0006 will record the approved distinction between invariant checks and evolving contracts/historical refactoring evidence.

Baseline root validation: Python architecture/context/report tests and architecture measurement started before source edits; logs /tmp/lorepia-review-fixes-architecture-before.log. Runtime/pagination/frontend baseline and targeted validation are recorded in their area logs. Full Rust, frontend, generator, architecture and security/workflow gates follow integration.

Root policy baseline: 75 Python tests passed before edits; current source architecture baseline failed only known public API contract drift plus existing WIP sizes as logged. Implemented explicit current-contract updates with independent layer validation; archived completed campaign evidence at original HEAD; retired-file-aware facade cleanup. No size limits were raised. Regression coverage includes context growth/deletion versus archive, evidence tampering, incomplete archive, unknown commit, live facade evasion, and forbidden dependency even with updated manifest.

Final source ratchets lowered by the measured parent reduction from the isolated snapshot (existing headroom preserved):
- apps/lorepia/src-tauri/src/contract.rs: {'bytes': 83713, 'lines': 2139} -> {'bytes': 83586, 'lines': 2131}
- apps/lorepia/src/app/app-controller.ts: {'bytes': 21693, 'lines': 566} -> {'bytes': 21618, 'lines': 562}
- crates/chat/src/generation.rs: {'bytes': 104268, 'lines': 2799} -> {'bytes': 103271, 'lines': 2772}
- crates/orchestration/src/resolver.rs: {'bytes': 78778, 'lines': 2033} -> {'bytes': 78360, 'lines': 2016}
- crates/shell-api/src/orchestration.rs: {'bytes': 226471, 'lines': 5951} -> {'bytes': 223594, 'lines': 5855}
- crates/storage/src/orchestration.rs: {'bytes': 29269, 'lines': 777} -> {'bytes': 28803, 'lines': 765}

Dependency manifest correction records lorepia-content -> ring already present in original HEAD; no Cargo manifest, lockfile, dependency or version changed. Current Core/Storage public API inventory was refreshed against the checked-in implementation, including pre-existing HEAD drift. Policy/archive review was independently checked by the pagination reviewer.
