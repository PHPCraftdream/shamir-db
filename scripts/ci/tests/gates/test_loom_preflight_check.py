"""unittest suite for scripts/ci/gates/loom_preflight_check.py.

Pure ``count_matching_tests`` / ``main`` tests over nextest list JSON
documents injected via the ``list_text`` keyword (same injection style as
test_rust_guard_select.py) — no subprocess, no filesystem fixture, no
network.
"""

import contextlib
import io
import json
import os
import sys
import unittest

sys.path.insert(
    0,
    os.path.join(
        os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))),
        "gates",
    ),
)

import loom_preflight_check as lpc  # noqa: E402


def doc(testcases):
    """Build a captured-shape nextest list document."""
    return json.dumps(
        {
            "rust-build-meta": {"target-directory": "target"},
            "test-count": len(testcases),
            "rust-suites": {
                "shamir-engine-1234abcdef": {
                    "binary-id": "shamir-engine-1234abcdef",
                    "testcases": testcases,
                }
            },
        }
    )


def tc(status="matches", ignored=False):
    return {"ignored": ignored, "filter-match": {"status": status}}


def run_main(argv=(), text=None):
    """Run lpc.main with injected list text; return (exit_code, stdout, stderr)."""
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        code = lpc.main(list(argv), list_text=text)
    return code, out.getvalue(), err.getvalue()


class CountTests(unittest.TestCase):
    def test_counts_matching_testcases(self):
        document = json.loads(
            doc({"a::t1": tc(), "a::t2": tc("mismatches"), "b::t3": tc()})
        )
        self.assertEqual(lpc.count_matching_tests(document, ""), 2)

    def test_mismatches_excluded(self):
        document = json.loads(doc({"a::t1": tc("mismatches")}))
        self.assertEqual(lpc.count_matching_tests(document, ""), 0)

    def test_name_filter_counts_only_containing_names(self):
        document = json.loads(doc({"loom::a": tc(), "other::b": tc()}))
        self.assertEqual(lpc.count_matching_tests(document, "loom::"), 1)

    def test_empty_suites_count_zero(self):
        document = json.loads(doc({}))
        self.assertEqual(lpc.count_matching_tests(document, ""), 0)


class MainTests(unittest.TestCase):
    def test_matches_exit_0_prints_count(self):
        code, out, err = run_main(
            ["--list-file", "list.json"], text=doc({"a::t1": tc()})
        )
        self.assertEqual((code, err), (0, ""))
        self.assertIn("1 matching test(s)", out)

    def test_zero_matches_exit_1(self):
        code, _, err = run_main(
            ["--list-file", "list.json"], text=doc({"a::t1": tc("mismatches")})
        )
        self.assertEqual(code, 1)
        self.assertIn("no tests matched", err)

    def test_name_filter_main(self):
        code, _, err = run_main(
            ["--list-file", "list.json", "--name-filter", "loom"],
            text=doc({"loom::a": tc(), "other::b": tc()}),
        )
        self.assertEqual(code, 0)
        code2, _, err2 = run_main(
            ["--list-file", "list.json", "--name-filter", "nomatch"],
            text=doc({"loom::a": tc()}),
        )
        self.assertEqual(code2, 1)
        self.assertIn("no tests matched", err2)

    def test_missing_rust_suites_exit_2(self):
        code, _, err = run_main(
            ["--list-file", "list.json"], text=json.dumps({"test-count": 3})
        )
        self.assertEqual(code, 2)
        self.assertIn("rust-suites", err)

    def test_bad_json_exit_2(self):
        code, _, err = run_main(["--list-file", "list.json"], text="{nope")
        self.assertEqual(code, 2)
        self.assertIn("unreadable nextest list", err)

    def test_non_object_json_exit_2(self):
        code, _, err = run_main(["--list-file", "list.json"], text="[1, 2]")
        self.assertEqual(code, 2)

    def test_missing_file_exit_2(self):
        code, _, err = run_main(
            ["--list-file", "does/not/exist.json"]
        )
        self.assertEqual(code, 2)
        self.assertIn("unreadable nextest list", err)


if __name__ == "__main__":
    unittest.main()
