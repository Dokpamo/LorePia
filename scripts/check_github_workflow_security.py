#!/usr/bin/env python3
"""Fail closed when GitHub workflow trust boundaries drift."""

from __future__ import annotations

import re
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
REMOTE_ACTION = re.compile(
    r"^([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)*)@([0-9a-f]{40})$"
)
USES_LINE = re.compile(r"^\s*-?\s*uses:\s*([^\s#]+)", re.MULTILINE)
CHECKOUT_LINE = re.compile(r"^\s*-\s*uses:\s*actions/checkout@[0-9a-f]{40}.*$", re.MULTILINE)
REQUIRED_WORKFLOW_MARKERS = {
    ".github/workflows/ci.yml": [
        "python3 scripts/check_github_workflow_security.py",
        "scripts/test_check_ai_context_map.py",
        "scripts/test_check_github_workflow_security.py",
        'python3 scripts/check_ai_context_map.py --base-ref "$SOURCE_RATCHET_BASE"',
        "python3 -m unittest scripts/test_generate_ipc_commands.py",
        "python3 scripts/generate_ipc_commands.py --check",
        "python3 scripts/check_source_architecture.py",
        "check_i18n_literal_baseline.py --base-ref",
        "npm audit --omit=dev --audit-level=high",
        "name: iOS simulator",
        "rustup target add aarch64-apple-ios-sim",
        "npm run tauri -- ios init --ci --skip-targets-install",
        "npm run tauri -- ios build --open --ci --debug",
        "--target aarch64-sim --no-sign",
        "--config src-tauri/tauri.release.conf.json",
        "-project src-tauri/gen/apple/lorepia-tauri.xcodeproj",
        "-scheme lorepia-tauri_iOS",
        '-destination "generic/platform=iOS Simulator"',
        "CODE_SIGNING_ALLOWED=NO",
        "CODE_SIGNING_REQUIRED=NO",
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
        "anchore/sbom-action@e22c389904149dbc22b58101806040fa8d37a610",
        "actions/attest@1e69f48acb82d1966a394da916b4c1698aa569d6",
        "python scripts/stage_release_candidate.py",
        "python scripts/write_release_checksums.py target/release/candidate",
        "python scripts/package_linux_release_candidate.py",
        "target/release/upload/lorepia-UNSIGNED-candidate-Linux.tar.gz",
        "python scripts/write_release_checksums.py target/release/upload",
        "target/release/upload/SHA256SUMS",
        "target/release/candidate/SHA256SUMS",
        "target/release/upload/**/*",
        "- name: Package mode-preserving Linux candidate\n        if: matrix.os == 'ubuntu-latest'",
        "- name: Upload mode-preserving Linux candidate\n        if: matrix.os == 'ubuntu-latest'",
        "- name: Upload unsigned candidate\n        if: matrix.os != 'ubuntu-latest'",
        "Release dependency gate",
        "python3 scripts/generate_ipc_commands.py --check",
        "npm run tauri build -- --config src-tauri/tauri.release.conf.json",
        "npm audit --omit=dev --audit-level=high",
        "Reject unsigned official release",
    ],
}
REQUIRED_ACTIVE_CI_LINES = {
    'SOURCE_RATCHET_BASE: ${{ github.event.pull_request.base.sha || github.event.before }}',
    "python3 scripts/check_github_workflow_security.py",
    "scripts/test_check_ai_context_map.py \\",
    "scripts/test_check_github_workflow_security.py \\",
    'python3 scripts/check_ai_context_map.py --base-ref "$SOURCE_RATCHET_BASE"',
}


def evaluate_workflow_security(root: Path) -> list[str]:
    failures: list[str] = []
    workflow_root = root / ".github" / "workflows"
    workflows = sorted(workflow_root.glob("*.yml")) + sorted(
        workflow_root.glob("*.yaml")
    )
    if not workflows:
        failures.append("no GitHub workflows found")

    for path in workflows:
        text = path.read_text(encoding="utf-8")
        relative = path.relative_to(root)
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
            if not any(
                re.fullmatch(r"\s*persist-credentials:\s*false(?:\s*#.*)?", line)
                for line in following
            ):
                failures.append(f"{relative} checkout must disable persisted credentials")

    for relative, markers in REQUIRED_WORKFLOW_MARKERS.items():
        path = root / relative
        if not path.is_file():
            failures.append(f"missing {relative}")
            continue
        text = path.read_text(encoding="utf-8")
        failures.extend(
            f"{relative} must contain: {marker}" for marker in markers if marker not in text
        )

    ci = root / ".github/workflows/ci.yml"
    if ci.is_file():
        ci_text = ci.read_text(encoding="utf-8")
        active_lines = {
            line.strip()
            for line in ci_text.splitlines()
            if line.strip() and not line.lstrip().startswith("#")
        }
        failures.extend(
            f".github/workflows/ci.yml must contain active line: {line}"
            for line in sorted(REQUIRED_ACTIVE_CI_LINES - active_lines)
        )

    release = root / ".github/workflows/release.yml"
    if release.is_file() and "secrets." in release.read_text(encoding="utf-8"):
        failures.append(
            ".github/workflows/release.yml unsigned candidate job must not receive signing secrets"
        )

    if ci.is_file() and "runner.temp" in ci.read_text(encoding="utf-8"):
        failures.append(
            ".github/workflows/ci.yml job environment must not use unavailable runner.temp context"
        )

    return failures


def main() -> int:
    failures = evaluate_workflow_security(REPO_ROOT)
    if failures:
        for failure in failures:
            print(f"workflow security: {failure}", file=sys.stderr)
        return 1
    print("workflow security: PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
