#!/usr/bin/env python3
"""Enforce source-size ratchets and the lowest-level Cargo boundaries."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_CONFIG = REPO_ROOT / "config" / "source-size-baseline.json"
SOURCE_ROOTS = (
    "apps/lorepia/src",
    "apps/lorepia/src-tauri",
    "crates",
    "plugins",
)
SOURCE_SUFFIXES = {".css", ".kt", ".lua", ".rs", ".svelte", ".swift", ".ts"}
FORBIDDEN_ORCHESTRATION_DEPENDENCIES = {
    "diesel",
    "hyper",
    "lorepia-platform",
    "lorepia-providers",
    "lorepia-storage",
    "reqwest",
    "rusqlite",
    "sea-orm",
    "sqlx",
    "tauri",
    "tokio",
    "ureq",
}
DECLARATION_RE = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:fn|struct|enum|trait|type|class|interface|function)\s+",
    re.MULTILINE,
)
PUBLIC_SYMBOL_RE = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+|export\s+(?:default\s+)?)",
    re.MULTILINE,
)
FRONTEND_IMPORT_RE = re.compile(
    r'''(?:\bfrom\s*|\bimport\s*\(\s*|\brequire\s*\(\s*|\bimport\s*)["'`]([^"'`]+)["'`]'''
)
FRONTEND_GLOB_RE = re.compile(
    r"\bimport\s*\.\s*meta\s*\.\s*glob(?:Eager)?(?:\s*<[^;{}()]+>)?\s*\(\s*([^)]{0,16384})\)",
    re.DOTALL,
)
FRONTEND_QUOTED_RE = re.compile(r'''["'`]((?:\\.|[^"'`\\])*)["'`]''')
FRONTEND_INTERPOLATED_IMPORT_RE = re.compile(
    r"\bimport\s*\(\s*`((?:\\.|[^`\\])*)`\s*\)", re.DOTALL
)
FRONTEND_TEST_NAME_RE = re.compile(r"\.(?:test|spec)(?:\.[^.]+)?$")
PORTABLE_REGEX_OPERATION = Path(
    "apps/lorepia/src/features/chat/portable-regex-operation.ts"
)
PORTABLE_REGEX_WORKER = Path("apps/lorepia/src/features/chat/portable-regex.worker.ts")


@dataclass(frozen=True)
class SourceMeasurement:
    path: str
    bytes: int
    lines: int
    declarations: int
    public_symbols: int
    byte_limit: int
    line_limit: int
    kind: str
    failed: bool


def is_production_source(relative: Path) -> bool:
    """Return whether a path is a hand-authored production source file."""

    if relative.suffix not in SOURCE_SUFFIXES:
        return False
    if relative.name.endswith((".test.ts", ".spec.ts")):
        return False
    parts = relative.parts
    if parts[:4] == ("apps", "lorepia", "src", "tests"):
        return False
    if parts[:3] == ("apps", "lorepia", "src"):
        return True
    if parts[:4] == ("apps", "lorepia", "src-tauri", "src"):
        return True
    if parts == ("apps", "lorepia", "src-tauri", "build.rs"):
        return True
    if len(parts) >= 4 and parts[0] in {"crates", "plugins"} and parts[2] == "src":
        return True
    if relative.name == "build.rs" and len(parts) == 3 and parts[0] in {"crates", "plugins"}:
        return True
    if (
        len(parts) >= 6
        and parts[0] == "plugins"
        and parts[2:5] == ("android", "src", "main")
    ):
        return True
    return len(parts) >= 5 and parts[0] == "plugins" and parts[2:4] == ("ios", "Sources")


def production_sources(root: Path) -> list[Path]:
    candidates: set[Path] = set()
    for source_root in SOURCE_ROOTS:
        absolute = root / source_root
        if not absolute.is_dir():
            continue
        for candidate in absolute.rglob("*"):
            if candidate.is_file():
                relative = candidate.relative_to(root)
                if is_production_source(relative):
                    candidates.add(relative)
    return sorted(candidates)


def strip_frontend_comments(source: str) -> str:
    """Remove JavaScript comments while preserving quoted import specifiers."""

    result: list[str] = []
    index = 0
    quote: str | None = None
    while index < len(source):
        character = source[index]
        following = source[index + 1] if index + 1 < len(source) else ""
        if quote is not None:
            result.append(character)
            if character == "\\" and following != "":
                result.append(following)
                index += 2
                continue
            if character == quote:
                quote = None
            index += 1
            continue
        if character in {'"', "'", "`"}:
            quote = character
            result.append(character)
            index += 1
            continue
        if character == "/" and following == "/":
            index += 2
            while index < len(source) and source[index] not in "\r\n":
                index += 1
            continue
        if character == "/" and following == "*":
            index += 2
            while index + 1 < len(source) and source[index : index + 2] != "*/":
                if source[index] in "\r\n":
                    result.append(source[index])
                index += 1
            index = min(len(source), index + 2)
            result.append(" ")
            continue
        result.append(character)
        index += 1
    return "".join(result)


def decode_frontend_specifier(value: str) -> str:
    """Decode the escape forms accepted inside JavaScript import strings."""

    def replace_escape(match: re.Match[str]) -> str:
        token = match.group(1)
        try:
            if token.startswith("u{"):
                return chr(int(token[2:-1], 16))
            if token.startswith("u"):
                return chr(int(token[1:], 16))
            if token.startswith("x"):
                return chr(int(token[1:], 16))
        except (ValueError, OverflowError):
            return "/"
        if token in {"\n", "\r", "\r\n"}:
            return ""
        return token

    decoded = re.sub(
        r"\\(u\{[0-9a-fA-F]{1,6}\}|u[0-9a-fA-F]{4}|x[0-9a-fA-F]{2}|\r\n|[\s\S])",
        replace_escape,
        value,
    )
    return decoded.replace("\\", "/")


def frontend_import_specifiers(source: str) -> list[str]:
    without_comments = strip_frontend_comments(source)
    specifiers = [match.group(1) for match in FRONTEND_IMPORT_RE.finditer(without_comments)]
    for glob in FRONTEND_GLOB_RE.finditer(without_comments):
        specifiers.extend(match.group(1) for match in FRONTEND_QUOTED_RE.finditer(glob.group(1)))
    return specifiers


def frontend_interpolated_imports(source: str) -> list[str]:
    without_comments = strip_frontend_comments(source)
    return [
        match.group(1)
        for match in FRONTEND_INTERPOLATED_IMPORT_RE.finditer(without_comments)
        if "${" in match.group(1)
    ]


def evaluate_frontend_test_imports(root: Path, sources: list[Path]) -> list[str]:
    """Reject production imports that point into excluded frontend test sources."""

    frontend_root = (root / "apps" / "lorepia" / "src").resolve()
    failures: list[str] = []
    for relative in sources:
        if relative.parts[:3] != ("apps", "lorepia", "src"):
            continue
        try:
            text = (root / relative).read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError) as error:
            failures.append(f"cannot inspect frontend imports in {relative.as_posix()}: {error}")
            continue
        for specifier in frontend_interpolated_imports(text):
            failures.append(
                f"{relative.as_posix()} uses interpolated dynamic import that could reach "
                f"excluded test source: `{specifier}`"
            )
        for raw_specifier in frontend_import_specifiers(text):
            specifier = decode_frontend_specifier(raw_specifier)
            specifier = specifier.removeprefix("!").split("?", 1)[0].split("#", 1)[0]
            if specifier.startswith("$lib/"):
                target = frontend_root / "lib" / specifier.removeprefix("$lib/")
            elif specifier.startswith("/src/"):
                target = frontend_root / specifier.removeprefix("/src/")
            elif specifier.startswith("."):
                target = root / relative.parent / specifier
            else:
                continue
            try:
                test_relative = target.resolve().relative_to(frontend_root)
            except ValueError:
                continue
            if (
                (test_relative.parts and test_relative.parts[0] == "tests")
                or FRONTEND_TEST_NAME_RE.search(test_relative.name) is not None
            ):
                failures.append(
                    f"{relative.as_posix()} imports excluded test source: {specifier}"
                )
    return failures


def evaluate_portable_regex_boundary(root: Path, sources: list[Path]) -> list[str]:
    """Keep attacker-controlled JavaScript RegExp construction inside its Worker."""

    operation = (root / PORTABLE_REGEX_OPERATION).resolve().with_suffix("")
    failures: list[str] = []
    for relative in sources:
        if relative.parts[:3] != ("apps", "lorepia", "src"):
            continue
        try:
            text = (root / relative).read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        for raw_specifier in frontend_import_specifiers(text):
            specifier = decode_frontend_specifier(raw_specifier)
            specifier = specifier.removeprefix("!").split("?", 1)[0].split("#", 1)[0]
            if not specifier.startswith("."):
                continue
            target = (root / relative.parent / specifier).resolve().with_suffix("")
            if target == operation and relative != PORTABLE_REGEX_WORKER:
                failures.append(
                    f"{relative.as_posix()} imports the Worker-only portable regex evaluator"
                )
    return failures


def load_config(config_path: Path) -> dict[str, Any]:
    try:
        config = json.loads(config_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"cannot read {config_path}: {error}") from error
    if config.get("version") != 1:
        raise ValueError("source-size baseline version must be 1")
    limits = config.get("new_file_limits")
    baselines = config.get("baselines")
    if not isinstance(limits, dict) or not isinstance(baselines, dict):
        raise ValueError("source-size baseline must contain limits and baselines objects")
    for key in ("bytes", "lines"):
        if not isinstance(limits.get(key), int) or limits[key] <= 0:
            raise ValueError(f"new_file_limits.{key} must be a positive integer")
    return config


def evaluate_baseline_changes(
    current: dict[str, Any], base: dict[str, Any]
) -> list[str]:
    failures: list[str] = []
    current_limits = current.get("new_file_limits", {})
    base_limits = base.get("new_file_limits", {})
    for key in ("bytes", "lines"):
        current_value = current_limits.get(key)
        base_value = base_limits.get(key)
        if isinstance(current_value, int) and isinstance(base_value, int) and current_value > base_value:
            failures.append(
                f"new-file {key} limit increased from {base_value} to {current_value}"
            )

    current_baselines = current.get("baselines", {})
    base_baselines = base.get("baselines", {})
    if not isinstance(current_baselines, dict) or not isinstance(base_baselines, dict):
        return [*failures, "base and current baseline maps must be objects"]
    for path, current_limit in current_baselines.items():
        if path not in base_baselines:
            failures.append(f"new baseline exception is not allowed after bootstrap: {path}")
            continue
        base_limit = base_baselines[path]
        if not isinstance(current_limit, dict) or not isinstance(base_limit, dict):
            continue
        for key in ("bytes", "lines"):
            current_value = current_limit.get(key)
            base_value = base_limit.get(key)
            if (
                isinstance(current_value, int)
                and isinstance(base_value, int)
                and current_value > base_value
            ):
                failures.append(
                    f"{path} {key} baseline increased from {base_value} to {current_value}"
                )
    return failures


def load_base_config(root: Path, config_path: Path, base_ref: str) -> dict[str, Any] | None:
    try:
        relative_config = config_path.relative_to(root).as_posix()
    except ValueError as error:
        raise ValueError("baseline config must be inside the repository root") from error
    verify = subprocess.run(
        ["git", "rev-parse", "--verify", f"{base_ref}^{{commit}}"],
        cwd=root,
        check=False,
        capture_output=True,
        text=True,
    )
    if verify.returncode != 0:
        raise ValueError(f"baseline comparison ref is not a commit: {base_ref}")
    process = subprocess.run(
        ["git", "show", f"{base_ref}:{relative_config}"],
        cwd=root,
        check=False,
        capture_output=True,
        text=True,
    )
    if process.returncode != 0:
        return None
    try:
        parsed = json.loads(process.stdout)
    except json.JSONDecodeError as error:
        raise ValueError(f"base revision has an invalid source-size baseline: {error}") from error
    if not isinstance(parsed, dict):
        raise ValueError("base revision source-size baseline must be an object")
    return parsed


def measure_source(
    root: Path,
    relative: Path,
    *,
    byte_limit: int,
    line_limit: int,
    kind: str,
) -> SourceMeasurement:
    contents = (root / relative).read_bytes()
    text = contents.decode("utf-8")
    line_count = len(text.splitlines())
    byte_count = len(contents)
    return SourceMeasurement(
        path=relative.as_posix(),
        bytes=byte_count,
        lines=line_count,
        declarations=len(DECLARATION_RE.findall(text)),
        public_symbols=len(PUBLIC_SYMBOL_RE.findall(text)),
        byte_limit=byte_limit,
        line_limit=line_limit,
        kind=kind,
        failed=byte_count > byte_limit or line_count > line_limit,
    )


def evaluate_source_sizes(
    root: Path, config_path: Path
) -> tuple[list[str], list[SourceMeasurement]]:
    config = load_config(config_path)
    limits = config["new_file_limits"]
    baselines = config["baselines"]
    failures: list[str] = []
    measurements: list[SourceMeasurement] = []
    sources = production_sources(root)
    source_names = {source.as_posix() for source in sources}
    failures.extend(evaluate_frontend_test_imports(root, sources))
    failures.extend(evaluate_portable_regex_boundary(root, sources))

    for baseline_path, baseline in sorted(baselines.items()):
        if not isinstance(baseline_path, str) or not isinstance(baseline, dict):
            failures.append("baseline entries must map source paths to limit objects")
            continue
        if baseline_path not in source_names:
            failures.append(
                f"stale or non-production baseline entry: {baseline_path}; remove it explicitly"
            )
            continue
        if not all(isinstance(baseline.get(key), int) and baseline[key] > 0 for key in ("bytes", "lines")):
            failures.append(f"invalid baseline limits for {baseline_path}")

    for relative in sources:
        relative_name = relative.as_posix()
        baseline = baselines.get(relative_name)
        if isinstance(baseline, dict):
            byte_limit = baseline.get("bytes")
            line_limit = baseline.get("lines")
            if not isinstance(byte_limit, int) or not isinstance(line_limit, int):
                continue
            kind = "baseline"
        else:
            byte_limit = limits["bytes"]
            line_limit = limits["lines"]
            kind = "new"
        try:
            measurement = measure_source(
                root,
                relative,
                byte_limit=byte_limit,
                line_limit=line_limit,
                kind=kind,
            )
        except (OSError, UnicodeDecodeError) as error:
            failures.append(f"cannot inspect {relative_name}: {error}")
            continue
        measurements.append(measurement)
        if measurement.failed:
            if kind == "baseline":
                failures.append(
                    f"{relative_name} grew beyond its baseline "
                    f"({measurement.bytes}/{byte_limit} bytes, "
                    f"{measurement.lines}/{line_limit} lines)"
                )
            else:
                failures.append(
                    f"new production source exceeds the design-review limit: {relative_name} "
                    f"({measurement.bytes}/{byte_limit} bytes, "
                    f"{measurement.lines}/{line_limit} lines)"
                )

    shown = [measurement for measurement in measurements if measurement.kind == "baseline" or measurement.failed]
    return failures, shown


def cargo_metadata(root: Path) -> dict[str, Any]:
    process = subprocess.run(
        [
            "cargo",
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
            str(root / "Cargo.toml"),
        ],
        cwd=root,
        check=False,
        capture_output=True,
        text=True,
    )
    if process.returncode != 0:
        detail = process.stderr.strip() or process.stdout.strip() or "unknown cargo metadata error"
        raise ValueError(f"cargo metadata failed: {detail}")
    try:
        return json.loads(process.stdout)
    except json.JSONDecodeError as error:
        raise ValueError(f"cargo metadata returned invalid JSON: {error}") from error


def evaluate_dependency_architecture(metadata: dict[str, Any]) -> list[str]:
    packages = metadata.get("packages")
    workspace_members = set(metadata.get("workspace_members", []))
    if not isinstance(packages, list):
        return ["cargo metadata did not contain a package list"]
    workspace_packages = {
        package["name"]: package
        for package in packages
        if isinstance(package, dict)
        and package.get("id") in workspace_members
        and isinstance(package.get("name"), str)
    }
    failures: list[str] = []

    domain = workspace_packages.get("lorepia-domain")
    if domain is None:
        failures.append("cargo workspace is missing lorepia-domain")
    else:
        for dependency in domain.get("dependencies", []):
            name = dependency.get("name")
            if name in workspace_packages:
                failures.append(f"lorepia-domain must not depend on workspace crate {name}")

    orchestration = workspace_packages.get("lorepia-orchestration")
    if orchestration is None:
        failures.append("cargo workspace is missing lorepia-orchestration")
    else:
        for dependency in orchestration.get("dependencies", []):
            name = dependency.get("name")
            if name in FORBIDDEN_ORCHESTRATION_DEPENDENCIES:
                failures.append(
                    f"lorepia-orchestration must not directly depend on I/O boundary crate {name}"
                )
            if name in workspace_packages and name != "lorepia-domain":
                failures.append(
                    f"lorepia-orchestration may only depend on lorepia-domain below its layer; found {name}"
                )
    return failures


def print_source_table(measurements: list[SourceMeasurement]) -> None:
    print("source-size ratchet (all current baselines and any failures)")
    print("status  bytes/current-cap  lines/current-cap  decl  public  source")
    for item in measurements:
        status = "FAIL" if item.failed else "ok"
        print(
            f"{status:6}  {item.bytes:7}/{item.byte_limit:<7}  "
            f"{item.lines:6}/{item.line_limit:<6}  {item.declarations:4}  "
            f"{item.public_symbols:6}  {item.path}"
        )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=REPO_ROOT)
    parser.add_argument("--config", type=Path, default=DEFAULT_CONFIG)
    parser.add_argument(
        "--base-ref",
        help="Reject baseline cap increases relative to this trusted Git commit.",
    )
    parser.add_argument(
        "--skip-dependency-check",
        action="store_true",
        help="Only for isolated regression tests that do not contain a Cargo workspace.",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    root = args.root.resolve()
    config = args.config.resolve()
    try:
        failures, measurements = evaluate_source_sizes(root, config)
        if args.base_ref:
            base_config = load_base_config(root, config, args.base_ref)
            if base_config is not None:
                failures.extend(evaluate_baseline_changes(load_config(config), base_config))
        if not args.skip_dependency_check:
            failures.extend(evaluate_dependency_architecture(cargo_metadata(root)))
    except ValueError as error:
        print(f"source architecture: {error}", file=sys.stderr)
        return 1

    print_source_table(measurements)
    if failures:
        for failure in failures:
            print(f"source architecture: {failure}", file=sys.stderr)
        return 1
    print("source architecture: PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
