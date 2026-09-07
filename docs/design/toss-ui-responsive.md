# Responsive native UI prototype

- Task ID: `UI-RESPONSIVE-20260906`.
- Baseline / merge-base: `82a77caed61713109dfd8dc86b36aafee3b834d6`.
- Starting worktree: 74 existing modified/untracked files, hashed in
  `/tmp/lorepia-ui-responsive-20260906/worktree-before.json`; preserve unrelated work.
- Request: smoother PC/mobile layout changes and uniform proportions below a
  minimum width. The related tap/hold question was answered; it does not add
  pressure detection or long-press actions to this task.
- Targets: `src/preview/ui/UiPreview.svelte`, page navigation affordances,
  responsive geometry/styles and focused tests, plus scaled swipe coordinates.
  The `ui-preview.html` entry and native window configuration stay in place.
- Owned invariants: orange flat styling, three connected pages, optional
  creator page, one mounted instance of each page, conversation/draft/form
  state, focused editing, scroll ownership and reduced-motion behavior.
- Keep the existing navigation, sample-data and component entry points. The
  viewport geometry helper is preview-local. No Rust, IPC, schema, capability,
  dependency, production renderer or data ownership change.
- Expected delta: roughly 300–500 lines across bounded geometry, styles,
  component integration and tests. No size baseline increase.
- Risks: breakpoint jumps, shrinking the active editor offscreen, duplicate
  page state, partially hidden controls remaining focusable, incorrect swipe
  distances after scaling, stale observers and resize animation.
- Checks: current preview tests before/after, geometry and resize/state cases,
  frontend check, architecture delta, IPC generation, browser widths around
  breakpoints and native app rebuild/direct use.
- Governing material: root/frontend `AGENTS.md`, ADR 0001, accepted preview and
  motion notes, user screenshots and this turn's responsive-layout request.
- Architecture baseline: the existing 75 Rust API diagnostics; no new ones.

## Layout choices

- 1120px and wider: management, chat and optional creator page together.
- 760–1119px: chat with either management or creator content alongside it.
- Below 760px: one page at a time, retaining horizontal direct manipulation.
- Below 360px: preserve a 360px logical layout and scale text, icons, controls
  and spacing together. Height compensates for scale so the composer remains
  at the bottom of the available viewport. Normal press feedback still keeps
  its hit area fixed relative to that layout.
- Side columns adjust within bounded widths. Animate only changes between
  layout modes; resizing within one mode follows the window directly.
- The two-pane layout gives either side the same width, so switching sides
  does not change the chat's line wrapping. Navigation icons disappear when
  their target page is already alongside the chat.

## Verification — 2026-09-06

- All 47 preview tests passed: existing flows/gestures plus viewport geometry,
  scaled pointer tracking, retained editor/draft, optional creator navigation
  and an open settings form across layout changes. Existing Lua/regex worker
  regressions also passed.
- Formatting, i18n, ESLint and Svelte typecheck passed. Two existing warnings
  remain in production `ChatPane.svelte`, outside this preview.
- IPC generation passed. Architecture output remained byte-for-byte identical
  to the initial 75-diagnostic Rust baseline. Existing dirty files changed only
  within the planned preview scope.
- Browser: visually checked 1280/960/393/360/320px and the right-side two-pane
  composition. Measured 759/760/1119/1120px boundaries with no meaningful gaps
  or overlap. The 320px view uses a 360px logical width at 8/9 scale; icon,
  button and composer dimensions follow that same ratio. Scaled swipe worked.
- Native macOS WebKit: built the standalone Tauri app, opened the previous
  selected conversation, expanded the window to three panes, returned to one
  pane and resized to two panes. The conversation stayed selected. Left the
  updated app open in the two-pane layout.
- No browser console warnings/errors were observed. This pass used macOS
  native input and browser viewport sizes, not physical iOS/Android devices.

The prototype continues to use local, session-only sample data. No pressure
sensor or duration-based long-press action was added by this layout task.
