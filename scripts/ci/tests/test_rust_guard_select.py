"""unittest suite for scripts/ci/rust_guard_select.py.

Pure-function tests over `select` / `strip_root` / `load_campaigns`, plus
`main(argv)` runs with the stages TOML, the cargo metadata JSON and the
changed-file list injected as keyword arguments — no filesystem fixtures
beyond what `main` needs, no git, no cargo, no stdin.
"""

import contextlib
import io
import os
import sys
import unittest

sys.path.insert(
    0,
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
)

import rust_guard_select as rgs  # noqa: E402

STAGES = b"""
[[campaigns]]
name = "split-repo-instance"
packages = ["shamir-db"]
delta = ".rust-guard/stages/split-repo-instance.toml"

[[campaigns]]
name = "extract-engine"
packages = ["shamir-engine", "ghost-crate"]
delta = ".rust-guard/stages/extract-engine.toml"
"""

# Absolute manifest paths as a Linux CI runner's `cargo metadata` emits them.
METADATA = """{
  "packages": [
    {"name": "shamir-db",
     "manifest_path": "/home/runner/work/repo/crates/shamir-db/Cargo.toml"},
    {"name": "shamir-engine",
     "manifest_path": "/home/runner/work/repo/crates/shamir-engine/Cargo.toml"}
  ]
}
"""
ROOT = "/home/runner/work/repo"


def run_main(stages=STAGES, metadata=METADATA, root=ROOT, changed=(), extra=()):
    """Run rgs.main with injected content; return (exit_code, stdout)."""
    argv = [
        "--stages", "s.toml",
        "--metadata", "m.json",
        "--root", root,
    ] + list(extra)
    buf = io.StringIO()
    with contextlib.redirect_stdout(buf):
        code = rgs.main(
            argv, stages_text=stages, metadata_text=metadata,
            changed=list(changed),
        )
    return code, buf.getvalue()


class StripRootTest(unittest.TestCase):
    def test_strips_root_prefix(self):
        self.assertEqual(
            rgs.strip_root(ROOT + "/crates/x/Cargo.toml", ROOT),
            "crates/x/Cargo.toml",
        )

    def test_normalises_backslashes(self):
        # Windows captures: backslash separators on both sides.
        self.assertEqual(
            rgs.strip_root("D:\\repo\\crates\\x\\Cargo.toml", "D:\\repo"),
            "crates/x/Cargo.toml",
        )

    def test_non_prefixed_path_is_kept_normalised(self):
        self.assertEqual(rgs.strip_root("/elsewhere/x", ROOT), "/elsewhere/x")


class SelectTest(unittest.TestCase):
    def pkg_dirs(self):
        return {
            "shamir-db": "crates/shamir-db",
            "shamir-engine": "crates/shamir-engine",
        }

    def test_absolute_metadata_vs_relative_diff_matches_after_fix(self):
        # THE round-1 bug: with absolute pkg dirs (no --root strip), the
        # repo-relative diff paths match nothing and every campaign is
        # silently skipped.  After the fix the same inputs match.
        campaigns = rgs.load_campaigns(STAGES)
        abs_dirs = {
            "shamir-db": ROOT + "/crates/shamir-db",
            "shamir-engine": ROOT + "/crates/shamir-engine",
        }
        self.assertEqual(
            rgs.select(campaigns, abs_dirs, ["crates/shamir-db/src/lib.rs"]),
            [],
        )
        fixed = rgs.select(
            campaigns, self.pkg_dirs(), ["crates/shamir-db/src/lib.rs"]
        )
        self.assertEqual(
            [(n, d, p) for n, d, p in fixed],
            [("split-repo-instance",
              ".rust-guard/stages/split-repo-instance.toml",
              ["shamir-db"])],
        )

    def test_backslash_separators_in_metadata_capture(self):
        # A Windows-captured metadata with backslash manifest dirs must
        # still match the forward-slash diff paths; without separator
        # normalisation the pkg dir would be "crates\\shamir-db" and the
        # startswith check would fail.
        campaigns = [{"name": "c", "packages": ["shamir-db"], "delta": "d.toml"}]
        dirs = {"shamir-db": "crates\\shamir-db"}
        self.assertEqual(
            [(n, d, p) for n, d, p in
             rgs.select(campaigns, {k: rgs.norm(v) for k, v in dirs.items()},
                        ["crates/shamir-db/src/lib.rs"])],
            [("c", "d.toml", ["shamir-db"])],
        )
        # Counterfactual: unnormalised, nothing matches.
        self.assertEqual(rgs.select(campaigns, dirs, ["crates/shamir-db/s.rs"]), [])

    def test_multiple_campaigns_both_selected(self):
        # Only the second campaign's ghost package is unknown, so its
        # known list is trimmed but it still runs.
        campaigns = rgs.load_campaigns(STAGES)
        out = rgs.select(
            campaigns, self.pkg_dirs(),
            ["crates/shamir-engine/src/a.rs", "crates/shamir-db/src/b.rs"],
        )
        self.assertEqual(
            [(n, p) for n, _, p in out],
            [("split-repo-instance", ["shamir-db"]),
             ("extract-engine", ["shamir-engine"])],
        )

    def test_campaign_with_only_unknown_packages_never_matches(self):
        # ghost-crate is not in the metadata; even a "crates/ghost-crate/..."
        # diff line must not select the campaign (the guard would fail on
        # an unknown --packages name).
        campaigns = [{"name": "c", "packages": ["ghost-crate"], "delta": "d.toml"}]
        self.assertEqual(
            rgs.select(campaigns, self.pkg_dirs(), ["crates/ghost-crate/src/lib.rs"]),
            [],
        )

    def test_prefix_boundary_is_slash_not_substring(self):
        # "crates/shamir-db-fork/..." must NOT match package shamir-db.
        campaigns = [{"name": "c", "packages": ["shamir-db"], "delta": "d.toml"}]
        self.assertEqual(
            rgs.select(campaigns, self.pkg_dirs(), ["crates/shamir-db-fork/s.rs"]),
            [],
        )


class MainTest(unittest.TestCase):
    def test_matching_diff_prints_tsv_line(self):
        code, out = run_main(changed=["crates/shamir-db/src/lib.rs", "README.md"])
        self.assertEqual(code, 0)
        self.assertEqual(
            out.splitlines(),
            ["split-repo-instance\t"
             ".rust-guard/stages/split-repo-instance.toml\tshamir-db"],
        )

    def test_empty_diff_prints_nothing_exit_0(self):
        code, out = run_main(changed=[])
        self.assertEqual(code, 0)
        self.assertEqual(out, "")

    def test_no_match_prints_nothing_exit_0(self):
        code, out = run_main(changed=["docs/x.md"])
        self.assertEqual(code, 0)
        self.assertEqual(out, "")

    def test_empty_campaigns_list_prints_nothing_exit_0(self):
        code, out = run_main(
            stages=b"campaigns = []\n",
            changed=["crates/shamir-db/src/lib.rs"],
        )
        self.assertEqual(code, 0)
        self.assertEqual(out, "")

    def test_malformed_stages_toml_exit_2(self):
        # Fail closed: a broken stages file must abort (exit 2), not
        # silently select zero campaigns.
        code, out = run_main(stages=b"campaigns = [", changed=["crates/shamir-db/s.rs"])
        self.assertEqual(code, 2)
        self.assertIn("rust_guard_select.py:", out)

    def test_campaign_missing_fields_exit_2(self):
        code, _ = run_main(
            stages=b'[[campaigns]]\nname = "c"\ndelta = "d.toml"\n',
            changed=["crates/shamir-db/s.rs"],
        )
        self.assertEqual(code, 2)

    def test_missing_required_argument_exit_2(self):
        # argparse raises SystemExit(2) from inside parse_args for a
        # missing required argument; unittest does not catch SystemExit,
        # so without assertRaises the test itself would ERROR.
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf), contextlib.redirect_stderr(buf):
            with self.assertRaises(SystemExit) as cm:
                rgs.main(
                    ["--stages", "s.toml"],
                    stages_text=STAGES, metadata_text=METADATA, changed=[],
                )
        self.assertEqual(cm.exception.code, 2)

    def test_unreadable_metadata_file_exit_2(self):
        # Real OSError path (metadata_text not injected): a path under a
        # nonexistent directory can never be readable from any CWD.
        buf = io.StringIO()
        argv = [
            "--stages", "s.toml",
            "--metadata", "no-such-dir/m.json",
            "--root", ROOT,
        ]
        with contextlib.redirect_stdout(buf), contextlib.redirect_stderr(buf):
            code = rgs.main(argv, stages_text=STAGES, changed=[])
        self.assertEqual(code, 2)


if __name__ == "__main__":
    unittest.main()
