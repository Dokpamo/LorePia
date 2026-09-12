# Conversation display modes

Task: `chat-display-modes-20260911`. Baseline: `15e559b`, on
`codex/retire-legacy-ui-20260910`. Existing uncommitted import compatibility,
schema 42 and module activation work is preserved.

The requested layouts are 기본 (full-width message regions), 소설 (continuous
text with a different user-text color), and 채팅 (existing bubbles). New rooms
start in 기본; existing rooms retain their chat/story appearance until changed.
The date divider is removed from the transcript, while history timestamps and
message identities remain unchanged.

The layout preference is device-local, keyed by opaque conversation ID, using
the existing local UI-preference pattern. 기본 and 채팅 both use native `chat`;
소설 uses native `story`. The generation mode, IPC DTOs, database schema,
prompt selection and durable generation authority are unchanged. A preference
is recorded only after the matching conversation has been created or its native
mode has been saved. Failed setup can retain the preference on its already
created pending conversation. A conflicting native mode takes precedence over
an old device preference.

`WorkspaceSettings` owns the mode form and binds successful results to the
requested room. `WorkspaceApp` projects the display choice and existing resolved
persona name. `ChatTranscript` retains scroll/virtualization ownership;
`ChatMessage` retains copy, edit and menu lifecycle. Profile media stays on
`CharacterImage`'s approved asset delivery path. The existing message menu owns
branch selection and deletion confirmation in every mode.

Default messages use edge-to-edge surfaces, the shared content inset (16px on
the 393px mobile viewport), a 36px profile,
12px between profile and body, and 44px tool targets. Novel messages use no
bubble surface. Chat notices float immediately below the visible header and
follow its existing scroll offset, without reflowing the transcript.
On wide windows, message surfaces still span the transcript while their text
uses the same readable width as the composer. Auxiliary runtime/status content
keeps that shared text inset.

Character cards, profile hero images, image groups, album images and viewer
thumbnails reuse the shared press action and visual child. Their scale-only
feedback keeps the pressed surface transparent; generic button fill must not
paint behind their images or captions. Pointer feedback is explicit because
image gestures may prevent the native default. Movement cancels the press
without intercepting scrolling or swiping, and release/cancel/blur/teardown
clears its state. The shared 150ms, size-dependent scale follows
[SEED Scale Feedback](https://seed-design.io/react/components/concepts/scale-feedback)
and [SEED motion](https://seed-design.io/foundations/motion). The user's requested
top notice placement takes precedence over SEED's usual bottom snackbar placement;
existing dismiss, retry, live announcement and reduced-motion behavior remains.

Validation: layout preference persistence/failure and native-mode fallback,
three-mode settings/new-room integration, persona/name and message tools,
date-divider removal, top notice placement, header offset/cleanup, existing
clipboard, branch, composer and scroll tests, Svelte checks, and visual review.

Verified on 2026-09-11: all 705 frontend tests (130 files), Lua and regex worker
sandbox regressions, formatting/lint/TypeScript/Svelte checks, IPC generation,
and the source architecture check passed. The macOS UI bundle built and launched.
Browser review covered all three modes at 393 × 852, light/dark user-text
contrast, 12px section gaps, full-width default surfaces, and the copy notice
below the header. The native app was also opened on the existing Alternate
Hunters conversation and switched to 기본; its message date divider is absent.

Press-feedback follow-up on 2026-09-11: all 710 frontend tests (131 files),
worker regressions, frontend checks and source architecture passed, and the
macOS UI bundle rebuilt. During real pointer presses at 393 × 852, browser
style samples confirmed shrinking and return to scale 1 for all five image
surfaces, with a transparent background throughout. Drag cancellation,
interruption cleanup and keyboard feedback have targeted regression coverage.
