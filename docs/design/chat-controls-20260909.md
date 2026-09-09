# Chat controls and activity, 2026-09-09

Checkpoint before this work: `331f515` on `codex/tauri-chat-preview`.

## Reference and decisions

- [SEED Menu](https://seed-design.io/components/menu): use a contextual surface
  attached to the pressed message, with an 8px gap, 240px target width and viewport
  collision handling. Keep deletion last and separated. Enter at 150ms; exit at
  100ms. This replaces the inline panel that pushed the conversation.
- [SEED Motion](https://seed-design.io/foundations/motion): small feedback stays
  short; the composer changes size at 300ms. Respect reduced motion.
- [SEED Spacing](https://seed-design.io/foundations/spacing): share dimensions
  rather than adjusting each screen separately. The resting composer and bottom
  navigation now use the same dock width, height and bottom inset tokens.
- [SEED Text Input](https://seed-design.io/components/text-input): preserve one
  writing surface and clear focus feedback. The user-requested composer remains
  automatic through ten lines, with internal scrolling after that.

## Changes and causes

- Removed chat-history row settings buttons. Show each conversation's existing
  `updated_at` as today's local time or an earlier calendar date. The full date
  and time remain in the time element's accessible label and title. Home cards
  show the latest valid conversation timestamp for that character. Unplayed
  cards do not receive an invented timestamp.
- Both search pages have no input border, outline or shadow. Focus changes the
  neutral surface instead. Scrollbars share theme-aware thumb and hover colors;
  native forced-color behavior remains available.
- The send button had inherited the generic icon visual's padding, which was
  incompatible with its smaller circular surface. Its 40px circle now has zero
  padding and centers the 24px Lucide arrow, preserving the larger hit target.
- Desktop CSS was forcing the composer open. Remove that exception. Empty
  composers now collapse after focus leaves, including outside pointer actions
  whose default focus change is intercepted by the back gesture.
- Use 12px above the text and 4px between text and the 44px tool row. Measure the
  final writing geometry rather than intermediate animated offsets, preserving
  stable ten-line limits and the existing full-screen editor morph.
- Holding a message for 450ms or using right click opens its menu. Movement over
  10px, scrolling, cancellation and teardown cancel the pending hold. Links and
  ordinary text interaction retain their native behavior.
- Message tools use the existing copy, user-message edit, branch, regenerate and
  deletion controllers. Branch creation and switching live in the message menu.
  The gray deletion confirmation is on the left; pistachio cancellation on the
  right. No destructive action occurs when opening the menu.
- A manually managed popover avoids the context-menu opening event immediately
  dismissing an automatic popover. Capture the menu's keyboard events before the
  native back handler so Escape closes the menu without leaving the chat.

## Verification

Browser measurements, CSS pixels, after transitions completed:

| Viewport | Resting composer and bottom navigation (both) |
| --- | --- |
| 360 × 640 | x 12.23, y 571.85, width 335.53, height 56.50 |
| 394 × 852 | x 13.39, y 777.41, width 367.22, height 61.84 |
| 1200 × 800 | x 380, y 709.91, width 440, height 74.09 |

- No horizontal document overflow at the measured widths.
- At 394px width, ten lines have a 256px textarea with equal client/scroll height
  and hidden overflow. Eleven lines retain 256px client height, 282px scroll
  height and automatic overflow. The text-to-tools gap is 4px.
- Both focused search bars report zero border, no outline and no shadow.
- Confirmed anchored deletion/cancellation, full-screen writing, empty-outside
  collapse, and Escape focus restoration in the browser. Temporary viewport
  override was reset after verification.
- Rebuilt and relaunched the native macOS Tauri application. Verified persisted
  activity dates, removed list gears, contextual message actions, Escape staying
  in the chat, the two-line composer and empty-outside collapse. Removed the QA
  draft without sending a message or calling a provider.
- Frontend tests: 148 files, 1,003 tests passed, including new hold/cancellation,
  activity formatting, empty composer and native-back/menu regression coverage.
  Lua and regex sandbox posttests passed.
- Frontend format, i18n, lint and type checks passed. Two existing controller
  capture warnings remain in the legacy `ChatPane.svelte`.
- Source architecture, generated IPC registry and `git diff --check` passed.
- Native UI build passed; the existing bundle-size warning remains.

The native edit command currently supports user messages only. Assistant-message
menus retain regeneration; this change does not introduce an assistant edit
contract. There are no Rust, IPC, schema, dependency or credential changes in
this patch.
