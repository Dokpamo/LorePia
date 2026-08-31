#!/usr/bin/env python3
"""Validate bounded, repository-local AI context bundles for refactoring tasks."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_CONFIG = REPO_ROOT / "config" / "ai-context-map.json"
DEFAULT_MANIFEST = REPO_ROOT / "config" / "refactoring" / "task-manifest.yaml"
PATH_GROUPS = ("documents", "entrypoints", "implementation", "tests")
EXPECTED_TASK_COUNT = 59
COMPLETION_TASK_ID = "ENF-004"
AI_CONTEXT_BOOTSTRAP_REF = "98e854effb89617147ee51f99dbebb5a5fcd1ccd"
CONTEXT_MAP_POLICY = (
    "Repository-local read starting points, not a security allowlist; reviewed "
    "pre-enforcement path migrations preserve entry counts, then base-ref path "
    "removals require explicit reviewed replacements"
)


@dataclass(frozen=True)
class ContextMeasurement:
    task_id: str
    bytes: int
    files: int
    target: int
    baseline: int | None

    @property
    def legacy_over_budget(self) -> bool:
        return self.bytes > self.target and self.baseline is not None


def load_json(path: Path, label: str) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"cannot read {label} {path}: {error}") from error
    if not isinstance(value, dict):
        raise ValueError(f"{label} must be an object")
    return value


def validate_string_list(value: object, label: str) -> list[str]:
    if not isinstance(value, list) or not all(
        isinstance(item, str)
        and item
        and item == item.strip()
        and "\n" not in item
        and "\r" not in item
        for item in value
    ):
        raise ValueError(f"{label} must be an array of non-empty single-line strings")
    if len(value) != len(set(value)):
        raise ValueError(f"{label} contains duplicates")
    return value


def dependency_closure(
    tasks: dict[str, dict[str, Any]], task_id: str
) -> set[str]:
    reached: set[str] = set()
    pending = [task_id]
    while pending:
        current = pending.pop()
        if current in reached:
            continue
        reached.add(current)
        pending.extend(tasks[current]["depends_on"])
    return reached


def manifest_tasks(
    manifest: dict[str, Any], *, require_completion: bool = True
) -> dict[str, dict[str, Any]]:
    if manifest.get("version") != 1:
        raise ValueError("task manifest version must be 1")
    raw_tasks = manifest.get("tasks")
    if not isinstance(raw_tasks, dict) or not raw_tasks:
        raise ValueError("task manifest must contain a non-empty tasks object")
    if not all(isinstance(task_id, str) and task_id for task_id in raw_tasks):
        raise ValueError("task manifest IDs must be non-empty strings")

    tasks: dict[str, dict[str, Any]] = {}
    for task_id, raw_entry in raw_tasks.items():
        if not isinstance(raw_entry, dict):
            raise ValueError(f"manifest task {task_id} must be an object")
        dependencies = validate_string_list(
            raw_entry.get("depends_on"), f"{task_id}.depends_on"
        )
        tasks[task_id] = {**raw_entry, "depends_on": dependencies}

    task_ids = set(tasks)
    for task_id, entry in tasks.items():
        unknown = sorted(set(entry["depends_on"]) - task_ids)
        if unknown:
            raise ValueError(
                f"{task_id}.depends_on contains unknown tasks: {', '.join(unknown)}"
            )
        if task_id in entry["depends_on"]:
            raise ValueError(f"{task_id}.depends_on must not contain itself")

    states: dict[str, int] = {}
    trail: list[str] = []

    def visit(task_id: str) -> None:
        state = states.get(task_id, 0)
        if state == 2:
            return
        if state == 1:
            cycle_start = trail.index(task_id)
            cycle = " -> ".join([*trail[cycle_start:], task_id])
            raise ValueError(f"task manifest dependency cycle: {cycle}")
        states[task_id] = 1
        trail.append(task_id)
        for dependency in sorted(tasks[task_id]["depends_on"]):
            visit(dependency)
        trail.pop()
        states[task_id] = 2

    for task_id in sorted(tasks):
        visit(task_id)

    decisions = manifest.get("decisions")
    if decisions is not None:
        if not isinstance(decisions, dict):
            raise ValueError("task manifest decisions must be an object")
        declared_count = decisions.get("task_count")
        if type(declared_count) is not int or declared_count != EXPECTED_TASK_COUNT:
            raise ValueError(
                f"task manifest decisions.task_count must be {EXPECTED_TASK_COUNT}"
            )
        if len(tasks) != declared_count:
            raise ValueError(
                f"task manifest contains {len(tasks)} tasks; expected {declared_count}"
            )

    if require_completion and COMPLETION_TASK_ID in tasks:
        reached = dependency_closure(tasks, COMPLETION_TASK_ID)
        missing = sorted(task_ids - reached)
        if missing:
            raise ValueError(
                f"{COMPLETION_TASK_ID} dependency DAG does not reach: "
                + ", ".join(missing)
            )
    elif require_completion and decisions is not None:
        raise ValueError(f"task manifest must contain {COMPLETION_TASK_ID}")
    return tasks


def manifest_task_ids(
    manifest: dict[str, Any], *, require_completion: bool = True
) -> set[str]:
    return set(manifest_tasks(manifest, require_completion=require_completion))


def validate_repository_manifest(manifest: dict[str, Any]) -> set[str]:
    if not isinstance(manifest.get("decisions"), dict):
        raise ValueError("task manifest must contain a decisions object")
    return manifest_task_ids(manifest)


def validate_relative_path(raw_path: object, label: str) -> str:
    if not isinstance(raw_path, str) or not raw_path:
        raise ValueError(f"{label} must be a non-empty repository-relative path")
    if "\\" in raw_path or any(character in raw_path for character in "\0\n\r\t"):
        raise ValueError(f"{label} must use POSIX separators: {raw_path}")
    pure = PurePosixPath(raw_path)
    if pure.is_absolute() or ".." in pure.parts or pure.as_posix() != raw_path:
        raise ValueError(f"{label} is not a canonical repository-relative path: {raw_path}")
    return raw_path


def validate_relative_file(root: Path, raw_path: object, label: str) -> tuple[str, int]:
    path = validate_relative_path(raw_path, label)
    pure = PurePosixPath(path)
    candidate = root.joinpath(*pure.parts)
    try:
        resolved = candidate.resolve(strict=True)
        resolved.relative_to(root)
    except (OSError, ValueError) as error:
        raise ValueError(f"{label} is missing or escapes the repository: {raw_path}") from error
    if not resolved.is_file():
        raise ValueError(f"{label} is not a regular file: {raw_path}")
    return resolved.relative_to(root).as_posix(), resolved.stat().st_size


def resolve_commit(root: Path, revision: str) -> str:
    result = subprocess.run(
        ["git", "rev-parse", "--verify", "--end-of-options", f"{revision}^{{commit}}"],
        cwd=root,
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise ValueError(f"base ref is not a commit: {revision}")
    return result.stdout.strip()


def repository_relative_path(root: Path, path: Path, label: str) -> Path:
    try:
        relative = path.resolve().relative_to(root)
    except ValueError as error:
        raise ValueError(f"{label} must be inside the repository: {path}") from error
    if not relative.parts:
        raise ValueError(f"{label} must name a file")
    return relative


def load_git_json(
    root: Path,
    commit: str,
    relative: Path,
    label: str,
    *,
    allow_missing: bool = False,
) -> dict[str, Any] | None:
    result = subprocess.run(
        ["git", "show", f"{commit}:{relative.as_posix()}"],
        cwd=root,
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        if allow_missing:
            return None
        raise ValueError(
            f"cannot read {label} at base ref {commit}: {relative.as_posix()}"
        )
    try:
        value = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise ValueError(f"invalid {label} JSON at base ref {commit}: {error}") from error
    if not isinstance(value, dict):
        raise ValueError(f"{label} at base ref {commit} must be an object")
    return value


def commit_is_ancestor(root: Path, ancestor: str, descendant: str) -> bool:
    result = subprocess.run(
        ["git", "merge-base", "--is-ancestor", ancestor, descendant],
        cwd=root,
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode not in {0, 1}:
        raise ValueError("cannot compare context-map bootstrap ancestry")
    return result.returncode == 0


def load_comparison_snapshot(
    root: Path, commit: str, config_relative: Path, manifest_relative: Path
) -> tuple[str, dict[str, Any], dict[str, Any]]:
    head = resolve_commit(root, "HEAD")
    if not commit_is_ancestor(root, commit, head):
        raise ValueError("base ref must be an ancestor of HEAD")
    base_config = load_git_json(
        root,
        commit,
        config_relative,
        "AI context map",
        allow_missing=True,
    )
    base_manifest = load_git_json(
        root,
        commit,
        manifest_relative,
        "task manifest",
        allow_missing=True,
    )
    if (base_config is None) != (base_manifest is None):
        raise ValueError("base ref must contain both the context map and task manifest")
    if base_config is not None and base_manifest is not None:
        return commit, base_config, base_manifest

    bootstrap = resolve_commit(root, AI_CONTEXT_BOOTSTRAP_REF)
    if not commit_is_ancestor(root, commit, bootstrap):
        raise ValueError(
            "base ref omits the context map after its reviewed bootstrap commit"
        )
    if not commit_is_ancestor(root, bootstrap, head):
        raise ValueError("AI context-map bootstrap must be an ancestor of HEAD")
    bootstrap_config = load_git_json(
        root, bootstrap, config_relative, "AI context map"
    )
    bootstrap_manifest = load_git_json(
        root, bootstrap, manifest_relative, "task manifest"
    )
    assert bootstrap_config is not None and bootstrap_manifest is not None
    return bootstrap, bootstrap_config, bootstrap_manifest


def git_tree_file_sizes(root: Path, commit: str) -> dict[str, int]:
    result = subprocess.run(
        ["git", "ls-tree", "-r", "-l", "-z", commit],
        cwd=root,
        check=False,
        capture_output=True,
    )
    if result.returncode != 0:
        raise ValueError(f"cannot inspect repository tree at base ref {commit}")
    files: dict[str, int] = {}
    for raw_record in result.stdout.split(b"\0"):
        if not raw_record:
            continue
        try:
            raw_metadata, raw_path = raw_record.split(b"\t", 1)
            mode, kind, _object_id, raw_size = raw_metadata.split(b" ", 3)
            path = raw_path.decode("utf-8")
        except (UnicodeDecodeError, ValueError) as error:
            raise ValueError(f"invalid git tree record at base ref {commit}") from error
        if kind == b"blob" and mode in {b"100644", b"100755"}:
            try:
                files[path] = int(raw_size)
            except ValueError as error:
                raise ValueError(f"invalid blob size for {path} at {commit}") from error
    return files


def measure_context_map_at_ref(
    root: Path,
    commit: str,
    config: dict[str, Any],
    manifest: dict[str, Any],
) -> dict[str, ContextMeasurement]:
    if config.get("version") != 1:
        raise ValueError("base AI context map version must be 1")
    tasks = manifest_task_ids(manifest, require_completion=False)
    contexts = config.get("contexts")
    if not isinstance(contexts, dict) or set(contexts) != tasks:
        raise ValueError("base context entries must exactly match base manifest tasks")
    shared = validate_string_list(config.get("shared_paths"), "base shared_paths")
    tree_sizes = git_tree_file_sizes(root, commit)

    def file_size(path: str, label: str) -> int:
        canonical = validate_relative_path(path, label)
        if canonical not in tree_sizes:
            raise ValueError(
                f"{label} is missing or not a regular file at base ref: {canonical}"
            )
        return tree_sizes[canonical]

    shared_sizes = {
        path: file_size(path, "base shared path") for path in shared
    }
    measurements: dict[str, ContextMeasurement] = {}
    default_limit = configured_context_limit(config, {}, "base context map")
    for task_id in sorted(tasks):
        entry = contexts[task_id]
        if not isinstance(entry, dict):
            raise ValueError(f"base {task_id}: context entry must be an object")
        raw_paths = entry.get("paths")
        if not isinstance(raw_paths, dict) or set(raw_paths) != set(PATH_GROUPS):
            raise ValueError(f"base {task_id}: paths have invalid groups")
        declared = set(shared)
        sizes: dict[str, int] = {}
        for group in PATH_GROUPS:
            paths = validate_string_list(
                raw_paths[group], f"base {task_id}.{group}"
            )
            for path in paths:
                if path in declared:
                    raise ValueError(f"base {task_id}: duplicate bundle path: {path}")
                declared.add(path)
                sizes[path] = file_size(path, f"base {task_id}.{group}")
        task_limit = configured_context_limit(config, entry, f"base {task_id}")
        if task_limit > default_limit:
            raise ValueError(f"base {task_id}: max_context_bytes exceeds its default")
        measurements[task_id] = ContextMeasurement(
            task_id=task_id,
            bytes=sum(shared_sizes.values()) + sum(sizes.values()),
            files=len(shared_sizes) + len(sizes),
            target=task_limit,
            baseline=configured_baseline(entry, f"base {task_id}"),
        )
    return measurements


def evaluate_bundle_size_drift(
    current: list[ContextMeasurement], base: dict[str, ContextMeasurement]
) -> list[str]:
    failures: list[str] = []
    current_by_task = {measurement.task_id: measurement for measurement in current}
    for task_id in sorted(set(current_by_task) & set(base)):
        measurement = current_by_task[task_id]
        base_measurement = base[task_id]
        allowed = max(base_measurement.target, base_measurement.bytes)
        if measurement.bytes > allowed:
            failures.append(
                f"{task_id}: context bytes grew beyond base-ref allowance: "
                f"{base_measurement.bytes} -> {measurement.bytes}; cap {allowed}"
            )
    return failures


def configured_context_limit(
    config: dict[str, Any], entry: dict[str, Any], label: str
) -> int:
    default_limit = config.get("default_max_context_bytes")
    if type(default_limit) is not int or default_limit <= 0:
        raise ValueError(f"{label} default_max_context_bytes must be a positive integer")
    task_limit = entry.get("max_context_bytes", default_limit)
    if type(task_limit) is not int or task_limit <= 0:
        raise ValueError(f"{label} max_context_bytes must be a positive integer")
    return task_limit


def configured_baseline(entry: dict[str, Any], label: str) -> int | None:
    baseline = entry.get("baseline_context_bytes")
    if baseline is not None and (type(baseline) is not int or baseline <= 0):
        raise ValueError(f"{label} baseline_context_bytes must be a positive integer or null")
    return baseline


def flattened_context_paths(entry: dict[str, Any], label: str) -> list[str]:
    raw_paths = entry.get("paths")
    if not isinstance(raw_paths, dict) or set(raw_paths) != set(PATH_GROUPS):
        raise ValueError(f"{label}: paths have invalid groups")
    flattened: list[str] = []
    for group in PATH_GROUPS:
        flattened.extend(validate_string_list(raw_paths[group], f"{label}.{group}"))
    if len(flattened) != len(set(flattened)):
        raise ValueError(f"{label}: duplicate paths across groups")
    return flattened


def reviewed_path_migrations(config: dict[str, Any]) -> dict[tuple[str, str], str]:
    raw_migrations = config.get("reviewed_path_migrations")
    if not isinstance(raw_migrations, list):
        raise ValueError("reviewed_path_migrations must be an array")
    migrations: dict[tuple[str, str], str] = {}
    required = {"from", "reason", "task_id", "to"}
    for index, raw_migration in enumerate(raw_migrations):
        label = f"reviewed_path_migrations[{index}]"
        if not isinstance(raw_migration, dict) or set(raw_migration) != required:
            raise ValueError(f"{label} has invalid fields")
        task_id = raw_migration["task_id"]
        reason = raw_migration["reason"]
        if not isinstance(task_id, str) or not task_id or task_id != task_id.strip():
            raise ValueError(f"{label}.task_id must be a non-empty trimmed string")
        if (
            not isinstance(reason, str)
            or not reason
            or reason != reason.strip()
            or "\n" in reason
            or "\r" in reason
        ):
            raise ValueError(f"{label}.reason must be a non-empty single-line string")
        source = validate_relative_path(raw_migration["from"], f"{label}.from")
        replacement = validate_relative_path(raw_migration["to"], f"{label}.to")
        if source == replacement:
            raise ValueError(f"{label} must replace a path with a different path")
        key = (task_id, source)
        if key in migrations:
            raise ValueError(f"duplicate reviewed path migration: {task_id}: {source}")
        migrations[key] = replacement
    return migrations


def evaluate_base_ref_drift(
    config: dict[str, Any],
    manifest: dict[str, Any],
    base_config: dict[str, Any],
    base_manifest: dict[str, Any],
) -> list[str]:
    """Compare durable task entries and context-budget ratchets with a base commit."""

    failures: list[str] = []
    current_tasks = manifest_task_ids(manifest)
    base_tasks = manifest_task_ids(base_manifest, require_completion=False)
    for task_id in sorted(base_tasks - current_tasks):
        failures.append(f"manifest task entry removed since base-ref: {task_id}")

    contexts = config.get("contexts")
    base_contexts = base_config.get("contexts")
    if base_config.get("version") != 1:
        raise ValueError("base AI context map version must be 1")
    if not isinstance(contexts, dict):
        raise ValueError("AI context map must contain a contexts object")
    if not isinstance(base_contexts, dict):
        raise ValueError("base AI context map must contain a contexts object")
    base_bootstrap = base_config.get("bootstrap_ref")
    migrations = reviewed_path_migrations(config)
    if (
        base_bootstrap is not None
        and config.get("bootstrap_ref") != base_bootstrap
    ):
        failures.append("AI context-map bootstrap_ref changed since base-ref")
    for task_id in sorted(set(base_contexts) - set(contexts)):
        failures.append(f"context entry removed since base-ref: {task_id}")
    shared_paths = validate_string_list(config.get("shared_paths"), "shared_paths")
    base_shared_paths = validate_string_list(
        base_config.get("shared_paths"), "base shared_paths"
    )
    if len(shared_paths) < len(base_shared_paths):
        failures.append(
            "shared path entries decreased since base-ref: "
            f"{len(base_shared_paths)} -> {len(shared_paths)}"
        )
    if base_bootstrap is not None:
        for path in sorted(set(base_shared_paths) - set(shared_paths)):
            failures.append(f"shared path entry removed since base-ref: {path}")

    current_default = configured_context_limit(config, {}, "current context map")
    base_default = configured_context_limit(base_config, {}, "base context map")
    if current_default > base_default:
        failures.append(
            "default_max_context_bytes increased since base-ref: "
            f"{base_default} -> {current_default}"
        )

    for task_id in sorted(set(contexts) & set(base_contexts)):
        entry = contexts[task_id]
        base_entry = base_contexts[task_id]
        if not isinstance(entry, dict):
            raise ValueError(f"{task_id}: context entry must be an object")
        if not isinstance(base_entry, dict):
            raise ValueError(f"base {task_id}: context entry must be an object")
        paths = flattened_context_paths(entry, task_id)
        base_paths = flattened_context_paths(base_entry, f"base {task_id}")
        path_count = len(paths)
        base_path_count = len(base_paths)
        if path_count < base_path_count:
            failures.append(
                f"{task_id}: context path entries decreased since base-ref: "
                f"{base_path_count} -> {path_count}"
            )
        if base_bootstrap is not None:
            for path in sorted(set(base_paths) - set(paths)):
                replacement = migrations.get((task_id, path))
                if replacement not in paths:
                    failures.append(
                        f"{task_id}: context path entry removed without a reviewed "
                        f"replacement: {path}"
                    )
        commands = entry.get("commands")
        base_commands = base_entry.get("commands")
        if not isinstance(commands, list) or not isinstance(base_commands, list):
            raise ValueError(f"{task_id}: current and base commands must be arrays")
        if len(commands) < len(base_commands):
            failures.append(
                f"{task_id}: targeted command entries decreased since base-ref: "
                f"{len(base_commands)} -> {len(commands)}"
            )
        task_limit = configured_context_limit(config, entry, task_id)
        base_task_limit = configured_context_limit(
            base_config, base_entry, f"base {task_id}"
        )
        if task_limit > base_task_limit:
            failures.append(
                f"{task_id}: max_context_bytes increased since base-ref: "
                f"{base_task_limit} -> {task_limit}"
            )
        baseline = configured_baseline(entry, task_id)
        base_baseline = configured_baseline(base_entry, f"base {task_id}")
        if baseline is not None and base_baseline is None:
            failures.append(f"{task_id}: new legacy context baseline added since base-ref")
        elif (
            baseline is not None
            and base_baseline is not None
            and baseline > base_baseline
        ):
            failures.append(
                f"{task_id}: baseline_context_bytes increased since base-ref: "
                f"{base_baseline} -> {baseline}"
            )

    for task_id in sorted(set(contexts) - set(base_contexts)):
        entry = contexts[task_id]
        if not isinstance(entry, dict):
            raise ValueError(f"{task_id}: context entry must be an object")
        if configured_baseline(entry, task_id) is not None:
            failures.append(f"{task_id}: new legacy context baseline added since base-ref")
    return failures


def evaluate_context_map(
    root: Path,
    config: dict[str, Any],
    manifest: dict[str, Any],
    *,
    strict_budget: bool = False,
) -> tuple[list[str], list[ContextMeasurement]]:
    failures: list[str] = []
    measurements: list[ContextMeasurement] = []
    if config.get("version") != 1:
        return ["AI context map version must be 1"], []
    if config.get("bootstrap_ref") != AI_CONTEXT_BOOTSTRAP_REF:
        failures.append(
            "AI context map bootstrap_ref must remain the reviewed commit "
            f"{AI_CONTEXT_BOOTSTRAP_REF}"
        )
    if config.get("policy") != CONTEXT_MAP_POLICY:
        failures.append("AI context map policy text does not match enforced policy")
    default_limit = config.get("default_max_context_bytes")
    if type(default_limit) is not int or default_limit <= 0:
        return ["default_max_context_bytes must be a positive integer"], []
    if default_limit > 250_000:
        failures.append("default_max_context_bytes must not exceed 250000")

    try:
        expected_tasks = manifest_task_ids(manifest)
        shared = validate_string_list(config.get("shared_paths"), "shared_paths")
    except ValueError as error:
        return [str(error)], []

    shared_sizes: dict[str, int] = {}
    shared_sources: dict[str, str] = {}
    for path in shared:
        try:
            canonical, size = validate_relative_file(root, path, "shared path")
            if canonical in shared_sizes:
                failures.append(
                    f"duplicate bundle path: {path} aliases {shared_sources[canonical]}"
                )
                continue
            shared_sizes[canonical] = size
            shared_sources[canonical] = path
        except ValueError as error:
            failures.append(str(error))

    contexts = config.get("contexts")
    if not isinstance(contexts, dict):
        return [*failures, "AI context map must contain a contexts object"], []
    try:
        migrations = reviewed_path_migrations(config)
    except ValueError as error:
        return [*failures, str(error)], []
    observed_tasks = set(contexts)
    for task_id in sorted(expected_tasks - observed_tasks):
        failures.append(f"missing context entry for manifest task {task_id}")
    for task_id in sorted(observed_tasks - expected_tasks):
        failures.append(f"context entry has no manifest task: {task_id}")

    for task_id in sorted(expected_tasks & observed_tasks):
        raw_entry = contexts[task_id]
        if not isinstance(raw_entry, dict):
            failures.append(f"{task_id}: context entry must be an object")
            continue
        raw_paths = raw_entry.get("paths")
        if not isinstance(raw_paths, dict):
            failures.append(f"{task_id}: paths must be an object")
            continue
        if set(raw_paths) != set(PATH_GROUPS):
            failures.append(
                f"{task_id}: paths must contain exactly {', '.join(PATH_GROUPS)}"
            )
            continue

        task_sizes: dict[str, int] = {}
        task_sources: dict[str, str] = {}
        declared_paths = set(shared)
        for group in PATH_GROUPS:
            try:
                paths = validate_string_list(raw_paths[group], f"{task_id}.{group}")
            except ValueError as error:
                failures.append(str(error))
                continue
            for path in paths:
                if path in declared_paths:
                    failures.append(f"{task_id}: duplicate bundle path: {path}")
                    continue
                declared_paths.add(path)
                try:
                    canonical, size = validate_relative_file(
                        root, path, f"{task_id}.{group}"
                    )
                    if canonical in shared_sizes:
                        failures.append(
                            f"{task_id}: duplicate bundle path: {path} aliases "
                            f"{shared_sources[canonical]}"
                        )
                        continue
                    if canonical in task_sizes:
                        failures.append(
                            f"{task_id}: duplicate bundle path: {path} aliases "
                            f"{task_sources[canonical]}"
                        )
                        continue
                    task_sizes[canonical] = size
                    task_sources[canonical] = path
                except ValueError as error:
                    failures.append(str(error))

        bundle_files = len(shared_sizes) + len(task_sizes)
        if bundle_files > 15:
            failures.append(f"{task_id}: context bundle has {bundle_files} files; maximum is 15")
        try:
            commands = validate_string_list(raw_entry.get("commands"), f"{task_id}.commands")
            if not commands:
                failures.append(f"{task_id}: commands must not be empty")
        except ValueError as error:
            failures.append(str(error))

        task_limit = raw_entry.get("max_context_bytes", default_limit)
        if (
            type(task_limit) is not int
            or task_limit <= 0
            or task_limit > default_limit
        ):
            failures.append(
                f"{task_id}: max_context_bytes must be positive and no greater than {default_limit}"
            )
            task_limit = default_limit
        baseline = raw_entry.get("baseline_context_bytes")
        reason = raw_entry.get("over_budget_reason")
        if baseline is not None and (
            type(baseline) is not int or baseline <= task_limit
        ):
            failures.append(
                f"{task_id}: baseline_context_bytes must be an integer above its target"
            )
            baseline = None
        if baseline is not None and (not isinstance(reason, str) or not reason.strip()):
            failures.append(f"{task_id}: legacy over-budget baseline requires a reason")
        if baseline is None and reason is not None:
            failures.append(f"{task_id}: over_budget_reason requires a baseline")

        total_bytes = sum(shared_sizes.values()) + sum(task_sizes.values())
        if total_bytes > task_limit:
            if strict_budget:
                failures.append(
                    f"{task_id}: context is {total_bytes} bytes, above target {task_limit}"
                )
            elif baseline is None or total_bytes > baseline:
                cap = baseline if baseline is not None else task_limit
                failures.append(
                    f"{task_id}: context is {total_bytes} bytes, above allowed cap {cap}"
                )
        measurements.append(
            ContextMeasurement(task_id, total_bytes, bundle_files, task_limit, baseline)
        )
    for (task_id, source), replacement in sorted(migrations.items()):
        entry = contexts.get(task_id)
        if not isinstance(entry, dict):
            failures.append(f"reviewed path migration has no context task: {task_id}")
            continue
        try:
            paths = flattened_context_paths(entry, task_id)
        except ValueError:
            continue
        if source in paths or replacement not in paths:
            failures.append(
                f"{task_id}: reviewed path migration must remove {source} and include "
                f"{replacement}"
            )
    return failures, measurements


def print_measurements(measurements: list[ContextMeasurement]) -> None:
    print("AI context bundles")
    print("status  bytes/target  files  task")
    for item in measurements:
        status = "legacy" if item.legacy_over_budget else "ok"
        print(
            f"{status:6}  {item.bytes:7}/{item.target:<7}  {item.files:5}  {item.task_id}"
        )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=REPO_ROOT)
    parser.add_argument("--config", type=Path, default=DEFAULT_CONFIG)
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--base-ref")
    parser.add_argument("--strict-budget", action="store_true")
    parser.add_argument("--print-commands", metavar="TASK_ID")
    args = parser.parse_args()
    root = args.root.resolve()
    config_path = args.config if args.config.is_absolute() else root / args.config
    manifest_path = args.manifest if args.manifest.is_absolute() else root / args.manifest
    try:
        config = load_json(config_path, "AI context map")
        manifest = load_json(manifest_path, "task manifest")
        validate_repository_manifest(manifest)
        failures, measurements = evaluate_context_map(
            root, config, manifest, strict_budget=args.strict_budget
        )
        if args.base_ref is not None:
            commit = resolve_commit(root, args.base_ref)
            config_relative = repository_relative_path(root, config_path, "config")
            manifest_relative = repository_relative_path(root, manifest_path, "manifest")
            comparison_commit, base_config, base_manifest = load_comparison_snapshot(
                root, commit, config_relative, manifest_relative
            )
            failures.extend(
                evaluate_base_ref_drift(config, manifest, base_config, base_manifest)
            )
            base_measurements = measure_context_map_at_ref(
                root, comparison_commit, base_config, base_manifest
            )
            failures.extend(evaluate_bundle_size_drift(measurements, base_measurements))
    except ValueError as error:
        print(f"AI context map: FAIL: {error}", file=sys.stderr)
        return 1

    if failures:
        print("AI context map: FAIL", file=sys.stderr)
        for failure in failures:
            print(f"- {failure}", file=sys.stderr)
        return 1

    if args.print_commands is not None:
        entry = config.get("contexts", {}).get(args.print_commands)
        if not isinstance(entry, dict):
            print(f"AI context map: unknown task {args.print_commands}", file=sys.stderr)
            return 1
        try:
            commands = validate_string_list(
                entry.get("commands"), f"{args.print_commands}.commands"
            )
        except ValueError as error:
            print(f"AI context map: FAIL: {error}", file=sys.stderr)
            return 1
        print("\n".join(commands))
        return 0

    print_measurements(measurements)
    print("AI context map: PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
