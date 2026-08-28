#!/usr/bin/env python3
"""Fail closed when GitHub workflow trust boundaries drift."""

from __future__ import annotations

import re
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
WORKFLOW_ROOT = REPO_ROOT / ".github" / "workflows"
REMOTE_ACTION = re.compile(
    r"^([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)*)@([0-9a-f]{40})$"
)
USES_LINE = re.compile(r"^\s*-?\s*uses:\s*([^\s#]+)", re.MULTILINE)
CHECKOUT_LINE = re.compile(r"^\s*-\s*uses:\s*actions/checkout@[0-9a-f]{40}.*$", re.MULTILINE)


def main() -> int:
    failures: list[str] = []
    workflows = sorted(WORKFLOW_ROOT.glob("*.yml")) + sorted(WORKFLOW_ROOT.glob("*.yaml"))
    if not workflows:
        failures.append("no GitHub workflows found")

    for path in workflows:
        text = path.read_text(encoding="utf-8")
        relative = path.relative_to(REPO_ROOT)
        if "permissions:" not in text or "contents: read" not in text:
            failures.append(f"{relative} must declare least-privilege contents: read")
        for match in USES_LINE.finditer(text):
            action = match.group(1)
            if action.startswith("./"):
                continue
            if REMOTE_ACTION.fullmatch(action) is None:
                failures.append(f"{relative} has an unpinned remote action: {action}")
        for match in CHECKOUT_LINE.finditer(text):
            following = text[match.end() :].splitlines()[:4]
            if not any("persist-credentials: false" in line for line in following):
                failures.append(f"{relative} checkout must disable persisted credentials")

    required = {
        ".github/workflows/ci.yml": [
            "python3 scripts/check_github_workflow_security.py",
            "python3 scripts/check_source_architecture.py",
            "npm audit --omit=dev --audit-level=high",
        ],
        ".github/workflows/security.yml": [
            "actions/dependency-review-action@a1d282b36b6f3519aa1f3fc636f609c47dddb294",
            "github/codeql-action/init@6f5948dfacef28e207b48d0905cf90c03365536d",
            "github/codeql-action/analyze@6f5948dfacef28e207b48d0905cf90c03365536d",
            "rustsec/audit-check@69366f33c96575abad1ee0dba8212993eecbe998",
        ],
        ".github/workflows/release.yml": [
            "if: github.event_name == 'workflow_dispatch'",
            "lorepia-UNSIGNED-candidate-",
            "python scripts/write_release_checksums.py target/release/bundle",
            "Reject unsigned official release",
        ],
    }
    for relative, markers in required.items():
        path = REPO_ROOT / relative
        if not path.is_file():
            failures.append(f"missing {relative}")
            continue
        text = path.read_text(encoding="utf-8")
        failures.extend(
            f"{relative} must contain: {marker}" for marker in markers if marker not in text
        )

    release = REPO_ROOT / ".github/workflows/release.yml"
    if release.is_file() and "secrets." in release.read_text(encoding="utf-8"):
        failures.append(
            ".github/workflows/release.yml unsigned candidate job must not receive signing secrets"
        )

    if failures:
        for failure in failures:
            print(f"workflow security: {failure}", file=sys.stderr)
        return 1
    print("workflow security: PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
