# Coupled native window resizing

- Task ID: `UI-WINDOW-RATIO-20260906`.
- Baseline / merge-base: `82a77caed61713109dfd8dc86b36aafee3b834d6`.
- Starting worktree: 84 modified/untracked files; hashes and the starting
  native entry/config captured in `/tmp/lorepia-ui-window-ratio-20260906`.
- Request: narrowing the phone-sized app must also reduce the window height.
  Earlier canvas framing and history-height changes did not implement that.
- Targets: the UI-preview branch of `src-tauri/src/lib.rs`, a private macOS
  window-sizing module and its tests, and `tauri.ui.conf.json` launch size.
  Retain the `run()` and `ui-preview.html` public entries and production window
  behavior. No IPC, renderer data, dependency, schema or public API changes.
- Invariants: full-bleed renderer, existing layout/navigation/press motion,
  fixed configured minimums, OS-owned border hit testing and live resize,
  fullscreen/maximize escape, and return to freely resizable desktop widths.
- Retained symbols: `hold_phone_aspect` and `uses_fixed_preview_minimum` keep
  the existing production minimum policy and preview selection. No movement
  or unrelated renames. New symbols remain private or `pub(super)`.
- Implementation: use AppKit's native window aspect constraint (9:19.5 for
  the whole decorated preview window) for live dragging. Set size only on
  entry to phone mode; do not chase every resize event with `set_size`.
  Release the constraint before the screen's usable height blocks expansion,
  with a small entry/exit band so the mode does not oscillate at the boundary.
- A small macOS-only Objective-C setter bridge avoids adding dependencies or
  platform-plugin APIs. It runs on the main thread, borrows only the NSWindow
  owned by Tauri, and passes two-double NSSize values to fixed SDK selectors.
  It never exposes or stores the native pointer outside that call.
- Expected size delta: about 180–260 lines of private policy/adapter/tests and
  fewer than 10 native-entry lines. No source baseline increase.
- Risks: resize feedback, a phone-mode trap at screen height, non-native
  aspect enforcement, fullscreen conflicts, mixed DPI, or broadening behavior
  beyond the isolated UI app. Tests cover entry/exit and screen limits; native
  launch and actual border resizing verify the resulting dimensions.
- Governing material: root/frontend repository guides, existing UI task notes,
  the user's latest appshot/correction, the pinned Tauri/Tao implementation,
  and [AppKit aspectRatio documentation](https://developer.apple.com/documentation/appkit/nswindow/aspectratio).

The user has now explicitly clarified that the window height must follow its
width. This supersedes the earlier optional canvas-vs-window default.

## Verification

- Rebuilt and reopened the macOS `LorePia UI.app`. Actual right-border dragging
  produced 360 × 780 and 320 × 693 window screenshots, matching 9:19.5 within
  native pixel rounding. The old 320 × 782 frame no longer remains tall when
  narrowed. The decorated launch frame measured 394 × 854 after native rounding.
- Expanded past phone mode into the two-pane desktop layout, then narrowed
  back to 342 × 741 and 320 × 693. The native constraint can be released and
  re-entered; the renderer continues to fill the window. These are functional
  drag checks, not a frame-time performance measurement.
- `cargo test -p lorepia-tauri --lib`: 180 passed, 2 existing ignored tests.
  The 8 new policy tests cover proportional entry, no per-frame resize
  requests, desktop return, screen limits, and fullscreen/maximize release.
- Targeted Clippy with warnings denied, workspace Rust format check, IPC
  generation check, and the UI Tauri build passed.
- Architecture checker still reports the same pre-existing diagnostics as
  the captured starting worktree; the report changes only the native entry's
  measured size by 8 lines. New adapter and test files are 162 and 99 lines.
- Starting-worktree hash comparison confirms this task changed only the
  native entry, preview launch config, new private adapter/tests, and this note.
  The rebuilt preview app is left open at mobile size.
