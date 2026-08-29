import importlib.util
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).with_name("check_i18n_literal_baseline.py")
SPEC = importlib.util.spec_from_file_location("check_i18n_literal_baseline", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class I18nLiteralBaselineTests(unittest.TestCase):
    def test_fingerprint_is_order_and_content_sensitive(self) -> None:
        first = MODULE.line_digest(["const value = '안녕';", "<!-- 설명 -->"])
        self.assertEqual(first, MODULE.line_digest(["const value = '안녕';", "<!-- 설명 -->"]))
        self.assertNotEqual(first, MODULE.line_digest(["<!-- 설명 -->", "const value = '안녕';"]))
        self.assertNotEqual(first, MODULE.line_digest(["const value = '안녕하세요';", "<!-- 설명 -->"]))

    def test_base_comparison_allows_only_existing_lines_or_removals(self) -> None:
        base = {"src/a.ts": ["const a = '기존';", "const b = '제거';"]}
        self.assertEqual(
            MODULE.compare_to_base({"src/a.ts": ["const a = '기존';"]}, base), []
        )
        failures = MODULE.compare_to_base(
            {"src/a.ts": ["const a = '기존';", "const c = '신규';"]}, base
        )
        self.assertEqual(len(failures), 1)

    def test_baseline_reports_new_changed_and_removed_debt(self) -> None:
        expected = MODULE.baseline_payload({"src/a.ts": ["const a = '기존';"]})
        changed = MODULE.baseline_payload({"src/a.ts": ["const a = '변경';"]})
        removed = MODULE.baseline_payload({})
        added = MODULE.baseline_payload({"src/b.ts": ["const b = '신규';"]})
        self.assertEqual(len(MODULE.compare_baseline(changed, expected)), 1)
        self.assertEqual(len(MODULE.compare_baseline(removed, expected)), 1)
        self.assertEqual(len(MODULE.compare_baseline(added, expected)), 2)


if __name__ == "__main__":
    unittest.main()
