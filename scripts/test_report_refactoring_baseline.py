import json
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT_DIRECTORY = Path(__file__).resolve().parent
if str(SCRIPT_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIRECTORY))

from report_refactoring_baseline import (
    build_report,
    ipc_command_names,
    serialized_report,
    source_files,
)


class RefactoringBaselineReportTests(unittest.TestCase):
    def make_root(self, temporary: str) -> Path:
        root = Path(temporary)
        (root / "apps/lorepia/src").mkdir(parents=True)
        (root / "apps/lorepia/src-tauri/src").mkdir(parents=True)
        (root / "crates/sample/src").mkdir(parents=True)
        (root / "plugins/sample/src").mkdir(parents=True)
        (root / "config").mkdir()
        (root / "crates/core/src").mkdir(parents=True)
        (root / "crates/shell-api/src").mkdir(parents=True)
        (root / "crates/core/src/lib.rs").write_text(
            "pub const CORE_API_VERSION: u32 = 10;\n", encoding="utf-8"
        )
        (root / "crates/shell-api/src/lib.rs").write_text(
            "pub const SHELL_API_VERSION: u32 = 3;\n", encoding="utf-8"
        )
        (root / "config/source-size-baseline.json").write_text(
            '{"version": 1}', encoding="utf-8"
        )
        (root / "config/core-storage-public-api-baseline.json").write_text(
            '{"version": 1, "allowed_stored_reexports": []}', encoding="utf-8"
        )
        (root / "config/ipc-commands.json").write_text(
            '{"commands": ["alpha", "beta"]}', encoding="utf-8"
        )
        return root

    def test_report_is_deterministic_and_sorted(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = self.make_root(temporary)
            (root / "crates/sample/src/lib.rs").write_text(
                "pub struct Visible;\nfn hidden() {}\n", encoding="utf-8"
            )
            (root / "apps/lorepia/src/z.ts").write_text(
                "export const value = 1;\n", encoding="utf-8"
            )

            first = build_report(root, "a" * 40)
            second = build_report(root, "a" * 40)

            self.assertEqual(serialized_report(first), serialized_report(second))
            self.assertEqual(first["summary"]["production_files"], 4)
            self.assertEqual(first["summary"]["public_symbols"], 4)
            self.assertEqual(list(first["files"]), sorted(first["files"]))
            self.assertNotIn(str(root), serialized_report(first))

    def test_test_and_generated_sources_are_excluded(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = self.make_root(temporary)
            production = root / "apps/lorepia/src/live.ts"
            production.write_text("export const live = true;\n", encoding="utf-8")
            (root / "apps/lorepia/src/live.test.ts").write_text(
                "export const test = true;\n", encoding="utf-8"
            )
            generated = root / "apps/lorepia/src-tauri/gen/generated.rs"
            generated.parent.mkdir(parents=True)
            generated.write_text("pub fn generated() {}\n", encoding="utf-8")
            child_tests = root / "crates/sample/src/tests/case.rs"
            child_tests.parent.mkdir(parents=True)
            child_tests.write_text("pub fn test_only() {}\n", encoding="utf-8")

            observed = source_files(root)
            self.assertIn(production.relative_to(root), observed)
            self.assertNotIn(
                Path("apps/lorepia/src/live.test.ts"), observed
            )
            self.assertNotIn(
                Path("apps/lorepia/src-tauri/gen/generated.rs"), observed
            )
            self.assertIn(Path("crates/sample/src/tests/case.rs"), observed)

    def test_ipc_commands_must_be_unique_and_sorted(self) -> None:
        self.assertEqual(ipc_command_names({"commands": ["b", "a"]}), ["a", "b"])
        with self.assertRaises(ValueError):
            ipc_command_names({"commands": ["a", "a"]})


if __name__ == "__main__":
    unittest.main()
