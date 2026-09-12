# Branch integration — 2026-09-12

Task: `INTEGRATE-BRANCHES-20260912`. The user requested merging all mergeable
branches into main and deleting the integrated branches.

Before integration, main/origin/main were `aac3b16e0697c710b1209f498caba71821746140`.
The two named work branches had uncommitted changes. Both working sets were
backed up, including file hashes, under `/tmp/lorepia-merge-main-20260912/` before
being committed as `c2bb9d6` (chat surfaces, 30 paths) and `7a956ec` (UX/resource
work, 158 paths). The older detached review worktree and index are preserved.

The detached dependency worktree trees match the squash commits for PRs #50/#51.
The other detached integration snapshots are ancestors of main. The older dirty
review snapshot is covered by the earlier comparison in
`main-integration-20260910.md`; it is not a new named branch to merge or delete.

## Conflict review

Eight overlapping files required explicit resolution. Preserve both changes:

- Keep bounded history, runtime global indices, suffix budgets and the portable
  asset selector from the performance branch.
- Keep the chat branch's card layer placement, shared grant controls, wrapped
  code blocks and pointer-capture handoff repair.
- Combine immediate first iframe hit-region publication with the performance
  branch's per-report computed-style cache. Preserve both regression cases.
- Preserve the chat branch's browser verification record.

No public symbol moves, schema/dependency changes or further API changes are
planned for integration. The additive history API and its explicit contract
update are inherited from the reviewed work branch. Root/frontend AGENTS and
existing ADR boundaries govern the conflict resolutions. Tests cover both card
startup with suspended animation frames and repeated cached-style reports,
swipe capture handoff, runtime controls and long-history paging. Expected source
growth is limited to the combined fixes and tests; size caps are not raised.

## Merge and cleanup policy

Main requires a linear history and ten GitHub checks. Use one reviewed squash
PR with both branch tips represented, wait for the required checks, then verify
the merged tree before deleting local/remote topic refs. Preserve any changed
head or unmerged work. Run the repository's full local pre-merge gate against
the combined tree; local results do not replace the protected GitHub checks.

## PR #53 review follow-up

Continue task `INTEGRATE-BRANCHES-20260912` from clean integration commit
`b1ff944a4d6d3e9fa39775206152ae12d8ac00ed`, with merge-base/main still
`aac3b16e0697c710b1209f498caba71821746140`. The full local gate and all ten
required GitHub checks passed, but two unresolved review threads require
checking text-only floating card surfaces and short, unscrollable history
pages before merging.

Targets are `portable-renderer-bridge.js` and its tests, plus
`ChatTranscript.svelte` and focused view-local history tests/helpers if needed.
Public entries, IPC contracts, dependencies and schema remain unchanged; no
symbols move. Preserve iframe clipping/sanitization, bounded hit regions,
per-report style caching, scroll anchoring, page/memory limits, request
deduplication, branch epochs and observer teardown. Expected growth is a small
local rendering fix and regression coverage within existing size caps. The
semantic risks are exposing oversized hit regions, redundant page requests,
and repeated loading after disposal or errors. Root/frontend AGENTS and the
existing ADR boundaries govern these fixes. Reproduce each report, run focused
tests, then frontend/architecture gates and required GitHub checks on the final
commit. The unchanged Rust tree retains the completed local workspace gate.

Browser follow-up also checks viewports large enough to fit the bounded window:
an explicit older-history action must remain available once automatic loading
stops at the memory cap. This adds `ChatPage.svelte` wiring and one entry in the
existing split pagination catalog to the same review task. It reuses the
controller's existing load/notice path and introduces no backend/API change.
Observe both the viewport and inner list with the same observer, so virtual
spacer corrections that reduce content height also trigger an underfill check.

Both review cases were reproduced before the fix. Direct text now contributes
line-sized Range bounds to the same clipped, capped region list. Underfilled
transcripts check after rendering and viewport/content resizing, suppress
repeated or non-growing automatic requests, and offer manual older navigation
outside the measured list. The existing controller retains race/error authority.

Follow-up validation: 152 frontend files / 820 tests passed, including Lua and
regex sandbox checks; format, lint, TypeScript/Svelte, production build, source
architecture and context budgets passed. Svelte reported zero errors/warnings.
Chrome QA used 2,000 synthetic messages: a 4,000px viewport filled without user
scroll; a 6,000px viewport retained at most 80 DOM messages and allowed another
older page through the manual action, then stopped requesting pages. Resizing
back to 900px hid that action. The actual frame document/bridge/layout path also
changed from an empty text-only clip to two visible line regions. The isolated
frame harness used a nonce-authorized data URL for the exact trusted script to
avoid Chrome's null-origin loopback restriction; production loading is unchanged.
Evidence and browser screenshots remain in the task's temporary backup folder.
