# Direct manipulation in the Tauri UI preview

- Task ID: `UI-MOTION-20260906`.
- Baseline / merge-base: `82a77caed61713109dfd8dc86b36aafee3b834d6`.
- Starting worktree: prior UI changes are already modified/untracked; their
  hashes are recorded in `/tmp/lorepia-ui-motion-20260906/worktree-before.json`.
- Request: make page movement follow the user's hand and add subtle pressed
  feedback to buttons in the existing native UI prototype.
- Targets: `src/preview/ui/swipe-pages.ts`, its tests, `UiPreview.svelte`,
  `IconButton.svelte`, `CharacterPage.svelte`, `SettingsPage.svelte`, preview
  styles and `main.ts`. The separate `ui-preview.html` entry stays unchanged.
- Symbols: keep `swipePages`, `navigate`, component props and callbacks in
  place. No public symbols move or change contracts.
- Owned invariants: current three-page arrangement, orange/flat styling,
  fixed hit targets, vertical scrolling, editable text, keyboard access,
  optional creator page, inert offscreen content and local sample data.
- Expected delta: approximately 400–550 lines of gesture handling, focused
  tests and shared motion styles; no dependencies or native/IPC/schema changes.
- Risks: a drag firing a button click; stealing vertical scroll/text editing;
  incorrect edge navigation; stale pointer capture after cancellation/resize;
  reduced-motion preferences; WebKit pointer behavior.
- Tests: existing `UiPreview.test.ts` flows, new gesture lifecycle tests,
  frontend check, architecture delta, IPC generation and direct native use.
- Architecture baseline: 75 pre-existing Rust diagnostics; do not expand it.
- Governing material: root/frontend `AGENTS.md`, ADR 0001, accepted layout in
  `toss-ui-preview.md`, user-provided screenshots and the latest motion request.

Toss's [interaction design article](https://toss.tech/article/interaction)
describes clear action feedback and a shared motion vocabulary, including
pressed feedback. Timing, scale and swipe thresholds here are LorePia prototype
choices, not claimed Toss specifications.

## Implemented behavior

- Horizontal direction lock after 8px; direct pointer tracking with transition
  disabled during the gesture. Native vertical scrolling, pinch zoom, text
  editing and mouse transcript selection retain their own interactions.
- Release settles over 280ms. A short held drag returns, a sufficiently long
  drag or short flick advances one page, and reversing direction can cancel.
- Outer edges resist further dragging. Interrupted settlement resumes from
  its visible position. Pointer cancellation, capture loss, resize, blur,
  additional touches and action disposal release transient gesture state.
- A drag can start on a button without triggering that button on release.
  Normal taps and keyboard activation remain immediate.
- Press feedback uses 70ms in / 160ms out: icon visuals 90%, character cards
  96%, history text and form buttons 98%. Their hit areas stay fixed.
- Reduced motion removes automatic movement and scaling; pressed opacity
  still provides feedback. The surface revealed at an edge follows the chosen
  theme, including when it differs from the system theme.

## Verification — 2026-09-06

- 24 preview component/gesture tests passed, plus the existing Lua and regex
  worker regressions. Gesture tests cover tracking before release, distance,
  velocity, reversal, edge bounds, editing, scrolling, accidental clicks and
  cancellation/cleanup.
- Frontend formatting, i18n, lint and typecheck passed. The two pre-existing
  production `ChatPane.svelte` warnings remain unchanged.
- IPC generation passed. Architecture output is identical to the initial
  75-diagnostic Rust baseline, with no added diagnostics.
- Browser at 393px: verified page dragging, short-drag return, 48px icon hit
  areas, no button shadows and no console warnings/errors. Wide layout uses
  zero translation and no automatic transition.
- macOS Tauri/WebKit: verified management → chat → creator dragging, a short
  drag over a history button without opening it, and normal button navigation
  after dragging. Rebuilt the standalone debug bundle and left it open on chat.
- Only the planned preview files changed from the recorded worktree hashes.

Physical iOS/Android touch behavior has not been tested in this pass. Native
verification used mouse input in macOS; reduced-motion behavior was code-reviewed.
