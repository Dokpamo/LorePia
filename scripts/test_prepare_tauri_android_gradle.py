#!/usr/bin/env python3
"""Regression tests for Android Gradle input preparation."""

from __future__ import annotations

import shutil
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

SCRIPT_DIRECTORY = Path(__file__).resolve().parent
if str(SCRIPT_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIRECTORY))

import prepare_tauri_android_gradle as prepare


class PrepareTauriAndroidGradleTests(unittest.TestCase):
    def fixture(
        self, fixture_root: Path
    ) -> tuple[Path, Path, Path, Path, list[dict[str, object]]]:
        app_android = fixture_root / "app-android"
        (app_android / "app").mkdir(parents=True)

        tauri_root = fixture_root / "tauri"
        tauri_android = tauri_root / "mobile/android"
        tauri_android.mkdir(parents=True)
        (tauri_root / "Cargo.toml").write_text("", encoding="utf-8")
        (tauri_android / "build.gradle.kts").write_text("", encoding="utf-8")
        (tauri_android / "source.txt").write_text("source", encoding="utf-8")

        plugin_root = fixture_root / "plugin"
        plugin_android = plugin_root / "android"
        plugin_android.mkdir(parents=True)
        (plugin_root / "Cargo.toml").write_text("", encoding="utf-8")
        (plugin_android / "build.gradle.kts").write_text("", encoding="utf-8")

        packages: list[dict[str, object]] = [
            {
                "name": "tauri",
                "version": prepare.EXPECTED_TAURI_VERSION,
                "manifest_path": str(tauri_root / "Cargo.toml"),
            },
            {
                "name": "tauri-plugin-lorepia-platform",
                "version": "0.1.0",
                "manifest_path": str(plugin_root / "Cargo.toml"),
            },
        ]
        return app_android, tauri_android, plugin_root, plugin_android, packages

    def run_main(
        self,
        app_android: Path,
        plugin_root: Path,
        packages: list[dict[str, object]],
    ) -> None:
        with (
            mock.patch.object(prepare, "REPO_ROOT", plugin_root.parent),
            mock.patch.object(prepare, "APP_ANDROID", app_android),
            mock.patch.object(prepare, "PLUGIN_ROOT", plugin_root),
            mock.patch.object(prepare, "cargo_packages", return_value=packages),
        ):
            prepare.main()

    def test_rejects_intermediate_tauri_symlink_before_recursive_copy(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            fixture_root = Path(temporary_directory).resolve()
            app_android, _, plugin_root, plugin_android, packages = self.fixture(
                fixture_root
            )

            external_directory = fixture_root / "external"
            external_tauri_api = external_directory / "tauri-api"
            external_tauri_api.mkdir(parents=True)
            sentinel = external_tauri_api / "must-survive.txt"
            sentinel.write_text("do not delete", encoding="utf-8")
            (plugin_android / ".tauri").symlink_to(
                external_directory, target_is_directory=True
            )

            with self.assertRaisesRegex(RuntimeError, "symlink"):
                self.run_main(app_android, plugin_root, packages)

            self.assertEqual(sentinel.read_text(encoding="utf-8"), "do not delete")

    def test_rejects_symlinked_app_android_directory(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            fixture_root = Path(temporary_directory).resolve()
            app_android, _, plugin_root, _, packages = self.fixture(fixture_root)
            shutil.rmtree(app_android)
            external = fixture_root / "external-app-android"
            (external / "app").mkdir(parents=True)
            app_android.symlink_to(external, target_is_directory=True)

            with self.assertRaisesRegex(RuntimeError, "symlink"):
                self.run_main(app_android, plugin_root, packages)

            self.assertFalse((external / "tauri.settings.gradle").exists())
            self.assertFalse((external / "app/tauri.build.gradle.kts").exists())

    def test_rejects_symlinked_app_subdirectory(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            fixture_root = Path(temporary_directory).resolve()
            app_android, _, plugin_root, _, packages = self.fixture(fixture_root)
            shutil.rmtree(app_android / "app")
            external = fixture_root / "external-app"
            external.mkdir()
            sentinel = external / "tauri.build.gradle.kts"
            sentinel.write_text("do not overwrite", encoding="utf-8")
            (app_android / "app").symlink_to(external, target_is_directory=True)

            with self.assertRaisesRegex(RuntimeError, "symlink"):
                self.run_main(app_android, plugin_root, packages)

            self.assertEqual(sentinel.read_text(encoding="utf-8"), "do not overwrite")

    def test_rejects_symlinked_output_file(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            fixture_root = Path(temporary_directory).resolve()
            app_android, _, plugin_root, _, packages = self.fixture(fixture_root)
            external = fixture_root / "external-settings.gradle"
            external.write_text("do not overwrite", encoding="utf-8")
            (app_android / "tauri.settings.gradle").symlink_to(external)

            with self.assertRaisesRegex(RuntimeError, "symlink"):
                self.run_main(app_android, plugin_root, packages)

            self.assertEqual(external.read_text(encoding="utf-8"), "do not overwrite")

    def test_rejects_symlinked_tauri_api_directory(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            fixture_root = Path(temporary_directory).resolve()
            app_android, _, plugin_root, plugin_android, packages = self.fixture(
                fixture_root
            )
            copied_parent = plugin_android / ".tauri"
            copied_parent.mkdir()
            external_tauri_api = fixture_root / "external-tauri-api"
            external_tauri_api.mkdir()
            sentinel = external_tauri_api / "must-survive.txt"
            sentinel.write_text("do not delete", encoding="utf-8")
            (copied_parent / "tauri-api").symlink_to(
                external_tauri_api, target_is_directory=True
            )

            with self.assertRaisesRegex(RuntimeError, "symlink"):
                self.run_main(app_android, plugin_root, packages)

            self.assertEqual(sentinel.read_text(encoding="utf-8"), "do not delete")

    def test_parent_swap_cannot_redirect_recursive_deletion(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            fixture_root = Path(temporary_directory).resolve()
            app_android, _, plugin_root, plugin_android, packages = self.fixture(
                fixture_root
            )
            copied_parent = plugin_android / ".tauri"
            copied_tauri_api = copied_parent / "tauri-api"
            copied_tauri_api.mkdir(parents=True)
            (copied_tauri_api / "old.txt").write_text("old", encoding="utf-8")

            external = fixture_root / "external"
            external_tauri_api = external / "tauri-api"
            external_tauri_api.mkdir(parents=True)
            sentinel = external_tauri_api / "must-survive.txt"
            sentinel.write_text("do not delete", encoding="utf-8")

            original_remove_tree = prepare.remove_tree_at
            raced = False

            def racing_remove_tree(
                parent_fd: int, name: str, description: str
            ) -> None:
                nonlocal raced
                if not raced and name == "tauri-api":
                    raced = True
                    copied_parent.rename(plugin_android / ".tauri-original")
                    copied_parent.symlink_to(external, target_is_directory=True)
                original_remove_tree(parent_fd, name, description)

            with mock.patch.object(
                prepare, "remove_tree_at", side_effect=racing_remove_tree
            ):
                with self.assertRaisesRegex(RuntimeError, "symlink"):
                    self.run_main(app_android, plugin_root, packages)

            self.assertEqual(sentinel.read_text(encoding="utf-8"), "do not delete")

    def test_replaces_generated_tauri_api_for_legitimate_repeat_runs(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            fixture_root = Path(temporary_directory).resolve()
            app_android, _, plugin_root, plugin_android, packages = self.fixture(
                fixture_root
            )

            self.run_main(app_android, plugin_root, packages)
            copied_tauri_api = plugin_android / ".tauri/tauri-api"
            self.assertEqual(
                (copied_tauri_api / "source.txt").read_text(encoding="utf-8"),
                "source",
            )
            stale = copied_tauri_api / "stale.txt"
            stale.write_text("stale", encoding="utf-8")

            self.run_main(app_android, plugin_root, packages)

            self.assertFalse(stale.exists())
            self.assertEqual(
                (copied_tauri_api / "source.txt").read_text(encoding="utf-8"),
                "source",
            )


if __name__ == "__main__":
    unittest.main()
