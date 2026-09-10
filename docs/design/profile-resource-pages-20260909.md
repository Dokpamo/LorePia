# Profile resource pages and three-column grids — 2026-09-09

Feature UI-RESOURCE-PAGES-20260909. Baseline c49a766; preserve the existing
dirty profile, gallery, language reader and import work. No public API, schema,
dependency, data mutation or refactoring-only extraction.

Follow the user's latest correction: remove hero position dots and their space;
retain swipe/keyboard image navigation and original-image viewing. Home cards,
image-group covers and image thumbnails use three columns. Show three image
groups in the profile preview. Remove the "Included resources" heading.

Use SEED List (https://seed-design.io/components/list) anatomy with neutral,
full-row navigation targets and consistent 16px row padding. Lorebook and
Scripts each open a dedicated page. Keep text transforms under Scripts, with
their own heading, explanation and display/output scope. Script and rule counts
remain visible at the profile entry so transforms are discoverable.

Lists retain their mounted DOM and scroll when a detail page covers them;
back returns to the original row. Reuse existing page motion, inert coverage,
focus restoration, Lucide icons and text/code readers. Contents remain inert;
the pages do not execute scripts, regexes or prompt planning. Use only the
existing renderer-safe profile response and approved asset renderer.

Verify current-character response races, nested list/detail back and focus,
script/rule separation, carousel controls after dot removal and native grid
layout. Run frontend checks, focused tests, source architecture and UI build.

Verification completed on the native 394px-wide LorePia UI window using the
already-imported card. Home cards, profile image-group previews, the full group
page and album thumbnails show three columns. Hero dots are absent and swipe
navigation still works. Lorebook and Scripts each open their own list; reading
an entry and returning preserves the parent page and its scroll position.
Script source and transform scope/pattern/replacement render as inert text.

Native inspection caught long imported keys and patterns overflowing the list.
Resource-row secondary text now wraps within the row and clamps to two lines;
the separate reader keeps the complete content. The rebuilt app has no
horizontal overflow in the script list.

Frontend formatting, i18n, lint and Svelte checks passed (two existing
ChatPane controller-capture warnings). All 13 tests across the six affected
profile/hero/gallery/album/reader suites passed across the focused runs, as did
the portable Lua and regex sandbox regressions. Source architecture and
`git diff --check` passed. The native UI build completed and was relaunched.
