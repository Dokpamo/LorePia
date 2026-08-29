import hashlib
import os
import stat
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path

SCRIPT_DIRECTORY = Path(__file__).resolve().parent
if str(SCRIPT_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIRECTORY))

from package_linux_release_candidate import package_linux_candidate
from write_release_checksums import write_checksums


class LinuxReleaseCandidatePackagingTests(unittest.TestCase):
    def _candidate(self, root: Path, mtime: int) -> Path:
        candidate = root / "candidate"
        (candidate / "appimage").mkdir(parents=True)
        appimage = candidate / "appimage" / "LorePia.AppImage"
        appimage.write_bytes(b"appimage")
        appimage.chmod(0o755)
        package = candidate / "LorePia.deb"
        package.write_bytes(b"deb")
        package.chmod(0o644)
        manifest = candidate / "SHA256SUMS"
        manifest.write_text("candidate checksums\n", encoding="utf-8")
        sbom = candidate / "lorepia-Linux.spdx.json"
        sbom.write_text('{"spdxVersion":"SPDX-2.3"}\n', encoding="utf-8")
        for path in (
            candidate,
            candidate / "appimage",
            appimage,
            package,
            manifest,
            sbom,
        ):
            os.utime(path, (mtime, mtime), follow_symlinks=False)
        return candidate

    @unittest.skipIf(os.name == "nt", "POSIX tar mode verification is required")
    def test_archive_is_deterministic_and_preserves_appimage_mode(self) -> None:
        with (
            tempfile.TemporaryDirectory() as first_temporary,
            tempfile.TemporaryDirectory() as second_temporary,
        ):
            first_root = Path(first_temporary)
            second_root = Path(second_temporary)
            first_candidate = self._candidate(first_root, 1_700_000_000)
            second_candidate = self._candidate(second_root, 1_800_000_000)
            first_output = first_root / "candidate.tar.gz"
            second_output = second_root / "candidate.tar.gz"

            package_linux_candidate(first_candidate, first_output)
            package_linux_candidate(second_candidate, second_output)

            self.assertEqual(first_output.read_bytes(), second_output.read_bytes())
            with tarfile.open(first_output, "r:gz") as archive:
                members = archive.getmembers()
                self.assertEqual(
                    [member.name for member in members],
                    [
                        "LorePia.deb",
                        "SHA256SUMS",
                        "appimage",
                        "appimage/LorePia.AppImage",
                        "lorepia-Linux.spdx.json",
                    ],
                )
                appimage = archive.getmember("appimage/LorePia.AppImage")
                package = archive.getmember("LorePia.deb")
                self.assertEqual(appimage.mode, 0o755)
                self.assertEqual(package.mode, 0o644)
                self.assertTrue(
                    all(
                        member.uid == 0
                        and member.gid == 0
                        and member.uname == ""
                        and member.gname == ""
                        and member.mtime == 0
                        for member in members
                    )
                )

    def test_upload_checksum_subjects_the_tarball(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            candidate = self._candidate(root, 1_700_000_000)
            upload = root / "upload"
            upload.mkdir()
            archive = upload / "lorepia-UNSIGNED-candidate-Linux.tar.gz"

            package_linux_candidate(candidate, archive)
            manifest = write_checksums(upload)

            digest = hashlib.sha256(archive.read_bytes()).hexdigest()
            self.assertEqual(
                manifest.read_text(encoding="utf-8"),
                f"{digest}  {archive.name}\n",
            )

    def test_rejects_symbolic_links(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            candidate = root / "candidate"
            candidate.mkdir()
            artifact = candidate / "LorePia.AppImage"
            artifact.write_bytes(b"appimage")
            (candidate / "alias.AppImage").symlink_to(artifact)

            with self.assertRaisesRegex(ValueError, "symbolic link"):
                package_linux_candidate(candidate, root / "candidate.tar.gz")

            self.assertFalse((root / "candidate.tar.gz").exists())


if __name__ == "__main__":
    unittest.main()
