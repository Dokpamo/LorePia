# Profile metadata preparation: existing test modules

Task PROFILE-METADATA-TESTS-20260909. Baseline c49a766, merge-base
6f22761a4305443541452e7409639114f4c10dfd. The working tree contains the ongoing
profile UI feature and previously uncommitted UI/demo work; preserve it.

The character and card adapter source files are at their existing size caps.
Before continuing feature edits, move their existing inline `tests` modules to
`character/tests.rs` and `adapters/tests.rs`. Keep `#[cfg(test)] mod tests;` at
the original locations. No test names, assertions, imports, visibility, callers,
production symbols or public entry points change in this extraction. All
production symbols remain in character.rs and adapters.rs. This is a separate
preparatory extraction; the profile metadata feature and its changed test
expectations are documented separately in profile-gallery-20260909.md.

Inventory: `rg` identified the single trailing `mod tests` in each file, all
`#[test]` functions and the adapter `assert_unsupported` helper. Governing material:
root AGENTS.md and ADR 0006. Expected parent reductions: about 200 and 230 lines,
respectively, with the same contents in test-only children. No cap increases,
baseline exceptions or archived report changes. Semantic risk is module path
resolution; test module names remain identical and `use super::*` retains the
same parent. Relevant checks: Domain and Content unit/integration tests and the
architecture gate. The initial Content run identified one feature-specific
unsupported-field expectation needing an update, unrelated to moving tests.
