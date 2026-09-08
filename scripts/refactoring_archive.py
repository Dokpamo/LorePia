"""Verify the immutable evidence for a completed refactoring campaign."""
from __future__ import annotations

import json
import re
import subprocess
from pathlib import Path

ARCHIVE_PATH = Path("config/refactoring/archive.json")
ARCHIVED_FILES = (
    "config/ai-context-map.json",
    "config/refactoring/task-manifest.yaml",
    "config/refactoring/completion-status.json",
    "config/refactoring/baseline-report.json",
    "config/refactoring/baseline-summary.json",
)


def archived_bytes(root: Path, commit: str, relative: str) -> bytes:
    result = subprocess.run(
        ["git", "show", f"{commit}:{relative}"], cwd=root,
        check=False, capture_output=True,
    )
    if result.returncode:
        raise ValueError(f"archived refactoring evidence is missing: {relative}")
    return result.stdout


def load_refactoring_archive(root: Path) -> str | None:
    """Return the verified archive commit; projects without an archive stay live."""
    path = root / ARCHIVE_PATH
    if not path.exists():
        return None
    try:
        descriptor = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"invalid refactoring archive: {error}") from error
    if not isinstance(descriptor, dict) or set(descriptor) != {"version", "commit"}:
        raise ValueError("refactoring archive must contain exactly version and commit")
    commit = descriptor["commit"]
    if descriptor["version"] != 1 or not isinstance(commit, str) or re.fullmatch(r"[0-9a-f]{40}", commit) is None:
        raise ValueError("refactoring archive requires version 1 and a full commit hash")
    ancestor = subprocess.run(
        ["git", "merge-base", "--is-ancestor", commit, "HEAD"], cwd=root,
        check=False, capture_output=True,
    )
    if ancestor.returncode:
        raise ValueError("refactoring archive commit must be an ancestor of HEAD")
    for relative in ARCHIVED_FILES:
        try:
            current = (root / relative).read_bytes()
        except OSError as error:
            raise ValueError(f"cannot read archived evidence: {relative}") from error
        if current != archived_bytes(root, commit, relative):
            raise ValueError(f"completed refactoring evidence changed: {relative}")
    try:
        status = json.loads(archived_bytes(root, commit, "config/refactoring/completion-status.json"))
        tasks = status["tasks"]
        gates = status["phase_gates"]
        complete = (
            status["overall_status"] == "complete"
            and len(tasks) == status["expected_task_count"] == 59
            and all(task["status"] == "complete" for task in tasks.values())
            and set(gates) == set("ABCDEF")
            and all(gate["status"] == "complete" for gate in gates.values())
        )
    except (KeyError, TypeError, AttributeError, json.JSONDecodeError) as error:
        raise ValueError("invalid archived completion evidence") from error
    if not complete:
        raise ValueError("only a completed refactoring campaign can be archived")
    return commit
