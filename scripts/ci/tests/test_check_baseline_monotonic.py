"""unittest suite for scripts/ci/check_baseline_monotonic.py.

Pure-function tests over `cbm.compare` and `cbm.parse_baseline_text`, plus
`main(argv)` runs with git content injected via ``base_text``/``head_text``
— git is never invoked, no network, no filesystem fixtures beyond tempfiles.
"""

import contextlib
import io
import os
import sys
import unittest
from pathlib import Path

sys.path.insert(
    0,
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
)

import check_baseline_monotonic as cbm  # noqa: E402


def run_main(
    base_text=None, head_text=None, extra=(), root="/fake/repo"
):
    """Run cbm.main with injected git content; return (exit_code, stdout).

    Pass ``cbm.MISSING`` to simulate the baseline file not existing at that
    revision.  ``None`` now means "do the real git read" — never pass it to
    simulate a missing file (see the round-1 bug noted in the tests below).
    """
    argv = ["--base", "origin/master"] + list(extra)
    buf = io.StringIO()
    with contextlib.redirect_stdout(buf):
        code = cbm.main(argv, root=root, base_text=base_text, head_text=head_text)
    return code, buf.getvalue()


def baseline_text(*lines):
    return "\n".join(lines) + "\n" if lines else ""


class CompareTest(unittest.TestCase):
    def test_no_change_passes(self):
        base = {("dir", "d"): 8, ("file", "a/x.rs"): 1001}
        self.assertEqual(cbm.compare(base, dict(base)), [])

    def test_new_entry_found(self):
        findings = cbm.compare({}, {("file", "big/x.rs"): 1001})
        self.assertEqual(
            [(f.tag, f.kind, f.path, f.head_n) for f in findings],
            [("NEW", "file", "big/x.rs", 1001)],
        )

    def test_grew_entry_found(self):
        findings = cbm.compare({("dir", "d"): 8}, {("dir", "d"): 9})
        self.assertEqual(
            [(f.tag, f.base_n, f.head_n) for f in findings],
            [("GREW", 8, 9)],
        )

    def test_shrank_and_removed_pass(self):
        base = {("dir", "d"): 10, ("dir", "gone"): 5}
        self.assertEqual(cbm.compare(base, {("dir", "d"): 8}), [])

    def test_output_sorted_by_kind_then_path(self):
        head = {
            ("file", "z.rs"): 1,
            ("dir", "a"): 1,
            ("dir", "b"): 1,
        }
        findings = cbm.compare({}, head)
        self.assertEqual(
            [f.key if hasattr(f, "key") else (f.kind, f.path) for f in findings],
            [("dir", "a"), ("dir", "b"), ("file", "z.rs")],
        )


class RenderTest(unittest.TestCase):
    def test_new_line_format(self):
        out = cbm.render_lines([cbm.Finding("NEW", "dir", "d", None, 3)])
        self.assertEqual(out, ["NEW dir d 3"])

    def test_grew_line_format(self):
        out = cbm.render_lines([cbm.Finding("GREW", "file", "a x.rs", 4, 7)])
        self.assertEqual(out, ["GREW file a x.rs 4 -> 7"])


class ParseBaselineTextTest(unittest.TestCase):
    def test_comments_and_blanks_ignored(self):
        entries = cbm.parse_baseline_text(
            baseline_text("# header", "", "   ", "# another", "dir d 8"),
            "test",
        )
        self.assertEqual(entries, {("dir", "d"): 8})

    def test_paths_with_spaces(self):
        entries = cbm.parse_baseline_text("file my dir/x.rs 1001\n", "test")
        self.assertEqual(entries, {("file", "my dir/x.rs"): 1001})

    def test_malformed_line_raises_runtime_error(self):
        with self.assertRaises(RuntimeError):
            cbm.parse_baseline_text("bogus line here\n", "test:base.txt")


class MainTest(unittest.TestCase):
    def test_no_change_exit_0(self):
        text = baseline_text("dir d 8", "file a/x.rs 1001")
        code, out = run_main(base_text=text, head_text=text)
        self.assertEqual(code, 0)
        self.assertIn("baseline monotonic check: OK", out)

    def test_new_entry_exit_1(self):
        code, out = run_main(
            base_text="dir d 8\n",
            head_text=baseline_text("dir d 8", "file new.rs 1001"),
        )
        self.assertEqual(code, 1)
        self.assertIn("NEW file new.rs 1001", out)
        self.assertIn("baseline monotonic check: FAILED", out)

    def test_grew_exit_1(self):
        code, out = run_main(
            base_text="dir d 8\n",
            head_text="dir d 9\n",
        )
        self.assertEqual(code, 1)
        self.assertIn("GREW dir d 8 -> 9", out)

    def test_shrank_exit_0(self):
        code, out = run_main(base_text="dir d 10\n", head_text="dir d 8\n")
        self.assertEqual(code, 0)
        self.assertIn("baseline monotonic check: OK", out)

    def test_removed_exit_0(self):
        code, out = run_main(
            base_text=baseline_text("dir d 8", "dir gone 5"),
            head_text="dir d 8\n",
        )
        self.assertEqual(code, 0)
        self.assertIn("baseline monotonic check: OK", out)

    def test_missing_baseline_at_head_exit_0_with_note(self):
        # Counterfactual (round-1 bug): with ``head_text=None`` main read the
        # content with a real `git show` against /fake/repo; that raised (or
        # on some platforms the test failed for the wrong reason), so this
        # branch — exit 0 + "nothing to protect" — was never exercised.  The
        # MISSING sentinel makes "file absent at this rev" explicit.
        code, out = run_main(base_text="dir d 8\n", head_text=cbm.MISSING)
        self.assertEqual(code, 0)
        self.assertIn("nothing to protect", out)

    def test_missing_baseline_at_base_is_empty(self):
        # Counterfactual (round-1 bug): ``base_text=None`` sent main into the
        # real-git path, so this test ran `git -C /fake/repo show` and never
        # verified the bootstrap rule.  With MISSING, base is treated as an
        # empty baseline and any HEAD entry counts as NEW.
        code, out = run_main(base_text=cbm.MISSING, head_text="dir d 1\n")
        self.assertEqual(code, 1)
        self.assertIn("NEW dir d 1", out)

    def test_missing_at_both_revs_exit_0(self):
        # MISSING at both revs: head wins (nothing to protect).
        code, out = run_main(base_text=cbm.MISSING, head_text=cbm.MISSING)
        self.assertEqual(code, 0)
        self.assertIn("nothing to protect", out)

    def test_malformed_head_line_exit_2(self):
        code, _ = run_main(base_text="", head_text="bogus line here\n")
        self.assertEqual(code, 2)

    def test_malformed_base_line_exit_2(self):
        code, _ = run_main(base_text="bogus\n", head_text="dir d 1\n")
        self.assertEqual(code, 2)

    def test_missing_base_argument_exit_2(self):
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf), contextlib.redirect_stderr(buf):
            code = cbm.main(
                [], root="/fake/repo", base_text="", head_text=""
            )
        self.assertEqual(code, 2)

    def test_baseline_flag_changes_rel_path(self):
        # The --baseline flag is forwarded verbatim; with injected content
        # only the message origin would differ, so assert pass-through works.
        code, out = run_main(
            base_text="dir d 1\n",
            head_text="dir d 1\n",
            extra=["--baseline", "other/baseline.txt"],
        )
        self.assertEqual(code, 0)
        self.assertIn("baseline monotonic check: OK", out)

    def test_head_default_is_HEAD_rev_flag(self):
        # --head is passed to git show verbatim; with injected content we
        # only verify the flag is accepted.
        code, _ = run_main(
            base_text="dir d 1\n",
            head_text="dir d 1\n",
            extra=["--head", "HEAD"],
        )
        self.assertEqual(code, 0)


if __name__ == "__main__":
    unittest.main()
