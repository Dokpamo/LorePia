# Chat room surfaces

Task: CHAT-SURFACES-20260912. Baseline: aac3b16 (clean main); implementation on
codex/chat-room-surfaces-20260912.

The transient chat notice is anchored to the top of the chat viewport, independently
of the collapsing navigation header. Approved card controls appear over the chat by
default; the right-hand page remains available for a dedicated card view. Enabling
card features and managing grants stays in the chat settings bottom sheet.

SEED's Bottom Sheet guidance retains the current context for supplementary actions
(https://seed-design.io/react/components/bottom-sheet). Use the existing sheet
handle, two detents, focus trap and dirty-draft departure behavior. No new UI library.

Owners: WorkspaceApp composes runtime and settings, ApplicationFrame owns page
gestures, SettingsPanel owns modal presentation, PortableMessage and its trusted
bridge own isolated card layout. No IPC, native, schema, credential or model-dispatch
contract changes. Runtime grants remain explicit and revision-bound.

Renderer layout contract: message height is a finite integer in [32, 65536] CSS px
instead of a 720px scrolling box. Source, CSS, asset, worker and capability budgets
are unchanged. Message scroll containers expand into the transcript. Room frames
can report at most 128 finite viewport rectangles, clipped to their own bounds;
these only define the iframe's hit/paint regions, never host DOM or code. Messages
still require the exact opaque frame source, runtime ID and channel. Empty regions
cannot intercept chat input. Full card panels stay below app navigation and above
the composer, and offscreen pages do not mount another active room renderer.

Regression coverage: notice position during header scroll, swipe direction/cancel
and input exclusion, sheet dismissal/expansion/draft preservation, approved card
placement and grant management, tall message layout, frame identity and malformed
layout messages. Visual validation at the mobile app's dimensions is required.
