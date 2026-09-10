# Dependency compatibility follow-up

Base: `fd11fa864610d7d2dce3da438b4391d0aba77dbb` (PR #47 on main).
The user approved correcting the held dependency updates and pushing the
validated changes. This is dependency maintenance under ADR 0006, not an
extraction or a change to archived refactoring evidence.

## Review groups

- SHA-2 0.11 / base64 0.23: preserve canonical lowercase SHA-256 identities,
  credential binding/checksums, signed catalog bytes, strict base64 rejection,
  and existing durable/golden fixtures. Replace the removed digest `LowerHex`
  implementation with the existing `hex` encoder. Explicit direct `hex`
  dependencies are permitted in its callers; workspace layer edges stay the
  same. Keep base64's standard scalar engines, explicitly enabling `std`
  without the newly default SIMD implementation.
- Android libraries: align the app and platform plugin declarations, resolved
  dependency locks, and strict SHA-256 verification metadata. Verify artifact
  resolution and compilation; a dependency report alone is insufficient.
- Android toolchain: consider Gradle 9.7.1, AGP 9.3.2, and Kotlin 2.4.10 as one
  compatibility change, including the pinned Tauri Android source. Preserve
  wrapper checksum validation and strict dependency verification.
- rusqlite 0.40.2: preserve SQLite integer bounds, SQL/schema, transaction
  ownership, migration fixtures, and recovery behavior. Validate separately.
- ZIP 8.6.0: preserve untrusted archive bounds, path/entry validation,
  decompression limits, supported formats, and import/export round trips.
  Validate separately.
- TypeScript 7 remains held while the pinned Svelte checker supports only
  TypeScript 5/6. Do not bypass peer dependency checks.

`config/refactoring/dependency-architecture.json` records the exact approved
Cargo requirements/features/direct dependency edges as each group is applied.
No public facade, IPC command, renderer DTO, schema, golden fixture, or source
size ceiling is authorized to change for these upgrades.

## Sources

- [SHA-2 changelog](https://github.com/RustCrypto/hashes/blob/master/sha2/CHANGELOG.md)
- [base64 release notes](https://github.com/marshallpierce/rust-base64/blob/master/RELEASE-NOTES.md)
- [Android Gradle plugin releases](https://developer.android.com/build/releases/gradle-plugin)

## Validation

SHA-2/base64: `cargo check --workspace --all-targets` passed. Domain, Content,
Providers, and native platform suites passed 689 tests (no failures or ignored
cases). The independent cross-platform canonical-plan JSON/SHA golden test
passed unchanged. Transitive consumers retain their supported sha2 0.10 and
base64 0.22 versions in Cargo.lock; only workspace direct consumers are upgraded.
Further group and full-gate results will follow.
