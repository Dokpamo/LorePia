# Profile reading pages — 2026-09-09

Task UI-PROFILE-READING-20260909. Baseline c49a766; merge base
6f22761a4305443541452e7409639114f4c10dfd. Preserve all existing uncommitted
profile, gallery, compatibility and preview work. This is a feature change.

Move author identity from the hero to immediately before the author note.
Replace inline description and opening-list expansion with ordinary detail
pages using SettingsPanel's existing forward/back gestures, focus and motion.
Keep the covered profile mounted so its scroll and greeting selection survive.
Use a neutral ghost action with a Lucide ChevronRight, a 44px minimum hit area,
and horizontal bleed to align the label with the body text.

Reference: https://seed-design.io/react/components/action-button (ghost/bleed)
and https://seed-design.io/react/components/list (radio list items).

The supplied CCv3 card has `first_mes: "Start"` and 40 alternate strings, without
separate opening-title metadata. Its embedded module does not define a title
catalog either. CCv3 defines alternate_greetings as string[]. Do not manufacture
author-supplied scene titles or execute card code to obtain them.
https://github.com/kwaroran/character-card-spec-v3/blob/main/SPEC_V3.md

## Additive display contract

The existing render-profile response adds optional/default-empty
`greeting_previews`: bounded inert display text keyed by existing greeting IDs.
Each entry has `id`, a nullable `title` (only a leading Markdown heading up to
96 characters), and an `excerpt` up to 320 Unicode characters. Projection scans
at most 8,192 characters per source and at most the default plus 128 alternates.
An absent title keeps the UI's numbered label, followed by the actual excerpt.
If a bracketed leading annotation is separated from the scene by a blank line,
prefer the following paragraph in the excerpt. Keep a preamble-only greeting
visible and preserve ordinary Markdown links. This presentation heuristic never
changes persisted content, greeting IDs, selection or the initial chat message.
It is derived from immutable content in Shell API, never stored or used to start
a conversation. No raw full greeting source is returned by the selector catalog;
its DTO, revision authority, selection IDs and atomic native start stay intact.
The renderer uses native previews only when both character and revision match
the active greeting catalog. Older shells retain numbered-label fallbacks.
No new command, input, grant, migration, Core/Storage API, dependency or source
baseline change. Preview remains synthetic and independent of imported data.

Targets: profile view/components/styles/types, WorkspaceApp's revision prop,
navigation messages, Shell API display DTO/projection and corresponding tests.
Expected delta: small focused view/helper modules; no refactoring-only moves.
Risks: stale revision labels, covered-page keyboard/gesture interception, lost
selection/scroll, huge or markup-heavy imported text, mobile title wrapping.
Verify bounded projection, omitted/empty openings, literal text rendering,
author order, description forward/back focus, all-opening selection/back,
stale revision fallback, frontend checks, Shell API/native tests, architecture,
IPC registry, native build and the imported card in the app.

## Verification

Frontend check passes with zero errors and the two existing ChatPane controller
capture warnings. The seven relevant frontend test files pass (22 tests), with
the four navigation/integration tests repeated after the final text adjustment.
Shell API's full 107-test suite passed before the additional display-edge tests;
all four final preview projection tests pass. The 25 native request/registry
contract tests, scoped Shell API Clippy, Rust formatting, source architecture,
IPC registry and whitespace checks pass. No test fixture contains imported user
content or media.

Rebuilt and reopened the native app with the already imported card. Verified
author-before-note order, the ghost read action, full-description page and back
focus, all 41 openings on their own page, and selecting a previously hidden
opening. Returning retains the scroll position, includes the selected opening in
the three-row summary, and focuses the originating action. Source inspection and
the native UI both confirm that the first two alternates now preview their
distinct Korean/English scene paragraphs, without changing their stored text.
The opening page uses the same 52px CTA and 24px mobile side gutter as the profile.
