# Phone aspect correction

- Task ID: `UI-PHONE-ASPECT-20260907`; baseline/merge-base
  `82a77caed61713109dfd8dc86b36aafee3b834d6`.
- User steering: the narrowed native window is too short to resemble a phone.
  Existing work is dirty; native snapshots are in
  `/tmp/lorepia-phone-aspect-20260907/before/`.
- Target private entry: `ui_preview_window::update`, native live-resize callback,
  pure resize policy and its tests. Preserve outward axis-only resizing and
  desktop freedom; shrinking in the phone range uses the 19.5:9 reference again,
  rather than inheriting an independently stretched window's current ratio.
- Account for the macOS title bar separately from the mobile content area. Keep
  launch/resize within the display work area; retain original delegate ownership
  and native callbacks. No app data, IPC, dependencies or production behavior.
- Expected size delta: under 100 lines. Owned invariants: bounds, scale factors,
  invalid-size handling, native main-thread lifecycle, no post-resize feedback
  loop, and existing cross-architecture Objective-C ABI handling.
- Validation: native policy tests, formatting/lint, debug app build and border
  dragging in the real Tauri window. UI back-motion work is resumed after this
  bounded native correction. Source: root AGENTS and user steering; phone ratio
  is a reference size, not a claim that every phone model has one aspect ratio.

## Result

- Net native delta: +36 lines across the existing three private preview-window
  files, including tests. No source-size baseline changes.
- `cargo test -p lorepia-tauri --lib`: 179 passed, 2 ignored.
  `cargo clippy -p lorepia-tauri --lib -- -D warnings` and Rust formatting passed.
- Debug app rebuilt and tested with real border dragging. Approximate screenshot
  sizes, including the native title bar: 394×886 at launch; reducing the right
  edge produced 341×771; expanding it produced 394×771; increasing height
  produced 394×851; reducing height produced 365×821. The shrinking content
  area follows 19.5:9, with screenshot/native rounding around the frame.
- These are macOS Tauri results. The policy does not enforce a device ratio on
  an actual phone or imply that independently expanded desktop windows retain it.
- Architecture checker output is identical to the task's starting output:
  75 pre-existing diagnostics; no new diagnostics from these changes.
