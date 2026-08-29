import os
import stat
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

SCRIPT_DIRECTORY = Path(__file__).resolve().parent
if str(SCRIPT_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIRECTORY))

import stage_release_candidate
from stage_release_candidate import stage_candidate


class ReleaseCandidateStagingTests(unittest.TestCase):
    def test_stages_only_final_platform_artifacts(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle = root / "bundle"
            bundle.mkdir()
            app_dir = bundle / "appimage" / "LorePia.AppDir"
            app_dir.mkdir(parents=True)
            (app_dir / "runtime-file").write_bytes(b"internal")
            (app_dir / ".DirIcon").symlink_to("runtime-file")
            (bundle / "appimage" / "LorePia_0.1.0_amd64.AppImage").write_bytes(b"appimage")
            (bundle / "deb").mkdir()
            (bundle / "deb" / "LorePia_0.1.0_amd64.deb").write_bytes(b"deb")

            candidate = root / "candidate"
            staged = stage_candidate(bundle, candidate, "linux")

            self.assertEqual(
                [path.relative_to(candidate).as_posix() for path in staged],
                ["appimage/LorePia_0.1.0_amd64.AppImage", "deb/LorePia_0.1.0_amd64.deb"],
            )
            self.assertFalse((candidate / "appimage" / "LorePia.AppDir").exists())

    def test_fails_closed_without_a_final_artifact(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle = root / "bundle"
            bundle.mkdir()
            (bundle / "intermediate.bin").write_bytes(b"not distributable")

            with self.assertRaisesRegex(ValueError, "no final macos artifacts"):
                stage_candidate(bundle, root / "candidate", "macos")

    @unittest.skipIf(os.name == "nt", "POSIX permission bits are required")
    def test_preserves_linux_appimage_executable_mode(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle = root / "bundle"
            bundle.mkdir()
            source = bundle / "LorePia_0.1.0_amd64.AppImage"
            source.write_bytes(b"appimage")
            source.chmod(0o741)

            candidate = root / "candidate"
            stage_candidate(bundle, candidate, "linux")

            destination = candidate / source.name
            self.assertEqual(stat.S_IMODE(destination.stat().st_mode) & 0o111, 0o101)
            self.assertEqual(destination.read_bytes(), b"appimage")

    @unittest.skipIf(os.name == "nt", "POSIX permission bits are required")
    def test_does_not_copy_executable_mode_to_linux_packages(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle = root / "bundle"
            bundle.mkdir()
            for suffix in (".deb", ".rpm"):
                source = bundle / f"LorePia_0.1.0_amd64{suffix}"
                source.write_bytes(suffix.encode())
                source.chmod(0o755)

            candidate = root / "candidate"
            stage_candidate(bundle, candidate, "linux")

            for suffix in (".deb", ".rpm"):
                destination = candidate / f"LorePia_0.1.0_amd64{suffix}"
                self.assertEqual(stat.S_IMODE(destination.stat().st_mode) & 0o111, 0)
                self.assertEqual(destination.read_bytes(), suffix.encode())

    @unittest.skipIf(os.name == "nt", "POSIX permission bits are required")
    def test_non_linux_artifacts_keep_content_only_copy_semantics(self) -> None:
        for platform, suffix in (
            ("macos", ".dmg"),
            ("windows", ".msi"),
            ("windows", ".exe"),
        ):
            with self.subTest(platform=platform), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                bundle = root / "bundle"
                bundle.mkdir()
                source = bundle / f"LorePia{suffix}"
                source.write_bytes(platform.encode())
                source.chmod(0o755)

                candidate = root / "candidate"
                stage_candidate(bundle, candidate, platform)

                destination = candidate / source.name
                self.assertEqual(stat.S_IMODE(destination.stat().st_mode) & 0o111, 0)
                self.assertEqual(destination.read_bytes(), platform.encode())

    @unittest.skipIf(os.name == "nt", "symlink race test requires POSIX no-follow")
    def test_rejects_artifact_swapped_to_symlink_before_open(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle = root / "bundle"
            bundle.mkdir()
            source = bundle / "LorePia.AppImage"
            source.write_bytes(b"approved")
            staged_source = source.resolve(strict=True)
            outside = root / "outside.AppImage"
            outside.write_bytes(b"not-approved")
            real_open = os.open
            swapped = False

            def swap_before_open(
                path: os.PathLike[str] | str,
                flags: int,
                mode: int = 0o777,
                *,
                dir_fd: int | None = None,
            ) -> int:
                nonlocal swapped
                if Path(path) == staged_source and not swapped:
                    source.unlink()
                    source.symlink_to(outside)
                    swapped = True
                if dir_fd is None:
                    return real_open(path, flags, mode)
                return real_open(path, flags, mode, dir_fd=dir_fd)

            with mock.patch.object(
                stage_release_candidate.os, "open", side_effect=swap_before_open
            ):
                with self.assertRaisesRegex(ValueError, "symbolic link|changed"):
                    stage_candidate(bundle, root / "candidate", "linux")

            self.assertFalse((root / "candidate" / source.name).exists())


if __name__ == "__main__":
    unittest.main()
