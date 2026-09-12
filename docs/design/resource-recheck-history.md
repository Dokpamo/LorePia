# RESOURCE-RECHECK-20260912: coherent recent-history reuse

Baseline and authorization are in `resource-recheck-task.md`. Existing public
page methods stay available. No request field, command name, permission, schema
or dependency version changes are needed. The existing rusqlite `hooks` feature
is enabled explicitly for the coordinated lineage fix; its declaration contract
is updated with the code.

## Contract selected before editing

- Storage's bounded page includes verified display sidecars and an opaque
  `snapshot_token`. Read canonical bodies, pending checkpoint bodies, sidecar
  rows and diagnostics in the same established read transaction; release the DB
  guard before decoding/hashing the sidecars. No Core-assembled transaction.
- The token hashes a random Storage-instance identity, branch identity, all
  local changes, external data version and schema version. It is only equality
  evidence, never authorization or a cursor. Every DB write conservatively
  changes this history proof; the separately filtered lineage cache is not used
  as body/projection freshness evidence. Reopen produces a different identity.
- Core maps already verified sidecars to its own presentations. The Shell page
  DTO adds only `snapshot_token`; no raw counter, path, row or handle escapes.
- The UI paints the latest 30 immediately. A compatible peer with a snapshot
  token loads up to 98 immediately preceding messages. It combines them only
  when token, head, count, absolute adjacency and message linkage agree. Drift
  or an invalidated anchor triggers one fresh latest-128 read. Peers without a
  token keep the existing bounded fallback. The supplemental latest assistant
  stays out of the message window and is deduplicated against both halves.

## Symbol and test map

`Storage::list_branch_messages_page` owns page transaction/lineage selection.
`message_display_projection::loading::{read_batch,verify_batch}` retain their
SQL/hash logic and gain crate-private reuse; the generic projection reader
remains supported. `Core::present_messages` keeps the compatibility path and
shares mapping with the page path. `MessagePresentationPage`, `BranchMessagePage`
and the explicit public API inventory are updated for their reviewed fields.

Baseline: Storage `database::` (including image policy, page, lineage and memory
source tests); Shell `message_history`; frontend message-history controller.
Add regressions for unchanged 30+98 transfer, same-ID body/status/checkpoint and
projection changes, deletion/rewind, reopen, missing tokens and distant assistant
semantics. Test canonical/display consistency within a read snapshot. Expected
delta: focused page/helper additions under existing source caps; no code movement
outside this ownership, baseline increase or schema rewrite.

## Before edit: bounded last-assistant lookup follow-up

Asset agent owns only extraction of `message_pages::load_last_assistant` (about
58 lines) to private `message_pages/last_assistant.rs` and focused child tests.
Task/baseline/dirty shared worktree remain as above. Public page API and snapshot
transaction remain; caller passes its existing newest-first start index instead
of a boolean. All other page/anchor/token/sidecar logic stays with root.

If the selected page contains an assistant, the only possible supplemental latest
assistant is in the newer prefix `lineage[..start]`. A head page therefore needs
no query. Otherwise search the validated newest-first lineage in batches of128
identities, stopping at the first assistant, and read that one effective body.
No giant lineage JSON or full-branch sort. Preserve conversation binding, role,
chronological newest selection, duplicate suppression, same read transaction,
corrupt selected-body failure and sidecar verification. Expected parent reduction
~55 lines, small helper/test additions; no source-cap/public contract changes.
Root runs Cargo. Focused tests cover zero-query head pages, a30-ID prefix on a
100k lineage with a VM-work bound, chunk boundary/latest selection, and duplicate
suppression. Existing distant-assistant/page/snapshot suites remain applicable.
