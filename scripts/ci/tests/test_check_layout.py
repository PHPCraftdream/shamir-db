"""unittest suite for scripts/ci/check_layout.py.

Integration-style tests through the pinned entry point `cl.main(argv,
tracked_files=..., root=...)` — with `tracked_files` injected, git is never
invoked. All fixtures live in a tempfile.TemporaryDirectory.
"""

import contextlib
import io
import os
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(
    0,
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
)

import check_layout as cl  # noqa: E402


def make_tree(root, tracked):
    """Create `tracked` files (posix rel paths) under `root` with content."""
    for rel, content in tracked.items():
        p = Path(root) / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_bytes(content.encode("utf-8"))


def write_raw_baseline(path, text):
    Path(path).write_text(text, encoding="utf-8", newline="\n")


def scanned(files):
    """Build a tmp tree for `files` and return its ScanResult."""
    ctx = tempfile.TemporaryDirectory()
    self_cleanup = contextlib.ExitStack()
    self_cleanup.callback(ctx.cleanup)
    root = Path(ctx.name)
    make_tree(root, files)
    result = cl.scan(root, sorted(files))
    self_cleanup.close()
    return result


def run(files, extra=(), baseline_text=None, baseline_path=None):
    """Run cl.main against a fresh tmp tree; return (exit_code, stdout, root)."""
    ctx = tempfile.TemporaryDirectory()
    root = Path(ctx.name)
    make_tree(root, files)
    if baseline_text is not None and baseline_path is not None:
        write_raw_baseline(root / baseline_path, baseline_text)
    extra = list(extra)
    if baseline_path is not None:
        extra = ["--baseline", str(root / baseline_path)] + extra
    buf = io.StringIO()
    try:
        with contextlib.redirect_stdout(buf):
            code = cl.main(
                extra,
                tracked_files=sorted(files),
                root=root,
            )
        return code, buf.getvalue(), root
    finally:
        ctx.cleanup()


def one_file(rel="a/x.rs", lines=1):
    return {rel: "x\n" * lines}


class FolderLimitTest(unittest.TestCase):
    def test_seven_children_pass(self):
        files = {"d/f%d.rs" % i: "x\n" for i in range(7)}
        code, out, _ = run(files)
        self.assertEqual(code, 0)
        self.assertIn("layout check: OK", out)

    def test_eight_children_fail(self):
        files = {"d/f%d.rs" % i: "x\n" for i in range(8)}
        code, out, _ = run(files)
        self.assertEqual(code, 1)
        self.assertIn("VIOLATION dir d: 8 direct entries (limit 7)", out)


class FileLimitTest(unittest.TestCase):
    def test_thousand_lines_pass(self):
        code, out, _ = run(one_file(lines=1000))
        self.assertEqual(code, 0)
        self.assertIn("layout check: OK", out)

    def test_thousand_one_lines_fail(self):
        code, out, _ = run(one_file(lines=1001))
        self.assertEqual(code, 1)
        self.assertIn(
            "VIOLATION file a/x.rs: 1001 lines (limit 1000)", out
        )


class ExclusionsTest(unittest.TestCase):
    def test_excluded_dir_file_not_line_checked(self):
        for name in ("docs", "fixtures", "vendor"):
            with self.subTest(name=name):
                code, out, _ = run({"p/%s/big.rs" % name: "x\n" * 5000})
                self.assertEqual(code, 0)
                self.assertNotIn("VIOLATION", out)

    def test_excluded_dir_not_entry_checked_but_occupies_slot(self):
        # 8 files inside docs/: docs itself is never entry-checked, but it
        # still occupies exactly one entry in p's count.
        files = {"p/docs/f%d.rs" % i: "x\n" for i in range(8)}
        files["p/keep.rs"] = "x\n"
        scan = scanned(files)
        self.assertNotIn("p/docs", scan.dir_children)
        self.assertEqual(scan.dir_children["p"], 2)

    def test_container_roots_exempt_from_folder_rule(self):
        files = {"f%d.rs" % i: "x\n" for i in range(9)}
        files.update({"crates/f%d.rs" % i: "x\n" for i in range(9)})
        code, out, _ = run(files)
        self.assertEqual(code, 0)
        self.assertIn("layout check: OK", out)

    def test_non_source_files_count_as_entries_only(self):
        files = {"d/a.js": "x\n", "d/b.md": "x\n", "d/c": "x\n"}
        # One source file keeps `d` itself in scope for the folder rule.
        files["d/s.rs"] = "x\n"
        code, out, _ = run(files)
        self.assertEqual(code, 0)
        self.assertNotIn("VIOLATION", out)
        scan = scanned(files)
        self.assertEqual(scan.dir_children.get("d"), 4)
        self.assertEqual(scan.file_lines, {"d/s.rs": 1})


class BaselinePassTest(unittest.TestCase):
    def test_baselined_violation_passes(self):
        files = {"d/f%d.rs" % i: "x\n" for i in range(8)}
        baseline = "# header\ndir d 8\n"
        ctx = tempfile.TemporaryDirectory()
        try:
            root = Path(ctx.name)
            make_tree(root, files)
            path = root / "base.txt"
            write_raw_baseline(path, baseline)
            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                code = cl.main(
                    ["--baseline", str(path)],
                    tracked_files=sorted(files),
                    root=root,
                )
            self.assertEqual(code, 0)
            self.assertIn("layout check: OK (1 baselined", buf.getvalue())
        finally:
            ctx.cleanup()


class BaselineGrowthTest(unittest.TestCase):
    def test_grown_baselined_entry_fails(self):
        ctx = tempfile.TemporaryDirectory()
        try:
            root = Path(ctx.name)
            files = {"d/f%d.rs" % i: "x\n" for i in range(9)}
            make_tree(root, files)
            path = root / "base.txt"
            cl.write_baseline(path, {("dir", "d"): 8})
            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                code = cl.main(
                    ["--baseline", str(path)],
                    tracked_files=sorted(files),
                    root=root,
                )
            self.assertEqual(code, 1)
            self.assertIn(
                "VIOLATION dir d: 9 direct entries (limit 8 [baselined; hard limit 7])",
                buf.getvalue(),
            )
        finally:
            ctx.cleanup()


class NewViolationTest(unittest.TestCase):
    def test_unbaselined_violation_fails_with_empty_baseline(self):
        files = one_file(lines=1001)
        code, out, _ = run(files, baseline_text="", baseline_path="b.txt")
        self.assertEqual(code, 1)
        self.assertIn("VIOLATION file a/x.rs", out)


class BaselineHintTest(unittest.TestCase):
    def run_with_baseline(self, files, entries):
        ctx = tempfile.TemporaryDirectory()
        self.addCleanup(ctx.cleanup)
        root = Path(ctx.name)
        make_tree(root, files)
        path = root / "base.txt"
        cl.write_baseline(path, entries)
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            code = cl.main(
                ["--baseline", str(path)],
                tracked_files=sorted(files),
                root=root,
            )
        return code, buf.getvalue(), path

    def test_shrunk_but_still_violating_gives_hint_and_passes(self):
        files = {"d/f%d.rs" % i: "x\n" for i in range(8)}
        code, out, _ = self.run_with_baseline(files, {("dir", "d"): 10})
        self.assertEqual(code, 0)
        self.assertIn(
            "HINT dir d: 8 direct entries (baseline 10, limit 7) - shrank; "
            "run --update-baseline to lower to 8",
            out,
        )

    def test_entry_now_within_limit_gives_removal_hint(self):
        files = one_file(lines=5)
        code, out, _ = self.run_with_baseline(files, {("file", "a/x.rs"): 1001})
        self.assertEqual(code, 0)
        self.assertIn(
            "HINT file a/x.rs: 5 lines (baseline 1001, limit 1000) - within "
            "limits; run --update-baseline to remove",
            out,
        )


class UpdateBaselineNeverAddsTest(unittest.TestCase):
    def test_new_violation_not_added_and_blocking(self):
        ctx = tempfile.TemporaryDirectory()
        try:
            root = Path(ctx.name)
            files = one_file(lines=1001)
            files["ok.rs"] = "x\n"
            make_tree(root, files)
            path = root / "base.txt"
            cl.write_baseline(path, {})
            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                code = cl.main(
                    ["--update-baseline", "--baseline", str(path)],
                    tracked_files=sorted(files),
                    root=root,
                )
            self.assertEqual(code, 1)
            self.assertIn(
                "baseline: NOT added file a/x.rs (new violation; "
                "--update-baseline never adds)",
                buf.getvalue(),
            )
            self.assertNotIn("a/x.rs", path.read_text(encoding="utf-8"))
        finally:
            ctx.cleanup()


class UpdateBaselineRatchetTest(unittest.TestCase):
    def setup_tree(self, files, entries):
        ctx = tempfile.TemporaryDirectory()
        self.addCleanup(ctx.cleanup)
        root = Path(ctx.name)
        make_tree(root, files)
        path = root / "base.txt"
        cl.write_baseline(path, entries)
        return root, path

    def run_update(self, files, path):
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            code = cl.main(
                ["--update-baseline", "--baseline", str(path)],
                tracked_files=sorted(files),
                root=Path(path).parent,
            )
        return code, buf.getvalue()

    def test_never_raises_grown_entry(self):
        files = {"d/f%d.rs" % i: "x\n" for i in range(9)}
        root, path = self.setup_tree(files, {("dir", "d"): 8})
        code, out = self.run_update(files, path)
        self.assertEqual(code, 1)
        self.assertIn(
            "baseline: NOT raised dir d (now 9 > baseline 8", out
        )
        self.assertIn("dir d 8\n", path.read_text(encoding="utf-8"))

    def test_lowers_shrunk_entry(self):
        files = {"d/f%d.rs" % i: "x\n" for i in range(8)}
        root, path = self.setup_tree(files, {("dir", "d"): 10})
        code, out = self.run_update(files, path)
        self.assertEqual(code, 0)
        self.assertIn("baseline: lowered dir d 10 -> 8", out)
        self.assertIn("dir d 8\n", path.read_text(encoding="utf-8"))

    def test_removes_entry_now_within_limit(self):
        files = one_file(lines=5)
        root, path = self.setup_tree(files, {("file", "a/x.rs"): 1001})
        code, out = self.run_update(files, path)
        self.assertEqual(code, 0)
        self.assertIn("baseline: removed file a/x.rs (now 5 <= 1000)", out)
        self.assertNotIn("a/x.rs", path.read_text(encoding="utf-8"))

    def test_removes_stale_entry(self):
        files = one_file()
        root, path = self.setup_tree(files, {("file", "gone/lost.rs"): 2000})
        code, out = self.run_update(files, path)
        self.assertEqual(code, 0)
        self.assertIn(
            "baseline: removed file gone/lost.rs (no longer measured)", out
        )
        self.assertNotIn("gone/lost.rs", path.read_text(encoding="utf-8"))

    def test_bootstrap_creates_baseline_with_all_violations(self):
        ctx = tempfile.TemporaryDirectory()
        try:
            root = Path(ctx.name)
            files = {
                "d/f%d.rs" % i: "x\n" for i in range(8)
            }
            files["big/x.rs"] = "x\n" * 1001
            make_tree(root, files)
            path = root / "sub" / "base.txt"
            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                code = cl.main(
                    ["--update-baseline", "--baseline", str(path)],
                    tracked_files=sorted(files),
                    root=root,
                )
            self.assertIn("baseline: created (2 entries)", buf.getvalue())
            text = path.read_text(encoding="utf-8")
            self.assertIn("dir d 8", text)
            self.assertIn("file big/x.rs 1001", text)
            # A re-check with the fresh baseline passes.
            with contextlib.redirect_stdout(io.StringIO()):
                recheck = cl.main(
                    ["--baseline", str(path)],
                    tracked_files=sorted(files),
                    root=root,
                )
            self.assertEqual(recheck, 0)
        finally:
            ctx.cleanup()


class DeepPropagationTest(unittest.TestCase):
    def test_file_free_dir_counts_one_entry_and_is_checked(self):
        files = {"d/s/x/y/f.rs": "x\n"}
        scan = scanned(files)
        self.assertEqual(
            scan.dir_children,
            {"d/s/x/y": 1, "d/s/x": 1, "d/s": 1, "d": 1},
        )
        # d/s has no direct file but its children {x} are entry-checked.
        code, out, _ = run(files)
        self.assertEqual(code, 0)


class RobustnessTest(unittest.TestCase):
    def test_malformed_baseline_line_exit_2(self):
        files = one_file()
        code, out, _ = run(
            files,
            baseline_text="bogus line here\n",
            baseline_path="b.txt",
        )
        self.assertEqual(code, 2)

    def test_comments_and_blank_lines_ignored(self):
        files = one_file()
        baseline = "# comment\n\n   \n# another\n"
        code, out, _ = run(
            files, baseline_text=baseline, baseline_path="b.txt"
        )
        self.assertEqual(code, 0)
        self.assertIn("layout check: OK", out)

    def test_baseline_path_with_space_parses(self):
        files = {"my dir/x.rs": "x\n" * 1001}
        baseline = "file my dir/x.rs 1001\n"
        code, out, _ = run(
            files, baseline_text=baseline, baseline_path="b.txt"
        )
        self.assertEqual(code, 0)
        self.assertIn("layout check: OK (1 baselined", out)

    def test_missing_baseline_behaves_as_empty(self):
        ctx = tempfile.TemporaryDirectory()
        try:
            root = Path(ctx.name)
            files = one_file(lines=1001)
            make_tree(root, files)
            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                code = cl.main(
                    ["--baseline", str(root / "nope.txt")],
                    tracked_files=sorted(files),
                    root=root,
                )
            self.assertEqual(code, 1)
            self.assertIn("VIOLATION file a/x.rs", buf.getvalue())
        finally:
            ctx.cleanup()


if __name__ == "__main__":
    unittest.main()
