# Conversation setup and deferred back confirmation

## Interaction

- A dirty editor completes the back gesture or back-button transition before
  asking whether to discard changes. The previous page remains visible behind
  the confirmation. The editor stays mounted offscreen, inert, with its draft.
- Continue editing brings that same editor back from the right and restores
  focus. Discard closes it without replaying the departure transition.
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

## Validation

- Frontend checks: formatting, i18n literal ratchet, ESLint, and Svelte typecheck
  pass. Two pre-existing legacy `ChatPane.svelte` capture warnings remain.
- All 152 Vitest files / 1,028 tests pass, plus Lua and regex sandbox regressions.
  Added coverage includes deferred departure/return, retained setup values,
  persona-before-entry order, duplicate-start guard, stale results, retry after
  lost acknowledgement, and default-title numbering.
- Source architecture and IPC code-generation checks pass; no size cap raised.
- Tauri debug app build succeeds. Verified on the running app with the imported
  character: dirty setup swipe leaves its profile visible, Continue restores the
  story selection, and Discard stays on that profile. No test conversation was
  created in the native data store.
- Browser QA at 394 × 852 also verifies the previous-page background, the return
  transition, persona choice, and titled starting-scene options.
