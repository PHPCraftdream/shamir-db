"""unittest suite for scripts/ci/rust_guard_select.py.

Pure-function tests over `select` / `strip_root` / `load_campaigns`, plus
`main(argv)` runs with the stages TOML, the cargo metadata JSON and the
changed-file list injected as keyword arguments — the only filesystem
fixture is one throwaway temp directory that plays the repo root; no
git, no cargo, no stdin.
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

import rust_guard_select as rgs  # noqa: E402

STAGES = b"""
[[campaigns]]
name = "split-repo-instance"
packages = ["shamir-db"]
delta = ".rust-guard/stages/split-repo-instance.toml"
discovery_delta = ".rust-guard/stages/split-repo-instance-discovery.toml"

[[campaigns]]
name = "extract-engine"
packages = ["shamir-engine", "ghost-crate"]
delta = ".rust-guard/stages/extract-engine.toml"
discovery_delta = ".rust-guard/stages/extract-engine-discovery.toml"
"""

# Absolute manifest paths as a `cargo metadata` run emits them: rooted
# under a real temp directory, RESOLVED on the running machine.  A
# hardcoded "/home/runner/work/repo" went through Path.resolve() in
# resolve_root, which rewrites foreign paths to the local drive (e.g.
# "C:/home/runner/work/repo" on Windows), so the absolute-metadata
# fixtures never matched and the TSV assertions failed off Linux.
# tempfile.mkdtemp() always exists, so resolve() is stable here.
ROOT = rgs.norm(str(Path(tempfile.mkdtemp()).resolve()))


def _metadata(root: str) -> bytes:
    """Cargo-metadata JSON with absolute manifest paths under `root`."""
    return ("""{
  "packages": [
    {"name": "shamir-db",
     "manifest_path": "%s/crates/shamir-db/Cargo.toml"},
    {"name": "shamir-engine",
     "manifest_path": "%s/crates/shamir-engine/Cargo.toml"}
  ]
}
""" % (root, root)).encode()


METADATA = _metadata(ROOT)


def run_main(stages=STAGES, metadata=METADATA, root=ROOT, changed=(), extra=()):
    """Run rgs.main with injected content; return (exit_code, stdout, stderr).

    Both streams are captured: ``main`` prints errors to STDERR, so a test
    asserting on the error message must look at the third element (the
    round-2 bug: only stdout was captured and the malformed-TOML assertion
    compared against an empty string).
    """
    argv = [
        "--stages", "s.toml",
        "--metadata", "m.json",
        "--root", root,
    ] + list(extra)
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        code = rgs.main(
            argv, stages_text=stages, metadata_text=metadata,
            changed=list(changed),
        )
    return code, out.getvalue(), err.getvalue()


class StripRootTest(unittest.TestCase):
    def test_strips_root_prefix(self):
        self.assertEqual(
            rgs.strip_root(ROOT + "/crates/x/Cargo.toml", ROOT),
            "crates/x/Cargo.toml",
        )

    def test_resolve_root_makes_relative_root_absolute(self):
        # THE round-2 bug: the workflow calls the script with `--root .`
        # while metadata manifest paths are absolute.  With the literal
        # ".", PurePosixPath(".") was never a prefix of an absolute path,
        # strip_root stripped nothing and every campaign was skipped.
        # resolve_root turns "." into a comparable absolute posix root.
        cwd = os.getcwd()
        tmp = tempfile.mkdtemp()
        try:
            os.chdir(tmp)
            resolved = rgs.resolve_root(".")
        finally:
            os.chdir(cwd)
        os.rmdir(tmp)
        self.assertEqual(resolved, rgs.norm(str(Path(tmp).resolve())))
        # And stripping against the resolved root yields the repo-relative
        # path the diff side uses; with the literal "." this kept the full
        # absolute path and matched nothing (counterfactual).
        self.assertEqual(
            rgs.strip_root(resolved + "/crates/x/Cargo.toml", resolved),
            "crates/x/Cargo.toml",
        )
        self.assertNotEqual(
            rgs.strip_root(resolved + "/crates/x/Cargo.toml", "."),
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
            [(n, d, p, dirs) for n, d, p, dirs in fixed],
            [("split-repo-instance",
              ".rust-guard/stages/split-repo-instance.toml",
              ["shamir-db"], ["crates/shamir-db"])],
        )

    def test_backslash_separators_in_metadata_capture(self):
        # A Windows-captured metadata with backslash manifest dirs must
        # still match the forward-slash diff paths; without separator
        # normalisation the pkg dir would be "crates\\shamir-db" and the
        # startswith check would fail.
        campaigns = [{"name": "c", "packages": ["shamir-db"], "delta": "d.toml"}]
        dirs = {"shamir-db": "crates\\shamir-db"}
        self.assertEqual(
            [(n, d, p, dirs) for n, d, p, dirs in
             rgs.select(campaigns, {k: rgs.norm(v) for k, v in dirs.items()},
                        ["crates/shamir-db/src/lib.rs"])],
            [("c", "d.toml", ["shamir-db"], ["crates/shamir-db"])],
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
            [(n, p, dirs) for n, _, p, dirs in out],
            [("split-repo-instance", ["shamir-db"], ["crates/shamir-db"]),
             ("extract-engine", ["shamir-engine"], ["crates/shamir-engine"])],
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
        code, out, _err = run_main(changed=["crates/shamir-db/src/lib.rs", "README.md", ".rust-guard/stages/split-repo-instance.toml"])
        self.assertEqual(code, 0)
        self.assertEqual(
            out.splitlines(),
            ["split-repo-instance\t"
             ".rust-guard/stages/split-repo-instance.toml\tshamir-db\tcrates/shamir-db\t"
             ".rust-guard/stages/split-repo-instance-discovery.toml"],
        )

    def test_main_from_temp_cwd_with_root_dot_and_real_stdin(self):
        cwd = os.getcwd()
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "stages.toml").write_bytes(STAGES)
            (root / "metadata.json").write_bytes(_metadata(rgs.norm(str(root.resolve()))))
            try:
                os.chdir(tmp)
                old_stdin = sys.stdin
                sys.stdin = io.StringIO("crates/shamir-db/src/lib.rs\n.rust-guard/stages/split-repo-instance.toml\n")
                out = io.StringIO()
                try:
                    with contextlib.redirect_stdout(out):
                        code = rgs.main(["--stages", "stages.toml", "--metadata", "metadata.json", "--root", "."])
                finally:
                    sys.stdin = old_stdin
            finally:
                os.chdir(cwd)
        self.assertEqual(code, 0)
        self.assertEqual(out.getvalue().splitlines(), [
            "split-repo-instance\t.rust-guard/stages/split-repo-instance.toml\tshamir-db\tcrates/shamir-db\t"
            ".rust-guard/stages/split-repo-instance-discovery.toml"
        ])

    def test_empty_diff_prints_nothing_exit_0(self):
        code, out, _err = run_main(changed=[])
        self.assertEqual(code, 0)
        self.assertEqual(out, "")

    def test_no_match_prints_nothing_exit_0(self):
        code, out, _err = run_main(changed=["docs/x.md"])
        self.assertEqual(code, 0)
        self.assertEqual(out, "")

    def test_empty_campaigns_list_prints_nothing_exit_0(self):
        code, out, _err = run_main(
            stages=b"campaigns = []\n",
            changed=["crates/shamir-db/src/lib.rs"],
        )
        self.assertEqual(code, 0)
        self.assertEqual(out, "")

    def test_malformed_stages_toml_exit_2(self):
        # Fail closed: a broken stages file must abort (exit 2), not
        # silently select zero campaigns.  The error goes to STDERR;
        # round-2 bug: the assertion ran against stdout only, which is
        # empty here, so assertIn FAILED (the test errored, it did not
        # pass vacuously).
        code, out, err = run_main(
            stages=b"campaigns = [", changed=["crates/shamir-db/s.rs"]
        )
        self.assertEqual(code, 2)
        self.assertEqual(out, "")
        self.assertIn("rust_guard_select.py:", err)

    def test_campaign_missing_fields_exit_2(self):
        code, _out, _err = run_main(
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

    def test_package_without_delta_is_inert(self):
        code, out, _ = run_main(changed=["crates/shamir-db/src/lib.rs"])
        self.assertEqual((code, out), (0, ""))

    def test_discovery_delta_selects_with_normalised_separators(self):
        code, out, _ = run_main(changed=[
            "crates/shamir-db/src/lib.rs",
            r".rust-guard\stages\split-repo-instance-discovery.toml",
        ])
        self.assertEqual(code, 0)
        self.assertEqual(out.splitlines(), [
            "split-repo-instance\t.rust-guard/stages/split-repo-instance.toml\t"
            "shamir-db\tcrates/shamir-db\t.rust-guard/stages/split-repo-instance-discovery.toml"
        ])

    def test_missing_discovery_delta_exit_2(self):
        stages = b'[[campaigns]]\nname = "c"\npackages = ["shamir-db"]\ndelta = "d"\n'
        code, _, _ = run_main(stages=stages)
        self.assertEqual(code, 2)

    def test_non_string_discovery_delta_exit_2(self):
        stages = b'[[campaigns]]\nname = "c"\npackages = ["shamir-db"]\ndelta = "d"\ndiscovery_delta = 1\n'
        code, _, _ = run_main(stages=stages)
        self.assertEqual(code, 2)

    def present_in(self, metadata, packages):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = rgs.main(["--present-in", "m.json", "--packages", packages],
                            metadata_text=metadata)
        return code, out.getvalue(), err.getvalue()

    def test_present_in_subset_preserves_order(self):
        code, out, _ = self.present_in(METADATA, "ghost,shamir-engine,shamir-db")
        self.assertEqual((code, out), (0, "shamir-engine,shamir-db\n"))

    def test_present_in_none_exit_2(self):
        code, out, _ = self.present_in(METADATA, "ghost")
        self.assertEqual((code, out), (2, ""))

    def test_present_in_bad_json_exit_2(self):
        code, out, _ = self.present_in("{", "shamir-db")
        self.assertEqual((code, out), (2, ""))

    def test_present_in_unreadable_exit_2(self):
        with contextlib.redirect_stderr(io.StringIO()):
            code = rgs.main(["--present-in", "no-such-dir/m.json", "--packages", "shamir-db"])
        self.assertEqual(code, 2)

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
