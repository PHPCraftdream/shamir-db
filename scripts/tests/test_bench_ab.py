"""Canned-data tests only; run by the PR selftest job, never benchmark code."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "bench_ab.py"
SPEC = importlib.util.spec_from_file_location("bench_ab", SCRIPT)
ab = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ab)


def sample(ns, count=10):
    return {"iters": count, "ns": ns, "ms": ns * count / 1e6}


class ParserTests(unittest.TestCase):
    def test_seven_token_harness_format(self):
        rows = ab.parse_output("== run `demo` (fixed iterations) ==\n"
                               "  cell/1 10000 iters 10.000 ms 1000.00 ns/op\n"
                               "  `demo` wall 0.010s SUM(ns/op) = 1000.0\n",
                               {"cell/1": 10000})
        self.assertEqual(rows["cell/1"], sample(1000, 10000))

    def test_malformed_duplicate_missing_stray_and_calibration(self):
        valid = "cell 10000 iters 10.000 ms 1000.00 ns/op\n"
        bad = ["", valid + valid, valid + "stray 1 iters 1 ms 1000000 ns/op\n",
               "cell 10000 iters 10 ms 1000 ns/op extra", "cell ten iters 10 ms 1000 ns/op",
               "cell 10000 iters 10 ms NaN ns/op", "cell 10000 iters 10 ms inf ns/op",
               "cell 10000 iters 10 ms 0 ns/op", "cell 10000 iters 10 ms -1 ns/op",
               "cell 10000 iters 10 us 1000 ns/op", "cell 10000 iters 10 ms 10 ns/op",
               valid + "JIT-calibrated — was missing from manifest"]
        for text in bad:
            with self.subTest(text=text), self.assertRaises(ValueError):
                ab.parse_output(text, {"cell": 10000})

    def test_rounded_zero_milliseconds_with_positive_ns_is_valid(self):
        self.assertEqual(ab.parse_output("cell 1 iters 0.000 ms 100.00 ns/op", {"cell": 1}),
                         {"cell": sample(100, 1) | {"ms": 0.0}})

    def test_full_key_filter_still_returns_bare_ids_and_prefix_matches(self):
        # Full demo::cell/100 is a substring of demo::cell/1000 too.
        rows = ab.parse_output(
            "== run `demo` (fixed iterations, scale=1.000, filter: `demo::cell/100`) ==\n"
            "cell/100 10 iters 1.000 ms 100000.00 ns/op\n"
            "cell/1000 20 iters 2.000 ms 100000.00 ns/op\n",
            {"cell/100": 10, "cell/1000": 20})
        self.assertEqual(set(rows), {"cell/100", "cell/1000"})

    def test_actual_count_must_match_frozen_scaled_count(self):
        with self.assertRaises(ValueError):
            ab.parse_output("cell 10 iters 1.000 ms 100000 ns/op", {"cell": 20})


class PairingTests(unittest.TestCase):
    def test_median_of_paired_ratios_not_ratio_of_medians(self):
        a = [sample(x) for x in [1, 10, 100, 1000, 10000]]
        b = [sample(x) for x in [2, 20, 50, 500, 20000]]
        result = ab.paired(a, b, 15)
        self.assertEqual(result["ratios"], [2, 2, 0.5, 0.5, 2])
        self.assertEqual(result["change_pct"], 100)
        self.assertEqual(result["a_ns"], 100)
        self.assertEqual(result["b_ns"], 50)

    def test_unequal_counts_fail_cell(self):
        with self.assertRaisesRegex(ValueError, "Unequal"):
            ab.paired([sample(100)] * 5, [sample(100, 11)] * 5, 15)

    def test_incomplete_pairs_fail(self):
        with self.assertRaises(ValueError):
            ab.paired([sample(100)] * 4, [sample(100)] * 5, 15)

    def test_class_thresholds_and_robust_noise(self):
        a = [sample(100)] * 5
        self.assertEqual(ab.paired(a, [sample(120)] * 5, 15)["status"], "flagged")
        self.assertEqual(ab.paired(a, [sample(120)] * 5, 35)["status"], "within margin")
        noisy = ab.paired(a, [sample(x) for x in [70, 90, 110, 130, 150]], 15)
        self.assertEqual(noisy["status"], "inconclusive")
        self.assertGreater(noisy["spread_pct"], 7.5)

    def test_two_predeclared_dedicated_batches(self):
        flagged = {"status": "flagged"}
        ok = {"status": "within margin"}
        self.assertEqual(ab.policy([flagged], "dedicated"), "inconclusive")
        self.assertEqual(ab.policy([flagged, flagged], "dedicated"), "flagged")
        self.assertEqual(ab.policy([flagged, ok], "dedicated"), "inconclusive")
        self.assertEqual(ab.policy([ok, ok], "dedicated"), "within margin")
        self.assertEqual(ab.policy([flagged], "hosted"), "flagged")


class PreflightTests(unittest.TestCase):
    def setUp(self):
        self.manifest = ab.load_manifest(SCRIPT.with_suffix(".toml"))
        self.disjoint = next(b for b in self.manifest["benches"]
                             if b["bench"] == "tx_concurrent" and b.get("optional"))
        self.absent = '''
            // disjoint_inserts/n_4 and n_8 were once cached.
            for &n in &[1usize] {
                let iter_ctr = Arc::clone(&iter_ctr);
                h.bench_batched_async(&format!("disjoint_inserts/n_{n}"),
                                     setup, routine);
            }
        '''
        self.present = self.absent.replace("[1usize]", "[1usize, 4, 8]")

    def test_cached_absent_n4_n8_remain_unsupported(self):
        counts = {"tx_concurrent::disjoint_inserts/n_4": 65,
                  "tx_concurrent::disjoint_inserts/n_8": 33}
        for cell in self.disjoint["cells"]:
            reason = ab.preflight_cell(self.disjoint, cell, [self.absent] * 2,
                                       counts, "compare")
            self.assertIn("not registered", reason)

    def test_optional_present_fixture_and_count_selects(self):
        key = "tx_concurrent::disjoint_inserts/n_4"
        self.assertIsNone(ab.preflight_cell(self.disjoint, "disjoint_inserts/n_4",
                                           [self.present] * 2, {key: 65}, "compare"))
        self.assertIn("not registered", ab.preflight_cell(
            self.disjoint, "disjoint_inserts/n_4", [self.present, self.absent],
            {key: 65}, "compare"))

    def test_optional_missing_count_is_unsupported_but_calibration_selects(self):
        cell = "disjoint_inserts/n_4"
        self.assertIn("lacks reviewed iteration", ab.preflight_cell(
            self.disjoint, cell, [self.present] * 2, {}, "compare"))
        self.assertIsNone(ab.preflight_cell(self.disjoint, cell,
                                           [self.present] * 2, {}, "calibrate"))
        self.assertIn("not registered", ab.preflight_cell(
            self.disjoint, cell, [self.absent] * 2, {}, "calibrate"))

    def test_required_missing_count_fails(self):
        bench = {"bench": "demo", "optional": False}
        with self.assertRaisesRegex(ValueError, "required cell lacks"):
            ab.preflight_cell(bench, "cell", ['h.bench("cell", routine);'] * 2,
                              {}, "compare")
        self.assertIsNone(ab.preflight_cell(bench, "cell", [], {}, "calibrate"))

    def test_pushdown_missing_counts_can_be_calibrated(self):
        bench = next(b for b in self.manifest["benches"]
                     if b["bench"] == "select_pipeline" and b.get("optional"))
        source = '''
            for &n in &[2_000usize, 5_000usize] {
                h.bench_async(&format!("read_limit_pushdown/pushdown_active_N={n}"), routine);
                h.bench_async(&format!("read_limit_pushdown/pushdown_disabled_N={n}"), routine);
            }
        '''
        for cell in bench["cells"]:
            self.assertIn("lacks reviewed iteration", ab.preflight_cell(
                bench, cell, [source] * 2, {}, "compare"))
            self.assertIsNone(ab.preflight_cell(bench, cell, [source] * 2, {}, "calibrate"))

    def test_unknown_optional_source_never_inferred_from_counts(self):
        bench = {"bench": "future", "optional": True}
        self.assertIn("no reviewed exact source", ab.preflight_cell(
            bench, "cell", ['h.bench("cell", routine);'] * 2,
            {"future::cell": 20}, "compare"))

    def test_selected_cells_blocked_by_build_run_are_unmeasured(self):
        result = {"selected": [{"key": "demo::one"}, {"key": "demo::two"}],
                  "cells": [], "errors": ["Build failed"]}
        self.assertEqual(ab.unmeasured_rows(result),
                         [{"cell": "demo::one", "reason": "Build failed"},
                          {"cell": "demo::two", "reason": "Build failed"}])
        result["cells"] = [{"key": "demo::one"}]
        result["errors"] = ["Run timed out"]
        self.assertEqual(ab.unmeasured_rows(result),
                         [{"cell": "demo::two", "reason": "Run timed out"}])


class ManifestTests(unittest.TestCase):
    def test_reviewed_manifest_ranks_classes_and_optional_coverage(self):
        manifest = ab.load_manifest(SCRIPT.with_suffix(".toml"))
        self.assertEqual({b["rank"] for b in manifest["benches"]}, set(range(1, 8)))
        self.assertEqual(manifest["scale"], 1)
        self.assertEqual(manifest["thresholds"]["io"]["hosted"], 40)
        self.assertTrue(any(b.get("optional") for b in manifest["benches"]))
        self.assertTrue(manifest["unsupported"])

    def test_counts_digest_and_reject_duplicates_zero_malformed(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "counts.txt"
            path.write_text("# frozen\nbench::cell = 20\n")
            self.assertEqual(ab.load_counts(path), {"bench::cell": 20})
            before = ab.digest(path)
            for text in ["bench::cell = 0", "malformed", "bench::cell = 1\nbench::cell = 2"]:
                path.write_text(text)
                with self.assertRaises(ValueError):
                    ab.load_counts(path)
            self.assertNotEqual(ab.digest(path), before)

    def test_full_immutable_sha_only(self):
        self.assertEqual(ab.validate_sha(ab.BASELINE), ab.BASELINE)
        for value in ["master", ab.BASELINE[:8], ab.BASELINE.upper(), ab.BASELINE + "\n",
                      "$(echo master)", "-" + ab.BASELINE]:
            with self.subTest(value=value), self.assertRaises(ValueError):
                ab.validate_sha(value)


if __name__ == "__main__":
    unittest.main()
