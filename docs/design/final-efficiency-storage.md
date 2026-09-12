# FINAL-EFFICIENCY-20260912 — Storage implementation record

Final implementation state recorded on 2026-09-12: selected memory-source reads,
migration 0043 and bounded checkpoint proofs are implemented. Parent owns final
combined verification; earlier test results below are dated implementation evidence,
not a claim that the final gate has completed.

Before edits: main baseline e32c13c5b04c3d5db1b60637be115c2b61fcfb78;
branch codex/final-resource-efficiency-20260912. Parent task record already
untracked; preserve parallel changes. Root/storage/core AGENTS, ADR0001–0006,
facades, earlier efficiency evidence and supplied Pro report govern this slice.

Targets: lifecycle_outbox claim predicate; memory_queue eligibility SQL;
Core auxiliary_tasks/summary source selection; database/message_pages and new
lineage_cache/range helpers; knowledge_embedding row snapshot/scoring;
database/private_path startup permissions. The initial checkpoint exclusion was
superseded by the explicitly authorized PERF08 scope below: this slice implemented
Storage journaling, while parent owns the Core forwarder. Shared Storage
module/field/facade/manifest wiring is coordinated by parent. The implemented
public range signatures are recorded below.

Preserve claim transaction, limits, timestamp comparisons, ordering, rate counts,
legacy attempts, exact source SHA/filtering/membership; complete ancestry count,
cycle/missing-parent corruption checks; cross-connection and local-write cache
invalidation; knowledge all-row corruption, arithmetic and work bounds; Unix
path/type/symlink and permission guarantees. No dependency changes. The initial
no-schema-change scope was superseded only by the explicitly authorized new 0043
migration; applied historical migrations remain unchanged.

Expected size: small private helpers/test children, no giant cap increases.
Risks: stale snapshot/cache, changed corruption precedence, premature rate limit,
full-history side effects, allocation versus lock-time tradeoffs. Do not silently
trade security/durability guarantees for speed. Targeted baseline: message_pages,
queue durable limits, knowledge_embedding_storage, private_storage_permissions,
lifecycle integration tests, followed by new regression and crate coverage.
No symbol movement except needed private helper extraction; old public methods
remain. Pro metadata LIMIT/UNION shortcuts and wholesale verification skipping
are rejected. Measurements use synthetic data and operation counts.

## Implemented scope and public contract

- F01: lifecycle claim SQL now explicitly includes its already implied
  `status != 'acknowledged'` predicate, allowing the existing delivery partial
  index. SQL moved into private lifecycle_outbox/claim.rs; claim transaction,
  predecessor ordering, lease and CAS remain unchanged.
- F02: memory eligibility computes durable concurrency/rate eligibility once per
  distinct due task revision, with MATERIALIZED requested/eligible sets. Original
  attempt-array and legacy started_at counts, julianday comparisons,32-attempt
  bound, null-profile behavior and final order remain. Private memory_queue/claim
  contains SQL and differential tests against the pre-change query. Work still
  grows with retained history and distinct requested revisions; no evidence is
  deleted and no persistent rate/index schema is introduced.
- PERF01: private database/lineage_cache holds one validated identity vector with
  8MiB admission accounting. Cache identity includes conversation, exact head,
  same-connection total_changes and external PRAGMA data_version. Callers have
  already opened a read snapshot through branch lookup; the cache accepts a
  Transaction reference. Full UNION traversal and parent reconstruction remain
  on misses. Snapshot-local reuse survives an external commit until transaction
  end; the next transaction invalidates. Every local write (including unrelated
  state changes and streaming checkpoints) conservatively invalidates. Oversized
  lineages are returned uncached. This improves stable-history browsing, not
  constant-time cursor lookup or every paging call during a live stream.
- F03: new public Storage `MemorySourceMessageIdentity { id: MessageId,
  role: MessageRole }`; `list_branch_memory_source_identities(&self,
  conversation_id: &ConversationId, branch_id: &ConversationBranchId) ->
  CoreResult<Vec<MemorySourceMessageIdentity>>`; and
  `list_branch_memory_source(&self, conversation_id: &ConversationId,
  branch_id: &ConversationBranchId, start: &MessageId, end: &MessageId) ->
  CoreResult<Vec<Message>>`. Root owns explicit facade/manifest updates. There
  is no Core re-export of this Storage view. Current-head manual enqueue uses
  compact cadence identities, then selected bodies. Historical lifecycle enqueue
  preserves its bounded exact-head prompt reader. Worker source revalidation uses
  the current-branch range API and retains immutable source SHA validation.
  Range count513 sentinel/byte preflight in the same transaction ensures at most
  512 messages/4MiB enter the selected-body sort; characters retain1,048,576 cap.
  All lineage IDs, roles/status/dates and SQLite content types are checked.
  Off-range TEXT bytes are not Rust UTF-8-decoded by the new range contract;
  selected-body and legacy full-history APIs keep their respective boundaries.
  Manual cadence still scans text for Unicode emptiness, but never retains an
  all-history body Vec. SQL trim's set is exhaustively matched to pinned Rust
  is_whitespace; embedded NUL is nonempty. Core's historical-only private reader
  was renamed to make the selected authority explicit.
- PERF02: knowledge rows become a bounded owned snapshot, then the connection
  guard ends before SHA/finite/norm/dot scoring and sort. Existing64MiB work meter,
  four stored-vector passes and10,000-row bound remain; vector payload admission
  is therefore below16MiB. This deliberately trades bounded owned vector memory
  for shorter shared DB lock time, not lower total RSS. Snapshot row errors are
  retained in original order so earlier hash corruption still precedes a later
  decode/budget failure. No skipped candidate or changed floating accumulation.
- F12/PERF07: Unix tree traversal still checks every path and rejects links/special
  types, but chmod runs only when all permission/special bits differ from exact
  0600/0700. No persistent permission cache or skipped traversal.

Shared parent edits: database.rs module/field/type re-export, bootstrap cache
initialization, lib.rs explicit identity re-export and public contract manifest.
Owned source paths: database/message_pages.rs(+existing tests), lineage_cache.rs
(+tests), memory_source.rs(+tests), private_path.rs(+tests), lifecycle_outbox.rs
and private claim child/tests, memory_queue.rs and private claim SQL/tests,
knowledge_embedding.rs and private snapshot/tests, Core auxiliary_tasks/summary.rs.
The later authorized checkpoint implementation and schema-fixture paths are listed
in the PERF08 section. No asset file or applied historical migration was edited here.

## Recorded validation evidence

Before change: page7, durable queue claim1, interaction-derived12, knowledge
storage3 and private permissions1 targeted tests passed. A full Storage lib run
at the first integrated implementation passed321 with1 existing manual ignored;
later refinements are checked again with their relevant targets. Core
memory_auxiliary_fallback3 passed through new worker/manual source paths.

Synthetic SQL operation counts (Python SQLite3.53.2; not native latency):
10,000 completed memory jobs+30 queued:8,126,554→272,208 VM steps;1,000+30:
836,554→29,208. queued0:58→73 steps;10,000+1:270,164→270,236. The small fixed
cost is explicit. Lifecycle100,000 acknowledged after ANALYZE:500,068→76;
new Rust regression verifies zero full-scan steps with the real0019 table/index.

New tests check row-for-row eligibility parity for current/legacy retry payloads,
rate/concurrency/null/missing-profile/attempt limits and a>5x VM-work improvement;
cache pointer/validation-count reuse and external/local mutation invalidation;
2,000-message page count/order/membership and source body limits; Unicode trim,
NUL, oversized off-range body, invalid off-range metadata, reversed/empty source;
earlier vector corruption versus later snapshot failure; and chmod operation
counts (0 for unchanged private tree,2 for two widened/special-bit paths).

Deliberately rejected: approximate lineage totals, LIMITed membership validation,
UNION ALL cycle weakening, retention deletion, changing synchronous/WAL policy,
using content||delta as a claim of reduced SQLite prefix rewrite, and claiming
constant paging work or lower RSS without evidence. The initial read-only PERF08
review was superseded by explicit approval and implementation of migration 0043
and the bounded proof cache described below; no schema decision remains pending.

## PERF08 approved scope and implemented contract

Parent accepted the measured delta-journal solution and authorized0043 plus
Storage read overlay, triggers, recovery, compatibility checkpoint and schema
registration/cutpoint tests. Root owns Core forwarder and manifest integration;
frontend agent independently inventoried pending-read consumers. This is an
explicit schema/API feature extension of FINAL-EFFICIENCY-20260912.

New public Rust-only API: append_pending_assistant_checkpoint(&self,
message_id: &MessageId, generation_id: &GenerationId, expected_bytes: u64,
delta: &str) -> CoreResult<u64>. Normal calls CAS against exact durable bytes;
an exact last committed offset+delta replay is accepted, conflicting/out-of-order
payloads fail. Existing full-prefix checkpoint_pending_assistant retains its
replacement semantics. No arbitrary new total-message cap is introduced.

0043 introduces generation-bound pending checkpoint state and ordered UTF-8
chunks, with offset/sequence/length/owner guards and lifecycle cleanup. Existing
message content is the base. A read-only messages_with_checkpoints view supplies
base+ordered chunks before SQL content/length filtering, and rejects incomplete
journal structure rather than exposing a shorter successful prefix. Content
replacement, terminal transitions and deletion clear journal rows atomically.

Targets: new database/pending_checkpoints.rs children;0043 migration;
schema.rs/migration_registry.rs/migration_runner.rs; messages.rs checkpoint read
consumers (parent checkpoint region coordination), message_pages selected and
assistant body reads; interrupted_generation_recovery assistant hydration and
legacy preserve-partial bulk transition; latest schema/cutpoint tests. Existing
finalize/compensate full-body writes rely on cleanup triggers and keep their
transaction boundaries. Terminal pair reads, complete-only source/prompt reads
and ancestry identity CTEs stay on canonical messages. No conversation export
reader was found; current content_export is asset/source export, not chat export.
Any future conversation serializer must consume effective message content.

Durability remains the same500ms/64KiB checkpoint cadence with FULL; no fsync
batching or weaker recovery. Recovery must hydrate before deciding whether a
partial exists; legacy pending preserve must copy effective content before a
status-trigger cleanup. Regression coverage targets Unicode split boundaries, exact replay, gaps,
stale generation/terminal writes, live pending page/legacy prompt bounds,
full-prefix replacement, finalization/compensation/deletion, crash reopen,
legacy preserve/drop, fresh43/upgrades/reopen and unchanged frozen fixtures.

Synthetic SQLite3.53.2 prototype, auto-checkpoint disabled only to count WAL
frames, FULL retained: final1MiB at64KiB×16 checkpoints gives2,223 frames for
prefixUPDATE and content||delta alike, versus306 for chunk inserts. At4KiB×256,
33,663 versus1,058. The prototype excludes full production journal watermark/
validation overhead; it demonstrates the mechanism, not final product speed or
normal WAL file retention. Script/results are in the temporary audit directory.

Earlier validation refinement: new vector error-precedence test initially
compared randomized CoreError operation_id values as well as semantic fields;
it now compares code/message/recoverable. An earlier rerun was blocked by shared
target disk exhaustion; parent subsequently cleared generated cache and resumed
combined verification. The earlier broad database run passed82 with1existing
ignored; Core memory_auxiliary_fallback3 passed. At that stage other-shard asset
lints remained while this slice's helper/test lints were corrected. Final combined
test and lint status is maintained by parent.

### PERF08 implementation and measured limitations

Migration 0043 stores an exact owner/base/total/sequence watermark and immutable
UTF-8 chunks of at most 64 KiB. The old full-prefix checkpoint and final writes
replace canonical content and clear chunks in the same transaction. The optional
append API preserves Immediate transactions and accepts only exact current-offset
appends or identical replay ending at the current watermark. No smaller total
Storage body cap was introduced. Pending-body reads use `messages_with_checkpoints`
so existing SQL byte/character filters see the effective content; a corrupt journal
raises an SQL error before those filters. Legacy preserved recovery materializes
that content before changing status. Normal completed-message readers remain on
canonical storage.

The exact new migration triggers and append validation SQL were measured with a
synthetic SQLite 3.53.2 database, WAL/FULL and autocheckpoint disabled solely to count
emitted frames (`/tmp/lorepia-final-audit-20260912/storage/checkpoint_actual_probe.py`).
For a final 1 MiB body, 16 × 64 KiB flushes emit 1,396,712 bytes / 339 frames versus
9,158,792 bytes / 2,223 frames for full-prefix replacement (6.56× reduction).
256 × 4 KiB flushes emit 6,464,312 bytes / 1,569 frames versus 138,691,592 bytes /
33,663 frames (21.45× reduction). These are synthetic write-amplification counts,
not user-database size or latency claims; the isolated fixture omits unrelated
production indexes. The equivalent `content || delta` update emitted the same WAL
as full-prefix replacement and is not considered an amplification fix.

Before the proof-cache follow-up, every append validated all chunk metadata. That
uncached implementation incurred O(k²) cumulative metadata work for k chunks: 9,429 total VM instructions (5,351
validation) at 16 chunks, and 1,285,389 (1,222,031 validation) at 256 chunks. SQLite VM
instruction counts do not include the cost of copying large content values, so
this is an explicit CPU tradeoff, not evidence of constant-cost appends. A tail-only
check would miss an externally deleted middle chunk. Avoiding repeated validation
safely required the snapshot/connection-change-bound proof cache now implemented
below. Full validation remains the fallback when its exact proof cannot be reused. Full content reconstruction is confined
to effective reads and the exceptional replay path, while normal writes append only
the new bytes and watermark. This does not claim all streaming work becomes linear.

Checkpoint regression coverage includes Unicode chunk boundaries/NUL, exact replay/conflict,
pending list/branch/page/prompt limits, canonical replacement/late append, missing
chunks before length filtering, and legacy preserved closure. Existing schema
cutpoint and previous-release inverse fixtures now include/remove exactly 0043.
Python verified all 43 migrations, the test fixture, and 43 removal/reapplication.
Rust tests and clippy are delegated to the parent's combined gate after disk-cache
cleanup; no extra concurrent Cargo build was started by this slice.

### Implemented bounded checkpoint validation proofs

Parent approved eliminating repeated metadata validation only when an exact
snapshot proof is reusable. New private pending_checkpoints/proof_cache.rs owns
at most 32 entries and 64 KiB of message/generation ID bytes. Epoch consists of
local total_changes and external PRAGMA data_version captured inside the Immediate
transaction after load_state establishes its snapshot. Exact owner/base/total/next
must match. Unknown state, any unrelated local write, or external commit falls back
to the complete existing validator. Only successful own commits advance the epoch
and establish a new exact proof; failed transactions cannot establish proofs. Root
owns Storage field/bootstrap wiring. No additional schema/API contract change.
Regression targets: repeated hits, local/external invalidation, failure/rollback,
oversized IDs and eviction. Earlier O(k²) numbers remain the cold-validation cost.

The follow-up is implemented: normal consecutive appends now reuse that exact
proof and perform no whole-journal metadata scan. Synthetic SQL runs including the
per-append data_version and next_sequence reads require 4,437 VM steps for 16
flushes and 68,037 for 256 flushes (versus uncached 9,429 and 1,285,389). WAL counts
are unchanged. The first validation contributes 71 VM steps in each empty-base
fixture. `/tmp/lorepia-final-audit-20260912/storage/checkpoint-cached-probe.json`
records this controlled cache-hit workload. Unrelated local/external writes or
uncacheable IDs retain the measured cold fallback; no full-corruption check is
removed. Added regression coverage asserts 20 appends require one validation,
rollback/unrelated write force revalidation, and an external middle-chunk deletion
fails the next append. Separate cache tests cover exact owner/epoch and finite
entry/ID-byte eviction. These supersede the earlier decision to defer the proof
cache; the uncached numbers remain useful fallback measurements.

### Hard-crash fixture representation follow-up

Combined Core tests exposed the pre-0043 test helper
`app/tests/generation_fixtures.rs::hard_crash_assistant_content`: it inspected only
canonical `messages.content` immediately after the child forcibly exits, omitting
the now-durable journal suffix. That one test-only SELECT now reads the effective
view. The launch/reopen policy assertions, exact expected partial content, actual
Core reopen/recovery, and lifecycle assertions remain unchanged. No production
recovery change or weakened expectation was needed. Targeted verification includes
`hard_crash_recovery_uses_durable_checkpoint_instead_of_reopen_setting` plus
`interrupted_generation_recovery_rolls_back_on_outbox_conflict`; parent owns the
final combined verification.

### Exact schema-36 discovery fixture follow-up

Final fixture-only follow-up: update the post-upgrade schema assertion in Core credential_recovery to 43; preserve the exact schema-36 inverse and audit multiline latest-schema assertions before editing.

The combined gate also found `provider_discovery/schema_fixture.rs` explicitly
undoing 42 through 38 without removing new migration 43. Before that existing
inverse, the fixture now removes exactly the 0043 view, attached cleanup triggers
and journal tables using shared test-only drop SQL, then requires deletion of its
one registry row. Schema-36 version, integrity, foreign-key and physical-authority
assertions remain unchanged. This is test-only schema representation maintenance; no production or
historical migration changes. Repository-wide latest-42/inverse-registry searches
found this remaining omission; the Storage latest/cutpoint fixtures were already
updated and explicit migration-42 historical references remain intentional.

The first seven checkpoint Rust regressions produced five passes and two fixture
errors: raw fixture timestamps used `Z` while save_message's exact identity compares
canonical `+00:00`, and direct deletion still had a referencing branch head.
Canonicalized fixture timestamps and detached the branch head before the deletion
case. Production identity/FK checks and all expected checkpoint assertions remain
unchanged. The independently passing tests already exercised effective reads,
Unicode/replay, corruption, recovery, and proof invalidation/bounds.
