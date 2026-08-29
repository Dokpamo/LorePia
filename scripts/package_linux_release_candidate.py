#!/usr/bin/env python3
"""Package a Linux candidate tree into a deterministic, mode-preserving tarball."""

from __future__ import annotations

import errno
import gzip
import os
import stat
import sys
import tarfile
from pathlib import Path


MAXIMUM_PATHS = 10_000


def _stable_regular_file(path: Path) -> tuple[int, os.stat_result]:
    snapshot = path.stat(follow_symlinks=False)
    if stat.S_ISLNK(snapshot.st_mode) or not stat.S_ISREG(snapshot.st_mode):
        raise ValueError(f"Linux release candidate contains a non-regular file: {path}")
    flags = (
        os.O_RDONLY
        | getattr(os, "O_BINARY", 0)
        | getattr(os, "O_CLOEXEC", 0)
        | getattr(os, "O_NOFOLLOW", 0)
        | getattr(os, "O_NONBLOCK", 0)
    )
    try:
        file_fd = os.open(path, flags)
    except OSError as error:
        if error.errno in (errno.ELOOP, errno.ENOTDIR):
            raise ValueError(
                f"Linux release candidate changed to a symbolic link: {path}"
            ) from error
        raise
    opened = os.fstat(file_fd)
    if not stat.S_ISREG(opened.st_mode) or not os.path.samestat(snapshot, opened):
        os.close(file_fd)
        raise ValueError(f"Linux release candidate changed while opening: {path}")
    return file_fd, opened


def _assert_file_unchanged(
    path: Path, file_fd: int, opened: os.stat_result
) -> None:
    current_fd = os.fstat(file_fd)
    current_path = path.stat(follow_symlinks=False)
    stable = (
        stat.S_ISREG(current_path.st_mode)
        and os.path.samestat(opened, current_fd)
        and os.path.samestat(opened, current_path)
        and opened.st_size == current_fd.st_size
        and opened.st_mtime_ns == current_fd.st_mtime_ns
        and opened.st_ctime_ns == current_fd.st_ctime_ns
    )
    if not stable:
        raise ValueError(f"Linux release candidate changed while packaging: {path}")


def _canonical_tar_info(name: str, mode: int, size: int = 0) -> tarfile.TarInfo:
    info = tarfile.TarInfo(name)
    info.mode = mode
    info.uid = 0
    info.gid = 0
    info.uname = ""
    info.gname = ""
    info.mtime = 0
    info.size = size
    return info


def package_linux_candidate(candidate_root: Path, output: Path) -> Path:
    candidate_root = candidate_root.resolve(strict=True)
    if not candidate_root.is_dir():
        raise ValueError("Linux release candidate root must be a directory")

    entries = sorted(
        candidate_root.rglob("*"),
        key=lambda path: path.relative_to(candidate_root).as_posix(),
    )
    if not entries:
        raise ValueError("Linux release candidate is empty")
    if len(entries) > MAXIMUM_PATHS:
        raise ValueError("Linux release candidate contains too many paths")
    for path in entries:
        path_stat = path.stat(follow_symlinks=False)
        if stat.S_ISLNK(path_stat.st_mode):
            raise ValueError(f"Linux release candidate contains a symbolic link: {path}")
        if not stat.S_ISDIR(path_stat.st_mode) and not stat.S_ISREG(path_stat.st_mode):
            raise ValueError(f"Linux release candidate contains a special file: {path}")

    output_parent = output.parent.resolve(strict=True)
    output = output_parent / output.name
    if output.exists():
        raise ValueError("Linux release candidate archive must not already exist")
    try:
        output.relative_to(candidate_root)
    except ValueError:
        pass
    else:
        raise ValueError("Linux release candidate archive must be outside the candidate tree")

    temporary = output.with_name(f".{output.name}.tmp")
    if temporary.exists():
        raise ValueError("Linux release candidate temporary archive must not already exist")
    try:
        with temporary.open("xb") as raw_archive:
            with gzip.GzipFile(
                filename="",
                mode="wb",
                compresslevel=9,
                fileobj=raw_archive,
                mtime=0,
            ) as compressed_archive:
                with tarfile.open(
                    fileobj=compressed_archive,
                    mode="w",
                    format=tarfile.USTAR_FORMAT,
                ) as archive:
                    for path in entries:
                        relative = path.relative_to(candidate_root).as_posix()
                        path_stat = path.stat(follow_symlinks=False)
                        if stat.S_ISDIR(path_stat.st_mode):
                            info = _canonical_tar_info(relative, 0o755)
                            info.type = tarfile.DIRTYPE
                            archive.addfile(info)
                            continue

                        file_fd, opened = _stable_regular_file(path)
                        try:
                            info = _canonical_tar_info(
                                relative,
                                stat.S_IMODE(opened.st_mode) & 0o777,
                                opened.st_size,
                            )
                            with os.fdopen(os.dup(file_fd), "rb") as artifact:
                                archive.addfile(info, artifact)
                            _assert_file_unchanged(path, file_fd, opened)
                        finally:
                            os.close(file_fd)
        os.replace(temporary, output)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise
    return output


def main() -> int:
    if len(sys.argv) != 3:
        print(
            "usage: package_linux_release_candidate.py CANDIDATE_ROOT OUTPUT_TAR_GZ",
            file=sys.stderr,
        )
        return 2
    try:
        output = package_linux_candidate(Path(sys.argv[1]), Path(sys.argv[2]))
    except (OSError, ValueError, tarfile.TarError) as error:
        print(f"Linux release candidate packaging: {error}", file=sys.stderr)
        return 1
    print(f"Linux release candidate packaging: wrote {output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
