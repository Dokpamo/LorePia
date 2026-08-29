#!/usr/bin/env python3
"""Copy only final, platform-distributable files into a release candidate tree."""

from __future__ import annotations

import errno
import os
import shutil
import stat
import sys
from pathlib import Path


ARTIFACT_SUFFIXES = {
    "linux": (".AppImage", ".deb", ".rpm"),
    "macos": (".dmg",),
    "windows": (".msi", ".exe"),
}
MAXIMUM_ARTIFACTS = 32
COPY_BUFFER_SIZE = 1024 * 1024


def _copy_appimage(source: Path, destination: Path) -> None:
    try:
        source_snapshot = source.stat(follow_symlinks=False)
    except FileNotFoundError as error:
        raise ValueError(f"release artifact disappeared before copying: {source}") from error
    if stat.S_ISLNK(source_snapshot.st_mode) or not stat.S_ISREG(
        source_snapshot.st_mode
    ):
        raise ValueError(f"release artifact must be a regular file: {source}")

    source_flags = (
        os.O_RDONLY
        | getattr(os, "O_BINARY", 0)
        | getattr(os, "O_CLOEXEC", 0)
        | getattr(os, "O_NOFOLLOW", 0)
        | getattr(os, "O_NONBLOCK", 0)
    )
    try:
        source_fd = os.open(source, source_flags)
    except OSError as error:
        if error.errno in (errno.ELOOP, errno.ENOTDIR):
            raise ValueError(
                f"release artifact changed to a symbolic link while opening: {source}"
            ) from error
        raise

    try:
        opened_source = os.fstat(source_fd)
        if not stat.S_ISREG(opened_source.st_mode) or not os.path.samestat(
            source_snapshot, opened_source
        ):
            raise ValueError(f"release artifact changed while opening: {source}")

        destination_flags = (
            os.O_WRONLY
            | os.O_CREAT
            | os.O_EXCL
            | getattr(os, "O_BINARY", 0)
            | getattr(os, "O_CLOEXEC", 0)
            | getattr(os, "O_NOFOLLOW", 0)
        )
        destination_fd = os.open(destination, destination_flags, 0o666)
        try:
            while chunk := os.read(source_fd, COPY_BUFFER_SIZE):
                remaining = memoryview(chunk)
                while remaining:
                    written = os.write(destination_fd, remaining)
                    if written == 0:
                        raise OSError("short write while staging release artifact")
                    remaining = remaining[written:]

            source_after_copy = os.fstat(source_fd)
            try:
                path_after_copy = source.stat(follow_symlinks=False)
            except FileNotFoundError as error:
                raise ValueError(
                    f"release artifact disappeared while copying: {source}"
                ) from error
            stable_snapshot = (
                stat.S_ISREG(path_after_copy.st_mode)
                and os.path.samestat(opened_source, path_after_copy)
                and os.path.samestat(opened_source, source_after_copy)
                and opened_source.st_size == source_after_copy.st_size
                and opened_source.st_mtime_ns == source_after_copy.st_mtime_ns
                and opened_source.st_ctime_ns == source_after_copy.st_ctime_ns
            )
            if not stable_snapshot:
                raise ValueError(f"release artifact changed while copying: {source}")

            # AppImage needs the source execute mask, but source write bits and
            # setuid/setgid/sticky metadata must not enter the candidate tree.
            destination_mode = stat.S_IMODE(os.fstat(destination_fd).st_mode)
            executable_mode = destination_mode | (
                stat.S_IMODE(opened_source.st_mode) & 0o111
            )
            os.fchmod(destination_fd, executable_mode)
            if stat.S_IMODE(os.fstat(destination_fd).st_mode) != executable_mode:
                raise OSError("failed to preserve release artifact permissions")
        finally:
            os.close(destination_fd)
    finally:
        os.close(source_fd)


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
        if platform == "linux" and source.name.endswith(".AppImage"):
            _copy_appimage(source, destination)
        else:
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
