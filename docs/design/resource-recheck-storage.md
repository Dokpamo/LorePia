# RESOURCE-RECHECK-20260912 — Storage slice

Before edits on 2026-09-12: baseline/HEAD
c999c43bafdb66f922d6e5a230a7dfd2c6c2cca7; branch
codex/final-resource-efficiency-20260912; merge-base main
e32c13c5b04c3d5db1b60637be115c2b61fcfb78. Initial source tree was clean;
parent task record untracked. Preserve all parallel edits. Governing material:
root/Storage/Core AGENTS, ADR0001–0006, facade/public API audit, parent
resource-recheck-task.md and supplied LorePia_recheck_c999c43.md RECHECK-05.

## Before-edit symbol and test map

Owned: database/lineage_cache.rs and child tests; database/message_pages.rs and
child tests; memory_source.rs only for the required private cache return/signature
adaptation. Public page and memory-source signatures remain supported unchanged.
Parent owns database.rs/bootstrap/facades/Cargo hooks feature and snapshot-token
contract. Shared change-tracking wiring requires agreement before edits.

Keep load_identities UNION traversal and exact parent reconstruction; move lookup
index into a private LineageSnapshot carrying identity Vec plus sorted Vec<usize>.
Replace page anchor linear position and retained_candidates full traversal with
binary lookup into that index. No duplicate String-key map. Include index retained
allocation in the existing 8 MiB admission accounting; cold peak is still not
bounded by that retained-cache cap. Cache misses preserve complete cycle, missing
parent, conversation membership, count and chronological ordering checks.

Baseline targets requested from parent (no competing Cargo process):
database::message_pages, database::lineage_cache, database::memory_source.
New tests: 100,000 identities and repeated checkpoint/unrelated-write/page
interleaving with validation counts; indexed first/middle/last/missing lookup;
parent mutation, delete, fork, external writer, rollback/ABA and eviction/oversize.
No elapsed-time assertions. Existing 2,000-message paging and source tests remain.
Expected size: focused private helper and tests, no giant cap increase; index
adds one usize per cached identity plus bounded struct/vector metadata.

## Proposed invalidation contract (shared wiring pending agreement)

Use a monotonic process-local relevant-write epoch for main.messages changes,
while retaining external PRAGMA data_version invalidation. Local checkpoint chunk
and unrelated settings writes must not change that epoch. Message content/status
updates may conservatively invalidate despite not changing ancestry. The counter
never rolls back with SQL, so rollback/ABA is invalidating; saturating at u64::MAX
must disable reuse rather than wrap. Load observes the epoch while holding the
connection and its established read transaction. Relevant DDL must invalidate too.

The bundled SQLite source documents update_hook exceptions: DELETE truncate does
not invoke row hooks (sqlite3.c:133225), and REPLACE deletions themselves do not
invoke them. REPLACE's resulting insert still invalidates. A narrowly scoped
SQLite authorizer returning Ignore for DELETE main.messages disables only truncate
optimization, preserving actual deletions and row hooks. DDL authorization can
conservatively bump the epoch; failed preparation/rollback may over-invalidate.
No callback may reenter SQLite. Parent coordinates the single installed callback
with full-content snapshot tokens. External connections remain conservatively
invalidating through data_version regardless of their hooks or affected tables.

## Implemented agreement and first validation

Parent approved `hooks`, assigned change_tracking.rs plus database/bootstrap wiring
to this slice, and retained Cargo/manifest ownership. Baseline database:: run passed
98 tests with one existing ignored test. The tracker installs one row-update hook
and one authorizer; it never reenters SQLite. All main.messages row writes bump a
saturating epoch; unrelated local writes do not. DDL/pragma preparation conservatively
bumps the epoch. Ordinary DELETE on messages uses Ignore solely to disable truncate;
SQLite's immediate DropTable/DropView/DropVtable→Delete authorization pair remains
Allow so DROP semantics are preserved. The first combined regression run exposed
and corrected that distinction rather than relaxing its DROP/recreate assertion.

Cache admission/reuse also requires SQLite transaction_state(main)==Read. A write
transaction may expose ancestry that a subsequent rollback restores without row
hooks; it is therefore fully validated but never cached. Misses check main.messages
is a rowid table with no temporary shadow via pragma_table_list. Local DDL epochs
or external data_version invalidate an old entry before this shape recheck;
unsupported schema stays uncached. No extra schema pragma is added on hot hits.
Tests include external WITHOUT ROWID replacement and temporary shadow mutation.

The parent-owned snapshot contract uses this tracker's per-instance random UUID
and domain-separated SHA-256 of full total_changes, data_version, schema_version
and branch identity. The helper runs under the established content transaction;
raw metadata never becomes a renderer token. Full-content tokens intentionally
change on checkpoint and unrelated writes even while ancestry proofs remain valid.
Parent added same-transaction display-sidecar reads in message_pages.rs after the
indexed anchor/membership handoff. Public body/status/display snapshot integration
is owned and verified by parent.

In the first updated database:: run, 102 tests passed, one authorizer DROP test
failed, and one existing test was ignored; the DROP fix and extra unsupported-schema
regression await the next parent run. The actual 100,000-message Storage workload
passed: 20 middle-page reads interleaved with durable append checkpoints and
unrelated conversation writes required one full lineage validation, with correct
absolute offsets and input-ordered duplicate-preserving membership. Earlier
whole-DB total_changes invalidation would force all 20 traversals. Binary lookup
now uses a sorted Vec<usize> (one index per ID) with no cloned-ID map; anchor work is
O(log N), candidate work O(k log N), and deterministic lineage order is unchanged.
The retained index is included in the unchanged 8 MiB admission budget. Cold work
still materializes complete identities and sorts the lookup index (O(N log N));
this does not claim an 8 MiB cold peak or remove single-entry/oversized-cache limits.

Owned final paths: database/change_tracking.rs and tests; lineage_cache.rs and
its tests/workload_tests; message_pages.rs anchor/membership portion (subsequently
parent-owned snapshot additions); memory_source.rs private cache adaptation;
database.rs module/field and bootstrap.rs installation. Parent coordinates the
source-size cap, complete gates, public contracts and hooks declaration inventory.
No Cargo process was started by this shard.

Parent revalidation after the DROP/read-state/schema-shape fixes passed all
2 change-tracking, 5 lineage (including the 100,000-message workload) and
10 page/last-assistant tests. The Shell's 9 page/snapshot tests also passed,
including concurrent canonical/display updates and fresh-instance token changes.
The explicit dependency declaration enables only rusqlite's existing `hooks`
feature, with no version change. database.rs's measured parent cap was reduced
from 871 lines/33,127 bytes to 867/33,084 after moving the private generation route
carrier to its own helper. Root's complete workspace gate is recorded separately.

Callback caveats were cross-checked against the pinned bundled SQLite source and
the [SQLite update hook](https://www.sqlite.org/c3ref/update_hook.html) and
[authorizer](https://www.sqlite.org/c3ref/set_authorizer.html) documentation.
