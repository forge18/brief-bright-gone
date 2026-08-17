import importlib.util
import unittest
from pathlib import Path


SPEC = importlib.util.spec_from_file_location("run_measurement", Path(__file__).with_name("run_measurement.py"))
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(MODULE)


class MeasurementTests(unittest.TestCase):
    def test_non_sigil_output_is_zero_sigil_not_malformed(self):
        result = MODULE.classify_sigil_output("A plain response without wire markers.")
        self.assertTrue(result["zero_sigil"])
        self.assertFalse(result["malformed"])

    def test_valid_terminal_is_parseable_but_requires_review(self):
        result = MODULE.classify_sigil_output("§ Status\n. done")
        self.assertFalse(result["zero_sigil"])
        self.assertFalse(result["malformed"])
        self.assertTrue(result["manual_review_required"])
        self.assertIsNone(result["silently_misdecoded"])

    def test_non_final_terminal_is_malformed(self):
        result = MODULE.classify_sigil_output(". first\nbody after")
        self.assertTrue(result["malformed"])

    def test_fenced_markers_are_not_counted_as_terminals(self):
        result = MODULE.classify_sigil_output("```\n. literal\n```")
        self.assertTrue(result["zero_sigil"])
        self.assertFalse(result["malformed"])


if __name__ == "__main__":
    unittest.main()
