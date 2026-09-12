# UX responsiveness implementation record

- Task ID: `UX-RESPONSIVENESS-20260912` (user-authorized UX behavior improvements).
- Baseline / merge-base: `aac3b16e0697c710b1209f498caba71821746140`.
- Branch: `codex/ux-responsiveness-20260912`.
- Worktree: `/Users/codexer/.codex/worktrees/lorepia-ux-20260912`.
- Starting state: 28 inherited modified/untracked files copied from
  `codex/chat-room-surfaces-20260912`; original checkout left intact. Baseline
  hashes, original files and patch: `/tmp/lorepia-ux-20260912/`.
- Public entries retained: `main.ts`, `WorkspaceApp.svelte`, root controller,
  shared typed IPC client, preview-only `preview/main.ts`.
- Targets: workspace navigation/composition/projection, conversation loading,
  settings data loading, library and transcript loading presentation, message
  composition feedback, profile back-button contrast, relevant colocated tests,
  and isolated preview QA.
- Symbols retained: controller selection/start methods, message/character
  projection functions, scroll owner and message identity. No code-only
  extraction or unrelated renaming is planned.
- Owned invariants: latest navigation wins; leaving a screen cannot navigate
  back on completion; stale/disposed results cannot publish; durable operations
  stay under existing Core authority; send waits for authoritative branch
  state; drafts, focus, scroll anchoring, runtime grants and sanitizer unchanged.
- No IPC, schema, native security boundary, dependencies or historical
  refactoring inventories are changed.
- Expected delta: several focused helpers and regression test files, roughly
  300–650 production lines net, without cap increases or giant-file exceptions.
- Risks: early navigation exposing false empty states or invalid send actions,
  stale greeting metadata, background errors being lost, hidden settings
  loading too early, projection caches retaining mutable/stale data, draft
  loss after delayed acknowledgements, and scroll identity churn.
- Governing material: root/frontend `AGENTS.md`, ADR 0001/0004/0005/0006,
  existing `toss-ui-resize-performance.md`, and the current UX request.
- Baseline tests: 347 passed; three existing failures in
  `WorkspaceNavigation.test.ts` and `WorkspaceSettings.integration.test.ts`
  still expect settings moved by the inherited chat-room work.
- Planned checks: deferred-response navigation/bootstrap/error races; request
  counts for hidden settings; stable streaming projections; draft preservation;
  closest component tests; full frontend check/tests/build; architecture, IPC
  and archive gates; browser clicks with intentionally slow preview reads.

Research, final findings and verification are recorded in
`toss-ux-responsiveness-audit.md`.

## Initial bundle follow-up

The first production build emits a 998.47 kB entry JavaScript chunk (251.09 kB
gzip), including settings editors unused on the home screen. Add a literal
dynamic-import boundary for `WorkspaceFeatures.svelte`, retaining its props,
controller ownership and mounted state after loading. A local loading/error
panel must appear immediately; failure is retryable and completion after close
must not remount a closed panel. This is part of the same UX feature task, not
a code-only extraction. Validate bundle output, settings navigation/focus and
frontend contracts without increasing size or dependency caps.

## Completion and concurrent work

- New UX strings use the existing `ko-workspace.ts` catalog because `ko.ts`
  is already at its byte cap. No size baseline was raised.
- Frontend: 758 tests in 138 files passed; check/build passed. Python repository
  tests: 102 passed. IPC, architecture, i18n, context, archive, workflow and Rust
  format checks passed. Full native/platform CI remains a pre-merge requirement.
- The original checkout continued changing during this task. Its later edits
  were not overwritten or absorbed into this tested snapshot. In particular,
  later chat-room changes overlap some inherited preview/test paths.
- `/tmp/lorepia-ux-20260912/ux-responsiveness.patch` isolates this task from the
  initial inherited snapshot. Apply it to that snapshot or reconcile later
  changes before applying; it is not a blind patch for the moving original tree.
- Changes remain uncommitted in the dedicated branch/worktree for review.
