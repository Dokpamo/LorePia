# SEED four-tab redesign

Task ID: UI-SEED-FOUR-TABS-20260908.

Baseline: checkpoint commit `2200173`; the worktree was clean immediately after
the user's requested checkpoint. The checkpoint intentionally preserves the
complete preceding work, including Rust changes, without claiming merge readiness.

This is an authorized UI feature redesign, not a behavior-preserving extraction.
The live entry remains `src/main.ts` and the root application/controller lifetimes
remain in `app/workspace/WorkspaceApp.svelte`. The preview injects its existing
in-memory client into the same composition. No native contract, dependency, schema,
asset boundary, provider dispatch, or durable storage change is required.

Owned paths: new `ui/navigation/` presentation components, shared workspace view
composition, frontend navigation catalog/controller, preview UI entry, related
locale keys and navigation tests. Existing chat runtime, approved assets, streaming,
drafts, input growth, choice/editor guards and controller cleanup must remain intact.
There are no public symbols to move or rename. Expected size delta is approximately
1,500–2,000 lines across bounded new components and styles; no baseline increases.

Baseline validation: WorkspaceApp, WorkspaceNavigation and WorkspaceSettings
integration tests: 16 passed. Architecture measurement is recorded in
`/tmp/lorepia-seed-baseline-architecture.txt`.

## Information architecture

- Home: character library, import, character information and resources.
- Chats: all conversations, character filtering, reopen and new conversation.
- Create: character and reusable material creation (confirmed by the user).
- Settings: app appearance/language, AI connections and defaults, plugin installation,
  personas and storage. Runtime use remains accessible from the active conversation.

Root tabs are peers, preserve their list state, and never enter the back stack as
if they were child pages. Chat and editing are detail destinations with a stable
back slot. Selecting a conversation from the global list resolves its character
through the existing controller before opening it. Stale async results may not
change the user's current destination. Tab navigation must not lose an active draft.

## SEED application

Sources read on 2026-09-08:

- https://seed-design.io/components/bottom-navigation
- https://seed-design.io/components/top-navigation
- https://seed-design.io/components/list
- https://seed-design.io/components/bottom-sheet
- https://seed-design.io/foundations/typography
- https://seed-design.io/foundations/spacing
- https://seed-design.io/foundations/layout
- https://seed-design.io/foundations/motion
- https://seed-design.io/foundations/feedback

Apply four equally sized, labeled navigation items; constrain their inner width
on desktop. Root headings and detail headings use separate patterns. Detail back
controls use the same slot and touch area. Use role-based system typography
(22/30 page, 18/24 navigation, 16/22 body, 14/19 detail), a shared spacing scale,
16px list gutters, quiet neutral selection and pistachio primary actions.

Small press feedback uses coordinated color/scale timing (150ms); page and sheet
entry use 250–300ms, dismissal 200ms, respecting reduced motion. Flat list rows
replace repeated decorative cards. Root surfaces reflow to their container; resizing
does not animate global scale or typography. No financial-app branding or assets
are copied. Preserve the existing pistachio palette.

All application SVG icons come from the existing `@lucide/svelte` dependency,
including the four bottom-navigation items (https://lucide.dev/guide/svelte).
The user's icon-source requirement takes precedence over using a custom filled
navigation family. Create and Settings rows use plain 22px leading icons in a
24px slot with a 12px text gap, without a persistent gray backplate. Icons and
row text remain inside the same press-feedback surface.

The later request for an iOS 26-style bottom bar applies only to root navigation.
Apple's [WWDC25 tab-bar guidance](https://developer.apple.com/videos/play/wwdc2025/284/)
informs a floating capsule above scrollable content. This Tauri renderer uses a
translucent CSS material rather than native UIKit Liquid Glass. A neutral selection
capsule moves between the four persistent Lucide items; existing root state and
detail/back behavior remain unchanged. Scroll end padding keeps the last row
reachable. Reduced motion, reduced transparency and higher contrast have fallbacks.

The subsequent screenshot-based chrome adjustment keeps title/actions on one
row, uses the reference's 576:97 capsule proportion, and aligns visible title and
button geometry at a normalized 390px viewport. See
[the pixel audit](toss-chrome-pixel-audit-20260908.md) for measurements and the
intentional difference between the reference's host-back control and our four tabs.

## Toss UX application

Source: https://developers-apps-in-toss.toss.im/design/consumer-ux-guide
Read on 2026-09-08. Apply predictable actions, explicit action labels, contextual
feedback, an unobstructed first screen, an available back/exit path, and concise
active Korean copy. This is a standalone Tauri app, so the Apps-in-Toss floating
navigation and host branding requirements are not transplanted. SEED owns the
root/detail navigation geometry and component specifications. Existing native
character entry is import-based; material editing uses existing revisioned CRUD.
No proprietary Toss illustrations or graphic assets are copied.

## Verification scope

Run new navigation/catalog tests, shared workspace integrations, frontend check,
full frontend tests and architecture/IPC checks. Build and inspect the Tauri app,
including narrow/mobile and desktop widths, root tab state, details/back gestures,
empty/loading/error states, character import, conversations and provider settings.

## Delivered and verified

- Live renderer and browser preview now use the same four-root composition.
  Each root retains mounted list/search state; chat retains its existing composer,
  session draft, transcript, message tools and runtime services.
- The global conversation controller serializes cross-character navigation and
  rejects stale completion after another selection or root-tab change.
- Material authoring uses a separate frontend lifecycle over the existing native
  revisioned document API. It works without a selected conversation. Lorebook
  names, keyword conditions and entry contents use the shared fullscreen editor;
  advanced document fields remain available through explicit JSON editing.
- Character addition currently uses the supported native card import/review path.
  Direct character-card authoring is not implemented by the existing native API;
  this change does not invent a working save action or silently add an IPC contract.
- Final frontend suite: 130 files, 945 tests passed, plus portable Lua timeout and
  regular-expression worker checks. `npm run check` passed; the two existing
  ChatPane initial-controller-reference warnings remain. No new diagnostics.
- IPC generation and source-architecture checks passed. The Tauri UI bundle built
  successfully. Rust APIs, schemas, permissions and dependency versions are unchanged.
- Browser measurements at widths 320, 390, 768 and 1280: no horizontal overflow;
  shared top navigation, square character images with equal top alignment, and
  bottom-navigation capsule capped at 440px after the iOS 26-style update. Light
  and dark themes inspected.
- The native app was opened and checked against its saved characters, conversation
  and lorebook catalog. Browser theme/viewport test overrides were restored.

The preliminary checkpoint remains `2200173`. The redesign is left as a reviewable
working-tree change; no push or second commit is implied by the checkpoint request.
