# Paged history compatibility in portable runtime

Task DEEP-EFFICIENCY-20260912. Worktree
`/Users/codexer/.codex/worktrees/lorepia-ux-20260912`, dirty baseline HEAD
`aac3b16e0697c710b1209f498caba71821746140`; snapshot under
`/tmp/lorepia-deep-efficiency-20260912/baseline-files`. Existing asset and resource
work is preserved. Frontend agent handed over portable-runtime-context.ts and
its tests after its serialization/suffix optimizations; those changes remain.

## Before editing

Targets: portable-runtime host/lifecycle/kernel/context/protocol and related
private tests under apps/lorepia/src/features/chat; a small window helper if
needed for the existing host source-size cap. Parent owns Core/Shell/IPC paging;
frontend agent owns controllers/UI and lifecycle construction metadata hookup.
Root/frontend AGENTS were read. Worker methods remain private application
protocol; no backend IPC/schema/dependency change.

Preserve complete-array legacy behavior when window metadata is absent. A
partial array is not deletion authority: retain older messageOverrides while
keeping existing persisted-state limits. Full arrays retain existing pruning
semantics. Worker context remains 128 messages/512 KiB, including metadata.
Global chat indices, editDisplay callback index, negative chat indexing and
chat_index/lastmessageid must use page offsets, not point at another message.
No extra capabilities or model-history access is granted for absent messages.

Affected symbols: setMessages/afterOutput/workerContext, lifecycle creation and
syncMessages, boundedPortableRuntimeChatContext, worker context guard and kernel
chat access/display/macro methods. Tests: portable-runtime context/protocol,
paging behavior through in-process worker, lifecycle and existing runtime suite.
Node 24.18.1. Host is already at 1,585-line legacy cap; move only the narrow
pruning operation to a helper and keep parent size under baseline. Kernel 854
lines remains under 900; other files remain small. No cap increases.

Risks: accidental override deletion during eviction, stale asynchronous metadata,
local/global index confusion, worker context budget metadata overhead, and
virtual pending-user indexing. Independent tests should distinguish omitted
older messages from authoritative full-history deletion.

## Implemented behavior

The lifecycle accepts optional currentMessageWindow metadata, and the host passes
it with setMessages/afterOutput into the worker context. Context selection
adjusts start_index for its own retained suffix and counts metadata in the
512 KiB context budget. Legacy contexts omit the field entirely.

The kernel interprets getChat/setChat/removeChat indices against total history,
then resolves only a present message or the virtual pending user. Missing older
messages return nil/false instead of selecting a different local-array row.
getChatLength and last-index macros use total history; editDisplay receives
start_index plus local index. getFullChat remains the existing bounded content
view: this does not make evicted history available to scripts or model calls.

The narrow prune helper only removes overrides for a complete history. Partial
metadata cannot distinguish deletion from eviction, so it is not deletion
authority; no new deletion tombstone or backend contract is invented. Persisted
state key/byte limits remain unchanged. Complete legacy arrays still prune.

Measured parent host size after formatting: 1,582 lines / 61,982 bytes versus
1,585 / 62,010 baseline; kernel 863 lines. No baseline increases. Existing
context serialization optimizations from the frontend agent are retained.

## Validation

- Before editing: 4 focused context/protocol/lifecycle/import-compat files,
  14 tests passed.
- After editing: 5 focused files including new paging test, 17 tests passed.
- Existing runtime, worker client and capability tests: 38 passed.
- Lua timeout sandbox and regex worker posttests passed.
- Targeted ESLint, Prettier and diff whitespace checks passed.
- TypeScript check reported only an in-progress frontend-owned history test
  argument error; the owning agent was notified. No runtime-slice errors reported.
- Shared source architecture check found no portable-runtime violations, but
  failed on in-progress pagination IPC/Storage facade size and public API
  contract updates outside this slice. Parent was notified and owns integration/full gates.

New tests exercise global indices 198/199, negative and missing-index reads and
writes, virtual message index 200, callback/macro indices, partial eviction
retention and full-history deletion pruning. Context tests verify suffix offset
872 from a 200-message input starting at 800, including the metadata byte budget.
Protocol tests reject negative, fractional, overflowing and out-of-range starts.

## Final independent read-only pagination review

Inspected WorkspaceApp metadata wiring, LiveChatSession, lifecycle, context and
protocol budgets, projection, ChatTranscript and ChatMessage. No code edits or
tests were performed in this review. Findings sent to parent/frontend owner:

1. Partial retention also preserves genuinely deleted override IDs: onRemoved
   only increments the runtime reset epoch, then the same branch's persisted
   state is loaded again. Repeating override-last-message/delete/append on a
   branch that remains longer than 128 messages can accumulate 256 stale keys
   and exhaust the existing override-key limit. This is bounded accumulation,
   not an unbounded leak. Explicit confirmed deletion identity is needed to
   distinguish this path from viewport eviction.
2. The bounded context helper counts window metadata but does not reject the
   case where metadata alone exceeds 512 KiB. An oversized typed head ID is a
   counterexample; normal native IDs are short, so no production trigger was
   established. The separate 8 MiB worker budget does not imply the 512 KiB
   context budget. Reject oversized metadata before returning a context.
3. Existing lifecycle creation checks its epoch before await refreshDisplay,
   but not again before publishing runtime/ready afterward. A delayed completion
   after cleanup can publish a stale candidate in an abstract deferred model;
   normal close usually rejects pending work, so a production race was not
   reproduced. A post-await scope check is the narrow defensive repair.

UI mutation callbacks bind the concrete message ID, not its local index. UI
indices add the page and virtual-window offsets. Runtime access maps global
indices only to present rows and returns nil/false outside the window. No
wrong-row mutation counterexample was found in those paths. Worker/persisted
node and byte guards remain unchanged, aside from metadata finding above.
Latest runtime data and older UI history use separate collections; the ready
fallback allows a complete latest tail while unrelated branch metadata is
still loading. The frontend owner also identified and is fixing raw-store
reactivity in the window callback to avoid token-by-token worker refreshes.

## Approved narrow review repairs

Parent authorized repairs for findings 2 and 3. Before production edits: retain
all previous dirty changes; own only context metadata preflight and lifecycle
post-refresh scope check, plus their existing tests. Frontend owner handles
confirmed deletion cleanup separately. No portable-runtime.ts/window/resetScope
changes in this follow-up. Expected additions under 10 production lines per
small file. Preserve worker/context budgets and scope ownership; regression
tests must fail on the reviewed implementation first, then pass after repairs.

Findings 2 and 3 are now repaired. Metadata serialization failure or metadata
alone exceeding the existing 512 KiB context budget throws before constructing
a worker context. Lifecycle creation rechecks scope after refreshDisplay and
closes a stale candidate instead of publishing runtime/ready. No protocol,
secret, node, worker byte, or persisted-state limit changed.

Both new regression cases failed before the production repairs (recorded in
/tmp/lorepia-deep-efficiency-20260912/runtime-review-regressions-before.txt).
The lifecycle case completes a replacement runtime first, then resolves the
old runtime's deferred display refresh and drains the promise continuation;
it verifies the replacement remains published and the old runtime is closed.
After repairs, context/lifecycle/paging/protocol passed 18 tests in four files;
Lua timeout and regex worker sandbox regressions also passed. Scoped ESLint,
Prettier and diff whitespace validation passed. Full integration gates remain
with the parent task. Finding 1 is being handled by the frontend owner.

## Card surface global index follow-up

Parent authorized a narrow lastMessageIndex getter correction after review found
CardRoomSurface passed local latest-128 length as global macro indices. Same
DEEP-EFFICIENCY-20260912 baseline and dirty worktree apply. Target only the
lifecycle getter and an independent CardRoomSurface test; preserve capability
checks, legacy complete-array behavior, and exclusion of the live assistant.
No symbols move; expected production delta under 10 lines. Test a 2,000-message
window through the actual portable card iframe and both index macros.

The real CardRoomSurface → PortableMessage iframe regression first failed with
127/127 (or 126/126 while the live assistant is excluded) instead of global
1999/1999 (1998/1998). The getter now uses total_messages and preserves the
current/display exclusion count; legacy complete windows keep their old rule.
The two card cases and three lifecycle cases pass, together with Lua/regex
sandbox post-tests and scoped ESLint/Prettier. Deletion cleanup remains a
separate frontend-owned implementation being independently reviewed.

## Deletion cleanup independent final review

Read the frontend owner's final lifecycle/window/runtime/WorkspaceApp cleanup
and tests without modifying those paths. Confirmed concrete IDs are selected
from bounded authoritative membership; missing membership, request failure,
changed response head, replaced marker or changed current scope/head preserve
overrides. The current-scope/head post-await guard was added after review.
Worker close invalidates its version before cleanup; persistence retains the
existing scoped revision/epoch and serialized write drain. Active cleanup waits
for persistence before reset, while an initial-runtime marker closes its
candidate and cleans it before a replacement can be published. The frontend
owner reports 17 targeted tests, ESLint and Svelte checks passing; inspected
deferred proof/drain and stale-head tests plus pending-initial replacement test.
No additional confirmed deletion-cleanup counterexample was found in this
bounded read-only review. The direct runtime persistence test uses local
storage; SQLite uses the existing unchanged scoped drain path.

A separate preserved-display issue remains under parent-owned follow-up:
lastCharacterMessage previously searched complete display history. If all of
the most recent 128 messages are user/system, an earlier last assistant is now
missing from CardRoomSurface and lastcharmessage macros. Preserving this UI
aggregate requires the latest non-live assistant's ID and content (ID applies
existing overrides); other DTO fields are not consumed by that helper. No
aggregate API change was made in this review.

## Approved last-assistant UI aggregate repair

Parent explicitly approved a bounded last-assistant page aggregate and frontend
wiring under the same Task ID. This slice owns only lifecycle's optional
currentLastAssistantMessage callback and independent card regressions. Prefer a
non-live assistant already in displayMessages; otherwise use the aggregate's
ID/content with existing runtime override lookup. Reject a fallback excluded
from the display tail as live. No worker/window messages, limits or grants are
expanded; production delta expected under 15 lines. Storage and frontend owners
implement the approved page field and callback wiring separately.

The lifecycle callback is implemented. A new actual CardRoomSurface iframe test
first failed for an older assistant beyond the 128-message tail, then passed
with its ID-based override; separate cases verify an in-tail assistant wins and
a live assistant excluded from display is not reintroduced by the aggregate.
These three cases plus card-index and lifecycle regressions pass 10 tests, with
Lua/regex sandbox post-tests and scoped ESLint/Prettier passing. The current
and worker message arrays remain 128 entries. Backend/frontend aggregate owners
were informed that a returned live assistant cannot stand in for the previous
non-live assistant; that snapshot/query detail remains in their wiring scope.
