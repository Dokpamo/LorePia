# Main integration review — 2026-09-10

## Scope and preservation

- UI checkpoint: `cb9ea67` on `codex/tauri-chat-preview`.
- Main character overview no longer renders the thumbnail strip. Hero image swiping and the original-image viewer remain; the viewer keeps its centered thumbnail navigation.
- Integration starts from that UI checkpoint. `main` was `6f22761`; remote `main` was `ba5fd68`.
- Review branch `3c39090` captures the earlier workspace. Its lineage is merged using the current tree because the extraction and native fixes were already incorporated by the later UI checkpoints.
- Compared 352 paths from the review commit plus its uncommitted work: 256 are byte-identical, 95 have later implementations, and 1 is a review-only report copied into this integration.
- Later implementations include the four root tabs, character galleries, conversation setup, shared sheets, scroll-aware headers, typed settings handlers, full PNG validation, and the greeting-detail command. The aggregate storage-count client regression test is retained in `storage-overview-client.test.ts`.
- The other review worktree and its index remain untouched. No branch is deleted, rebased, or force-pushed.

## Automated branch decisions

Ten update branches are included for validation. Fifteen remain open for separate, coordinated changes. Passing an old PR run is not enough to approve a changed archive boundary or a dependency declaration without its checked-in contract.

| PR | Update | Decision | Evidence / follow-up |
| --- | --- | --- | --- |
| [#3](https://github.com/Dokpamo/LorePia/pull/3) | chore(deps-dev): bump eslint-plugin-svelte from 3.22.0 to 3.23.0 in /apps/lorepia | Included | Patch/minor update; run the full gate against the combined current UI. |
| [#4](https://github.com/Dokpamo/LorePia/pull/4) | chore(deps): bump androidx.test.espresso:espresso-core from 3.5.0 to 3.7.0 in /apps/lorepia/src-tauri/gen/android | Held | Android declaration-only change omits the matching dependency locks and strict-verification metadata; review the resolved graph and update integrity artifacts together. |
| [#5](https://github.com/Dokpamo/LorePia/pull/5) | chore(deps): bump svelte from 5.56.8 to 5.56.10 in /apps/lorepia | Included | Patch/minor update; run the full gate against the combined current UI. |
| [#6](https://github.com/Dokpamo/LorePia/pull/6) | chore(deps): bump androidx.test.ext:junit from 1.1.4 to 1.3.0 in /apps/lorepia/src-tauri/gen/android | Held | Android declaration-only change omits the matching dependency locks and strict-verification metadata; review the resolved graph and update integrity artifacts together. |
| [#7](https://github.com/Dokpamo/LorePia/pull/7) | chore(deps-dev): bump typescript from 6.0.3 to 7.0.2 in /apps/lorepia | Held | svelte-check 4.7.4 requires TypeScript 5 or 6; npm dependency resolution fails with 7. |
| [#8](https://github.com/Dokpamo/LorePia/pull/8) | chore(deps): bump zip from 4.6.1 to 8.6.0 | Held | ZIP 4 → 8 changes an untrusted archive boundary; requires a dependency-contract update and a dedicated archive compatibility review. |
| [#9](https://github.com/Dokpamo/LorePia/pull/9) | chore(deps): bump androidx.webkit:webkit from 1.14.0 to 1.17.0 in /apps/lorepia/src-tauri/gen/android | Held | Android declaration-only change omits the matching dependency locks and strict-verification metadata; review the resolved graph and update integrity artifacts together. |
| [#10](https://github.com/Dokpamo/LorePia/pull/10) | chore(deps-dev): bump @testing-library/jest-dom from 7.0.0 to 7.0.1 in /apps/lorepia | Included | Patch/minor update; run the full gate against the combined current UI. |
| [#11](https://github.com/Dokpamo/LorePia/pull/11) | chore(deps): bump sha2 from 0.10.9 to 0.11.0 | Held | sha2 0.11 digest no longer implements the LowerHex contract used by stable hashes; Rust compilation fails. |
| [#12](https://github.com/Dokpamo/LorePia/pull/12) | chore(deps-dev): bump typescript-eslint from 8.65.0 to 8.68.0 in /apps/lorepia | Included | Patch/minor update; run the full gate against the combined current UI. |
| [#13](https://github.com/Dokpamo/LorePia/pull/13) | chore(deps): bump rusqlite from 0.37.0 to 0.40.2 | Held | rusqlite 0.40 removes u64 SQL conversions used throughout Storage; all three Rust jobs fail. |
| [#14](https://github.com/Dokpamo/LorePia/pull/14) | chore(deps): bump gradle-wrapper from 8.14.3 to 9.7.1 in /apps/lorepia/src-tauri/gen/android | Held | Wrapper checksum pin does not match the new Gradle distribution; AGP compatibility needs a coordinated upgrade. |
| [#15](https://github.com/Dokpamo/LorePia/pull/15) | chore(deps): bump androidx.appcompat:appcompat from 1.7.1 to 1.8.0 in /apps/lorepia/src-tauri/gen/android | Held | Android declaration-only change omits the matching dependency locks and strict-verification metadata; review the resolved graph and update integrity artifacts together. |
| [#16](https://github.com/Dokpamo/LorePia/pull/16) | chore(deps): bump base64 from 0.22.1 to 0.23.1 | Held | Cargo declaration change needs an explicitly reviewed dependency-contract update and decoding compatibility review. |
| [#17](https://github.com/Dokpamo/LorePia/pull/17) | chore(deps): bump psl from 2.1.223 to 2.1.226 | Included | PSL patch only; Cargo declarations unchanged; verify provider-policy and cross-platform tests. |
| [#18](https://github.com/Dokpamo/LorePia/pull/18) | chore(deps): bump github/codeql-action/init from 3.37.9 to 4.37.9 | Included | Update init and analyze together to the same v4.37.9 SHA; the isolated PR failed because v3 and v4 were mixed. |
| [#19](https://github.com/Dokpamo/LorePia/pull/19) | chore(deps): bump actions/checkout from 6.1.0 to 7.0.1 | Included | Pinned official action update; preserve permissions and persist-credentials: false; require current GitHub checks. |
| [#20](https://github.com/Dokpamo/LorePia/pull/20) | chore(deps): bump actions/setup-python from 6.3.0 to 7.0.0 | Included | Pinned official action update; preserve permissions and persist-credentials: false; require current GitHub checks. |
| [#21](https://github.com/Dokpamo/LorePia/pull/21) | chore(deps): bump actions/setup-java from 5.7.0 to 6.0.0 | Included | Pinned action update; repair the test fixture that depended on an old SHA without changing the security checker. |
| [#22](https://github.com/Dokpamo/LorePia/pull/22) | chore(deps): bump org.jetbrains.kotlin.android from 1.9.25 to 2.4.10 in /plugins/lorepia-platform/android | Held | Strict Gradle verification rejects the new Kotlin plugin marker; verification metadata is absent. |
| [#23](https://github.com/Dokpamo/LorePia/pull/23) | chore(deps): bump androidx.test:runner from 1.6.2 to 1.7.0 in /plugins/lorepia-platform/android | Held | Android declaration-only change omits the matching dependency locks and strict-verification metadata; review the resolved graph and update integrity artifacts together. |
| [#24](https://github.com/Dokpamo/LorePia/pull/24) | chore(deps): bump com.android.library from 8.11.0 to 9.3.2 in /plugins/lorepia-platform/android | Held | Strict Gradle verification rejects the new AGP plugin marker; verification metadata is absent. |
| [#25](https://github.com/Dokpamo/LorePia/pull/25) | chore(deps): bump androidx.test.ext:junit from 1.2.1 to 1.3.0 in /plugins/lorepia-platform/android | Held | Android declaration-only change omits the matching dependency locks and strict-verification metadata; review the resolved graph and update integrity artifacts together. |
| [#26](https://github.com/Dokpamo/LorePia/pull/26) | chore(deps): bump androidx.appcompat:appcompat from 1.6.0 to 1.8.0 in /plugins/lorepia-platform/android | Held | Android declaration-only change omits the matching dependency locks and strict-verification metadata; review the resolved graph and update integrity artifacts together. |
| [#27](https://github.com/Dokpamo/LorePia/pull/27) | chore(deps): bump github/codeql-action/analyze from 3.37.9 to 4.37.9 | Included | Update init and analyze together to the same v4.37.9 SHA; the isolated PR failed because v3 and v4 were mixed. |

## Validation policy

- The repository pre-merge Rust, frontend, IPC, architecture, i18n, archived-refactoring, and workflow checks run on the integration worktree.
- Current UI regression coverage must retain hero swiping, fullscreen thumbnails, character-specific conversations, persona selection, sheets, and scroll-aware navigation.
- Main remains protected by all ten required GitHub checks; local checks do not replace them.
- No new Cargo declaration, public API, schema, golden fixture, architecture limit, or security waiver is introduced by dependency consolidation.

## Official action release references

- [Checkout v7.0.1](https://github.com/actions/checkout/releases/tag/v7.0.1)
- [Setup Java v6.0.0](https://github.com/actions/setup-java/releases/tag/v6.0.0)
- [Setup Python v7.0.0](https://github.com/actions/setup-python/releases/tag/v7.0.0)
- [CodeQL action v4.37.9](https://github.com/github/codeql-action/releases/tag/v4.37.9)
