import json
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT_DIRECTORY = Path(__file__).resolve().parent
if str(SCRIPT_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIRECTORY))

from check_ai_context_map import evaluate_context_map


def manifest(*task_ids: str) -> dict[str, object]:
    return {"version": 1, "tasks": {task_id: {} for task_id in task_ids}}


def entry(*paths: str, limit: int = 250_000) -> dict[str, object]:
    return {
        "paths": {
            "documents": list(paths),
            "entrypoints": [],
            "implementation": [],
            "tests": [],
        },
        "commands": ["python3 -m unittest"],
        "max_context_bytes": limit,
        "baseline_context_bytes": None,
        "over_budget_reason": None,
    }


class AiContextMapTests(unittest.TestCase):
    def make_root(self, temporary: str) -> Path:
        root = Path(temporary).resolve()
        (root / "docs").mkdir()
        (root / "docs/guide.md").write_text("guide\n", encoding="utf-8")
        (root / "src").mkdir()
        (root / "src/feature.rs").write_text("fn feature() {}\n", encoding="utf-8")
        return root

    def base_config(self) -> dict[str, object]:
        return {
            "version": 1,
            "default_max_context_bytes": 250_000,
            "shared_paths": ["docs/guide.md"],
            "contexts": {"GOV-001": entry("src/feature.rs")},
        }

    def test_valid_bundle_reports_deterministic_measurement(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = self.make_root(temporary)
            failures, measurements = evaluate_context_map(
                root, self.base_config(), manifest("GOV-001")
            )
            self.assertEqual(failures, [])
            self.assertEqual([item.task_id for item in measurements], ["GOV-001"])
            self.assertEqual(measurements[0].files, 2)

    def test_manifest_and_context_ids_must_match(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = self.make_root(temporary)
            failures, _ = evaluate_context_map(
                root, self.base_config(), manifest("GOV-001", "GOV-002")
            )
            self.assertIn("missing context entry for manifest task GOV-002", failures)

    def test_missing_duplicate_and_traversal_paths_fail(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = self.make_root(temporary)
            config = self.base_config()
            contexts = config["contexts"]
            assert isinstance(contexts, dict)
            bad = entry("docs/guide.md", "../escape", "src/missing.rs")
            contexts["GOV-001"] = bad
            failures, _ = evaluate_context_map(root, config, manifest("GOV-001"))
            self.assertTrue(any("duplicate bundle path" in item for item in failures))
            self.assertTrue(any("not a canonical" in item for item in failures))
            self.assertTrue(any("missing or escapes" in item for item in failures))

    def test_context_is_limited_to_fifteen_files(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = self.make_root(temporary)
            paths: list[str] = []
            for index in range(15):
                path = root / "src" / f"{index}.rs"
                path.write_text("fn item() {}\n", encoding="utf-8")
                paths.append(path.relative_to(root).as_posix())
            config = self.base_config()
            contexts = config["contexts"]
            assert isinstance(contexts, dict)
            contexts["GOV-001"] = entry(*paths)
            failures, _ = evaluate_context_map(root, config, manifest("GOV-001"))
            self.assertTrue(any("maximum is 15" in item for item in failures))

    def test_legacy_budget_is_a_non_growth_cap(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = self.make_root(temporary)
            large = root / "src/large.rs"
            large.write_bytes(b"x" * 300)
            config = self.base_config()
            config["default_max_context_bytes"] = 100
            contexts = config["contexts"]
            assert isinstance(contexts, dict)
            legacy = entry("src/large.rs", limit=100)
            legacy["baseline_context_bytes"] = 400
            legacy["over_budget_reason"] = "legacy parent pending extraction"
            contexts["GOV-001"] = legacy

            failures, measurements = evaluate_context_map(
                root, config, manifest("GOV-001")
            )
            self.assertEqual(failures, [])
            self.assertTrue(measurements[0].legacy_over_budget)

            strict_failures, _ = evaluate_context_map(
                root, config, manifest("GOV-001"), strict_budget=True
            )
            self.assertTrue(any("above target" in item for item in strict_failures))

            large.write_bytes(b"x" * 500)
            growth_failures, _ = evaluate_context_map(
                root, config, manifest("GOV-001")
            )
            self.assertTrue(any("above allowed cap" in item for item in growth_failures))

    def test_invalid_command_and_budget_shapes_fail(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = self.make_root(temporary)
            config = self.base_config()
            config["default_max_context_bytes"] = 250_001
            contexts = config["contexts"]
            assert isinstance(contexts, dict)
            invalid = entry("src/feature.rs")
            invalid["commands"] = [""]
            invalid["baseline_context_bytes"] = 200_000
            invalid["over_budget_reason"] = "unneeded"
            contexts["GOV-001"] = invalid
            failures, _ = evaluate_context_map(root, config, manifest("GOV-001"))
            self.assertTrue(any("must not exceed 250000" in item for item in failures))
            self.assertTrue(any("commands" in item for item in failures))
            self.assertTrue(any("must be an integer above" in item for item in failures))

    def test_manifest_file_is_json_compatible_yaml(self) -> None:
        payload = manifest("GOV-001")
        rendered = json.dumps(payload, indent=2, sort_keys=True)
        self.assertEqual(json.loads(rendered), payload)


if __name__ == "__main__":
    unittest.main()
