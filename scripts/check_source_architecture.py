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
DEFAULT_TEST_CONFIG = REPO_ROOT / "config" / "test-source-size-baseline.json"
DEFAULT_CORE_STORAGE_API_CONFIG = (
    REPO_ROOT / "config" / "core-storage-public-api-baseline.json"
)
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
    "config/source-size-baseline.json",
    "config/test-source-size-baseline.json",
    "scripts/check_source_architecture.py",
    "scripts/test_check_source_architecture.py",
}
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
CORE_STORAGE_REEXPORT_RE = re.compile(
    r"\bpub\s+use\s+lorepia_storage::(?P<body>[^;]+);", re.DOTALL
)
STORED_TYPE_RE = re.compile(r"\bStored[A-Za-z0-9_]*\b")
RUST_RAW_STRING_RE = re.compile(r'(?:br|cr|r)(?P<hashes>#{0,255})"')


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
    if not isinstance(config, dict) or config.get("version") != 1:
        raise ValueError("core-storage public API baseline version must be 1")
    allowed = config.get("allowed_stored_reexports")
    if not isinstance(allowed, list) or not all(isinstance(name, str) for name in allowed):
        raise ValueError("allowed_stored_reexports must be a string array")
    if any(STORED_TYPE_RE.fullmatch(name) is None for name in allowed):
        raise ValueError("allowed_stored_reexports may contain only Stored* type names")
    if allowed != sorted(set(allowed)):
        raise ValueError("allowed_stored_reexports must be unique and sorted")
    return config


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
    current_allowed = set(current["allowed_stored_reexports"])
    base_allowed = set(validate_core_storage_api_config(base)["allowed_stored_reexports"])
    additions = sorted(current_allowed - base_allowed)
    return [
        f"new Core storage Stored* re-export exception is not allowed: {name}"
        for name in additions
    ]


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
    changed = subprocess.run(
        ["git", "diff", "--name-only", bootstrap_ref, "--"],
        cwd=root,
        check=False,
        capture_output=True,
        text=True,
    )
    if changed.returncode != 0:
        raise ValueError(f"cannot verify source-size bootstrap transition: {changed.stderr.strip()}")
    unexpected = sorted(set(changed.stdout.splitlines()) - V2_BOOTSTRAP_EDIT_PATHS)
    if unexpected:
        raise ValueError(
            "v2 bootstrap must be based on the exact pre-enforcement tree; "
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


def strip_rust_comments_and_strings(content: str) -> str:
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
            blank(quote_start, min(index, length))
            continue
        index += 1

    return "".join(output)


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
        for reexport in CORE_STORAGE_REEXPORT_RE.finditer(content):
            body = reexport.group("body")
            if "*" in body:
                failures.append(
                    f"{relative} must not wildcard-reexport lorepia_storage from Core"
                )
            for name in STORED_TYPE_RE.findall(body):
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
            directory_deltas, parent_deltas, parent_child_deltas = source_aggregate_deltas(
                root,
                args.base_ref,
                facade_paths=set(source_configuration["facade_paths"]),
                parent_paths=parent_paths,
                parent_child_groups=source_configuration["parent_child_groups"],
            )
        failures.extend(
            evaluate_core_storage_public_reexports(
                root, set(core_storage_api["allowed_stored_reexports"])
            )
        )
        if not args.skip_dependency_check:
            failures.extend(evaluate_dependency_architecture(cargo_metadata(root)))
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
