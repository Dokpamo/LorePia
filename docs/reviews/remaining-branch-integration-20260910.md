# Remaining branch integration — 2026-09-10

Base: `6addfb7cc5f48917ae64581225658b9e26f164ea` (PR #50).
The user requested integrating the remaining updates, resolving their
compatibility problems, pushing, and deleting integrated branch refs.

## Reviewed remaining updates

| PR | Reviewed head | Integration |
| --- | --- | --- |
| #7 | `37f15b79bb5f1e97241938481b1cd704d2f779de` | TypeScript 7.0.2 using Microsoft's official side-by-side transition |
| #24 | `fe8ffb860291363e6bd637d25542676e93da8bac` | AGP 9.4.0, aligned in the app, buildSrc, and standalone plugin |
| #48 | `740db08d017199fe7de6b4887affb39c807074cd` | Vitest and its mocker 4.1.11 |
| #49 | `0c25adaa83178426d0825996bf7f895d507ccc2a` | Same Vitest 4.1.11 update as #48 |

PR #24 was updated by Dependabot after #50 merged. Its current proposal is
9.4.0, although its branch name still ends in 9.3.2. The other original Android
and Rust updates were already integrated by #50. The two Vitest proposals have
the same package/lock changes and are applied once.

## TypeScript 7 compatibility

TypeScript 7 replaces the compiler implementation and does not provide the
legacy compiler API required by the current Svelte checker and typed ESLint
parser. Merely replacing the direct TypeScript version breaks npm peer
resolution and those tools.

Use Microsoft's documented npm aliases:

- `@typescript/native` resolves to the stable `typescript` 7.0.2 package and
  owns the `tsc` executable. `typecheck:native` runs it against the existing
  project configuration, including TypeScript tests.
- `typescript` resolves to `@typescript/typescript6` 6.0.2, Microsoft's API
  compatibility package. Its locked `@typescript/old` dependency remains on
  TypeScript 6.0.3, the version used before this change. The wrapper exposes
  `tsc6`, avoiding an executable-name collision with 7.
- `typecheck` requires both native TypeScript 7 and the existing Svelte
  checker to pass. Typed ESLint rules, Svelte diagnostics, compiler strictness,
  the included source files, and the existing CI gate remain enabled.

No forced peer installation, legacy-peer-deps flag, patched dependency, parser
warning suppression, or application API/type widening is used. One imported
compatibility test helper explicitly declares its heterogeneous hint map;
otherwise the TypeScript 7 inference treats the spread as containing only the
`import_kind` string. Runtime fixture values and assertions are unchanged.

## Android compatibility

AGP 9.4.0 requires Gradle 9.6.0 or later and JDK 17. The existing pinned Gradle
9.7.1, JDK 17, SDK 36, and explicit NDK satisfy those requirements. Keep Kotlin
2.4.20 and the reviewed Tauri 2.11.5 build-script overlay, Java/Kotlin target
alignment, and AGP 9 external-Kotlin/legacy-DSL compatibility flags.

Kotlin's published fully tested AGP range currently ends at 9.3.1. Its
documentation permits newer AGP releases with possible deprecation warnings;
therefore this combination is qualified with actual strict-verification app
and plugin builds and tests, without suppressing those warnings. AGP 10 is
not introduced.

Regenerate dependency locks and SHA-256 metadata together. Include AAPT2
`9.4.0-15978811` artifacts for Linux, macOS, and Windows, and verify new hashes
against their primary publishers. Preserve every existing checksum, strict
verification, lock enforcement, and the pinned Gradle wrapper.

## Branch cleanup

The UI, earlier integration, and review branch tips are ancestors of main.
The dependency-compatibility branch has the exact tree merged by #50. Their
local refs and the three remaining corresponding remote refs were removed.
Existing worktree files remain in place; the uncommitted review work and its
index were preserved unchanged. A local Git bundle and uncommitted-file backup
were retained outside the repository before deleting these refs.

After the remaining updates pass the required checks and merge, delete their
superseded remote branch refs and this integration branch as well. No branch
with a changed or unreviewed head is included in deletion.

## References

- [Microsoft: running TypeScript 7 alongside 6](https://devblogs.microsoft.com/typescript/announcing-typescript-7-0/)
- [typescript-eslint dependency support](https://typescript-eslint.io/users/dependency-versions/)
- [AGP 9.4 compatibility](https://developer.android.com/build/releases/agp-9-4-0-release-notes)
- [Kotlin Gradle/AGP compatibility](https://kotlinlang.org/docs/gradle-configure-project.html)

Full local and required cross-platform validation results are recorded in the
integration PR. A successful dependency-resolution report alone does not
qualify these changes for merging.
