# Opening reader and language variants — 2026-09-09

Task UI-OPENING-READER-20260909, feature change. Baseline c49a766; merge base
6f22761a4305443541452e7409639114f4c10dfd. Preserve the existing dirty profile,
gallery, import compatibility and demo work. No refactoring-only extraction.

Make the neutral full-description action full width with centered contents.
Opening rows navigate to a reader rather than changing a radio selection.
Keep parent pages mounted/covered to preserve scroll, focus and back gestures.
The reader's bottom Start action selects the exact greeting ID and opens the
existing conversation setup; reading or switching languages never starts chat.

References: https://seed-design.io/react/components/list,
https://seed-design.io/react/components/segmented-control,
https://seed-design.io/components/action-button. Use our existing spacing,
neutral ghost rows, Lucide chevrons and pistachio primary CTA; no dependencies.

## Additive read contract (ADR 0006)

Add `get_character_greeting_detail` to Shell API/native IPC and its exact grants.
Request: character_id, character_content_revision_id, greeting_id. Response:
the same identifiers and the full text of that one immutable opening. A changed
revision, absent/empty opening or noncanonical ID fails closed. The read is
bounded to 256 KiB; oversized content returns the existing resource-limit error
instead of truncating the scene. No schema, Core/Storage API, generation,
credential, script or transaction changes. The selector catalog still exposes
only IDs; the reader response is never a conversation-creation input.

Render-profile greeting previews add default-null `language` and `group_id`.
Grouping is a display heuristic, not author-supplied translation metadata.
Scan at most 8192 characters and 64 media anchors per opening. Infer language
from script, then pair different known languages only with mutually unique
matching scene anchors: identical substantial opening preambles or matching
image sequences with at least two shared references for partial matches.
Ambiguous candidates remain separate. Group IDs use the first existing ID in
source order. Never infer equivalence from adjacency alone or merge source text.
No real user file/content is checked into fixtures; tests use synthetic scenes.

Targets: Shell API DTO/library, strict Tauri request/command/registry and both
capabilities, typed client, profile list/reader/view/style/i18n, preview client,
focused tests. Expected delta: small bounded modules, no size cap increase.
Risks: false grouping, stale revision/language responses, unintended runtime
execution, wrong opening selection, covered-page navigation/focus regressions.
Verify these plus native imported-card reading, exact-language setup, browser
checks, Shell API/native tests, IPC generation, formatting and architecture.

## Image navigation follow-up

The user's follow-up adds a representative-image hero carousel (cover first,
then optional declared presentation IDs or folder covers), position dots, and
a full-width View all images action after two folder previews. Full gallery
is folders → thumbnail grid → uncropped original-asset viewer. Keep list/grid
mounted for back focus and scroll. The standalone viewer owns horizontal pans
and downward dismissal; it never installs the normal horizontal back gesture.
Image rendering remains on TrustedAsset, with adjacent images only mounted in
carousels and lazy thumbnail resolution. No new image, upload, lore binding or
author metadata is invented, and no source image is modified.

## Verification

Final frontend check: no errors, with the two pre-existing ChatPane controller
capture warnings. Twelve targeted files pass (41 tests); the default-opening
translation case also passes after the last grouping adjustment. The full
Shell API suite passes (113 tests); the three final language-grouping tests,
Shell API Clippy, 25 existing native contract tests plus the new strict reader
request test, IPC generation, source architecture and Rust formatting pass.
No baseline limits were raised. Native UI build succeeds.

Read the already imported card in the native app: 40 translated alternates
become 20 scene groups, plus the default opening (21 displayed entries).
Verified the full Korean and English scene text and each language's image,
selection of the exact English variant in existing chat setup, then return to
the same reader. The source preamble differs by terminal punctuation; grouping
normalizes that display-only anchor without modifying stored text.

Verified the cover-first carousel/dots, full-width centered read action, folder
index, thumbnail grid, uncropped original asset, left/right drag in the viewer
without back navigation, downward drag returning to the grid/profile, and
focus returning to the original thumbnail. Native data was not reimported, no
conversation was created and no model or imported script was executed.

Grouping uses evidence from source anchors; ambiguous or short/unidentified
variants remain separate. Optional authored hero presentation IDs take priority
over folder representatives after the cover. No separate imported hero metadata
was supplied by this card, so it uses folder representatives.
