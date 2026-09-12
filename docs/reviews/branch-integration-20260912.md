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
