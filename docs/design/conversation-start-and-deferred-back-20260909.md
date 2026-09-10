# Conversation setup and deferred back confirmation

## Interaction

- A dirty editor completes its departure gesture or close-button transition before
  asking whether to discard changes. The previous page remains visible behind
  the confirmation. The editor stays mounted offscreen, inert, with its draft.
- Continue editing brings that same editor back and restores focus. Page editors
  return from the right; text popups return from below. Discard closes the editor
  without replaying the departure transition.
- Busy saves still block departure. Repeated back input, covered-state updates,
  and window resizing cannot return a pending editor behind the confirmation.
- The existing two-button arrangement remains: discard in gray on the left,
  continue in pistachio on the right. Escape continues editing.

## Starting a conversation

- The setup page offers a title, saved persona (or no persona), starting scene,
  optional scene language, and chat/story mode. Selections stay local until Start.
- Starting scenes use the existing revision-bound greeting catalog. Matching
  render-profile reading aids supply scene titles, excerpts, and language groups.
  Missing or stale reading aids fall back to catalog labels, not another revision.
- Choosing a group prefers the app language, then the author's recommended
  language, then English. An explicitly selected variant remains selected.
- Default titles are `새 대화`, `새 대화 2`, `새 대화 3`, etc., within the selected
  character's existing conversations. Custom names remain available.
- The conversation controller creates the greeting/mode-bound room, then pins
  the selected persona through the existing persona API before entering chat.
  Repeated Start is guarded. If persona acknowledgement or room loading fails,
  retry uses the created room and verifies an existing persona selection first.
  Pending setup values remain available when returning to that setup.
- Current native card/profile contracts have no dedicated embedded user-persona
  field. Unknown card extensions are not interpreted as personas. The native
  setup can select existing local personas; the browser demo supplies its existing
  three sample personas. No new card schema, IPC command, or database migration
  was introduced.

## References

- [SEED motion](https://seed-design.io/foundations/motion): preserve spatial
  continuity with the existing navigation spring and reduced-motion behavior.
- [SEED alert dialog](https://seed-design.io/components/alert-dialog): focused
  confirmation with explicit actions and retained input.
- [SEED select box](https://seed-design.io/components/select-box): full-row
  selection targets and a separate final submit action. The existing choice
  sheet and radio controls are reused.

## Setup layout review after checkpoint ec796ec

- The previous screen mixed a stacked name editor, separated picker hints, and
  large mode radio cards. Repeated form/group margins made the spacing uneven;
  its submit button was part of scrolling content instead of the page footer.
- The setup now uses a compact inline navigation title and one list alignment
  for name, persona, scene, optional language, and mode. Name opens the text
  editing sheet; other choices use the selection sheet. Persona descriptions,
  scene excerpts, and mode explanations are associated with their sheet options
  as accessible descriptions, visually clamped to two lines.
- Persona catalogs remain openable when empty: the choice sheet offers no
  persona and a fixed Add persona action, alongside any saved personas. The
  action waits for the sheet to close, then opens a full-page editor using the
  existing name/description text editors and native persona save API. The
  returned persona identity becomes the setup selection. Load errors and retry
  remain available; no conversation is created until Start.
- [SEED List](https://seed-design.io/components/list) and
  [Spacing](https://seed-design.io/foundations/spacing) inform the shared 16px
  gutter, consistent row alignment, and full-row targets. The compact 18px/24px
  title follows [Top Navigation](https://seed-design.io/components/top-navigation).
  A single pistachio submit button uses the high-emphasis hierarchy from
  [Action Button](https://seed-design.io/components/action-button).
- Measured at 394 × 852 CSS pixels: title 18px, row labels 16px, values 14px,
  four 56px rows, and a 362 × 52px CTA at x=16, y=788. Header/content/footer share
  a 640px maximum width on desktop; the 1280px viewport has a 608 × 52px CTA.
  A long name wraps within the value column at 320 × 568 without horizontal
  overflow. Temporary browser viewport overrides were reset after QA.
- The rebuilt native app was opened on the same imported character. Its
  21 starting-scene groups show readable excerpts in the selection sheet.
  Its original default selections were retained during verification.

## Choice headers, icons, and home cards

- The shared choice-sheet handle, title, close button, and optional footer are
  outside the scrollable radio list, following the anatomy and scroll behavior
  in [SEED Bottom Sheet](https://seed-design.io/components/bottom-sheet). The
  mobile sheet retains the requested 70% height; keyboard focus scrolls the
  selected row within the list.
- Verified on the native 394px-wide window: scrolling three pages from the
  first scene to scenes 16–20 keeps the header at the same vertical position
  and the close action available. Mobile scrollbars remain hidden.
- Home cards and their search results show only thumbnail and title. The gray
  description and recent-chat caption were removed. Chat history keeps its
  timestamps.
- The home tab has an optical size correction for its narrower Lucide glyph.
  Create uses [Lucide CirclePlus](https://lucide.dev/icons/circle-plus); selection
  fills the circle and retains a contrasting plus. Stroke treatment stays shared.
- Browser QA at 394 × 852 confirmed image/title cards, the home/create icons,
  and name/description editing followed by automatic selection of the saved
  persona. Temporary viewport settings were restored.
- The rebuilt native app also opens the Add persona page from its empty
  catalog and returns to the setup when the blank editor is closed. No test
  persona or conversation was saved in the native data store.

## Choice entrance jump follow-up, September 10

- Default `focus()` on the entering option allowed browser-driven ancestor
  scrolling to interfere with the sheet transition. Focus now prevents that
  scrolling; revealing an offscreen option changes only the positioned list's
  scroll offset using local layout coordinates. The overlay clips its animation
  without becoming a scroll container.
- The selected-option focus regression fails against the previous code and
  passes with this correction. All 16 targeted focus, selection, sheet motion,
  and conversation-start tests pass, as do frontend checks, architecture/IPC
  checks, and the Tauri debug build.
- Browser measurements show entrance positions moving toward the 256px resting
  position without an upward overshoot and with outer scroll remaining zero.
  At 320 × 568, End focuses the last persona and moves only the list to 134px;
  the outer scroll remains zero. Temporary viewport overrides were reset.
- The rebuilt native app was restarted after the Mac was unlocked. Persona and
  starting-scene choices were checked again with the imported character.

## Expandable text and selection sheets, September 10

- Name, description, and other text-field editors now open from below at 70%
  of the available height. They share the choice sheet's handle, title/close
  header, 16px outer gutter, and fixed footer. Text alone scrolls; Done remains
  available at the bottom. Search keeps its search page and the chat composer
  retains its existing field-to-fullscreen morph.
- Following [SEED's handle and snap-point pattern](https://seed-design.io/components/bottom-sheet),
  the handle follows an upward drag and settles at full height. A downward drag
  from full height settles at 70%; another downward drag dismisses. The user's
  requested full-height endpoint overrides SEED's general 90% recommendation.
  A handle click toggles height, and keyboard Up/Down offers the same actions.
- Text and choice sheets use the same gesture implementation, including logical
  coordinates for scaled layouts, velocity thresholds, cancelled gestures,
  resize cleanup, trailing-click suppression, and reduced motion. Native button
  activation is preserved on pointerdown.
- Dirty text drafts remain mounted below the screen while the discard dialog
  appears over the prior page. Continue returns that draft vertically. Saves
  block repeated Done and dismissal until accepted or rejected. The outro's
  completion notification runs outside its teardown batch so the latest request
  state is read and the previously covered page can restore focus.
- At 394 × 852 CSS pixels, browser measurements showed the sheet at y=255.6,
  height=596.4 before dragging; full height was y=0, height=852. Downward release
  restored the first geometry, and the next downward drag dismissed it and
  focused its opener. Outer scroll stayed at zero. Persona entrance positions
  approached y=256 monotonically (278, 268, 262, 261, 258, 256).
- The rebuilt native app was also checked with real pointer input: name editing
  expands, collapses, and dismisses; the original name remains unchanged. The
  imported card's scene list scrolls through scenes 16–20 while the header stays
  fixed, and the same handle expands this scrolled list to full height. No test
  conversation or persona was saved in native storage.
- All 154 Vitest files / 1,040 tests passed, including Lua and regex sandbox
  regressions. After the native pointer adjustment, the 30 targeted sheet,
  editor, focus, and selection tests passed again. Frontend checks and the
  Tauri debug build passed; the two existing legacy ChatPane warnings remain.

## Validation

- Frontend checks: formatting, i18n literal ratchet, ESLint, and Svelte typecheck
  pass. Two pre-existing legacy `ChatPane.svelte` capture warnings remain.
- All 152 Vitest files / 1,031 tests pass, plus Lua and regex sandbox regressions.
  Added coverage includes deferred departure/return, retained setup values,
  persona-before-entry order, duplicate-start guard, stale results, retry after
  lost acknowledgement, and default-title numbering.
- Additional coverage checks the empty-catalog Add action, full-screen persona
  creation and exact saved-ID selection, and post-sheet navigation/focus order.
- Source architecture and IPC code-generation checks pass; no size cap raised.
- Tauri debug app build succeeds. Verified on the running app with the imported
  character: dirty setup swipe leaves its profile visible, Continue restores the
  story selection, and Discard stays on that profile. No test conversation was
  created in the native data store.
- Browser QA at 394 × 852 also verifies the previous-page background, the return
  transition, persona choice, and titled starting-scene options.
