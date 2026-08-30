#!/usr/bin/env python3
"""Validate bounded, repository-local AI context bundles for refactoring tasks."""

from __future__ import annotations

import argparse
import json
import sys
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_CONFIG = REPO_ROOT / "config" / "ai-context-map.json"
DEFAULT_MANIFEST = REPO_ROOT / "config" / "refactoring" / "task-manifest.yaml"
PATH_GROUPS = ("documents", "entrypoints", "implementation", "tests")


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


def manifest_task_ids(manifest: dict[str, Any]) -> set[str]:
    if manifest.get("version") != 1:
        raise ValueError("task manifest version must be 1")
    tasks = manifest.get("tasks")
    if not isinstance(tasks, dict) or not tasks:
        raise ValueError("task manifest must contain a non-empty tasks object")
    if not all(isinstance(task_id, str) and task_id for task_id in tasks):
        raise ValueError("task manifest IDs must be non-empty strings")
    return set(tasks)


def validate_relative_file(root: Path, raw_path: object, label: str) -> tuple[str, int]:
    if not isinstance(raw_path, str) or not raw_path:
        raise ValueError(f"{label} must be a non-empty repository-relative path")
    if "\\" in raw_path:
        raise ValueError(f"{label} must use POSIX separators: {raw_path}")
    pure = PurePosixPath(raw_path)
    if pure.is_absolute() or ".." in pure.parts or pure.as_posix() != raw_path:
        raise ValueError(f"{label} is not a canonical repository-relative path: {raw_path}")
    candidate = root.joinpath(*pure.parts)
    try:
        resolved = candidate.resolve(strict=True)
        resolved.relative_to(root)
    except (OSError, ValueError) as error:
        raise ValueError(f"{label} is missing or escapes the repository: {raw_path}") from error
    if not resolved.is_file():
        raise ValueError(f"{label} is not a regular file: {raw_path}")
    return raw_path, resolved.stat().st_size


def validate_string_list(value: object, label: str) -> list[str]:
    if not isinstance(value, list) or not all(
        isinstance(item, str) and item.strip() for item in value
    ):
        raise ValueError(f"{label} must be an array of non-empty strings")
    if len(value) != len(set(value)):
        raise ValueError(f"{label} contains duplicates")
    return value


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
    default_limit = config.get("default_max_context_bytes")
    if not isinstance(default_limit, int) or default_limit <= 0:
        return ["default_max_context_bytes must be a positive integer"], []
    if default_limit > 250_000:
        failures.append("default_max_context_bytes must not exceed 250000")

    try:
        expected_tasks = manifest_task_ids(manifest)
        shared = validate_string_list(config.get("shared_paths"), "shared_paths")
    except ValueError as error:
        return [str(error)], []

    shared_sizes: dict[str, int] = {}
    for path in shared:
        try:
            canonical, size = validate_relative_file(root, path, "shared path")
            shared_sizes[canonical] = size
        except ValueError as error:
            failures.append(str(error))

    contexts = config.get("contexts")
    if not isinstance(contexts, dict):
        return [*failures, "AI context map must contain a contexts object"], []
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
        for group in PATH_GROUPS:
            try:
                paths = validate_string_list(raw_paths[group], f"{task_id}.{group}")
            except ValueError as error:
                failures.append(str(error))
                continue
            for path in paths:
                if path in shared_sizes or path in task_sizes:
                    failures.append(f"{task_id}: duplicate bundle path: {path}")
                    continue
                try:
                    canonical, size = validate_relative_file(
                        root, path, f"{task_id}.{group}"
                    )
                    task_sizes[canonical] = size
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
        if not isinstance(task_limit, int) or task_limit <= 0 or task_limit > default_limit:
            failures.append(
                f"{task_id}: max_context_bytes must be positive and no greater than {default_limit}"
            )
            task_limit = default_limit
        baseline = raw_entry.get("baseline_context_bytes")
        reason = raw_entry.get("over_budget_reason")
        if baseline is not None and (not isinstance(baseline, int) or baseline <= task_limit):
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
    parser.add_argument("--strict-budget", action="store_true")
    parser.add_argument("--print-commands", metavar="TASK_ID")
    args = parser.parse_args()
    root = args.root.resolve()
    config_path = args.config if args.config.is_absolute() else root / args.config
    manifest_path = args.manifest if args.manifest.is_absolute() else root / args.manifest
    try:
        config = load_json(config_path, "AI context map")
        manifest = load_json(manifest_path, "task manifest")
        failures, measurements = evaluate_context_map(
            root, config, manifest, strict_budget=args.strict_budget
        )
    except ValueError as error:
        print(f"AI context map: FAIL: {error}", file=sys.stderr)
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
        print("\n".join(sorted(commands)))
        return 0

    print_measurements(measurements)
    if failures:
        print("AI context map: FAIL", file=sys.stderr)
        for failure in failures:
            print(f"- {failure}", file=sys.stderr)
        return 1
    print("AI context map: PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
