# Profile pagination and opening language — 2026-09-09

Feature task UI-PROFILE-PAGINATION-LANGUAGE-20260909. Baseline c49a766;
merge base 6f22761a4305443541452e7409639114f4c10dfd. Preserve the existing
dirty profile/gallery/reader work. No extraction, dependency or schema change.

References: https://seed-design.io/components/pagination,
https://seed-design.io/components/select,
https://seed-design.io/components/menu-sheet,
https://seed-design.io/foundations/motion.

Keep one stationary dot per hero image, including the first and last. The
user explicitly requests all dots rather than the numbered Pagination
component's ellipsis policy. Reuse its stable-position/selected-state principle.
Use neutral selection, 150ms color feedback and a 300ms image slide, with
reduced-motion support. Each dot selects the corresponding image; arrow keys
and swipes use the same index. Do not change representative-image ordering.

Opening defaults use the app locale, then an explicitly authored recommended
language, then English, then the first available source variant. Match exact
language tags before their primary language. A previous greeting selection may
only break ties within the preferred language; it cannot override this order.
An explicit reader language choice remains local while that reader is open.
Only show a single language selector when multiple languages are available.
Reuse the existing choice sheet, selected checkmark, drag-to-close, focus and
covered-page behavior instead of a segmented pair of buttons.

## Additive metadata contract (ADR 0006)

CCv2/CCv3 may declare `data.extensions.lorepia.recommended_language` as a
bounded ASCII language tag (2–8 letter language, optional 1–8 alphanumeric
subtags, at most 35 bytes). Absent/null/empty means no recommendation; malformed
explicit values fail the existing import validation. Preserve this inert value
as optional `CharacterContentV1.recommended_language` and expose it on the
existing `CharacterRenderProfileDto`. Default to None and omit absent values
from stored content so old companion serialization/hash inputs do not change.
No new command, capability, migration, credential, script or generation path.
Do not infer recommendations from prose, executable extensions or source order.
Previously imported records without this metadata use the documented fallback.

Targets: profile hero/CSS, opening grouping/list/reader, optional presentation
metadata, normalized Domain/Content field, existing render-profile DTO and TS
contract, synthetic fixtures and focused tests. Preserve revision checks,
stale-read guards, canonical asset descriptors and exact greeting IDs.
Verify dot count/selection across distant jumps and swipes, language priority,
selector focus and dismissal, native metadata round trips, frontend checks,
affected Rust checks, architecture ratchets and the running native app.

## Verification

All seven affected frontend test files pass (19 tests across the focused runs),
including a regression for focus restoration after the covered page resumes.
Frontend formatting, i18n, lint and type checks pass; two pre-existing ChatPane
controller-capture warnings remain. Content, Domain and Shell API tests pass,
as do their all-target Clippy checks, Rust formatting, IPC generation and source
architecture. The explicit CharacterContentV1 contract inventory was updated;
no source-size cap was raised. Native UI builds succeed.

In the already imported native card, all 29 dots are visible immediately once
the render profile loads. Clicking dot 3 displays its map image and selects dot
3; a left swipe displays image 4 and selects dot 4 without moving other dots.
Distant selections crossfade without sliding across unloaded intermediate
images. The language reader initially shows Korean, opens one choice sheet,
switches to the exact English variant, retains its checked selection and
returns focus to the language selector on dismissal. Native focus verification
caught a page-uncover race; restoration now runs after the page's focus effect
and does not steal focus from a newly opened sheet.

The imported card was not reimported or modified, and no conversation,
generation or imported script was started. Author recommendation parsing and
locale/recommendation/English fallback cases use synthetic test inputs.
