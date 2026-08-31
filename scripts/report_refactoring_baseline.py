#!/usr/bin/env python3
"""Create a deterministic refactoring hotspot and public-surface snapshot."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path
from typing import Any

from check_source_architecture import (
    classify_source,
    generated_sources,
    production_sources,
    source_language,
    source_size_at_ref,
    test_sources,
)


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUTPUT = REPO_ROOT / "config" / "refactoring" / "baseline-report.json"
DEFAULT_SUMMARY_OUTPUT = (
    REPO_ROOT / "config" / "refactoring" / "baseline-summary.json"
)
SOURCE_SIZE_CONFIG = Path("config/source-size-baseline.json")
TEST_SOURCE_SIZE_CONFIG = Path("config/test-source-size-baseline.json")
CORE_STORAGE_API_CONFIG = Path("config/core-storage-public-api-baseline.json")
IPC_CONFIG = Path("config/ipc-commands.json")
AI_CONTEXT_CONFIG = Path("config/ai-context-map.json")
TASK_MANIFEST_CONFIG = Path("config/refactoring/task-manifest.yaml")
COMPLETION_STATUS_CONFIG = Path("config/refactoring/completion-status.json")
EXPECTED_TASK_COUNT = 59
EXPECTED_PHASES = ("A", "B", "C", "D", "E", "F")
TASK_STATUSES = {"blocked", "complete", "in_progress"}
LOCAL_VALIDATION_STATES = {"fail", "pass", "pending"}
GITHUB_CHECK_STATES = {"fail", "not_run", "pass"}
FULL_COMMIT_RE = re.compile(r"[0-9a-f]{40}")
GITHUB_RUN_RE = re.compile(
    r"https://github\.com/Dokpamo/LorePia/actions/runs/[0-9]+"
)
P0_TARGETS = {
    "apps/lorepia/src/app/app-controller.ts": {"bytes": 40_960, "lines": 900},
    "apps/lorepia/src/features/chat/ChatPane.svelte": {"bytes": 30_720, "lines": 600},
    "apps/lorepia/src/features/orchestration/OrchestrationStudio.svelte": {
        "bytes": 30_720,
        "lines": 600,
    },
    "crates/core/src/app.rs": {"bytes": 40_960, "lines": 1_000},
    "crates/core/src/orchestration_runtime.rs": {"bytes": 40_960, "lines": 900},
    "crates/core/src/provider_discovery.rs": {"bytes": 35_840, "lines": 800},
    "crates/storage/src/database.rs": {"bytes": 40_960, "lines": 900},
    "crates/storage/src/discovery_repository.rs": {"bytes": 30_720, "lines": 700},
    "crates/storage/src/interaction_repository.rs": {"bytes": 40_960, "lines": 900},
    "crates/storage/src/orchestration.rs": {"bytes": 35_840, "lines": 800},
}
RUST_PUBLIC_RE = re.compile(
    r"^\s*pub(?:\([^)]*\))?\s+(?:async\s+)?(?:const|enum|fn|mod|static|struct|trait|type|use)\b"
)
FRONTEND_PUBLIC_RE = re.compile(
    r"^\s*export\s+(?:default\s+)?(?:abstract\s+)?(?:async\s+)?"
    r"(?:class|const|enum|function|interface|let|type|var)\b"
)
NATIVE_PUBLIC_RE = re.compile(
    r"^\s*(?:(?:public|open)\s+|public\s+(?:final\s+)?)"
    r"(?:class|enum|fun|interface|object|protocol|struct|typealias|val|var)\b"
)
DECLARATION_RE = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+|export\s+(?:default\s+)?)?"
    r"(?:async\s+)?(?:class|enum|fn|function|interface|struct|trait|type)\b",
    re.MULTILINE,
)


def sha256_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def resolve_commit(root: Path, revision: str) -> str:
    result = subprocess.run(
        ["git", "rev-parse", "--verify", f"{revision}^{{commit}}"],
        cwd=root,
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise ValueError(f"baseline ref is not a commit: {revision}")
    return result.stdout.strip()


def source_files(root: Path) -> list[Path]:
    """Use the exact production-source definition enforced by the v1 ratchet."""

    return production_sources(root)


def normalized_public_lines(path: Path, text: str) -> list[str]:
    if path.suffix == ".rs":
        matcher = RUST_PUBLIC_RE
    elif path.suffix in {".ts", ".svelte"}:
        matcher = FRONTEND_PUBLIC_RE
    elif path.suffix in {".kt", ".swift"}:
        matcher = NATIVE_PUBLIC_RE
    else:
        return []
    return [" ".join(line.split()) for line in text.splitlines() if matcher.match(line)]


def load_json(root: Path, relative: Path) -> Any:
    path = root / relative
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"cannot read {relative.as_posix()}: {error}") from error


def load_json_at_commit(root: Path, relative: Path, commit: str) -> Any:
    process = subprocess.run(
        ["git", "show", f"{commit}:{relative.as_posix()}"],
        cwd=root,
        check=False,
        capture_output=True,
        text=True,
    )
    if process.returncode != 0:
        raise ValueError(f"cannot read {relative.as_posix()} at {commit}")
    try:
        return json.loads(process.stdout)
    except json.JSONDecodeError as error:
        raise ValueError(
            f"invalid {relative.as_posix()} at {commit}: {error}"
        ) from error


def json_digest(value: Any) -> str:
    encoded = json.dumps(
        value, ensure_ascii=False, sort_keys=True, separators=(",", ":")
    ).encode("utf-8")
    return sha256_bytes(encoded)


def ipc_command_names(config: Any) -> list[str]:
    if not isinstance(config, dict):
        raise ValueError("IPC command config must be an object")
    raw_commands = config.get("commands")
    if not isinstance(raw_commands, list):
        raise ValueError("IPC command config must contain a commands array")
    names: list[str] = []
    for command in raw_commands:
        if isinstance(command, str):
            names.append(command)
        elif isinstance(command, dict) and isinstance(command.get("name"), str):
            names.append(command["name"])
        else:
            raise ValueError("IPC command entries must be strings or named objects")
    if len(names) != len(set(names)):
        raise ValueError("IPC command names must be unique")
    return sorted(names)


def rust_u32_constant(root: Path, relative: Path, name: str) -> int:
    try:
        text = (root / relative).read_text(encoding="utf-8")
    except OSError as error:
        raise ValueError(f"cannot read {relative.as_posix()}: {error}") from error
    match = re.search(rf"\bpub\s+const\s+{re.escape(name)}\s*:\s*u32\s*=\s*(\d+)\s*;", text)
    if match is None:
        raise ValueError(f"cannot find {name} in {relative.as_posix()}")
    return int(match.group(1))


def require_ancestor_commit(root: Path, value: Any, *, label: str) -> str:
    if not isinstance(value, str) or FULL_COMMIT_RE.fullmatch(value) is None:
        raise ValueError(f"{label} must be a full lowercase commit hash")
    if resolve_commit(root, value) != value:
        raise ValueError(f"{label} must identify its exact commit")
    result = subprocess.run(
        ["git", "merge-base", "--is-ancestor", value, "HEAD"],
        cwd=root,
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise ValueError(f"{label} must be an ancestor of HEAD")
    return value


def commit_is_ancestor(root: Path, ancestor: str, descendant: str) -> bool:
    return (
        subprocess.run(
            ["git", "merge-base", "--is-ancestor", ancestor, descendant],
            cwd=root,
            check=False,
            capture_output=True,
            text=True,
        ).returncode
        == 0
    )


def approved_hotspot_exceptions(_root: Path, status: dict[str, Any]) -> dict[str, str]:
    raw_exceptions = status.get("approved_hotspot_exceptions")
    if not isinstance(raw_exceptions, list):
        raise ValueError("completion status must define approved_hotspot_exceptions")
    if raw_exceptions:
        raise ValueError(
            "completion schema v2 permits no hotspot exception; add a structured ADR schema before recording one"
        )
    return {}


def completion_evidence(
    root: Path, hotspot_evidence: dict[str, Any] | None = None
) -> dict[str, Any]:
    manifest = load_json(root, TASK_MANIFEST_CONFIG)
    status = load_json(root, COMPLETION_STATUS_CONFIG)
    if not isinstance(manifest, dict) or not isinstance(status, dict):
        raise ValueError("task manifest and completion status must be objects")
    required_status_fields = {
        "approved_hotspot_exceptions",
        "expected_task_count",
        "overall_status",
        "phase_gates",
        "policy",
        "tasks",
        "version",
    }
    if set(status) != required_status_fields or status.get("version") != 2:
        raise ValueError("completion status must use the exact version 2 schema")
    approved_hotspot_exceptions(root, status)
    tasks = manifest.get("tasks")
    task_statuses = status.get("tasks")
    if not isinstance(tasks, dict) or not isinstance(task_statuses, dict):
        raise ValueError("task manifest and completion status must contain task objects")
    if len(tasks) != EXPECTED_TASK_COUNT:
        raise ValueError(f"task manifest must contain {EXPECTED_TASK_COUNT} tasks")
    if status.get("expected_task_count") != EXPECTED_TASK_COUNT:
        raise ValueError(f"completion status must expect {EXPECTED_TASK_COUNT} tasks")
    if set(task_statuses) != set(tasks):
        raise ValueError("completion task IDs must exactly match the task manifest")

    phases: dict[str, list[str]] = {phase: [] for phase in EXPECTED_PHASES}
    status_counts = {value: 0 for value in sorted(TASK_STATUSES)}
    verified_tasks: dict[str, dict[str, Any]] = {}
    evidence_owners: dict[str, str] = {}
    for task_id, task in tasks.items():
        if not isinstance(task, dict) or task.get("phase") not in phases:
            raise ValueError(f"manifest task has an invalid phase: {task_id}")
        record = task_statuses[task_id]
        if not isinstance(record, dict) or set(record) != {"evidence_commits", "status"}:
            raise ValueError(f"completion task has invalid fields: {task_id}")
        observed_status = record["status"]
        commits = record["evidence_commits"]
        if observed_status not in TASK_STATUSES:
            raise ValueError(f"completion task has an invalid status: {task_id}")
        if not isinstance(commits, list) or commits != sorted(set(commits)):
            raise ValueError(f"completion task evidence must be a sorted unique array: {task_id}")
        if observed_status == "complete" and not commits:
            raise ValueError(f"complete task has no commit evidence: {task_id}")
        for commit in commits:
            require_ancestor_commit(root, commit, label=f"task {task_id} evidence")
            previous_owner = evidence_owners.setdefault(commit, task_id)
            if previous_owner != task_id:
                raise ValueError(
                    f"task evidence commit is reused: {previous_owner} and {task_id}"
                )
        phases[task["phase"]].append(task_id)
        status_counts[observed_status] += 1
        verified_tasks[task_id] = {
            "evidence_commits": commits,
            "status": observed_status,
        }

    for task_id, task in tasks.items():
        record = verified_tasks[task_id]
        if record["status"] != "complete":
            continue
        dependencies = task.get("depends_on")
        if not isinstance(dependencies, list) or not all(
            isinstance(dependency, str) for dependency in dependencies
        ):
            raise ValueError(f"manifest task has invalid dependencies: {task_id}")
        for dependency in dependencies:
            dependency_record = verified_tasks.get(dependency)
            if dependency_record is None or dependency_record["status"] != "complete":
                raise ValueError(f"complete task has incomplete dependency: {task_id}")
            if not any(
                dependency_commit != task_commit
                and commit_is_ancestor(root, dependency_commit, task_commit)
                for dependency_commit in dependency_record["evidence_commits"]
                for task_commit in record["evidence_commits"]
            ):
                raise ValueError(
                    f"task evidence predates dependency evidence: {task_id} <- {dependency}"
                )

    raw_gates = status.get("phase_gates")
    if not isinstance(raw_gates, dict) or set(raw_gates) != set(EXPECTED_PHASES):
        raise ValueError("completion status must define exactly phase gates A through F")
    gates: dict[str, dict[str, Any]] = {}
    required_gate_fields = {"github_checks", "local_validation", "status"}
    for phase in EXPECTED_PHASES:
        gate = raw_gates[phase]
        if not isinstance(gate, dict) or set(gate) != required_gate_fields:
            raise ValueError(f"phase gate {phase} has invalid fields")
        local_validation = gate["local_validation"]
        github_checks = gate["github_checks"]
        if not isinstance(local_validation, dict) or set(local_validation) != {"commit", "status"}:
            raise ValueError(f"phase gate {phase} has invalid local validation evidence")
        if not isinstance(github_checks, dict) or set(github_checks) != {"commit", "run_url", "status"}:
            raise ValueError(f"phase gate {phase} has invalid GitHub check evidence")
        local_state = local_validation["status"]
        github_state = github_checks["status"]
        gate_status = gate["status"]
        phase_tasks = sorted(phases[phase])

        def require_gate_covers_tasks(commit: str, *, label: str) -> None:
            for task_id in phase_tasks:
                for task_commit in verified_tasks[task_id]["evidence_commits"]:
                    if not commit_is_ancestor(root, task_commit, commit):
                        raise ValueError(
                            f"{label} predates task evidence: {task_id}"
                        )

        if local_state not in LOCAL_VALIDATION_STATES:
            raise ValueError(f"phase gate {phase} has an invalid local validation state")
        if github_state not in GITHUB_CHECK_STATES:
            raise ValueError(f"phase gate {phase} has an invalid GitHub check state")
        if gate_status not in {"complete", "incomplete"}:
            raise ValueError(f"phase gate {phase} has an invalid status")
        if local_state == "pending":
            if local_validation["commit"] is not None:
                raise ValueError(f"phase gate {phase} pending local validation has a commit")
        else:
            local_commit = require_ancestor_commit(
                root,
                local_validation["commit"],
                label=f"phase gate {phase} local validation",
            )
            require_gate_covers_tasks(
                local_commit, label=f"phase gate {phase} local validation"
            )
        if github_state == "not_run":
            if github_checks["commit"] is not None or github_checks["run_url"] is not None:
                raise ValueError(f"phase gate {phase} not-run GitHub checks have evidence")
        else:
            github_commit = require_ancestor_commit(
                root,
                github_checks["commit"],
                label=f"phase gate {phase} GitHub checks",
            )
            require_gate_covers_tasks(
                github_commit, label=f"phase gate {phase} GitHub checks"
            )
            run_url = github_checks["run_url"]
            if not isinstance(run_url, str) or GITHUB_RUN_RE.fullmatch(run_url) is None:
                raise ValueError(f"phase gate {phase} has an invalid GitHub run URL")
        phase_complete = all(
            verified_tasks[task_id]["status"] == "complete" for task_id in phase_tasks
        )
        can_be_complete = phase_complete and local_state == "pass" and github_state == "pass"
        if (gate_status == "complete") != can_be_complete:
            raise ValueError(f"phase gate {phase} status does not match its evidence")
        gates[phase] = {
            **gate,
            "task_count": len(phase_tasks),
            "task_status_counts": {
                value: sum(
                    verified_tasks[task_id]["status"] == value for task_id in phase_tasks
                )
                for value in sorted(TASK_STATUSES)
            },
        }

    if hotspot_evidence is None:
        hotspot_evidence = remaining_hotspots(root)
    unresolved_plan_targets = hotspot_evidence["plan_targets"]["unresolved_count"]
    overall_status = status.get("overall_status")
    expected_overall = (
        "complete"
        if status_counts["complete"] == EXPECTED_TASK_COUNT
        and all(gate["status"] == "complete" for gate in gates.values())
        and unresolved_plan_targets == 0
        else "incomplete"
    )
    if overall_status != expected_overall:
        raise ValueError("overall completion status does not match task, gate, and hotspot evidence")
    policy = status.get("policy")
    if not isinstance(policy, str) or not policy.strip():
        raise ValueError("completion status must define a policy")
    return {
        "approved_hotspot_exception_count": len(status["approved_hotspot_exceptions"]),
        "ledger_sha256": json_digest(status),
        "overall_status": overall_status,
        "phase_gates": gates,
        "policy": policy,
        "task_count": len(task_statuses),
        "task_status_counts": status_counts,
        "tasks": dict(sorted(verified_tasks.items())),
        "unresolved_plan_target_count": unresolved_plan_targets,
    }


def remaining_hotspots(root: Path) -> dict[str, Any]:
    source_config = load_json(root, SOURCE_SIZE_CONFIG)
    test_config = load_json(root, TEST_SOURCE_SIZE_CONFIG)
    completion_status = load_json(root, COMPLETION_STATUS_CONFIG)
    if not all(
        isinstance(value, dict)
        for value in (source_config, test_config, completion_status)
    ):
        raise ValueError("source-size and completion configurations must be objects")
    approvals = approved_hotspot_exceptions(root, completion_status)
    facade_paths = set(source_config.get("facade_paths", []))
    source_baselines = source_config.get("baselines")
    test_baselines = test_config.get("baselines")
    if not isinstance(source_baselines, dict) or not isinstance(test_baselines, dict):
        raise ValueError("source-size configurations must contain baseline objects")
    source_bootstrap = source_config.get("bootstrap_ref")
    test_bootstrap = test_config.get("bootstrap_ref")
    if source_bootstrap != test_bootstrap:
        raise ValueError("source and test size ratchets must share one bootstrap_ref")
    bootstrap_ref = require_ancestor_commit(
        root, source_bootstrap, label="source-size bootstrap_ref"
    )
    bootstrap_configs = {
        SOURCE_SIZE_CONFIG.as_posix(): load_json_at_commit(
            root, SOURCE_SIZE_CONFIG, bootstrap_ref
        ),
        TEST_SOURCE_SIZE_CONFIG.as_posix(): load_json_at_commit(
            root, TEST_SOURCE_SIZE_CONFIG, bootstrap_ref
        ),
    }
    for authority, current_baselines in (
        (SOURCE_SIZE_CONFIG, source_baselines),
        (TEST_SOURCE_SIZE_CONFIG, test_baselines),
    ):
        bootstrap_config = bootstrap_configs[authority.as_posix()]
        bootstrap_baselines = (
            bootstrap_config.get("baselines")
            if isinstance(bootstrap_config, dict)
            else None
        )
        if not isinstance(bootstrap_baselines, dict):
            raise ValueError(f"bootstrap {authority.as_posix()} has no baselines")
        for path, ceiling in current_baselines.items():
            bootstrap_ceiling = bootstrap_baselines.get(path)
            if not isinstance(ceiling, dict) or not isinstance(bootstrap_ceiling, dict):
                raise ValueError(f"configured ratchet lacks bootstrap provenance: {path}")
            for dimension in ("bytes", "lines"):
                current_limit = ceiling.get(dimension)
                bootstrap_limit = bootstrap_ceiling.get(dimension)
                if not isinstance(current_limit, int) or not isinstance(
                    bootstrap_limit, int
                ):
                    raise ValueError(f"invalid configured ratchet ceiling: {path}")
                if current_limit > bootstrap_limit:
                    raise ValueError(f"configured ratchet exceeds bootstrap ceiling: {path}")

    overages: list[dict[str, Any]] = []
    configured_ratchets: list[dict[str, Any]] = []
    observed_baselines: set[tuple[str, str]] = set()

    def record(
        relative: Path,
        kind: str,
        language: str,
        limits: dict[str, Any],
        baselines: dict[str, Any],
        authority: Path,
        bootstrap_ref: Any,
    ) -> None:
        path = relative.as_posix()
        content = (root / relative).read_bytes()
        try:
            lines = len(content.decode("utf-8").splitlines())
        except UnicodeDecodeError as error:
            raise ValueError(f"source is not UTF-8: {path}") from error
        try:
            hard_limit = limits[kind][language]
            byte_limit = hard_limit["bytes"]
            line_limit = hard_limit["lines"]
        except (KeyError, TypeError) as error:
            raise ValueError(f"missing hard target for {kind}.{language}") from error
        bytes_count = len(content)
        exceeds_hard_target = bytes_count > byte_limit or lines > line_limit
        baseline = baselines.get(path)
        if baseline is not None:
            if not isinstance(baseline, dict) or not all(
                isinstance(baseline.get(key), int) and baseline[key] > 0
                for key in ("bytes", "lines")
            ):
                raise ValueError(f"invalid hotspot baseline: {path}")
            if bytes_count > baseline["bytes"] or lines > baseline["lines"]:
                raise ValueError(f"configured ratchet is below the current source: {path}")
            observed_baselines.add((authority.as_posix(), path))
            configured_ratchets.append(
                {
                    "authority": f"{authority.as_posix()}#baselines/{path}",
                    "current_bytes": bytes_count,
                    "current_lines": lines,
                    "hard_target_bytes": byte_limit,
                    "hard_target_lines": line_limit,
                    "path": path,
                    "ratchet_bytes": baseline["bytes"],
                    "ratchet_lines": baseline["lines"],
                    "status": (
                        "active-over-hard-target"
                        if exceeds_hard_target
                        else "eligible-for-removal"
                    ),
                }
            )
        if not exceeds_hard_target:
            return
        if baseline is not None:
            ratchet = {
                "authority": f"{authority.as_posix()}#baselines/{path}",
                "bytes": baseline["bytes"],
                "kind": "configured-baseline",
                "lines": baseline["lines"],
            }
        elif kind == "test":
            raise ValueError(f"unratcheted test hotspot: {path}")
        else:
            if not isinstance(bootstrap_ref, str) or FULL_COMMIT_RE.fullmatch(bootstrap_ref) is None:
                raise ValueError("source-size baseline has no valid bootstrap_ref")
            require_ancestor_commit(root, bootstrap_ref, label="source-size bootstrap_ref")
            bootstrap_size = source_size_at_ref(root, bootstrap_ref, relative)
            if bootstrap_size is None:
                raise ValueError(f"unratcheted source hotspot was absent at bootstrap: {path}")
            if bytes_count > byte_limit and bootstrap_size.bytes <= byte_limit:
                raise ValueError(f"source byte hotspot was not over target at bootstrap: {path}")
            if lines > line_limit and bootstrap_size.lines <= line_limit:
                raise ValueError(f"source line hotspot was not over target at bootstrap: {path}")
            if (
                bytes_count > max(byte_limit, bootstrap_size.bytes)
                or lines > max(line_limit, bootstrap_size.lines)
            ):
                raise ValueError(f"source hotspot exceeds its bootstrap ratchet: {path}")
            ratchet = {
                "authority": f"{SOURCE_SIZE_CONFIG.as_posix()}#bootstrap_ref",
                "bootstrap_bytes": bootstrap_size.bytes,
                "bootstrap_lines": bootstrap_size.lines,
                "kind": "bootstrap-legacy",
                "ref": bootstrap_ref,
            }
        approval = approvals.get(path)
        overages.append(
            {
                "approved_exception": (
                    {"authority": approval} if approval is not None else None
                ),
                "bytes": bytes_count,
                "disposition": "approved-exception" if approval is not None else "unresolved",
                "hard_target_bytes": byte_limit,
                "hard_target_lines": line_limit,
                "kind": kind,
                "language": language,
                "lines": lines,
                "path": path,
                "ratchet": ratchet,
            }
        )

    source_limits = source_config.get("limits")
    if not isinstance(source_limits, dict):
        raise ValueError("source-size baseline must contain limits")
    source_paths = sorted(
        {
            *production_sources(root, exclude_test_sources=True),
            *generated_sources(root),
        }
    )
    for relative in source_paths:
        classification = classify_source(relative, facade_paths)
        if classification is None:
            continue
        kind, language = classification
        record(
            relative,
            kind,
            language,
            source_limits,
            source_baselines,
            SOURCE_SIZE_CONFIG,
            source_config.get("bootstrap_ref"),
        )

    test_limits = test_config.get("limits")
    if not isinstance(test_limits, dict):
        raise ValueError("test-source-size baseline must contain limits")
    for relative in test_sources(root):
        language = source_language(relative)
        if language is None:
            continue
        record(
            relative,
            "test",
            language,
            test_limits,
            test_baselines,
            TEST_SOURCE_SIZE_CONFIG,
            test_config.get("bootstrap_ref"),
        )

    expected_baselines = {
        *((SOURCE_SIZE_CONFIG.as_posix(), path) for path in source_baselines),
        *((TEST_SOURCE_SIZE_CONFIG.as_posix(), path) for path in test_baselines),
    }
    missing_baselines = sorted(expected_baselines - observed_baselines)
    if missing_baselines:
        authority, path = missing_baselines[0]
        raise ValueError(f"stale or unmeasured configured ratchet: {authority}#{path}")
    plan_targets: list[dict[str, Any]] = []
    for path, target in sorted(P0_TARGETS.items()):
        content = (root / path).read_bytes()
        try:
            lines = len(content.decode("utf-8").splitlines())
        except UnicodeDecodeError as error:
            raise ValueError(f"P0 target is not UTF-8: {path}") from error
        bytes_count = len(content)
        target_met = bytes_count <= target["bytes"] and lines <= target["lines"]
        approval = approvals.get(path)
        disposition = (
            "target-met"
            if target_met
            else "approved-exception"
            if approval is not None
            else "unresolved"
        )
        plan_targets.append(
            {
                "approved_exception": (
                    {"authority": approval} if approval is not None else None
                ),
                "bytes": bytes_count,
                "disposition": disposition,
                "lines": lines,
                "path": path,
                "target_bytes": target["bytes"],
                "target_lines": target["lines"],
            }
        )

    observed_overages = {entry["path"] for entry in overages}
    observed_plan_exceptions = {
        entry["path"] for entry in plan_targets if entry["disposition"] != "target-met"
    }
    stale_approvals = sorted(
        set(approvals) - observed_overages - observed_plan_exceptions
    )
    if stale_approvals:
        raise ValueError(f"approved hotspot exception is not a current overage: {stale_approvals[0]}")

    overages.sort(key=lambda entry: (-entry["bytes"], -entry["lines"], entry["path"]))
    configured_ratchets.sort(key=lambda entry: entry["path"])
    approved_count = sum(entry["disposition"] == "approved-exception" for entry in overages)
    unresolved_count = len(overages) - approved_count
    active_ratchets = sum(
        entry["status"] == "active-over-hard-target" for entry in configured_ratchets
    )
    met_plan_targets = sum(
        entry["disposition"] == "target-met" for entry in plan_targets
    )
    approved_plan_targets = sum(
        entry["disposition"] == "approved-exception" for entry in plan_targets
    )
    return {
        "configured_ratchets": {
            "active_over_hard_target_count": active_ratchets,
            "count": len(configured_ratchets),
            "eligible_for_removal_count": len(configured_ratchets) - active_ratchets,
            "entries": configured_ratchets,
        },
        "hard_target_overages": {
            "approved_exception_count": approved_count,
            "count": len(overages),
            "entries": overages,
            "unresolved_count": unresolved_count,
        },
        "plan_targets": {
            "approved_exception_count": approved_plan_targets,
            "count": len(plan_targets),
            "entries": plan_targets,
            "met_count": met_plan_targets,
            "unresolved_count": len(plan_targets)
            - met_plan_targets
            - approved_plan_targets,
        },
        "policy": "Size baselines and bootstrap history are non-growth ratchets, not design approvals. Schema v2 records zero approved exceptions and rejects non-empty entries until a structured ADR binding is implemented.",
    }


def build_report(root: Path, baseline_commit: str) -> dict[str, Any]:
    files: dict[str, dict[str, Any]] = {}
    public_by_file: dict[str, dict[str, Any]] = {}
    total_bytes = 0
    total_lines = 0
    total_declarations = 0
    total_public = 0

    for relative in source_files(root):
        content = (root / relative).read_bytes()
        try:
            text = content.decode("utf-8")
        except UnicodeDecodeError as error:
            raise ValueError(f"source is not UTF-8: {relative.as_posix()}") from error
        lines = len(text.splitlines())
        declarations = len(DECLARATION_RE.findall(text))
        public_lines = normalized_public_lines(relative, text)
        path = relative.as_posix()
        files[path] = {
            "bytes": len(content),
            "declarations": declarations,
            "lines": lines,
            "public_symbols": len(public_lines),
        }
        if public_lines:
            public_by_file[path] = {
                "count": len(public_lines),
                "sha256": json_digest(public_lines),
            }
        total_bytes += len(content)
        total_lines += lines
        total_declarations += declarations
        total_public += len(public_lines)

    hotspot_order = sorted(
        files,
        key=lambda path: (-files[path]["bytes"], -files[path]["lines"], path),
    )
    source_size_config = load_json(root, SOURCE_SIZE_CONFIG)
    core_storage_api = load_json(root, CORE_STORAGE_API_CONFIG)
    ipc_commands = ipc_command_names(load_json(root, IPC_CONFIG))
    combined_public_surface = [
        f"{path}:{entry['count']}:{entry['sha256']}"
        for path, entry in public_by_file.items()
    ]

    hotspots = remaining_hotspots(root)
    completion = completion_evidence(root, hotspots)
    test_source_size_config = load_json(root, TEST_SOURCE_SIZE_CONFIG)
    ai_context_config = load_json(root, AI_CONTEXT_CONFIG)
    task_manifest = load_json(root, TASK_MANIFEST_CONFIG)
    completion_status = load_json(root, COMPLETION_STATUS_CONFIG)

    return {
        "format_version": 2,
        "baseline_commit": baseline_commit,
        "completion": completion,
        "policy": "deterministic worktree measurement; no timestamps or host paths",
        "summary": {
            "production_bytes": total_bytes,
            "production_declarations": total_declarations,
            "production_files": len(files),
            "production_lines": total_lines,
            "public_symbols": total_public,
        },
        "hotspots": [
            {"path": path, **files[path]} for path in hotspot_order[:100]
        ],
        "files": files,
        "public_api": {
            "by_file": public_by_file,
            "core_api_version": rust_u32_constant(
                root, Path("crates/core/src/lib.rs"), "CORE_API_VERSION"
            ),
            "shell_api_version": rust_u32_constant(
                root, Path("crates/shell-api/src/lib.rs"), "SHELL_API_VERSION"
            ),
            "policy": "conservative source-level declaration fingerprint, not semantic rustdoc API",
            "sha256": json_digest(combined_public_surface),
        },
        "ipc_commands": {
            "count": len(ipc_commands),
            "names": ipc_commands,
            "sha256": json_digest(ipc_commands),
        },
        "ratchets": {
            AI_CONTEXT_CONFIG.as_posix(): json_digest(ai_context_config),
            COMPLETION_STATUS_CONFIG.as_posix(): json_digest(completion_status),
            SOURCE_SIZE_CONFIG.as_posix(): json_digest(source_size_config),
            TASK_MANIFEST_CONFIG.as_posix(): json_digest(task_manifest),
            TEST_SOURCE_SIZE_CONFIG.as_posix(): json_digest(test_source_size_config),
            CORE_STORAGE_API_CONFIG.as_posix(): json_digest(core_storage_api),
        },
        "remaining_hotspots": hotspots,
    }


def serialized_report(report: dict[str, Any]) -> str:
    return json.dumps(report, ensure_ascii=False, indent=2, sort_keys=True) + "\n"


def compact_summary(report: dict[str, Any]) -> dict[str, Any]:
    completion = report["completion"]
    remaining = report["remaining_hotspots"]
    return {
        "baseline_commit": report["baseline_commit"],
        "completion": {
            "overall_status": completion["overall_status"],
            "phase_gates": {
                phase: {
                    "github_checks": gate["github_checks"]["status"],
                    "local_validation": gate["local_validation"]["status"],
                    "status": gate["status"],
                }
                for phase, gate in completion["phase_gates"].items()
            },
            "task_status_counts": completion["task_status_counts"],
            "unresolved_plan_target_count": completion[
                "unresolved_plan_target_count"
            ],
        },
        "format_version": 1,
        "full_report_sha256": sha256_bytes(
            serialized_report(report).encode("utf-8")
        ),
        "remaining_hotspots": {
            "approved_exception_count": remaining["hard_target_overages"][
                "approved_exception_count"
            ],
            "configured_ratchet_count": remaining["configured_ratchets"]["count"],
            "hard_target_overage_count": remaining["hard_target_overages"]["count"],
            "plan_target_met_count": remaining["plan_targets"]["met_count"],
            "plan_target_unresolved_count": remaining["plan_targets"][
                "unresolved_count"
            ],
            "unresolved_hard_target_count": remaining["hard_target_overages"][
                "unresolved_count"
            ],
        },
        "summary": report["summary"],
    }


def serialized_summary(report: dict[str, Any]) -> str:
    return json.dumps(
        compact_summary(report), ensure_ascii=False, indent=2, sort_keys=True
    ) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=REPO_ROOT)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--summary-output", type=Path, default=DEFAULT_SUMMARY_OUTPUT)
    parser.add_argument("--baseline-ref", default="HEAD")
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--print", dest="print_report", action="store_true")
    args = parser.parse_args()

    root = args.root.resolve()
    output = args.output
    if not output.is_absolute():
        output = root / output
    summary_output = args.summary_output
    if not summary_output.is_absolute():
        summary_output = root / summary_output
    try:
        if args.check and output.is_file():
            expected = json.loads(output.read_text(encoding="utf-8"))
            recorded_commit = expected.get("baseline_commit")
            if not isinstance(recorded_commit, str):
                raise ValueError("baseline report has no baseline_commit")
            baseline_commit = recorded_commit
        else:
            baseline_commit = resolve_commit(root, args.baseline_ref)
        report = build_report(root, baseline_commit)
    except (OSError, json.JSONDecodeError, ValueError) as error:
        print(f"refactoring baseline: FAIL: {error}", file=sys.stderr)
        return 1

    rendered = serialized_report(report)
    rendered_summary = serialized_summary(report)
    if args.print_report:
        print(rendered, end="")
        return 0
    if args.check:
        try:
            current = output.read_text(encoding="utf-8")
            current_summary = summary_output.read_text(encoding="utf-8")
        except OSError as error:
            print(f"refactoring baseline: FAIL: cannot read snapshot: {error}", file=sys.stderr)
            return 1
        if current != rendered or current_summary != rendered_summary:
            print(
                "refactoring baseline: FAIL: snapshot differs from the current worktree",
                file=sys.stderr,
            )
            return 1
        print("refactoring baseline: PASS")
        return 0

    output.parent.mkdir(parents=True, exist_ok=True)
    summary_output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(rendered, encoding="utf-8")
    summary_output.write_text(rendered_summary, encoding="utf-8")
    print(
        "refactoring baseline: wrote "
        f"{output.relative_to(root)} and {summary_output.relative_to(root)}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
