# Frontend resource optimization slice

- Task: RESOURCE-OPTIMIZATION-20260912, parallel frontend slice.
- Baseline HEAD: aac3b16e0697c710b1209f498caba71821746140. Worktree is dirty with inherited UX changes and concurrent authorized resource slices. No inherited changes are reverted.
- Scope: PortableMessage.svelte asset selection; private portable asset-selection helper and tests; portable-renderer-bridge.js room layout and tests. Public component props, frame protocol and IPC remain unchanged.
- Symbols moved: IndexedAssetAlias, selectAsset, indexAssetAliases, normalizedAlias, stableIndex become a private feature module implementation. Document construction, sanitization, delivery validation and macro rendering remain in PortableMessage.
- Invariants: exact UTF-16 FNV update order and modulo result; stable alias ranking and duplicate candidates; source/reference hash separator; 32,768 alias cap; approved descriptor resolution; all sanitizer, CSP, origin/runtime checks, worker budgets, cancellation and width-driven rebuilding. Room style cache lasts one report only; message overflow writes remain uncached.
- Baseline tests: PortableMessage, portable-frame-layout, portable-runtime-packaging. Verification adds old/new asset-selection equivalence and executable bridge layout tests, then focused formatting/lint/type checks.
- Expected size: PortableMessage (591 lines before this slice) decreases by moving selection into a small helper; bridge (114 lines) grows slightly within its cap. No baseline increase or exception.
- Risks: accidentally changing FNV signed/unsigned conversion, surrogate handling, duplicate tie choices or reuse across source changes; stale style across reports; room/message behavior crossover.
- Governing material: root AGENTS.md, apps/lorepia/src/AGENTS.md, ADR 0001 and ADR 0005. This task explicitly authorizes these internal performance changes, with no public API/schema/dependency changes.

## Implemented and verified

- The build-local selector lazily indexes aliases, performs no source hashing for singleton candidates, and reuses the exact source+NUL FNV state for ambiguous references. Prefix strings for alias matching are formed once per selection rather than per alias. No descriptor result is cached.
- The room bridge reuses computed styles within one report only. Next reports read fresh styles. Message overflow normalization and resize deduplication remain unchanged.
- Baseline: 3 files / 28 tests passed, plus Lua sandbox and regex worker posttests.
- Focused verification: 5 files / 34 tests passed. Hash equivalence covers 245 source/reference/candidate-count combinations, including Korean, astral characters, lone high/low surrogates, NUL and the 262,144-character limit. Separate tests cover duplicate aliases, exact/fallback ranking and alias-cap rejection.
- Measured JavaScript charCodeAt call count for 128 four-character references and a 262,144-character source: optimized 262,657 versus prior algorithm's 33,555,072 (127.75x fewer hash steps). Singleton selection performs zero hash steps. A second selector recomputes its own prefix.
- Room fixture: 4 elements share ancestors. One report makes 4 computed-style calls; a second report makes 4 new calls and correctly removes regions when ancestor opacity becomes zero. The prior traversal requires 11 calls per unchanged visible report for this fixture. Message overflow normalization and repeated-report publication deduplication pass.
- Modified source sizes: PortableMessage 591 → 518 lines; private selector 88 lines; bridge 114 → 126 lines. No cap or contract file was modified.
- Focused ESLint and formatter checks pass; git diff whitespace check passes. Initial full typecheck reached only concurrent asset-delivery-loader.test.ts:45 errors (TS2493/TS18048), outside this slice; parent owns the final full frontend checks after parallel work completes.
- Files: apps/lorepia/src/features/chat/{PortableMessage.svelte,portable-asset-selection.ts,portable-asset-selection.test.ts,portable-renderer-bridge.js,portable-renderer-bridge.test.ts} and this record. No commit created.
