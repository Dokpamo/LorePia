# Resource optimization: Content PNG slice

Task: RESOURCE-OPTIMIZATION-20260912 (parallel Content slice).
Baseline HEAD and merge-base: `aac3b16e0697c710b1209f498caba71821746140`.
Worktree: `/Users/codexer/.codex/worktrees/lorepia-ux-20260912`, branch
`codex/ux-responsiveness-20260912`; dirty with existing UX and other resource
work. The assigned PNG files were clean before this slice.

## Scope recorded before editing

- Targets: `crates/content/src/png.rs`, private PNG child tests, and this record.
- Symbols changed in place: `read_text_chunk`, `read_compressed_text_chunk`,
  `extract_card_metadata`, `decode_base64`; no symbol movement.
- Public callers remain `inspect_file`, `inspect_character_file`, `prepare_import`
  through private `inspect_png_source`. No public API, dependency, schema,
  golden fixture, or original image bytes changes.
- Preserve source hashes, PNG framing checks and limits, zTXt inflation ceiling,
  V3 preference and first V2 fallback ownership, arbitrary keyword bytes,
  ASCII whitespace handling, metadata limits and failure classifications.
- Relevant tests: `cargo +1.96.0 test -p lorepia-content --test png_character_cards`
  baseline and after; private PNG tests; targeted crate Clippy and rustfmt.
  Shared target: `/Users/codexer/lorepia/target`. Full gates belong to parent.
- Expected delta: PNG parent 228 lines to approximately 240–265; private tests
  in a child module. No size-baseline increases or giant exceptions.
- Risks: borrowed text cannot outlive its payload; only retained legacy metadata
  becomes owned. Compressed text must stay owned after inflation. Fast-path
  base64 must preserve invalid byte handling and existing check order.
- Governing material: root AGENTS.md, ADR 0001–0003 and 0005 were read. No
  nearer Content AGENTS.md exists.

## Findings

`tEXt` parsing copied its keyword and full text, even for irrelevant keywords;
base64 decoding copied the encoded text again even with no whitespace. Borrowing
these buffers avoids transient copies without changing persisted content.

The archive metadata preallocation suspicion was a false positive:
`prepare_entry` rejects declared metadata above 4 MiB before allocation. Archive
reading already streams through a 64 KiB buffer; archive code is unchanged.

Native PNG responses already reuse their full buffer with copy_within/truncate.
Full PNG CRC checks remain necessary. Verified asset leases retain at most 128
file handles, not complete image byte buffers. Export inspection is an explicit
fail-closed contract. None of those paths is changed by this slice.

Derived thumbnails require a codec, pixel/decode budgets, source-to-derived
digest authority, approved delivery descriptors, and retention/recovery design.
Replacing original PNG bytes would change embedded metadata/provenance and
digest identity; this work does not recompress originals.

## Validation

- Before editing: PNG integration baseline passed, 8/8.
- After editing: PNG integration passed, 8/8; private PNG tests passed, 4/4.
- `cargo +1.96.0 clippy -p lorepia-content --all-targets -- -D warnings` passed
  after fixing two test-helper style warnings.
- Targeted rustfmt check and diff whitespace check passed.
- `python3 scripts/check_source_architecture.py --base-ref
  aac3b16e0697c710b1209f498caba71821746140` passed for the shared worktree.
- Measured PNG parent: 228 to 239 lines (+11); private child tests: 106 lines.
  No baseline changes. Parent owns full workspace/cross-platform gates.

The private tests confirm large unrelated tEXt uses the original borrowed buffer,
first legacy payload survives subsequent chunk reads, compressed V3 takes
priority, inflation returns owned bytes, wrapped and compact base64 decode to
the same arbitrary bytes, and empty/invalid/oversized inputs keep their failure
classification. Existing integration tests cover import staging, V2 promotion,
V3 parsing and decompression-bomb rejection.

The allocation benefit is structural rather than an end-to-end RSS benchmark:
uncompressed tEXt no longer creates another full payload copy, and whitespace-free
base64 no longer materializes a compact copy. The original bounded PNG payload
and decoded metadata remain allocated; original image bytes remain unchanged.
