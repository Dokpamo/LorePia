# Deep efficiency: frontend slice

- Task ID: DEEP-EFFICIENCY-20260912.
- HEAD: aac3b16e0697c710b1209f498caba71821746140. Dirty inherited UX/resource work is preserved; parent baseline snapshot is /tmp/lorepia-deep-efficiency-20260912/baseline-files.
- Governing guidance: root and frontend AGENTS.md, ADR 0001/0005. Only internal CPU/allocation changes; no public API, DTO, dependency or policy change.
- Targets/entry points: boundedPortableRuntimeChatContext and portableRuntimeChatContextSource in portable-runtime-context.ts; ProviderWorkspaceLoader.load in provider-workspace-loader.ts. Existing caller signatures remain unchanged.
- Symbols: change private fitMessageSuffix result to carry its measured JSON byte count; retain message ordering, UTF-16 suffix boundaries and strict byte budget. Build lore text from required suffix only. Build previous provider comparison maps only when refreshCredentials=false.
- Owned invariants: byte-for-byte context/source outputs; surrogate and JSON escaping accounting; unchanged context count/byte/worker budgets; no guard removal; provider credential requests, settings and credential epochs remain unchanged; no cross-request validation cache.
- Tests: portable-runtime-context.test.ts, app-controller.provider-loading.test.ts; add serialization call counts and bounded-source differential cases. Focused Vitest uses --maxWorkers=2. Parent runs full check/suite.
- Expected size: small additions within existing source caps, no baseline increases or giant exceptions; no public entry movement.
- Risks: an internal byte count could be associated with the wrong truncated candidate; reverse suffix assembly could drop empty-message separators or change zero/fractional/negative slice behavior; skipping unused provider comparisons must not skip credential refresh.
- Before-edit baseline tests are recorded below.

## Additional pre-edit stream target

- ChatStreamController.acceptStreamItem/reconcile/detachStream has a write-only reconcileBufferedItems array. Every item queued while disposal/reconciliation waits is subsequently discarded; no reader/replay exists. Remove this unbounded retention while preserving the immediate-return behavior and authoritative snapshot/new subscription recovery.
- Target entry/symbols: ChatStreamController.acceptStreamItem; delete the private buffer field, its pushes and clears only. Epoch, sequence verifier, delta flush, subscription and reconciliation paths stay unchanged.
- Regression: hold stream disposal pending, inject 1,000 stale large deltas, verify they are neither retained nor applied, then finish reconciliation and check recovered state. Existing chat-stream lifecycle tests cover reattachment/races.

## Bounded history before-edit record

- Authorized expanded targets: app state/controller message loading and reconciliation; new MessageHistoryController; LiveChatSession/WorkspaceApp/ConversationProjection/ChatPage/ChatTranscript history integration and colocated tests.
- Existing behavior: listBranchMessages loads full branch into state; UI DOM virtualization caps 80 elements with overscan 8, but all text/identity/layout arrays remain in memory. No backend history prefetch exists.
- New contract provided by parent: listBranchMessagesPage, exclusive before/after anchors, max 128 messages; chronological messages plus has_older/has_newer/head_message_id/total_messages/start_index.
- Proposed ownership: messages.items retains only latest 128 messages for runtime, with metadata; initial latest 30 can paint while phase is loading. Separate UI history retains at most 90 messages (three pages of 30), fetching before/after near edges and returning to latest on demand. Local DOM virtualization/anchor logic remains intact.
- Runtime partial-window/global-index/override changes are delegated to asset_resource_audit; page IPC/type/backend changes are parent/Storage work. Metadata-free clients preserve existing complete-list behavior for compatibility.
- Risks: branch/head races, empty/prepended-page anchor restoration, page evictions mistaken for deletion, global macro indices, action targets in older history. All asynchronous publication uses scope/request epochs; only complete runtime context becomes ready.

- Completed early checks: context/provider baseline 17 tests and stream baseline 18 tests passed. Context/provider optimized 21 tests passed, including 128 encoder calls for 128 retained messages and 1,911 join/slice equivalence cases. Lua/regex posttests passed.

## Implemented history behavior and resource evidence

- Live initial conversation read requests 30 messages first (15 ordinary user/assistant pairs); it can render immediately. The independent latest runtime context then requests at most 128. If the first page has no older history, there is no second request. An obsolete initial request cannot trigger the second query after its scope becomes stale.
- All controller full-branch reload sites now use the bounded recent loader. Metadata follows returned message arrays through a weak map and is copied into app message state. Terminal pair reconciliation trims the retained latest context to 128 and updates absolute offsets/counts.
- Transcript history begins with 30 messages, requests another 30 within 640px of either retained edge, and keeps at most 90 persisted messages. The existing 80-element DOM cap/overscan and scroll anchoring still apply. Evicted message components/iframes unmount. The latest action loads the latest page before scrolling; absolute macro indices and ARIA positions use backend offsets.
- History owns one outstanding page request, coalesces repeated latest requests, checks scope epochs and head identities, preserves the old window on error and offers explicit retry. Automatic scroll prefetch pauses after errors, preventing per-scroll failed-request storms.
- Long-history fixture: 2,000 messages. Initial window 1,970..1,999; runtime context 1,872..1,999. Ten older fetches retain only 1,670..1,759; one newer fetch moves to 1,700..1,789; latest returns 1,970..1,999. No full-branch call is used by the live paged path.
- Tests cover initial page before delayed context completion, both directions, 90-message retention, stale branch responses, changed heads, latest request coalescing (20 clicks → one queued latest request), retry, and disposal. Root WorkspaceApp integration covers initial 30/absolute positions, scrolling to three older pages, and latest. Existing scroll scaling/virtual-window regression tests remain passing.
- A write-only retired-stream reconciliation queue was removed: 1,000 incoming 8KiB stale payloads are neither retained nor applied while disposal is delayed. Recovery still reads authoritative state and uses the replacement receiver.
- Partial-runtime metadata/global indices/override retention are implemented by the supporting runtime agent; WorkspaceApp supplies currentMessageWindow and gates initial runtime creation/sync until a complete bounded context is available.
- Verification during development: 33 controller/projection/stream/history tests; 30 UI/provider/preview/history tests; and 16 history/scroll tests passed. Focused TypeScript/Svelte check passed with zero errors/warnings. Parent owns the final full suite. No baseline increases, dependency changes or commits.
- Final scoped history/preview/controller regression run: 6 files, 41 tests passed. Explicit failed-edge scroll regression confirms five further scroll events do not retry a failed page; latest remains available. Preview pagination verifies exclusive anchors, absolute offsets, limit validation and cloned return values.
- Runtime metadata and display-message callbacks now read stable derived fields rather than the whole raw app state, so unchanged metadata cannot retrigger runtime synchronization merely because streamed text changes.

## Explicit deletion override cleanup: before-edit record

- New confirmed counterexample: repeated override/delete/append on a long partial branch can leave deleted overrides until the 256-key cap. Eviction must still preserve older overrides.
- Authorized scope: WorkspaceApp/LiveChatSession successful-deletion callback; portable-runtime/lifecycle/window helper and tests; preview membership option. Parent extends the new page API with check_message_ids (max 256) and retained_message_ids in the same head snapshot.
- Cleanup requires successful same-scope deletion plus a complete, matching-head membership proof. Missing/failed membership preserves state. No full-text history reload or arbitrary prefix/suffix assumptions.
- Retire the current worker before the proof so old callbacks cannot restore deleted state. Wait for persistence drain before recreating. A deletion while creation is pending is checked before publishing its next runtime.
- Size plan: move unchanged private runtimeStateScopeEquals into the existing private window helper to free room under portable-runtime's fixed cap; add only targeted cleanup accessors/method. Preserve all binding comparisons exactly.
- Regression: 256 repeated retained override/delete cycles plus valid older override preservation, missing/mismatched proof rejection, success/failure/scope callback tests and lifecycle publication ordering.

### Deletion cleanup verification

- The active worker closes and its creation epoch is invalidated before the bounded membership lookup. Exact candidate IDs (at most 256) are checked with a one-message page; no full history content is fetched. Missing membership, lookup failure, an unexpected retained ID, or a changed response head preserves overrides.
- The lifecycle checks the deletion marker and a captured current scope/head predicate again after the lookup. The unchanged runtime writer drains persistence before recreation. If no runtime has been published yet, the next candidate is closed and cleaned before a replacement may be published.
- Regression evidence: 512 override/delete replacement cycles preserve an unseen valid older override without exhausting the 256-key cap; real Wasmoon runtime cleanup persists through reopening; deferred lifecycle persistence blocks reset/publication; stale current-head approval is rejected; an initial unpublished runtime is cleaned once before replacement. Preview membership retains input order/duplicates and rejects 257 IDs. Session cleanup runs only for committed same-scope deletion and retains the mutation lock until completion.
- `svelte-check`: zero errors/warnings. Scoped ESLint passes. Runtime remains within its existing 1585-line / 62010-byte cap (1585 lines / 62001 bytes). Asset-runtime owner cross-reviewed worker retirement, unchanged SQLite drain authority, pending creation, and current snapshot guards; no further confirmed deletion-path counterexample found.

### Last-assistant aggregate preservation

- Same DEEP-EFFICIENCY-20260912 baseline/dirty worktree; newly authorized additive page option is owned by parent/Storage. Target symbols: loadRecentBranchMessages, MessageWindowMetadata, app-state messages, WorkspaceApp stable runtime fallback callback, preview page fixture; lifecycle getter/test owned by runtime agent. Expected size: bounded optional metadata and one DTO reference, no worker or transcript array expansion.
- Counterexample: more than 128 trailing user/system messages hide the prior last assistant from the card-room aggregate that formerly searched the complete branch. Final 128-context request asks for that assistant; initial 30 stays unchanged, and complete short histories require no aggregate request. The response is carried separately to card display. Current visible assistant and live-message exclusion remain authoritative; runtime-agent validates the getter.
- Retained cost stays 128 runtime bodies and a 90-message transcript window plus at most one fallback body; no unbounded ancestry bodies or worker context expansion. Loader regression verifies the option only on the 128 request and exact separate metadata retention.
- Final aggregate validation: loader/history/root workspace 17 tests pass; preview aggregate and deterministic session cleanup 11 tests pass; scoped ESLint and Svelte check (zero errors/warnings) pass. Preview returns the optional body only outside the selected page, matching Storage. Session test waits for an explicit callback-entry promise instead of shared DOM polling/timers.
