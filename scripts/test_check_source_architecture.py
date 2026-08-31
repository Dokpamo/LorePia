import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT_DIRECTORY = Path(__file__).resolve().parent
if str(SCRIPT_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIRECTORY))

from check_source_architecture import (
    SourceChange,
    SourceSize,
    aggregate_changes,
    aggregate_parent_child_groups,
    baseline_parent_key,
    classify_source,
    evaluate_baseline_changes,
    evaluate_character_runtime_transform_boundary,
    evaluate_core_storage_api_baseline_changes,
    evaluate_core_storage_public_reexports,
    evaluate_source_sizes,
    evaluate_test_baseline_changes,
    evaluate_test_source_sizes,
    generated_sources,
    is_test_source,
    load_config,
    parent_child_group_deltas,
    require_v2_bootstrap_transition,
    source_directory_key,
    strip_rust_comments_and_strings,
    test_sources,
)


BOOTSTRAP_REF = "0" * 40
SOURCE_LANGUAGES = ("css", "kotlin", "lua", "rust", "svelte", "swift", "typescript")
TEST_LANGUAGES = ("kotlin", "rust", "svelte", "swift", "typescript")


def limits(languages: tuple[str, ...], *, bytes_limit: int = 100, lines: int = 5):
    return {
        language: {"bytes": bytes_limit, "lines": lines}
        for language in languages
    }


def source_config(
    *,
    baselines: dict[str, dict[str, int]],
    facade_paths: list[str] | None = None,
    parent_child_groups: dict[str, list[str]] | None = None,
) -> dict:
    return {
        "version": 2,
        "bootstrap_ref": BOOTSTRAP_REF,
        "facade_paths": facade_paths or [],
        "parent_child_groups": parent_child_groups or {},
        "limits": {
            "facade": limits(SOURCE_LANGUAGES),
            "generated": limits(SOURCE_LANGUAGES),
            "production": limits(SOURCE_LANGUAGES),
        },
        "baselines": baselines,
    }


def test_config(*, baselines: dict[str, dict[str, int]]) -> dict:
    return {
        "version": 2,
        "bootstrap_ref": BOOTSTRAP_REF,
        "limits": {"test": limits(TEST_LANGUAGES)},
        "baselines": baselines,
    }


def write_config(
    root: Path,
    *,
    baselines: dict[str, dict[str, int]],
    facade_paths: list[str] | None = None,
    parent_child_groups: dict[str, list[str]] | None = None,
) -> Path:
    config = root / "source-size-baseline.json"
    config.write_text(
        json.dumps(
            source_config(
                baselines=baselines,
                facade_paths=facade_paths,
                parent_child_groups=parent_child_groups,
            )
        ),
        encoding="utf-8",
    )
    return config


def write_test_config(root: Path, *, baselines: dict[str, dict[str, int]]) -> Path:
    config = root / "test-source-size-baseline.json"
    config.write_text(
        json.dumps(test_config(baselines=baselines)),
        encoding="utf-8",
    )
    return config


class SourceArchitectureTests(unittest.TestCase):
    def test_core_cannot_implicitly_apply_character_runtime_native_transforms(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "crates" / "core" / "src" / "orchestration.rs"
            source.parent.mkdir(parents=True)
            source.write_text(
                "fn unsafe_projection(content: CharacterContent) {\n"
                "    let _ = content.runtime.transform_set_id;\n"
                "}\n",
                encoding="utf-8",
            )

            failures = evaluate_character_runtime_transform_boundary(root)

            self.assertEqual(len(failures), 1)
            self.assertIn("revision-bound grant", failures[0])

            source.write_text(
                "// content.runtime.transform_set_id stays on the frontend grant path.\n"
                "fn safe_prompt_transforms() {}\n",
                encoding="utf-8",
            )
            self.assertEqual(evaluate_character_runtime_transform_boundary(root), [])

    def test_core_transform_boundary_scans_moved_alias_and_destructuring_access(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "crates" / "core" / "src" / "app.rs"
            source.parent.mkdir(parents=True)
            source.write_text(
                "fn unsafe_alias(content: CharacterContent) {\n"
                "    let runtime = content.runtime;\n"
                "    let _ = runtime.transform_set_id;\n"
                "}\n",
                encoding="utf-8",
            )
            failures = evaluate_character_runtime_transform_boundary(root)
            self.assertEqual(len(failures), 1)
            self.assertIn("crates/core/src/app.rs", failures[0])

            source.write_text(
                "fn unsafe_destructure(content: CharacterContent) {\n"
                "    let CharacterRuntime { transform_set_id, .. } = content.runtime;\n"
                "    drop(transform_set_id);\n"
                "}\n",
                encoding="utf-8",
            )
            failures = evaluate_character_runtime_transform_boundary(root)
            self.assertEqual(len(failures), 1)
            self.assertIn("crates/core/src/app.rs", failures[0])

    def test_core_cannot_add_storage_persistence_row_reexports(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "crates" / "core" / "src" / "lib.rs"
            source.parent.mkdir(parents=True)
            source.write_text(
                "pub use lorepia_storage::{DatabaseStats, "
                "StoredNewPersistenceRow as CoreAlias};\n",
                encoding="utf-8",
            )

            failures = evaluate_core_storage_public_reexports(root, set())

            self.assertEqual(len(failures), 1)
            self.assertIn("StoredNewPersistenceRow", failures[0])

    def test_core_cannot_wildcard_reexport_storage(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "crates" / "core" / "src" / "facade.rs"
            source.parent.mkdir(parents=True)
            source.write_text("pub use lorepia_storage::*;\n", encoding="utf-8")

            failures = evaluate_core_storage_public_reexports(root, set())

            self.assertEqual(len(failures), 1)
            self.assertIn("wildcard-reexport", failures[0])

    def test_core_storage_reexport_baseline_must_shrink_with_source(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "crates" / "core" / "src" / "lib.rs"
            source.parent.mkdir(parents=True)
            source.write_text("pub use lorepia_storage::DatabaseStats;\n", encoding="utf-8")

            failures = evaluate_core_storage_public_reexports(
                root, {"StoredRemovedPersistenceRow"}
            )

            self.assertEqual(len(failures), 1)
            self.assertIn("StoredRemovedPersistenceRow", failures[0])

    def test_empty_core_storage_reexport_baseline_cannot_regrow(self) -> None:
        base = {"version": 1, "allowed_stored_reexports": []}
        current = {
            "version": 1,
            "allowed_stored_reexports": ["StoredNewRow"],
        }

        failures = evaluate_core_storage_api_baseline_changes(current, base)

        self.assertEqual(len(failures), 1)
        self.assertIn("StoredNewRow", failures[0])

    def test_rust_comments_and_strings_do_not_create_reexports(self) -> None:
        content = (
            '// pub use lorepia_storage::StoredComment;\n'
            'const EXAMPLE: &str = r#"pub use lorepia_storage::StoredString;"#;\n'
            'pub use lorepia_storage::StoredVisible;\n'
        )

        stripped = strip_rust_comments_and_strings(content)

        self.assertNotIn("StoredComment", stripped)
        self.assertNotIn("StoredString", stripped)
        self.assertIn("StoredVisible", stripped)

    def test_existing_giant_may_shrink_but_must_not_grow(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "crates" / "sample" / "src" / "giant.rs"
            source.parent.mkdir(parents=True)
            original = "fn item() {}\n" * 6
            source.write_text(original, encoding="utf-8")
            relative = source.relative_to(root).as_posix()
            config = write_config(
                root,
                baselines={
                    relative: {
                        "bytes": len(original.encode("utf-8")),
                        "lines": 6,
                    }
                },
            )

            self.assertEqual(evaluate_source_sizes(root, config)[0], [])

            source.write_text(original + "fn grew() {}\n", encoding="utf-8")
            self.assertIn("grew beyond its baseline", evaluate_source_sizes(root, config)[0][0])

            source.write_text("fn smaller() {}\n", encoding="utf-8")
            self.assertEqual(evaluate_source_sizes(root, config)[0], [])

    def test_new_oversized_production_source_fails(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "apps" / "lorepia" / "src" / "new-module.ts"
            source.parent.mkdir(parents=True)
            source.write_text("export const value = 1;\n" * 6, encoding="utf-8")
            config = write_config(root, baselines={})

            failures, _ = evaluate_source_sizes(root, config)

            self.assertEqual(len(failures), 1)
            self.assertIn("production:typescript source exceeds", failures[0])

    def test_production_file_cannot_hide_under_migration_named_directory(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "crates" / "sample" / "src" / "migrations" / "hidden.rs"
            source.parent.mkdir(parents=True)
            source.write_text("pub fn value() {}\n" * 6, encoding="utf-8")
            config = write_config(root, baselines={})

            failures, _ = evaluate_source_sizes(root, config)

            self.assertEqual(len(failures), 1)
            self.assertIn("production:rust source exceeds", failures[0])

    def test_test_sources_do_not_count_as_production_but_native_main_sources_do(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            sources = [
                root / "crates" / "sample" / "src" / "tests" / "reachable.rs",
                root
                / "plugins"
                / "sample"
                / "android"
                / "src"
                / "main"
                / "java"
                / "Reachable.kt",
                root / "plugins" / "sample" / "ios" / "Sources" / "Reachable.swift",
            ]
            for source in sources:
                source.parent.mkdir(parents=True, exist_ok=True)
                source.write_text("public value\n" * 6, encoding="utf-8")
            config = write_config(root, baselines={})

            failures, production = evaluate_source_sizes(root, config)

            self.assertEqual(len(failures), 2)
            self.assertNotIn(sources[0].relative_to(root).as_posix(), {item.path for item in production})

            test_config = write_test_config(root, baselines={})
            test_failures, _ = evaluate_test_source_sizes(root, test_config)
            self.assertEqual(len(test_failures), 1)
            self.assertIn(sources[0].relative_to(root).as_posix(), test_failures[0])

    def test_frontend_rust_android_and_ios_tests_are_classified(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = [
                root / "apps/lorepia/src/feature.test.ts",
                root / "apps/lorepia/src/tests/support.ts",
                root / "apps/lorepia/src-tauri/tests/contract.rs",
                root / "crates/sample/tests/integration.rs",
                root / "crates/sample/src/feature/tests.rs",
                root / "crates/sample/src/feature/child_tests.rs",
                root / "plugins/sample/android/src/test/java/PluginTest.kt",
                root / "plugins/sample/android/src/androidTest/java/PluginDeviceTest.kt",
                root / "plugins/sample/ios/Tests/PluginTests.swift",
            ]
            for path in paths:
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("test\n", encoding="utf-8")

            observed = test_sources(root)

            self.assertEqual(observed, sorted(path.relative_to(root) for path in paths))
            self.assertTrue(all(is_test_source(path.relative_to(root)) for path in paths))

    def test_existing_test_may_shrink_but_must_not_grow(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "crates/sample/tests/giant.rs"
            source.parent.mkdir(parents=True)
            original = "fn item() {}\n" * 6
            source.write_text(original, encoding="utf-8")
            relative = source.relative_to(root).as_posix()
            config = write_test_config(
                root,
                baselines={
                    relative: {
                        "bytes": len(original.encode("utf-8")),
                        "lines": 6,
                    }
                },
            )

            self.assertEqual(evaluate_test_source_sizes(root, config)[0], [])
            source.write_text(original + "fn grew() {}\n", encoding="utf-8")
            self.assertIn(
                "grew beyond its test baseline",
                evaluate_test_source_sizes(root, config)[0][0],
            )

    def test_test_baseline_caps_and_exceptions_cannot_grow(self) -> None:
        base = {
            "version": 1,
            "new_test_file_limits": {"bytes": 100, "lines": 5},
            "baselines": {"crates/sample/tests/a.rs": {"bytes": 200, "lines": 10}},
        }
        current = {
            "version": 1,
            "new_test_file_limits": {"bytes": 101, "lines": 5},
            "baselines": {
                "crates/sample/tests/a.rs": {"bytes": 201, "lines": 10},
                "crates/sample/tests/b.rs": {"bytes": 300, "lines": 20},
            },
        }

        failures = evaluate_test_baseline_changes(current, base)

        self.assertEqual(len(failures), 3)

    def test_frontend_production_cannot_import_excluded_test_sources(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source_root = root / "apps" / "lorepia" / "src"
            (source_root / "tests").mkdir(parents=True)
            (source_root / "tests" / "reachable.ts").write_text(
                "export const hidden = 1;\n", encoding="utf-8"
            )
            (source_root / "reachable.test.ts").write_text(
                "export const hidden = 2;\n", encoding="utf-8"
            )
            entry = source_root / "entry.ts"
            entry_text = (
                "import './tests/reachable';\n"
                "import './reachable.test';\n"
                "import/* split token */('.\\\\/tests/reachable');\n"
                "const hidden = import.meta/* split token */.glob(\n"
                "    '.\\u002ftests/*.ts', { eager: true }\n"
                ");\n"
                "const dynamic = import(`./tests/reachable.ts`);\n"
                "const folder = 'tests';\n"
                "const hiddenFolder = import(`./${folder}/reachable.ts`);\n"
                "const kind = 'test';\n"
                "const hiddenSuffix = import(`./reachable.${kind}.ts`);\n"
            )
            entry.write_text(entry_text, encoding="utf-8")
            config = write_config(
                root,
                baselines={
                    entry.relative_to(root).as_posix(): {
                        "bytes": len(entry_text.encode("utf-8")),
                        "lines": len(entry_text.splitlines()),
                    }
                },
            )

            failures, _ = evaluate_source_sizes(root, config)

            self.assertEqual(len(failures), 7)
            self.assertTrue(all("excluded test source" in failure for failure in failures))

    def test_portable_regex_evaluator_is_worker_only_in_production(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source_root = root / "apps" / "lorepia" / "src" / "features" / "chat"
            source_root.mkdir(parents=True)
            operation = source_root / "portable-regex-operation.ts"
            operation.write_text("export function evaluate() {}\n", encoding="utf-8")
            worker = source_root / "portable-regex.worker.ts"
            worker.write_text(
                "import { evaluate } from './portable-regex-operation';\n",
                encoding="utf-8",
            )
            renderer = source_root / "portable-display.ts"
            renderer.write_text(
                "import { evaluate } from './portable-regex-operation';\n",
                encoding="utf-8",
            )
            config = write_config(root, baselines={})

            failures, _ = evaluate_source_sizes(root, config)

            boundary_failures = [
                failure for failure in failures if "Worker-only portable regex" in failure
            ]
            self.assertEqual(
                boundary_failures,
                [
                    "apps/lorepia/src/features/chat/portable-display.ts imports the "
                    "Worker-only portable regex evaluator"
                ],
            )

    def test_base_revision_prevents_cap_increases_and_new_exceptions(self) -> None:
        base = {
            "version": 1,
            "new_file_limits": {"bytes": 100, "lines": 5},
            "baselines": {"crates/sample/src/giant.rs": {"bytes": 200, "lines": 10}},
        }
        current = {
            "version": 1,
            "new_file_limits": {"bytes": 101, "lines": 5},
            "baselines": {
                "crates/sample/src/giant.rs": {"bytes": 201, "lines": 10},
                "crates/sample/src/new-giant.rs": {"bytes": 300, "lines": 20},
            },
        }

        failures = evaluate_baseline_changes(current, base)

        self.assertEqual(len(failures), 3)

    def test_v2_classifies_generated_test_facade_and_language_before_production(self) -> None:
        orchestration_facade = (
            "apps/lorepia/src/features/orchestration/orchestration-controller.ts"
        )
        facade_paths = {"crates/sample/src/stable.rs", orchestration_facade}

        self.assertEqual(
            classify_source(
                Path("apps/lorepia/src/lib/ipc/commands.generated.ts"), facade_paths
            ),
            ("generated", "typescript"),
        )
        self.assertEqual(
            classify_source(Path("apps/lorepia/src/index.test.ts"), facade_paths),
            ("test", "typescript"),
        )
        self.assertEqual(
            classify_source(Path("crates/sample/src/stable.rs"), facade_paths),
            ("facade", "rust"),
        )
        self.assertEqual(
            classify_source(Path("crates/sample/src/lib.rs"), set()),
            ("facade", "rust"),
        )
        self.assertEqual(
            classify_source(Path(orchestration_facade), facade_paths),
            ("facade", "typescript"),
        )
        self.assertEqual(
            classify_source(Path("crates/sample/src/feature.rs"), facade_paths),
            ("production", "rust"),
        )

    def test_generated_registries_are_scanned_with_generated_limits(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = [
                root / "apps/lorepia/src/lib/ipc/commands.generated.ts",
                root / "apps/lorepia/src-tauri/generated/app_commands.rs",
            ]
            for path in paths:
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("generated\n" * 6, encoding="utf-8")
            config = write_config(root, baselines={})

            failures, _ = evaluate_source_sizes(root, config)

            self.assertEqual(
                generated_sources(root), sorted(path.relative_to(root) for path in paths)
            )
            self.assertEqual(len(failures), 2)
            self.assertTrue(all("generated:" in failure for failure in failures))

    def test_explicit_facade_uses_stricter_kind_limit(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "crates/sample/src/stable.rs"
            source.parent.mkdir(parents=True)
            source.write_text("pub fn stable() {}\n" * 6, encoding="utf-8")
            data = source_config(
                baselines={}, facade_paths=[source.relative_to(root).as_posix()]
            )
            data["limits"]["production"]["rust"]["lines"] = 10
            config = root / "source-size-baseline.json"
            config.write_text(json.dumps(data), encoding="utf-8")

            failures, _ = evaluate_source_sizes(root, config)

            self.assertEqual(len(failures), 1)
            self.assertIn("facade:rust source exceeds", failures[0])

    def test_v2_limit_baseline_bootstrap_and_facade_ratchets_cannot_weaken(self) -> None:
        base = source_config(
            baselines={"crates/sample/src/legacy.rs": {"bytes": 200, "lines": 10}},
            facade_paths=["crates/sample/src/stable.rs"],
        )
        current = json.loads(json.dumps(base))
        current["bootstrap_ref"] = "1" * 40
        current["facade_paths"] = []
        current["limits"]["production"]["rust"]["bytes"] = 101
        current["baselines"]["crates/sample/src/new.rs"] = {
            "bytes": 300,
            "lines": 20,
        }

        failures = evaluate_baseline_changes(current, base)

        self.assertEqual(len(failures), 4)
        self.assertTrue(any("bootstrap_ref" in failure for failure in failures))
        self.assertTrue(any("facade classification" in failure for failure in failures))
        self.assertTrue(any("limit increased" in failure for failure in failures))
        self.assertTrue(any("new baseline exception" in failure for failure in failures))

    def test_v2_parent_child_groups_cannot_shrink(self) -> None:
        parent = "crates/sample/src/stable.rs"
        base = source_config(
            baselines={},
            parent_child_groups={
                parent: [
                    "crates/sample/src/stable-child.rs",
                    "crates/sample/src/stable/",
                ]
            },
        )
        current = json.loads(json.dumps(base))
        current["parent_child_groups"][parent] = [
            "crates/sample/src/stable/"
        ]

        entry_failures = evaluate_baseline_changes(current, base)
        del current["parent_child_groups"][parent]
        group_failures = evaluate_baseline_changes(current, base)

        self.assertEqual(len(entry_failures), 1)
        self.assertIn("aggregate entry cannot be removed", entry_failures[0])
        self.assertEqual(len(group_failures), 1)
        self.assertIn("aggregate group cannot be removed", group_failures[0])

    def test_v1_to_v2_bootstrap_may_capture_existing_files_without_raising_caps(self) -> None:
        base = {
            "version": 1,
            "new_file_limits": {"bytes": 100, "lines": 5},
            "baselines": {},
        }
        current = source_config(
            baselines={"crates/sample/src/existing.rs": {"bytes": 200, "lines": 10}}
        )
        bootstrap = {
            "version": 1,
            "new_file_limits": {"bytes": 100, "lines": 5},
            "baselines": {
                "crates/sample/src/existing.rs": {"bytes": 200, "lines": 10}
            },
        }

        self.assertEqual(
            evaluate_baseline_changes(current, base, bootstrap=bootstrap), []
        )
        self.assertIn(
            "new baseline exception",
            evaluate_baseline_changes(current, base)[0],
        )

    def test_v2_bootstrap_transition_allows_only_enforcement_paths(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            checker = root / "scripts/check_source_architecture.py"
            production = root / "crates/sample/src/lib.rs"
            checker.parent.mkdir(parents=True)
            production.parent.mkdir(parents=True)
            checker.write_text("before\n", encoding="utf-8")
            production.write_text("before\n", encoding="utf-8")
            subprocess.run(["git", "init", "-q"], cwd=root, check=True)
            subprocess.run(
                ["git", "config", "user.email", "test@example.invalid"],
                cwd=root,
                check=True,
            )
            subprocess.run(
                ["git", "config", "user.name", "Test"], cwd=root, check=True
            )
            subprocess.run(["git", "add", "."], cwd=root, check=True)
            subprocess.run(
                ["git", "commit", "-qm", "bootstrap"], cwd=root, check=True
            )
            bootstrap_ref = subprocess.run(
                ["git", "rev-parse", "HEAD"],
                cwd=root,
                check=True,
                capture_output=True,
                text=True,
            ).stdout.strip()

            checker.write_text("after\n", encoding="utf-8")
            require_v2_bootstrap_transition(root, bootstrap_ref)

            production.write_text("after\n", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "unexpected changed path"):
                require_v2_bootstrap_transition(root, bootstrap_ref)

    def test_v2_config_requires_every_language_for_every_kind(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            data = source_config(baselines={})
            del data["limits"]["generated"]["swift"]
            config = root / "source-size-baseline.json"
            config.write_text(json.dumps(data), encoding="utf-8")

            with self.assertRaisesRegex(ValueError, "must define exactly"):
                load_config(config)

    def test_parent_child_config_requires_sorted_entries(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            data = source_config(
                baselines={},
                parent_child_groups={
                    "crates/sample/src/stable.rs": [
                        "crates/sample/src/z/",
                        "crates/sample/src/a/",
                    ]
                },
            )
            config = root / "source-size-baseline.json"
            config.write_text(json.dumps(data), encoding="utf-8")

            with self.assertRaisesRegex(ValueError, "unique, and sorted"):
                load_config(config)

    def test_parent_child_and_directory_aggregate_deltas_are_sorted(self) -> None:
        changes = [
            SourceChange(
                before_path=Path("crates/core/src/app.rs"),
                before_size=SourceSize(bytes=100, lines=10),
                after_path=Path("crates/core/src/app.rs"),
                after_size=SourceSize(bytes=20, lines=2),
            ),
            SourceChange(
                before_path=None,
                before_size=None,
                after_path=Path("crates/core/src/app/generation.rs"),
                after_size=SourceSize(bytes=80, lines=8),
            ),
            SourceChange(
                before_path=Path("apps/lorepia/src/old.ts"),
                before_size=SourceSize(bytes=10, lines=1),
                after_path=None,
                after_size=None,
            ),
        ]

        directories = aggregate_changes(changes, key_for_path=source_directory_key)
        parents = aggregate_changes(
            changes,
            key_for_path=lambda path: baseline_parent_key(
                path, {"crates/core/src/app.rs"}
            ),
        )
        groups = aggregate_parent_child_groups(
            {
                Path("crates/core/src/app.rs"): SourceSize(bytes=100, lines=10),
                Path("crates/core/src/app/existing.rs"): SourceSize(
                    bytes=40, lines=4
                ),
                Path("crates/core/src/app-support.rs"): SourceSize(
                    bytes=30, lines=3
                ),
                Path("crates/core/src/unrelated.rs"): SourceSize(
                    bytes=900, lines=90
                ),
            },
            {
                Path("crates/core/src/app.rs"): SourceSize(bytes=20, lines=2),
                Path("crates/core/src/app/existing.rs"): SourceSize(
                    bytes=50, lines=5
                ),
                Path("crates/core/src/app/generation.rs"): SourceSize(
                    bytes=80, lines=8
                ),
                Path("crates/core/src/app-support.rs"): SourceSize(
                    bytes=35, lines=4
                ),
                Path("crates/core/src/unrelated.rs"): SourceSize(
                    bytes=900, lines=90
                ),
            },
            {
                "crates/core/src/app.rs": [
                    "crates/core/src/app-support.rs",
                    "crates/core/src/app/",
                ]
            },
        )

        self.assertEqual([item.path for item in directories], ["apps/lorepia/src", "crates/core/src"])
        self.assertEqual(
            (parents[0].before_files, parents[0].after_files),
            (1, 1),
        )
        self.assertEqual(
            (parents[0].before_bytes, parents[0].after_bytes),
            (100, 20),
        )
        self.assertEqual(
            (groups[0].before_files, groups[0].after_files),
            (3, 4),
        )
        self.assertEqual(
            (groups[0].before_bytes, groups[0].after_bytes),
            (170, 185),
        )

    def test_git_parent_child_aggregate_covers_full_trees_and_stale_entries(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            parent = Path("crates/sample/src/facade.rs")
            prefix = "crates/sample/src/facade/"
            existing = Path(f"{prefix}existing.rs")
            deleted = Path(f"{prefix}deleted.rs")
            outgoing = Path(f"{prefix}outgoing.rs")
            incoming = Path("crates/sample/src/incoming.rs")
            base_contents = {
                parent: "pub fn facade() {}\n",
                existing: "before\n",
                deleted: "deleted\n",
                outgoing: "outgoing\n",
                incoming: "incoming\n",
            }
            for path, contents in base_contents.items():
                absolute = root / path
                absolute.parent.mkdir(parents=True, exist_ok=True)
                absolute.write_text(contents, encoding="utf-8")
            subprocess.run(["git", "init", "-q"], cwd=root, check=True)
            subprocess.run(
                ["git", "config", "user.email", "test@example.invalid"],
                cwd=root,
                check=True,
            )
            subprocess.run(
                ["git", "config", "user.name", "Test"], cwd=root, check=True
            )
            subprocess.run(["git", "add", "."], cwd=root, check=True)
            subprocess.run(["git", "commit", "-qm", "base"], cwd=root, check=True)
            base_ref = subprocess.run(
                ["git", "rev-parse", "HEAD"],
                cwd=root,
                check=True,
                capture_output=True,
                text=True,
            ).stdout.strip()

            existing_after = "after expanded\n"
            generated_contents = "generated\n"
            test_contents = "#[test]\nfn check() {}\n"
            (root / existing).write_text(existing_after, encoding="utf-8")
            (root / deleted).unlink()
            (root / outgoing).rename(root / "crates/sample/src/outgoing.rs")
            incoming_child = root / f"{prefix}incoming.rs"
            (root / incoming).rename(incoming_child)
            (root / f"{prefix}registry.generated.rs").write_text(
                generated_contents, encoding="utf-8"
            )
            (root / f"{prefix}tests.rs").write_text(test_contents, encoding="utf-8")

            aggregates = parent_child_group_deltas(
                root,
                base_ref,
                facade_paths={parent.as_posix()},
                groups={parent.as_posix(): [prefix]},
            )

            before_group = [
                base_contents[parent],
                base_contents[existing],
                base_contents[deleted],
                base_contents[outgoing],
            ]
            after_group = [
                base_contents[parent],
                existing_after,
                base_contents[incoming],
                generated_contents,
                test_contents,
            ]
            self.assertEqual(
                (aggregates[0].before_files, aggregates[0].after_files), (4, 5)
            )
            self.assertEqual(
                (aggregates[0].before_bytes, aggregates[0].after_bytes),
                (
                    sum(len(contents.encode()) for contents in before_group),
                    sum(len(contents.encode()) for contents in after_group),
                ),
            )
            self.assertEqual(
                (aggregates[0].before_lines, aggregates[0].after_lines), (4, 6)
            )

            data = source_config(
                baselines={},
                facade_paths=[parent.as_posix()],
                parent_child_groups={
                    parent.as_posix(): [
                        f"{prefix}missing.rs",
                        "crates/sample/src/missing/",
                    ]
                },
            )
            data["bootstrap_ref"] = base_ref
            config = root / "source-size-baseline.json"
            config.write_text(json.dumps(data), encoding="utf-8")

            failures, _ = evaluate_source_sizes(root, config)
            stale_failures = [
                failure
                for failure in failures
                if failure.startswith("stale parent-child source entry:")
            ]
            self.assertEqual(len(stale_failures), 2)


if __name__ == "__main__":
    unittest.main()
