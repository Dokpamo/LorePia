# Connected mobile UI in Tauri

- Task ID: `TAURI-UI-PREVIEW-20260906`
- Baseline / merge-base: `82a77caed61713109dfd8dc86b36aafee3b834d6`.
- Starting worktree: 52 pre-existing modified/untracked files from earlier UI work;
  their contents were recorded before this task. Preserve them.
- Request: make the accepted `lorepia-toss-layout.html` mockup usable in the
  existing Tauri app so the user can try and refine the layout.
- Targets: a separate `ui-preview.html` entry, `src/preview/ui/` components,
  `lib/i18n/ko-ui-preview.ts`, its catalog registration, one Tauri UI configuration,
  a dedicated Vite build input and repeatable package scripts. Production `main.ts` and `preview/main.ts`
  remain their existing entry points.
- Owned invariants: the orange accent, flat SVG controls, color-only composer,
  left character/history management, central chat, optional creator page,
  24px icons / 48px targets, common spacing and type scale.
- No existing symbols move. No Rust, IPC, schema, credential, asset delivery,
  dependency or capability changes. The UI entry uses fixed local sample data
  and never constructs a live client or calls native APIs.
- Expected addition: approximately 1,200–1,700 lines across bounded components,
  styles, localized labels, data, checks and launch configuration.
- Risks: WebKit versus browser sizing; macOS titlebar overlap; inaccessible
  offscreen pages; lost per-conversation drafts; navigation/focus restoration.
- Checks: frontend check/build, focused component flows, source architecture
  delta, generated IPC agreement, and direct use of the running native window.
- Architecture baseline: 75 pre-existing Rust API diagnostics. Do not expand
  that baseline or modify unrelated Rust code to suppress it.
- Governing material: root and frontend `AGENTS.md`, ADR 0001, the user's
  latest accepted mockup and supplied Toss light/dark screenshots. The older
  `mobile-ui.md` home/bottom-tab proposal does not govern this prototype.

## Run

From the repository root, with the pinned Node and Rust toolchains:

```sh
npm run tauri:ui --prefix apps/lorepia
```

This opens a 393px-wide native window. Vite updates the running UI as its source
changes. All cards, conversations, messages and preferences here are samples
held for this window session; AI generation and native data are not connected.

On macOS, append `-- --config src-tauri/tauri.macos.dev.conf.json` to select the
existing development app identity. The UI itself uses no native IPC in either
launch mode. For a standalone macOS app, use `npm run tauri:ui:build --prefix
apps/lorepia -- --config src-tauri/tauri.macos.dev.conf.json`; the bundle is
`target/debug/bundle/macos/LorePia UI.app`.

## Verification — 2026-09-06

- Pinned Node 24.18.1 and Rust 1.96.0.
- Frontend formatting, i18n, lint and typecheck passed. Two existing warnings
  remain in the production `ChatPane.svelte`; none originate in this UI entry.
- Four component flow tests passed, including per-conversation drafts, new
  conversations, Korean composition, optional sidepage, focus restoration and
  session isolation. Existing Lua and regex worker regressions passed.
- Dedicated Vite UI build and the macOS Tauri debug app bundle succeeded.
- Native WebKit: verified management → chat, message insertion, new conversation,
  creator page and local theme selection. Corrected native select sizing to
  48px and rechecked it in the final bundle. The app was reopened on management.
- Architecture diagnostics stayed at the same 75 baseline Rust diagnostics;
  no new violations. IPC generation check passed.
- Pre-existing worktree files were unchanged except the registration of the
  new locale catalog. The final catalog composition lives in `lib/i18n/index.ts`
  so the base `ko.ts` stays within its existing source-size limit.

This validates the local macOS UI prototype. It does not claim AI dispatch,
durable conversation storage, or iOS/Android runtime verification.
