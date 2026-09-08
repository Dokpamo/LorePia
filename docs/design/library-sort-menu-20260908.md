# Library sort menu — 2026-09-08

Superseded presentation: `library-header-icons-20260908.md` records the user's
later removal of chips, vertical header menu and shared icon paint policy.

Task ID: UI-LIBRARY-SORT-MENU-20260908. Baseline: `2200173`; merge-base with
main: `6f22761a4305443541452e7409639114f4c10dfd`. The worktree contains the
uncommitted four-tab redesign and search-page work. Preserve those changes.
The two existing navigation/library test files pass all nine tests.

Requested change: replace the separate visible sort-label row with a Lucide
Ellipsis button at the right of the Home and Chats filter rows. The button
opens the existing sort choices. Search remains a root header action opening
the dedicated search page.

Targets: `LibrarySort.svelte` and `character-library.css`. Entry points remain
the shared LibrarySort bindings in CharacterLibraryFilters and
ConversationLibraryFilters. No symbols move; no dependency, Rust, IPC, schema
or sorting-algorithm changes. Expected source delta: approximately -20 lines.

Invariants: real saved-date/name sorting, independent root sort state, selected
choice state, keyboard operation, 44px touch targets, common press feedback,
focus restoration and horizontally scrollable chips. Align the sort button
with the right header action at mobile and desktop sizes. The main risk is
crowding the chip row at narrow widths; verify 320, 390, 430 and 1280px widths.

Sources: https://lucide.dev/icons/search and https://lucide.dev/icons/ellipsis;
the prior SEED spacing and navigation audit remains in
`library-search-page-20260908.md`. Installed @lucide/svelte 1.33.0 Search uses
the supplied reference's exact circle and handle paths. The app's existing
shared 2.25 stroke width remains intentional.

Verification: existing navigation/library tests, frontend and architecture
checks; browser sort/focus/search and geometry checks; native build and visual
inspection. No new test suite is needed for the presentation-only change.

## Verified result

Home and Chats now share one filter row with scrollable chips and a fixed
44×44px Ellipsis button containing the standard 24px icon. At 320, 390, 430
and 1280px widths, the button center matches the rightmost header action
within 0.02px. There is no document overflow and the last chip remains
reachable without covering the sort button. At 390px, content starts at
y124 instead of y180; the separate 44px sort row and its 12px gap are gone.

Browser checks verified selected options, real sort changes, Enter/Escape,
trigger focus restoration, and both dedicated 44px search pages. Nine
existing navigation/library tests passed, along with frontend and architecture
checks. Frontend checking reports only the two pre-existing ChatPane warnings.
The native build succeeded; in the rebuilt app the sort sheet opens, changing
the selection reorders the actual saved cards, and both root layouts match.
The app was left on Chats with Home's original newest-first order restored.

Measurements and screenshots: `/tmp/lorepia-library-layout/sort-menu-measurements.json`
and `*-sort-menu-*.png`. Verification logs: `/tmp/lorepia-sort-menu-*.txt`.
