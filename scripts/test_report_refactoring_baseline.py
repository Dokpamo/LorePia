import hashlib
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT_DIRECTORY = Path(__file__).resolve().parent
if str(SCRIPT_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIRECTORY))

from report_refactoring_baseline import (
    build_report,
    compact_summary,
    completion_evidence,
    ipc_command_names,
    remaining_hotspots,
    serialized_report,
    source_files,
)


class RefactoringBaselineReportTests(unittest.TestCase):
    def git(self, root: Path, *arguments: str) -> str:
        return subprocess.run(
            ["git", *arguments],
            cwd=root,
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()

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
        p0_paths = (
            "apps/lorepia/src/app/app-controller.ts",
            "apps/lorepia/src/features/chat/ChatPane.svelte",
            "apps/lorepia/src/features/orchestration/OrchestrationStudio.svelte",
            "crates/core/src/app.rs",
            "crates/core/src/orchestration_runtime.rs",
            "crates/core/src/provider_discovery.rs",
            "crates/storage/src/database.rs",
            "crates/storage/src/discovery_repository.rs",
            "crates/storage/src/interaction_repository.rs",
            "crates/storage/src/orchestration.rs",
        )
        for relative in p0_paths:
            path = root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("", encoding="utf-8")
        (root / "crates/sample/src/baseline.rs").write_text("x\n", encoding="utf-8")
        (root / "crates/sample/src/legacy.rs").write_text(
            "x" * 1_500, encoding="utf-8"
        )
        languages = ("css", "kotlin", "lua", "rust", "svelte", "swift", "typescript")
        source_limits = {
            kind: {
                language: {"bytes": 1_024, "lines": 100}
                for language in languages
            }
            for kind in ("facade", "generated", "production")
        }
        (root / "config/source-size-baseline.json").write_text(
            json.dumps(
                {
                    "baselines": {
                        "crates/sample/src/baseline.rs": {
                            "bytes": 2_000,
                            "lines": 100,
                        }
                    },
                    "bootstrap_ref": "0" * 40,
                    "facade_paths": [],
                    "limits": source_limits,
                    "parent_child_groups": {},
                    "version": 2,
                }
            ),
            encoding="utf-8",
        )
        (root / "config/test-source-size-baseline.json").write_text(
            json.dumps(
                {
                    "baselines": {},
                    "bootstrap_ref": "0" * 40,
                    "limits": {
                        "test": {
                            language: {"bytes": 1_024, "lines": 100}
                            for language in languages
                        }
                    },
                    "version": 2,
                }
            ),
            encoding="utf-8",
        )
        (root / "config/core-storage-public-api-baseline.json").write_text(
            '{"version": 1, "allowed_stored_reexports": []}', encoding="utf-8"
        )
        (root / "config/ipc-commands.json").write_text(
            '{"commands": ["alpha", "beta"]}', encoding="utf-8"
        )
        task_ids = [f"TASK-{index:03}" for index in range(59)]
        phases = ("A", "B", "C", "D", "E", "F")
        (root / "config/refactoring").mkdir()
        (root / "config/refactoring/task-manifest.yaml").write_text(
            json.dumps(
                {
                    "decisions": {"task_count": 59},
                    "tasks": {
                        task_id: {
                            "depends_on": [],
                            "phase": phases[index % len(phases)],
                        }
                        for index, task_id in enumerate(task_ids)
                    },
                }
            ),
            encoding="utf-8",
        )
        incomplete_gate = {
            "github_checks": {
                "commit": None,
                "run_url": None,
                "status": "not_run",
            },
            "local_validation": {"commit": None, "status": "pending"},
            "status": "incomplete",
        }
        (root / "config/refactoring/completion-status.json").write_text(
            json.dumps(
                {
                    "approved_hotspot_exceptions": [],
                    "expected_task_count": 59,
                    "overall_status": "incomplete",
                    "phase_gates": {
                        phase: dict(incomplete_gate) for phase in phases
                    },
                    "policy": "fixture completion policy",
                    "tasks": {
                        task_id: {"evidence_commits": [], "status": "complete"}
                        for task_id in task_ids
                    },
                    "version": 2,
                }
            ),
            encoding="utf-8",
        )
        (root / "config/ai-context-map.json").write_text(
            '{"version": 1}', encoding="utf-8"
        )
        self.git(root, "init", "-q")
        self.git(root, "config", "user.name", "LorePia Test")
        self.git(root, "config", "user.email", "test@example.invalid")
        self.git(root, "add", ".")
        self.git(root, "-c", "commit.gpgsign=false", "commit", "-q", "-m", "fixture bootstrap")
        bootstrap = self.git(root, "rev-parse", "HEAD")
        for relative in (
            "config/source-size-baseline.json",
            "config/test-source-size-baseline.json",
        ):
            path = root / relative
            config = json.loads(path.read_text(encoding="utf-8"))
            config["bootstrap_ref"] = bootstrap
            path.write_text(json.dumps(config), encoding="utf-8")
        completion_path = root / "config/refactoring/completion-status.json"
        self.git(root, "add", ".")
        self.git(root, "-c", "commit.gpgsign=false", "commit", "-q", "-m", "fixture ledgers")
        evidence_commits: dict[str, str] = {}
        for task_id in task_ids:
            self.git(
                root,
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                f"evidence {task_id}",
            )
            evidence_commits[task_id] = self.git(root, "rev-parse", "HEAD")
        completion = json.loads(completion_path.read_text(encoding="utf-8"))
        for task_id, record in completion["tasks"].items():
            record["evidence_commits"] = [evidence_commits[task_id]]
        completion_path.write_text(json.dumps(completion), encoding="utf-8")
        self.git(root, "add", str(completion_path.relative_to(root)))
        self.git(
            root,
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "record task evidence",
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
            self.assertEqual(first["summary"]["production_files"], len(source_files(root)))
            self.assertEqual(first["summary"]["public_symbols"], 4)
            self.assertEqual(first["completion"]["task_count"], 59)
            self.assertEqual(
                first["remaining_hotspots"]["hard_target_overages"]["count"], 1
            )
            self.assertEqual(
                first["remaining_hotspots"]["configured_ratchets"]["count"], 1
            )
            self.assertEqual(first["remaining_hotspots"]["plan_targets"]["met_count"], 10)
            self.assertEqual(compact_summary(first), compact_summary(second))
            self.assertEqual(list(first["files"]), sorted(first["files"]))
            self.assertNotIn(str(root), serialized_report(first))

    def test_completion_status_must_cover_every_manifest_task(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = self.make_root(temporary)
            path = root / "config/refactoring/completion-status.json"
            status = json.loads(path.read_text(encoding="utf-8"))
            status["tasks"].pop("TASK-000")
            path.write_text(json.dumps(status), encoding="utf-8")

            with self.assertRaisesRegex(ValueError, "exactly match"):
                completion_evidence(root)

    def test_remaining_hotspot_records_ratchet_without_claiming_approval(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = self.make_root(temporary)
            hotspot = root / "crates/sample/src/baseline.rs"
            hotspot.write_text("x" * 2_000, encoding="utf-8")

            report = build_report(root, "a" * 40)
            entry = next(
                item
                for item in report["remaining_hotspots"]["hard_target_overages"][
                    "entries"
                ]
                if item["path"] == "crates/sample/src/baseline.rs"
            )
            self.assertEqual(entry["ratchet"]["kind"], "configured-baseline")
            self.assertIsNone(entry["approved_exception"])
            self.assertEqual(entry["disposition"], "unresolved")

    def test_under_target_configured_ratchet_remains_visible(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = self.make_root(temporary)

            evidence = remaining_hotspots(root)
            entry = next(
                item
                for item in evidence["configured_ratchets"]["entries"]
                if item["path"] == "crates/sample/src/baseline.rs"
            )
            self.assertEqual(entry["status"], "eligible-for-removal")

    def test_untracked_oversized_source_cannot_claim_bootstrap_history(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = self.make_root(temporary)
            (root / "crates/sample/src/new_hotspot.rs").write_text(
                "x" * 2_000, encoding="utf-8"
            )

            with self.assertRaisesRegex(ValueError, "absent at bootstrap"):
                remaining_hotspots(root)

    def test_complete_task_requires_commit_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = self.make_root(temporary)
            path = root / "config/refactoring/completion-status.json"
            status = json.loads(path.read_text(encoding="utf-8"))
            status["tasks"]["TASK-000"]["evidence_commits"] = []
            path.write_text(json.dumps(status), encoding="utf-8")

            with self.assertRaisesRegex(ValueError, "no commit evidence"):
                completion_evidence(root)

    def test_task_evidence_commit_cannot_be_reused(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = self.make_root(temporary)
            path = root / "config/refactoring/completion-status.json"
            status = json.loads(path.read_text(encoding="utf-8"))
            status["tasks"]["TASK-001"]["evidence_commits"] = status["tasks"][
                "TASK-000"
            ]["evidence_commits"]
            path.write_text(json.dumps(status), encoding="utf-8")

            with self.assertRaisesRegex(ValueError, "reused"):
                completion_evidence(root)

    def test_phase_validation_must_follow_every_task_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = self.make_root(temporary)
            path = root / "config/refactoring/completion-status.json"
            status = json.loads(path.read_text(encoding="utf-8"))
            source_config = json.loads(
                (root / "config/source-size-baseline.json").read_text(encoding="utf-8")
            )
            status["phase_gates"]["A"]["local_validation"] = {
                "commit": source_config["bootstrap_ref"],
                "status": "pass",
            }
            path.write_text(json.dumps(status), encoding="utf-8")

            with self.assertRaisesRegex(ValueError, "predates task evidence"):
                completion_evidence(root)

    def test_unverified_github_pass_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = self.make_root(temporary)
            path = root / "config/refactoring/completion-status.json"
            status = json.loads(path.read_text(encoding="utf-8"))
            head = self.git(root, "rev-parse", "HEAD")
            status["phase_gates"]["A"] = {
                "github_checks": {
                    "commit": head,
                    "run_url": "https://github.com/Dokpamo/LorePia/actions/runs/1",
                    "status": "pass",
                },
                "local_validation": {"commit": head, "status": "pass"},
                "status": "complete",
            }
            path.write_text(json.dumps(status), encoding="utf-8")

            with self.assertRaisesRegex(ValueError, "unverifiable GitHub pass"):
                completion_evidence(root)

    def test_unstructured_hotspot_exception_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = self.make_root(temporary)
            path = root / "config/refactoring/completion-status.json"
            status = json.loads(path.read_text(encoding="utf-8"))
            status["approved_hotspot_exceptions"] = [
                {
                    "authority": "config/refactoring/task-manifest.yaml#fake",
                    "path": "crates/storage/src/database.rs",
                }
            ]
            path.write_text(json.dumps(status), encoding="utf-8")

            with self.assertRaisesRegex(ValueError, "permits no hotspot exception"):
                remaining_hotspots(root)

    def test_compact_summary_hashes_exact_report_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = self.make_root(temporary)
            report = build_report(root, "a" * 40)

            self.assertEqual(
                compact_summary(report)["full_report_sha256"],
                hashlib.sha256(serialized_report(report).encode("utf-8")).hexdigest(),
            )

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
