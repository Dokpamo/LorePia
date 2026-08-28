import json
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT_DIRECTORY = Path(__file__).resolve().parent
if str(SCRIPT_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIRECTORY))

from check_source_architecture import evaluate_baseline_changes, evaluate_source_sizes


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
