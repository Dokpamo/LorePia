# Final efficiency contract changes

Task FINAL-EFFICIENCY-20260912 implements the user's accepted performance
findings at baseline e32c13c5b04c3d5db1b60637be115c2b61fcfb78. Under ADR 0006,
these explicit changes update the current API and dependency inventories;
historical refactoring evidence is not regenerated.

## Selected memory sources

Storage adds `MemorySourceMessageIdentity` (ID and role only),
`list_branch_memory_source_identities` and `list_branch_memory_source`.
The first returns compact eligible identities for the existing summary-window
algorithm. The second validates lineage and selected range in one snapshot,
limits it to 512 messages / 4 MiB of source bodies, and materializes only that
range. Its contract validates body storage types, but does not decode the UTF-8
of bodies outside the selected range. Existing complete-history APIs retain
their behavior. These are Rust-only Storage operations, not new Core re-exports
or renderer commands. The Core summary path retains original order and hashes.

## Native image validation

`ApprovedAssetRange`, `AssetDeliveryRange` and `AssetProtocolRange` add
`image_validation_policy: u32` and an `IMAGE_VALIDATION_POLICY` constant.
Storage owns version 1; Core and Shell forward it. These carriers have no
renderer deserialization contract. The native protocol trusts only the exact
current policy from its trusted Rust backend. A value from renderer input,
unknown version or legacy zero is not an attestation.

The version binds cold same-handle verification to actual image dimensions and,
for PNG, the existing complete CRC validation. Identity checks and fixed lease
expiry invalidate reuse. Descriptor metadata is not overwritten with inferred
dimensions. Oversize display images fail with UnsupportedContent; original
import/export data is preserved. Limits and remaining animation constraints
are documented in final-efficiency-assets.md.

## Header-only image dependency

Add exactly `imagesize = 0.15.0` to Storage through the workspace declaration,
with default features disabled and only png/jpeg/gif/webp/heif enabled. This
MIT-licensed crate has no runtime transitive dependencies and reads dimensions
without decoding pixels. Cargo.lock adds only this package; no package is
upgraded. The caller enforces read/seek/operation limits, sticky budget failure,
and a 32-bit HEIF guard around the pinned parser.

No IPC names, renderer DTO fields, credential or URL authority, frozen schema
fixtures, golden data, source caps or archived contracts are relaxed.

## Durable streaming suffixes (schema 0043)

The confirmed PERF-08 change adds a numbered migration and a Rust-only Storage
append operation. Core supplies the message/generation identity, expected
durable byte offset and new UTF-8 suffix. Storage atomically validates ownership
and pending status, advances a sequence/byte watermark and stores the suffix.
Every pending-content reader and interrupted recovery must see the exact
ordered base-plus-suffix content. Full snapshots and terminal transitions clear
the journal in the same transaction. The legacy whole-message checkpoint entry
remains available to Rust callers.

The implementation preserves the 500 ms / 64 KiB checkpoint trigger and
WAL/FULL settings. It does not increase the allowed crash-loss interval or
report an incomplete terminal write as success. Fresh/upgrade/reopen/cutpoint,
duplicate/conflicting append, stale append after terminal, cancellation and
interrupted recovery cases are required integration coverage.
