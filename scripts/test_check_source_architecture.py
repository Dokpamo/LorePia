import json
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT_DIRECTORY = Path(__file__).resolve().parent
if str(SCRIPT_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIRECTORY))

from check_source_architecture import (
    evaluate_baseline_changes,
    evaluate_character_runtime_transform_boundary,
    evaluate_core_storage_api_baseline_changes,
    evaluate_core_storage_public_reexports,
    evaluate_source_sizes,
    strip_rust_comments_and_strings,
)


def write_config(root: Path, *, baselines: dict[str, dict[str, int]]) -> Path:
    config = root / "source-size-baseline.json"
    config.write_text(
        json.dumps(
            {
                "version": 1,
                "new_file_limits": {"bytes": 100, "lines": 5},
                "baselines": baselines,
            }
        ),
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
            self.assertIn("new production source exceeds", failures[0])

    def test_production_file_cannot_hide_under_migration_named_directory(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "crates" / "sample" / "src" / "migrations" / "hidden.rs"
            source.parent.mkdir(parents=True)
            source.write_text("pub fn value() {}\n" * 6, encoding="utf-8")
            config = write_config(root, baselines={})

            failures, _ = evaluate_source_sizes(root, config)

            self.assertEqual(len(failures), 1)
            self.assertIn("new production source exceeds", failures[0])

    def test_production_file_cannot_hide_under_tests_or_native_source_trees(self) -> None:
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

            failures, _ = evaluate_source_sizes(root, config)

            self.assertEqual(len(failures), len(sources))

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


if __name__ == "__main__":
    unittest.main()
