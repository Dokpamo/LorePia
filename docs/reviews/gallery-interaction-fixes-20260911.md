# Gallery and home loading fixes — UI-GALLERY-20260911

- Baseline: `211f1c6f0770485f957beed3d31229211e1db3a3`; branch
  `codex/retire-legacy-ui-20260910` with completed, uncommitted legacy retirement.
  Preserve that work. This task is an authorized behavior fix, not further retirement.
- Scope: `CharacterProfileHero`, `image-gestures`, `ProfileThumbnail`,
  `CharacterLibrary`/its WorkspaceApp binding, trusted asset presentation loading,
  and their regression tests. Keep current layout, palette, entry points and data.
- Invariants: existing image order, original-image viewing, vertical viewer dismissal,
  keyboard navigation, interactive page-back/cancel, scroll position, teardown and
  stale-request guards. Native asset verification, quotas, canonical URLs, IPC and
  storage contracts remain unchanged (ADR 0005).
- Findings: hero pans ignore bounds and consume the first-image back gesture;
  the empty-library CTA renders during bootstrap and list loading; thumbnail
  observers disconnect after first visibility, and descriptor resolution treats
  recoverable native hash-budget exhaustion (128 cold assets/minute) as permanent.
- Changes: bounded elastic carousel edges and first-image gesture handoff; show
  empty state only after library readiness; visibility-bound thumbnails and bounded,
  cancellable retry of recoverable asset resolution. No symbols moved/renamed and
  no dependency/schema/API changes. Expected addition below 450 production lines.
- Evidence before edits: 33 targeted tests passed. Re-run asset, gallery, gesture,
  bootstrap and full frontend checks, architecture checks, then native UI smoke.
- References: https://seed-design.io/foundations/motion and
  https://seed-design.io/foundations/feedback (input-following motion and the
  existing 300ms enter curve for page-scale settling).

## Verification

- Full frontend suite: 123 files / 671 tests passed, plus Lua and regex worker
  regressions. Final pointer-handoff/ARIA adjustment: 3 files / 6 tests passed.
- Native debug bundle built successfully. Startup shows the stable header and
  tabs without the old empty-library CTA, then displays the stored cards.
- Native smoke: carousel paging and keyboard Home/End, last-photo resistance,
  first-photo rightward back navigation, and the affected 278-image album's
  first/middle/final viewport. The visible thumbnails decoded successfully.
- Assets keep the four-request presentation queue and native limits. Only
  recoverable StorageUnavailable descriptor errors retry; invalid descriptors
  and corrupted/missing assets still fail closed. Hidden thumbnails unmount
  and cancel queued work/retry timers. No data or native contracts were changed.

## Follow-up: confirmed limits and overscroll background

- The previous native smoke verified selected viewports, not uninterrupted fast
  browsing of the entire album. The user reproduced blank images after that check.
- Read-only verification of all 278 affected album files: every CAS file exists,
  its digest and length match the stored descriptor, and all 278 are valid WebP
  containers despite the imported `.png.png` names. macOS ImageIO fully decoded
  all 278 (479×700 or 700×700) without errors. No user files were changed.
- A temporary native count-only probe reproduced the blank album during rapid
  scrolling and recorded 46 rejected cold verifications at `used=128 limit=128`.
  This proves exhaustion of the rolling one-minute verification budget in this
  scenario. Immediate retries cannot reset that native window.
- A regression test also reproduced queue starvation: four recoverable requests
  held all four presentation slots during retry sleeps and prevented an available
  fifth asset from loading. Slots now cover in-flight IPC only; retry sleeps
  yield their slot. Visible album thumbnails distinguish automatic retry waiting
  from an unexplained initial-letter tile. The native quota remains unchanged,
  so fast browsing can still wait for the next verification opportunity.
- Separately, the `연결 확인` test card has a 70-byte `avatar.png` with an invalid
  IDAT CRC; the native protocol rejects it with 415. This is a different issue
  from the 278-image album and retrying cannot repair its bytes. Its immutable
  stored content is preserved; it has not been silently rewritten.
- Remove the hero image backplate. The edge reveals the paper surface (white in
  light appearance); the lower blur moves and settles with the last image so it
  cannot leave another image behind. Interactive back behavior remains unchanged.
- Temporary native probe code was removed after reproduction. No Rust, asset
  policy, dependency, API, schema or existing data changes are part of this fix.

### Follow-up verification

- Full frontend: 123 test files / 672 tests passed; Lua and regex worker checks
  passed. Final catalog placement change: 6 files / 39 targeted tests passed.
- `npm run check`: formatting, i18n, lint and both type checks passed with zero
  Svelte errors or warnings. Architecture and strict context checks passed.
  The new message lives in the existing workspace catalog; no size cap was raised.
- Native final debug bundle built. Rapidly traversing the 278-image album showed
  explicit retry-wait tiles at the far end. Leaving the viewport untouched let
  every visible tile recover into its image on a later retry. The quota-induced
  delay remains; this is not a claim of immediate, unlimited album loading.
- Native original-image open/close and last-image elastic return checked. The hero
  regression verifies the blur follows the same bounded drag offset and restores
  with it, with no stationary image backplate in the carousel.

## Follow-up: avoid spending the budget on transient viewports

- Task `UI-GALLERY-VISIBILITY-20260911`, same baseline and dirty worktree as
  above. Preserve completed work and the native asset contract and limits.
- Reproduced again in the native 278-image album: nine consecutive page scrolls
  took about 1.4 seconds and left every visible tile waiting. Keeping that exact
  viewport stationary eventually displayed the images. The retry completed,
  but immediately resolving every briefly intersecting thumbnail spent the
  native cold-verification budget before the final viewport could load.
- Target `ProfileThumbnail` visibility lifecycle and a shared scroll-settling
  helper, with colocated tests. Defer new thumbnail work until the containing
  scroll surface settles; cancel deferred and retry work when it leaves view.
  Already mounted visible media must not flash or reload during scrolling.
- Preserve exact descriptors/URLs, the four-request queue, fail-closed errors,
  image order, original viewing and native limits. No new IPC, dependency,
  durable data or contract changes. Expected addition under 140 production lines.
- Verify fast traversal requests only the final viewport, stable visible media
  is retained, disposal cancels timers, and native rapid-scroll/revisit flows.

### Scroll-settling verification

- The new regression failed against the previous implementation: a fast pass
  through the first 252 thumbnails made 252 descriptor requests. It now makes
  zero for those transient viewports and 26 for the final viewport after scroll
  events stop. Unmounting before the deferred start makes no request and leaves
  no timer. Already mounted media inside the visible region remains mounted.
- New work waits for 150ms of scroll quiet using one shared capture listener;
  the nearby loading margin is one partial row (80px). Offscreen work is still
  cancelled. Native checks and all limits remain unchanged.
- Full frontend suite: 123 files / 674 tests passed, plus Lua and regex worker
  regressions. Formatting, lint, i18n and type checks passed with zero Svelte
  errors/warnings. Architecture and strict context checks passed.
- Native bundle rebuilt and restarted. The same nine consecutive downward
  page scrolls now displayed the final viewport's images in the immediate
  screenshot (about 1.1 seconds for the actions), instead of all waiting tiles.
  Four more downward page scrolls reached the final images successfully; opening
  an original and closing it restored the album position. Thirteen upward page
  scrolls returned to the first images without long waiting tiles.
- This fixes wasted verification during a fast traversal. Deliberately viewing
  more than 128 distinct cold assets inside one minute can still reach the
  existing native budget; this change does not claim to remove that constraint.

## Follow-up: account for verification work, not album length

- Task `ASSET-DELIVERY-PROGRESS-20260911`, baseline and existing dirty UI work
  unchanged. The user reproduced the remaining waiting state and asked why the
  limit exists and whether file damage or format incompatibility could cause it.
- The previous scroll fix does not fix deliberate browsing beyond 128 cold
  assets. Treat this as an asset-delivery behavior correction, not a refactor.
- Read root/frontend/Storage guides, ADRs 0001–0003/0005/0006, Storage facade
  and public API audit, the cache, both Storage cold-resolution callers, Core
  and Shell asset facades, and renderer retry/component tests before editing.
- Target the private verification admission accounting and its Storage call
  sites, plus renderer retry wake/deadline behavior and their tests. Preserve
  descriptor/URL/API/schema, exact hash and MIME checks, same-handle identity,
  no-follow opens, 128 handle leases, and native response/concurrency bounds.
- Replace the one-minute image-count cliff with bounded work: charge file size
  with a 128KiB minimum per cold verification, allow at most 64MiB initially,
  and refill 8MiB per second. This lowers the maximum hash-byte allowance from
  up to 8GiB/minute (128 × 64MiB) to 544MiB in any minute including a full burst.
  Tiny-file work is also finite: at most 512 initially and 64 per second after
  that. Reject over-limit descriptors before hashing; do not bypass verification
  or cache renderer approvals. Document the changed accounting in ADR 0005.
- Relevant evidence: existing cache baseline 6/6 passed. Validate a 278-image
  cold album, exhaustion/refill and frequent rejected retries with injected
  monotonic time, cache-hit/expiry and tamper behavior, resume from a background
  renderer, terminal error cleanup, native real-album traversal, and normal gates.
- No symbol move/rename or dependency change is needed. Expected new production
  source below 180 lines; semantic risks are admission arithmetic, permanent
  oversize errors, and stale callbacks on visibility/selector changes.

### Follow-up: remove artificial loading feedback

- The user reported that the 150ms scroll-quiet delay and gray waiting blocks
  made the gallery feel slower. This supersedes the earlier scroll-settling
  approach: remove that helper and start visible work immediately, with a zero
  observer margin. Offscreen queued/retry work is cancelled; the native calls
  remain capped at four even if a consumer cancels or times out.
- Keep already decoded thumbnail elements for nearby back-and-forth scrolling,
  bounded globally to 64 elements and an estimated 64MiB of encoded bytes plus
  decoded pixels. This is a presentation lifecycle cache, not a cached approval
  or descriptor. Release on teardown, identity/client change, viewport entry,
  or least-recently-used eviction. Unloaded images cannot occupy that cache.
- Remove gallery initial-letter tiles, gray loading covers and visible retry
  prose. Keep screen-reader loading status and actual terminal error text. The
  image pixels can display as soon as the WebView paints them; no loading fade,
  minimum delay or scroll debounce hides them.
- Reference: https://seed-design.io/foundations/feedback and
  https://seed-design.io/foundations/motion. Immediate feedback is the relevant
  principle. SEED's pressed-animation timing is not a reason to delay an image
  request; these cache and loading decisions are specific to this gallery.
- Read-only audit of the current card's 1,396 unique images: every file exists,
  hash and length match its descriptor, and actual format matches stored MIME
  (1,393 WebP and 3 PNG). All PNG chunk CRCs are valid. macOS ImageIO decoded and
  drew every image successfully. This checks source compatibility independently
  of the renderer; it is not a claim that all images were inspected visually.

### Final behavior verification

- The old native count policy failed the 278-small-image regression at index 128.
  The byte-work budget passes it along with exhaustion/refill, fractional-time,
  burst-cap, lease-expiry, stable-cache-hit and tamper checks. The Storage test
  additionally imports an actual oversized synthetic image and rejects all three
  delivery entry points permanently before hashing, without disabling immutable
  descriptor triggers or changing any live data.
- The old renderer failed the suspended-background wake regression and left a
  never-resolving IPC call pending. Both now settle as specified; returning to
  the foreground retries immediately, while deadline/cancellation does not
  release native concurrency before the underlying call ends.
- Final frontend suite: 124 test files / 679 tests passed, plus Lua/regex worker
  checks. Formatting, i18n, ESLint, TypeScript and Svelte checks passed with zero
  Svelte errors or warnings. IPC generation, source architecture and strict
  context checks passed. No source-size cap was raised.
- Core asset tests: 4 passed; Shell asset tests: 5 passed; Tauri asset protocol
  tests: 19 passed. Storage clippy with warnings denied passed.
- Rebuilt and restarted the actual native debug app. Opened the current card's
  278-image album and scrolled one page at a time with a screenshot after every
  action, including every middle/late viewport beyond the old count cutoff.
  All visible thumbnails displayed at capture; no gray initial/wait tiles or
  terminal errors remained. Reached the final two images, moved back one page
  and forward again, and opened the 278th original successfully.
- Closed the original, then made 13 consecutive upward page scrolls. The first
  viewport displayed its images in the immediate screenshot. This verifies
  native browsing at this device/album size, not a promise of zero network or
  decode latency on every device or for unlimited assets.
- Removed the temporary local audit inventory containing private CAS paths.
- Full `cargo test -p lorepia-storage` completed successfully, including integration
  and doc tests. Final `cargo fmt --all --check` and `git diff --check` passed.
  The rebuilt native app remains open on the verified album for review.

## Follow-up: scrolling continuity and gallery edge spacing

- Task `UI-GALLERY-SCROLL-20260911`, same baseline and pre-existing dirty worktree.
  The user sees flicker during scrolling and wants narrower image-grid outer
  gutters, with the existing gaps between images preserved.
- Read thumbnail mounting/retention, image rendering, album/group composition,
  header scrolling, gestures and the associated tests. Compare native scrolling
  and viewport transitions; do not infer motion continuity from settled-only
  screenshots. Preserve immediate foreground loading and all native asset checks.
- Narrow album and image-group outer gutters from 24px to 12px within their
  existing page. Preserve album gaps of 8px and group gaps of 12px, three columns,
  title/navigation alignment and the edge-to-edge original viewer.
- Reference: https://seed-design.io/foundations/spacing. Use its spacing scale and
  distinction between gutters and component gaps; 12px here is the user-requested
  gallery density choice, not a claim that SEED prescribes it globally.
- Replace per-thumbnail, document-root visibility observers with one observer per
  actual scrolling container and a 200px preparation margin. A document-root
  margin cannot extend the clipping edge of a nested scroller. Nearby rows now
  start preparing before they become visible, without a timer or loading cover.
- Process entering thumbnails before departing thumbnails, first removing every
  entering image from the offscreen eviction candidates. This closes the race
  where an earlier exit callback could evict an image that had already returned
  to the viewport but had not received its own observer callback yet. Preserve
  the existing bounded decoded-image retention and native concurrency limits.
- Added regression coverage for the clipped viewport, entering-image eviction
  race, shared-observer teardown and late callbacks. Existing immediate-load,
  cancellation, decoded-node reuse and changed-identity tests still pass.
- Verification: 125 frontend test files / 682 tests passed, plus both worker
  checks. The new visibility tests also passed after their strict-TypeScript
  fixture correction. Full frontend check passed with zero Svelte errors or
  warnings; source architecture, IPC generation and diff whitespace checks passed.
- Rebuilt and restarted the native debug app. The 394px-wide album capture shows
  12px left/right gutters, three 118px images and 8px gaps. Image groups also use
  12px outer gutters and retain their existing 12px internal horizontal gaps.
- Native verification included three short scroll/direction changes, three
  consecutive full-page downward scrolls, and an immediate upward page return
  in the 278-image album. Images remained populated in the immediate captures.
  This fixes identified thumbnail lifecycle causes; the user's broader wording
  about whole-screen flashing was not separately reproduced or established by
  these captures. The actual rebuilt app remains open for review.

### User-reported continuation: preparation before scrolling

- The user still sees loading/flicker after the previous narrow-buffer change.
  Keep this as an unresolved report until broader preparation is verified. The
  additional spacing correction makes album gutters and internal gaps share the
  same 8px value; image groups keep their shared 12px value.
- Scope the follow-up to thumbnail visibility/retention, the existing asset
  request queue and image decode lifecycle. Prepare several viewport heights,
  dynamically prioritize currently visible requests over queued preloads, and
  finish decoding before marking media ready. Preserve native checks, canonical
  URLs, byte bounds, request cancellation and four real in-flight native calls.
- References: https://seed-design.io/foundations/feedback and
  https://developer.mozilla.org/en-US/docs/Web/API/HTMLImageElement/decode.
  Preparing the image ahead of presentation addresses real rendering work; no
  minimum loading duration, placeholder animation or approval cache is added.
- Preparation now extends two scrolling-container heights both above and below
  the viewport (two widths for the horizontal strip). Resize replaces the
  observer with a generation check, so old queued callbacks cannot unload media
  after the preparation region grows. The previous bounded retained-image cache
  remains outside this prepared region; no native limits were relaxed.
- The shared request queue re-evaluates each thumbnail's distance whenever a
  native slot opens. Visible images overtake old preloads after scrolling or
  reversing direction. Background-only mounts collect for one microtask so they
  cannot occupy the first slots ahead of visible children in the same render;
  foreground requests still dispatch immediately. No elapsed-time delay is used.
- Trusted images use asynchronous decoding and call `decode()` after load, while
  they are still ahead of the viewport. Only the same connected image/descriptor
  may report readiness or decode failure. Errors keep their existing retry/failure
  handling; stale decode completion cannot mark a replacement image ready.
- Verification: 125 frontend test files / 687 tests passed, plus both worker
  checks. The affected final asset/visibility suite passed 38 tests after fixture
  typing/lint corrections. Full frontend check passed with zero Svelte errors or
  warnings. Architecture, IPC generation and diff checks passed.
- Rebuilt and restarted the actual native app. Confirmed the album's outer and
  internal spacing are now both 8px. Checked the first two subsequent screens,
  immediate reversal, 13 consecutive downward page scrolls to the last images,
  one page back, then 13 consecutive upward page scrolls to the start. Images
  were populated in the immediate captures at each inspected point. No extra
  wait or loading cover was introduced. This is device/album-specific observed
  verification, not a claim of zero latency for arbitrary jump distances or
  damaged assets. The updated app is open on the album's first screen.

### Follow-up: consistent home, image-group and album geometry

- User requests the same outer and internal image spacing across all three
  screens, including Home, and smaller titles. The previous album value was 8px,
  image groups 12px, and Home used a responsive outer gutter with 16px gaps.
- All three image grids now share `--seed-image-grid-gap: 8px` for outer gutters
  and horizontal spacing. Home and image-group rows retain room for their labels
  and share a 20px row gap; the unlabeled album has an 8px row gap.
- Root navigation titles move from the viewport-scaled 20–24px value to the
  existing 20px title role, and resource-page titles move from 22px to 20px. Home
  card names move from 16px/700 to 14px/600, matching image-group names. Text-scale
  preferences and the existing navigation hit targets are preserved.
- References: https://seed-design.io/foundations/spacing and
  https://seed-design.io/foundations/typography. Apply shared spacing and type
  roles; the compact grid values are the user's requested density, not a claim
  that SEED mandates these exact values for every screen.
- Verification: full frontend check passed with zero Svelte errors or warnings;
  the native UI build and diff check passed. Restarted the built macOS app and
  compared Home, image groups and the 278-image album at the same 394px content
  width. All three show aligned 8px outer gutters and horizontal gaps, with
  approximately 121px square images. The smaller headings and Home card names
  are visible, and the three-column layout remains intact. The updated app is
  open on the album for review.

### Follow-up: preserve profile preview's content gutter

- The user wants the inline image preview in character information to align
  with the surrounding content again. Limit the image-group grid's expanded
  margins to standalone resource pages, so the inline preview inherits the
  profile's existing responsive content gutter. Home and dedicated image
  groups/albums retain their shared 8px outer spacing and image gaps.
- Verification: frontend check passed with zero Svelte errors or warnings;
  native UI build passed. In the rebuilt macOS app at 394px content width, the
  inline preview aligns with its heading and surrounding body at approximately
  24px from both sides. Opening the separate image-group page still shows 8px
  outer gutters. Both views retain three columns.

### Follow-up: quiet image and character-card presses

- Before committing, the user requests no hover/click highlight on image
  thumbnails or Home character cards. Remove these surfaces from the shared
  button press treatment, including image groups, album cells and the viewer's
  thumbnail strip. Keep touch tap highlights transparent. Click actions, focus
  indicators, image gestures and the strip's centered selection remain intact.
- Verification: 13 existing profile/album/strip/accessibility tests and both
  worker checks passed. Full frontend check reports zero Svelte errors or
  warnings, and the native UI build passed. In the restarted macOS app, Home
  card selection, image-group opening, album-image opening and selection of a
  neighboring viewer thumbnail all work; the strip still centers and enlarges
  the selected image.
