import unittest
from pathlib import Path
from import_koopman import parse_page


class ImportTests(unittest.TestCase):
    def test_notation_reciprocals_and_tentative_bounds(self):
        source = {"url": "fixture", "sha256": "fixture-digest"}
        html = """<ul><li><tt>(<a>0x8810</a>; 0x11021) &lt;=&gt;
          (0x8408; 0x10811) {32751,&gt;=1000,?HD=5?&gt;=128?} | test</tt></li></ul>"""
        candidates = parse_page(html, 16, source)
        self.assertEqual([c["polynomial"] for c in candidates], ["0x1021", "0x811"])
        self.assertEqual(candidates[0]["hd"][0]["distance"], 3)
        self.assertEqual(candidates[0]["hd"][1]["status"], "lower_bound")
        self.assertEqual(candidates[0]["hd"][2]["status"], "unknown")
        self.assertIsNone(candidates[0]["hd"][2]["max_data_bits"])

    def test_malformed_rows_fail_instead_of_silently_dropping_data(self):
        source = {"url": "fixture", "sha256": "fixture-digest"}
        for html in ["<li>(0x8810; 0x11020) &lt;=&gt; (0x8408; 0x10811) {100}</li>",
                     "<li>(0x8810; 0x11021) {100}</li>",
                     "<li>(0x8810; 0x11021) &lt;=&gt; (0x8408; 0x10811) {?HD=5?}</li>",
                     "<p>source format changed</p>"]:
            with self.assertRaises(ValueError):
                parse_page(html, 16, source)

    def test_all_saved_pages_and_64_bit_conversion(self):
        root = Path(__file__).resolve().parents[1] / "data/koopman/sources"
        for width in [8, 16, 32, 64]:
            candidates = parse_page((root / f"crc{width}.html").read_text(), width,
                                    {"url": "fixture", "sha256": "fixture"})
            self.assertTrue(candidates)
            self.assertEqual(len(candidates), len({c["polynomial"] for c in candidates}))
            self.assertTrue(all(int(c["polynomial"], 16) < 1 << width for c in candidates))
            if width == 64:
                ecma = next(c for c in candidates if c["polynomial"] == "0x42f0e1eba9ea3693")
                self.assertEqual(ecma["koopman"], "0xa17870f5d4f51b49")
                self.assertEqual(ecma["hd"][4]["status"], "lower_bound")
                self.assertEqual(ecma["hd"][6]["status"], "unknown")


if __name__ == "__main__":
    unittest.main()
