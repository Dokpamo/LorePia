# Character author and image groups — 2026-09-09

Task UI-PROFILE-GALLERY-20260909. Baseline c49a766; preserve the prior
uncommitted profile and demo work. This is a UI feature change, not an extraction.

Move the compact author identity above the hero title and use 작가 in Korean.
Keep the note below the work description. Show representative image groups
without a disclosure; selecting one opens an album with a main image and directly
selectable thumbnails. Examples include people, places and items. Existing
Start, back/gesture, 4:5 hero and transparent header remain unchanged.

Targets: profile view components/types/styles, their tests, and Korean/English
navigation messages. Use the user-selected JPEG/CHARX card through the native
import path. Do not generate or check in its media. Display metadata compatibility
also touches Domain content, Content normalization, the existing Storage revision
projection, and Shell API/renderer DTOs. See the additive contract below.
Native asset delivery remains CharacterImage/TrustedAsset. Presentation-only demo
group metadata is validated against the returned asset list. Native aliases can
form groups only through explicit folder or known expression naming conventions;
unclassified assets remain visible. No inferred semantic lore/image bindings.

Rust currently exposes independent assets/aliases and runtime_knowledge lists.
PortableMessage resolves explicit image references against those aliases. Do not
represent a gallery group as a durable relation or claim viewing it triggers lore.
Sample scripts and regex stay inert in this profile and preview conversations.

SEED references: Avatar for compact identity, Image Frame/Aspect Ratio for stable
thumbnail geometry, and existing progressive disclosure for technical content.
https://seed-design.io/react/components/avatar
https://seed-design.io/react/components/image-frame
https://seed-design.io/react/components/aspect-ratio

Comparison: SillyTavern documents world info as conditional prompt content and
expressions as separately managed images. The grouping UI here is our proposal,
not a claimed requirement of either system.
https://docs.sillytavern.app/usage/core-concepts/worldinfo/
https://docs.sillytavern.app/extensions/expression-images/

Expected size delta: bounded new gallery/album/helper modules, ~500 source lines,
plus tests and sample images. No source baseline increases. Verify author order,
group coverage/deduplication, default representative and fallback, album selection
and back focus, image loads/geometry, full frontend check and native build.

## Additive profile display contract

ADR 0006 permits checked-in feature contract evolution. `CharacterContentV1`
adds `creator`, `creator_notes`, and `tags` for CCv2/CCv3 imports. Missing values
default empty. Empty values are omitted during serialization so previously empty
documents keep their canonical shape. Nonempty fields participate in new content
revision identities. The four existing CharacterContentV1 definition fingerprints
in config/core-storage-public-api-baseline.json are updated to that exact additive
shape; no method inventory or size baseline is widened. No migration or mutation of existing immutable revisions;
previously imported cards need reimport to acquire metadata that was omitted.

Content validates the author to 1,024 bytes / 256 characters, notes to the existing
256 KiB / 64 Ki-character text limit, and tags to at most 128 strings of 512 bytes /
128 characters each. These standard fields survive safe-mode import. Unknown
extension quarantine and runtime approval behavior remain unchanged. Storage
retains the fields in the canonical content document and fills the existing
creator_notes projection inside the same transaction.

The existing `get_character_render_profile` response adds inert `creator`,
`creator_notes`, and `tags`, and each asset adds its verified `media_type`. The
renderer accepts absent display metadata from older shells. No command/input,
capability, registry, dependency, or Core/Storage method changes are introduced.
Native asset descriptors and digest delivery still govern viewing; audio/video
assets do not become image thumbnails. Text is displayed as text, never executed
as markup, scripts, regex or prompt instructions.

Long imported descriptions can expand on demand. At most three introductions
appear initially, with access to all existing greeting choices. Thumbnail media
is requested only near the viewport, avoiding eager loading of a large archive.
Image grouping uses full folder identity or stable name prefixes; it creates no
new persisted relationship with lorebooks and does not infer a world entity type.

Validation includes legacy empty serialization, bounded metadata and safe import,
Shell API projection, native import/reopen, complete/deduplicated image grouping,
audio exclusion, lazy thumbnails and author placement. This feature does not
claim full Risu runtime compatibility from successful file inspection alone.

## Verified result

The supplied JPEG/CHARX file was imported through the native file picker and
normal safe-import review. The original dog cover and author `cnxeui` are shown;
the author note is read from the imported card. The app reports 1,411 assets,
41 greeting choices, 69 non-folder lore entries, one script and 31 enabled
display/output rules. Audio remains outside the image gallery. No runtime grant
or model invocation was performed.

Native screenshots verified the transparent hero header, author above title,
visible representative thumbnails and an album of 278 images. Selecting a
different thumbnail changed the main image from item 72 to item 1. Returning
restored the character profile and focus on the originating image group.
The user's native app remains available for interaction with this imported card.

Checks: frontend format/i18n/lint/typecheck (zero errors, two existing ChatPane
warnings), 35 targeted frontend tests, Lua/regex sandbox regressions, Domain and
Content tests, Shell API tests including the new profile projection, five Storage
greeting/restart tests, 13 Core import tests, 195 native shell tests (two existing
ignored tests), the cross-platform golden, scoped Rust Clippy, Rust formatting,
architecture and IPC registry checks. The UI app was rebuilt after the final
typography/button styling adjustment. No production/user-data fixture is copied
into the isolated browser preview or the repository.
