#!/usr/bin/env python3
"""Materialize the ignored Gradle inputs emitted by Tauri Android build scripts."""

from __future__ import annotations

import errno
import json
import os
import stat
import subprocess
from contextlib import ExitStack
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
APP_ANDROID = REPO_ROOT / "apps/lorepia/src-tauri/gen/android"
PLUGIN_ROOT = REPO_ROOT / "plugins/lorepia-platform"
EXPECTED_TAURI_VERSION = "2.11.5"
COPY_BUFFER_SIZE = 1024 * 1024


def cargo_packages() -> list[dict[str, object]]:
    result = subprocess.run(
        [
            "cargo",
            "metadata",
            "--locked",
            "--format-version",
            "1",
            "--filter-platform",
            "aarch64-linux-android",
        ],
        cwd=REPO_ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(result.stdout)["packages"]


def unique_package(
    packages: list[dict[str, object]], name: str, version: str | None = None
) -> dict[str, object]:
    matches = [
        package
        for package in packages
        if package.get("name") == name
        and (version is None or package.get("version") == version)
    ]
    if len(matches) != 1:
        expected = f"{name} {version}" if version else name
        raise RuntimeError(f"expected exactly one locked Cargo package for {expected}")
    return matches[0]


def manifest_directory(package: dict[str, object]) -> Path:
    manifest_path = package.get("manifest_path")
    if not isinstance(manifest_path, str):
        raise RuntimeError("Cargo package is missing a manifest_path")
    return Path(manifest_path).resolve().parent


def lstat_if_present(path: Path) -> os.stat_result | None:
    try:
        return path.lstat()
    except FileNotFoundError:
        return None


def require_real_directory(path: Path, description: str) -> Path:
    path_stat = lstat_if_present(path)
    if path_stat is None:
        raise RuntimeError(f"missing {description}: {path}")
    if stat.S_ISLNK(path_stat.st_mode):
        raise RuntimeError(f"refusing symlink for {description}: {path}")
    if not stat.S_ISDIR(path_stat.st_mode):
        raise RuntimeError(f"{description} must be a real directory: {path}")
    return path.resolve(strict=True)


def require_contained(path: Path, root: Path, description: str) -> None:
    try:
        path.relative_to(root)
    except ValueError as error:
        raise RuntimeError(
            f"{description} escapes its allowed directory: {path}"
        ) from error


def require_secure_filesystem_support() -> None:
    required_dir_fd_functions = (os.open, os.mkdir, os.rmdir, os.stat, os.unlink)
    supported = (
        getattr(os, "O_DIRECTORY", 0) != 0
        and getattr(os, "O_NOFOLLOW", 0) != 0
        and all(function in os.supports_dir_fd for function in required_dir_fd_functions)
        and os.stat in os.supports_follow_symlinks
        and os.listdir in os.supports_fd
    )
    if not supported:
        raise RuntimeError(
            "secure no-follow directory operations are unavailable on this platform"
        )


def directory_open_flags() -> int:
    return (
        os.O_RDONLY
        | os.O_DIRECTORY
        | os.O_NOFOLLOW
        | getattr(os, "O_CLOEXEC", 0)
    )


def file_open_flags() -> int:
    return os.O_NOFOLLOW | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NONBLOCK", 0)


def open_real_directory(path: Path, description: str) -> int:
    path_stat = lstat_if_present(path)
    if path_stat is None:
        raise RuntimeError(f"missing {description}: {path}")
    if stat.S_ISLNK(path_stat.st_mode):
        raise RuntimeError(f"refusing symlink for {description}: {path}")
    if not stat.S_ISDIR(path_stat.st_mode):
        raise RuntimeError(f"{description} must be a real directory: {path}")
    try:
        directory_fd = os.open(path, directory_open_flags())
    except OSError as error:
        if error.errno in (errno.ELOOP, errno.ENOTDIR):
            raise RuntimeError(
                f"refusing symlink or non-directory for {description}: {path}"
            ) from error
        raise
    opened_stat = os.fstat(directory_fd)
    if not stat.S_ISDIR(opened_stat.st_mode) or not os.path.samestat(
        path_stat, opened_stat
    ):
        os.close(directory_fd)
        raise RuntimeError(f"{description} changed while it was being opened: {path}")
    return directory_fd


def open_directory_at(parent_fd: int, name: str, description: str) -> int:
    try:
        path_stat = os.stat(name, dir_fd=parent_fd, follow_symlinks=False)
    except FileNotFoundError as error:
        raise RuntimeError(f"missing {description}: {name}") from error
    if stat.S_ISLNK(path_stat.st_mode):
        raise RuntimeError(f"refusing symlink for {description}: {name}")
    if not stat.S_ISDIR(path_stat.st_mode):
        raise RuntimeError(f"{description} must be a real directory: {name}")
    try:
        directory_fd = os.open(name, directory_open_flags(), dir_fd=parent_fd)
    except OSError as error:
        if error.errno in (errno.ELOOP, errno.ENOTDIR):
            raise RuntimeError(
                f"refusing symlink or non-directory for {description}: {name}"
            ) from error
        raise
    opened_stat = os.fstat(directory_fd)
    if not stat.S_ISDIR(opened_stat.st_mode) or not os.path.samestat(
        path_stat, opened_stat
    ):
        os.close(directory_fd)
        raise RuntimeError(f"{description} changed while it was being opened: {name}")
    return directory_fd


def contained_parts(path: Path, root: Path, description: str) -> tuple[str, ...]:
    if not path.is_absolute() or not root.is_absolute() or ".." in path.parts:
        raise RuntimeError(f"{description} must be an absolute path beneath {root}")
    try:
        relative = path.relative_to(root)
    except ValueError as error:
        raise RuntimeError(f"{description} escapes the repository: {path}") from error
    if not relative.parts or any(part in ("", ".", "..") for part in relative.parts):
        raise RuntimeError(f"invalid {description}: {path}")
    return relative.parts


def open_directory_beneath(
    root_fd: int, path: Path, root: Path, description: str
) -> int:
    parts = contained_parts(path, root, description)
    current_fd = os.dup(root_fd)
    try:
        for index, component in enumerate(parts):
            component_description = (
                description
                if index == len(parts) - 1
                else f"{description} parent"
            )
            next_fd = open_directory_at(current_fd, component, component_description)
            os.close(current_fd)
            current_fd = next_fd
        return current_fd
    except BaseException:
        os.close(current_fd)
        raise


def require_entry_matches_fd(
    parent_fd: int, name: str, opened_fd: int, description: str
) -> None:
    try:
        path_stat = os.stat(name, dir_fd=parent_fd, follow_symlinks=False)
    except FileNotFoundError as error:
        raise RuntimeError(f"{description} disappeared while preparing inputs") from error
    if stat.S_ISLNK(path_stat.st_mode):
        raise RuntimeError(f"refusing symlink for {description}: {name}")
    if not os.path.samestat(path_stat, os.fstat(opened_fd)):
        raise RuntimeError(f"{description} changed while preparing inputs: {name}")


def require_regular_file_at(parent_fd: int, name: str, description: str) -> None:
    try:
        path_stat = os.stat(name, dir_fd=parent_fd, follow_symlinks=False)
    except FileNotFoundError as error:
        raise RuntimeError(f"missing {description}: {name}") from error
    if stat.S_ISLNK(path_stat.st_mode):
        raise RuntimeError(f"refusing symlink for {description}: {name}")
    if not stat.S_ISREG(path_stat.st_mode):
        raise RuntimeError(f"{description} must be a regular file: {name}")


def read_all(file_fd: int) -> bytes:
    os.lseek(file_fd, 0, os.SEEK_SET)
    chunks: list[bytes] = []
    while chunk := os.read(file_fd, COPY_BUFFER_SIZE):
        chunks.append(chunk)
    return b"".join(chunks)


def write_all(file_fd: int, content: bytes) -> None:
    os.lseek(file_fd, 0, os.SEEK_SET)
    os.ftruncate(file_fd, 0)
    remaining = memoryview(content)
    while remaining:
        written = os.write(file_fd, remaining)
        if written == 0:
            raise RuntimeError("short write while preparing Android Gradle input")
        remaining = remaining[written:]


def validate_output_fd(file_fd: int, description: str) -> os.stat_result:
    file_stat = os.fstat(file_fd)
    if not stat.S_ISREG(file_stat.st_mode):
        raise RuntimeError(f"{description} must be a regular file")
    if file_stat.st_nlink != 1:
        raise RuntimeError(f"refusing multiply linked {description}")
    return file_stat


def write_if_changed_at(
    parent_fd: int, name: str, content: str, description: str
) -> None:
    encoded = content.encode("utf-8")
    try:
        read_fd = os.open(
            name, os.O_RDONLY | file_open_flags(), dir_fd=parent_fd
        )
    except FileNotFoundError:
        try:
            output_fd = os.open(
                name,
                os.O_WRONLY | os.O_CREAT | os.O_EXCL | file_open_flags(),
                0o644,
                dir_fd=parent_fd,
            )
        except FileExistsError as error:
            raise RuntimeError(
                f"{description} changed while it was being created: {name}"
            ) from error
        except OSError as error:
            if error.errno in (errno.ELOOP, errno.ENOTDIR):
                raise RuntimeError(f"refusing symlink for {description}: {name}") from error
            raise
        try:
            output_stat = validate_output_fd(output_fd, description)
            write_all(output_fd, encoded)
            require_entry_matches_fd(parent_fd, name, output_fd, description)
            if not os.path.samestat(output_stat, os.fstat(output_fd)):
                raise RuntimeError(f"{description} changed while it was being written")
        finally:
            os.close(output_fd)
        return
    except OSError as error:
        if error.errno in (errno.ELOOP, errno.ENOTDIR):
            raise RuntimeError(f"refusing symlink for {description}: {name}") from error
        raise

    try:
        read_stat = validate_output_fd(read_fd, description)
        if read_all(read_fd) == encoded:
            require_entry_matches_fd(parent_fd, name, read_fd, description)
            return
    finally:
        os.close(read_fd)

    try:
        output_fd = os.open(name, os.O_WRONLY | file_open_flags(), dir_fd=parent_fd)
    except OSError as error:
        if error.errno in (errno.ELOOP, errno.ENOTDIR):
            raise RuntimeError(f"refusing symlink for {description}: {name}") from error
        raise
    try:
        output_stat = validate_output_fd(output_fd, description)
        if not os.path.samestat(read_stat, output_stat):
            raise RuntimeError(f"{description} changed while it was being opened: {name}")
        write_all(output_fd, encoded)
        require_entry_matches_fd(parent_fd, name, output_fd, description)
    finally:
        os.close(output_fd)


def get_or_create_directory_at(parent_fd: int, name: str, description: str) -> int:
    try:
        os.mkdir(name, 0o755, dir_fd=parent_fd)
    except FileExistsError:
        pass
    return open_directory_at(parent_fd, name, description)


def remove_tree_at(parent_fd: int, name: str, description: str) -> None:
    try:
        path_stat = os.stat(name, dir_fd=parent_fd, follow_symlinks=False)
    except FileNotFoundError:
        return
    if stat.S_ISLNK(path_stat.st_mode):
        raise RuntimeError(f"refusing symlink for {description}: {name}")
    if not stat.S_ISDIR(path_stat.st_mode):
        raise RuntimeError(f"{description} must be a real directory: {name}")

    directory_fd = open_directory_at(parent_fd, name, description)
    try:
        remove_directory_contents(directory_fd, description)
        require_entry_matches_fd(parent_fd, name, directory_fd, description)
    finally:
        os.close(directory_fd)
    try:
        os.rmdir(name, dir_fd=parent_fd)
    except OSError as error:
        raise RuntimeError(f"failed to safely replace {description}: {name}") from error


def remove_directory_contents(directory_fd: int, description: str) -> None:
    for name in sorted(os.listdir(directory_fd)):
        path_stat = os.stat(name, dir_fd=directory_fd, follow_symlinks=False)
        if not stat.S_ISDIR(path_stat.st_mode):
            try:
                os.unlink(name, dir_fd=directory_fd)
            except OSError as error:
                raise RuntimeError(
                    f"failed to safely remove an entry from {description}: {name}"
                ) from error
            continue

        child_fd = open_directory_at(directory_fd, name, description)
        try:
            remove_directory_contents(child_fd, description)
            require_entry_matches_fd(directory_fd, name, child_fd, description)
        finally:
            os.close(child_fd)
        try:
            os.rmdir(name, dir_fd=directory_fd)
        except OSError as error:
            raise RuntimeError(
                f"failed to safely remove a directory from {description}: {name}"
            ) from error


def copy_file_at(
    source_parent_fd: int,
    destination_parent_fd: int,
    name: str,
    source_stat: os.stat_result,
) -> None:
    try:
        source_fd = os.open(
            name, os.O_RDONLY | file_open_flags(), dir_fd=source_parent_fd
        )
    except OSError as error:
        if error.errno in (errno.ELOOP, errno.ENOTDIR):
            raise RuntimeError(f"refusing symlink in locked Tauri source: {name}") from error
        raise
    try:
        opened_source_stat = os.fstat(source_fd)
        if not stat.S_ISREG(opened_source_stat.st_mode) or not os.path.samestat(
            source_stat, opened_source_stat
        ):
            raise RuntimeError(f"locked Tauri source changed while copying: {name}")
        try:
            destination_fd = os.open(
                name,
                os.O_WRONLY | os.O_CREAT | os.O_EXCL | file_open_flags(),
                0o600,
                dir_fd=destination_parent_fd,
            )
        except FileExistsError as error:
            raise RuntimeError(
                f"generated Tauri Android API changed while copying: {name}"
            ) from error
        try:
            validate_output_fd(destination_fd, "generated Tauri Android API file")
            while chunk := os.read(source_fd, COPY_BUFFER_SIZE):
                remaining = memoryview(chunk)
                while remaining:
                    written = os.write(destination_fd, remaining)
                    if written == 0:
                        raise RuntimeError(
                            "short write while copying locked Tauri Android source"
                        )
                    remaining = remaining[written:]
            os.fchmod(destination_fd, stat.S_IMODE(opened_source_stat.st_mode))
            require_entry_matches_fd(
                destination_parent_fd,
                name,
                destination_fd,
                "generated Tauri Android API file",
            )
        finally:
            os.close(destination_fd)
    finally:
        os.close(source_fd)


def copy_directory_contents(source_fd: int, destination_fd: int) -> None:
    for name in sorted(os.listdir(source_fd)):
        source_stat = os.stat(name, dir_fd=source_fd, follow_symlinks=False)
        if stat.S_ISLNK(source_stat.st_mode):
            raise RuntimeError(f"refusing symlink in locked Tauri Android source: {name}")
        if stat.S_ISREG(source_stat.st_mode):
            copy_file_at(source_fd, destination_fd, name, source_stat)
            continue
        if not stat.S_ISDIR(source_stat.st_mode):
            raise RuntimeError(f"unsupported entry in locked Tauri Android source: {name}")

        source_child_fd = open_directory_at(
            source_fd, name, "locked Tauri Android source directory"
        )
        try:
            try:
                os.mkdir(name, 0o700, dir_fd=destination_fd)
            except FileExistsError as error:
                raise RuntimeError(
                    f"generated Tauri Android API changed while copying: {name}"
                ) from error
            destination_child_fd = open_directory_at(
                destination_fd, name, "generated Tauri Android API directory"
            )
            try:
                copy_directory_contents(source_child_fd, destination_child_fd)
                os.fchmod(destination_child_fd, stat.S_IMODE(source_stat.st_mode))
                require_entry_matches_fd(
                    destination_fd,
                    name,
                    destination_child_fd,
                    "generated Tauri Android API directory",
                )
            finally:
                os.close(destination_child_fd)
        finally:
            os.close(source_child_fd)


def main() -> None:
    require_secure_filesystem_support()
    packages = cargo_packages()
    tauri = unique_package(packages, "tauri", EXPECTED_TAURI_VERSION)
    plugin = unique_package(packages, "tauri-plugin-lorepia-platform")

    tauri_root = manifest_directory(tauri)
    tauri_android = tauri_root / "mobile/android"
    plugin_root = manifest_directory(plugin)
    if plugin_root != PLUGIN_ROOT.resolve():
        raise RuntimeError(
            "tauri-plugin-lorepia-platform must resolve to the repository path"
        )
    plugin_android = plugin_root / "android"

    tauri_root_real = require_real_directory(tauri_root, "locked Tauri package")
    tauri_android_real = require_real_directory(
        tauri_android, "locked Tauri Android source directory"
    )
    require_contained(
        tauri_android_real,
        tauri_root_real,
        "locked Tauri Android source directory",
    )

    settings = (
        "// THIS IS AN AUTOGENERATED FILE. DO NOT EDIT THIS FILE DIRECTLY.\n"
        "include ':tauri-android'\n"
        f"project(':tauri-android').projectDir = new File({json.dumps(str(tauri_android))})\n"
        "include ':tauri-plugin-lorepia-platform'\n"
        f"project(':tauri-plugin-lorepia-platform').projectDir = "
        f"new File({json.dumps(str(plugin_android))})\n"
    )
    app_build = """// THIS IS AN AUTOGENERATED FILE. DO NOT EDIT THIS FILE DIRECTLY.
val implementation by configurations
dependencies {
  implementation("androidx.lifecycle:lifecycle-process:2.10.0")
  implementation(project(":tauri-android"))
  implementation(project(":tauri-plugin-lorepia-platform"))
}"""

    with ExitStack() as descriptors:
        tauri_root_fd = open_real_directory(
            tauri_root_real, "locked Tauri package"
        )
        descriptors.callback(os.close, tauri_root_fd)
        tauri_mobile_fd = open_directory_at(
            tauri_root_fd, "mobile", "locked Tauri mobile source directory"
        )
        descriptors.callback(os.close, tauri_mobile_fd)
        tauri_android_fd = open_directory_at(
            tauri_mobile_fd, "android", "locked Tauri Android source directory"
        )
        descriptors.callback(os.close, tauri_android_fd)
        require_regular_file_at(
            tauri_android_fd,
            "build.gradle.kts",
            "locked Tauri Android Gradle input",
        )

        repository_fd = open_real_directory(REPO_ROOT, "repository root")
        descriptors.callback(os.close, repository_fd)
        app_android_fd = open_directory_beneath(
            repository_fd,
            APP_ANDROID,
            REPO_ROOT,
            "generated app Android directory",
        )
        descriptors.callback(os.close, app_android_fd)
        app_module_fd = open_directory_at(
            app_android_fd, "app", "generated app module directory"
        )
        descriptors.callback(os.close, app_module_fd)
        plugin_android_fd = open_directory_beneath(
            repository_fd,
            PLUGIN_ROOT / "android",
            REPO_ROOT,
            "plugin Android directory",
        )
        descriptors.callback(os.close, plugin_android_fd)
        require_regular_file_at(
            plugin_android_fd,
            "build.gradle.kts",
            "plugin Android Gradle input",
        )
        copied_tauri_parent_fd = get_or_create_directory_at(
            plugin_android_fd, ".tauri", "generated .tauri directory"
        )
        descriptors.callback(os.close, copied_tauri_parent_fd)

        write_if_changed_at(
            app_android_fd,
            "tauri.settings.gradle",
            settings,
            "generated Tauri settings file",
        )
        write_if_changed_at(
            app_module_fd,
            "tauri.build.gradle.kts",
            app_build,
            "generated Tauri app build file",
        )

        remove_tree_at(
            copied_tauri_parent_fd,
            "tauri-api",
            "generated Tauri Android API directory",
        )
        copied_tauri_api_fd = get_or_create_directory_at(
            copied_tauri_parent_fd,
            "tauri-api",
            "generated Tauri Android API directory",
        )
        descriptors.callback(os.close, copied_tauri_api_fd)
        copy_directory_contents(tauri_android_fd, copied_tauri_api_fd)
        os.fchmod(
            copied_tauri_api_fd, stat.S_IMODE(os.fstat(tauri_android_fd).st_mode)
        )
        require_entry_matches_fd(
            copied_tauri_parent_fd,
            "tauri-api",
            copied_tauri_api_fd,
            "generated Tauri Android API directory",
        )
        require_entry_matches_fd(
            plugin_android_fd,
            ".tauri",
            copied_tauri_parent_fd,
            "generated .tauri directory",
        )
    print("prepared ignored Tauri Android Gradle inputs from locked Cargo packages")


if __name__ == "__main__":
    main()
