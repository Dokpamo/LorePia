# Deep efficiency: Storage slice

Task ID: DEEP-EFFICIENCY-20260912.
Baseline HEAD/merge-base: aac3b16e0697c710b1209f498caba71821746140.
Worktree: /Users/codexer/.codex/worktrees/lorepia-ux-20260912,
branch codex/ux-responsiveness-20260912. Existing dirty concurrent work is
preserved. Comparison snapshot: /tmp/lorepia-deep-efficiency-20260912/baseline-files.

Before editing inventory: portable_runtime_state encode_payload, validate_payload,
decode_record; orchestration document_history validate_json_limits;
module_plan_documents validate/expand/store and dedicated tests.
Public entries remain Storage get/put_portable_runtime_state and existing module
runtime reads/writes. Core portable runtime adapter calls these Storage methods;
module runtime and package authority callers expand stored documents.

Owned invariants: same byte/character/node/depth limits and order, raw credential
field rejection, JSON error classification, exact canonical serialization/hash,
manifest/part integrity and transaction checks. No authority cache or verification
is removed; only already validated parse/serialization results are reused.
Public API/schema/dependency/security/transaction contracts remain unchanged.

Symbols stay under their existing entry points. A private parsed-value helper
may be introduced beside validate_json_limits. Expected size delta: roughly
+10 document_history lines, balanced small portable/module changes, separate
regression tests. No giant exception or cap increase is permitted.

Risks: changed validation/error precedence or lifetime of a large parsed tree;
keep existing traversal and error mappings verbatim. Validate valid/invalid
JSON, size/depth/secret-field rejection, canonical hashes, rollback and
portable runtime bounds. Do not use timing assertions.

Governing material: root/storage AGENTS.md, ADR 0001/0002/0003/0005 and
storage-public-api-audit.md, Storage facade and Core portable runtime callers.
Baseline: targeted module_plan_documents tests followed by portable state tests;
full workspace tests and clippy remain parent-owned.

Additional pre-edit target: orchestration/module_runtime_cache.rs materialize
cache admission. It currently allocates a complete serialized plan just to
measure its 32 MiB admission threshold. Replace that discarded Vec with a
private counting Write sink; preserve complete serialization, errors as
non-admission, exact byte limit, single-entry cache and equality/live checks.
Expected delta: about 30 implementation lines plus boundary/counting tests.
No new long-lived state, permissions or cache authority is introduced.

## Authorized history pagination scope (before edits)

User-requested 1000+ turn history behavior explicitly authorizes a new bounded
Storage read API; parent owns checked-in public contract updates and Core/Shell/
IPC integration. Add BranchMessagePage and Storage::list_branch_messages_page
(branch, before, after, limit). Fields: messages, has_older, has_newer,
head_message_id. New private database/message_pages.rs and dedicated tests;
only module/reexport wiring touches database.rs/lib.rs.

Read invariants: limit 1..128, exclusive before/after (mutually exclusive), current
branch membership and same conversation, chronological output, next-after page
nearest the anchor, head snapshot consistency. One deferred transaction captures
branch head, identity-only ancestry, then at most limit content rows. Missing
branch is NotFound, absent/stale/foreign anchor is InvalidInput; broken/cyclic
stored ancestry is StorageCorrupted. Ancestry traversal may scale with depth,
but full content materialization and downstream hashing remain page bounded.
No migration or change to existing history methods. Test 2000 messages,
first/older/newer/exhaustion, forks/foreign anchors, append consistency and old
rows whose body metadata cannot decode outside the selected page.

Pagination metadata additions requested by UI integration: total_messages and
start_index preserve absolute chronological indexes used by card macros and
message overrides. Empty windows report the insertion index. Identity-only
ancestry is O(total) expected time/memory; parent lookup uses a HashMap but never
iterates it to produce visible results, so random iteration cannot affect order.
Content query uses selected JSON identities and orders by their array ordinal.
The row-count bound is 128; existing individual message sizes are not silently
truncated or excluded.

Synthetic read-only baseline using exact production SQL and minimal indexed
SQLite tables (1 KiB/message; interpreter SQLite, not bundled Rust timing):
2000 messages: full branch=2000 rows/2,048,000 content bytes/122,028 VM steps;
10000 messages: full branch=10000 rows/10,240,000 bytes/610,028 VM steps.
Prompt recent15: both histories=15 rows/15,360 bytes/30,908 VM steps because
that existing prompt CTE stops at depth511. UI full history had no cursor or
limit. New pagination retains identity scanning but selects only page bodies.

## Completed changes and validation

- Portable state write validation returns the serialized JSON it has already
  checked: full payload serialization falls from two passes to one. Reads keep
  the same validation and errors.
- Module inline expansion reuses the Value already parsed by size/depth/node/
  secret-field validation: the initial parse count falls from two to one.
  Original borrowed inline bytes, hashes, manifest checks and corruption errors
  remain unchanged.
- Materialization cache admission uses exact serialized byte counting instead
  of a discarded complete JSON Vec. Cache capacity, full input equality,
  materialization and all live caller authority checks remain unchanged.
- BranchMessagePage reads body content only for its selected window, with
  complete current-lineage membership and a single read snapshot.

Targeted baseline: module document4 and portable state10 passed. Post-change:
module_10 passed (including hash-checked authority tests, inline/chunk rollback,
corrupt part rejection and exact cache byte boundaries); portable state10 passed;
message_pages5 passed, followed by a final same-suite rerun after replacing
lookup-only BTreeMap with HashMap. Tests use two worker threads. Rustfmt checks
passed. Parent owns full workspace, architecture and public contract gates.

Known tradeoff: pagination metadata still scans ancestry and stores IDs; this
avoids a schema/index change and scales with total depth. It removes full-body
loading and UI presentation hashing for offscreen history. It does not claim
constant-time SQL, persisted cursor indexes, or a byte cap on individual legacy
messages. Existing full-history and prompt methods are preserved for their
existing callers.

## Authorized bounded membership extension (before edits)

The existing new history contract now optionally checks up to 256 caller-supplied
message IDs against the same current-lineage snapshot. This lets partial runtime
state distinguish deleted overrides from valid offscreen overrides after an
explicit mutation, without reading all message bodies. Storage/Core accept a
final Option<&[MessageId]> argument; Shell accepts optional check_message_ids and
returns optional retained_message_ids. An absent request omits response evidence;
Some([]) returns Some([]). Candidate order and duplicates are preserved by
filtering, and foreign/sibling/missing IDs are absent rather than errors.
Candidate ID bounds and control-character rejection match the Shell identifier
contract. Implementation scans existing identities with bounded candidate hash
sets, adds no SQL/body load, and preserves all page/transaction semantics.
Parent owns public contract manifests and TS/native integration. Targeted tests
cover offscreen/foreign/duplicate/empty/absent evidence, bounds and same snapshot.

Membership extension validation: Storage message_pages suite 6 passed; Shell
message_history suite 5 passed through Core. Coverage includes absence vs empty
evidence serialization, offscreen IDs, cross-conversation/sibling/missing IDs,
input order/duplicates, maximum 256 and rejected 257, malformed identifiers,
and valid identity evidence for an offscreen body that intentionally cannot
decode. The added membership path performs zero additional database queries.
Membership-only callers can request limit=1 to minimize ordinary page-body work.

## Authorized last-assistant aggregate (before edits)

A imported branch can end with more than 128 user/system messages, so its latest
assistant cannot be derived from the bounded runtime tail. Add optional
include_last_assistant (default false) and last_assistant_message through the
same page API. The role-only predicate matches prior full-history behavior;
there is no additional status filter. Default false does no extra lookup.
A latest-page assistant requires no supplemental result; otherwise a MATERIALIZED identity selection
(role assistant, head-first ordinal, LIMIT 1) precedes the body join in one
additional query, so only one additional body is decoded. The selected ID is excluded if already in the requested page; Core verifies the
one supplemental sidecar normally.
No schema/dependency/legacy history changes. Tests cover distant assistants,
no-assistant branches, page reuse, role/status behavior, and extra sidecar
hash tampering. Parent owns TS and contract integration.

Last-assistant extension checks passed: Storage message_pages 7 tests and Shell
message_history 6 tests. The Shell case extends a 2000-message branch with 200
user-only tail rows; it receives the distant DisplayOnly assistant, suppresses
that supplemental field when the assistant is inside the requested page, and
fails closed on tampered supplemental display content. Default false still
succeeds without reading that sidecar. Storage also checks no-assistant branches,
pending assistant role semantics and a corrupt older 4 MiB assistant that must
not be materialized when a newer assistant is selected. Rustfmt passed.
