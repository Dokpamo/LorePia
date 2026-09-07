# Phone proportions and navigation alignment

- Task ID: `UI-PHONE-FRAME-20260906`.
- Baseline / merge-base: `82a77caed61713109dfd8dc86b36aafee3b834d6`.
- Starting worktree: 83 modified/untracked files; hashes captured in
  `/tmp/lorepia-ui-phone-20260906/worktree-before.json`.
- Request: preserve smartphone proportions at narrow widths, remove square
  outlines on pointer clicks, and align leading/back controls with the supplied
  Toss screenshots.
- Targets: preview-local viewport geometry, `UiPreview.svelte`, page headers,
  local tokens/layout styles and focused tests. Keep the `ui-preview.html`
  entry and existing native bundle. Window-vs-canvas proportion behavior was
  asked as an optional clarification; work on focus/header alignment proceeds
  independently.
- Invariants: orange flat UI; three connected pages; 3/2/1-pane navigation;
  non-restarting responsive motion; 48px logical hit targets; conversation,
  draft, form, scroll and focus state; keyboard focus indication and reduced
  motion. No new dependencies, native IPC, DTOs or capability changes.
- Retained symbols: page navigation, sample models, `responsiveLayout`,
  `blendLayout`, and `swipePages`. No unrelated moves or renames.
- Expected delta: about 100–160 lines across bounded geometry and tests, plus
  small header/style changes. No architecture baseline increase.
- Risks: confusing native-window and canvas proportions, keyboard resize,
  abrupt framing at thresholds, scaled pointer coordinates, hidden keyboard
  focus, or accidentally reducing icon hit targets.
- Baseline checks: 57 UI tests plus Lua/regex regressions pass. Architecture
  records the existing 75 diagnostics.
- Verification: focused geometry, continuity and focus-state cases; frontend
  check; architecture/IPC checks; browser screenshots, coordinates and pointer
  vs keyboard outlines; then build and reopen the native UI bundle.
- Governing material: root/frontend `AGENTS.md`, ADR 0001, accepted preview and
  motion notes, current request and user-provided light/dark screenshots.

## Reference observations

At 393px logical width, the old back hit target began at (20,12) with a 48px
square target, putting its center at (44,36). The reference arrow is nearer the
top-left, approximately (29,30) at the same width. Use a 60px toolbar, 4px leading
hit-target inset and 48px target: center (28,30). Back SVGs use a 28px box.
Detail-page titles belong below the back toolbar, as in the certificate image.

The browser reproduces a 2px orange `:focus-visible` outline after pointer-clicking
the character-name input. Keep focus itself and restoration; make the visible
ring depend on keyboard navigation modality, so pointer focus stays flat.

## Phone framing choice

No clarification had arrived while the independent fixes were completed, so
the stated default is a phone-proportioned UI canvas within a freely resizable
native window. Below 480px, use a 9:19.5 frame contained and centered in the
available space. Between 480px and the existing 760px one-pane boundary, blend
continuously back to the full viewport. No frame border or shadow is added.
Keep the existing 360px minimum logical width and uniform scaling. Wide and
split layouts fill the window as before; no native window sizing is changed.

## Verification results

- 64 focused preview tests pass, including contained/centered phone frames,
  continuity at 480px and 760px, responsive state, and focus restoration.
  Portable Lua and regex regressions also pass.
- Frontend formatting, lint, i18n and type checks pass with the two existing
  `ChatPane.svelte` warnings. IPC generation check passes. Architecture output
  is byte-for-byte unchanged from the baseline (75 existing diagnostics).
- Browser checks at 320, 393, 480, 600, 900 and 1280px show no horizontal
  overflow. The narrow frame stays at 9:19.5; split/wide layouts fill the window.
  Light and dark themes were viewed. Browser console has no warnings/errors.
- At an unscaled phone frame, the back target is 48 by 48px with center (28,30),
  its SVG box is 28px, and the detail title begins at y=92px. Coordinates are
  relative to the UI canvas; a short desktop window adds centered outer space.
- Pointer-clicked text input has `outline-style: none`; Tab navigation restores
  the 2px orange keyboard indicator. Native WebKit input was also clicked and
  visually checked after rebuilding the app.
- The macOS debug UI bundle built successfully and was reopened. No unsent
  native message was present before restart. Native border dragging was not
  automated or intercepted; the existing freely resizable shell is preserved.
- Worktree hash comparison confines this task to 12 preview files and this
  note. Existing native/backend and other worktree changes are preserved.

## Follow-up: fill the window

- Task ID: `UI-FILL-VIEWPORT-20260906`.
- Baseline / merge-base: `82a77caed61713109dfd8dc86b36aafee3b834d6`.
- Starting state: 84 modified/untracked files, captured in
  `/tmp/lorepia-ui-fill-20260906/worktree-before.json`.
- The user rejected the outer margins caused by the phone frame. This request
  supersedes the centered 9:19.5 canvas choice above: all four UI edges must
  meet the available viewport, including short and tall windows.
- Targets: `responsive-layout.ts`, `UiPreview.svelte`, `ui-responsive.css`, and
  their existing geometry/responsive tests. Retain `ui-preview.html` and its
  native bundle as entry points; no public contract or native change.
- Remove the preview-local `viewportFrame` helper and its centering offsets.
  Keep `responsiveLayout`, `blendLayout`, the 360px minimum logical width and
  uniform control scaling, existing page motion, keyboard focus and back-button
  alignment. No code movement, unrelated renaming or dependency changes.
- Expected delta: a net reduction of approximately 40 lines. Risks are stale
  inline sizes after resizing and cropping the composer under uniform scaling.
- Checks: focused preview tests, frontend check, unchanged architecture
  diagnostics, browser edge/composer measurements, and native rebuild/reopen.
- Governing material: root/frontend `AGENTS.md`, the preceding UI task notes,
  and the latest user correction. No architecture baseline increase.
- Additional user direction: apply the back button's sizing/alignment rule to
  other controls. Extend targets to preview-local button/layout/content tokens:
  all action icons use 28px boxes and 48px targets; header centers share y=30;
  outer control targets use the same 4px inset. Align rail controls, trailing
  header controls, group actions and send. Keep decorative icons separate.
  The rail uses a 56px control lane with centered 48px targets and 32px card
  thumbnails. No page order or interaction behavior changes.
- Result: the UI now fills the viewport at 320×552, 320×640, 393×748, 393×900,
  480×700, 600×500, 900×748 and 1280×800. Browser measurements show zero space
  at all four outer edges, no horizontal overflow, the composer at the bottom,
  and the draft retained through changes. Controls scale uniformly below 360px.
- All measured action buttons have 48px logical targets and 28px SVG boxes.
  App settings and card-add share x=28; top controls share y=30. The rightmost
  top control and send share the 4px outer target inset; history add/settings
  share the same inset relative to their group. The composer input is 48px tall.
- Validation: 64 preview tests and portable Lua/regex tests pass. Frontend
  checks pass with the two existing ChatPane warnings; IPC passes; architecture
  output matches the previous task exactly. Light/dark views were checked and
  the browser console has no warnings/errors. Native debug bundle rebuilt and
  reopened; native screenshot confirms no phone-frame margins. Changes are
  confined to eight preview files and this note.

## Follow-up: use the remaining height for history

- Task ID: `UI-HISTORY-FILL-20260906`.
- Baseline / merge-base: `82a77caed61713109dfd8dc86b36aafee3b834d6`.
- Starting state: 84 modified/untracked files; hashes recorded in
  `/tmp/lorepia-ui-history-fill-20260906/worktree-before.json`.
- The user's appshot points to the large unused space below the history group,
  not the viewport bounds. At a 360×896 viewport the group ends at y=524,
  leaving 372px unused below it. The prior outer-edge check missed this.
- Target: `ui-layout.css` through the existing `CharacterPage.svelte` and
  `ui-preview.html` entries. Make the left body and history group flexible;
  the group consumes the remaining height and the history list scrolls within
  it. Keep a usable minimum list height and outer scrolling as a fallback for
  very long character descriptions or very short viewports.
- Retain all component symbols, page order, button sizes/positions, row density,
  draft and selection state, focus, and existing swipe/navigation motion. No
  native, IPC, data, dependency or public contract changes; no code movement.
- Expected delta: about 15 CSS lines. Risks: flex shrinking rows, inaccessible
  last history, clipping long descriptions, or losing scroll/focus behavior.
- Checks: existing preview tests and frontend check; architecture size check;
  browser long/short viewport measurements, history overflow and keyboard
  access, then native build and visual confirmation. No baseline increase.
- Governing material: root/frontend `AGENTS.md`, preceding UI notes, and the
  user's current appshot and correction.
- Result: at 360×896, the history group now ends at y=876 instead of y=524;
  the 372px unused area becomes the same 20px inset used at the sides. The
  group also keeps this inset at 393×748, 900×748 and 1280×800, without changing
  the 80px row height. On 320px windows the inset scales uniformly with the UI.
- Browser QA added five sample histories through the UI (eight total). At
  320×552 only the inner list scrolls; the profile and add button remain in
  position. Tab reaches the final history fully in view, with its focus ring
  intact. A long description retains a usable minimum list and outer scroll
  fallback; its new-chat action remains reachable. Light/dark views pass and
  the browser console has no warnings/errors.
- 64 preview tests plus portable Lua/regex regressions pass. Frontend checks
  pass with the two existing ChatPane warnings; architecture output is
  unchanged. The native debug UI bundle rebuilt and was reopened; its screenshot
  confirms the history group extends down to the bottom inset. Only
  `ui-layout.css` and this note changed in this task.
