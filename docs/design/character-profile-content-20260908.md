# Character profile content hierarchy — 2026-09-08

Task ID: UI-PROFILE-CONTENT-20260908. Baseline `c49a766`, merge-base
`6f22761a4305443541452e7409639114f4c10dfd`. Preserve the uncommitted profile
work from the preceding task. Its 15 targeted tests and architecture gate pass.

Move description out of the portrait: title → inline hashtag metadata → work
description → creator avatar/name → author note → guide → selectable starting
situations → included resources. Preserve 4:5 artwork, progressive lower blur,
fixed Start, navigation/focus, and existing chat setup. No schema, IPC, native
capability, dependency or controller-authority changes.

Owned entries/targets: CharacterOverview and bounded profile components/styles;
WorkspaceApp passes existing greeting state/selection and optional presentation
data; the existing renderer client reads the unscoped character render profile.
Preview-only presentation fixtures never enter native production defaults.
Expected source growth around 900 lines split among small components. Keep
all existing public client signatures and source-size caps. No symbol extraction.

Existing card content also includes personality, scenario, example dialogue,
system/post-history instructions, alternate greetings, transforms and runtime
variables. CharacterRenderProfileDto already exposes assets, runtime knowledge,
script metadata/source and transforms. Read that contract without a conversation
scope so a conversation override is not presented as base card content. Display
source text as inert text, never execute it. Assets continue through TrustedAsset.

Tags, creator identity/note, guide and greeting body have no current base-card
DTO. Use optional renderer presentation fields and isolated deterministic demo
fixtures; omit absent hashtag groups and show honest missing author/guide states.
Actual greeting selection uses the shared controller's IDs and survives opening
Start setup. No invented IDs, automatic chat creation, or silent selection drift.

SEED references: Tag Group (14px neutral inline metadata, consistent size),
Avatar/List (identity + label; whole-row targets), Radio (single selection),
Accordion (progressive disclosure of supporting detail). Use plain content
surfaces, a horizontal image gallery, named lore/script rows and collapsed
technical details. Main color remains reserved for Start.

- https://seed-design.io/components/tag-group
- https://seed-design.io/components/avatar
- https://seed-design.io/components/list
- https://seed-design.io/components/radio
- https://seed-design.io/components/accordion

Validate content order, absent metadata, two starting situations and retained
selection, scoped async results, resource drill-down/back, large text and narrow
viewports, light/dark, and native WKWebView. Run focused tests, lint/type/format,
architecture, browser measurements and native build after implementation.

Browser verification found the back-gesture recognizer capturing radio/label
pointerdown within its 32px edge and swallowing activation. Treat these native
controls like existing buttons/links: retain clicks until a drag is established.
Add the gesture handler and its regression tests to the target scope; keep drag
thresholds, settling, controller authority and navigation history unchanged.

The same focus check exposed retained offscreen pages scrolling the hidden root
sideways and the live announcement adding a 1px document scroll range. Clip the
fixed SEED app stack (page bodies still scroll) and anchor the visually hidden
status at the origin. Include seed-navigation.css and workspace.css for this
focus/layout correction. Check both footer axes after selecting an offscreen row.


## Result and verification

Implemented inline 14px hashtags without a visible group heading or pill surface;
16px work description; a 42px creator avatar with 12px name gap; author note below.
The 4:5 portrait retains only the 20–24px title in its progressively blurred base.
Guide and supporting resource groups start collapsed. Starting situations use
native single-selection semantics, a consistent custom 20px indicator, optional
preview text and the existing controller selection. Start remains a fixed 52px
primary action and opens the existing chat setup with that selection retained.

The resource gallery uses the card's registered assets, falling back to its cover
when the asset list is empty; it does not add the same cover again under a second
asset identity. Lorebook entries, script source and text transforms open inert
read-only details. Resource back navigation restores the disclosure and focus.
Main color remains reserved for Start; radio selection is neutral.

- Frontend check passes: format, i18n, ESLint and Svelte/TypeScript. Two existing
  controller-capture warnings remain in features/chat/ChatPane.svelte.
- Seven focused test files pass (34 tests), plus portable Lua/regex worker gates.
  A subsequent focused rerun of resource/gesture tests passes (13 tests).
- Architecture check passes against merge-base
  `6f22761a4305443541452e7409639114f4c10dfd`; no baselines were increased.
- Browser: light/dark at 320, 390 and 1280px widths. Hero measurements are
  320×400, 390×487.5 and 560×700. Tags/body measure 14/16px, creator avatar 42px,
  name gap 12px. No horizontal overflow. Footer coordinates remain unchanged on
  both axes when a lower starting situation is focused/selected after expanding
  the guide. Selection and expansion survive Start setup → Back.
- Native debug bundle builds and runs. On the existing image test card, the
  real unscoped render profile loads; the gallery opens an approved asset,
  resource Back restores the retained profile, and the final gallery count is
  one rather than the former duplicated cover count.
- Visually checked the filled browser example and the actual native profile.
  Large-text wrapping is provided by layout/CSS, but no separate OS text-size
  matrix was run in this change.

Browser evidence: `/tmp/lorepia-profile-detail/measurements.json` and the adjacent
light/dark screenshots. The current native bundle is
`target/debug/bundle/macos/LorePia UI.app`. Browser UI remains at
`http://127.0.0.1:5176/ui-preview.html`.

Native data limitations are explicit: tags, author identity/note, guide text and
starting-situation prose are currently browser presentation fixtures only. The
native greeting IDs/choice and resource data use existing backend contracts.
No Rust/schema/IPC changes, provider dispatches or data migrations were needed.
The earlier checkpoint commit remains unchanged; no new commit/push in this step.
