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
- Android toolchain: use Gradle 9.7.1, AGP 9.3.2, and Kotlin 2.4.20 as one
  compatibility change, including the pinned Tauri Android source. Preserve
  wrapper checksum validation and strict dependency verification. Kotlin 2.4.20
  supersedes the held 2.4.10 proposal to cover Gradle 9.7 / AGP 9.3 in JetBrains'
  compatibility matrix. Tauri 2.11.5 still applies the external Kotlin plugin;
  use AGP's documented legacy DSL / external Kotlin compatibility flags in both
  builds until that pinned dependency migrates. Replace the removed
  `Project.exec` with injected `ExecOperations`, preserving arguments, working
  directory, exit checks, and Windows fallbacks. AppCompat is a direct runtime
  requirement of the platform plugin; declaring it only as compile-only left
  standalone runtime resolution on Tauri's older version. Explicitly align the
  buildSrc Kotlin plugin dependency as well: AGP otherwise exposes its older
  transitive Kotlin plugin to the app, overriding the root declaration and
  producing a shared platform-plugin lock incompatible with standalone builds.
- rusqlite 0.40.2: preserve SQLite integer bounds, SQL/schema, transaction
  ownership, migration fixtures, and recovery behavior. Its `fallible_uint`
  feature preserves the original checked u64/usize conversions; `cache`
  preserves prepared-statement caching. Explicit features avoid enabling the
  new default WebAssembly backend. No SQL or row conversion rewrite is needed.
  Validate separately.
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
- [AGP 9.3 compatibility](https://developer.android.com/build/releases/agp-9-3-0-release-notes)
- [Kotlin/Gradle/AGP compatibility](https://kotlinlang.org/docs/gradle-configure-project.html)
- [AGP compatibility flags](https://developer.android.com/build/migrate-to-built-in-kotlin)
- [Gradle 9.7.1 checksum](https://services.gradle.org/distributions/gradle-9.7.1-bin.zip.sha256)
- [rusqlite 0.40.2 release](https://github.com/rusqlite/rusqlite/releases/tag/v0.40.2)

## Validation

SHA-2/base64: `cargo check --workspace --all-targets` passed. Domain, Content,
Providers, and native platform suites passed 689 tests (no failures or ignored
cases). The independent cross-platform canonical-plan JSON/SHA golden test
passed unchanged. Transitive consumers retain their supported sha2 0.10 and
base64 0.22 versions in Cargo.lock; only workspace direct consumers are upgraded.
Full-gate results are recorded below as they finish.

rusqlite: the `lorepia-storage` run passed 413 tests, with its 3 existing ignored
helper/manual tests unchanged, including immutable
schema-11 fixtures, cutover/reopen and recovery tests. The added unsigned-counter
regression verifies INTEGER affinity and round trips at 0/1/i64::MAX, rejects
larger u64 inputs and negative/floating reads, and checks rollback after a
rejected bind. No schema or fixture was changed.

Android compatibility also includes `config/android/tauri-2.11.5.gradle.kts`.
Both settings files select this checked-in build script while keeping each
Tauri project's original source directory. It only replaces the deprecated
Android/Kotlin compiler-options DSL; the Tauri source tree and its dependencies
are unchanged. The locked upstream build script has SHA-256
`aab1b0ecb929ea70b33ad42d7df55c185a3485f9609ec998047232b45df76a4d`.
The app explicitly keeps both Java and Kotlin on JVM 1.8, avoiding AGP's changed
Java default. The platform plugin retains Java/Kotlin 17. CI now builds the
Tauri Android APK before compiling instrumented sources, so Cargo emits the
real Wry/Tauri activity classes; it also runs the standalone plugin's unit tests.

Android lock/verification maintenance can be reproduced with the checked-in
`scripts/resolve_android_dependencies.gradle` init script and Gradle's
`--write-locks --write-verification-metadata sha256 resolveDependencyIntegrity`
flags, using `--no-configuration-cache` for this maintenance-only task. Run it
for the app and standalone plugin after the existing Tauri preparation step,
review the new artifact hashes, then run normal strict-verification builds.

ZIP 8.6.0: retain `default-features = false` with only `deflate`; both old
and new versions use the same zopfli/flate2-zlib-rs feature composition. The
existing Content suite and 10 Core import/export vertical tests passed. Static
CHARX and malicious archive fixtures remain byte-for-byte unchanged, covering
paths/collisions, symlinks, duplicate/ZIP64 records, size/ratio boundaries, MIME
mismatch and corrupted metadata. Source export/reimport still preserves exact
original bytes. No archive validation path or limit was relaxed.

The Gradle host-tool metadata includes the Linux, macOS, and Windows AAPT2
classifiers for `com.android.tools.build:aapt2:9.3.2-15703166`, verified against
Google Maven. This is needed because the local build runs on macOS and the
required Android GitHub job runs on Linux. Generate these additional classifier
entries with a temporary detached Gradle configuration and the same
`--write-verification-metadata sha256` maintenance flag; do not change strict
verification for normal builds.

Frontend: Svelte/TypeScript checking passed with the two existing ChatPane
initial-reference warnings. All 1,072 tests (163 files) passed on the full rerun
with `--maxWorkers=2`; an earlier concurrent run had a transient import-preview
validation failure, which also passed in isolation. Lua and regexp worker
posttests passed. No renderer code changed.

Repository Python suites passed 108 tests, including Android preparation path
safety. IPC generation, frozen refactoring report, workflow security, Android
integrity, i18n baseline, AI context budgets, and source architecture checks
passed. No architecture/size ceiling or archived report was relaxed.

Android: the full Tauri CLI produced a debug ARM64 APK with strict verification.
After aligning buildSrc with Kotlin 2.4.20, standalone plugin Kotlin and
instrumented sources recompiled with `--dependency-verification strict
--rerun-tasks`, and all 30 platform JVM tests passed. The app's ARM64 APK and
instrumented sources then compiled in strict mode again, reusing the unchanged
native library from the Tauri build. CI runs the full native+Gradle build on
Linux without skipping the native task. Instrumented Android tests were compiled,
not executed on a device.

All 443 new Gradle artifact checksums were independently verified against their
primary Maven publisher repositories. The wrapper JAR and distribution match
Gradle's official SHA-256 checksums. No existing artifact checksum was replaced,
and no trusted-artifact or verification bypass was introduced. AGP 9's explicit
external-Kotlin/legacy-DSL compatibility flags remain necessary for Tauri 2.11.5;
AGP 10 is outside this update and needs a separate upstream migration.

Workspace formatting and warning-free Clippy passed. Full Rust workspace tests
and the required cross-platform GitHub gates must pass before this follow-up is
merged; the targeted group results above do not replace them.

CI uses the committed Android project plus the existing path-safe ignored-input
preparation script. Re-running `tauri android init` overwrites the reviewed
BuildTask with its old `Project.exec` template, so the job no longer reinitializes
an already tracked project. Before and after the build, Git verifies that no
tracked Android configuration or lock/verification file changed.
