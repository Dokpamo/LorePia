# Chat interaction completion

- Task ID: `UI-CHAT-COMPLETE-20260907`.
- Baseline/merge-base: `82a77caed61713109dfd8dc86b36aafee3b834d6`.
- Worktree: prior UI and native preview changes are dirty. Initial hashes and
  target copies are recorded in `/tmp/lorepia-chat-complete-20260907/`.
- Request: implement the preceding chat review, examine the original GitHub
  conversation/story/bubble behavior more closely, restore tap-to-reveal tools,
  and fix the transient enlargement while narrowing the window.
- Targets/entries: isolated `preview/ui/UiPreview`, chat, composer, settings,
  preview-local state/presentation helpers, their tests and Korean catalog;
  private native `ui_preview_window::resize_step` and its policy tests.
- Shared view helper adjustment: normalize DOM measurements in the existing
  `ChatScrollLifecycle` when its surface is CSS-scaled below 360px. Default
  unscaled production geometry stays unchanged. The private AppKit callback
  must also avoid reapplying launch sizing during a live inward drag.
- Existing source to reuse: view-local `ChatScrollLifecycle` and its bounded
  virtual message/anchor logic; source styles/actions/mode controls as behavior
  references. No production controller or backend data connection is introduced.
- Owned invariants: stable message IDs, safe text rendering, separate draft and
  conversation state, original branch retained by edit/regeneration, cancelled
  sample stream cannot update another room, caret/scroll/keyboard focus remain
  stable, horizontal back vs vertical scroll vs native selection arbitration,
  reduced motion and observer/timer cleanup, no shadows or blue theme additions.
- Symbols: existing public renderer/native entries remain. New preview helpers
  are internal presentation/sample-state code; no public API moves or renames.
- Expected delta: approximately 900–1,500 lines including focused tests, split
  by responsibility. No source-size baseline increase or giant-file exception.
- Risks: resize minimum/aspect conflict, composer/viewport scroll feedback,
  animated row measurement, branch identity, live-response replacement, focus
  during transitions and virtual-row removal.
- Validation: preview tests, existing ChatPane scroll/virtualization tests,
  frontend check, private native resize tests and native lint/build, architecture
  baseline comparison, browser/mobile geometry and real Tauri interactions.
- Governing material: root/frontend AGENTS; ADR 0001. GitHub main checked at
  `ba5fd685159df1a1533d7054d7b2e01d3b267202`; original `chat.css`,
  `ChatMessageList`, `MessageActionsState`, `ChatRoomControls`, `ChatViewport`,
  composer and scroll lifecycle inspected. Chat/story styling is explicitly
  distinct on desktop upstream; the requested distinction will also be exposed
  on mobile here. Existing orange, flat three-page frame is retained.
- The preview remains in-memory sample UI. It does not dispatch a real model,
  import private data, change IPC/schema/security boundaries or add dependencies.

## Implemented behavior and verification

- Reused the original scroll lifecycle and safe Markdown presentation, with
  stable message IDs and a bounded virtual window. Text streaming does not
  rebuild the full identity index. CSS-scaled surfaces measure in logical pixels.
- Conversation mode uses left/right bubbles; story mode gives assistant prose
  the full reading width on mobile and desktop. Room settings select the mode.
- Tapping a message reveals tools underneath it over 360 ms, pushing subsequent
  content. The selected message retains its position and tools remain above
  the composer. Manual scrolling interrupts automatic tool reveal scrolling.
- Copy, branch, edit, regenerate, and confirmed tail deletion are connected to
  local sample state. Edits and regenerated responses retain the original branch.
  Removed tools hand keyboard focus to the transcript. A local sample stream
  demonstrates pending, stop, failure and retry without calling a provider.
- The composer publishes field height, bottom inset and total overlay together.
  Its two-line fullscreen affordance and ten-line expansion remain. The latest
  message button has a real hit area above the field. Typing while reading old
  messages preserves the reading position; sending follows the new response.
- Interactive QA exposed an additional focus bug: an overflow-hidden chat page
  could itself scroll, shifting its header/composer up and leaving a blank area.
  The fixed chat surface now uses overflow clipping; only the transcript scrolls.
- Native shrinking cannot enlarge the other axis while fitting phone proportions.
  The AppKit callback no longer reapplies the launch fit during a live drag.
- Browser QA: 320 × 693, 393 × 851 and 1200 × 800; chat/story, formatted long
  messages, inline actions, two/ten-line composer, failure/retry, and send-follow.
  At 393px the ten-line field is 336px tall, with 264px visible text and scrollTop
  zero. At 320px tools stay about 12px above the field, the fixed header stays
  at y=0, and the page scrollTop stays zero. Browser error/warning log was empty.
- Actual macOS border drags (captured whole-window sizes): 452 × 886 → 442 × 886
  → 380 × 856 → 322 × 730; independent height 322 × 851 → 322 × 831.
  Rightward back gesture and inline tools were also exercised in the rebuilt app.
- Frontend format/i18n/lint/type checks pass; two existing ChatPane controller
  capture warnings remain. Preview and original scroll tests: 125 passed, including
  scaled geometry, branch retention, stream cancellation and composer inset.
  Native library tests: 181 passed, 2 ignored. Native clippy and Rust format pass.
  Architecture comparison retains exactly the same 75 pre-existing diagnostics.
- iOS/Android physical keyboard behavior, real provider responses, reasoning
  output and portable creator-content execution remain outside this isolated
  preview validation; their original runtime and safety boundaries are unchanged.
