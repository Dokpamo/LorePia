import contextlib
import copy
import io
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock


SCRIPT_DIRECTORY = Path(__file__).resolve().parent
if str(SCRIPT_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIRECTORY))

from check_ai_context_map import (
    AI_CONTEXT_BOOTSTRAP_REF,
    CONTEXT_MAP_POLICY,
    ContextMeasurement,
    evaluate_base_ref_drift,
    evaluate_bundle_size_drift,
    evaluate_context_map,
    main,
    manifest_task_ids,
    validate_repository_manifest,
)


def manifest(*task_ids: str) -> dict[str, object]:
    return {
        "version": 1,
        "tasks": {task_id: {"depends_on": []} for task_id in task_ids},
    }


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


def repository_manifest() -> dict[str, object]:
    task_ids = ["GOV-001", *(f"TASK-{index:03}" for index in range(57))]
    tasks = {task_id: {"depends_on": []} for task_id in task_ids}
    tasks["ENF-004"] = {"depends_on": task_ids}
    return {
        "version": 1,
        "decisions": {"task_count": 59},
        "tasks": tasks,
    }


def expand_repository_contexts(config: dict[str, object]) -> None:
    contexts = config["contexts"]
    assert isinstance(contexts, dict)
    template = contexts["GOV-001"]
    manifest_payload = repository_manifest()
    tasks = manifest_payload["tasks"]
    assert isinstance(tasks, dict)
    config["contexts"] = {
        task_id: copy.deepcopy(template) for task_id in tasks
    }


def run_checker_cli(*arguments: str) -> tuple[int, str, str]:
    stdout = io.StringIO()
    stderr = io.StringIO()
    argv = ["check_ai_context_map.py", *arguments]
    with mock.patch.object(sys, "argv", argv), contextlib.redirect_stdout(
        stdout
    ), contextlib.redirect_stderr(stderr):
        result = main()
    return result, stdout.getvalue(), stderr.getvalue()


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
            "bootstrap_ref": AI_CONTEXT_BOOTSTRAP_REF,
            "version": 1,
            "default_max_context_bytes": 250_000,
            "policy": CONTEXT_MAP_POLICY,
            "reviewed_path_migrations": [],
            "shared_paths": ["docs/guide.md"],
            "contexts": {"GOV-001": entry("src/feature.rs")},
        }

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

    def test_symlink_alias_is_a_duplicate_bundle_path(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = self.make_root(temporary)
            (root / "src/feature-alias.rs").symlink_to("feature.rs")
            config = self.base_config()
            contexts = config["contexts"]
            assert isinstance(contexts, dict)
            aliased = entry("src/feature.rs")
            paths = aliased["paths"]
            assert isinstance(paths, dict)
            paths["tests"] = ["src/feature-alias.rs"]
            contexts["GOV-001"] = aliased
            failures, _ = evaluate_context_map(root, config, manifest("GOV-001"))
            self.assertTrue(any("aliases src/feature.rs" in item for item in failures))

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
            base_measurement = ContextMeasurement("GOV-001", 300, 2, 100, 400)
            current_measurement = ContextMeasurement("GOV-001", 301, 2, 100, 400)
            self.assertTrue(
                evaluate_bundle_size_drift(
                    [current_measurement], {"GOV-001": base_measurement}
                )
            )

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

    def test_manifest_dependencies_must_be_known_unique_and_acyclic(self) -> None:
        fixtures = {
            "unknown": {
                "version": 1,
                "tasks": {"A": {"depends_on": ["MISSING"]}},
            },
            "duplicate": {
                "version": 1,
                "tasks": {
                    "A": {"depends_on": []},
                    "B": {"depends_on": ["A", "A"]},
                },
            },
            "cycle": {
                "version": 1,
                "tasks": {
                    "A": {"depends_on": ["B"]},
                    "B": {"depends_on": ["A"]},
                },
            },
        }
        for label, payload in fixtures.items():
            with self.subTest(label=label), self.assertRaises(ValueError):
                manifest_task_ids(payload)

    def test_completion_task_must_reach_every_manifest_task(self) -> None:
        payload = {
            "version": 1,
            "tasks": {
                "A": {"depends_on": []},
                "B": {"depends_on": []},
                "ENF-004": {"depends_on": ["A"]},
            },
        }
        with self.assertRaisesRegex(ValueError, "does not reach: B"):
            manifest_task_ids(payload)
        payload["tasks"]["ENF-004"]["depends_on"].append("B")
        self.assertEqual(manifest_task_ids(payload), {"A", "B", "ENF-004"})

    def test_repository_manifest_declares_exactly_fifty_nine_tasks(self) -> None:
        with self.assertRaisesRegex(ValueError, "decisions object"):
            validate_repository_manifest(manifest("GOV-001"))
        tasks = {
            f"TASK-{index:03}": {"depends_on": []} for index in range(58)
        }
        tasks["ENF-004"] = {"depends_on": sorted(tasks)}
        payload = {
            "version": 1,
            "decisions": {"task_count": 58},
            "tasks": tasks,
        }
        with self.assertRaisesRegex(ValueError, "must be 59"):
            manifest_task_ids(payload)
        payload["decisions"]["task_count"] = 59
        self.assertEqual(len(manifest_task_ids(payload)), 59)

    def test_base_ref_rejects_entry_loss_and_ratchet_growth(self) -> None:
        base_config = self.base_config()
        base_config["default_max_context_bytes"] = 100
        base_contexts = base_config["contexts"]
        assert isinstance(base_contexts, dict)
        base_contexts["GOV-001"] = entry("src/feature.rs", limit=80)
        base_manifest = manifest("GOV-001", "GOV-002")

        current_config = copy.deepcopy(base_config)
        current_config["default_max_context_bytes"] = 101
        current_contexts = current_config["contexts"]
        assert isinstance(current_contexts, dict)
        current_entry = current_contexts["GOV-001"]
        assert isinstance(current_entry, dict)
        current_entry["max_context_bytes"] = 81
        current_entry["baseline_context_bytes"] = 200
        current_entry["over_budget_reason"] = "new exception"
        current_manifest = manifest("GOV-001")

        failures = evaluate_base_ref_drift(
            current_config, current_manifest, base_config, base_manifest
        )
        self.assertTrue(any("manifest task entry removed" in item for item in failures))
        self.assertTrue(any("default_max_context_bytes increased" in item for item in failures))
        self.assertTrue(any("max_context_bytes increased" in item for item in failures))
        self.assertTrue(any("new legacy context baseline" in item for item in failures))

        del current_contexts["GOV-001"]
        failures = evaluate_base_ref_drift(
            current_config, current_manifest, base_config, base_manifest
        )
        self.assertTrue(any("context entry removed" in item for item in failures))

    def test_base_ref_allows_ratchets_to_shrink_or_disappear(self) -> None:
        base_config = self.base_config()
        base_config["default_max_context_bytes"] = 100
        base_contexts = base_config["contexts"]
        assert isinstance(base_contexts, dict)
        base_entry = entry("src/feature.rs", limit=80)
        base_entry["baseline_context_bytes"] = 200
        base_entry["over_budget_reason"] = "legacy"
        base_contexts["GOV-001"] = base_entry

        current_config = copy.deepcopy(base_config)
        current_config["default_max_context_bytes"] = 90
        current_contexts = current_config["contexts"]
        assert isinstance(current_contexts, dict)
        current_entry = current_contexts["GOV-001"]
        assert isinstance(current_entry, dict)
        current_entry["max_context_bytes"] = 70
        current_entry["baseline_context_bytes"] = None
        current_entry["over_budget_reason"] = None
        self.assertEqual(
            evaluate_base_ref_drift(
                current_config,
                manifest("GOV-001"),
                base_config,
                manifest("GOV-001"),
            ),
            [],
        )

    def test_base_ref_preserves_path_and_targeted_command_entry_counts(self) -> None:
        base_config = self.base_config()
        del base_config["bootstrap_ref"]
        current_config = copy.deepcopy(base_config)
        contexts = current_config["contexts"]
        assert isinstance(contexts, dict)
        current_entry = contexts["GOV-001"]
        assert isinstance(current_entry, dict)
        paths = current_entry["paths"]
        assert isinstance(paths, dict)
        paths["documents"] = []
        current_entry["commands"] = []
        failures = evaluate_base_ref_drift(
            current_config,
            manifest("GOV-001"),
            base_config,
            manifest("GOV-001"),
        )
        self.assertTrue(any("path entries decreased" in item for item in failures))
        self.assertTrue(any("command entries decreased" in item for item in failures))

        paths["documents"] = ["src/replacement.rs"]
        current_entry["commands"] = ["python3 replacement_test.py"]
        self.assertEqual(
            evaluate_base_ref_drift(
                current_config,
                manifest("GOV-001"),
                base_config,
                manifest("GOV-001"),
            ),
            [],
        )

        enforced_base = self.base_config()
        failures = evaluate_base_ref_drift(
            current_config,
            manifest("GOV-001"),
            enforced_base,
            manifest("GOV-001"),
        )
        self.assertTrue(any("removed without a reviewed replacement" in item for item in failures))

        current_config["bootstrap_ref"] = AI_CONTEXT_BOOTSTRAP_REF
        current_config["reviewed_path_migrations"] = [
            {
                "from": "src/feature.rs",
                "reason": "fixture path moved",
                "task_id": "GOV-001",
                "to": "src/replacement.rs",
            }
        ]
        failures = evaluate_base_ref_drift(
            current_config,
            manifest("GOV-001"),
            enforced_base,
            manifest("GOV-001"),
        )
        self.assertFalse(any("removed without" in item for item in failures))

        shared_removed = copy.deepcopy(enforced_base)
        shared_removed["shared_paths"] = []
        failures = evaluate_base_ref_drift(
            shared_removed,
            manifest("GOV-001"),
            enforced_base,
            manifest("GOV-001"),
        )
        self.assertTrue(any("shared path entry removed" in item for item in failures))

    def test_commands_must_be_trimmed_single_lines(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = self.make_root(temporary)
            config = self.base_config()
            contexts = config["contexts"]
            assert isinstance(contexts, dict)
            context = contexts["GOV-001"]
            assert isinstance(context, dict)
            context["commands"] = ["python3 good.py\npython3 hidden.py"]
            failures, _ = evaluate_context_map(root, config, manifest("GOV-001"))
            self.assertTrue(any("single-line" in item for item in failures))

    def test_cli_base_ref_reads_committed_map_and_rejects_growth(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = self.make_root(temporary)
            config_dir = root / "config"
            (config_dir / "refactoring").mkdir(parents=True)
            config = self.base_config()
            config["default_max_context_bytes"] = 100
            contexts = config["contexts"]
            assert isinstance(contexts, dict)
            contexts["GOV-001"] = entry("src/feature.rs", limit=80)
            expand_repository_contexts(config)
            manifest_payload = repository_manifest()
            config_path = config_dir / "ai-context-map.json"
            manifest_path = config_dir / "refactoring/task-manifest.yaml"
            config_path.write_text(json.dumps(config), encoding="utf-8")
            manifest_path.write_text(json.dumps(manifest_payload), encoding="utf-8")
            subprocess.run(["git", "init", "-q"], cwd=root, check=True)
            subprocess.run(
                ["git", "config", "user.email", "context-test@example.invalid"],
                cwd=root,
                check=True,
            )
            subprocess.run(
                ["git", "config", "user.name", "Context Test"], cwd=root, check=True
            )
            subprocess.run(["git", "add", "."], cwd=root, check=True)
            subprocess.run(
                ["git", "commit", "-qm", "baseline"], cwd=root, check=True
            )

            config["default_max_context_bytes"] = 101
            config_path.write_text(json.dumps(config), encoding="utf-8")
            arguments = [
                "--root",
                str(root),
                "--config",
                "config/ai-context-map.json",
                "--manifest",
                "config/refactoring/task-manifest.yaml",
                "--base-ref",
                "HEAD",
            ]
            result, _, stderr = run_checker_cli(*arguments)
            self.assertEqual(result, 1)
            self.assertIn("default_max_context_bytes increased", stderr)

            arguments[-1] = "missing-ref"
            result, _, stderr = run_checker_cli(*arguments)
            self.assertEqual(result, 1)
            self.assertIn("base ref is not a commit", stderr)

            empty_tree = subprocess.check_output(
                ["git", "mktree"], cwd=root, input="", text=True
            ).strip()
            unrelated = subprocess.check_output(
                ["git", "commit-tree", empty_tree, "-m", "unrelated"],
                cwd=root,
                text=True,
            ).strip()
            arguments[-1] = unrelated
            result, _, stderr = run_checker_cli(*arguments)
            self.assertEqual(result, 1)
            self.assertIn("base ref must be an ancestor", stderr)

    def test_print_commands_fails_when_the_full_map_is_invalid(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = self.make_root(temporary)
            config_path = root / "context.json"
            manifest_path = root / "manifest.json"
            config = self.base_config()
            expand_repository_contexts(config)
            contexts = config["contexts"]
            assert isinstance(contexts, dict)
            del contexts["TASK-056"]
            config_path.write_text(json.dumps(config), encoding="utf-8")
            manifest_path.write_text(json.dumps(repository_manifest()), encoding="utf-8")
            arguments = [
                "--root",
                str(root),
                "--config",
                str(config_path),
                "--manifest",
                str(manifest_path),
                "--print-commands",
                "GOV-001",
            ]
            result, stdout, _ = run_checker_cli(*arguments)
            self.assertEqual(result, 1)
            self.assertEqual(stdout, "")

if __name__ == "__main__":
    unittest.main()
