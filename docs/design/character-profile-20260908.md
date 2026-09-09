# Character profile — 2026-09-08

Task ID: UI-CHARACTER-PROFILE-20260908. Baseline `c49a766`, main merge-base
`6f22761a4305443541452e7409639114f4c10dfd`. Working tree clean after the
requested checkpoint commit. Existing architecture check passes.

Requested composition: edge-to-edge 4:5 portrait behind Back, with a sharp
title and description over a progressively blurred lower image. Continue with
tags, creator, creator note, guide and introduction. Keep Start fixed below the
scroll area. The 4:5 interpretation was stated to the user before implementation.

Targets: CharacterOverview, a bounded profile hero and profile stylesheet,
SettingsPanel's optional title/footer presentation, localized navigation copy,
and the closest profile/navigation tests. Public renderer entry remains
WorkspaceApp; no symbols are moved across feature ownership. Expected size
delta around +300 source lines; no baseline increases.

Invariants: retain SettingsPanel back/gesture/focus and covered-parent behavior,
the approved CharacterImage/TrustedAsset asset path, actual selected-character
name and description, and existing new-chat mode/greeting flow. Keep existing
management destinations reachable after the requested profile sections.
Rust, IPC, dependencies, schemas and shared controller authority are unchanged.

The current CharacterDto only exposes name, description, avatar and creation
time. Greeting catalogs contain IDs, not greeting bodies. Metadata sections
therefore show explicit empty presentation values; do not fabricate creator,
tags, note, guide or introduction from unrelated fields. Wiring these fields
requires a separately scoped renderer-safe contract change.

Risks/checks: fixed aspect ratio with long text, arbitrary portrait contrast,
safe-area footer and keyboard focus not obscuring content, small/large viewports,
light/dark themes, nested new-chat return and swipe Back, loaded image stability.
Run frontend check, targeted workspace tests and architecture checker; inspect
browser pixels and the rebuilt native WKWebView.

Design references:
- https://seed-design.io/components/image-frame — fixed aspect ratio and square
  edges when an image reaches the screen edges.
- https://seed-design.io/components/top-navigation — Back retains its placement
  and history behavior while overlaying an image.
- https://seed-design.io/components/scroll-fog — a gradient overlaps the scroll
  boundary without moving content; use it above the fixed action.

The lower portrait blur is the user's requested visual treatment. Text itself
is never filtered. A second view of the same approved asset is blurred and
masked over the lower portrait; its fade follows the measured summary height.
This avoids the masked backdrop-filter failing to blur in browser pixel QA.
Preserve shared Lucide stroke and button press behavior.

## Visual and interaction verification

Eight browser cases cover 320×640, 390×844, 430×932 and 1280×900, in both
light and dark themes. The image is exactly 4:5, begins behind Back, and uses
a maximum 560px reading column on desktop. The footer reaches the viewport
edge and retains a 52px action target while the body scrolls independently.
The profile scrollbar does not allocate a white gutter beside the portrait.

Long-title/description checks at 320, 390 and 1280px preserve the ratio and
keep the full-description action reachable. Both cancelled and committed
edge swipes work. Existing new-chat setup opens without creating a conversation,
then returns to the retained profile. Fifteen targeted workspace tests pass.

Pixel QA uses a browser-only diagonal pattern with the same tint gradient for
blur-on/off comparison: horizontal edge contrast above the title decreases
66.0% with blur enabled. The fixture is not product content. Native WKWebView
was rebuilt and inspected using its existing saved character image; only one
image is exposed to accessibility, and Start opens the existing chat setup.

Frontend format/i18n/lint/type checks and the source architecture gate pass.
The two existing ChatPane controller-capture warnings remain unchanged.

Evidence: `/tmp/lorepia-profile/measurements.json`, `blur-pixel-check.json`,
screenshots beside them, and `/tmp/lorepia-profile-*.txt` validation logs.
The five metadata sections remain honest empty values pending a scoped DTO
extension; no metadata or introduction text has been invented.
