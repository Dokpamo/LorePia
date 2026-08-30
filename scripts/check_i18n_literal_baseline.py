#!/usr/bin/env python3
"""Keep inline Hangul debt frozen to exact, versioned line fingerprints."""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
from collections import Counter
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parents[1]
APP_ROOT = REPO_ROOT / "apps" / "lorepia"
SOURCE_ROOT = APP_ROOT / "src"
BASELINE_PATH = REPO_ROOT / "config" / "i18n-literal-baseline.json"
SOURCE_SUFFIXES = {".ts", ".svelte"}
EXCLUDED_PREFIXES = (Path("src/lib/i18n"), Path("src/preview"))


def contains_hangul(value: str) -> bool:
    return any("\uac00" <= character <= "\ud7a3" for character in value)


def is_excluded(relative: Path) -> bool:
    return any(relative == prefix or relative.is_relative_to(prefix) for prefix in EXCLUDED_PREFIXES)


def hangul_lines(value: str) -> list[str]:
    return [line.strip() for line in value.splitlines() if contains_hangul(line)]


def collect_worktree_lines() -> dict[str, list[str]]:
    collected: dict[str, list[str]] = {}
    for source in sorted(SOURCE_ROOT.rglob("*")):
        if not source.is_file() or source.suffix not in SOURCE_SUFFIXES:
            continue
        relative = source.relative_to(APP_ROOT)
        if is_excluded(relative):
            continue
        lines = hangul_lines(source.read_text(encoding="utf-8"))
        if lines:
            collected[relative.as_posix()] = lines
    return collected


def line_digest(lines: list[str]) -> str:
    digest = hashlib.sha256()
    for line in lines:
        encoded = line.encode("utf-8")
        digest.update(len(encoded).to_bytes(8, "big"))
        digest.update(encoded)
    return digest.hexdigest()


def baseline_payload(lines_by_file: dict[str, list[str]]) -> dict[str, Any]:
    return {
        "format_version": 1,
        "policy": "exact ordered fingerprints of non-i18n Hangul-containing source lines",
        "excluded_roots": [prefix.as_posix() for prefix in EXCLUDED_PREFIXES],
        "files": {
            path: {"hangul_lines": len(lines), "sha256": line_digest(lines)}
            for path, lines in sorted(lines_by_file.items())
        },
    }


def load_baseline(path: Path = BASELINE_PATH) -> dict[str, Any]:
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"cannot read {path}: {error}") from error
    if payload.get("format_version") != 1 or not isinstance(payload.get("files"), dict):
        raise ValueError(f"unsupported i18n baseline format in {path}")
    return payload


def compare_baseline(current: dict[str, Any], expected: dict[str, Any]) -> list[str]:
    failures: list[str] = []
    current_files = current["files"]
    expected_files = expected["files"]
    for path in sorted(set(current_files) | set(expected_files)):
        observed = current_files.get(path)
        allowed = expected_files.get(path)
        if observed == allowed:
            continue
        if allowed is None:
            failures.append(
                f"{path}: new inline Hangul debt ({observed['hangul_lines']} lines)"
            )
        elif observed is None:
            failures.append(
                f"{path}: inline Hangul was removed; shrink the baseline ({allowed['hangul_lines']} -> 0)"
            )
        else:
            failures.append(
                f"{path}: inline Hangul fingerprint changed "
                f"({allowed['hangul_lines']} -> {observed['hangul_lines']} lines)"
            )
    return failures


def git_show(base_ref: str, path: str) -> str | None:
    result = subprocess.run(
        ["git", "show", f"{base_ref}:{path}"],
        cwd=REPO_ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    return result.stdout if result.returncode == 0 else None


def collect_git_lines(base_ref: str) -> dict[str, list[str]]:
    listing = subprocess.run(
        ["git", "ls-tree", "-r", "--name-only", base_ref, "--", "apps/lorepia/src"],
        cwd=REPO_ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    if listing.returncode != 0:
        raise ValueError(f"cannot inspect i18n base ref {base_ref!r}")
    collected: dict[str, list[str]] = {}
    for repository_path in listing.stdout.splitlines():
        relative = Path(repository_path).relative_to("apps/lorepia")
        if relative.suffix not in SOURCE_SUFFIXES or is_excluded(relative):
            continue
        value = git_show(base_ref, repository_path)
        if value is None:
            continue
        lines = hangul_lines(value)
        if lines:
            collected[relative.as_posix()] = lines
    return collected


def compare_to_base(
    current: dict[str, list[str]], base: dict[str, list[str]]
) -> list[str]:
    """Reject new Hangul source lines while allowing exact path-only moves.

    Refactors may split a source file without changing any literal-bearing line.
    Consume the base revision's workspace-wide multiset deterministically so an
    existing line may move paths, but a duplicate or changed line still fails.
    """
    failures: list[str] = []
    remaining = Counter(
        line for path in sorted(base) for line in base[path]
    )
    for path, lines in sorted(current.items()):
        additions = 0
        for line in lines:
            if remaining[line] > 0:
                remaining[line] -= 1
            else:
                additions += 1
        if additions:
            failures.append(
                f"{path}: {additions} Hangul source line(s) are not in the base revision"
            )
    return failures


def debt_summary(payload: dict[str, Any]) -> tuple[int, int]:
    files = payload["files"]
    return sum(entry["hangul_lines"] for entry in files.values()), len(files)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-ref")
    parser.add_argument("--print-baseline", action="store_true")
    args = parser.parse_args()

    current_lines = collect_worktree_lines()
    current = baseline_payload(current_lines)
    if args.print_baseline:
        print(json.dumps(current, ensure_ascii=False, indent=2, sort_keys=True))
        return 0

    try:
        expected = load_baseline()
    except ValueError as error:
        print(f"i18n literal baseline: FAIL: {error}", file=sys.stderr)
        return 1
    failures = compare_baseline(current, expected)

    if args.base_ref:
        base_baseline = git_show(args.base_ref, "config/i18n-literal-baseline.json")
        if base_baseline is not None:
            try:
                json.loads(base_baseline)
                base_lines = collect_git_lines(args.base_ref)
                failures.extend(compare_to_base(current_lines, base_lines))
            except (json.JSONDecodeError, ValueError) as error:
                failures.append(str(error))
        else:
            print("i18n literal baseline: base has no baseline; accepting initial snapshot")

    lines, files = debt_summary(current)
    if failures:
        print("i18n literal baseline: FAIL", file=sys.stderr)
        for failure in failures:
            print(f"- {failure}", file=sys.stderr)
        print(f"remaining inline Hangul: {lines} lines across {files} files", file=sys.stderr)
        return 1
    print(f"i18n literal baseline: PASS ({lines} lines across {files} files)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
