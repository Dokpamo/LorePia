# Composer continuity and back navigation

- Task ID: `UI-COMPOSER-GESTURE-20260907`.
- Baseline / merge-base: `82a77caed61713109dfd8dc86b36aafee3b834d6`.
- Starting worktree is dirty; all 1,568 source-file hashes were captured at
  `/tmp/lorepia-ui-composer-gesture-20260907/worktree-before.json`.
- Scope: correct rightward back gestures in the isolated UI preview and carry
  over the existing composer's measured geometry and fullscreen morph.
- Initial targets/public entry: `src/preview/ui/` components, gesture/motion helpers,
  styles and tests; `ui-preview.html` remains the entry. Production composer,
  native window sizing, Rust, IPC, dependencies and storage remain unchanged.
- Invariants: full-window focused editing, retained per-conversation drafts,
  IME safety, orange flat surfaces, 48px logical hit areas, no letterboxing,
  native vertical scrolling/text selection, one back action per gesture,
  reduced motion and cleanup/focus restoration.
- Inventory: `swipePages` owns the three-page track; `edgeBack` owns overlays.
  Neither handles horizontal wheel/trackpad input. Overlay dragging currently
  starts only in the first 24px. `MessageComposer` is always two rows and
  `TextEditor` uses a generic fly transition; production `ChatComposerState`
  measures the field, text and control origins for a continuous clip-path morph.
- No public symbols move. Add bounded preview-only wheel and geometry helpers.
  Expected delta: 450–750 lines including regression tests; no cap changes.
- Risks: trackpad momentum causing double navigation, stealing vertical scroll
  or text selection, interrupted morphs, stale origin after resizing, focus and
  draft loss. Tests cover these paths; native/browser checks cover actual layout.
- Governing material: root/frontend AGENTS, ADR 0001, accepted flat/orange
  screenshot direction and `toss-ui-card-editing.md`. The production composer
  components, `composer-state.svelte.ts`, `styles/chat/chat.css` and mobile
  overrides are read-only references, also inspected in the running preview.
- Validation: preview and production composer tests, frontend check, unchanged
  architecture/IPC reports, native Tauri build and hands-on gesture/composer QA.

## Additional user steering

The user subsequently authorized asymmetric native resizing: shrinking keeps
the current proportions in phone-width mode; growing changes only the dragged
axis. This explicitly extends this same task to private `ui_preview_window.rs`
and its tests. Existing exported entry points and the production window policy
remain unchanged. AppKit's `windowWillResize:toSize:` supplies the size before
painting, avoiding a post-resize `set_size` feedback loop. The pinned Tao
delegate has no implementation of this optional method; install only missing
optional hooks and scope their behavior to registered preview windows. No
dependency, IPC, schema or capability changes are needed.

The UI-only Tauri configuration now explicitly retains the running prototype's
`dev.lorepia.mac.dev` bundle identity, so rebuilding does not inherit the
production app identifier. This preserves the app the user is already testing.

Reference: [Apple's NSWindowDelegate sizing callback](https://developer.apple.com/documentation/appkit/nswindowdelegate/windowwillresize%28_%3Ato%3A%29?language=objc).

## Implemented behavior and verification

- The mobile composer starts at 64px with 12px side/bottom spacing and grows
  upward for multiple lines. Its field, text and controls supply measured
  origins to the 420ms opening / 360ms closing fullscreen transition. Drafts
  survive closing, resizing and navigation; background input remains inert
  through the closing transition, then focus returns to the original control.
- Rightward dragging supports free page/overlay surfaces and the left edge of
  editable text. Horizontal wheel navigation accumulates directional intent,
  ignores zero-delta phase events and groups momentum into one navigation.
  Browser wheel navigation and native pointer back gestures were exercised.
  Native CUA scroll injection did not navigate, so physical Mac trackpad
  behavior remains unverified; no iOS/Android hardware claim is made.
- The macOS preview preserves its current proportions only while shrinking
  within phone-width mode (480px or less). Growing changes the grabbed axis.
  Real window-border drags produced screenshot sizes 394×854 → 454×854 for
  horizontal growth, then 454×914 for vertical growth; shrinking yielded
  394×794, then 370×744. Screenshot dimensions include window-edge pixels.
- Native editing was exercised with a two-line draft: open, type, edge-drag
  back, retain both lines and restore focus. The rebuilt UI app is left open.
- Final preview suite: 85 tests passed; the combined preview and existing
  composer/action run passed 111 tests. Tauri library tests: 179 passed,
  2 ignored. Frontend check, native Clippy, Rust formatting, IPC generation
  check and the final Tauri bundle build passed. Frontend check retains two
  existing `ChatPane` warnings.
- Architecture output is byte-for-byte identical before and after (75 existing
  diagnostics). No baseline or public boundary was changed. Source hashes show
  changes only in the preview UI, private preview window policy, its UI-only
  Tauri configuration and this task record. Logs and the final scope inventory
  are under the task's `/tmp` directory above.
