# Root navigation gestures, 2026-09-09

Follow-up to the chat controls work on `codex/tauri-chat-preview`.

## Behavior

- Swipe left/right through Home, Chats, Create, and Settings in their existing
  bottom-navigation order. The bottom navigation stays in place; adjacent pages
  follow the pointer or horizontal trackpad gesture directly.
- A short pan returns to its origin. Distance or a deliberate flick commits one
  adjacent destination. The first and last tabs resist outward dragging and do
  not wrap. A settling transition can be picked up again.
- Retain visited screens, their loaded images and scroll positions. Prepare an
  adjacent screen only when the gesture starts approaching it. Inactive screens
  remain inert and hidden from accessibility navigation.
- Root gestures stop while a detail, search, editor, choice sheet or other modal
  is open. Existing detail-back and image-gallery gestures keep their ownership.
  Inputs, selectable text, sliders and horizontal scrollers are excluded.
- Lock the horizontal direction after 8px and cancel for vertical intent. Suppress
  the release click only after a horizontal drag, including drags that start on
  cards or settings rows. Cancel cleanly on pointer cancellation, another touch,
  window blur, resize and teardown.
- The SettingsPanel root now allows root swiping. Its nested panels still opt out;
  the previous unconditional opt-out caused a setting to open after a drag.

The 300ms settling motion uses the enter easing documented in
[SEED Motion](https://seed-design.io/foundations/motion). Direct manipulation has
no delayed transition; reduced motion removes the settling animation. The four
destinations retain the role described by
[SEED Bottom Navigation](https://seed-design.io/components/bottom-navigation).

## Spacing and scrollbar follow-up

- Below the live app's 768px desktop breakpoint, hide scrollbar chrome throughout
  the workspace without changing overflow or scrolling. Keep custom thin desktop
  scrollbars. This applies to nested dialogs, the composer and galleries as well.
- Remove the extra 24px group margin before the profile's View chats and Plugins
  rows. Use a selector stronger than the later shared group margin, and block
  layout for the profile rows so button baselines do not add gaps.

## Verification

- At 394 × 852, profile scrolling advances from 0 to 1379px, with computed
  scrollbar width `none`. At 1100 × 800, it remains `thin` and overflow is `auto`.
- Lorebook → Scripts → View chats → Plugins have 0px external gaps at both
  widths. Rows retain their normal internal padding and description layout.
- Browser and rebuilt macOS Tauri app: all six adjacent tab transitions work,
  including a reverse swipe starting on a settings row. Exactly one root remains
  active. Native profile back swipe returns to Home without changing root tabs.
- Viewport overrides were reset and the native app was left on Home.
- Frontend: 150 files, 1020 tests passed. Includes action-level gesture coverage
  and real WorkspaceApp root traversal, click suppression, scroll retention and
  nested-page exclusion. Lua and regex sandbox posttests passed.
- Formatting, i18n, lint and type checking passed. The two existing legacy
  ChatPane controller-capture warnings remain. Architecture, IPC generation and
  whitespace checks passed. Native UI build passed with its existing bundle-size
  warning.

No Rust, IPC, schema, dependencies or durable data changed in this follow-up.
