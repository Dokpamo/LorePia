#!/usr/bin/env python3
"""Write a deterministic SHA-256 manifest for one release bundle tree."""

from __future__ import annotations

import hashlib
import os
import sys
from pathlib import Path


MANIFEST_NAME = "SHA256SUMS"
MAXIMUM_ARTIFACTS = 10_000


def write_checksums(root: Path) -> Path:
    root = root.resolve(strict=True)
    if not root.is_dir():
        raise ValueError("release bundle root must be a directory")
    artifacts = sorted(
        (
            path
            for path in root.rglob("*")
            if path.name != MANIFEST_NAME and path.name != f".{MANIFEST_NAME}.tmp"
        ),
        key=lambda path: path.relative_to(root).as_posix(),
    )
    if len(artifacts) > MAXIMUM_ARTIFACTS:
        raise ValueError("release bundle contains too many paths")

    lines: list[str] = []
    for path in artifacts:
        if path.is_symlink():
            raise ValueError(f"release bundle contains a symbolic link: {path}")
        if path.is_dir():
            continue
        if not path.is_file():
            raise ValueError(f"release bundle contains a special file: {path}")
        digest = hashlib.sha256()
        with path.open("rb") as artifact:
            for chunk in iter(lambda: artifact.read(1024 * 1024), b""):
                digest.update(chunk)
        lines.append(f"{digest.hexdigest()}  {path.relative_to(root).as_posix()}")

    output = root / MANIFEST_NAME
    temporary = root / f".{MANIFEST_NAME}.tmp"
    temporary.write_text("\n".join(lines) + ("\n" if lines else ""), encoding="utf-8")
    os.replace(temporary, output)
    return output


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: write_release_checksums.py BUNDLE_ROOT", file=sys.stderr)
        return 2
    try:
        output = write_checksums(Path(sys.argv[1]))
    except (OSError, ValueError) as error:
        print(f"release checksums: {error}", file=sys.stderr)
        return 1
    print(f"release checksums: wrote {output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
