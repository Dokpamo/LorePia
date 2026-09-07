# Management layout and composer correction

- Task ID: `UI-MANAGEMENT-COMPOSER-20260907`.
- Baseline/merge-base: `82a77caed61713109dfd8dc86b36aafee3b834d6`.
- Worktree: existing UI work is dirty. Snapshot of 1,574 file hashes and copies
  of starting preview sources: `/tmp/lorepia-ui-management-20260907/`.
- Request: use the supplied Discord screenshot for the management page while
  retaining the connected management/chat/optional creator frame. Subsequent
  steering prioritizes restoring the original inline composer, two-line
  fullscreen eligibility, growth through ten lines, and rightward back gestures.
- Targets/entry: `src/preview/ui/`, its local locale catalog, `ui-preview.html`.
  Existing native window resize behavior, live renderer, Rust, IPC and storage
  remain unchanged. No public symbol moves, dependency or contract changes.
- Owned invariants: orange flat surfaces, shared spacing and 48px controls,
  retained conversation drafts and IME safety, focused fullscreen field editing,
  smooth measured transitions, reduced motion, native vertical scrolling,
  keyboard/text selection alternatives, folder operations and existing navigation.
- Symbol/test inventory: `CharacterPage`, `CharacterRail`, `MessageComposer`,
  `TextEditor`, `swipePages`, `edgeBack`; preview component, resize, gesture and
  morph tests. Add a bounded composer measurement helper and behavioral tests.
- Expected delta: roughly 300–650 lines including tests; no size cap increases.
- Risks: resizing/textarea measurement feedback, caret scrolling during growth,
  two-line eligibility versus ordinary focus expansion, gesture/selection
  arbitration, scrolling long history lists, focus restoration after filtering.
- Governing material: root/frontend AGENTS, ADR 0001, prior accepted flat/orange
  frame and provided screenshots. User steering supersedes the previous
  prototype's unconditional fullscreen composer behavior.
- Upstream reference: GitHub `main` resolved to
  `ba5fd685159df1a1533d7054d7b2e01d3b267202`.
  `composer-state.svelte.ts` and `styles/chat/chat.css` match the local originals.
  `measureComposer` reads actual line height, padding, scroll height, width-only
  changes and anchors visible lines for three frames. `canFullscreen` requires
  two rendered lines or overflow; `setFullscreen` guards this condition.
  The checked revision uses viewport/pixel height caps, not a ten-line constant;
  ten-line growth is the user's explicit requested behavior for this correction.
- Validation: targeted baseline and regression tests, frontend check/build,
  architecture diagnostic delta, direct typing/scroll/gesture and native app QA.

## Result and verification

- Management retains the same three-page frame. A narrow character rail leads
  into one continuous character surface with a banner, name, card settings/info,
  conversation search/new-chat controls, and flat history rows. The orange accent,
  flat surfaces, 48px control targets and existing folder operations remain.
- The composer uses an inline textarea. Fullscreen is available from two rendered
  lines; the input grows through ten lines before internal scrolling, with a
  smaller cap only when the available viewport cannot fit ten lines and controls.
  Measurement follows the original final-width, line-height/padding, scroll-anchor
  and three-frame caret correction rules. Fullscreen transitions align the text's
  actual padded origin and restore the inline draft/focus. Enter/Shift+Enter and
  IME guards follow the original composer behavior.
- Rightward mouse dragging over message text now mirrors touch even when the
  initial movement is slow. A 250ms selection timeout caused real native drags to
  be discarded and was removed. Shift, double-click and leftward selection remain
  native. Independently scrolling surfaces explicitly retain vertical touch pan.
- Tests: 109 passed in 11 preview/original-composer test files, including two-line
  eligibility, ten/eleven-line growth, scroll anchoring and teardown, draft/focus
  restoration, fullscreen origins, search, and slow/fast gesture arbitration.
  Both portable worker post-tests passed. `npm run check` passed with zero errors
  and two existing `ChatPane.svelte` warnings; the final gesture changes additionally
  passed focused formatting/lint. Tauri debug app build passed.
- Browser QA: 320x693, 393x852 and 1280x800, light/dark management, search and
  navigation. Ten-line textarea measured 264px client/scroll height with hidden
  overflow; eleven lines retained 264px height with 290px scroll height and auto
  overflow. At 1280px the management/chat/creator widths were 360/612.8/307.2px,
  all 800px high, with no overlapping boundaries or outer gaps. No console errors.
- Native QA: the built `LorePia UI.app` accepted direct inline entry, exposed the
  fullscreen button only from two lines, displayed ten lines, returned from the
  fullscreen left edge while retaining the draft, and returned from message text
  to management after the final gesture fix. The app was left open on management.
  This validates the macOS Tauri shell; physical iOS/Android touch and keyboards
  were not tested.
- Architecture diagnostics were unchanged from the starting worktree (75 existing
  violations); no cap was raised. Measured frontend delta is +457 lines including
  tests (2,006 to 2,463 across changed/added files). Hash comparison against all 1,574 starting files
  limits this task's changes to preview presentation/tests, its locale, and this
  record. Native window policy, Rust, live renderer, IPC, and dependencies have no
  changes from this task. Temporary gesture tracing was removed.
