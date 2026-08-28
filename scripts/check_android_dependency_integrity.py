#!/usr/bin/env python3
"""Fail closed when committed Android dependency-integrity controls drift."""

from __future__ import annotations

import re
import sys
import xml.etree.ElementTree as ET
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
SHA256_RE = re.compile(r"[0-9a-f]{64}")
MODULE_LOCK_RE = re.compile(r"^[^#\s][^=]*:[^=]*:[^=]*=[^\s]+$")


def local_name(tag: str) -> str:
    return tag.rsplit("}", 1)[-1]


def require_file(relative_path: str, failures: list[str]) -> Path:
    path = REPO_ROOT / relative_path
    if not path.is_file():
        failures.append(f"missing {relative_path}")
    return path


def require_text(relative_path: str, expected: str, failures: list[str]) -> None:
    path = require_file(relative_path, failures)
    if path.is_file() and expected not in path.read_text(encoding="utf-8"):
        failures.append(f"{relative_path} must contain: {expected}")


def forbid_text(relative_path: str, forbidden: str, failures: list[str]) -> None:
    path = require_file(relative_path, failures)
    if path.is_file() and forbidden in path.read_text(encoding="utf-8"):
        failures.append(f"{relative_path} must not contain: {forbidden}")


def require_occurrences(
    relative_path: str, expected: str, minimum: int, failures: list[str]
) -> None:
    path = require_file(relative_path, failures)
    if path.is_file():
        count = path.read_text(encoding="utf-8").count(expected)
        if count < minimum:
            failures.append(
                f"{relative_path} must contain {expected!r} at least {minimum} times"
            )


def check_metadata(relative_path: str, failures: list[str]) -> None:
    path = require_file(relative_path, failures)
    if not path.is_file():
        return
    try:
        root = ET.parse(path).getroot()
    except (ET.ParseError, OSError) as error:
        failures.append(f"invalid {relative_path}: {error}")
        return
    if local_name(root.tag) != "verification-metadata":
        failures.append(f"{relative_path} has an unexpected root element")
        return

    elements = list(root.iter())
    verify_metadata = [
        element
        for element in elements
        if local_name(element.tag) == "verify-metadata"
    ]
    if len(verify_metadata) != 1 or (verify_metadata[0].text or "").strip() != "true":
        failures.append(f"{relative_path} must set verify-metadata to true")
    if any(local_name(element.tag) == "trusted-artifacts" for element in elements):
        failures.append(f"{relative_path} must not bypass verification with trusted-artifacts")

    artifacts = [element for element in elements if local_name(element.tag) == "artifact"]
    if not artifacts:
        failures.append(f"{relative_path} does not verify any dependency artifact")
        return
    for artifact in artifacts:
        hashes = [
            child.attrib.get("value", "")
            for child in artifact
            if local_name(child.tag) == "sha256"
        ]
        if not hashes or any(SHA256_RE.fullmatch(value) is None for value in hashes):
            failures.append(
                f"{relative_path} artifact {artifact.attrib.get('name', '<unnamed>')} "
                "must have only valid SHA-256 values"
            )


def check_lockfile(relative_path: str, failures: list[str]) -> None:
    path = require_file(relative_path, failures)
    if not path.is_file():
        return
    entries = [
        line.strip()
        for line in path.read_text(encoding="utf-8").splitlines()
        if line.strip() and not line.lstrip().startswith("#")
    ]
    modules = [line for line in entries if not line.startswith("empty=")]
    if not modules:
        failures.append(f"{relative_path} does not lock any dependency")
    for module in modules:
        if MODULE_LOCK_RE.fullmatch(module) is None:
            failures.append(f"{relative_path} has an invalid lock entry: {module}")


def main() -> int:
    failures: list[str] = []

    require_text(
        "apps/lorepia/src-tauri/gen/android/gradle/wrapper/gradle-wrapper.properties",
        "distributionSha256Sum=bd71102213493060956ec229d946beee57158dbd89d0e62b91bca0fa2c5f3531",
        failures,
    )
    for properties in (
        "apps/lorepia/src-tauri/gen/android/gradle.properties",
        "plugins/lorepia-platform/android/gradle.properties",
    ):
        require_text(properties, "org.gradle.dependency.verification=strict", failures)

    check_metadata(
        "apps/lorepia/src-tauri/gen/android/gradle/verification-metadata.xml",
        failures,
    )
    check_metadata(
        "plugins/lorepia-platform/android/gradle/verification-metadata.xml",
        failures,
    )

    for lockfile in (
        "apps/lorepia/src-tauri/gen/android/buildscript-gradle.lockfile",
        "apps/lorepia/src-tauri/gen/android/app/gradle.lockfile",
        "apps/lorepia/src-tauri/gen/android/buildSrc/gradle.lockfile",
        "apps/lorepia/src-tauri/gen/android/tauri-android-gradle.lockfile",
        "plugins/lorepia-platform/android/gradle.lockfile",
        "plugins/lorepia-platform/android/tauri-android-gradle.lockfile",
    ):
        check_lockfile(lockfile, failures)

    require_text(
        "apps/lorepia/src-tauri/gen/android/build.gradle.kts",
        "resolutionStrategy.activateDependencyLocking()",
        failures,
    )
    for build_file in (
        "apps/lorepia/src-tauri/gen/android/build.gradle.kts",
        "apps/lorepia/src-tauri/gen/android/app/build.gradle.kts",
        "apps/lorepia/src-tauri/gen/android/buildSrc/build.gradle.kts",
        "plugins/lorepia-platform/android/build.gradle.kts",
    ):
        require_text(build_file, "lockAllConfigurations()", failures)
        require_text(build_file, "lockMode.set(LockMode.STRICT)", failures)

    for root_build_file in (
        "apps/lorepia/src-tauri/gen/android/build.gradle.kts",
        "plugins/lorepia-platform/android/build.gradle.kts",
    ):
        require_text(root_build_file, 'project.name == "tauri-android"', failures)
        require_text(
            root_build_file,
            'lockFile.set(rootProject.layout.projectDirectory.file("tauri-android-gradle.lockfile"))',
            failures,
        )

    workflow = ".github/workflows/ci.yml"
    for expected in (
        "android-actions/setup-android@40fd30fb8d7440372e1316f5d1809ec01dcd3699",
        "cmdline-tools-version: 14742923",
        "command -v sdkmanager",
        "sdkmanager --version",
        "npm run tauri -- android init --ci --skip-targets-install",
        "python3 scripts/prepare_tauri_android_gradle.py",
        '"platforms;android-36"',
        '"build-tools;36.0.0"',
        '"ndk;27.2.12479018"',
        "buildEnvironment",
        ":app:dependencies",
        "arm64ReleaseRuntimeClasspath",
        ":tauri-plugin-lorepia-platform:dependencies",
        ":tauri-android:dependencies",
        "releaseRuntimeClasspath",
    ):
        require_text(workflow, expected, failures)
    forbid_text(workflow, "node-version-file: apps/lorepia/.node-version", failures)
    require_occurrences(workflow, "node-version-file: .node-version", 2, failures)
    require_occurrences(workflow, "persist-credentials: false", 3, failures)
    require_occurrences(workflow, "--dependency-verification strict", 6, failures)

    if failures:
        for failure in failures:
            print(f"android dependency integrity: {failure}", file=sys.stderr)
        return 1
    print("android dependency integrity: PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
