# Profile image navigation and play links — 2026-09-09

Feature UI-PROFILE-FILMSTRIP-20260909; baseline c49a766. Preserve the current
dirty profile, resource-page, gallery and native import work. No dependency,
native API, schema, source-cap or asset-policy changes.

Replace management heading/materials/settings rows with chat history and a
character-related plugin page. Use existing read-only module/binding APIs;
never show unrelated character bindings as applicable or execute plugin code.
Retain parent scroll, inert coverage, focus and back navigation.

Add a shared thumbnail strip between hero art and title and beneath the full
image viewer. Multiple representatives remain available. The selected image
stays centered with neighboring previews; swiping, thumbnail selection and
keyboard navigation share the same index. Remove viewer arrows and visible
n/n counter. Keep original-image fit and downward dismissal.

References: [SEED Image Frame](https://seed-design.io/components/image-frame),
[Scroll Fog](https://seed-design.io/components/scroll-fog) and
[Motion](https://seed-design.io/foundations/motion). Apply consistent thumbnail
ratios, 20px edge fades/padding, short selection feedback, direct drag response,
and reduced-motion support. The filmstrip composition is our application of
these principles, not a prebuilt SEED component.

Verify thumbnail/image synchronization, distant jumps, boundaries, keyboard
focus, nested back navigation, lazy asset loading and plugin scope/races.
Check frontend, targeted tests, architecture and a rebuilt native app.

Completed after the earlier resource-page/three-column work was verified.
The same lazy thumbnail strip now serves hero representatives and original
image viewing. It centers the selection without scrolling the parent page;
keyboard, swipe and thumbnail activation stay synchronized. Distant thumbnail
jumps crossfade without travelling across unloaded slides. Closing the direct
viewer retains the last representative and focuses its active hero button.

The plugin page uses Core-projected lifecycle bindings when a matching current
conversation and branch are available. Before a conversation it lists exact
character bindings and enabled, approved app-wide bindings through existing
document APIs; it describes the narrower context in the UI. A configured
binding is not labelled as currently applied without Core's lifecycle status.
Other-character bindings are excluded, read failures remain errors, and
pagination/truncation are explicit. There are no activation or storage writes.

Validation: 23 tests in eight targeted suites passed; portable Lua and regex
sandbox post-tests passed. Frontend format/i18n/lint/type checks passed with
only the two existing ChatPane controller-capture warnings. Source architecture
and `git diff --check` passed. The native UI build was relaunched.

At 394px native window width, verified 29 representatives, thumbnail centering,
swipe synchronization, original-image fit, downward dismissal and selection
retention. Verified a 14-image folder uses the same strip and advances its
selection on swipe. Profile actions contain only chat history and plugins,
and the actual card's empty plugin state came from its existing bindings.
