# Frontend Guide

## Scope

- This file applies to `apps/lorepia/src/**` and supplements the repository
  guide. It does not govern the sibling `src-tauri` tree.
- This tree is the Svelte 5/Vite renderer. It uses typed IPC contracts and does
  not own native, provider-network, or Storage behavior.
- Read the owning controller/component, colocated tests, IPC facade, and
  relevant ADR before changing a feature boundary.

## Public Entry Points

- `main.ts`: live entry; mounts `app/App.svelte` without an explicit client.
  `App.svelte` creates the live client fallback.
- `preview/main.ts`: isolated demo entry; injects `createPreviewClient()`.
- `app/App.svelte`: screen composition, navigation, and responsive shell.
- `app/app-controller.ts`: root bootstrap/library/conversation/chat/provider
  application state and action facade.
- `lib/ipc/contracts.ts`: renderer-safe DTO/client contracts and versions.
- `lib/ipc/client.ts`: the only general Tauri invoke/listen/channel facade.
- `lib/ipc/commands.generated.ts`: generated command registry; never hand-edit.

The live entry must not import `preview/`. Preview must not import the live
entry or use production data, Tauri IPC, credentials, database, or host files.

## State and Transaction Owner

- `App.svelte` owns transient navigation, responsive layout, gestures, and open
  panel state. It creates one shared client and the root feature controllers,
  then disposes subscriptions/controllers on unmount.
- `LorepiaAppController` owns the root store and composes the memory, provider,
  discovery, stream, generation, library, import, and conversation controllers.
- `app/controllers/` owns the corresponding request epochs, serialized
  mutations, stream verification/reconciliation, and generation nonce/resume
  authority; `app/operations/` supplies their shared race primitives.
- Orchestration, content-package, module-lifecycle, prompt-history, persona,
  interaction-room, and generation-approval controllers own their bounded async
  state and authority.
- `ChatPane.svelte` wires the chat surfaces and owns view-local subscriptions
  and teardown. Its interaction-room, portable-runtime, scroll, message-action,
  composer, and utility-swipe helpers own their focused lifecycle/state.
- DB transactions, CAS/recovery, durable generation state, and credential
  material are not renderer state.

## Security and Determinism Invariants

- Preserve command names, DTO fields, error codes, API/event versions, and
  client signatures during refactors.
- Renderer contracts/state contain no raw credential, DB row/connection,
  absolute host path, raw handle, unrestricted package bytes, or
  renderer-authored durable authority.
- Keep strict payload guards and bootstrap compatibility checks. Malformed or
  unknown event data fails closed.
- Preserve chat route/version, monotonic sequence, watermark, terminal,
  reattachment, and reconciliation behavior. Disposing a renderer channel does
  not implicitly cancel Core generation.
- Preserve operation-nonce/resume exclusivity, revision/hash authority, epochs,
  serialized mutation order, admission, cancellation tombstones, and cleanup.
- Preserve Svelte rune dependencies, closure capture, mount/unmount lifecycle,
  focus restoration, scroll anchoring, observers, ARIA, keyboard/top-layer
  behavior, and responsive gestures.
- Ordinary media uses approved descriptors and canonical digest URLs through
  `features/assets/TrustedAsset.svelte`. Portable markup stays on
  `PortableMessage.svelte`'s sanitizer and digest resolver path.
- Do not weaken portable markup/CSS sanitization, iframe CSP/sandbox, worker
  budgets, runtime grants, or state limits.
- `features/chat/portable-regex-operation.ts` remains worker-only.
- Production source never imports tests and never uses interpolated dynamic
  imports.
- Do not add direct `invoke` or `listen` outside `lib/ipc/client.ts`. Existing
  narrow `isTauri` and `convertFileSrc` uses are not a general IPC escape hatch.
- Preserve stable ordering, fixed demo identities/timestamps, canonical
  serialization, and golden/hash inputs.

## Module Map

- `app/`: root shell/state facade; `app/controllers/` owns feature workflow
  authority and `app/operations/` owns epochs, identities, and serialization.
- `components/`: shared controls and detail-page primitives.
- `features/chat/`: transcript/composer, verified stream handling,
  virtualization, approvals/interactions, and portable runtime/workers.
- `features/orchestration/`: prompt, memory, knowledge, creator documents,
  packages/modules, and their controllers.
- `features/providers/`: connections, capabilities, sync, discovery, and catalog.
- `features/library`, `conversations`, `import`, `assets`, `personas`, `licenses`:
  bounded feature UI.
- `lib/ipc/`: DTOs, client, command registry, errors, and payload guards.
- `lib/i18n/`: Korean message catalog; use reactive `$tr` in templates and
  `t(...)` in imperative code, including imperative code inside Svelte files.
- `preview/`: fixed in-memory demo data/client only.
- `styles/`: global Paper & Ink tokens and responsive/semantic contracts.
- Tests are colocated as `*.test.ts` or grouped under feature `tests/` shards
  such as `app/tests`, `features/chat/tests`, and
  `features/orchestration/tests`; shared setup lives in `tests/setup.ts`.

## Common Change Recipes

### UI or Controller Change

1. Put shared workflow/race authority in the owning controller. View-local
   loading, busy, error, interaction, and presentation state may remain in the
   component. Durable state remains in Rust/Storage.
2. Pass the shared client/controller through props; do not construct another
   live client inside a feature.
3. Add user-visible text through `lib/i18n/ko.ts`.
4. Update the closest controller and component tests, including focus, scroll,
   accessibility, stale-result, and cleanup behavior when relevant.

### IPC Change

1. Stop unless the task authorizes the required Shell API, Tauri, config, and
   frontend paths; then read their governing guides/contracts.
2. Add the Shell API DTO/method and strict native contract.
3. Update root `config/ipc-commands.json` and run the generator.
4. Add the handler to `generate_handler!`, grant both development/release
   capabilities, and review/Git-track the generated permission.
5. Update frontend contracts/client and exact-set tests.

### Preview Change

Update `preview/demo-data.ts`, `preview/mock-client.ts`, and its test together.
Keep fixed identities/timestamps, clone returned values, and use no user data.

### Asset, Stream, or Portable Runtime Change

Read both renderer and native/protocol boundaries first. Update the policy,
controller, worker/channel, and relevant limits/reconciliation tests together.

## Targeted Tests

Run these commands from the repository root:

```bash
npm run check --prefix apps/lorepia
npm run test --prefix apps/lorepia -- src/app
npm run test --prefix apps/lorepia -- src/features/chat
npm run test --prefix apps/lorepia -- src/features/orchestration
npm run test --prefix apps/lorepia -- src/features/providers
npm run test --prefix apps/lorepia -- src/lib/ipc
npm run test --prefix apps/lorepia -- src/preview/mock-client.test.ts
python3 scripts/generate_ipc_commands.py --check
python3 scripts/check_source_architecture.py --base-ref "$(git merge-base HEAD main)"
```

For one Vitest target, use a real path such as
`npm run test --prefix apps/lorepia -- src/features/chat/ChatPane.test.ts`.
IPC/native changes additionally require Shell API and Tauri Rust tests; the
root guide defines the full pre-merge gate.

## Forbidden Dependencies and Changes

- Live production source outside `preview/` must not import `preview/`. No
  production source may import `tests/`, `*.test.ts`, or `*.spec.ts`.
- No direct dependency on Core, Storage, Providers, SQL, the OS credential vault,
  or native file APIs.
- Do not create another live IPC client inside a feature or bypass typed DTOs.
- Do not hand-edit generated commands/permissions or widen native capabilities.
- In a frontend refactor, do not add a framework/state manager/dependency,
  redesign UI, change IPC/schema semantics, rename unrelated code, raise a size
  baseline, or add a new giant-source exception.
- Do not copy secrets, paths, native diagnostics, or durable authority into UI
  state, logs, fixtures, or error messages.
