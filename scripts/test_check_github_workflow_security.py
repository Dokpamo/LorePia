import shutil
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT_DIRECTORY = Path(__file__).resolve().parent
if str(SCRIPT_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIRECTORY))

from check_github_workflow_security import (
    REPO_ROOT,
    evaluate_workflow_security,
)


class GithubWorkflowSecurityTests(unittest.TestCase):
    def copied_workflows(self, temporary: str) -> Path:
        root = Path(temporary).resolve()
        shutil.copytree(
            REPO_ROOT / ".github/workflows", root / ".github/workflows"
        )
        return root

    def test_current_workflows_pass(self) -> None:
        self.assertEqual(evaluate_workflow_security(REPO_ROOT), [])

    def test_ai_context_base_ref_and_regression_tests_are_required(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = self.copied_workflows(temporary)
            ci = root / ".github/workflows/ci.yml"
            text = ci.read_text(encoding="utf-8")
            text = text.replace(
                'python3 scripts/check_ai_context_map.py --base-ref "$SOURCE_RATCHET_BASE"',
                '# python3 scripts/check_ai_context_map.py --base-ref "$SOURCE_RATCHET_BASE"',
            )
            text = text.replace(
                "scripts/test_check_ai_context_map.py \\\n",
                "# scripts/test_check_ai_context_map.py \\\n",
            )
            text = text.replace(
                "scripts/test_check_github_workflow_security.py \\\n",
                "# scripts/test_check_github_workflow_security.py \\\n",
            )
            text = text.replace(
                "SOURCE_RATCHET_BASE: ${{ github.event.pull_request.base.sha || github.event.before }}",
                "# SOURCE_RATCHET_BASE: ${{ github.event.pull_request.base.sha || github.event.before }}",
            )
            ci.write_text(text, encoding="utf-8")

            failures = evaluate_workflow_security(root)
            self.assertTrue(any("--base-ref" in item for item in failures))
            self.assertTrue(any("test_check_ai_context_map.py" in item for item in failures))
            self.assertTrue(
                any("test_check_github_workflow_security.py" in item for item in failures)
            )
            self.assertTrue(any("SOURCE_RATCHET_BASE" in item for item in failures))

    def test_unpinned_action_and_persisted_checkout_credentials_fail(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = self.copied_workflows(temporary)
            ci = root / ".github/workflows/ci.yml"
            text = ci.read_text(encoding="utf-8")
            text = text.replace(
                "actions/setup-java@b6effb05e454b25005698d916606bdc6ffcbf961",
                "actions/setup-java@v5",
                1,
            )
            text = text.replace(
                "          persist-credentials: false\n",
                "          # persist-credentials: false\n",
                1,
            )
            ci.write_text(text, encoding="utf-8")

            failures = evaluate_workflow_security(root)
            self.assertTrue(any("unpinned remote action" in item for item in failures))
            self.assertTrue(any("persisted credentials" in item for item in failures))


if __name__ == "__main__":
    unittest.main()
