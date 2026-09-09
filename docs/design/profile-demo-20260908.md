# Profile demo fixtures — 2026-09-08

Task UI-PROFILE-DEMO-20260908, baseline c49a766. Preserve the uncommitted
profile layout work. Remove the profile header's painted backdrop: the portrait
continues behind a single Back icon. Retain the lower title blur and 4:5 frame.

Create four deterministic browser-only characters with bundled original artwork,
creator identity, long-form descriptions, guide, three starting situations and
inspectable lore/script/transform resources. Selected greetings must actually
seed a new demo conversation. Demo replies are canned, not provider requests;
recreating the client resets mutations. No native data, migrations or IPC changes.

Owned targets: preview data/client/tests, bounded fixture modules and media;
CharacterImage/CharacterOverview's presentation injection and profile CSS. An
explicit Svelte mount context supplies a fixture-only image component for the
isolated browser entry. Native entries do not set this context and continue to
use CharacterAvatar/TrustedAsset unchanged. Do not alter their descriptor, URL,
protocol, MIME or digest validation, and do not spoof Tauri globals/protocols.

Expected growth: approximately 700 lines across bounded fixture modules and
presentation wiring, plus generated media. No extraction or baseline increases.
Read-only sample scripts are visible in profile resources; scoped chat profiles
omit executable fixture scripts so the preview does not claim to execute them.

Verify fixture identity/isolation, selected opening content, resources, image
loading, transparent header, gallery/detail/back, and chat setup/send. Run the
frontend check, focused tests, architecture gate and native UI build.

## Generated artwork

Created with the built-in image_gen tool, without the CLI or an API key. The
original generated files were preserved; copies below are bundled exclusively
by the browser preview entry. All four portraits are original fictional adults.
The four illustrated characters also appear as supporting characters in the
resource galleries. Creator avatars use their chosen cover art as an avatar.

### noa

Asset: `apps/lorepia/src/preview/media/noa.png` (1122 × 1402).
Rendered in a 4:5 frame with CSS object-fit; no bitmap edits.

Prompt:

> Use case: illustration-story. Project asset: original fictional character cover for LorePia chat app test data. Make one polished full-bleed 4:5 portrait illustration, 1024x1280 desired. Cohesive elegant painterly anime-inspired illustration with fine clean face anatomy, tactile brush texture, restrained natural lighting, rich environmental detail, no logo, no lettering, no frame, no UI. Adult character age 25+. Upper-left corner should contain quiet light background so a small black back-arrow UI can overlay it; do not draw the arrow. Position head around upper-middle with ample headroom, upper-body figure fills middle; lower fifth remains environmental detail for a title overlay. An original thoughtful young adult painter with short warm brown wavy hair, kind expressive eyes, cream linen shirt and muted pistachio apron. They sit in a sunlit atelier holding a sketchbook. Open window, plants, glass jars of brushes, warm off-white walls, unfinished landscape painting. Calm dreamy literary mood, sage, buttercream and terracotta palette.

### aria

Asset: `apps/lorepia/src/preview/media/aria.png` (1122 × 1402).
Rendered in a 4:5 frame with CSS object-fit; no bitmap edits.

Prompt:

> Use case: illustration-story. Project asset: original fictional character cover for LorePia chat app test data. Make one polished full-bleed 4:5 portrait illustration, 1024x1280 desired. Cohesive elegant painterly anime-inspired illustration with fine clean face anatomy, tactile brush texture, restrained natural lighting, rich environmental detail, no logo, no lettering, no frame, no UI. Adult character age 25+. Upper-left corner should contain quiet light background so a small black back-arrow UI can overlay it; do not draw the arrow. Position head around upper-middle with ample headroom, upper-body figure fills middle; lower fifth remains environmental detail for a title overlay. An original adult woman librarian with dark chestnut shoulder-length hair, gentle inquisitive eyes and a soft midnight-blue cardigan over an ivory blouse. Holding an old cloth-bound book in a tall library with winding wooden stairs, astronomical globes and rows of weathered books. Moonlight with warm lamplight. Quiet, mysterious, welcoming mood.

### kai

Asset: `apps/lorepia/src/preview/media/kai.png` (1122 × 1402).
Rendered in a 4:5 frame with CSS object-fit; no bitmap edits.

Prompt:

> Use case: illustration-story. Project asset: original fictional character cover for LorePia chat app test data. Make one polished full-bleed 4:5 portrait illustration, 1024x1280 desired. Cohesive elegant painterly anime-inspired illustration with fine clean face anatomy, tactile brush texture, restrained natural lighting, rich environmental detail, no logo, no lettering, no frame, no UI. Adult character age 25+. Upper-left corner should contain quiet light background so a small black back-arrow UI can overlay it; do not draw the arrow. Position head around upper-middle with ample headroom, upper-body figure fills middle; lower fifth remains environmental detail for a title overlay. An original adult man mechanic with tousled charcoal hair, olive work jacket over a white shirt, a small practical repair tool in one hand. A moody retro repair workshop at dusk, radios and brass instruments on the workbench, warm lamps, soft pale window light. Reserved but kind expression. Dusty teal and warm amber palette.

### sera

Asset: `apps/lorepia/src/preview/media/sera.png` (1122 × 1402).
Rendered in a 4:5 frame with CSS object-fit; no bitmap edits.

Prompt:

> Use case: illustration-story. Project asset: original fictional character cover for LorePia chat app test data. Make one polished full-bleed 4:5 portrait illustration, 1024x1280 desired. Cohesive elegant painterly anime-inspired illustration with fine clean face anatomy, tactile brush texture, restrained natural lighting, rich environmental detail, no logo, no lettering, no frame, no UI. Adult character age 25+. Upper-left corner should contain quiet light background so a small black back-arrow UI can overlay it; do not draw the arrow. Position head around upper-middle with ample headroom, upper-body figure fills middle; lower fifth remains environmental detail for a title overlay. An original adult woman starship navigator with a neat silver bob, alert warm eyes, simple practical burnt-orange flight jacket with no insignia. In a tranquil spacecraft observation lounge with a curved window showing distant stars and a softly glowing planet. Pale softly lit bulkhead at upper left, rich indigo environment, subtle jade instrument light, expansive adventurous mood.

## Verification

- Frontend format, i18n, lint and type checks passed. Two existing ChatPane
  controller-capture warnings remain.
- Six focused test files, 22 tests passed, including all twelve opening choices,
  invalid greeting rejection before mutation, fixture cloning and reset, media
  availability, resource inspection, and deterministic reply/event order.
- Portable Lua timeout and regex worker regression checks passed.
- Source architecture gate passed without changing any baselines.
- Native UI app built successfully.
- Browser checked at 390 × 844: portrait 390 × 487.5 (4:5), transparent header,
  loaded local art, fixed Start button. Selected the third Noa opening, created
  a conversation and sent a message; the selected opening and canned Noa reply
  appeared in the transcript. Opened image, lore and script detail views and
  returned to the profile.

Try it at http://127.0.0.1:5176/ui-preview.html. Each reload restores the four
original demo characters and conversations. Replies are fixture text, and sample
scripts are read-only. Native user data and provider requests are untouched.
