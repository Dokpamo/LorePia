import hashlib
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT_DIRECTORY = Path(__file__).resolve().parent
if str(SCRIPT_DIRECTORY) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIRECTORY))

from write_release_checksums import write_checksums


class ReleaseChecksumTests(unittest.TestCase):
    def test_writes_sorted_relative_sha256_manifest(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "nested").mkdir()
            (root / "z.bin").write_bytes(b"z")
            (root / "nested" / "a.bin").write_bytes(b"a")

            output = write_checksums(root)

            self.assertEqual(output, root.resolve() / "SHA256SUMS")
            self.assertEqual(
                output.read_text(encoding="utf-8").splitlines(),
                [
                    f"{hashlib.sha256(b'a').hexdigest()}  nested/a.bin",
                    f"{hashlib.sha256(b'z').hexdigest()}  z.bin",
                ],
            )


if __name__ == "__main__":
    unittest.main()
