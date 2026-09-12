# Deep efficiency: native assets

Task ID: DEEP-EFFICIENCY-20260912, parallel native assets slice.
Before editing: HEAD/merge-base `aac3b16e0697c710b1209f498caba71821746140`,
branch `codex/ux-responsiveness-20260912`, worktree
`/Users/codexer/.codex/worktrees/lorepia-ux-20260912`. Existing UX/resource
changes remain dirty; authoritative start snapshot is
`/tmp/lorepia-deep-efficiency-20260912/baseline-files/` and `baseline.json`.

## Scope and invariants recorded before editing

Targets: `apps/lorepia/src-tauri/src/asset_protocol/png.rs` and its private
`png/tests.rs`. Existing parent sizes: 84 and 62 lines. Expected production
increase under 45 lines, tests under 160 lines; no size baseline changes.
Symbols updated in place: private `crc_table` and `crc32`; no public symbols
move. `has_valid_chunk_checksums`, called by `handle_with_backend`, keeps the
same signature, input bytes, chunk framing/order/bounds and corruption result.

Root/local AGENTS and ADR 0005 have been read, along with protocol caller,
PNG tests, renderer verified-handle cache and source cache/callers. No nearer
native AGENTS exists. No dependency, schema, API, golden, authority/cache trust,
no-follow, same-handle read, identity, SHA-256, CRC validation, lock/durability,
original-image, or admission contract changes are permitted.

Candidate: the existing CRC byte loop has one serial table dependency per byte.
An eight-byte CRC step can use eight independent table lookups while computing
the exact same reflected polynomial. Table memory grows from 1 KiB to 8 KiB,
fixed read-only data, with no per-request heap allocation. Risks are byte order,
remainder processing, short inputs, and table generation; scalar differential
checks, known vectors and existing corruption/protocol tests must cover these.

Baseline: targeted `cargo +1.96.0 test -p lorepia-tauri --lib asset_protocol --
--test-threads=2` started before edits with shared CARGO_TARGET_DIR
`/Users/codexer/lorepia/target`. Afterward run the same tests, targeted Clippy,
rustfmt, architecture check, and standalone optimized benchmark against the
snapshot implementation on small and large byte buffers. Full suite is parent's.

## Investigated but unchanged

- Source lookup/clone identity checks bracket distinct operations. Removing a
  pre/post check would weaken mutation detection; they are not redundant proof.
- Verified renderer cache serialization protects shared seek/read position and
  bounded verification work. Moving hash outside its lock needs new in-flight
  coordination, not simply dropping a lock.
- Full PNG reads and CRC checks on partial responses preserve the existing PNG
  integrity policy. This slice accelerates the check without bypassing it.
- Prior PNG metadata borrow optimization and DB descriptor SELECT improvements
  are completed work and remain unchanged.

## Result and measurements

Production PNG module grows 84 to 110 lines (+26); private tests grow 62 to
123 lines (+61). The validator's chunk walker and native delivery are unchanged.
The old byte CRC loop remains the algorithm for a trailing 0–7 bytes; complete
blocks use eight table slices. Explicit little-endian conversion makes host
endianness and input alignment irrelevant. The fixed table grows by 7 KiB.

A standalone benchmark extracted the CRC functions from the start snapshot and
this implementation, compiled with `rustc +1.96.0 --edition 2024 -O`, uses
`black_box`, deterministic identical inputs, and at least four rounds per size.
Each result is one local run, not an app FPS/latency or cross-platform claim.
The harness and raw results are in `/tmp/lorepia-deep-efficiency-20260912/`
(`crc-bench.rs`, `crc-bench-results.txt`).

| Input bytes | Before ns/call | After ns/call |
| ---: | ---: | ---: |
| 1 | 0.655 | 0.566 |
| 7 | 2.684 | 1.821 |
| 13 | 6.317 | 2.758 |
| 70 | 88.729 | 16.263 |
| 4,096 | 8,169.800 | 1,807.993 |
| 65,536 | 134,645.508 | 28,582.195 |
| 1,048,576 | 2,096,179.688 | 497,994.750 |
| 16,777,216 | 34,044,427.000 | 7,640,020.750 |

This reduces measured bulk CRC CPU by roughly 4.2–4.7 times, while adding a
small fixed read-only table rather than a response/cache allocation. The
required file reads, hashes, authority checks and complete PNG CRC coverage
remain intact. Very short inputs showed no regression in this local run;
sub-nanosecond timings are noisy and should not be extrapolated.

## Validation

- Baseline asset_protocol tests: 19/19 passed before editing.
- After change: 21/21 passed with two test threads.
- New differential test compares an independent bitwise scalar reference with
  every length 0–257, 4,095/4,096/16,384/65,536 bytes and offsets 0–15; reference
  updates are also split into two segments. Known empty and standard CRC vectors
  are checked. New multi-IDAT tests use independently generated CRCs and reject
  a payload mutation in every differently sized chunk.
- Existing truncated/ancillary/malformed PNG tests and full/partial delivery,
  including corruption outside the requested range, continue to pass.
- Targeted rustfmt, diff whitespace and shared-worktree architecture checks pass.
- Native library and test-target Clippy with `-D warnings` passed.
  Parent owns full and cross-platform gates.
