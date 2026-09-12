# Final efficiency — backend assets

Task ID FINAL-EFFICIENCY-20260912, baseline/merge-base
`e32c13c5b04c3d5db1b60637be115c2b61fcfb78`, shared branch
`codex/final-resource-efficiency-20260912` at `/Users/codexer/lorepia`.
Initial code clean; root task record and other assigned slices may become dirty.
Read root/frontend/Storage AGENTS, ADR0001–0006, previous asset reports and supplied
Pro PERF05/06/09. User authorizes accepted performance fixes; root owns contracts.

## Before edits: cache synchronization slice

Own `crates/storage/src/verified_asset_cache.rs`, private child helpers/tests,
`crates/storage/src/database/asset_delivery.rs` and its affected private tests;
later native asset_protocol policy/helpers/tests are also assigned. Supported
entry points remain Storage::resolve_approved_asset_by_id/by_sha256 and
read_approved_asset_range plus native asset handler. No facade/manifest/database
field edits without root coordination. Frontend agent owns descriptor queue/UI.

Replace global-cache-held I/O with a bounded digest slot table whose per-file
mutex owns verification state and seek position. Global mutex only selects or
registers slots and admits bounded hash jobs/budget. Preserve at most128 slots
including in-use handles (do not evict an externally held Arc), one-minute
verified lease, current descriptor requery, no-follow opens, same-handle hashes,
signature, bounds and pre/post identity checks. No File::try_clone read sharing.
At most4 simultaneous cold verification jobs; saturation is existing recoverable
storage_unavailable, as is existing hash budget WouldBlock. No new error/DTO.

Symbols that change: VerifiedAssetCache lookup/insert/read implementation and
Storage private lease coordination. Public symbols remain in place. New private
lease/job helper may be extracted to keep source caps; expected parent reduction
or small growth, no cap increase. Risks: lock inversion, duplicate hash, dropped
job permit, eviction causing >128 handles, expired lease reuse, stale file cursor,
poison handling. Tests must prove same-key singleflight, different-key progress,
held-slot backpressure, permit return, TTL and mutation rejection. Run existing
cache/approved-asset baseline first, targeted regressions, then Storage/native
suites; root owns entire workspace gates. All test data is synthetic.

Pixel/derivative and PNG streaming/result reuse require a separately recorded
policy decision before edits; original source bytes, no-store and CRC rejection
outside requested ranges are invariant. No unverified renderer cache is planned.

## Before edits: image policy and Rust-only attestation

Parent approved imagesize =0.15.0 (MIT, no runtime dependencies), minimal PNG,
JPEG, GIF, WebP and HEIF features; parent owns manifests/lock/contracts. Source
reader_size accepts BufRead+Seek; wrap the same verified file with bounded byte,
operation and seek limits. Display-only maximum side8192 and total16,777,216
pixels is a single-frame64MiB RGBA8 estimate, not a measured decoder allocation
or animation budget. UnsupportedContent distinguishes valid oversized images.
Original import/export and immutable descriptor dimensions remain unchanged.

Cold PNG validation uses the existing native CRC implementation on one bounded
scratch buffer, freed before serving bytes. This intentionally avoids changing
CRC semantics during relocation; warm ranges no longer allocate/read full PNG.
ApprovedAssetRange::IMAGE_VALIDATION_POLICY owns version1, forwarded through
Core AssetDeliveryRange and Shell AssetProtocolRange associated constants and
image_validation_policy:u32 fields. These Rust-only non-Serialize types are not
renderer DTOs. Native requires exact version for images. The in-process lease
cannot outlive its executable policy; insertion happens only after cold policy
validation and identity recheck, and every warm read checks the same handle.

Owned expanded files: Core asset_delivery.rs, Shell asset.rs, native
asset_protocol.rs/png tests, Storage private image_validation helpers/tests.
Public field/constant additions require root public-surface contract update.
Risks: stale/forged policy, PNG outside-range corruption, parser seek/CPU abuse,
invalid dimensions, preserving descriptor equality. Test cold corruption and
warm mutation, exact native policy, valid range bytes and parser budgets.

## Implemented and measured

Global map work now excludes hash, signature, image checks, file reads and waits
for a digest mutex. Slots include held/in-flight handles; only an unheld slot can
be evicted. Cache concurrency tests14/14 passed (including old TTL/symlink/link/
identity tests). Real Storage concurrent PNG test sends8 cold ranges, observes
exactly1 SHA verification and correct bytes/policy for each; subsequent same-file
mutation is StorageCorrupted. A CAS object with its own correct digest but a bad
PNG CRC outside the requested range is rejected. Existing approved asset tests3
and initial image policy tests3 passed. Native policy/range/CRC/admission tests22/22 passed, including unchanged scalar
CRC differential tests. An intermediate combined run stopped at linker errno28
(no disk space). Root removed regenerable repository build output and reran
integration with bounded build jobs and debug symbols disabled. Initial clippy
findings (test integer casts and public bounded expect) were corrected before
the slice was frozen. The final multi-format/work-budget/full-buffer-reuse and
workspace lint/test results are recorded in final-efficiency-review.md.

Policy1 is minted only after same-handle SHA/signature/dimension/CRC checks and
final identity validation. A warm read rechecks that handle before and after I/O;
its executable policy cannot change during a process-local lease. Native rejects
missing/old/unknown versions. A trusted Rust backend returning deliberately false
proof is outside this boundary, as with its existing approved descriptor and byte
contract; renderer inputs cannot instantiate these non-Serialize results.

The bounded header reader remembers rejected read/seek/work operations because
HEIF parsing can otherwise swallow a late reader error and return earlier
candidate dimensions. AVIF display is explicitly unsupported on32-bit targets:
the pinned library's internal usize area multiplication is only safe for two
u32 dimensions on64-bit targets. No hand-written multi-format dimension parser
was added. Optional immutable descriptor dimensions are preserved, not promoted
to newly verified renderer metadata. This is per-image display admission, not
an aggregate decoded-memory or animation-frame budget.

Cost tradeoff: warm PNG range work is proportional to the requested bytes rather
than the entire PNG. A direct cold range GET reuses the one-time CRC scratch after final handle
identity checks and copies only the requested range to its response, avoiding
an additional full-file read. A full-file range returns that verified Vec
directly without copying; a partial range gets a right-sized Vec. The full
scratch never enters the cache. A cold
descriptor-only resolve validates then drops its scratch; a subsequent GET
reads its requested range from the warm handle. At most4 cold jobs exist. A future fused SHA/CRC stream would
need its own equivalence tests; the present patch deliberately retains the exact
existing CRC algorithm and outside-range rejection. Original compression,
permanent derivatives and thumbnail schema/API are not introduced.
