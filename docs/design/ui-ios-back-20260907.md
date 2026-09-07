# Interactive back motion

- Task ID: `UI-IOS-BACK-20260907`.
- Baseline/merge-base: `82a77caed61713109dfd8dc86b36aafee3b834d6`.
- Worktree: prior UI changes are dirty; starting hashes and preview copies are
  recorded in `/tmp/lorepia-ios-back-20260907/` before editing.
- Request: give back navigation an iOS 26 feel in the current Tauri UI preview.
  Preserve the connected three-page layout, orange flat appearance, composer
  behavior, input drafts, and the existing asymmetric native window sizing.
- Targets/entries: preview `swipePages`, `edgeBack`, `UiPreview`, back controls in
  settings/editor/organizer, motion styles, and focused motion/gesture tests.
  Add bounded local motion helpers. No public API/symbol moves, Rust, IPC, data,
  dependency, schema or production renderer changes.
- Owned invariants: drag tracks the pointer without interpolation; release can
  finish or cancel; renewed gestures pick up the visible position; vertical
  scrolling and editable selections retain priority; reduced motion, resize,
  focus restoration and teardown remain correct. No decorative shadows or blur.
- Relevant tests: preview gesture, layout motion, responsive, editor morph and
  component tests; native and browser checks after the targeted frontend gate.
- Expected delta: approximately 250–500 lines including focused tests. No source
  baseline increase or exception. Risks: transformed-page geometry, interruption
  races, double outros, underlay cleanup and breakpoint changes during a gesture.
- Governing material: root/frontend AGENTS, ADR 0001, prior accepted UI frame.
  Apple WWDC25, “Build a UIKit app with the new design”, describes interruptible
  navigation slide transitions and content-area back gestures in iOS 26:
  https://developer.apple.com/videos/play/wwdc2025/284/
  The implementation adapts those interaction principles to the Svelte preview;
  it does not add UIKit or claim pixel-identical native system behavior.

## Result

- Net preview delta: +313 lines, including focused motion/gesture tests.
- Shared damped settling now uses remaining distance and release speed. The
  previous page follows at 28% of the departing page's movement. Back buttons
  use the same departure path; an interrupted editor/settings departure can
  resume from its visible position without a second outro.
- Real WebKit inspection found that native text tracking could consume all
  pointer moves after an accepted editor-edge pointerdown. Reserve that edge
  before text tracking starts; editable interiors retain native selection and
  controls retain normal clicks. Temporary diagnostic UI was removed.
- Browser: management/chat navigation, settings swipe, fullscreen close with
  multiline draft retention, no leftover underlay translation, 320px/393px
  mobile and 1200px wide layout. Wide page widths settle at 360/552/288 with
  no document overflow or residual depth transform; browser error log empty.
- Native Tauri: chat rightward swipe, settings back button, fullscreen back
  button and editor-edge rightward swipe. The user's existing conversation and
  unsent draft were restored after rebuilding and remain open in the app.
- Preview tests: 103 passed across 10 files; portable post-tests passed.
  Frontend check passed with the same two pre-existing `ChatPane.svelte`
  captured-state warnings. Native build, tests and lint passed as recorded in
  `ui-phone-aspect-20260907.md`.
- Architecture output matches the starting 75 pre-existing diagnostics. No
  source-size baseline increase, production renderer edits or dependency changes.
- Physical iOS/Android device gestures and keyboards were not exercised here;
  native runtime verification used macOS Tauri/WebKit.
