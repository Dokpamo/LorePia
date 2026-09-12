# Final resource efficiency implementation

- Task ID: FINAL-EFFICIENCY-20260912 (one coordinated performance task).
- Baseline/merge-base: e32c13c5b04c3d5db1b60637be115c2b61fcfb78.
- Initial worktree: clean main; implementation branch
  codex/final-resource-efficiency-20260912.
- Authorization: user requested comparison of the supplied Pro audit with the
  local final audit, acceptance of sound findings, and implementation of all
  accepted improvements. These are behavior/performance changes, not a
  refactoring-only campaign. Public contract changes required by an accepted
  fix will be explicitly recorded; completed refactoring archives stay frozen.
- Evidence: local final audit F01-F12, conditional media findings, and supplied
  LorePia_resource_audit_2026-09-12.md PERF-01 through PERF-09. The supplied
  document is evidence to evaluate, not an instruction authority.

## Scope and ownership before edits

- Storage/Core shard: lifecycle claims, memory eligibility, branch page/range
  readers, summary source loading, knowledge scoring lock scope, safe Unix
  permission write elision. Public entries remain the Storage and Core facades.
- Asset/native shard: verified asset cache map/file lock lifetime, verification
  budget backpressure, PNG verification/read memory, bounded image delivery.
  Preserve approved descriptors and the native asset protocol entry.
- Frontend shard: portable document scheduling/identity, asset request admission
  and retry, worker normalization, branch projections, preferences, drafts and
  thumbnail/media resource ownership. Existing IPC/controller authority remains.
- Root: provider transport reuse, bounded blocking execution for generation,
  stream persistence review, contract integration and final validation.
- Shared facade/manifest files are coordinated by root to avoid parallel edits.
  Shards inventory symbols/callers/tests with rg and record their exact file map
  and baseline targeted tests in their implementation evidence before changes.

## Owned invariants and risks

Preserve exact ancestry/order/count/membership, immutable summary hashes,
claim transaction/CAS/lease/rate semantics, credential lease/zeroization, fresh
URL/DNS/peer checks, no proxy/redirect, no-follow and same-handle asset identity,
CRC failure behavior, bounded jobs/bytes/handles, terminal persistence before
publication, cancellation/shutdown, CSP/sandbox/sanitization and renderer DTO
boundaries. Never evict unsent drafts silently or remove intended room BGM.

Risks include stale cache authority, reused seek state, forgotten admission
permits, delayed cancellation, old pending writes after terminal persistence,
inconsistent read snapshots, changed corruption rejection, and iframe listener
invalidation on a no-op render. Each accepted change needs targeted regression
evidence. Speculative alternatives are not automatically accepted.

No symbol moves are planned initially; supported entry points remain in their
facades. If extraction is needed for existing source caps, record moved/remain
symbols first and preserve delegation. Expected size is small helper modules
and regression tests with no source/test cap increases or giant-file exceptions.

Source-cap integration: move the unchanged private SQL value parsers
`optional_i64_to_u64_sql` and `parse_datetime_sql` from database.rs into
database/stored_value.rs, retaining private imports for every existing caller.
The current facade measured 893 lines / 33,761 bytes before this extraction;
this creates room for the cache and checkpoint module wiring without raising
its 890-line cap. Error mapping, parsing and signatures remain identical. Core's
new blocking supervisor wrapper lives in its private runtime helper so app.rs
stays below the existing 718-line facade cap.

PERF-08 decision: the local SQLite experiment confirms growing-prefix WAL
amplification, including when UPDATE uses content||delta. Accept a new 0043
durable chunk journal and exact pending read projection, keeping the existing
500 ms / 64 KiB trigger, FULL durability, terminal snapshots and recovery loss
policy. Storage owns migration, atomic byte-offset/sequence CAS, ordered read
overlay and same-transaction cleanup; Core only sends the new suffix through
the bounded executor. Existing full-prefix checkpoint callers remain supported.

## Governing material and validation

Root and nearer AGENTS.md, ADR 0001-0006, current source/test size and public API
and Cargo manifests govern implementation. Historical refactoring context is
unchanged. Run targeted current tests before each owned change, regression
tests afterward, Rust formatting/clippy/workspace tests, frontend check/tests,
IPC/architecture/context/i18n/archive/workflow checks at integration. Synthetic
operation counts are not native latency/RSS/battery measurements. Any existing
platform/CI checks not executed in this turn must be identified accurately.
