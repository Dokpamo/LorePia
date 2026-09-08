# Library search page — 2026-09-08

The later sort-row presentation and current root geometry are recorded in
`library-sort-menu-20260908.md`; the search flow below remains unchanged.

Task ID: UI-LIBRARY-SEARCH-PAGE-20260908. Baseline commit: `2200173`; worktree
contains the uncommitted four-tab redesign and the verified Home/Chats parity
update. Preserve that work. The preceding checks passed: frontend check (two
existing ChatPane warnings), six focused navigation/library tests, architecture
and the native build.

Requested change: remove persistent search fields from both library roots and
put a Lucide search action beside the existing add action. Search opens a full
page with a back control and the same 44px input in one header row. Removing
the 44px field and 12px layout gap raises the root chips/results by 56px.

Targets: bounded `ui/navigation` library components and styles, root detail
visibility wiring in WorkspaceApp, and the closest integration tests. Root
titles, result-rendering snippets, search input and gesture primitives remain
the public/internal entry points. No dependency, Rust, IPC or schema changes.
Expected net delta: 200–300 lines including tests and this record.

Invariants: root filters/sorts/scroll stay mounted; a hidden search query never
filters the root; each library retains its own search query; search results
open the existing character/chat detail flow and Back returns through search;
closing search restores focus to its trigger. Native assets and navigation
authority stay under their existing owners. The existing edgeBack/pageSlide
primitives provide transitions and swipe-back, with a local underlay container
so search cannot translate its own ancestor.

Sources: https://seed-design.io/components/top-navigation (root action slots,
standard back slot and history-based return for search), and the previously
reviewed SEED spacing scale. Search uses two root actions, preserving the
existing icon size and touch targets.

Verification: focused search/open/back/filter/draft integration flows;
frontend and architecture checks; browser mobile/desktop geometry, keyboard
focus, empty results, reduced motion and swipe-back; native build and inspection.

## Verified result

The shared LibraryScreen owns local search visibility and trigger focus. Root
filters and results remain mounted beneath it, and the frame hides the bottom
navigation while search is open. Search uses the existing pageSlide/edgeBack
motion with a local underlay. Escape excludes surfaces hidden by their ancestor,
so returning from a chat reveals its search page before the library root.

At 390px wide, the root chip surface moved from y132 to y76 and sorting from
y184 to y128. At 320/430/1280px, both roots also match and move up exactly 56px.
The search input stays 44px high; the search page fills the 808px test viewport
at every width. Browser checks passed for focus restoration, cleared/empty
results, swipe-back and reduced motion. A character search → information →
search → root round trip retained the query and the root's 100px scroll offset.
The hidden root keeps its bottom scroll clearance, avoiding clamping when the
tab bar disappears.

Eleven focused library/navigation tests passed. Frontend checks report zero
errors and only the two pre-existing ChatPane warnings; architecture passed.
Geometry and screenshots are in `/tmp/lorepia-library-layout/`.

Native verification exposed WKWebView scrolling the page horizontally when an
offscreen input received focus during its entering transition. LibraryScreen now
focuses the input on intro completion, guarded against covered/closed pages;
the local frame clips transitioning content. The rebuilt macOS app was inspected
with the input focused and the page correctly occupying the full window.
