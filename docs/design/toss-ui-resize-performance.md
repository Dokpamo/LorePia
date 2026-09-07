# UI preview resize response

- Task ID: `UI-RESIZE-PERF-20260906`.
- Baseline / merge-base: `82a77caed61713109dfd8dc86b36aafee3b834d6`.
- Starting worktree: 79 modified/untracked files, hashed in
  `/tmp/lorepia-ui-resize-perf-20260906/worktree-before.json`.
- Request: remove delayed, stuttering response during PC window resizing.
- Targets: `src/preview/ui/UiPreview.svelte`, `ui-responsive.css`, and the
  colocated responsive component tests. Entry remains `ui-preview.html` through
  `src/preview/ui/main.ts`; rebuild the existing standalone Tauri UI bundle.
- Invariants: orange flat design; three/two/one-pane geometry; uniform scaling
  below 360px; mounted pages, selected conversation, drafts, form/focus state;
  immediate pointer tracking and page-settle/press motion; reduced motion.
- Symbols retained: `responsiveLayout`, `layoutMode`, `swipePages`, navigation
  and sample-data entry points. Replace only the viewport observation and
  resize-settle timers; no symbol movement or unrelated cleanup.
- Expected size delta: approximately -20 production lines and +50–100 test
  lines. No source-size baseline changes.
- Risks: accidentally animating a resized track, interrupting a swipe,
  duplicated layout measurements, delayed gesture re-enablement, stale frame
  callbacks on unmount, or losing focused content across a breakpoint.
- Governing material: root/frontend `AGENTS.md`, ADR 0001, accepted preview,
  motion and responsive notes, and the user's current performance feedback.
- Baseline checks: 47 preview tests and Lua/regex regressions pass. Architecture
  checker retains the existing 75 Rust diagnostics; compare before/after.
- Planned verification: focused tests, frontend check, IPC generation,
  architecture delta, immediate browser geometry around both breakpoints,
  gesture/state checks, and direct use of the rebuilt native app.

## Observed cause

At 1119px the old renderer immediately declared a two-pane layout while its
columns were still interpolating for 240ms. The first observed left/chat widths
were 353.22/660.36px instead of their target 335.70/783.30px. Further resize
events retargeted those flex-basis transitions. Both a window listener and a
ResizeObserver also measured the viewport, and 120/240ms timers kept resize
and gesture-lock state alive after the geometry had changed.

ResizeObserver supplies the measured size; a window listener is only a fallback.
Gestures need no timed lock. The initial attempt to remove responsive motion
was rejected by the user; the final motion below supersedes that approach.
## Follow-up within the same task

The user rejected removing the responsive transition and also reported trouble
grabbing the native resize edge. Extend this task to preview-local layout motion
and the native window's minimum-size policy in `src-tauri/src/lib.rs`.

- Interpolate layout-mode weights over 180ms, using the current viewport for
  every frame. Pixel resize events must not restart the animation. Reversing a
  breakpoint continues from the current weights. Existing page navigation and
  press motion remain separate.
- Keep a single geometry update owner and snap to the final mode on a new
  pointer interaction or reduced-motion preference; no timed interaction lock.
- The preview uses its configured fixed minimum size. Exempt only the
  `ui-preview.html` window from the legacy height-dependent phone aspect floor,
  which currently overrides that minimum on native resize events. Preserve the
  live app's policy and the standard decorated macOS resize border.
- Additional targets: responsive geometry helper/tests and a small preview-only
  layout-motion helper. Expected additional production delta: about 100 lines.
- Additional checks: interpolated viewport coverage, motion/reversal behavior,
  preview-vs-live native sizing policy, Tauri library tests and clippy.
- No custom native resize IPC, capability, dependency, backend or public
  contract changes. The installed macOS windowing backend does not support
  programmatic resize-dragging, so adding a fake CSS resize cursor is not a fix.

## Verification

- 57 preview tests pass, including the regression that a mode change completes
  despite continued pixel resize events; positive column widths and full
  viewport coverage throughout interpolation; drafts/focus; immediate gestures;
  reduced-motion observation; and frame/observer cleanup. Lua/regex regressions
  pass as well.
- Frontend formatting, i18n, lint and typecheck pass. The two existing production
  `ChatPane.svelte` warnings remain outside this task.
- Tauri library tests: 172 pass, two existing external-fixture tests ignored.
  Tauri library/test clippy and workspace format check pass. IPC generation
  passes; the 75 pre-existing architecture diagnostics are unchanged. The
  architecture report additionally records the authorized native file growth.
- Browser: confirmed live interpolation during repeated 1119→1000px changes,
  then exact 300/700px final columns. At 320px, chat fills the physical width
  with its left edge at zero, uniform 8/9 scaling and retained draft/focus.
  Right-side split composition fills 900px correctly, and deliberate page
  navigation still uses its 280ms settle motion. No console errors/warnings.
- A final responsive frame must not start a second CSS settle transition.
  Resize keeps navigation motion off until an explicit page navigation or
  swipe settlement (`SwipeOptions.onsettle`) opts back in. Pointer input can
  immediately finish an in-flight responsive morph.
- Native border thickness/hit geometry stays under macOS control. The native
  fix removes the preview's conflicting minimum-width policy; it does not add
  wider synthetic hit areas. During direct native checks, concurrent user
  window interaction invalidated automation state, so automated mouse dragging
  was stopped to avoid interfering with the user's own resizing.
- Rebuilt the final standalone macOS bundle and reopened `LorePia UI.app` with
  the existing selected sample character/conversation and no unsent native
  draft to restore. Left the app open. Closed only the task's QA browser tab
  and stopped its 5176 development server; the user's 5175 tabs remain intact.

No FPS improvement percentage is claimed. These checks establish removal of
restarted transitions and unwanted constraint updates, not hardware frame-time
measurements.
