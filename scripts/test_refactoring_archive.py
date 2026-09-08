import contextlib
import io
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
from refactoring_archive import ARCHIVED_FILES, load_refactoring_archive
from report_refactoring_baseline import main as report_main


class RefactoringArchiveTests(unittest.TestCase):
    def archive(self, root: Path, *, complete: bool = True) -> str:
        subprocess.run(["git", "init", "-q"], cwd=root, check=True)
        for relative in ARCHIVED_FILES:
            path = root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("{}\n", encoding="utf-8")
        status = {
            "overall_status": "complete" if complete else "incomplete",
            "expected_task_count": 59,
            "tasks": {str(index): {"status": "complete"} for index in range(59)},
            "phase_gates": {phase: {"status": "complete"} for phase in "ABCDEF"},
        }
        (root / "config/refactoring/completion-status.json").write_text(json.dumps(status), encoding="utf-8")
        (root / "source.rs").write_text("fn initial() {}\n", encoding="utf-8")
        subprocess.run(["git", "add", "."], cwd=root, check=True)
        subprocess.run(["git", "-c", "user.name=Test", "-c", "user.email=test@example.invalid", "commit", "-qm", "completed evidence"], cwd=root, check=True)
        commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
        (root / "config/refactoring/archive.json").write_text(json.dumps({"version": 1, "commit": commit}), encoding="utf-8")
        return commit

    def test_later_source_growth_or_retirement_does_not_change_completed_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            commit = self.archive(root)
            (root / "source.rs").write_text("// unrelated feature\n" * 100000, encoding="utf-8")
            self.assertEqual(load_refactoring_archive(root), commit)
            (root / "source.rs").unlink()
            self.assertEqual(load_refactoring_archive(root), commit)
            arguments = ["report_refactoring_baseline.py", "--root", str(root), "--check", "--output", "config/refactoring/baseline-report.json", "--summary-output", "config/refactoring/baseline-summary.json"]
            with patch.object(sys, "argv", arguments), contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(report_main(), 0)

    def test_changed_report_or_completion_evidence_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.archive(root)
            for relative in ARCHIVED_FILES:
                path = root / relative
                previous = path.read_bytes()
                path.write_bytes(previous + b" ")
                with self.assertRaisesRegex(ValueError, "evidence changed"):
                    load_refactoring_archive(root)
                path.write_bytes(previous)

    def test_incomplete_campaign_and_unrelated_commit_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.archive(root, complete=False)
            with self.assertRaisesRegex(ValueError, "only a completed"):
                load_refactoring_archive(root)
            descriptor = root / "config/refactoring/archive.json"
            descriptor.write_text(json.dumps({"version": 1, "commit": "0" * 40}), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "ancestor"):
                load_refactoring_archive(root)


if __name__ == "__main__":
    unittest.main()
