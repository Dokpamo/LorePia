# Library header and icon consistency — 2026-09-08

Task ID: UI-LIBRARY-HEADER-ICONS-20260908. Baseline `2200173`, main merge-base
`6f22761a4305443541452e7409639114f4c10dfd`. Preserve the uncommitted four-tab
redesign. The preceding nine navigation/library tests, frontend check and
architecture checker passed (two existing ChatPane warnings).

Requested behavior: Home and Chats remove the chip rows and place sort after
the add action in the header: Search, Plus, EllipsisVertical. The menu keeps
real sorting. All app icons use black in light mode and white in dark mode,
with a consistent 2.25px stroke independent of the SVG viewport size.

Targets/entries: CharacterLibrary, ConversationLibrary, LibrarySort, their
two obsolete filter components and associated CSS; shared workspace icon
tokens/styles; navigation tab colors; WorkspaceFourTabs integration tests.
Expected delta: about -100 source lines. No Rust, IPC, schema, dependency,
controller authority or sorting-helper changes. Remove only filter presentation
and its local state. Keep search pages, queries, sort state, choice-sheet focus,
back navigation and native approved assets under their existing owners.

Icon policy: define one paint rule on Lucide SVG geometry. This takes precedence
over legacy embedded feature stroke/color values inherited from their SVG
container, without changing unrelated feature layout. Apply non-scaling-stroke
to the actual shapes because vector-effect is not inherited. Do not target
user images or arbitrary SVG. Filled selected-tab paths keep their existing
cutouts; primary action surfaces keep a dark contrasting icon in both themes.
Disabled opacity and normal press feedback remain available.

Risks/verification: header fit at 320px, all chats visible after removing the
character filter, saved-date/title sorting and search/back focus preserved;
actual icon color and shape stroke values across four roots and nested panels,
light/dark themes, mobile/desktop sizes, native WKWebView. Run the existing
navigation/library tests and frontend/architecture checks, then rebuild and
inspect the native app.

Sources: https://lucide.dev/icons/ellipsis-vertical and
https://developer.mozilla.org/en-US/docs/Web/SVG/Reference/Attribute/vector-effect.
The installed Lucide component calculates absoluteStrokeWidth from its size
prop; our dimensions are responsive CSS, so use SVG non-scaling strokes instead.

## Verified result

Both libraries now place the Lucide EllipsisVertical action immediately after
Plus in the header. The chip components, local character-filter state and
obsolete filter CSS were removed. Chats still shows every saved conversation;
date/title sorting and independent search/sort state pass the updated tests.
At 390px wide, root content begins at y72, exactly 12px after the 60px header.
All three header actions share the same target and icon size.

The common Lucide shape rule paints black in light mode and white in dark
mode, with 2.25px non-scaling strokes. The separate primary-surface token
keeps icons black on pale pistachio in either theme. Existing secondary text,
disabled opacity, selected-tab fills/cutouts and press feedback are preserved.

Browser verification passed at 320/390/430/1280px in both themes for all four
roots, sort sheets and search pages: color, shape stroke, SVG identity, action
order, no chip remnants, no horizontal overflow, sort selection, keyboard
focus restoration and search/back. Pixel coverage measurements of Plus at
23/28/30.86/34px SVG sizes all yielded 2.251 CSS px after antialiasing.

Fourteen existing library/navigation/settings tests passed. Frontend check
and architecture passed; only the two known ChatPane warnings remain. The
native build succeeded and the relaunched app was visually inspected on both
Home and Chats; the new header menu opened and restored focus when closed.

Evidence: `/tmp/lorepia-header-icons/measurements.json`, `pixel-strokes.json`,
the screenshots beside them, and `/tmp/lorepia-header-icons-*.txt` logs.
