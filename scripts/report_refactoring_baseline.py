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

from check_source_architecture import production_sources


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUTPUT = REPO_ROOT / "config" / "refactoring" / "baseline-report.json"
SOURCE_SIZE_CONFIG = Path("config/source-size-baseline.json")
CORE_STORAGE_API_CONFIG = Path("config/core-storage-public-api-baseline.json")
IPC_CONFIG = Path("config/ipc-commands.json")
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

    return {
        "format_version": 1,
        "baseline_commit": baseline_commit,
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
            SOURCE_SIZE_CONFIG.as_posix(): json_digest(source_size_config),
            CORE_STORAGE_API_CONFIG.as_posix(): json_digest(core_storage_api),
        },
    }


def serialized_report(report: dict[str, Any]) -> str:
    return json.dumps(report, ensure_ascii=False, indent=2, sort_keys=True) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=REPO_ROOT)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--baseline-ref", default="HEAD")
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--print", dest="print_report", action="store_true")
    args = parser.parse_args()

    root = args.root.resolve()
    output = args.output
    if not output.is_absolute():
        output = root / output
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
    if args.print_report:
        print(rendered, end="")
        return 0
    if args.check:
        try:
            current = output.read_text(encoding="utf-8")
        except OSError as error:
            print(f"refactoring baseline: FAIL: cannot read {output}: {error}", file=sys.stderr)
            return 1
        if current != rendered:
            print(
                "refactoring baseline: FAIL: snapshot differs from the current worktree",
                file=sys.stderr,
            )
            return 1
        print("refactoring baseline: PASS")
        return 0

    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(rendered, encoding="utf-8")
    print(f"refactoring baseline: wrote {output.relative_to(root)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
