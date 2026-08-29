#!/usr/bin/env python3
"""Copy only final, platform-distributable files into a release candidate tree."""

from __future__ import annotations

import shutil
import sys
from pathlib import Path


ARTIFACT_SUFFIXES = {
    "linux": (".AppImage", ".deb", ".rpm"),
    "macos": (".dmg",),
    "windows": (".msi", ".exe"),
}
MAXIMUM_ARTIFACTS = 32


def stage_candidate(bundle_root: Path, candidate_root: Path, platform: str) -> list[Path]:
    bundle_root = bundle_root.resolve(strict=True)
    if not bundle_root.is_dir():
        raise ValueError("release bundle root must be a directory")
    platform = platform.lower()
    suffixes = ARTIFACT_SUFFIXES.get(platform)
    if suffixes is None:
        raise ValueError(f"unsupported release platform: {platform}")
    if candidate_root.exists():
        raise ValueError("release candidate root must not already exist")

    artifacts = sorted(
        (
            path
            for path in bundle_root.rglob("*")
            if not path.is_symlink()
            and path.is_file()
            and path.name.endswith(suffixes)
        ),
        key=lambda path: path.relative_to(bundle_root).as_posix(),
    )
    if not artifacts:
        raise ValueError(f"release bundle has no final {platform} artifacts")
    if len(artifacts) > MAXIMUM_ARTIFACTS:
        raise ValueError("release bundle contains too many final artifacts")

    candidate_root.mkdir(parents=True, exist_ok=False)
    staged: list[Path] = []
    for source in artifacts:
        relative = source.relative_to(bundle_root)
        parent = bundle_root
        for part in relative.parts[:-1]:
            parent /= part
            if parent.is_symlink():
                raise ValueError(f"release artifact is nested below a symbolic link: {source}")
        destination = candidate_root / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, destination)
        staged.append(destination)
    return staged


def main() -> int:
    if len(sys.argv) != 4:
        print(
            "usage: stage_release_candidate.py BUNDLE_ROOT CANDIDATE_ROOT PLATFORM",
            file=sys.stderr,
        )
        return 2
    try:
        staged = stage_candidate(Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3])
    except (OSError, ValueError) as error:
        print(f"release candidate staging: {error}", file=sys.stderr)
        return 1
    print(f"release candidate staging: copied {len(staged)} artifact(s)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
