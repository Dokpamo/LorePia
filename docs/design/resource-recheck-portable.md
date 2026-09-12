# Portable pre-build dependency gating

Task RESOURCE-RECHECK-20260912, baseline c999c43bafdb66f922d6e5a230a7dfd2c6c2cca7;
shared branch codex/final-resource-efficiency-20260912, initially clean before
parallel authorized recheck work. Governing material: root/frontend AGENTS,
ADRs0001–0006 and resource-recheck-task.md. No IPC/API/dependency changes.

Before editing, inventoried PortableMessage effects/buildPortableDocument,
portable-document-queue submit/drain, portable-display macros/transforms,
portable-asset-resolution and existing tests. Existing targeted baseline31/31
passed (PortableMessage, queue, asset resolution). Own PortableMessage and new
focused portable-build-dependencies helper/tests; shared manifests untouched.

Public component props remain. Preserve profile/client/frame identity, source,
background, names, message index, surface/floating/width, and dynamic context.
Only sources/backgrounds with no macro delimiters and profiles with no output
or display transforms may omit live head/last-assistant/variables dependencies.
Any unknown/malformed macro delimiter or transform falls back to full evaluation.
A one-entry input memo stabilizes Svelte prop publications before build; it stores
input references only, never approvals, sanitized DOM, resolved asset maps or
additional documents. Input changes retain the original resolve/sanitize path.
Normalization uses its own source/profile/enabled tuple so unrelated publications
do not re-run provider-output transforms. Queue/bridge/BGM lifetimes stay intact.

Expected size: small component growth and <100-line helper; no cap increases.
Risks: hidden macro dependencies, stale approval after profile/client switch,
losing a latest queued update, static/dynamic transitions and test-only rerender
noise. Regression tests must count display/resolve/sanitize and normalization,
preserve bridge IDs on irrelevant updates, and force revalidation on authority,
source/background and dynamic macro changes. Root owns full gates.

## Result

Implemented two single-entry input gates (normalization and document build).
Three mounted static cards changing head, last assistant and variables now incur
zero additional normalization/display/resolve/sanitize calls and zero srcdoc
changes. The baseline independent probe incurred3 resolves,6 display evaluations
and3 sanitizations for those cards despite0 srcdoc replacements.

Unknown/malformed delimiters, even known context-free button/audio tokens, and
all profiles containing transforms remain on the conservative dynamic path.
This patch does not claim per-variable incremental evaluation for those cards.
Profile/client/source replacement still revalidates; dynamic source/background
head macros, output transforms introducing macros, variable/last-assistant
updates, existing BGM handling, cancellation and bridge identity regressions pass.

Validation: affected baseline31/31; final focused5 files37/37 tests passed with
maxWorkers4. Scoped ESLint passed. Svelte diagnostics0 errors/0 warnings after
production changes. Prettier and git diff --check passed. Root owns full gates.
PortableMessage grows535→573 lines,20270→21378 bytes; helper31 lines1072 bytes.
No source cap, serialized contract, API, manifest or dependency changed.
Evidence logs: /tmp/lorepia-recheck-portable-{baseline,final,svelte}.log.
