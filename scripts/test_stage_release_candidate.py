import tempfile
import sys
import unittest
from pathlib import Path

SCRIPT_DIRECTORY = Path(__file__).resolve().parent
if str(SCRIPT_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIRECTORY))

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


if __name__ == "__main__":
    unittest.main()
