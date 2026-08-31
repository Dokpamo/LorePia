#!/usr/bin/env python3
"""Enforce source-size ratchets and the lowest-level Cargo boundaries."""

from __future__ import annotations

import argparse
from bisect import bisect_right
import hashlib
import json
import re
import subprocess
import sys
from collections import Counter
from dataclasses import dataclass
from functools import lru_cache
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_CONFIG = REPO_ROOT / "config" / "source-size-baseline.json"
DEFAULT_TEST_CONFIG = REPO_ROOT / "config" / "test-source-size-baseline.json"
DEFAULT_CORE_STORAGE_API_CONFIG = (
    REPO_ROOT / "config" / "core-storage-public-api-baseline.json"
)
DEFAULT_DEPENDENCY_ARCHITECTURE_CONFIG = (
    REPO_ROOT / "config" / "refactoring" / "dependency-architecture.json"
)
ENF002_BOOTSTRAP_REF = "00a881131a04cdd998996a7ea03e5462ab72e16b"
SOURCE_ROOTS = (
    "apps/lorepia/src",
    "apps/lorepia/src-tauri",
    "crates",
    "plugins",
)
LANGUAGE_BY_SUFFIX = {
    ".css": "css",
    ".kt": "kotlin",
    ".lua": "lua",
    ".rs": "rust",
    ".svelte": "svelte",
    ".swift": "swift",
    ".ts": "typescript",
    ".tsx": "typescript",
}
SOURCE_SUFFIXES = set(LANGUAGE_BY_SUFFIX)
TEST_SOURCE_SUFFIXES = SOURCE_SUFFIXES - {".css", ".lua"}
SOURCE_KINDS = ("facade", "generated", "production")
TEST_SOURCE_KINDS = ("test",)
CONVENTIONAL_FACADE_NAMES = {"index.ts", "lib.rs", "mod.rs"}
V2_BOOTSTRAP_EDIT_PATHS = {
    ".github/workflows/ci.yml",
    "config/core-storage-public-api-baseline.json",
    "config/refactoring/dependency-architecture.json",
    "config/source-size-baseline.json",
    "config/test-source-size-baseline.json",
    "docs/architecture/storage-public-api-audit.md",
    "scripts/check_source_architecture.py",
    "scripts/test_check_source_architecture.py",
}
ENF002_BOOTSTRAP_EDIT_PATHS = {
    ".github/workflows/ci.yml",
    "config/core-storage-public-api-baseline.json",
    "config/refactoring/dependency-architecture.json",
    "docs/architecture/storage-public-api-audit.md",
    "scripts/check_source_architecture.py",
    "scripts/test_check_source_architecture.py",
}
LATER_ENFORCEMENT_EDIT_PATHS = {
    "config/ai-context-map.json",
    "scripts/check_ai_context_map.py",
    "scripts/check_github_workflow_security.py",
    "scripts/report_refactoring_baseline.py",
    "scripts/test_check_ai_context_map.py",
    "scripts/test_check_github_workflow_security.py",
    "scripts/test_report_refactoring_baseline.py",
}
ENFORCEMENT_EDIT_PREFIXES = {
    "config/refactoring/",
    "docs/refactoring/",
}
V2_BOOTSTRAP_EDIT_PATHS.update(LATER_ENFORCEMENT_EDIT_PATHS)
ENF002_BOOTSTRAP_EDIT_PATHS.update(LATER_ENFORCEMENT_EDIT_PATHS)
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
CORE_SOURCE_ROOT = Path("crates/core/src")
CHARACTER_RUNTIME_NATIVE_TRANSFORM_RE = re.compile(
    r"\.\s*runtime\s*\.\s*transform_set_id\b"
)
CHARACTER_RUNTIME_ALIAS_RE = re.compile(
    r"\blet\s+(?:mut\s+)?(?P<alias>[A-Za-z_][A-Za-z0-9_]*)\s*=\s*"
    r"[^;]{0,2048}\.\s*runtime\s*;",
    re.DOTALL,
)
CHARACTER_RUNTIME_TRANSFORM_DESTRUCTURE_RE = re.compile(
    r"\blet\s+(?:mut\s+)?(?:[A-Za-z_][A-Za-z0-9_]*\s*)?"
    r"\{[^{};]{0,2048}\btransform_set_id\b[^{};]{0,2048}\}\s*=\s*"
    r"[^;]{0,2048}\.\s*runtime\b",
    re.DOTALL,
)
STORED_TYPE_RE = re.compile(r"\bStored[A-Za-z0-9_]*\b")
RUST_RAW_STRING_RE = re.compile(r'(?:br|cr|r)(?P<hashes>#{0,255})"')
UNRESTRICTED_PUBLIC_RE = re.compile(r"\bpub\s+(?!\s*\()")
PUBLIC_API_CRATES = {
    "lorepia-core": Path("crates/core/src"),
    "lorepia-storage": Path("crates/storage/src"),
}
PUBLIC_API_WORKSPACE_SOURCE_ROOTS = {
    "lorepia_chat": Path("crates/chat/src"),
    "lorepia_content": Path("crates/content/src"),
    "lorepia_domain": Path("crates/domain/src"),
    "lorepia_orchestration": Path("crates/orchestration/src"),
    "lorepia_providers": Path("crates/providers/src"),
    "lorepia_storage": Path("crates/storage/src"),
}
PUBLIC_API_WILDCARD_TARGETS = {
    "lorepia_domain::discovery": Path("crates/domain/src/discovery.rs"),
    "lorepia_domain::orchestration": Path("crates/domain/src/orchestration.rs"),
}
CRATE_PUBLIC_INVENTORY_KEY = "__lorepia_crate_public_inventory__"
DEPENDENCY_RECORD_KINDS = {"build", "dev", "normal"}


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


@dataclass(frozen=True)
class SourceSize:
    bytes: int
    lines: int


@dataclass(frozen=True)
class AggregateDelta:
    path: str
    before_files: int
    after_files: int
    before_bytes: int
    after_bytes: int
    before_lines: int
    after_lines: int


@dataclass(frozen=True)
class SourceChange:
    before_path: Path | None
    before_size: SourceSize | None
    after_path: Path | None
    after_size: SourceSize | None


def source_language(relative: Path) -> str | None:
    return LANGUAGE_BY_SUFFIX.get(relative.suffix)


def is_generated_source(relative: Path) -> bool:
    parts = relative.parts
    return (
        parts[:4] == ("apps", "lorepia", "src-tauri", "gen")
        or "generated" in parts
        or ".generated." in relative.name
    )


def is_facade_source(relative: Path, facade_paths: set[str]) -> bool:
    return relative.name in CONVENTIONAL_FACADE_NAMES or relative.as_posix() in facade_paths


def is_production_source(relative: Path) -> bool:
    """Return whether a path is a hand-authored production source file."""

    if relative.suffix not in SOURCE_SUFFIXES:
        return False
    if is_generated_source(relative):
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


def is_test_source(relative: Path) -> bool:
    """Return whether a path is a hand-authored frontend, Rust, or native test."""

    if relative.suffix not in TEST_SOURCE_SUFFIXES:
        return False
    if is_generated_source(relative):
        return False
    parts = relative.parts
    if any(part in {".tauri", "node_modules", "target"} for part in parts):
        return False
    if parts[:3] == ("apps", "lorepia", "src"):
        return (
            (len(parts) >= 4 and parts[3] == "tests")
            or FRONTEND_TEST_NAME_RE.search(relative.name) is not None
        )
    if parts[:3] == ("apps", "lorepia", "src-tauri"):
        if len(parts) >= 4 and parts[3] == "gen":
            return False
        if relative.suffix != ".rs" or len(parts) < 5:
            return False
        if parts[3] == "tests":
            return True
        if parts[3] == "src":
            tail = parts[4:]
            return (
                "tests" in tail
                or relative.name == "tests.rs"
                or relative.name.endswith("_tests.rs")
            )
        return False
    if parts and parts[0] in {"crates", "plugins"} and len(parts) >= 4:
        if relative.suffix == ".rs":
            if parts[2] == "tests":
                return True
            if parts[2] == "src":
                tail = parts[3:]
                return (
                    "tests" in tail
                    or relative.name == "tests.rs"
                    or relative.name.endswith("_tests.rs")
                )
        if relative.suffix == ".kt":
            return any(
                parts[index] == "src" and parts[index + 1] in {"test", "androidTest"}
                for index in range(len(parts) - 1)
            )
        if relative.suffix == ".swift":
            return "Tests" in parts
    return False


def production_sources(root: Path, *, exclude_test_sources: bool = False) -> list[Path]:
    candidates: set[Path] = set()
    for source_root in SOURCE_ROOTS:
        absolute = root / source_root
        if not absolute.is_dir():
            continue
        for candidate in absolute.rglob("*"):
            if candidate.is_file():
                relative = candidate.relative_to(root)
                if is_production_source(relative) and not (
                    exclude_test_sources and is_test_source(relative)
                ):
                    candidates.add(relative)
    return sorted(candidates)


def test_sources(root: Path) -> list[Path]:
    candidates: set[Path] = set()
    for source_root in SOURCE_ROOTS:
        absolute = root / source_root
        if not absolute.is_dir():
            continue
        for candidate in absolute.rglob("*"):
            if candidate.is_file():
                relative = candidate.relative_to(root)
                if is_test_source(relative):
                    candidates.add(relative)
    return sorted(candidates)


def generated_sources(root: Path) -> list[Path]:
    candidates: set[Path] = set()
    for source_root in SOURCE_ROOTS:
        absolute = root / source_root
        if not absolute.is_dir():
            continue
        for candidate in absolute.rglob("*"):
            if candidate.is_file():
                relative = candidate.relative_to(root)
                if source_language(relative) is not None and is_generated_source(relative):
                    candidates.add(relative)
    return sorted(candidates)


def classify_source(relative: Path, facade_paths: set[str]) -> tuple[str, str] | None:
    language = source_language(relative)
    if language is None:
        return None
    if is_generated_source(relative):
        return "generated", language
    if is_test_source(relative):
        return "test", language
    if not is_production_source(relative):
        return None
    if is_facade_source(relative, facade_paths):
        return "facade", language
    return "production", language


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


def evaluate_character_runtime_transform_boundary(root: Path) -> list[str]:
    """Keep imported card transforms behind the frontend's session grant."""

    core_source_root = root / CORE_SOURCE_ROOT
    if not core_source_root.is_dir():
        return []
    failures: list[str] = []
    for source in sorted(core_source_root.rglob("*.rs")):
        relative = source.relative_to(root)
        try:
            stripped = strip_rust_comments_and_strings(source.read_text(encoding="utf-8"))
        except (OSError, UnicodeDecodeError) as error:
            failures.append(
                f"cannot inspect character runtime transform boundary in "
                f"{relative.as_posix()}: {error}"
            )
            continue
        consumes_native_projection = (
            "character_runtime_transform_set" in stripped
            or CHARACTER_RUNTIME_NATIVE_TRANSFORM_RE.search(stripped) is not None
            or CHARACTER_RUNTIME_TRANSFORM_DESTRUCTURE_RE.search(stripped) is not None
        )
        if not consumes_native_projection:
            for alias_match in CHARACTER_RUNTIME_ALIAS_RE.finditer(stripped):
                alias = re.escape(alias_match.group("alias"))
                if re.search(rf"\b{alias}\s*\.\s*transform_set_id\b", stripped):
                    consumes_native_projection = True
                    break
        if consumes_native_projection:
            failures.append(
                f"{relative.as_posix()} must not implicitly consume the "
                "character runtime native transform projection without a revision-bound grant"
            )
    return failures


def read_json_config(config_path: Path) -> dict[str, Any]:
    try:
        config = json.loads(config_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"cannot read {config_path}: {error}") from error
    if not isinstance(config, dict):
        raise ValueError(f"{config_path.name} must contain an object")
    return config


def validate_limit_table(
    config: dict[str, Any], *, kinds: tuple[str, ...], languages: set[str], label: str
) -> None:
    limits = config.get("limits")
    if not isinstance(limits, dict) or set(limits) != set(kinds):
        raise ValueError(f"{label}.limits must define exactly: {', '.join(kinds)}")
    for kind in kinds:
        language_limits = limits.get(kind)
        if not isinstance(language_limits, dict) or set(language_limits) != languages:
            raise ValueError(
                f"{label}.limits.{kind} must define exactly: {', '.join(sorted(languages))}"
            )
        for language, limit in language_limits.items():
            if not isinstance(limit, dict):
                raise ValueError(f"{label}.limits.{kind}.{language} must be an object")
            for key in ("bytes", "lines"):
                if not isinstance(limit.get(key), int) or limit[key] <= 0:
                    raise ValueError(
                        f"{label}.limits.{kind}.{language}.{key} must be a positive integer"
                    )


def validate_bootstrap_and_baselines(config: dict[str, Any], *, label: str) -> None:
    bootstrap_ref = config.get("bootstrap_ref")
    if not isinstance(bootstrap_ref, str) or re.fullmatch(r"[0-9a-f]{40}", bootstrap_ref) is None:
        raise ValueError(f"{label}.bootstrap_ref must be a full lowercase commit hash")
    baselines = config.get("baselines")
    if not isinstance(baselines, dict):
        raise ValueError(f"{label}.baselines must be an object")


def validate_parent_child_groups(config: dict[str, Any]) -> None:
    groups = config.get("parent_child_groups")
    if not isinstance(groups, dict):
        raise ValueError("source-size baseline.parent_child_groups must be an object")
    if list(groups) != sorted(groups):
        raise ValueError("source-size baseline.parent_child_groups keys must be sorted")
    for parent, child_entries in groups.items():
        parent_path = Path(parent) if isinstance(parent, str) else Path()
        if (
            not isinstance(parent, str)
            or parent_path.is_absolute()
            or ".." in parent_path.parts
            or not is_production_source(parent_path)
        ):
            raise ValueError(f"invalid parent source path: {parent}")
        if not isinstance(child_entries, list) or not all(
            isinstance(entry, str) for entry in child_entries
        ):
            raise ValueError(f"parent-child group for {parent} must be a string array")
        if not child_entries or child_entries != sorted(set(child_entries)):
            raise ValueError(
                f"parent-child entries for {parent} must be non-empty, unique, and sorted"
            )
        for entry in child_entries:
            relative = Path(entry[:-1] if entry.endswith("/") else entry)
            under_source_root = any(
                entry.startswith(f"{source_root}/") for source_root in SOURCE_ROOTS
            )
            if relative.is_absolute() or ".." in relative.parts or not under_source_root:
                raise ValueError(f"invalid child source entry: {entry}")
            if entry.endswith("/"):
                continue
            if entry == parent or source_language(relative) is None:
                raise ValueError(f"invalid exact child source path: {entry}")


def load_config(config_path: Path) -> dict[str, Any]:
    config = read_json_config(config_path)
    if config.get("version") != 2:
        raise ValueError("source-size baseline version must be 2")
    validate_limit_table(
        config,
        kinds=SOURCE_KINDS,
        languages=set(LANGUAGE_BY_SUFFIX.values()),
        label="source-size baseline",
    )
    validate_bootstrap_and_baselines(config, label="source-size baseline")
    facade_paths = config.get("facade_paths")
    if not isinstance(facade_paths, list) or not all(
        isinstance(path, str) for path in facade_paths
    ):
        raise ValueError("source-size baseline.facade_paths must be a string array")
    if facade_paths != sorted(set(facade_paths)):
        raise ValueError("source-size baseline.facade_paths must be unique and sorted")
    for path in facade_paths:
        relative = Path(path)
        if relative.is_absolute() or ".." in relative.parts or source_language(relative) is None:
            raise ValueError(f"invalid facade path: {path}")
    validate_parent_child_groups(config)
    return config


def load_test_config(config_path: Path) -> dict[str, Any]:
    config = read_json_config(config_path)
    if config.get("version") != 2:
        raise ValueError("test-source-size baseline version must be 2")
    validate_limit_table(
        config,
        kinds=TEST_SOURCE_KINDS,
        languages={LANGUAGE_BY_SUFFIX[suffix] for suffix in TEST_SOURCE_SUFFIXES},
        label="test-source-size baseline",
    )
    validate_bootstrap_and_baselines(config, label="test-source-size baseline")
    return config


def load_core_storage_api_config(config_path: Path) -> dict[str, Any]:
    try:
        config = json.loads(config_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"cannot read {config_path}: {error}") from error
    return validate_core_storage_api_config(config)


def validate_core_storage_api_config(config: object) -> dict[str, Any]:
    if not isinstance(config, dict) or config.get("version") not in {1, 2}:
        raise ValueError("core-storage public API baseline version must be 1 or 2")
    allowed = config.get("allowed_stored_reexports")
    if not isinstance(allowed, list) or not all(isinstance(name, str) for name in allowed):
        raise ValueError("allowed_stored_reexports must be a string array")
    if any(STORED_TYPE_RE.fullmatch(name) is None for name in allowed):
        raise ValueError("allowed_stored_reexports may contain only Stored* type names")
    if allowed != sorted(set(allowed)):
        raise ValueError("allowed_stored_reexports must be unique and sorted")
    if config["version"] == 1:
        if set(config) != {"version", "allowed_stored_reexports"}:
            raise ValueError("version 1 core-storage public API baseline has unknown fields")
        return config

    required = {
        "allowed_stored_reexports",
        "bootstrap_ref",
        "legacy_wildcard_reexports",
        "public_surface",
        "version",
    }
    if set(config) != required:
        raise ValueError("version 2 core-storage public API baseline fields are invalid")
    bootstrap_ref = config.get("bootstrap_ref")
    if not isinstance(bootstrap_ref, str) or re.fullmatch(
        r"[0-9a-f]{40}", bootstrap_ref
    ) is None:
        raise ValueError("core-storage public API bootstrap_ref must be a commit hash")
    public_surface = config.get("public_surface")
    if not isinstance(public_surface, dict) or set(public_surface) != set(
        PUBLIC_API_CRATES
    ):
        raise ValueError("public_surface must define lorepia-core and lorepia-storage")
    for crate_name, anchors in public_surface.items():
        if not isinstance(anchors, list) or not all(
            isinstance(anchor, str) and anchor and "\n" not in anchor
            for anchor in anchors
        ):
            raise ValueError(f"public_surface.{crate_name} must be a string array")
        if anchors != sorted(set(anchors)):
            raise ValueError(f"public_surface.{crate_name} must be unique and sorted")
    wildcards = config.get("legacy_wildcard_reexports")
    if not isinstance(wildcards, list) or not all(
        isinstance(anchor, str) and anchor.startswith("wildcard:")
        for anchor in wildcards
    ):
        raise ValueError("legacy_wildcard_reexports must contain wildcard anchors")
    if wildcards != sorted(set(wildcards)):
        raise ValueError("legacy_wildcard_reexports must be unique and sorted")
    return config


def dependency_record_key(record: dict[str, Any], *, workspace: bool) -> tuple[Any, ...]:
    target_key = "to" if workspace else "package"
    source = (record["requirement"],) if workspace else (
        record["source"],
        record["requirement"],
    )
    return (
        record["from"],
        record[target_key],
        *source,
        record["kind"],
        record["target"] or "",
        record["optional"],
        record["default_features"],
        record["rename"] or "",
        tuple(record["features"]),
    )


def validate_dependency_record(
    record: object, *, workspace: bool, label: str
) -> dict[str, Any]:
    target_key = "to" if workspace else "package"
    required = {
        "default_features",
        "features",
        "from",
        "kind",
        "optional",
        "requirement",
        "rename",
        "target",
        target_key,
    }
    if not workspace:
        required.add("source")
    if not isinstance(record, dict) or set(record) != required:
        raise ValueError(f"{label} has invalid fields")
    for key in ("from", target_key):
        if not isinstance(record[key], str) or not record[key]:
            raise ValueError(f"{label}.{key} must be a non-empty string")
    if record["kind"] not in DEPENDENCY_RECORD_KINDS:
        raise ValueError(f"{label}.kind must be normal, dev, or build")
    if record["target"] is not None and not isinstance(record["target"], str):
        raise ValueError(f"{label}.target must be null or a string")
    if record["rename"] is not None and not isinstance(record["rename"], str):
        raise ValueError(f"{label}.rename must be null or a string")
    for key in ("optional", "default_features"):
        if not isinstance(record[key], bool):
            raise ValueError(f"{label}.{key} must be a boolean")
    features = record["features"]
    if not isinstance(features, list) or not all(
        isinstance(feature, str) and feature for feature in features
    ):
        raise ValueError(f"{label}.features must be a string array")
    if features != sorted(set(features)):
        raise ValueError(f"{label}.features must be unique and sorted")
    if not isinstance(record["requirement"], str) or not record["requirement"]:
        raise ValueError(f"{label}.requirement must be a non-empty string")
    if not workspace:
        if not isinstance(record["source"], str) or not record["source"]:
            raise ValueError(f"{label}.source must be a non-empty string")
    return record


def validate_dependency_architecture_config(config: object) -> dict[str, Any]:
    required = {
        "bootstrap_ref",
        "direct_external_dependencies",
        "package_features",
        "version",
        "workspace_dependencies",
        "workspace_packages",
    }
    if not isinstance(config, dict) or set(config) != required or config.get("version") != 1:
        raise ValueError("dependency architecture config fields or version are invalid")
    bootstrap_ref = config.get("bootstrap_ref")
    if not isinstance(bootstrap_ref, str) or re.fullmatch(
        r"[0-9a-f]{40}", bootstrap_ref
    ) is None:
        raise ValueError("dependency architecture bootstrap_ref must be a commit hash")

    packages = config.get("workspace_packages")
    if not isinstance(packages, list):
        raise ValueError("workspace_packages must be an array")
    package_keys: list[tuple[str, str]] = []
    for index, package in enumerate(packages):
        if not isinstance(package, dict) or set(package) != {"manifest", "name"}:
            raise ValueError(f"workspace_packages[{index}] has invalid fields")
        name = package.get("name")
        manifest = package.get("manifest")
        if not isinstance(name, str) or not name:
            raise ValueError(f"workspace_packages[{index}].name is invalid")
        if (
            not isinstance(manifest, str)
            or not manifest.endswith("/Cargo.toml")
            or Path(manifest).is_absolute()
            or ".." in Path(manifest).parts
            or Path(manifest).as_posix() != manifest
        ):
            raise ValueError(f"workspace_packages[{index}].manifest is invalid")
        package_keys.append((name, manifest))
    if package_keys != sorted(set(package_keys)):
        raise ValueError("workspace_packages must be unique and sorted")
    if len({name for name, _ in package_keys}) != len(package_keys) or len(
        {manifest for _, manifest in package_keys}
    ) != len(package_keys):
        raise ValueError("workspace package names and manifests must be unique")

    package_names = {name for name, _ in package_keys}
    features = config.get("package_features")
    if not isinstance(features, dict) or list(features) != sorted(features):
        raise ValueError("package_features must be an object with sorted package keys")
    if not set(features).issubset(package_names):
        raise ValueError("package_features contains an unknown workspace package")
    for package_name, feature_map in features.items():
        if not isinstance(feature_map, dict) or list(feature_map) != sorted(feature_map):
            raise ValueError(f"package_features.{package_name} keys must be sorted")
        for feature_name, activations in feature_map.items():
            if not isinstance(feature_name, str) or not feature_name:
                raise ValueError(f"package_features.{package_name} has an invalid feature")
            if not isinstance(activations, list) or not all(
                isinstance(activation, str) and activation for activation in activations
            ):
                raise ValueError(
                    f"package_features.{package_name}.{feature_name} must be a string array"
                )
            if activations != sorted(set(activations)):
                raise ValueError(
                    f"package_features.{package_name}.{feature_name} must be sorted and unique"
                )

    for field, workspace in (
        ("workspace_dependencies", True),
        ("direct_external_dependencies", False),
    ):
        records = config.get(field)
        if not isinstance(records, list):
            raise ValueError(f"{field} must be an array")
        validated = [
            validate_dependency_record(record, workspace=workspace, label=f"{field}[{index}]")
            for index, record in enumerate(records)
        ]
        keys = [dependency_record_key(record, workspace=workspace) for record in validated]
        if keys != sorted(set(keys)):
            raise ValueError(f"{field} must be unique and sorted")
        for record in validated:
            if record["from"] not in package_names:
                raise ValueError(f"{field} contains an unknown source package")
            if workspace and record["to"] not in package_names:
                raise ValueError(f"{field} contains an unknown target package")
    return config


def load_dependency_architecture_config(config_path: Path) -> dict[str, Any]:
    try:
        config = json.loads(config_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"cannot read {config_path}: {error}") from error
    return validate_dependency_architecture_config(config)


def normalized_limits(
    config: dict[str, Any], *, test_config: bool
) -> dict[tuple[str, str, str], int]:
    version = config.get("version")
    kinds = TEST_SOURCE_KINDS if test_config else SOURCE_KINDS
    languages = (
        {LANGUAGE_BY_SUFFIX[suffix] for suffix in TEST_SOURCE_SUFFIXES}
        if test_config
        else set(LANGUAGE_BY_SUFFIX.values())
    )
    if version == 2:
        limits = config.get("limits", {})
        return {
            (kind, language, key): limits.get(kind, {}).get(language, {}).get(key)
            for kind in kinds
            for language in languages
            for key in ("bytes", "lines")
        }
    legacy_key = "new_test_file_limits" if test_config else "new_file_limits"
    legacy = config.get(legacy_key, {})
    return {
        (kind, language, key): legacy.get(key)
        for kind in kinds
        for language in languages
        for key in ("bytes", "lines")
    }


def evaluate_size_config_changes(
    current: dict[str, Any],
    base: dict[str, Any],
    *,
    test_config: bool,
    bootstrap: dict[str, Any] | None = None,
) -> list[str]:
    failures: list[str] = []
    current_version = current.get("version")
    base_version = base.get("version")
    if current_version not in {1, 2} or base_version not in {1, 2}:
        return ["source-size comparison supports only versions 1 and 2"]
    if current_version < base_version:
        failures.append(
            f"source-size baseline version regressed from {base_version} to {current_version}"
        )

    if current_version == 1 and base_version == 1:
        legacy_key = "new_test_file_limits" if test_config else "new_file_limits"
        for key in ("bytes", "lines"):
            current_value = current.get(legacy_key, {}).get(key)
            base_value = base.get(legacy_key, {}).get(key)
            if (
                isinstance(current_value, int)
                and isinstance(base_value, int)
                and current_value > base_value
            ):
                failures.append(
                    f"legacy {key} limit increased from {base_value} to {current_value}"
                )
    else:
        current_limits = normalized_limits(current, test_config=test_config)
        base_limits = normalized_limits(base, test_config=test_config)
        for identity, current_value in sorted(current_limits.items()):
            base_value = base_limits.get(identity)
            if (
                isinstance(current_value, int)
                and isinstance(base_value, int)
                and current_value > base_value
            ):
                kind, language, key = identity
                failures.append(
                    f"{kind}.{language} {key} limit increased from "
                    f"{base_value} to {current_value}"
                )

    if current_version == 2 and base_version == 2:
        if current.get("bootstrap_ref") != base.get("bootstrap_ref"):
            failures.append("source-size bootstrap_ref is immutable after v2 bootstrap")
        if not test_config:
            current_facades = set(current.get("facade_paths", []))
            base_facades = set(base.get("facade_paths", []))
            for path in sorted(base_facades - current_facades):
                failures.append(f"facade classification cannot be removed: {path}")
            current_groups = current.get("parent_child_groups", {})
            base_groups = base.get("parent_child_groups", {})
            if isinstance(current_groups, dict) and isinstance(base_groups, dict):
                for parent, base_entries in sorted(base_groups.items()):
                    current_entries = current_groups.get(parent)
                    if not isinstance(current_entries, list):
                        failures.append(
                            f"parent-child aggregate group cannot be removed: {parent}"
                        )
                        continue
                    for entry in sorted(set(base_entries) - set(current_entries)):
                        failures.append(
                            f"parent-child aggregate entry cannot be removed: "
                            f"{parent} -> {entry}"
                        )

    current_baselines = current.get("baselines", {})
    baseline_authority = (
        bootstrap
        if current_version == 2 and base_version == 1 and bootstrap is not None
        else base
    )
    base_baselines = baseline_authority.get("baselines", {})
    if not isinstance(current_baselines, dict) or not isinstance(base_baselines, dict):
        return [*failures, "base and current baseline maps must be objects"]
    for path, current_limit in current_baselines.items():
        if path not in base_baselines:
            prefix = "test " if test_config else ""
            failures.append(
                f"new {prefix}baseline exception is not allowed after bootstrap: {path}"
            )
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
                qualifier = " test" if test_config else ""
                failures.append(
                    f"{path} {key}{qualifier} baseline increased from "
                    f"{base_value} to {current_value}"
                )
    return failures


def evaluate_baseline_changes(
    current: dict[str, Any], base: dict[str, Any], bootstrap: dict[str, Any] | None = None
) -> list[str]:
    return evaluate_size_config_changes(
        current, base, test_config=False, bootstrap=bootstrap
    )


def evaluate_test_baseline_changes(
    current: dict[str, Any], base: dict[str, Any], bootstrap: dict[str, Any] | None = None
) -> list[str]:
    return evaluate_size_config_changes(
        current, base, test_config=True, bootstrap=bootstrap
    )


def evaluate_core_storage_api_baseline_changes(
    current: dict[str, Any], base: dict[str, Any]
) -> list[str]:
    current = validate_core_storage_api_config(current)
    base = validate_core_storage_api_config(base)
    current_allowed = set(current["allowed_stored_reexports"])
    base_allowed = set(base["allowed_stored_reexports"])
    additions = sorted(current_allowed - base_allowed)
    failures = [
        f"new Core storage Stored* re-export exception is not allowed: {name}"
        for name in additions
    ]
    if current["version"] < base["version"]:
        failures.append(
            f"core-storage public API baseline version regressed from "
            f"{base['version']} to {current['version']}"
        )
        return failures
    if current["version"] != 2 or base["version"] != 2:
        return failures
    if current["bootstrap_ref"] != base["bootstrap_ref"]:
        failures.append("core-storage public API bootstrap_ref is immutable")
    for crate_name in sorted(PUBLIC_API_CRATES):
        added = Counter(current["public_surface"][crate_name]) - Counter(
            base["public_surface"][crate_name]
        )
        for anchor in sorted(added):
            failures.append(
                f"new {crate_name} public API baseline anchor is not allowed: {anchor}"
            )
    wildcard_additions = sorted(
        set(current["legacy_wildcard_reexports"])
        - set(base["legacy_wildcard_reexports"])
    )
    failures.extend(
        f"new legacy public wildcard exception is not allowed: {anchor}"
        for anchor in wildcard_additions
    )
    return failures


def flatten_package_features(config: dict[str, Any]) -> set[tuple[str, str, str]]:
    return {
        (package_name, feature_name, activation)
        for package_name, feature_map in config["package_features"].items()
        for feature_name, activations in feature_map.items()
        for activation in activations
    } | {
        (package_name, feature_name, "")
        for package_name, feature_map in config["package_features"].items()
        for feature_name, activations in feature_map.items()
        if not activations
    }


def evaluate_dependency_policy_changes(
    current: dict[str, Any], base: dict[str, Any]
) -> list[str]:
    current = validate_dependency_architecture_config(current)
    base = validate_dependency_architecture_config(base)
    failures: list[str] = []
    if current["bootstrap_ref"] != base["bootstrap_ref"]:
        failures.append("dependency architecture bootstrap_ref is immutable")

    current_packages = {
        (package["name"], package["manifest"])
        for package in current["workspace_packages"]
    }
    base_packages = {
        (package["name"], package["manifest"])
        for package in base["workspace_packages"]
    }
    for name, manifest in sorted(current_packages - base_packages):
        failures.append(
            f"new workspace package policy entry is not allowed: {name} ({manifest})"
        )

    for field, workspace in (
        ("workspace_dependencies", True),
        ("direct_external_dependencies", False),
    ):
        current_records = {
            dependency_record_key(record, workspace=workspace)
            for record in current[field]
        }
        base_records = {
            dependency_record_key(record, workspace=workspace)
            for record in base[field]
        }
        for record in sorted(current_records - base_records):
            failures.append(
                f"new dependency policy entry is not allowed in {field}: {record}"
            )

    for package_name, feature_name, activation in sorted(
        flatten_package_features(current) - flatten_package_features(base)
    ):
        rendered = activation or "<empty>"
        failures.append(
            f"new package feature activation is not allowed: "
            f"{package_name}/{feature_name} -> {rendered}"
        )
    return failures


def load_json_at_ref(root: Path, config_path: Path, base_ref: str) -> object | None:
    try:
        relative_config = config_path.relative_to(root).as_posix()
    except ValueError as error:
        raise ValueError("architecture config must be inside the repository root") from error
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
        raise ValueError(f"base revision has invalid architecture JSON: {error}") from error
    return parsed


def require_commit(root: Path, ref: str, *, label: str) -> None:
    process = subprocess.run(
        ["git", "rev-parse", "--verify", f"{ref}^{{commit}}"],
        cwd=root,
        check=False,
        capture_output=True,
        text=True,
    )
    if process.returncode != 0:
        raise ValueError(f"{label} is not a commit: {ref}")


def enforcement_changed_paths(root: Path, bootstrap_ref: str) -> set[str]:
    changed = subprocess.run(
        ["git", "diff", "--name-only", bootstrap_ref, "--"],
        cwd=root,
        check=False,
        capture_output=True,
        text=True,
    )
    if changed.returncode != 0:
        raise ValueError(f"cannot inspect bootstrap changes: {changed.stderr.strip()}")
    untracked = subprocess.run(
        ["git", "ls-files", "--others", "--exclude-standard"],
        cwd=root,
        check=False,
        capture_output=True,
        text=True,
    )
    if untracked.returncode != 0:
        raise ValueError(f"cannot inspect untracked bootstrap paths: {untracked.stderr.strip()}")
    return set(changed.stdout.splitlines()) | set(untracked.stdout.splitlines())


def unexpected_enforcement_paths(
    paths: set[str], allowed_paths: set[str]
) -> list[str]:
    return sorted(
        path
        for path in paths
        if path not in allowed_paths
        and not any(path.startswith(prefix) for prefix in ENFORCEMENT_EDIT_PREFIXES)
    )


def require_v2_bootstrap_transition(root: Path, bootstrap_ref: str) -> None:
    require_commit(root, bootstrap_ref, label="source-size bootstrap_ref")
    ancestor = subprocess.run(
        ["git", "merge-base", "--is-ancestor", bootstrap_ref, "HEAD"],
        cwd=root,
        check=False,
        capture_output=True,
        text=True,
    )
    if ancestor.returncode != 0:
        raise ValueError("source-size bootstrap_ref must be an ancestor of HEAD")
    unexpected = unexpected_enforcement_paths(
        enforcement_changed_paths(root, bootstrap_ref), V2_BOOTSTRAP_EDIT_PATHS
    )
    if unexpected:
        raise ValueError(
            "v2 bootstrap must be based on the exact pre-enforcement tree; "
            f"unexpected changed path: {unexpected[0]}"
        )


def require_enf002_bootstrap_transition(root: Path, bootstrap_ref: str) -> None:
    require_commit(root, bootstrap_ref, label="ENF-002 bootstrap_ref")
    api_at_ref = load_json_at_ref(
        root,
        root / "config" / "core-storage-public-api-baseline.json",
        bootstrap_ref,
    )
    if not isinstance(api_at_ref, dict) or api_at_ref.get("version") != 1:
        raise ValueError("ENF-002 bootstrap_ref must identify the version 1 API policy tree")
    dependency_at_ref = load_json_at_ref(
        root,
        root / "config" / "refactoring" / "dependency-architecture.json",
        bootstrap_ref,
    )
    if dependency_at_ref is not None:
        raise ValueError("ENF-002 bootstrap_ref must predate the dependency policy")
    ancestor = subprocess.run(
        ["git", "merge-base", "--is-ancestor", bootstrap_ref, "HEAD"],
        cwd=root,
        check=False,
        capture_output=True,
        text=True,
    )
    if ancestor.returncode != 0:
        raise ValueError("ENF-002 bootstrap_ref must be an ancestor of HEAD")
    unexpected = unexpected_enforcement_paths(
        enforcement_changed_paths(root, bootstrap_ref), ENF002_BOOTSTRAP_EDIT_PATHS
    )
    if unexpected:
        raise ValueError(
            "ENF-002 bootstrap must be based on the exact pre-enforcement tree; "
            f"unexpected changed path: {unexpected[0]}"
        )


def load_base_config(root: Path, config_path: Path, base_ref: str) -> dict[str, Any] | None:
    parsed = load_json_at_ref(root, config_path, base_ref)
    if parsed is None:
        return None
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


def source_size(contents: bytes) -> SourceSize:
    return SourceSize(bytes=len(contents), lines=len(contents.decode("utf-8").splitlines()))


def source_size_at_ref(root: Path, ref: str, relative: Path) -> SourceSize | None:
    process = subprocess.run(
        ["git", "show", f"{ref}:{relative.as_posix()}"],
        cwd=root,
        check=False,
        capture_output=True,
    )
    if process.returncode != 0:
        return None
    try:
        return source_size(process.stdout)
    except UnicodeDecodeError:
        return None


def effective_limits(
    root: Path,
    relative: Path,
    *,
    config: dict[str, Any],
    kind: str,
    language: str,
    current_size: SourceSize,
    base_ref: str | None,
) -> tuple[int, int, str]:
    baseline = config["baselines"].get(relative.as_posix())
    if isinstance(baseline, dict) and all(
        isinstance(baseline.get(key), int) and baseline[key] > 0
        for key in ("bytes", "lines")
    ):
        return baseline["bytes"], baseline["lines"], "baseline"

    hard_limit = config["limits"][kind][language]
    byte_limit = hard_limit["bytes"]
    line_limit = hard_limit["lines"]
    if current_size.bytes <= byte_limit and current_size.lines <= line_limit:
        return byte_limit, line_limit, f"{kind}:{language}"

    bootstrap_size = source_size_at_ref(root, config["bootstrap_ref"], relative)
    if bootstrap_size is None:
        return byte_limit, line_limit, f"{kind}:{language}"
    byte_limit = max(byte_limit, bootstrap_size.bytes)
    line_limit = max(line_limit, bootstrap_size.lines)

    if base_ref is not None:
        base_size = source_size_at_ref(root, base_ref, relative)
        if base_size is not None:
            byte_limit = max(hard_limit["bytes"], min(byte_limit, base_size.bytes))
            line_limit = max(hard_limit["lines"], min(line_limit, base_size.lines))
    return byte_limit, line_limit, f"legacy-{kind}:{language}"


def evaluate_source_sizes(
    root: Path, config_path: Path, *, base_ref: str | None = None
) -> tuple[list[str], list[SourceMeasurement]]:
    config = load_config(config_path)
    baselines = config["baselines"]
    facade_paths = set(config["facade_paths"])
    failures: list[str] = []
    measurements: list[SourceMeasurement] = []
    hand_authored_sources = production_sources(root, exclude_test_sources=True)
    sources = sorted({*hand_authored_sources, *generated_sources(root)})
    source_names = {source.as_posix() for source in sources}
    all_source_names = {
        source.as_posix() for source in {*sources, *test_sources(root)}
    }
    failures.extend(evaluate_frontend_test_imports(root, hand_authored_sources))
    failures.extend(evaluate_portable_regex_boundary(root, hand_authored_sources))
    failures.extend(evaluate_character_runtime_transform_boundary(root))

    for facade_path in sorted(facade_paths - source_names):
        failures.append(f"stale or non-production facade path: {facade_path}")

    for parent, child_entries in config["parent_child_groups"].items():
        if parent not in source_names:
            failures.append(f"stale or non-production parent-child parent: {parent}")
        for entry in child_entries:
            if entry.endswith("/"):
                found = any(
                    path != parent and path.startswith(entry)
                    for path in all_source_names
                )
            else:
                found = entry != parent and entry in all_source_names
            if not found:
                failures.append(
                    f"stale parent-child source entry: {parent} -> {entry}"
                )

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
        try:
            current_size = source_size((root / relative).read_bytes())
            classification = classify_source(relative, facade_paths)
            if classification is None or classification[0] == "test":
                failures.append(f"cannot classify production source: {relative_name}")
                continue
            source_kind, language = classification
            byte_limit, line_limit, measurement_kind = effective_limits(
                root,
                relative,
                config=config,
                kind=source_kind,
                language=language,
                current_size=current_size,
                base_ref=base_ref,
            )
            measurement = measure_source(
                root,
                relative,
                byte_limit=byte_limit,
                line_limit=line_limit,
                kind=measurement_kind,
            )
        except (OSError, UnicodeDecodeError) as error:
            failures.append(f"cannot inspect {relative_name}: {error}")
            continue
        measurements.append(measurement)
        if measurement.failed:
            if measurement.kind == "baseline":
                failures.append(
                    f"{relative_name} grew beyond its baseline "
                    f"({measurement.bytes}/{byte_limit} bytes, "
                    f"{measurement.lines}/{line_limit} lines)"
                )
            else:
                failures.append(
                    f"{measurement.kind} source exceeds the design-review limit: "
                    f"{relative_name} "
                    f"({measurement.bytes}/{byte_limit} bytes, "
                    f"{measurement.lines}/{line_limit} lines)"
                )

    shown = [
        measurement
        for measurement in measurements
        if measurement.kind == "baseline"
        or measurement.kind.startswith("legacy-")
        or measurement.kind.startswith("generated:")
        or measurement.failed
    ]
    return failures, shown


def evaluate_test_source_sizes(
    root: Path, config_path: Path, *, base_ref: str | None = None
) -> tuple[list[str], list[SourceMeasurement]]:
    config = load_test_config(config_path)
    baselines = config["baselines"]
    failures: list[str] = []
    measurements: list[SourceMeasurement] = []
    sources = test_sources(root)
    source_names = {source.as_posix() for source in sources}

    for baseline_path, baseline in sorted(baselines.items()):
        if not isinstance(baseline_path, str) or not isinstance(baseline, dict):
            failures.append("test baseline entries must map source paths to limit objects")
            continue
        if baseline_path not in source_names:
            failures.append(
                f"stale or non-test baseline entry: {baseline_path}; remove it explicitly"
            )
            continue
        if not all(
            isinstance(baseline.get(key), int) and baseline[key] > 0
            for key in ("bytes", "lines")
        ):
            failures.append(f"invalid test baseline limits for {baseline_path}")

    for relative in sources:
        relative_name = relative.as_posix()
        try:
            current_size = source_size((root / relative).read_bytes())
            language = source_language(relative)
            if language is None:
                failures.append(f"cannot classify test source: {relative_name}")
                continue
            byte_limit, line_limit, measurement_kind = effective_limits(
                root,
                relative,
                config=config,
                kind="test",
                language=language,
                current_size=current_size,
                base_ref=base_ref,
            )
            measurement = measure_source(
                root,
                relative,
                byte_limit=byte_limit,
                line_limit=line_limit,
                kind=(
                    "test-baseline" if measurement_kind == "baseline" else measurement_kind
                ),
            )
        except (OSError, UnicodeDecodeError) as error:
            failures.append(f"cannot inspect test source {relative_name}: {error}")
            continue
        measurements.append(measurement)
        if measurement.failed:
            if measurement.kind == "test-baseline":
                failures.append(
                    f"{relative_name} grew beyond its test baseline "
                    f"({measurement.bytes}/{byte_limit} bytes, "
                    f"{measurement.lines}/{line_limit} lines)"
                )
            else:
                failures.append(
                    f"{measurement.kind} source exceeds the design-review limit: "
                    f"{relative_name} "
                    f"({measurement.bytes}/{byte_limit} bytes, "
                    f"{measurement.lines}/{line_limit} lines)"
                )

    shown = [
        measurement
        for measurement in measurements
        if measurement.kind == "test-baseline"
        or measurement.kind.startswith("legacy-")
        or measurement.failed
    ]
    return failures, shown


def source_directory_key(relative: Path) -> str:
    parts = relative.parts
    if parts[:3] == ("apps", "lorepia", "src"):
        return "apps/lorepia/src"
    if parts[:3] == ("apps", "lorepia", "src-tauri"):
        if len(parts) >= 4 and parts[3] in {"gen", "generated", "src", "tests"}:
            return "/".join(parts[:4])
        return "apps/lorepia/src-tauri"
    if len(parts) >= 3 and parts[0] in {"crates", "plugins"}:
        return "/".join(parts[:3])
    return relative.parent.as_posix()


def baseline_parent_key(relative: Path, parent_paths: set[str]) -> str | None:
    relative_name = relative.as_posix()
    return relative_name if relative_name in parent_paths else None


def aggregate_changes(
    changes: list[SourceChange], *, key_for_path: Any
) -> list[AggregateDelta]:
    totals: dict[str, list[int]] = {}

    def add(path: Path, size: SourceSize, *, before: bool) -> None:
        key = key_for_path(path)
        if key is None:
            return
        values = totals.setdefault(key, [0, 0, 0, 0, 0, 0])
        offset = 0 if before else 1
        values[offset] += 1
        values[2 + offset] += size.bytes
        values[4 + offset] += size.lines

    for change in changes:
        if change.before_path is not None and change.before_size is not None:
            add(change.before_path, change.before_size, before=True)
        if change.after_path is not None and change.after_size is not None:
            add(change.after_path, change.after_size, before=False)

    return [
        AggregateDelta(
            path=path,
            before_files=values[0],
            after_files=values[1],
            before_bytes=values[2],
            after_bytes=values[3],
            before_lines=values[4],
            after_lines=values[5],
        )
        for path, values in sorted(totals.items())
    ]


def changed_sources(
    root: Path, base_ref: str, facade_paths: set[str]
) -> list[SourceChange]:
    process = subprocess.run(
        ["git", "diff", "--name-status", "--find-renames=100%", base_ref, "--"],
        cwd=root,
        check=False,
        capture_output=True,
        text=True,
    )
    if process.returncode != 0:
        raise ValueError(f"cannot inspect source deltas against {base_ref}: {process.stderr.strip()}")
    changes: list[SourceChange] = []
    for line in process.stdout.splitlines():
        fields = line.split("\t")
        status = fields[0]
        if status.startswith(("R", "C")) and len(fields) == 3:
            before_path = Path(fields[1])
            after_path = Path(fields[2])
        elif len(fields) == 2:
            path = Path(fields[1])
            before_path = None if status == "A" else path
            after_path = None if status == "D" else path
        else:
            continue
        before_class = (
            classify_source(before_path, facade_paths) if before_path is not None else None
        )
        after_class = (
            classify_source(after_path, facade_paths) if after_path is not None else None
        )
        if before_class is None and after_class is None:
            continue
        before_size = (
            source_size_at_ref(root, base_ref, before_path)
            if before_path is not None and before_class is not None
            else None
        )
        try:
            after_size = (
                source_size((root / after_path).read_bytes())
                if after_path is not None and after_class is not None
                else None
            )
        except (OSError, UnicodeDecodeError) as error:
            raise ValueError(f"cannot inspect changed source {after_path}: {error}") from error
        changes.append(
            SourceChange(
                before_path=before_path if before_size is not None else None,
                before_size=before_size,
                after_path=after_path if after_size is not None else None,
                after_size=after_size,
            )
        )
    return changes


def path_is_in_parent_child_group(
    relative: Path, parent: str, child_entries: list[str]
) -> bool:
    relative_name = relative.as_posix()
    return relative_name == parent or any(
        relative_name.startswith(entry) if entry.endswith("/") else relative_name == entry
        for entry in child_entries
    )


def aggregate_parent_child_groups(
    before_sizes: dict[Path, SourceSize],
    after_sizes: dict[Path, SourceSize],
    groups: dict[str, list[str]],
) -> list[AggregateDelta]:
    aggregates: list[AggregateDelta] = []
    for parent, child_entries in sorted(groups.items()):
        before = [
            size
            for path, size in before_sizes.items()
            if path_is_in_parent_child_group(path, parent, child_entries)
        ]
        after = [
            size
            for path, size in after_sizes.items()
            if path_is_in_parent_child_group(path, parent, child_entries)
        ]
        aggregates.append(
            AggregateDelta(
                path=parent,
                before_files=len(before),
                after_files=len(after),
                before_bytes=sum(size.bytes for size in before),
                after_bytes=sum(size.bytes for size in after),
                before_lines=sum(size.lines for size in before),
                after_lines=sum(size.lines for size in after),
            )
        )
    return aggregates


def parent_child_group_deltas(
    root: Path,
    base_ref: str,
    *,
    facade_paths: set[str],
    groups: dict[str, list[str]],
) -> list[AggregateDelta]:
    base_listing = subprocess.run(
        ["git", "ls-tree", "-r", "--name-only", base_ref],
        cwd=root,
        check=False,
        capture_output=True,
        text=True,
    )
    if base_listing.returncode != 0:
        raise ValueError(
            f"cannot inspect parent-child sources against {base_ref}: "
            f"{base_listing.stderr.strip()}"
        )
    base_paths = {
        Path(path)
        for path in base_listing.stdout.splitlines()
        if path and classify_source(Path(path), facade_paths) is not None
    }
    current_paths = {
        *production_sources(root),
        *generated_sources(root),
        *test_sources(root),
    }
    relevant_base_paths = {
        path
        for path in base_paths
        if any(
            path_is_in_parent_child_group(path, parent, prefixes)
            for parent, prefixes in groups.items()
        )
    }
    relevant_current_paths = {
        path
        for path in current_paths
        if any(
            path_is_in_parent_child_group(path, parent, prefixes)
            for parent, prefixes in groups.items()
        )
    }
    before_sizes = {
        path: size
        for path in sorted(relevant_base_paths)
        if (size := source_size_at_ref(root, base_ref, path)) is not None
    }
    try:
        after_sizes = {
            path: source_size((root / path).read_bytes())
            for path in sorted(relevant_current_paths)
        }
    except (OSError, UnicodeDecodeError) as error:
        raise ValueError(f"cannot inspect parent-child source aggregate: {error}") from error
    return aggregate_parent_child_groups(before_sizes, after_sizes, groups)


def source_aggregate_deltas(
    root: Path,
    base_ref: str,
    *,
    facade_paths: set[str],
    parent_paths: set[str],
    parent_child_groups: dict[str, list[str]],
) -> tuple[list[AggregateDelta], list[AggregateDelta], list[AggregateDelta]]:
    changes = changed_sources(root, base_ref, facade_paths)
    directories = aggregate_changes(changes, key_for_path=source_directory_key)
    parents = aggregate_changes(
        changes,
        key_for_path=lambda path: baseline_parent_key(path, parent_paths),
    )
    groups = parent_child_group_deltas(
        root,
        base_ref,
        facade_paths=facade_paths,
        groups=parent_child_groups,
    )
    return directories, parents, groups


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


def normalize_dependency_architecture(
    metadata: dict[str, Any], root: Path
) -> dict[str, Any]:
    packages = metadata.get("packages")
    workspace_member_list = metadata.get("workspace_members")
    if not isinstance(packages, list) or not isinstance(workspace_member_list, list):
        raise ValueError("cargo metadata did not contain packages and workspace_members")
    workspace_members = set(workspace_member_list)
    root = root.resolve()
    members: list[dict[str, Any]] = []
    for package in packages:
        if not isinstance(package, dict) or package.get("id") not in workspace_members:
            continue
        name = package.get("name")
        manifest_path = package.get("manifest_path")
        if not isinstance(name, str) or not isinstance(manifest_path, str):
            raise ValueError("workspace package metadata is missing name or manifest_path")
        manifest = Path(manifest_path).resolve()
        try:
            relative_manifest = manifest.relative_to(root).as_posix()
        except ValueError as error:
            raise ValueError(f"workspace manifest is outside the repository: {manifest}") from error
        if not relative_manifest.endswith("/Cargo.toml"):
            raise ValueError(f"workspace manifest is not a Cargo.toml: {relative_manifest}")
        members.append(
            {
                "metadata": package,
                "manifest": relative_manifest,
                "name": name,
                "path": manifest.parent,
            }
        )
    if len({member["name"] for member in members}) != len(members):
        raise ValueError("workspace package names must be unique")
    if len({member["path"] for member in members}) != len(members):
        raise ValueError("workspace package paths must be unique")

    workspace_by_path = {member["path"]: member["name"] for member in members}
    workspace_dependencies: list[dict[str, Any]] = []
    external_dependencies: list[dict[str, Any]] = []
    for member in members:
        dependencies = member["metadata"].get("dependencies", [])
        if not isinstance(dependencies, list):
            raise ValueError(f"{member['name']} dependencies metadata must be an array")
        for dependency in dependencies:
            if not isinstance(dependency, dict):
                raise ValueError(f"{member['name']} dependency metadata is malformed")
            name = dependency.get("name")
            requirement = dependency.get("req")
            if not isinstance(name, str) or not isinstance(requirement, str):
                raise ValueError(f"{member['name']} dependency is missing name or requirement")
            kind = dependency.get("kind") or "normal"
            features = dependency.get("features", [])
            if kind not in DEPENDENCY_RECORD_KINDS or not isinstance(features, list):
                raise ValueError(f"{member['name']} dependency profile is malformed")
            common = {
                "default_features": dependency.get("uses_default_features"),
                "features": sorted(set(features)),
                "from": member["name"],
                "kind": kind,
                "optional": dependency.get("optional"),
                "requirement": requirement,
                "rename": dependency.get("rename"),
                "target": dependency.get("target"),
            }
            dependency_path = dependency.get("path")
            target_name = None
            if isinstance(dependency_path, str):
                target_name = workspace_by_path.get(Path(dependency_path).resolve())
            if target_name is not None:
                workspace_dependencies.append({**common, "to": target_name})
            else:
                source = dependency.get("source")
                external_dependencies.append(
                    {
                        **common,
                        "package": name,
                        "source": source if isinstance(source, str) and source else "path",
                    }
                )

    package_features: dict[str, dict[str, list[str]]] = {}
    for member in sorted(members, key=lambda item: item["name"]):
        raw_features = member["metadata"].get("features", {})
        if not isinstance(raw_features, dict):
            raise ValueError(f"{member['name']} package features metadata is malformed")
        normalized_features: dict[str, list[str]] = {}
        for feature_name, activations in sorted(raw_features.items()):
            if not isinstance(feature_name, str) or not isinstance(activations, list):
                raise ValueError(f"{member['name']} package feature is malformed")
            normalized_features[feature_name] = sorted(set(activations))
        if normalized_features:
            package_features[member["name"]] = normalized_features

    return {
        "direct_external_dependencies": sorted(
            external_dependencies,
            key=lambda record: dependency_record_key(record, workspace=False),
        ),
        "package_features": package_features,
        "workspace_dependencies": sorted(
            workspace_dependencies,
            key=lambda record: dependency_record_key(record, workspace=True),
        ),
        "workspace_packages": sorted(
            (
                {"manifest": member["manifest"], "name": member["name"]}
                for member in members
            ),
            key=lambda package: (package["name"], package["manifest"]),
        ),
    }


def describe_dependency_record(record: dict[str, Any], *, workspace: bool) -> str:
    target_key = "to" if workspace else "package"
    profile = f"{record['kind']},req={record['requirement']}"
    if record["optional"]:
        profile += ",optional"
    if record["target"]:
        profile += f",target={record['target']}"
    if record["features"]:
        profile += f",features={','.join(record['features'])}"
    if record["rename"]:
        profile += f",rename={record['rename']}"
    return f"{record['from']} -> {record[target_key]} ({profile})"


def evaluate_dependency_architecture(
    metadata: dict[str, Any], policy: dict[str, Any], root: Path
) -> list[str]:
    policy = validate_dependency_architecture_config(policy)
    actual = normalize_dependency_architecture(metadata, root)
    failures: list[str] = []

    expected_packages = {
        (package["name"], package["manifest"])
        for package in policy["workspace_packages"]
    }
    actual_packages = {
        (package["name"], package["manifest"])
        for package in actual["workspace_packages"]
    }
    for name, manifest in sorted(actual_packages - expected_packages):
        failures.append(f"unapproved workspace package: {name} ({manifest})")
    for name, manifest in sorted(expected_packages - actual_packages):
        failures.append(f"stale workspace package policy: {name} ({manifest})")

    for field, workspace in (
        ("workspace_dependencies", True),
        ("direct_external_dependencies", False),
    ):
        expected_by_key = {
            dependency_record_key(record, workspace=workspace): record
            for record in policy[field]
        }
        actual_by_key = {
            dependency_record_key(record, workspace=workspace): record
            for record in actual[field]
        }
        for key in sorted(actual_by_key.keys() - expected_by_key.keys()):
            failures.append(
                f"unapproved direct dependency: "
                f"{describe_dependency_record(actual_by_key[key], workspace=workspace)}"
            )
        for key in sorted(expected_by_key.keys() - actual_by_key.keys()):
            failures.append(
                f"stale dependency policy after removal: "
                f"{describe_dependency_record(expected_by_key[key], workspace=workspace)}"
            )

    actual_feature_tokens = flatten_package_features(
        {"package_features": actual["package_features"]}
    )
    expected_feature_tokens = flatten_package_features(policy)
    for package_name, feature_name, activation in sorted(
        actual_feature_tokens - expected_feature_tokens
    ):
        failures.append(
            f"unapproved package feature activation: {package_name}/{feature_name} -> "
            f"{activation or '<empty>'}"
        )
    for package_name, feature_name, activation in sorted(
        expected_feature_tokens - actual_feature_tokens
    ):
        failures.append(
            f"stale package feature policy after removal: {package_name}/{feature_name} -> "
            f"{activation or '<empty>'}"
        )

    workspace_names = {package["name"] for package in actual["workspace_packages"]}
    for dependency in actual["direct_external_dependencies"]:
        if dependency["from"] == "lorepia-orchestration" and dependency[
            "package"
        ] in FORBIDDEN_ORCHESTRATION_DEPENDENCIES:
            failures.append(
                "lorepia-orchestration must not directly depend on I/O boundary crate "
                f"{dependency['package']}"
            )
    domain_edges = [
        record
        for record in actual["workspace_dependencies"]
        if record["from"] == "lorepia-domain"
    ]
    for record in domain_edges:
        failures.append(
            f"lorepia-domain must not depend on workspace crate {record['to']}"
        )
    for record in actual["workspace_dependencies"]:
        if record["from"] == "lorepia-orchestration" and record["to"] != "lorepia-domain":
            failures.append(
                "lorepia-orchestration may only depend on lorepia-domain below its layer; "
                f"found {record['to']}"
            )
    if "lorepia-domain" not in workspace_names:
        failures.append("cargo workspace is missing lorepia-domain")
    if "lorepia-orchestration" not in workspace_names:
        failures.append("cargo workspace is missing lorepia-orchestration")
    return sorted(set(failures))


@lru_cache(maxsize=512)
def strip_rust_comments_and_strings(
    content: str, *, strip_strings: bool = True
) -> str:
    """Blank Rust comments and strings while preserving source positions."""

    output = list(content)
    index = 0
    length = len(content)

    def blank(start: int, end: int) -> None:
        for offset in range(start, end):
            if output[offset] != "\n":
                output[offset] = " "

    while index < length:
        if content.startswith("//", index):
            end = content.find("\n", index + 2)
            end = length if end == -1 else end
            blank(index, end)
            index = end
            continue
        if content.startswith("/*", index):
            start = index
            index += 2
            depth = 1
            while index < length and depth:
                if content.startswith("/*", index):
                    depth += 1
                    index += 2
                elif content.startswith("*/", index):
                    depth -= 1
                    index += 2
                else:
                    index += 1
            blank(start, index)
            continue

        raw = RUST_RAW_STRING_RE.match(content, index)
        if raw is not None:
            start = index
            delimiter = '"' + raw.group("hashes")
            body_start = raw.end()
            end = content.find(delimiter, body_start)
            index = length if end == -1 else end + len(delimiter)
            if strip_strings:
                blank(start, index)
            continue

        quote_start = index
        if content.startswith(('b"', 'c"'), index):
            index += 1
        if content[index] == '"':
            index += 1
            while index < length:
                if content[index] == "\\":
                    index += 2
                elif content[index] == '"':
                    index += 1
                    break
                else:
                    index += 1
            if strip_strings:
                blank(quote_start, min(index, length))
            continue

        char_start = index
        if content.startswith("b'", index):
            index += 1
        if content[index] == "'":
            cursor = index + 1
            if cursor < length and content[cursor] == "\\":
                if content.startswith("\\u{", cursor):
                    escape_end = content.find("}", cursor + 3)
                    cursor = length if escape_end == -1 else escape_end + 1
                elif content.startswith("\\x", cursor):
                    cursor += 4
                else:
                    cursor += 2
            else:
                cursor += 1
            if cursor < length and content[cursor] == "'":
                index = cursor + 1
                if strip_strings:
                    blank(char_start, index)
                continue
            index = char_start + 1
            continue
        index += 1

    return "".join(output)


RUST_USE_TOKEN_RE = re.compile(
    r"::|[{},*]|r#[A-Za-z_][A-Za-z0-9_]*|[A-Za-z_][A-Za-z0-9_]*"
)


@lru_cache(maxsize=512)
def rust_brace_depths(content: str) -> list[int]:
    depths: list[int] = [0] * (len(content) + 1)
    depth = 0
    for index, character in enumerate(content):
        depths[index] = depth
        if character == "{":
            depth += 1
        elif character == "}":
            if depth == 0:
                raise ValueError("unbalanced Rust closing brace while reading public API")
            depth -= 1
    depths[len(content)] = depth
    if depth != 0:
        raise ValueError("unbalanced Rust braces while reading public API")
    return depths


@lru_cache(maxsize=512)
def rust_group_depths(content: str) -> list[int]:
    depths: list[int] = [0] * (len(content) + 1)
    stack: list[str] = []
    closing = {")": "(", "]": "["}
    for index, character in enumerate(content):
        depths[index] = len(stack)
        if character in {"(", "["}:
            stack.append(character)
        elif character in closing:
            if not stack or stack[-1] != closing[character]:
                raise ValueError("unbalanced Rust delimiter while reading public API")
            stack.pop()
    depths[len(content)] = len(stack)
    if stack:
        raise ValueError("unbalanced Rust delimiter while reading public API")
    return depths


def rust_use_tokens(body: str) -> list[str]:
    tokens: list[str] = []
    cursor = 0
    for match in RUST_USE_TOKEN_RE.finditer(body):
        if body[cursor : match.start()].strip():
            raise ValueError(
                f"unsupported public use syntax near {body[cursor:match.start()].strip()!r}"
            )
        tokens.append(match.group(0))
        cursor = match.end()
    if body[cursor:].strip():
        raise ValueError(f"unsupported public use syntax near {body[cursor:].strip()!r}")
    if not tokens:
        raise ValueError("empty public use declaration")
    return tokens


def parse_rust_use_tree(body: str) -> list[tuple[str, str, bool]]:
    """Expand a Rust use tree into (public name, origin, wildcard) leaves."""

    tokens = rust_use_tokens(body)
    cursor = 0

    def parse_item(prefix: list[str]) -> list[tuple[str, str, bool]]:
        nonlocal cursor
        absolute = False
        if cursor < len(tokens) and tokens[cursor] == "::":
            absolute = True
            cursor += 1
        if cursor < len(tokens) and tokens[cursor] == "{":
            cursor += 1
            leaves = parse_group(prefix)
            return leaves

        segments: list[str] = []
        while cursor < len(tokens):
            token = tokens[cursor]
            if token in {"{", "}", ",", "*", "::"} or token == "as":
                break
            segments.append(token)
            cursor += 1
            if cursor >= len(tokens) or tokens[cursor] != "::":
                break
            cursor += 1
            if cursor < len(tokens) and tokens[cursor] == "{":
                cursor += 1
                base = [*prefix, *segments]
                return parse_group(base)
            if cursor < len(tokens) and tokens[cursor] == "*":
                cursor += 1
                origin = "::".join([*prefix, *segments])
                if absolute:
                    origin = f"::{origin}"
                return [("*", f"{origin}::*", True)]

        if not segments:
            raise ValueError("public use tree is missing a path segment")
        if segments == ["self"]:
            origin_segments = prefix
        else:
            origin_segments = [*prefix, *segments]
        if not origin_segments:
            raise ValueError("public use self leaf has no parent path")
        public_name = origin_segments[-1]
        if cursor < len(tokens) and tokens[cursor] == "as":
            cursor += 1
            if cursor >= len(tokens) or tokens[cursor] in {"{", "}", ",", "*", "::", "as"}:
                raise ValueError("public use alias is missing a name")
            public_name = tokens[cursor]
            cursor += 1
        origin = "::".join(origin_segments)
        if absolute:
            origin = f"::{origin}"
        return [(public_name, origin, False)]

    def parse_group(prefix: list[str]) -> list[tuple[str, str, bool]]:
        nonlocal cursor
        leaves: list[tuple[str, str, bool]] = []
        expect_item = True
        while cursor < len(tokens):
            if tokens[cursor] == "}":
                cursor += 1
                return leaves
            if not expect_item:
                if tokens[cursor] != ",":
                    raise ValueError("public use group is missing a comma")
                cursor += 1
                if cursor < len(tokens) and tokens[cursor] == "}":
                    cursor += 1
                    return leaves
            leaves.extend(parse_item(prefix))
            expect_item = False
        raise ValueError("unterminated public use group")

    leaves = parse_item([])
    if cursor != len(tokens):
        raise ValueError(f"unexpected public use token: {tokens[cursor]}")
    return leaves


def rust_same_depth_terminator(
    content: str,
    depths: list[int],
    start: int,
    *,
    depth: int,
    characters: set[str],
    group_depths: list[int] | None = None,
    group_depth: int = 0,
    angle_depths: list[int] | None = None,
    angle_depth: int = 0,
    ignored_indices: set[int] | None = None,
) -> int:
    for index in range(start, len(content)):
        if (
            depths[index] == depth
            and (group_depths is None or group_depths[index] == group_depth)
            and (angle_depths is None or angle_depths[index] == angle_depth)
            and (ignored_indices is None or index not in ignored_indices)
            and content[index] in characters
        ):
            return index
    raise ValueError("unterminated Rust public declaration")


def rust_public_item_kind_and_name(header: str) -> tuple[str, str]:
    function = re.match(
        r"\s*(?:(?:async|const|default|unsafe)\s+)*"
        r'(?:extern(?:\s+"[^"]+")?\s+)?fn\s+'
        r"(r#[A-Za-z_][A-Za-z0-9_]*|[A-Za-z_][A-Za-z0-9_]*)",
        header,
    )
    if function is not None:
        return "fn", function.group(1)
    static_item = re.match(
        r"\s*static\s+(?:mut\s+)?"
        r"(r#[A-Za-z_][A-Za-z0-9_]*|[A-Za-z_][A-Za-z0-9_]*)",
        header,
    )
    if static_item is not None:
        return "static", static_item.group(1)
    item = re.match(
        r"\s*(?:unsafe\s+)?"
        r"(const|enum|macro|mod|static|struct|trait|type|union)\s+"
        r"(r#[A-Za-z_][A-Za-z0-9_]*|[A-Za-z_][A-Za-z0-9_]*)",
        header,
    )
    if item is None:
        raise ValueError(f"unsupported unrestricted public declaration: {header.strip()[:120]}")
    return item.group(1), item.group(2)


RUST_SURFACE_TOKEN_RE = re.compile(
    r"r#[A-Za-z_][A-Za-z0-9_]*|"
    r"'[A-Za-z_][A-Za-z0-9_]*|"
    r"[A-Za-z_][A-Za-z0-9_]*|"
    r"0[xob][0-9A-Fa-f_]+|[0-9][0-9A-Za-z_]*|"
    r"::|->|=>|\.\.=|\.\.|==|!=|<=|>=|&&|\|\||<<|>>|\+=|-=|\*=|/=|%=|&=|\|=|\^=|"
    r"[^\s]"
)
RUST_RAW_STRING_START_RE = re.compile(
    r'(?:br|cr|r)(?P<hashes>#{0,255})"'
)
RUST_CHARACTER_LITERAL_RE = re.compile(
    r"(?:b)?'(?:\\(?:u\{[0-9A-Fa-f_]+\}|x[0-9A-Fa-f]{2}|.)|[^'\\\n])'"
)


@lru_cache(maxsize=1024)
def rust_surface_token_spans(content: str) -> list[tuple[str, int, int]]:
    tokens: list[tuple[str, int, int]] = []
    cursor = 0
    while cursor < len(content):
        if content[cursor].isspace():
            cursor += 1
            continue

        raw = RUST_RAW_STRING_START_RE.match(content, cursor)
        if raw is not None:
            closing = f'"{raw.group("hashes")}'
            end = content.find(closing, raw.end())
            if end == -1:
                raise ValueError("unterminated Rust raw string in public API surface")
            end += len(closing)
            tokens.append((content[cursor:end], cursor, end))
            cursor = end
            continue

        string_prefix = next(
            (
                prefix
                for prefix in ('b"', 'c"', '"')
                if content.startswith(prefix, cursor)
            ),
            None,
        )
        if string_prefix is not None:
            end = cursor + len(string_prefix)
            while end < len(content):
                if content[end] == "\\":
                    end += 2
                elif content[end] == '"':
                    end += 1
                    break
                else:
                    end += 1
            else:
                raise ValueError("unterminated Rust string in public API surface")
            tokens.append((content[cursor:end], cursor, end))
            cursor = end
            continue

        character = RUST_CHARACTER_LITERAL_RE.match(content, cursor)
        if character is not None:
            end = character.end()
            tokens.append((content[cursor:end], cursor, end))
            cursor = end
            continue

        token = RUST_SURFACE_TOKEN_RE.match(content, cursor)
        if token is None:
            raise ValueError(
                f"unsupported Rust public API surface token near {content[cursor:cursor + 40]!r}"
            )
        tokens.append((token.group(0), cursor, token.end()))
        cursor = token.end()
    return tokens


def rust_surface_tokens(content: str) -> list[str]:
    return [token for token, _, _ in rust_surface_token_spans(content)]


@lru_cache(maxsize=512)
def rust_brace_macro_delimiters(content: str) -> tuple[set[int], set[int]]:
    spans = rust_surface_token_spans(content)
    brace_stack: list[int] = []
    brace_pairs: dict[int, int] = {}
    for token, start, _ in spans:
        if token == "{":
            brace_stack.append(start)
        elif token == "}":
            if not brace_stack:
                raise ValueError("unbalanced Rust brace in public API surface")
            brace_pairs[brace_stack.pop()] = start
    if brace_stack:
        raise ValueError("unterminated Rust brace in public API surface")
    openers: set[int] = set()
    closers: set[int] = set()
    for index, (token, start, _) in enumerate(spans):
        direct_macro = (
            index >= 2
            and spans[index - 1][0] == "!"
            and re.fullmatch(
                r"r#[A-Za-z_][A-Za-z0-9_]*|[A-Za-z_][A-Za-z0-9_]*",
                spans[index - 2][0],
            )
            is not None
        )
        macro_rules_definition = (
            index >= 3
            and spans[index - 2][0] == "!"
            and spans[index - 3][0] == "macro_rules"
            and re.fullmatch(
                r"r#[A-Za-z_][A-Za-z0-9_]*|[A-Za-z_][A-Za-z0-9_]*",
                spans[index - 1][0],
            )
            is not None
        )
        if token != "{" or not (direct_macro or macro_rules_definition):
            continue
        close = brace_pairs.get(start)
        if close is None:
            raise ValueError("unterminated brace-delimited Rust macro in public API")
        openers.add(start)
        closers.add(close)
    return openers, closers


def rust_paired_brace_macro_ranges(
    depths: list[int], openers: set[int], closers: set[int]
) -> list[tuple[int, int]]:
    closers_by_depth: dict[int, list[int]] = {}
    for closer in sorted(closers):
        closers_by_depth.setdefault(depths[closer], []).append(closer)
    ranges: list[tuple[int, int]] = []
    for opener in sorted(openers):
        matching_closers = closers_by_depth.get(depths[opener] + 1, [])
        position = bisect_right(matching_closers, opener)
        if position == len(matching_closers):
            raise ValueError("unterminated brace-delimited Rust macro")
        ranges.append((opener, matching_closers[position]))
    return ranges


def rust_inline_module_scopes(
    masked: str,
    depths: list[int],
    group_depths: list[int],
    brace_macro_ranges: list[tuple[int, int]],
) -> list[tuple[int, int, int, bool]]:
    scopes: list[tuple[int, int, int, bool]] = []
    for module in re.finditer(
        r"\bmod\s+(?P<name>r#[A-Za-z_][A-Za-z0-9_]*|"
        r"[A-Za-z_][A-Za-z0-9_]*)\s*\{",
        masked,
    ):
        if group_depths[module.start()] != 0 or any(
            opener < module.start() < closer
            for opener, closer in brace_macro_ranges
        ):
            continue
        open_brace = masked.find("{", module.start(), module.end())
        close_brace = rust_same_depth_terminator(
            masked,
            depths,
            open_brace + 1,
            depth=depths[open_brace] + 1,
            characters={"}"},
        )
        attribute_start = rust_outer_attribute_start(
            masked,
            depths,
            group_depths,
            module.start(),
            depth=depths[module.start()],
        )
        attributes = masked[attribute_start : module.start()]
        inherited_test_scope = any(
            is_test and start <= module.start() < end
            for start, end, _, is_test in scopes
        )
        is_test_scope = (
            inherited_test_scope
            or re.search(
                r"#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]",
                attributes,
            )
            is not None
        )
        scopes.append(
            (
                open_brace + 1,
                close_brace,
                depths[open_brace] + 1,
                is_test_scope,
            )
        )
    return scopes


def rust_production_item_scopes(
    masked: str, depths: list[int], group_depths: list[int]
) -> list[tuple[int, int, int]]:
    brace_macro_openers, brace_macro_closers = rust_brace_macro_delimiters(masked)
    brace_macro_ranges = rust_paired_brace_macro_ranges(
        depths, brace_macro_openers, brace_macro_closers
    )
    scopes: list[tuple[int, int, int]] = [(0, len(masked), 0)]
    scopes.extend(
        (start, end, depth)
        for start, end, depth, is_test in rust_inline_module_scopes(
            masked, depths, group_depths, brace_macro_ranges
        )
        if not is_test
    )
    return scopes


def rust_is_production_item_position(
    position: int,
    depths: list[int],
    scopes: list[tuple[int, int, int]],
) -> bool:
    return any(
        start <= position < end and depths[position] == depth
        for start, end, depth in scopes
    )


def rust_angle_depths(
    content: str, brace_depths: list[int], group_depths: list[int]
) -> list[int]:
    depths = [0] * (len(content) + 1)
    token_starts = {
        start: token for token, start, _ in rust_surface_token_spans(content)
    }
    levels: dict[tuple[int, int], int] = {}
    for index in range(len(content)):
        brace_depth = brace_depths[index]
        group_depth = group_depths[index]
        scope = (brace_depth, group_depth)
        depths[index] = levels.get(scope, 0)
        token = token_starts.get(index)
        if token in {"<", "<<"}:
            levels[scope] = levels.get(scope, 0) + len(token)
        elif token in {">", ">>"}:
            levels[scope] = max(
                0, levels.get(scope, 0) - len(token)
            )
        elif token == ";" and group_depths[index] == 0:
            levels[scope] = 0
        elif token in {")", "]"}:
            levels[scope] = 0
        elif token == "}":
            for nested_scope in [
                nested_scope
                for nested_scope in levels
                if nested_scope[0] == brace_depth
            ]:
                levels[nested_scope] = 0
    depths[len(content)] = levels.get(
        (brace_depths[len(content)], group_depths[len(content)]), 0
    )
    return depths


def normalized_rust_surface(content: str) -> str:
    return " ".join(rust_surface_tokens(content))


def rust_surface_digest(content: str) -> str:
    return hashlib.sha256(normalized_rust_surface(content).encode("utf-8")).hexdigest()


def rust_outer_attribute_start(
    masked: str,
    depths: list[int],
    group_depths: list[int],
    item_start: int,
    *,
    depth: int,
) -> int:
    cursor = item_start
    while True:
        end = cursor
        while end > 0 and masked[end - 1].isspace():
            end -= 1
        if end == 0 or masked[end - 1] != "]":
            return cursor
        bracket_depth = 1
        start = end - 2
        while start >= 0:
            if masked[start] == "]":
                bracket_depth += 1
            elif masked[start] == "[":
                bracket_depth -= 1
                if bracket_depth == 0:
                    break
            start -= 1
        if start <= 0:
            return cursor
        hash_index = start - 1
        if masked[hash_index] != "#":
            return cursor
        if depths[hash_index] != depth or group_depths[hash_index] != 0:
            return cursor
        cursor = hash_index


def rust_top_level_segments(
    content: str,
    depths: list[int],
    group_depths: list[int],
    angle_depths: list[int],
    start: int,
    end: int,
    *,
    depth: int,
    group_depth: int,
) -> list[tuple[int, int]]:
    segments: list[tuple[int, int]] = []
    segment_start = start
    for index in range(start, end):
        if (
            content[index] == ","
            and depths[index] == depth
            and group_depths[index] == group_depth
            and angle_depths[index] == 0
        ):
            segments.append((segment_start, index))
            segment_start = index + 1
    segments.append((segment_start, end))
    return segments


def rust_segment_has_unrestricted_public(
    masked: str,
    depths: list[int],
    group_depths: list[int],
    start: int,
    end: int,
    *,
    depth: int,
    group_depth: int,
) -> bool:
    return any(
        depths[match.start()] == depth
        and group_depths[match.start()] == group_depth
        for match in UNRESTRICTED_PUBLIC_RE.finditer(masked, start, end)
    )


def rust_public_item_surface(
    masked: str,
    surface_source: str,
    depths: list[int],
    group_depths: list[int],
    angle_depths: list[int],
    brace_macro_openers: set[int],
    brace_macro_closers: set[int],
    public_match: re.Match[str],
    *,
    depth: int,
) -> tuple[str, str, str]:
    declaration_start = public_match.end()
    kind, name = rust_public_item_kind_and_name(masked[declaration_start:])
    terminators = {";"} if kind in {"const", "static", "type"} else {";", "{"}
    end = rust_same_depth_terminator(
        masked,
        depths,
        declaration_start,
        depth=depth,
        characters=terminators,
        group_depths=group_depths,
        angle_depths=(
            None if kind in {"const", "static", "type"} else angle_depths
        ),
        ignored_indices=brace_macro_openers,
        )
    delimiter = masked[end]
    surface_start = rust_outer_attribute_start(
        masked,
        depths,
        group_depths,
        public_match.start(),
        depth=depth,
    )
    header = surface_source[surface_start:end]
    if delimiter == ";":
        if kind not in {"struct", "union"}:
            return kind, name, f"{header};"
        open_paren = next(
            (
                index
                for index in range(declaration_start, end)
                if masked[index] == "("
                and group_depths[index] == 0
                and angle_depths[index] == 0
            ),
            None,
        )
        if open_paren is None:
            return kind, name, f"{header};"
        close_paren = next(
            (
                index
                for index in range(open_paren + 1, end)
                if masked[index] == ")" and group_depths[index] == 1
            ),
            None,
        )
        if close_paren is None:
            raise ValueError(f"unterminated tuple {kind} {name}")
        fields: list[str] = []
        has_private_fields = False
        for ordinal, (field_start, field_end) in enumerate(
            rust_top_level_segments(
                masked,
                depths,
                group_depths,
                angle_depths,
                open_paren + 1,
                close_paren,
                depth=depth,
                group_depth=1,
            )
        ):
            if not masked[field_start:field_end].strip():
                continue
            if rust_segment_has_unrestricted_public(
                masked,
                depths,
                group_depths,
                field_start,
                field_end,
                depth=depth,
                group_depth=1,
            ):
                fields.append(
                    f"field[{ordinal}]={surface_source[field_start:field_end]}"
                )
            else:
                has_private_fields = True
        prefix = surface_source[surface_start:open_paren]
        suffix = surface_source[close_paren + 1 : end]
        return (
            kind,
            name,
            f"{prefix}({'|'.join(fields)}|has_private_fields="
            f"{str(has_private_fields).lower()}){suffix};",
        )
    if kind == "fn":
        return kind, name, header

    close_brace = rust_same_depth_terminator(
        masked, depths, end + 1, depth=depth + 1, characters={"}"}
    )
    if kind in {"struct", "union"}:
        public_fields: list[str] = []
        has_private_fields = False
        for field_start, field_end in rust_top_level_segments(
            masked,
            depths,
            group_depths,
            angle_depths,
            end + 1,
            close_brace,
            depth=depth + 1,
            group_depth=0,
        ):
            if not masked[field_start:field_end].strip():
                continue
            if rust_segment_has_unrestricted_public(
                masked,
                depths,
                group_depths,
                field_start,
                field_end,
                depth=depth + 1,
                group_depth=0,
            ):
                public_fields.append(surface_source[field_start:field_end])
            else:
                has_private_fields = True
        return (
            kind,
            name,
            f"{header}{{{'|'.join(public_fields)}|has_private_fields="
            f"{str(has_private_fields).lower()}}}",
        )
    if kind == "trait":
        pieces = [header, "{"]
        cursor = end + 1
        method_body_closes: set[int] = set()
        for index in range(end + 1, close_brace):
            if not (
                masked[index] == "{"
                and depths[index] == depth + 1
                and group_depths[index] == 0
                and angle_depths[index] == 0
                and index not in brace_macro_openers
            ):
                continue
            item_start = end + 1
            for prior in range(index - 1, end, -1):
                if (
                    group_depths[prior] == 0
                    and (
                        (masked[prior] == ";" and depths[prior] == depth + 1)
                        or (
                            prior in method_body_closes
                        )
                    )
                ):
                    item_start = prior + 1
                    break
            if re.search(
                r"\bfn\s+(?:r#[A-Za-z_][A-Za-z0-9_]*|[A-Za-z_][A-Za-z0-9_]*)",
                masked[item_start:index],
            ) is None:
                continue
            nested_close = rust_same_depth_terminator(
                masked,
                depths,
                index + 1,
                depth=depth + 2,
                characters={"}"},
            )
            pieces.append(surface_source[cursor:index])
            pieces.append("{}")
            cursor = nested_close + 1
            method_body_closes.add(nested_close)
        pieces.append(surface_source[cursor:close_brace])
        pieces.append("}")
        return kind, name, "".join(pieces)
    return kind, name, surface_source[surface_start : close_brace + 1]


@lru_cache(maxsize=512)
def rust_top_level_public_items(content: str) -> list[tuple[str, str, str]]:
    masked = strip_rust_comments_and_strings(content)
    surface_source = strip_rust_comments_and_strings(content, strip_strings=False)
    depths = rust_brace_depths(masked)
    group_depths = rust_group_depths(masked)
    item_scopes = rust_production_item_scopes(masked, depths, group_depths)
    angle_depths = rust_angle_depths(masked, depths, group_depths)
    brace_macro_openers, brace_macro_closers = rust_brace_macro_delimiters(masked)
    items: list[tuple[str, str, str]] = []
    for public_match in UNRESTRICTED_PUBLIC_RE.finditer(masked):
        if (
            not rust_is_production_item_position(
                public_match.start(), depths, item_scopes
            )
            or group_depths[public_match.start()] != 0
        ):
            continue
        declaration_start = public_match.end()
        if re.match(r"use\b", masked[declaration_start:]) is not None:
            continue
        items.append(
            rust_public_item_surface(
                masked,
                surface_source,
                depths,
                group_depths,
                angle_depths,
                brace_macro_openers,
                brace_macro_closers,
                public_match,
                depth=depths[public_match.start()],
            )
        )
    return items


@lru_cache(maxsize=512)
def rust_top_level_nominal_type_names(content: str) -> list[str]:
    masked = strip_rust_comments_and_strings(content)
    depths = rust_brace_depths(masked)
    group_depths = rust_group_depths(masked)
    item_scopes = rust_production_item_scopes(masked, depths, group_depths)
    return [
        match.group("name")
        for match in re.finditer(
            r"\b(?:struct|enum|trait|type|union)\s+"
            r"(?P<name>r#[A-Za-z_][A-Za-z0-9_]*|[A-Za-z_][A-Za-z0-9_]*)",
            masked,
        )
        if rust_is_production_item_position(match.start(), depths, item_scopes)
        and group_depths[match.start()] == 0
    ]


@lru_cache(maxsize=512)
def rust_top_level_simple_type_aliases(content: str) -> list[tuple[str, str]]:
    masked = strip_rust_comments_and_strings(content)
    depths = rust_brace_depths(masked)
    group_depths = rust_group_depths(masked)
    item_scopes = rust_production_item_scopes(masked, depths, group_depths)
    aliases: list[tuple[str, str]] = []
    for match in re.finditer(
        r"\btype\s+(?P<name>r#[A-Za-z_][A-Za-z0-9_]*|[A-Za-z_][A-Za-z0-9_]*)",
        masked,
    ):
        if (
            not rust_is_production_item_position(match.start(), depths, item_scopes)
            or group_depths[match.start()] != 0
        ):
            continue
        end = rust_same_depth_terminator(
            masked,
            depths,
            match.end(),
            depth=depths[match.start()],
            characters={";"},
            group_depths=group_depths,
        )
        target = rust_simple_type_alias_target(masked[match.start() : end + 1])
        if target is not None:
            aliases.append((match.group("name"), target))
    return aliases


@lru_cache(maxsize=512)
def rust_top_level_simple_use_aliases(content: str) -> list[tuple[str, str]]:
    masked = strip_rust_comments_and_strings(content)
    depths = rust_brace_depths(masked)
    group_depths = rust_group_depths(masked)
    item_scopes = rust_production_item_scopes(masked, depths, group_depths)
    aliases: list[tuple[str, str]] = []
    for match in re.finditer(r"\buse\b", masked):
        if (
            not rust_is_production_item_position(match.start(), depths, item_scopes)
            or group_depths[match.start()] != 0
        ):
            continue
        end = rust_same_depth_terminator(
            masked,
            depths,
            match.end(),
            depth=depths[match.start()],
            characters={";"},
            group_depths=group_depths,
        )
        use_surface = masked[match.end() : end]
        if re.search(r"\bas\b", use_surface) is None:
            continue
        for local_name, origin, wildcard in parse_rust_use_tree(
            use_surface
        ):
            if wildcard or local_name == "_":
                continue
            target = origin.removeprefix("::").split("::")[-1]
            if local_name != target:
                aliases.append((local_name, target))
    return aliases


def rust_reject_public_foreign_items(content: str) -> None:
    masked = strip_rust_comments_and_strings(content)
    if any(ord(character) > 127 for character in masked):
        raise ValueError(
            "non-ASCII Rust identifier syntax requires an explicit public API "
            "scanner policy"
        )
    depths = rust_brace_depths(masked)
    group_depths = rust_group_depths(masked)
    for foreign in re.finditer(
        r"\b(?:unsafe\s+)?extern(?:\s+\"[^\"]+\")?\s*\{",
        masked,
    ):
        if depths[foreign.start()] != 0 or group_depths[foreign.start()] != 0:
            continue
        open_brace = masked.find("{", foreign.start(), foreign.end())
        close_brace = rust_same_depth_terminator(
            masked,
            depths,
            open_brace + 1,
            depth=1,
            characters={"}"},
        )
        for public_match in UNRESTRICTED_PUBLIC_RE.finditer(
            masked, open_brace + 1, close_brace
        ):
            if (
                depths[public_match.start()] == 1
                and group_depths[public_match.start()] == 0
            ):
                raise ValueError(
                    "public foreign items require an explicit public API scanner policy"
                )


def rust_public_surface_anchor(
    prefix: str, kind: str, name: str, surface: str
) -> str:
    return f"{prefix}:{kind}:{name}:sha256:{rust_surface_digest(surface)}"


def rust_public_item_inventory_anchors(
    contents: tuple[str, ...],
) -> set[str]:
    """Fingerprint all production public items without tying them to file paths."""

    combined_content = "\n".join(contents)
    raw_anchors: list[str] = []
    for content in contents:
        for kind, name, surface in rust_top_level_public_items(content):
            raw_anchors.append(
                rust_public_surface_anchor(
                    "definition", kind, name, surface
                )
            )
            if "!" in surface and "macro_rules" in combined_content:
                raw_anchors.extend(
                    rust_local_macro_dependency_anchors(
                        combined_content,
                        surface,
                        f"crate-public-item:{kind}:{name}",
                    )
                )
    counts = Counter(raw_anchors)
    return {
        f"{anchor}:occurrence:{occurrence}"
        for anchor, count in counts.items()
        for occurrence in range(1, count + 1)
    }


def rust_public_inventory_owner(anchor: str) -> str | None:
    parts = anchor.split(":", maxsplit=3)
    if (
        len(parts) >= 3
        and parts[0] == "definition"
        and parts[1] in {"enum", "struct", "trait", "type", "union"}
    ):
        return parts[2]
    if len(parts) >= 2 and parts[0] in {"member", "trait-impl"}:
        return parts[1]
    return None


def canonical_public_export_origin(origin: str) -> str:
    normalized = origin.removeprefix("::")
    segments = normalized.split("::")
    if segments[0].startswith("lorepia_"):
        return normalized
    return f"local::{segments[-1]}"


def rust_facade_public_export_leaves(
    content: str,
) -> list[tuple[str, str, bool]]:
    masked = strip_rust_comments_and_strings(content)
    depths = rust_brace_depths(masked)
    group_depths = rust_group_depths(masked)
    leaves: list[tuple[str, str, bool]] = []
    for public_match in UNRESTRICTED_PUBLIC_RE.finditer(masked):
        if (
            depths[public_match.start()] != 0
            or group_depths[public_match.start()] != 0
        ):
            continue
        declaration_start = public_match.end()
        use_match = re.match(r"use\b", masked[declaration_start:])
        if use_match is None:
            continue
        body_start = declaration_start + use_match.end()
        end = rust_same_depth_terminator(
            masked, depths, body_start, depth=0, characters={";"}
        )
        leaves.extend(parse_rust_use_tree(masked[body_start:end]))
    return leaves


@lru_cache(maxsize=512)
def rust_workspace_macro_use_aliases(
    content: str,
) -> tuple[tuple[str, str], ...]:
    masked = strip_rust_comments_and_strings(content)
    depths = rust_brace_depths(masked)
    group_depths = rust_group_depths(masked)
    item_scopes = rust_production_item_scopes(masked, depths, group_depths)
    aliases: list[tuple[str, str]] = []
    for use_match in re.finditer(r"\buse\b", masked):
        if (
            not rust_is_production_item_position(
                use_match.start(), depths, item_scopes
            )
            or group_depths[use_match.start()] != 0
        ):
            continue
        end = rust_same_depth_terminator(
            masked,
            depths,
            use_match.end(),
            depth=depths[use_match.start()],
            characters={";"},
            group_depths=group_depths,
            group_depth=0,
        )
        use_tree = masked[use_match.end() : end].strip()
        for local_name, origin, wildcard in parse_rust_use_tree(use_tree):
            normalized_origin = origin.removeprefix("::")
            if (
                not wildcard
                and normalized_origin.split("::")[0].startswith("lorepia_")
            ):
                aliases.append((local_name, normalized_origin))
    return tuple(sorted(aliases))


def rust_facade_public_anchors(content: str) -> tuple[set[str], set[str]]:
    rust_reject_public_foreign_items(content)
    masked = strip_rust_comments_and_strings(content)
    has_local_macros = "macro_rules" in masked
    surface_source = strip_rust_comments_and_strings(content, strip_strings=False)
    depths = rust_brace_depths(masked)
    group_depths = rust_group_depths(masked)
    angle_depths = rust_angle_depths(masked, depths, group_depths)
    brace_macro_openers, brace_macro_closers = rust_brace_macro_delimiters(masked)
    anchors: set[str] = set()
    wildcards: set[str] = set()
    for match in UNRESTRICTED_PUBLIC_RE.finditer(masked):
        if depths[match.start()] != 0 or group_depths[match.start()] != 0:
            continue
        declaration_start = match.end()
        if re.match(r"use\b", masked[declaration_start:]) is not None:
            attribute_start = rust_outer_attribute_start(
                masked,
                depths,
                group_depths,
                match.start(),
                depth=0,
            )
            declaration_attributes = rust_surface_digest(
                surface_source[attribute_start : match.end()]
            )
            body_start = declaration_start + re.match(
                r"use\b", masked[declaration_start:]
            ).end()
            end = rust_same_depth_terminator(
                masked, depths, body_start, depth=0, characters={";"}
            )
            for public_name, origin, wildcard in parse_rust_use_tree(
                masked[body_start:end]
            ):
                if wildcard:
                    anchor = f"wildcard:{origin.removesuffix('::*')}"
                    anchors.add(anchor)
                    anchors.add(
                        f"wildcard-declaration:{origin.removesuffix('::*')}:"
                        f"sha256:{declaration_attributes}"
                    )
                    wildcards.add(anchor)
                elif public_name != "_":
                    canonical_origin = canonical_public_export_origin(origin)
                    anchors.add(
                        f"export:{public_name}<-{canonical_origin}:sha256:"
                        f"{declaration_attributes}"
                    )
            continue
        kind, name, surface = rust_public_item_surface(
            masked,
            surface_source,
            depths,
            group_depths,
            angle_depths,
            brace_macro_openers,
            brace_macro_closers,
            match,
            depth=0,
        )
        anchors.add(rust_public_surface_anchor("item", kind, name, surface))
        if has_local_macros:
            anchors.update(
                rust_local_macro_dependency_anchors(
                    content, surface, f"item:{kind}:{name}"
                )
            )
    return anchors, wildcards


def rust_facade_local_exported_symbols(content: str) -> set[str]:
    symbols: set[str] = set()
    for _, origin, wildcard in rust_facade_public_export_leaves(content):
        normalized = origin.removeprefix("::")
        if wildcard or normalized.split("::", maxsplit=1)[0].startswith(
            "lorepia_"
        ):
            continue
        symbols.add(normalized.rsplit("::", maxsplit=1)[-1])
    return symbols


def rust_skip_angle_group(
    content: str, spans: list[tuple[str, int, int]], start: int
) -> int:
    tokens = [token for token, _, _ in spans]
    if start >= len(tokens) or tokens[start] not in {"<", "<<"}:
        return start
    brace_depths = rust_brace_depths(content)
    group_depths = rust_group_depths(content)
    angle_depths = rust_angle_depths(content, brace_depths, group_depths)
    _, open_start, _ = spans[start]
    base_brace_depth = brace_depths[open_start]
    base_angle_depth = angle_depths[open_start]
    for index in range(start + 1, len(tokens)):
        token, token_start, token_end = spans[index]
        if (
            token in {">", ">>"}
            and brace_depths[token_start] == base_brace_depth
            and angle_depths[token_end] == base_angle_depth
        ):
            return index + 1
    raise ValueError("unterminated Rust angle-bracket group in public API surface")


def rust_impl_owner_and_trait(header: str) -> tuple[str | None, str | None]:
    spans = rust_surface_token_spans(header)
    tokens = [token for token, _, _ in spans]
    brace_depths = rust_brace_depths(header)
    group_depths = rust_group_depths(header)
    angle_depths = rust_angle_depths(header, brace_depths, group_depths)
    try:
        cursor = tokens.index("impl") + 1
    except ValueError as error:
        raise ValueError(f"missing impl keyword: {header[:120]}") from error
    cursor = rust_skip_angle_group(header, spans, cursor)

    for_positions: list[int] = []
    where_index = len(tokens)
    for index in range(cursor, len(tokens)):
        token, start, _ = spans[index]
        is_top_level = (
            brace_depths[start] == 0
            and group_depths[start] == 0
            and angle_depths[start] == 0
        )
        if is_top_level and token == "where":
            where_index = index
            break
        if is_top_level and token == "for":
            for_positions.append(index)

    trait_for = for_positions[-1] if for_positions else None
    target_start = trait_for + 1 if trait_for is not None else cursor
    target_tokens = tokens[target_start:where_index]
    if trait_for is None and target_tokens[:1] == ["dyn"]:
        target_tokens = target_tokens[1:]
        if "+" in target_tokens:
            target_tokens = target_tokens[: target_tokens.index("+")]
    owner_tokens: list[str] = []
    for token in target_tokens:
        if token in {"<", "<<"}:
            break
        if re.fullmatch(
            r"r#[A-Za-z_][A-Za-z0-9_]*|[A-Za-z_][A-Za-z0-9_]*",
            token,
        ):
            owner_tokens.append(token)
    trait_name = (
        " ".join(tokens[cursor:trait_for]) if trait_for is not None else None
    )
    return (owner_tokens[-1] if owner_tokens else None), trait_name


def rust_canonical_inherent_impl_context(
    masked_context: str,
    surface_context: str,
    owner: str,
    canonical_owner: str | None = None,
) -> str:
    spans = rust_surface_token_spans(masked_context)
    tokens = [token for token, _, _ in spans]
    try:
        cursor = tokens.index("impl") + 1
    except ValueError as error:
        raise ValueError(f"missing impl keyword for public member owner {owner}")
    cursor = rust_skip_angle_group(masked_context, spans, cursor)
    if cursor >= len(tokens):
        raise ValueError(f"unsupported impl target for public member owner {owner}")
    target_start = cursor
    observed_owner_index: int | None = None
    if tokens[cursor] == "dyn":
        cursor += 1
    while cursor < len(tokens) and tokens[cursor] not in {
        "+",
        "<",
        "<<",
        "where",
    }:
        if re.fullmatch(
            r"r#[A-Za-z_][A-Za-z0-9_]*|[A-Za-z_][A-Za-z0-9_]*",
            tokens[cursor],
        ):
            observed_owner_index = cursor
        cursor += 1
    if observed_owner_index is None or tokens[observed_owner_index] != owner:
        observed = " ".join(tokens[target_start:cursor])
        raise ValueError(
            f"impl target {observed} does not end in public owner {owner}"
        )
    path_start = observed_owner_index
    while (
        path_start >= target_start + 2
        and tokens[path_start - 1] == "::"
        and re.fullmatch(
            r"r#[A-Za-z_][A-Za-z0-9_]*|[A-Za-z_][A-Za-z0-9_]*",
            tokens[path_start - 2],
        )
    ):
        path_start -= 2
    if path_start > target_start and tokens[path_start - 1] == "::":
        path_start -= 1
    replace_start = spans[path_start][1]
    replace_end = spans[observed_owner_index][2]
    return (
        surface_context[:replace_start]
        + (canonical_owner or owner)
        + surface_context[replace_end:]
    )


def rust_canonical_trait_impl_context(
    masked_context: str,
    surface_context: str,
    owner: str,
    canonical_owner: str | None = None,
) -> str:
    spans = rust_surface_token_spans(masked_context)
    tokens = [token for token, _, _ in spans]
    brace_depths = rust_brace_depths(masked_context)
    group_depths = rust_group_depths(masked_context)
    angle_depths = rust_angle_depths(masked_context, brace_depths, group_depths)
    try:
        cursor = tokens.index("impl") + 1
    except ValueError as error:
        raise ValueError(f"missing trait impl keyword for public owner {owner}") from error
    cursor = rust_skip_angle_group(masked_context, spans, cursor)
    for_positions: list[int] = []
    for index in range(cursor, len(tokens)):
        token, start, _ = spans[index]
        if (
            token == "where"
            and brace_depths[start] == 0
            and group_depths[start] == 0
            and angle_depths[start] == 0
        ):
            break
        if (
            token == "for"
            and brace_depths[start] == 0
            and group_depths[start] == 0
            and angle_depths[start] == 0
        ):
            for_positions.append(index)
    if not for_positions:
        raise ValueError(f"missing trait target for public owner {owner}")
    target_start = for_positions[-1] + 1
    cursor = target_start
    observed_owner_index: int | None = None
    while cursor < len(tokens) and tokens[cursor] not in {"<", "<<", "where"}:
        if re.fullmatch(
            r"r#[A-Za-z_][A-Za-z0-9_]*|[A-Za-z_][A-Za-z0-9_]*",
            tokens[cursor],
        ):
            observed_owner_index = cursor
        cursor += 1
    if observed_owner_index is None or tokens[observed_owner_index] != owner:
        observed = " ".join(tokens[target_start:cursor])
        raise ValueError(
            f"trait impl target {observed} does not end in public owner {owner}"
        )
    path_start = observed_owner_index
    while (
        path_start >= target_start + 2
        and tokens[path_start - 1] == "::"
        and re.fullmatch(
            r"r#[A-Za-z_][A-Za-z0-9_]*|[A-Za-z_][A-Za-z0-9_]*",
            tokens[path_start - 2],
        )
    ):
        path_start -= 2
    if path_start > target_start and tokens[path_start - 1] == "::":
        path_start -= 1
    replace_start = spans[path_start][1]
    replace_end = spans[observed_owner_index][2]
    return (
        surface_context[:replace_start]
        + (canonical_owner or owner)
        + surface_context[replace_end:]
    )


def rust_trait_impl_referenced_owners(
    header: str, exported_owners: set[str]
) -> set[str]:
    spans = rust_surface_token_spans(header)
    tokens = [token for token, _, _ in spans]
    brace_depths = rust_brace_depths(header)
    group_depths = rust_group_depths(header)
    angle_depths = rust_angle_depths(header, brace_depths, group_depths)
    try:
        cursor = tokens.index("impl") + 1
    except ValueError as error:
        raise ValueError("missing trait impl keyword") from error
    cursor = rust_skip_angle_group(header, spans, cursor)
    where_index = len(tokens)
    for_positions: list[int] = []
    for index in range(cursor, len(tokens)):
        token, start, _ = spans[index]
        if (
            brace_depths[start] == 0
            and group_depths[start] == 0
            and angle_depths[start] == 0
        ):
            if token == "where":
                where_index = index
                break
            if token == "for":
                for_positions.append(index)
    if not for_positions:
        return set()
    trait_for = for_positions[-1]
    return {
        token
        for token in (*tokens[cursor:trait_for], *tokens[trait_for + 1 : where_index])
        if token in exported_owners
    }


def rust_canonical_public_owner_paths(
    masked_context: str,
    surface_context: str,
    owner_aliases: dict[str, str],
) -> str:
    spans = rust_surface_token_spans(masked_context)
    tokens = [token for token, _, _ in spans]
    try:
        impl_index = tokens.index("impl")
    except ValueError as error:
        raise ValueError("missing impl keyword for public owner canonicalization") from error
    replacements: list[tuple[int, int, str]] = []
    for index in range(impl_index + 1, len(tokens)):
        token = tokens[index]
        canonical_owner = owner_aliases.get(token)
        if canonical_owner is None:
            continue
        path_start = index
        while (
            path_start >= impl_index + 3
            and tokens[path_start - 1] == "::"
            and re.fullmatch(
                r"r#[A-Za-z_][A-Za-z0-9_]*|[A-Za-z_][A-Za-z0-9_]*",
                tokens[path_start - 2],
            )
        ):
            path_start -= 2
        if path_start > impl_index + 1 and tokens[path_start - 1] == "::":
            path_start -= 1
        replacements.append(
            (spans[path_start][1], spans[index][2], canonical_owner)
        )
    canonical = surface_context
    last_start = len(surface_context)
    for start, end, replacement in sorted(replacements, reverse=True):
        if end > last_start:
            continue
        canonical = canonical[:start] + replacement + canonical[end:]
        last_start = start
    return canonical


def rust_trait_impl_associated_anchors(
    content: str,
    masked: str,
    surface_source: str,
    depths: list[int],
    group_depths: list[int],
    open_brace: int,
    close_brace: int,
    *,
    has_local_macros: bool,
    impl_depth: int,
    owner: str,
    trait_digest: str,
) -> set[str]:
    anchors: set[str] = set()
    associated_re = re.compile(
        r"\b(?P<kind>type|const)\s+"
        r"(?P<name>r#[A-Za-z_][A-Za-z0-9_]*|[A-Za-z_][A-Za-z0-9_]*)"
    )
    for match in associated_re.finditer(masked, open_brace + 1, close_brace):
        if (
            depths[match.start()] != impl_depth + 1
            or group_depths[match.start()] != 0
        ):
            continue
        surface_start = rust_outer_attribute_start(
            masked,
            depths,
            group_depths,
            match.start(),
            depth=impl_depth + 1,
        )
        boundary = surface_start
        while boundary > open_brace + 1 and masked[boundary - 1].isspace():
            boundary -= 1
        if boundary > open_brace + 1 and masked[boundary - 1] not in {";", "}"}:
            continue
        end = next(
            (
                index
                for index in range(match.end(), close_brace)
                if masked[index] == ";"
                and depths[index] == impl_depth + 1
                and group_depths[index] == 0
            ),
            None,
        )
        if end is None:
            raise ValueError(
                f"unterminated associated {match.group('kind')} on trait impl {owner}"
            )
        surface = surface_source[surface_start : end + 1]
        context = (
            f"{owner}:trait-impl:associated:{match.group('kind')}:"
            f"{match.group('name')}"
        )
        anchors.add(
            f"trait-impl:{owner}:sha256:{trait_digest}:associated:"
            f"{match.group('kind')}:{match.group('name')}:sha256:"
            f"{rust_surface_digest(surface)}"
        )
        if has_local_macros:
            anchors.update(
                rust_local_macro_dependency_anchors(content, surface, context)
            )
    return anchors


@lru_cache(maxsize=256)
def rust_top_level_macro_definitions(
    content: str,
) -> tuple[tuple[str, str, str, bool], ...]:
    masked = strip_rust_comments_and_strings(content)
    surface_source = strip_rust_comments_and_strings(content, strip_strings=False)
    depths = rust_brace_depths(masked)
    group_depths = rust_group_depths(masked)
    brace_macro_openers, brace_macro_closers = rust_brace_macro_delimiters(masked)
    brace_macro_ranges = rust_paired_brace_macro_ranges(
        depths, brace_macro_openers, brace_macro_closers
    )
    test_module_ranges = [
        (start, end)
        for start, end, _, is_test in rust_inline_module_scopes(
            masked, depths, group_depths, brace_macro_ranges
        )
        if is_test
    ]
    delimiter_pairs = {"(": ")", "[": "]", "{": "}"}

    def matching_delimiter(open_index: int) -> int:
        stack = [masked[open_index]]
        for index in range(open_index + 1, len(masked)):
            character = masked[index]
            if character in delimiter_pairs:
                stack.append(character)
            elif character in delimiter_pairs.values():
                if not stack or delimiter_pairs[stack[-1]] != character:
                    raise ValueError("unbalanced local macro definition delimiter")
                stack.pop()
                if not stack:
                    return index
        raise ValueError("unterminated local macro definition")

    definitions: list[tuple[str, str, str, bool]] = []
    for definition in re.finditer(
        r"\bmacro_rules\s*!\s*(?P<name>r#[A-Za-z_][A-Za-z0-9_]*|"
        r"[A-Za-z_][A-Za-z0-9_]*)\s*"
        r"(?P<open>[({[])",
        masked,
    ):
        if group_depths[definition.start()] != 0 or any(
            start <= definition.start() < end
            for start, end in test_module_ranges
        ):
            continue
        open_index = definition.end("open") - 1
        close_index = matching_delimiter(open_index)
        surface_start = rust_outer_attribute_start(
            masked,
            depths,
            group_depths,
            definition.start(),
            depth=depths[definition.start()],
        )
        definition_surface = surface_source[surface_start : close_index + 1]
        attributes = masked[surface_start : definition.start()]
        exported = (
            re.search(r"#\s*\[\s*macro_export(?:\s*\([^]]*\))?\s*\]", attributes)
            is not None
        )
        definitions.append(
            (
                definition.group("name"),
                rust_surface_digest(definition_surface),
                definition_surface,
                exported,
            )
        )
    return tuple(definitions)


@lru_cache(maxsize=256)
def rust_local_macro_summaries(
    content: str,
) -> tuple[tuple[str, str, bool, tuple[str, ...]], ...]:
    """Return deterministic, transitive summaries for local macro definitions."""

    records: dict[str, list[tuple[str, str]]] = {}
    for name, digest, surface, _ in rust_top_level_macro_definitions(content):
        records.setdefault(name, []).append((digest, surface))
    if not records:
        return ()

    invocation_re = re.compile(
        r"(?P<path>(?:::)?(?:(?:r#[A-Za-z_][A-Za-z0-9_]*|"
        r"[A-Za-z_][A-Za-z0-9_]*)::)*(?:r#[A-Za-z_][A-Za-z0-9_]*|"
        r"[A-Za-z_][A-Za-z0-9_]*))\s*!\s*[({[]"
    )
    known_names = set(records)
    base: dict[str, tuple[str, bool, set[str], set[str]]] = {}
    for name, candidates in records.items():
        candidate_digests = sorted(digest for digest, _ in candidates)
        digest = (
            candidate_digests[0]
            if len(candidate_digests) == 1
            else rust_surface_digest(" ".join(candidate_digests))
        )
        emits_literal_public = False
        fixed_owners: set[str] = set()
        dependencies: set[str] = set()
        for _, surface in candidates:
            masked_surface = strip_rust_comments_and_strings(surface)
            emits_literal_public = (
                emits_literal_public
                or UNRESTRICTED_PUBLIC_RE.search(masked_surface) is not None
            )
            dependencies.update(
                terminal
                for match in invocation_re.finditer(masked_surface)
                if (
                    terminal := match.group("path").split("::")[-1]
                ) in known_names
            )
            for public_match in UNRESTRICTED_PUBLIC_RE.finditer(masked_surface):
                declaration = re.match(
                    r"\s*(?:struct|enum|mod|trait|union|type)\s+"
                    r"(?P<name>r#[A-Za-z_][A-Za-z0-9_]*|"
                    r"[A-Za-z_][A-Za-z0-9_]*)",
                    masked_surface[public_match.end() :],
                )
                if declaration is not None:
                    fixed_owners.add(declaration.group("name"))
        base[name] = (
            digest,
            emits_literal_public,
            fixed_owners,
            dependencies,
        )

    summaries: list[tuple[str, str, bool, tuple[str, ...]]] = []
    for name in sorted(base):
        reachable: set[str] = set()
        pending = [name]
        while pending:
            candidate = pending.pop()
            if candidate in reachable:
                continue
            reachable.add(candidate)
            pending.extend(base[candidate][3] - reachable)
        reachable_digests = sorted(base[candidate][0] for candidate in reachable)
        digest = (
            reachable_digests[0]
            if len(reachable_digests) == 1
            else rust_surface_digest(" ".join(reachable_digests))
        )
        emits_literal_public = any(base[candidate][1] for candidate in reachable)
        fixed_owners = sorted(
            owner
            for candidate in reachable
            for owner in base[candidate][2]
        )
        summaries.append(
            (name, digest, emits_literal_public, tuple(sorted(set(fixed_owners))))
        )
    return tuple(summaries)


def rust_local_macro_dependency_anchors(
    content: str, surface: str, context: str
) -> set[str]:
    definitions: dict[str, list[tuple[str, str]]] = {}
    for name, digest, definition_surface, _ in rust_top_level_macro_definitions(
        content
    ):
        definitions.setdefault(name, []).append((digest, definition_surface))
    if not definitions:
        return set()

    invocation_re = re.compile(
        r"(?P<path>(?:::)?(?:(?:r#[A-Za-z_][A-Za-z0-9_]*|"
        r"[A-Za-z_][A-Za-z0-9_]*)::)*(?:r#[A-Za-z_][A-Za-z0-9_]*|"
        r"[A-Za-z_][A-Za-z0-9_]*))\s*!\s*[({[]"
    )

    def references(candidate: str) -> set[str]:
        masked_candidate = strip_rust_comments_and_strings(candidate)
        return {
            match.group("path").split("::")[-1]
            for match in invocation_re.finditer(masked_candidate)
            if match.group("path").split("::")[-1] in definitions
        }

    pending = list(references(surface))
    visited: set[str] = set()
    anchors: set[str] = set()
    while pending:
        name = pending.pop()
        if name in visited:
            continue
        visited.add(name)
        for digest, definition_surface in definitions[name]:
            anchors.add(
                f"macro-dependency:{context}:{name}:sha256:{digest}"
            )
            pending.extend(references(definition_surface) - visited)
    return anchors


def rust_public_exported_macro_definition_anchors(content: str) -> set[str]:
    return {
        f"macro-export-definition:{name}:sha256:{digest}"
        for name, digest, _, exported in rust_top_level_macro_definitions(content)
        if exported
    }


def rust_direct_scope_macro_anchors(
    content: str,
    masked: str,
    surface_source: str,
    depths: list[int],
    group_depths: list[int],
    start: int,
    end: int,
    *,
    context: str,
    depth: int,
) -> set[str]:
    invocation_re = re.compile(
        r"(?P<path>(?:::)?(?:(?:r#[A-Za-z_][A-Za-z0-9_]*|"
        r"[A-Za-z_][A-Za-z0-9_]*)::)*(?:r#[A-Za-z_][A-Za-z0-9_]*|"
        r"[A-Za-z_][A-Za-z0-9_]*))\s*!\s*(?P<open>[({[])"
    )
    delimiter_pairs = {"(": ")", "[": "]", "{": "}"}
    definitions = {
        name: digest
        for name, digest, _, _ in rust_local_macro_summaries(content)
    }

    anchors: set[str] = set()
    for invocation in invocation_re.finditer(masked, start, end):
        if (
            depths[invocation.start()] != depth
            or group_depths[invocation.start()] != 0
            or invocation.group("path") == "macro_rules"
        ):
            continue
        open_index = invocation.end("open") - 1
        stack = [masked[open_index]]
        close_index: int | None = None
        for index in range(open_index + 1, end):
            character = masked[index]
            if character in delimiter_pairs:
                stack.append(character)
            elif character in delimiter_pairs.values():
                if not stack or delimiter_pairs[stack[-1]] != character:
                    raise ValueError("unbalanced direct-scope macro invocation")
                stack.pop()
                if not stack:
                    close_index = index
                    break
        if close_index is None:
            raise ValueError("unterminated direct-scope macro invocation")
        surface_start = rust_outer_attribute_start(
            masked,
            depths,
            group_depths,
            invocation.start(),
            depth=depth,
        )
        macro_name = invocation.group("path").split("::")[-1]
        definition_digest = definitions.get(macro_name, "external")
        digest = rust_surface_digest(
            f"{definition_digest} "
            f"{surface_source[surface_start : close_index + 1]}"
        )
        anchors.add(
            f"direct-macro:{context}:{macro_name}:sha256:{digest}"
        )
    return anchors


def rust_primary_type_public_anchors(
    content: str,
    exported_owners: set[str],
    owner_aliases: dict[str, str] | None = None,
) -> set[str]:
    masked = strip_rust_comments_and_strings(content)
    has_local_macros = "macro_rules" in masked
    surface_source = strip_rust_comments_and_strings(content, strip_strings=False)
    depths = rust_brace_depths(masked)
    group_depths = rust_group_depths(masked)
    angle_depths = rust_angle_depths(masked, depths, group_depths)
    brace_macro_openers, brace_macro_closers = rust_brace_macro_delimiters(masked)
    token_spans = rust_surface_token_spans(masked)
    token_indices = {start: index for index, (_, start, _) in enumerate(token_spans)}
    brace_macro_ranges = rust_paired_brace_macro_ranges(
        depths, brace_macro_openers, brace_macro_closers
    )
    test_module_ranges = [
        (start, end)
        for start, end, _, is_test in rust_inline_module_scopes(
            masked, depths, group_depths, brace_macro_ranges
        )
        if is_test
    ]
    anchors: set[str] = set()
    for impl_match in re.finditer(r"\bimpl\b", masked):
        impl_depth = depths[impl_match.start()]
        if group_depths[impl_match.start()] != 0 or any(
            opener < impl_match.start() < closer
            for opener, closer in brace_macro_ranges
        ) or any(
            start <= impl_match.start() < end
            for start, end in test_module_ranges
        ):
            continue
        token_index = token_indices.get(impl_match.start())
        if token_index is None:
            continue
        boundary_index = token_index - 1
        while (
            boundary_index >= 0
            and token_spans[boundary_index][0] in {"const", "default", "unsafe"}
        ):
            boundary_index -= 1
        if (
            boundary_index >= 0
            and token_spans[boundary_index][0] not in {";", "[", "]", "{", "}"}
        ):
            continue
        impl_item_start = (
            token_spans[boundary_index + 1][1]
            if boundary_index + 1 < token_index
            else impl_match.start()
        )
        open_brace = rust_same_depth_terminator(
            masked,
            depths,
            impl_match.end(),
            depth=impl_depth,
            characters={"{", ";"},
            group_depths=group_depths,
            angle_depths=angle_depths,
            ignored_indices=brace_macro_openers,
        )
        if masked[open_brace] != "{":
            continue
        header = " ".join(masked[impl_match.start() : open_brace].split())
        observed_owner, trait_name = rust_impl_owner_and_trait(header)
        close_brace = rust_same_depth_terminator(
            masked,
            depths,
            open_brace + 1,
            depth=impl_depth + 1,
            characters={"}"},
        )
        if trait_name is not None:
            referenced_owners = rust_trait_impl_referenced_owners(
                header, exported_owners
            )
            if not referenced_owners:
                continue
            surface_start = rust_outer_attribute_start(
                masked,
                depths,
                group_depths,
                impl_item_start,
                depth=impl_depth,
            )
            canonical_paths = {
                owner: (owner_aliases or {}).get(owner, owner)
                for owner in exported_owners
            }
            trait_context = rust_canonical_public_owner_paths(
                masked[surface_start:open_brace],
                surface_source[surface_start:open_brace],
                canonical_paths,
            )
            trait_digest = rust_surface_digest(
                trait_context
            )
            canonical_owners = {
                canonical_paths[owner] for owner in referenced_owners
            }
            for canonical_owner in sorted(canonical_owners):
                anchors.add(
                    f"trait-impl:{canonical_owner}:sha256:{trait_digest}"
                )
                if "!" in masked[open_brace + 1 : close_brace]:
                    anchors.update(
                        rust_direct_scope_macro_anchors(
                            content,
                            masked,
                            surface_source,
                            depths,
                            group_depths,
                            open_brace + 1,
                            close_brace,
                            context=f"{canonical_owner}:trait-impl",
                            depth=impl_depth + 1,
                        )
                    )
                if has_local_macros:
                    anchors.update(
                        rust_local_macro_dependency_anchors(
                            content,
                            surface_source[surface_start:open_brace],
                            f"{canonical_owner}:trait-impl",
                        )
                    )
                anchors.update(
                    rust_trait_impl_associated_anchors(
                        content,
                        masked,
                        surface_source,
                        depths,
                        group_depths,
                        open_brace,
                        close_brace,
                        has_local_macros=has_local_macros,
                        impl_depth=impl_depth,
                        owner=canonical_owner,
                        trait_digest=trait_digest,
                    )
                )
            continue
        if observed_owner is None or observed_owner not in exported_owners:
            continue
        canonical_owner = (owner_aliases or {}).get(observed_owner, observed_owner)
        impl_surface_start = rust_outer_attribute_start(
            masked,
            depths,
            group_depths,
            impl_item_start,
            depth=impl_depth,
        )
        impl_context = rust_canonical_inherent_impl_context(
            masked[impl_surface_start:open_brace],
            surface_source[impl_surface_start:open_brace],
            observed_owner,
            canonical_owner,
        )
        if "!" in masked[open_brace + 1 : close_brace]:
            anchors.update(
                rust_direct_scope_macro_anchors(
                    content,
                    masked,
                    surface_source,
                    depths,
                    group_depths,
                    open_brace + 1,
                    close_brace,
                    context=f"{canonical_owner}:member",
                    depth=impl_depth + 1,
                )
            )
        for public_match in UNRESTRICTED_PUBLIC_RE.finditer(
            masked, open_brace + 1, close_brace
        ):
            if (
                depths[public_match.start()] != impl_depth + 1
                or group_depths[public_match.start()] != 0
            ):
                continue
            kind, name, surface = rust_public_item_surface(
                masked,
                surface_source,
                depths,
                group_depths,
                angle_depths,
                brace_macro_openers,
                brace_macro_closers,
                public_match,
                depth=impl_depth + 1,
            )
            member_context = f"{canonical_owner}:member:{kind}:{name}"
            member_surface = f"{impl_context} {{ {surface}"
            anchors.add(
                rust_public_surface_anchor(
                    f"member:{canonical_owner}", kind, name, member_surface
                )
            )
            if has_local_macros:
                anchors.update(
                    rust_local_macro_dependency_anchors(
                        content, member_surface, member_context
                    )
                )
    return anchors


@lru_cache(maxsize=512)
def rust_public_macro_invocation_anchors(
    content: str,
    *,
    fail_unresolved: bool = False,
) -> tuple[set[str], set[str]]:
    masked = strip_rust_comments_and_strings(content)
    surface_source = strip_rust_comments_and_strings(content, strip_strings=False)
    depths = rust_brace_depths(masked)
    group_depths = rust_group_depths(masked)
    brace_macro_openers, brace_macro_closers = rust_brace_macro_delimiters(masked)
    brace_macro_ranges = rust_paired_brace_macro_ranges(
        depths, brace_macro_openers, brace_macro_closers
    )
    delimiter_pairs = {"(": ")", "[": "]", "{": "}"}

    module_scopes = rust_inline_module_scopes(
        masked, depths, group_depths, brace_macro_ranges
    )
    item_scopes: list[tuple[int, int, int]] = [(0, len(masked), 0)]
    item_scopes.extend(
        (start, end, depth)
        for start, end, depth, is_test in module_scopes
        if not is_test
    )

    def is_item_scope(position: int) -> bool:
        return any(
            start <= position < end and depths[position] == depth
            for start, end, depth in item_scopes
        )

    def matching_delimiter(open_index: int) -> int:
        stack = [masked[open_index]]
        for index in range(open_index + 1, len(masked)):
            character = masked[index]
            if character in delimiter_pairs:
                stack.append(character)
            elif character in delimiter_pairs.values():
                if not stack or delimiter_pairs[stack[-1]] != character:
                    raise ValueError("unbalanced public macro invocation delimiter")
                stack.pop()
                if not stack:
                    return index
        raise ValueError("unterminated public macro invocation")

    local_macros = {
        name: (digest, emits_literal_public, set(fixed_owners))
        for name, digest, emits_literal_public, fixed_owners
        in rust_local_macro_summaries(content)
    }
    exporting_macros = {
        name: digest
        for name, (digest, emits_literal_public, _) in local_macros.items()
        if emits_literal_public
    }
    workspace_alias_records: dict[str, set[str]] = {}
    for alias, origin in rust_workspace_macro_use_aliases(content):
        workspace_alias_records.setdefault(alias, set()).add(origin)
    def workspace_macro_path(macro_path: str, macro_name: str) -> str | None:
        normalized_path = macro_path.removeprefix("::")
        segments = normalized_path.split("::")
        if segments[0].startswith("lorepia_"):
            return normalized_path
        origins = workspace_alias_records.get(segments[0], set())
        if len(origins) > 1:
            raise ValueError(
                f"workspace macro alias {segments[0]} has ambiguous "
                f"bindings: {sorted(origins)}"
            )
        if origins:
            origin = next(iter(origins))
            return "::".join((origin, *segments[1:]))
        return None

    invocations: list[tuple[str, str, int, int, int]] = []
    for invocation in re.finditer(
        r"(?P<path>(?:::)?(?:(?:r#[A-Za-z_][A-Za-z0-9_]*|"
        r"[A-Za-z_][A-Za-z0-9_]*)::)*(?:r#[A-Za-z_][A-Za-z0-9_]*|"
        r"[A-Za-z_][A-Za-z0-9_]*))\s*!\s*(?P<open>[({[])",
        masked,
    ):
        if (
            not is_item_scope(invocation.start())
            or group_depths[invocation.start()] != 0
            or invocation.group("path") == "macro_rules"
        ):
            continue
        open_index = invocation.end("open") - 1
        invocation_surface_start = rust_outer_attribute_start(
            masked,
            depths,
            group_depths,
            invocation.start(),
            depth=depths[invocation.start()],
        )
        invocations.append(
            (
                invocation.group("path"),
                invocation.group("path").split("::")[-1],
                invocation_surface_start,
                open_index,
                matching_delimiter(open_index),
            )
        )

    anchors: set[str] = set()
    generated_owners: set[str] = set()
    for macro_path, macro_name, invocation_start, _, close_index in invocations:
        is_bare_local = (
            macro_path.removeprefix("::") == macro_name
            and workspace_macro_path(macro_path, macro_name) is None
        )
        definition_digest = (
            local_macros.get(macro_name, ("external", False, set()))[0]
            if is_bare_local
            else "external"
        )
        invocation_digest = rust_surface_digest(
            f"{definition_digest} "
            f"{surface_source[invocation_start : close_index + 1]}"
        )
        anchors.add(
            f"macro-top-level-invocation:{macro_name}:sha256:"
            f"{invocation_digest}"
        )
    for macro_name in sorted(exporting_macros):
        for macro_path, name, invocation_start, open_index, close_index in invocations:
            if (
                name != macro_name
                or macro_path.removeprefix("::") != macro_name
                or workspace_macro_path(macro_path, name) is not None
            ):
                continue
            arguments = masked[open_index + 1 : close_index]
            residue = re.sub(
                r"r#[A-Za-z_][A-Za-z0-9_]*|[A-Za-z_][A-Za-z0-9_]*|[\s,]",
                "",
                arguments,
            )
            if residue:
                raise ValueError(
                    f"unsupported public macro invocation arguments for {macro_name}: "
                    f"{residue[:40]}"
                )
            identifiers = set(re.findall(
                r"r#[A-Za-z_][A-Za-z0-9_]*|[A-Za-z_][A-Za-z0-9_]*",
                arguments,
            )) | local_macros[macro_name][2]
            if not identifiers:
                identifiers = {"unknown"}
            for identifier in sorted(identifiers):
                if identifier != "unknown":
                    generated_owners.add(identifier)
                invocation_digest = rust_surface_digest(
                    f"{exporting_macros[macro_name]} "
                    f"{surface_source[invocation_start : open_index + 1]} "
                    f"{identifier} {surface_source[close_index]}"
                )
                anchors.add(
                    f"macro-export:{macro_name}:{identifier}:sha256:"
                    f"{invocation_digest}"
                )

    for macro_path, macro_name, invocation_start, open_index, close_index in invocations:
        arguments = masked[open_index + 1 : close_index]
        if UNRESTRICTED_PUBLIC_RE.search(arguments) is None:
            continue
        definition_digest = (
            local_macros.get(macro_name, ("external", False, set()))[0]
            if macro_path.removeprefix("::") == macro_name
            and workspace_macro_path(macro_path, macro_name) is None
            else "external"
        )
        invocation_surface = surface_source[invocation_start : close_index + 1]
        invocation_digest = rust_surface_digest(
            f"{definition_digest} {invocation_surface}"
        )
        invocation_owners: set[str] = set()
        for public_match in UNRESTRICTED_PUBLIC_RE.finditer(arguments):
            declaration = re.match(
                r"\s*(?:(?P<kind>struct|enum|union|type|fn|mod|trait|const|static)\s+)?"
                r"(?P<name>r#[A-Za-z_][A-Za-z0-9_]*|[A-Za-z_][A-Za-z0-9_]*)",
                arguments[public_match.end() :],
            )
            if declaration is not None and declaration.group("kind") in {
                None,
                "struct",
                "enum",
                "mod",
                "trait",
                "union",
                "type",
            }:
                invocation_owners.add(declaration.group("name"))
        generated_owners.update(invocation_owners)
        for owner in sorted(invocation_owners or {"unknown"}):
            anchors.add(
                f"macro-public-invocation:{macro_name}:{owner}:sha256:"
                f"{invocation_digest}"
            )

    for macro_path, macro_name, invocation_start, open_index, close_index in invocations:
        normalized_path = workspace_macro_path(macro_path, macro_name)
        if normalized_path is None:
            continue
        arguments = masked[open_index + 1 : close_index]
        residue = re.sub(
            r"r#[A-Za-z_][A-Za-z0-9_]*|[A-Za-z_][A-Za-z0-9_]*|[\s,]",
            "",
            arguments,
        )
        identifiers = set(
            re.findall(
                r"r#[A-Za-z_][A-Za-z0-9_]*|[A-Za-z_][A-Za-z0-9_]*",
                arguments,
            )
        )
        if residue or not identifiers:
            raise ValueError(
                f"workspace item macro {macro_path} requires a reviewed owner "
                "mapping for public API scanning"
            )
        invocation_digest = rust_surface_digest(
            surface_source[invocation_start : close_index + 1]
        )
        anchor_path = normalized_path.replace("::", "/")
        for identifier in sorted(identifiers):
            generated_owners.add(identifier)
            anchors.add(
                f"macro-workspace-owner:{anchor_path}:{identifier}:sha256:"
                f"{invocation_digest}"
            )
    if fail_unresolved:
        safe_builtin_macros = {
            "cfg",
            "column",
            "concat",
            "env",
            "file",
            "format_args",
            "include",
            "include_bytes",
            "include_str",
            "line",
            "module_path",
            "option_env",
            "stringify",
            "thread_local",
        }
        for macro_path, macro_name, _, open_index, close_index in invocations:
            arguments = masked[open_index + 1 : close_index]
            if (
                macro_name in safe_builtin_macros
                or (
                    macro_path.removeprefix("::") == macro_name
                    and macro_name in local_macros
                )
                or workspace_macro_path(macro_path, macro_name) is not None
                or UNRESTRICTED_PUBLIC_RE.search(arguments) is not None
            ):
                continue
            raise ValueError(
                f"unresolved production item macro {macro_path}! requires a "
                "reviewed public API owner mapping"
            )
    return anchors, generated_owners


def rust_simple_type_alias_target(surface: str) -> str | None:
    tokens = rust_surface_tokens(surface)
    if "=" not in tokens:
        return None
    rhs = tokens[tokens.index("=") + 1 :]
    if not rhs or rhs[0] in {
        "&",
        "*",
        "(",
        "[",
        "<",
        "dyn",
        "fn",
        "impl",
    }:
        return None
    cursor = 1 if rhs[0] == "::" else 0
    names: list[str] = []
    expect_name = True
    while cursor < len(rhs):
        token = rhs[cursor]
        if token in {"<", ";", "where"}:
            break
        if expect_name:
            if re.fullmatch(
                r"r#[A-Za-z_][A-Za-z0-9_]*|[A-Za-z_][A-Za-z0-9_]*",
                token,
            ) is None:
                return None
            names.append(token)
        elif token != "::":
            return None
        expect_name = not expect_name
        cursor += 1
    if expect_name or not names:
        return None
    return names[-1] if names else None


def rust_external_module_source(
    source_root: Path,
    declaring_source: Path,
    module_name: str,
    surface: str,
) -> Path | None:
    if not surface.rstrip().endswith(";"):
        return None
    path_attribute = re.search(
        r"#\s*\[\s*path\s*=\s*\"(?P<path>[^\"]+)\"\s*\]",
        surface,
    )
    if path_attribute is not None:
        relative = Path(path_attribute.group("path"))
        if relative.is_absolute() or ".." in relative.parts:
            raise ValueError(f"invalid public module path attribute: {relative}")
        candidates = [declaring_source.parent / relative]
    else:
        module_parent = (
            declaring_source.parent
            if declaring_source.name in {"lib.rs", "mod.rs"}
            else declaring_source.with_suffix("")
        )
        candidates = [
            module_parent / f"{module_name}.rs",
            module_parent / module_name / "mod.rs",
        ]
    matches = [
        candidate
        for candidate in candidates
        if candidate.is_file() and candidate.is_relative_to(source_root)
    ]
    if len(matches) != 1:
        raise ValueError(
            f"public module {module_name} from {declaring_source} must resolve to "
            f"exactly one source file; found {len(matches)}"
        )
    return matches[0]


def rust_reject_unconditional_test_module_includes(
    root: Path,
    source_root: Path,
    declaring_source: Path,
    content: str,
) -> None:
    masked = strip_rust_comments_and_strings(content)
    surface_source = strip_rust_comments_and_strings(content, strip_strings=False)
    depths = rust_brace_depths(masked)
    group_depths = rust_group_depths(masked)
    item_scopes = rust_production_item_scopes(masked, depths, group_depths)
    for declaration in re.finditer(
        r"\bmod\s+(?P<name>r#[A-Za-z_][A-Za-z0-9_]*|"
        r"[A-Za-z_][A-Za-z0-9_]*)\s*;",
        masked,
    ):
        if (
            not rust_is_production_item_position(
                declaration.start(), depths, item_scopes
            )
            or group_depths[declaration.start()] != 0
        ):
            continue
        attribute_start = rust_outer_attribute_start(
            masked,
            depths,
            group_depths,
            declaration.start(),
            depth=depths[declaration.start()],
        )
        attributes = surface_source[attribute_start : declaration.start()]
        if re.search(
            r"#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]", attributes
        ) is not None:
            continue
        path_attribute = re.search(
            r"#\s*\[\s*path\s*=\s*\"(?P<path>[^\"]+)\"\s*\]",
            attributes,
        )
        if path_attribute is not None:
            candidates = [
                declaring_source.parent / Path(path_attribute.group("path"))
            ]
        else:
            module_name = declaration.group("name").removeprefix("r#")
            module_parent = (
                declaring_source.parent
                if declaring_source.name in {"lib.rs", "mod.rs"}
                else declaring_source.with_suffix("")
            )
            candidates = [
                module_parent / f"{module_name}.rs",
                module_parent / module_name / "mod.rs",
            ]
        for candidate in candidates:
            if (
                candidate.is_file()
                and candidate.is_relative_to(source_root)
                and is_test_source(candidate.relative_to(root))
            ):
                raise ValueError(
                    f"test-classified Rust module {candidate.relative_to(root)} "
                    "must be included only under #[cfg(test)]"
                )


def rust_production_literal_include_sources(
    root: Path,
    source_root: Path,
    declaring_source: Path,
    content: str,
) -> set[Path]:
    masked = strip_rust_comments_and_strings(content)
    surface_source = strip_rust_comments_and_strings(content, strip_strings=False)
    depths = rust_brace_depths(masked)
    group_depths = rust_group_depths(masked)
    item_scopes = rust_production_item_scopes(masked, depths, group_depths)
    delimiter_pairs = {"(": ")", "[": "]", "{": "}"}
    includes: set[Path] = set()
    for invocation in re.finditer(
        r"\binclude\s*!\s*(?P<open>[({[])", masked
    ):
        if not rust_is_production_item_position(
            invocation.start(), depths, item_scopes
        ):
            continue
        attribute_start = rust_outer_attribute_start(
            masked,
            depths,
            group_depths,
            invocation.start(),
            depth=depths[invocation.start()],
        )
        attributes = surface_source[attribute_start : invocation.start()]
        if re.search(
            r"#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]", attributes
        ) is not None:
            continue
        open_index = invocation.end("open") - 1
        stack = [masked[open_index]]
        close_index: int | None = None
        for index in range(open_index + 1, len(masked)):
            character = masked[index]
            if character in delimiter_pairs:
                stack.append(character)
            elif character in delimiter_pairs.values():
                if not stack or delimiter_pairs[stack[-1]] != character:
                    raise ValueError("unbalanced production include! delimiter")
                stack.pop()
                if not stack:
                    close_index = index
                    break
        if close_index is None:
            raise ValueError("unterminated production include!")
        arguments = surface_source[open_index + 1 : close_index]
        literal = re.fullmatch(r'\s*"(?P<path>[^"\\]+)"\s*', arguments)
        if literal is None:
            raise ValueError(
                "production item-scope include! must use a plain relative "
                "string literal for public API scanning"
            )
        candidate = (
            declaring_source.parent / literal.group("path")
        ).resolve()
        resolved_source_root = source_root.resolve()
        if not candidate.is_relative_to(resolved_source_root):
            raise ValueError(
                f"production include! target escapes its source root: {candidate}"
            )
        if not candidate.is_file():
            raise ValueError(f"production include! target is missing: {candidate}")
        relative = candidate.relative_to(root.resolve())
        includes.add(root / relative)
    return includes


def collect_workspace_exported_symbol_surfaces(
    root: Path,
    source_root_relative: Path,
    symbols: set[str],
    *,
    include_public_inventory: bool = False,
) -> dict[str, set[str]]:
    if CRATE_PUBLIC_INVENTORY_KEY in symbols:
        raise ValueError("reserved public API inventory key requested as a symbol")
    source_root = root / source_root_relative
    if not source_root.is_dir():
        raise ValueError(
            f"workspace public API source root is missing: {source_root_relative}"
        )
    source_contents: dict[Path, str] = {}
    definitions: dict[str, list[tuple[str, str, Path]]] = {}
    macro_candidates: dict[str, dict[Path, set[str]]] = {}
    macro_invocation_sources: dict[str, set[Path]] = {}
    workspace_item_macro_anchors: set[str] = set()
    workspace_contextual_macro_anchors: set[str] = set()
    nominal_sources: dict[str, list[Path]] = {}
    alias_targets: dict[str, list[tuple[str, Path]]] = {}
    include_graph: dict[Path, set[Path]] = {}
    pending_sources = [
        source
        for source in sorted(source_root.rglob("*.rs"))
        if not is_test_source(source.relative_to(root))
        and not is_generated_source(source.relative_to(root))
    ]
    while pending_sources:
        source = pending_sources.pop(0)
        if source in source_contents:
            continue
        relative = source.relative_to(root)
        content = source.read_text(encoding="utf-8")
        rust_reject_public_foreign_items(content)
        rust_reject_unconditional_test_module_includes(
            root, source_root, source, content
        )
        included_sources = rust_production_literal_include_sources(
            root, source_root, source, content
        )
        include_graph[source] = included_sources
        pending_sources.extend(
            sorted(
                included_sources
                - set(source_contents)
                - set(pending_sources)
            )
        )
        source_contents[source] = content
        for name in rust_top_level_nominal_type_names(content):
            nominal_sources.setdefault(name, []).append(source)
        for alias, target in rust_top_level_simple_type_aliases(content):
            alias_targets.setdefault(alias, []).append((target, source))
        for alias, target in rust_top_level_simple_use_aliases(content):
            alias_targets.setdefault(alias, []).append((target, source))
            nominal_sources.setdefault(alias, []).append(source)
        for kind, name, surface in rust_top_level_public_items(content):
            definitions.setdefault(name, []).append((kind, surface, source))
        macro_anchors, macro_owners = rust_public_macro_invocation_anchors(content)
        workspace_contextual_macro_anchors.update(
            f"macro-source:{source.relative_to(source_root).as_posix()}:{anchor}"
            for anchor in macro_anchors
            if anchor.startswith("macro-top-level-invocation:")
        )
        workspace_item_macro_anchors.update(
            anchor
            for anchor in macro_anchors
            if anchor.startswith("macro-top-level-invocation:")
        )
        for anchor in macro_anchors:
            if anchor.startswith("macro-top-level-invocation:"):
                macro_name = anchor.split(":", maxsplit=2)[1]
                macro_invocation_sources.setdefault(macro_name, set()).add(
                    source
                )
        for generated_name in macro_owners:
            owner_anchors = {
                anchor
                for anchor in macro_anchors
                if anchor.split(":", maxsplit=3)[2] == generated_name
            }
            if owner_anchors:
                macro_candidates.setdefault(generated_name, {}).setdefault(
                    source, set()
                ).update(owner_anchors)
    root_included_sources: set[Path] = set()
    pending_root_includes = list(include_graph.get(source_root / "lib.rs", set()))
    while pending_root_includes:
        included_source = pending_root_includes.pop()
        if included_source in root_included_sources:
            continue
        root_included_sources.add(included_source)
        pending_root_includes.extend(
            include_graph.get(included_source, set()) - root_included_sources
        )
    combined_macro_content = "\n".join(
        source_contents[source] for source in sorted(source_contents)
    )
    combined_macro_anchors, combined_macro_owners = (
        rust_public_macro_invocation_anchors(
            combined_macro_content, fail_unresolved=True
        )
    )
    workspace_item_macro_anchors.update(
        anchor
        for anchor in combined_macro_anchors
        if anchor.startswith("macro-top-level-invocation:")
    )
    combined_macro_source = source_root / "__crate_macro_surface__.rs"
    for generated_name in combined_macro_owners:
        owner_anchors = {
            anchor
            for anchor in combined_macro_anchors
            if anchor.split(":", maxsplit=3)[2] == generated_name
        }
        if owner_anchors:
            existing_candidates = macro_candidates.get(generated_name)
            if existing_candidates:
                for anchors in existing_candidates.values():
                    anchors.update(owner_anchors)
            else:
                generating_macros = {
                    parts[1]
                    for anchor in owner_anchors
                    if len(parts := anchor.split(":", maxsplit=3)) >= 3
                    and parts[0]
                    in {
                        "macro-export",
                        "macro-public-invocation",
                        "macro-workspace-owner",
                    }
                }
                candidate_sources = {
                    source
                    for macro_name in generating_macros
                    for source in macro_invocation_sources.get(
                        macro_name, set()
                    )
                } or {combined_macro_source}
                for candidate_source in candidate_sources:
                    macro_candidates.setdefault(
                        generated_name, {}
                    ).setdefault(candidate_source, set()).update(owner_anchors)

    all_macro_definitions = {
        owner: [(set(anchors), source) for source, anchors in sorted(by_source.items())]
        for owner, by_source in macro_candidates.items()
    }
    macro_extensions = {
        owner: records
        for owner, records in all_macro_definitions.items()
        if owner in definitions
    }
    macro_definitions = {
        owner: records
        for owner, records in all_macro_definitions.items()
        if owner not in definitions
    }
    public_nominal_owners = {
        name
        for name, records in definitions.items()
        if any(kind in {"enum", "struct", "trait", "union"} for kind, _, _ in records)
    } | set(macro_definitions)
    public_alias_owners = {
        name
        for name, records in definitions.items()
        if any(kind == "type" for kind, _, _ in records)
    }
    alias_roots = {owner: owner for owner in public_nominal_owners}
    changed = True
    while changed:
        changed = False
        for alias, records in sorted(alias_targets.items()):
            resolved_records = [
                alias_roots[target]
                for target, _ in records
                if target in alias_roots
            ]
            resolved_roots = set(resolved_records)
            if not resolved_roots:
                continue
            if len(resolved_records) != len(records) or len(resolved_roots) != 1:
                raise ValueError(
                    f"public API alias owner {alias} has an ambiguous target binding "
                    f"under {source_root_relative}"
                )
            resolved_root = next(iter(resolved_roots))
            existing_root = alias_roots.get(alias)
            if existing_root is not None and existing_root != resolved_root:
                raise ValueError(
                    f"public API alias owner {alias} resolves to multiple roots "
                    f"under {source_root_relative}"
                )
            if existing_root is None:
                alias_roots[alias] = resolved_root
                changed = True
    public_impl_owners = public_nominal_owners | public_alias_owners | set(alias_roots)
    for owner in sorted(public_impl_owners):
        public_binding_count = sum(
            kind in {"enum", "struct", "trait", "type", "union"}
            for kind, _, _ in definitions.get(owner, [])
        )
        observed_binding_count = len(nominal_sources.get(owner, []))
        expected_binding_count = public_binding_count
        if (
            public_binding_count == 0
            and owner in alias_roots
            and owner not in macro_definitions
        ):
            alias_records = alias_targets.get(owner, [])
            if not alias_records:
                raise ValueError(
                    f"public API alias owner {owner} has an ambiguous target binding "
                    f"under {source_root_relative}"
                )
            expected_binding_count = len(alias_records)
        if observed_binding_count != expected_binding_count:
            raise ValueError(
                f"public API owner {owner} has an ambiguous private same-name "
                f"owner binding under {source_root_relative}"
            )
    primary_anchor_index: dict[str, set[str]] = {}
    for anchor in rust_primary_type_public_anchors(
        combined_macro_content, public_impl_owners, alias_roots
    ):
        owner = anchor.split(":", maxsplit=2)[1]
        primary_anchor_index.setdefault(owner, set()).add(anchor)
    for owner, records in macro_extensions.items():
        for anchors, _ in records:
            primary_anchor_index.setdefault(owner, set()).update(
                f"macro-owner:{owner}:{anchor}" for anchor in anchors
            )

    symbol_cache: dict[str, set[str]] = {}
    module_cache: dict[tuple[Path, Path | None], set[str]] = {}

    def primary_type_anchors(owner: str) -> set[str]:
        return set(primary_anchor_index.get(owner, set()))

    def resolve_private_alias_terminal(
        alias: str, stack: tuple[str, ...]
    ) -> str:
        if alias in stack:
            raise ValueError(
                f"cyclic public type alias surface: {' -> '.join((*stack, alias))}"
            )
        if alias in definitions or alias in macro_definitions:
            return alias
        records = alias_targets.get(alias, [])
        targets = {target for target, _ in records}
        if len(targets) != 1:
            raise ValueError(
                f"public type alias target {alias} must resolve to exactly one "
                f"binding under {source_root_relative}; found {len(targets)}"
            )
        return resolve_private_alias_terminal(
            next(iter(targets)), (*stack, alias)
        )

    def resolve_module_surface(
        source: Path,
        stack: tuple[str, ...],
        module_context_source: Path | None = None,
    ) -> set[str]:
        cache_key = (source, module_context_source)
        if cache_key in module_cache:
            return set(module_cache[cache_key])
        module_marker = (
            f"module:{source.relative_to(source_root).as_posix()}:context:"
            f"{module_context_source or source}"
        )
        if module_marker in stack:
            raise ValueError(f"cyclic public module surface: {' -> '.join((*stack, module_marker))}")
        content = source_contents.get(source)
        if content is None:
            raise ValueError(
                f"public module source is outside the production source inventory: "
                f"{source.relative_to(root)}"
            )
        next_stack = (*stack, module_marker)
        anchors, wildcards = rust_facade_public_anchors(content)
        if wildcards:
            raise ValueError(
                f"public module surface contains an unmapped wildcard: "
                f"{source.relative_to(root)}"
            )
        macro_anchors, macro_owners = rust_public_macro_invocation_anchors(content)
        anchors.update(macro_anchors)
        for owner in macro_owners:
            resolved_owner = resolve_symbol(owner, next_stack)
            anchors.update(
                anchor
                for anchor in resolved_owner
                if anchor.startswith(("member:", "trait-impl:"))
            )

        for kind, name, surface in rust_top_level_public_items(content):
            if kind in {"enum", "struct", "trait", "union"}:
                resolved_owner = resolve_symbol(name, next_stack)
                anchors.update(
                    anchor
                    for anchor in resolved_owner
                    if anchor.startswith(("member:", "trait-impl:"))
                )
            elif kind == "type":
                resolved_alias = resolve_symbol(name, next_stack)
                anchors.update(
                    anchor
                    for anchor in resolved_alias
                    if not anchor.startswith("definition:")
                )
            elif kind == "mod":
                nested_source = rust_external_module_source(
                    source_root,
                    module_context_source or source,
                    name,
                    surface,
                )
                if nested_source is not None:
                    anchors.update(
                        f"module-surface:{name}:{anchor}"
                        for anchor in resolve_module_surface(nested_source, next_stack)
                    )

        for public_name, origin, wildcard in rust_facade_public_export_leaves(content):
            if wildcard or public_name == "_":
                continue
            normalized_origin = origin.removeprefix("::")
            origin_segments = normalized_origin.split("::")
            terminal = origin_segments[-1]
            if origin_segments[0].startswith("lorepia_"):
                crate_name = origin_segments[0]
                target_root = PUBLIC_API_WORKSPACE_SOURCE_ROOTS.get(crate_name)
                if target_root is None:
                    raise ValueError(
                        f"public workspace export has no source mapping: {normalized_origin}"
                    )
                target_surfaces = collect_workspace_exported_symbol_surfaces(
                    root, target_root, {terminal}
                )[terminal]
            else:
                target_surfaces = resolve_symbol(terminal, next_stack)
            anchors.update(
                f"export-surface:{public_name}<-{normalized_origin}:{anchor}"
                for anchor in target_surfaces
            )

        for included_source in sorted(include_graph.get(source, set())):
            anchors.update(
                f"include-surface:{anchor}"
                for anchor in resolve_module_surface(
                    included_source,
                    next_stack,
                    included_source.parent / "mod.rs",
                )
            )

        module_cache[cache_key] = set(anchors)
        return set(anchors)

    def resolve_symbol(symbol: str, stack: tuple[str, ...]) -> set[str]:
        if symbol in stack:
            raise ValueError(f"cyclic public type alias surface: {' -> '.join((*stack, symbol))}")
        if symbol in symbol_cache:
            return set(symbol_cache[symbol])
        direct = definitions.get(symbol, [])
        generated = macro_definitions.get(symbol, [])
        definition_count = len(direct) + len(generated)
        if definition_count != 1:
            raise ValueError(
                f"workspace public export {symbol} must resolve to exactly one "
                f"definition surface under {source_root_relative}; found "
                f"{definition_count}"
            )
        next_stack = (*stack, symbol)
        if generated:
            anchors = {*generated[0][0], *primary_type_anchors(symbol)}
            generated_source = generated[0][1]
            if generated_source != combined_macro_source:
                module_parent = (
                    generated_source.parent
                    if generated_source.name in {"lib.rs", "mod.rs"}
                    else generated_source.with_suffix("")
                )
                module_candidates = [
                    module_parent / f"{symbol}.rs",
                    module_parent / symbol / "mod.rs",
                ]
                module_matches = [
                    candidate
                    for candidate in module_candidates
                    if candidate.is_file()
                    and candidate.is_relative_to(source_root)
                ]
                if len(module_matches) > 1:
                    raise ValueError(
                        f"macro-generated public module {symbol} must resolve "
                        f"to at most one source file; found {len(module_matches)}"
                    )
                if module_matches:
                    anchors.update(
                        f"macro-module-surface:{symbol}:{anchor}"
                        for anchor in resolve_module_surface(
                            module_matches[0], next_stack
                        )
                    )
        else:
            kind, surface, source = direct[0]
            anchors = {
                rust_public_surface_anchor("definition", kind, symbol, surface)
            }
            if "macro_rules" in combined_macro_content:
                anchors.update(
                    rust_local_macro_dependency_anchors(
                        combined_macro_content,
                        surface,
                        f"definition:{kind}:{symbol}",
                    )
                )
            if kind in {"enum", "struct", "trait", "union"}:
                anchors.update(primary_type_anchors(symbol))
            elif kind == "type":
                anchors.update(primary_type_anchors(symbol))
                target = rust_simple_type_alias_target(surface)
                if target is not None and (
                    target not in definitions and target not in macro_definitions
                ) and target in alias_targets:
                    target = resolve_private_alias_terminal(target, next_stack)
                if target in definitions or target in macro_definitions:
                    anchors.update(
                        f"alias-target:{symbol}:{anchor}"
                        for anchor in resolve_symbol(target, next_stack)
                    )
            elif kind == "mod":
                module_source = rust_external_module_source(
                    source_root, source, symbol, surface
                )
                if module_source is not None:
                    anchors.update(
                        f"module-surface:{symbol}:{anchor}"
                        for anchor in resolve_module_surface(module_source, next_stack)
                    )
        symbol_cache[symbol] = set(anchors)
        return set(anchors)

    resolved = {
        symbol: resolve_symbol(symbol, ()) for symbol in sorted(symbols)
    }
    if include_public_inventory:
        inventory = rust_public_item_inventory_anchors(
            tuple(source_contents[source] for source in sorted(source_contents))
        )
        for source in sorted(root_included_sources):
            included_content = source_contents.get(source)
            if included_content is None:
                raise ValueError(
                    f"crate-root include was not scanned: {source}"
                )
            inventory.update(
                f"crate-root-include:{anchor}"
                for anchor in rust_public_item_inventory_anchors(
                    (included_content,)
                )
            )
            inventory.update(
                f"crate-root-include-surface:{anchor}"
                for anchor in resolve_module_surface(
                    source,
                    (),
                    source.parent / "mod.rs",
                )
            )
        inventory.update(workspace_item_macro_anchors)
        inventory.update(workspace_contextual_macro_anchors)
        for anchors in primary_anchor_index.values():
            inventory.update(anchors)
        for content in source_contents.values():
            inventory.update(
                rust_public_exported_macro_definition_anchors(content)
            )
        resolved[CRATE_PUBLIC_INVENTORY_KEY] = inventory
    return resolved


def collect_explicit_external_export_surface(
    root: Path, facade_content: str
) -> set[str]:
    exports_by_crate: dict[str, list[tuple[str, str, str]]] = {}
    for public_name, origin, wildcard in rust_facade_public_export_leaves(
        facade_content
    ):
        if wildcard or public_name == "_":
            continue
        normalized_origin = origin.removeprefix("::")
        origin_segments = normalized_origin.split("::")
        crate_name = origin_segments[0]
        if not crate_name.startswith("lorepia_"):
            continue
        if crate_name not in PUBLIC_API_WORKSPACE_SOURCE_ROOTS:
            raise ValueError(
                f"public workspace export has no source mapping: {normalized_origin}"
            )
        exports_by_crate.setdefault(crate_name, []).append(
            (public_name, normalized_origin, origin_segments[-1])
        )

    anchors: set[str] = set()
    for crate_name, exports in sorted(exports_by_crate.items()):
        source_root_relative = PUBLIC_API_WORKSPACE_SOURCE_ROOTS[crate_name]
        symbols = {symbol for _, _, symbol in exports}
        surfaces = collect_workspace_exported_symbol_surfaces(
            root,
            source_root_relative,
            symbols,
            include_public_inventory=True,
        )
        for public_name, origin, symbol in exports:
            anchors.update(
                f"export-surface:{public_name}<-{origin}:{anchor}"
                for anchor in surfaces[symbol]
            )
        for anchor in surfaces[CRATE_PUBLIC_INVENTORY_KEY]:
            owner = rust_public_inventory_owner(anchor)
            if owner is None:
                anchors.add(
                    f"workspace-public-inventory:{crate_name}:{anchor}"
                )
            else:
                anchors.add(
                    f"workspace-public-owner:{crate_name}:{owner}:{anchor}"
                )
    return anchors


def collect_wildcard_target_surface(root: Path, origin: str) -> set[str]:
    target = PUBLIC_API_WILDCARD_TARGETS.get(origin)
    if target is None:
        raise ValueError(f"public wildcard has no reviewed target mapping: {origin}")
    source = root / target
    if not source.is_file():
        raise ValueError(f"public wildcard target is missing: {target.as_posix()}")
    content = source.read_text(encoding="utf-8")
    facade_anchors, nested_wildcards = rust_facade_public_anchors(content)
    if nested_wildcards:
        raise ValueError(f"public wildcard target contains a nested wildcard: {origin}")
    macro_anchors, macro_owners = rust_public_macro_invocation_anchors(content)
    target_owners = {
        name
        for kind, name, _ in rust_top_level_public_items(content)
        if kind in {"enum", "struct", "trait", "type", "union"}
    } | macro_owners
    workspace_surfaces = collect_workspace_exported_symbol_surfaces(
        root,
        target.parent,
        set(),
        include_public_inventory=True,
    )
    workspace_public_anchors: set[str] = set()
    for anchor in workspace_surfaces[CRATE_PUBLIC_INVENTORY_KEY]:
        owner = rust_public_inventory_owner(anchor)
        if owner is None:
            workspace_public_anchors.add(
                f"workspace-public-inventory:{anchor}"
            )
        else:
            workspace_public_anchors.add(
                f"workspace-public-owner:{owner}:{anchor}"
            )
    target_anchors = {
        *facade_anchors,
        *rust_primary_type_public_anchors(content, target_owners),
        *macro_anchors,
        *workspace_public_anchors,
    }
    return {
        f"wildcard-surface:{origin}:{anchor}" for anchor in target_anchors
    }


def collect_core_storage_public_surface(root: Path) -> dict[str, list[str]]:
    result: dict[str, list[str]] = {}
    for crate_name, source_root_relative in PUBLIC_API_CRATES.items():
        source_root = root / source_root_relative
        facade = source_root / "lib.rs"
        if not facade.is_file():
            raise ValueError(f"missing public facade for {crate_name}: {facade}")
        facade_content = facade.read_text(encoding="utf-8")
        facade_anchors, _ = rust_facade_public_anchors(
            facade_content
        )
        anchors = set(facade_anchors)
        facade_macro_anchors, facade_macro_owners = (
            rust_public_macro_invocation_anchors(facade_content)
        )
        anchors.update(facade_macro_anchors)
        anchors.update(collect_explicit_external_export_surface(root, facade_content))
        exported_symbols = rust_facade_local_exported_symbols(facade_content)
        facade_owners = {
            name
            for kind, name, _ in rust_top_level_public_items(facade_content)
            if kind in {"enum", "mod", "struct", "trait", "type", "union"}
        } | facade_macro_owners
        local_surfaces = collect_workspace_exported_symbol_surfaces(
            root,
            source_root_relative,
            exported_symbols | facade_owners,
            include_public_inventory=True,
        )
        for symbol, symbol_anchors in local_surfaces.items():
            if symbol == CRATE_PUBLIC_INVENTORY_KEY:
                anchors.update(symbol_anchors)
            else:
                anchors.update(symbol_anchors)
        if crate_name == "lorepia-core":
            for wildcard in sorted(
                anchor.removeprefix("wildcard:")
                for anchor in anchors
                if anchor.startswith("wildcard:")
            ):
                anchors.update(collect_wildcard_target_surface(root, wildcard))
        result[crate_name] = sorted(anchors)
    return result


def evaluate_core_storage_public_surface(
    root: Path, config: dict[str, Any]
) -> list[str]:
    if config.get("version") != 2:
        return []
    observed = collect_core_storage_public_surface(root)
    failures: list[str] = []
    for crate_name in sorted(PUBLIC_API_CRATES):
        current = set(observed[crate_name])
        expected = set(config["public_surface"][crate_name])
        failures.extend(
            f"unapproved {crate_name} public API growth: {anchor}"
            for anchor in sorted(current - expected)
        )
        failures.extend(
            f"stale {crate_name} public API baseline after removal: {anchor}"
            for anchor in sorted(expected - current)
        )
    observed_wildcards = sorted(
        anchor
        for anchors in observed.values()
        for anchor in anchors
        if anchor.startswith("wildcard:")
    )
    expected_wildcards = config["legacy_wildcard_reexports"]
    failures.extend(
        f"unapproved public wildcard re-export: {anchor}"
        for anchor in sorted(set(observed_wildcards) - set(expected_wildcards))
    )
    failures.extend(
        f"stale legacy public wildcard baseline: {anchor}"
        for anchor in sorted(set(expected_wildcards) - set(observed_wildcards))
    )
    return failures


def evaluate_core_storage_public_reexports(
    root: Path, allowed_stored_reexports: set[str]
) -> list[str]:
    """Prevent Core from publicly exposing additional Storage persistence rows."""

    source_root = root / "crates" / "core" / "src"
    if not source_root.is_dir():
        return []

    failures: list[str] = []
    observed: set[str] = set()
    for source in sorted(source_root.rglob("*.rs")):
        relative = source.relative_to(root).as_posix()
        content = strip_rust_comments_and_strings(source.read_text(encoding="utf-8"))
        depths = rust_brace_depths(content)
        for public_match in UNRESTRICTED_PUBLIC_RE.finditer(content):
            declaration_start = public_match.end()
            use_match = re.match(r"use\b", content[declaration_start:])
            if use_match is None:
                continue
            depth = depths[public_match.start()]
            body_start = declaration_start + use_match.end()
            end = rust_same_depth_terminator(
                content, depths, body_start, depth=depth, characters={";"}
            )
            for _, origin, wildcard in parse_rust_use_tree(content[body_start:end]):
                normalized_origin = origin.removeprefix("::")
                if wildcard and (
                    normalized_origin == "lorepia_storage::*"
                    or normalized_origin.startswith("lorepia_storage::")
                ):
                    failures.append(
                        f"{relative} must not wildcard-reexport lorepia_storage from Core"
                    )
                    continue
                if not normalized_origin.startswith("lorepia_storage::"):
                    continue
                name = normalized_origin.rsplit("::", maxsplit=1)[-1]
                if STORED_TYPE_RE.fullmatch(name) is None:
                    continue
                observed.add(name)
                if name not in allowed_stored_reexports:
                    failures.append(
                        f"{relative} must not publicly re-export storage persistence row "
                        f"{name}; define a Core-owned view instead"
                    )
    for name in sorted(allowed_stored_reexports - observed):
        failures.append(
            f"core-storage public API baseline is stale after removing re-export {name}"
        )
    return sorted(set(failures))


def print_source_table(measurements: list[SourceMeasurement]) -> None:
    print("source-size ratchet (all current baselines and any failures)")
    print("status  kind                         bytes/current-cap  lines/current-cap  decl  public  source")
    for item in measurements:
        status = "FAIL" if item.failed else "ok"
        print(
            f"{status:6}  {item.kind:27}  {item.bytes:7}/{item.byte_limit:<7}  "
            f"{item.lines:6}/{item.line_limit:<6}  {item.declarations:4}  "
            f"{item.public_symbols:6}  {item.path}"
        )


def print_aggregate_table(title: str, aggregates: list[AggregateDelta]) -> None:
    print(title)
    print("files before/after  bytes before/after (delta)  lines before/after (delta)  path")
    if not aggregates:
        print("(no changed source paths)")
        return
    for item in aggregates:
        print(
            f"{item.before_files:5}/{item.after_files:<5}  "
            f"{item.before_bytes:8}/{item.after_bytes:<8} "
            f"({item.after_bytes - item.before_bytes:+8})  "
            f"{item.before_lines:7}/{item.after_lines:<7} "
            f"({item.after_lines - item.before_lines:+7})  {item.path}"
        )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=REPO_ROOT)
    parser.add_argument("--config", type=Path, default=DEFAULT_CONFIG)
    parser.add_argument("--test-config", type=Path, default=DEFAULT_TEST_CONFIG)
    parser.add_argument(
        "--core-storage-api-config",
        type=Path,
        default=DEFAULT_CORE_STORAGE_API_CONFIG,
    )
    parser.add_argument(
        "--dependency-config",
        type=Path,
        default=DEFAULT_DEPENDENCY_ARCHITECTURE_CONFIG,
    )
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
    test_config = args.test_config.resolve()
    core_storage_api_config = args.core_storage_api_config.resolve()
    dependency_config = args.dependency_config.resolve()
    directory_deltas: list[AggregateDelta] = []
    parent_deltas: list[AggregateDelta] = []
    parent_child_deltas: list[AggregateDelta] = []
    try:
        source_configuration = load_config(config)
        test_configuration = load_test_config(test_config)
        if source_configuration["bootstrap_ref"] != test_configuration["bootstrap_ref"]:
            raise ValueError("source and test size baselines must share one bootstrap_ref")
        bootstrap_ref = source_configuration["bootstrap_ref"]
        require_commit(root, bootstrap_ref, label="source-size bootstrap_ref")
        bootstrap_source_configuration = load_json_at_ref(root, config, bootstrap_ref)
        bootstrap_test_configuration = load_json_at_ref(root, test_config, bootstrap_ref)
        if not isinstance(bootstrap_source_configuration, dict) or not isinstance(
            bootstrap_test_configuration, dict
        ):
            raise ValueError(
                "source-size bootstrap_ref must contain both v1 baseline configs"
            )
        if (
            bootstrap_source_configuration.get("version") != 1
            or bootstrap_test_configuration.get("version") != 1
        ):
            raise ValueError("source-size bootstrap_ref must identify the v1 policy tree")
        failures, measurements = evaluate_source_sizes(
            root, config, base_ref=args.base_ref
        )
        test_failures, test_measurements = evaluate_test_source_sizes(
            root, test_config, base_ref=args.base_ref
        )
        failures.extend(test_failures)
        failures.extend(
            evaluate_baseline_changes(
                source_configuration,
                bootstrap_source_configuration,
                bootstrap=bootstrap_source_configuration,
            )
        )
        failures.extend(
            evaluate_test_baseline_changes(
                test_configuration,
                bootstrap_test_configuration,
                bootstrap=bootstrap_test_configuration,
            )
        )
        measurements.extend(test_measurements)
        core_storage_api = load_core_storage_api_config(core_storage_api_config)
        dependency_policy = load_dependency_architecture_config(dependency_config)
        if core_storage_api.get("version") != 2:
            raise ValueError("current core-storage public API baseline must use version 2")
        if core_storage_api["bootstrap_ref"] != dependency_policy["bootstrap_ref"]:
            raise ValueError("ENF-002 public API and dependency policies must share bootstrap_ref")
        if core_storage_api["bootstrap_ref"] != ENF002_BOOTSTRAP_REF:
            raise ValueError(
                "ENF-002 bootstrap_ref must remain the reviewed ENF-001 commit "
                f"{ENF002_BOOTSTRAP_REF}"
            )
        require_commit(
            root,
            core_storage_api["bootstrap_ref"],
            label="ENF-002 bootstrap_ref",
        )
        parent_paths = {
            *source_configuration["baselines"],
            *source_configuration["parent_child_groups"],
        }
        if args.base_ref:
            base_config = load_base_config(root, config, args.base_ref)
            if base_config is not None:
                parent_paths.update(base_config.get("baselines", {}))
                if base_config.get("version") == 2:
                    failures.extend(
                        evaluate_baseline_changes(
                            source_configuration,
                            base_config,
                        )
                    )
            base_test_config = load_base_config(root, test_config, args.base_ref)
            if base_test_config is not None:
                if base_test_config.get("version") == 2:
                    failures.extend(
                        evaluate_test_baseline_changes(
                            test_configuration,
                            base_test_config,
                        )
                    )
            if (base_config is not None and base_config.get("version") == 1) or (
                base_test_config is not None and base_test_config.get("version") == 1
            ):
                require_v2_bootstrap_transition(root, bootstrap_ref)
            base_core_storage_api = load_json_at_ref(
                root, core_storage_api_config, args.base_ref
            )
            if base_core_storage_api is not None:
                failures.extend(
                    evaluate_core_storage_api_baseline_changes(
                        core_storage_api, base_core_storage_api
                    )
                )
            base_dependency_policy = load_json_at_ref(
                root, dependency_config, args.base_ref
            )
            if base_dependency_policy is not None:
                if not isinstance(base_dependency_policy, dict):
                    raise ValueError("base dependency architecture policy must be an object")
                failures.extend(
                    evaluate_dependency_policy_changes(
                        dependency_policy, base_dependency_policy
                    )
                )
            if (
                base_core_storage_api is None
                or not isinstance(base_core_storage_api, dict)
                or base_core_storage_api.get("version") != 2
                or base_dependency_policy is None
            ):
                require_enf002_bootstrap_transition(
                    root, core_storage_api["bootstrap_ref"]
                )
            directory_deltas, parent_deltas, parent_child_deltas = source_aggregate_deltas(
                root,
                args.base_ref,
                facade_paths=set(source_configuration["facade_paths"]),
                parent_paths=parent_paths,
                parent_child_groups=source_configuration["parent_child_groups"],
            )
        failures.extend(
            evaluate_core_storage_public_surface(root, core_storage_api)
        )
        failures.extend(
            evaluate_core_storage_public_reexports(
                root, set(core_storage_api["allowed_stored_reexports"])
            )
        )
        if not args.skip_dependency_check:
            failures.extend(
                evaluate_dependency_architecture(
                    cargo_metadata(root), dependency_policy, root
                )
            )
    except ValueError as error:
        print(f"source architecture: {error}", file=sys.stderr)
        return 1

    print_source_table(measurements)
    if args.base_ref:
        print_aggregate_table("changed source directory aggregates", directory_deltas)
        print_aggregate_table("baseline parent file deltas", parent_deltas)
        print_aggregate_table(
            "explicit parent/child source group aggregates", parent_child_deltas
        )
    if failures:
        for failure in failures:
            print(f"source architecture: {failure}", file=sys.stderr)
        return 1
    print("source architecture: PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
