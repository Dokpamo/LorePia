# Provider SSE event borrowing

Task DEEP-EFFICIENCY-20260912. Worktree
`/Users/codexer/.codex/worktrees/lorepia-ux-20260912`, dirty HEAD/base
`aac3b16e0697c710b1209f498caba71821746140`; preserve prior resource/pagination
work and compare against the deep-efficiency snapshot. Root AGENTS read; no
nearer Providers AGENTS exists.

## Before editing

Targets: private providers sse.rs, its child tests, and four SSE adapters
(openai_compatible/openai_responses/anthropic_messages/gemini_generate_content).
Symbol change: SseEventBuffer::take_event returns a borrow into its existing
buffer; compact_if_worthwhile moves to immediately before a nonempty append.
Adapters pass that slice through their existing awaited event processor. No
public reexports, crate APIs, dependencies, IPC/schema or golden changes.

Each nonempty event is currently copied to a Vec before processing and discarded after
the await. Borrowing eliminates that allocation/copy; the mutable-buffer borrow
prevents append or compaction during processing. Consumed bytes may remain in
Vec length until the next nonempty append, but capacity was already retained by
clear/truncate before this change. No additional buffer allocation is required.

Preserve separator preference, deferred CR at chunk edges, CRCRLF continuation,
scan bounds, stream/event limits, cancellation checks, sink backpressure and
terminal early return. Empty chunks must remain no-ops. All adapter edits should
be only passing event rather than &event. sse.rs is small; giant adapter changes
reduce a byte per call and need no baseline increase.

Baseline: four existing sse_framing_work tests, pinned Rust 1.96.0 and shared
CARGO_TARGET_DIR=/Users/codexer/lorepia/target, --test-threads=2. Add pointer/
compaction tests and a framing differential across chunk splits. Run Providers
library tests and targeted Clippy after edits; parent owns full workspace gates.

## Current result

The event view is borrowed directly from pending bytes; pointer-equality tests
confirm this even for a 70 KiB event. A second same-chunk event stays at its
original address. Compaction waits until a subsequent nonempty append, and
empty chunks do not modify the pending prefix. Full consumption followed by a
new append reuses the original capacity. There is no new heap buffer or unsafe
code. Borrow scope ends after the awaited processor and before the next buffer
mutation; Rust checks this in all four adapters.

SSE parent remains 244 lines (7,890 to 7,901 bytes); child tests add 118 lines.
Each giant adapter changes only two calls from &event to event (two bytes less).
No source cap changes.

Baseline four linear-framing tests passed; after editing the SSE-filtered
suite passed 8/8. Differential tests compare a separate draining reference
across every small chunk size, truncated stream endings, all supported
separator forms, empty chunks and large-prefix compaction. Pointer tests prove
the eliminated copy structurally, not via an end-to-end throughput claim.
Targeted rustfmt and diff whitespace checks passed. Providers full library
passed 400/400 with two threads, including adapter terminal, cancellation,
size-bound and split-separator tests. All-target Providers Clippy with
-D warnings and shared source architecture check passed. Raw library output:
`/tmp/lorepia-deep-efficiency-20260912/providers-tests.txt`; architecture output:
`/tmp/lorepia-deep-efficiency-20260912/providers-architecture.txt`.
