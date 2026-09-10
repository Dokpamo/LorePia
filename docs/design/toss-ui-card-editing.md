# Card organization and focused editing

- Task ID: `UI-CARD-EDITING-20260906`.
- Baseline / merge-base: `82a77caed61713109dfd8dc86b36aafee3b834d6`.
- Starting worktree: 87 changed/untracked files; their hashes are captured in
  `/tmp/lorepia-ui-interactions-20260906/worktree-before.json`.
- Authorized scope: refine the running, isolated Tauri UI prototype: square
  character tiles without captions, drag-to-folder organization, gear icons,
  the existing composer's draft/fullscreen ideas, edge-back navigation,
  fullscreen text editing, readable chat and consistent motion.
- Targets: `src/preview/ui/` components/actions/styles/tests and the existing
  `lib/i18n/ko-ui-preview.ts` catalog. Keep `ui-preview.html` and its native
  launch/resizing policy. No production, Rust, IPC, schema or dependency changes.
- Owned invariants: orange/flat styling; 48px hit targets; existing three-page
  layout; full-bleed native window; local sample state; per-conversation drafts;
  IME safety; vertical scroll; selection; focus return and reduced motion.
- Symbol/caller inventory: `UiPreview` owns sample state and navigation;
  `CharacterPage`, `ChatPage`, `SettingsPage` own presentation. Existing
  `swipePages` handles page gestures. New folder and edge-back actions must
  not compete with text editing, rail scrolling or page gestures.
- References: production `ChatComposer`, `ChatFullscreenComposer`,
  `composer-state.svelte.ts`, and `ChatPane` are read-only references. Carry
  forward their draft, composition, expanding-preview and focus ideas without
  importing production controllers into the isolated sample UI.
- No public symbols move. New local components/actions stay bounded. Expected
  addition about 900–1,400 lines across components, styles and gesture/model
  tests; no source-size cap or exception changes.
- Risks: unintended clicks after dragging, lost cards/drafts, competing scroll
  gestures, nested editor focus, interrupted transitions and WebKit resizing.
- Baseline: 64 preview tests and Lua/regex regressions passed. Run frontend
  checks, affected preview tests, production composer references, UI build,
  architecture delta and direct native UI flows after implementation.
- Governing material: root/frontend AGENTS, ADR 0001, prior accepted UI task
  notes, the supplied Toss screenshots and the user's latest request.

The visual reference remains flat, with whitespace and surface color defining
groups. Toss's [product principles](https://toss.tech/article/mydoc) and
[interaction approach](https://toss.tech/article/interaction) inform clear
actions, focused editing and immediate feedback. Sizes and timing here are
LorePia choices, not claims of official Toss specifications.

Folders and edits in this app remain sample state for the current window
session. This task does not introduce a durable folder schema or AI dispatch.

## Implemented behavior

- The rail uses rounded square tiles without visible captions, while retaining
  accessible names and tooltips. Selected-card identity remains beside its image.
- Mouse dragging combines cards/folders; touch uses a 300ms hold. Quick vertical
  movements scroll the rail. Folder members remain selectable, can be extracted,
  and single-member folders dissolve. Context menu / Shift+F10 provides a keyboard
  alternative. Folder renaming uses the common editor.
- Settings controls use gear icons. All hit targets retain the shared 48px
  logical control size; secondary glyph sizes follow their context.
- Every text field opens the same full-window editor. Closing retains the
  field draft and returns focus. Message drafts remain attached to their
  conversation. Enter inserts a line break; Cmd/Ctrl+Enter sends, with IME
  composition protection. The compact composer previews up to four lines.
- Chat keeps readable author/body grouping, user bubbles, copy and user-message
  editing, scroll-position preservation, and a jump-to-latest control.
- Settings, folder organization and text editors support edge-back. The pointer
  drives the surface during the gesture; short drags return. Completion holds
  its offscreen frame through the component outro, avoiding a return-frame flash.
  Background surfaces remain visible during the gesture but cannot be edited.
- Page/press motion shares the existing vocabulary. Organization uses FLIP;
  editors/settings use short compositor transitions. Reduced motion is respected.

## Verification — 2026-09-07

- 77 preview tests passed, covering grouping without card loss, extraction,
  keyboard organization, text editing, IME, conversation drafts, focus,
  responsive resizing, touch-hold/scroll separation, and gesture cancellation.
- 27 existing composer/chat tests passed. Lua and regex worker regressions passed.
- Frontend format, i18n, lint and typecheck passed. Only the two pre-existing
  production ChatPane warnings remain. IPC generation passed; the architecture
  report is unchanged from the captured 75-diagnostic starting baseline.
- Browser: 320px tiles remain square (42.67 × 42.67 after the existing uniform
  scaling), without shadows. Fullscreen editor measured 1280 × 800, then
  320 × 661 after narrowing, retaining both draft and textarea focus.
- Native macOS WebKit: actual card dragging created a folder; name editing,
  extraction and an edge-back drag restored the expected parent view. Native
  window borders retain their resize affordance; mouse edge-back starts just
  inside the border. Physical iOS/Android touches and frame-time profiling were
  not performed. Synthetic pointer tests cover touch classification; the
  reduced-motion branches were reviewed in code.
- Rebuilt and reopened the final native bundle. The full-window message editor
  retained a two-line draft after edge-back and returned focus to the composer.
- Scope hash comparison confirms changes are limited to the planned isolated
  UI files, its catalog and this note. No Rust, production renderer, IPC,
  dependencies, schema, capabilities or baselines were changed in this task.
