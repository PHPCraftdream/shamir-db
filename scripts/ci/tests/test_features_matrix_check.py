"""unittest suite for scripts/ci/features_matrix_check.py.

Pure-function tests over ``parse_flags`` / ``resolve_features`` plus
``main(argv)`` runs with the manifest TOML injected as a keyword argument
(the same injection style test_rust_guard_select.py uses) — no
filesystem fixture, no cargo, no network.
"""

import contextlib
import io
import os
import sys
import unittest

sys.path.insert(
    0,
    os.path.join(
        os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
        "gates",
    ),
)

import features_matrix_check as fmc  # noqa: E402

MANIFEST = b"""
[package]
name = "shamir-engine"
version = "0.1.0-alpha.1"

[features]
default = ["all-backends"]
all-backends = ["fjall"]
fts_indexed_long = []
fjall = ["shamir-storage/fjall"]
test-util = []
loom = []
"""


def run_main(argv=(), manifest=MANIFEST):
    """Run fmc.main with injected manifest; return (exit_code, stdout, stderr)."""
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        code = fmc.main(list(argv), manifest_text=manifest)
    return code, out.getvalue(), err.getvalue()


class ParseFlagsTests(unittest.TestCase):
    def test_no_default_and_features(self):
        no_default, req = fmc.parse_flags(
            "--no-default-features --features fjall,test-util"
        )
        self.assertTrue(no_default)
        self.assertEqual(req, ["fjall", "test-util"])

    def test_features_equals_form(self):
        no_default, req = fmc.parse_flags("--features=test-util")
        self.assertFalse(no_default)
        self.assertEqual(req, ["test-util"])

    def test_unknown_tokens_ignored(self):
        no_default, req = fmc.parse_flags("--locked --all-targets --features loom")
        self.assertFalse(no_default)
        self.assertEqual(req, ["loom"])

    def test_features_without_value_is_error(self):
        with self.assertRaises(ValueError):
            fmc.parse_flags("--features")


class ResolveFeaturesTests(unittest.TestCase):
    def load(self, text=MANIFEST):
        import tomllib

        return tomllib.loads(text.decode())

    def test_no_default(self):
        self.assertEqual(fmc.resolve_features(self.load(), True, []), set())

    def test_default_expands_all_backends_to_fjall(self):
        self.assertEqual(
            fmc.resolve_features(self.load(), False, []),
            {"all-backends", "fjall"},
        )

    def test_requested_union(self):
        self.assertEqual(
            fmc.resolve_features(self.load(), True, ["fjall", "test-util"]),
            {"fjall", "test-util"},
        )

    def test_no_default_wins_over_default(self):
        self.assertEqual(
            fmc.resolve_features(self.load(), True, ["loom"]),
            {"loom"},
        )

    def test_dep_qualified_refs_excluded(self):
        # fjall = ["shamir-storage/fjall"] must not leak the dep ref in.
        self.assertEqual(
            fmc.resolve_features(self.load(), True, ["fjall"]), {"fjall"}
        )


class MainCellTests(unittest.TestCase):
    """Every cell id resolves to its documented expected set."""

    CASES = [
        ("no-default", "--no-default-features", set()),
        ("fjall", "--no-default-features --features fjall", {"fjall"}),
        ("test-util", "--no-default-features --features test-util", {"test-util"}),
        (
            "fjall-test-util",
            "--no-default-features --features fjall,test-util",
            {"fjall", "test-util"},
        ),
        (
            "default-plus",
            "--features test-util,fts_indexed_long",
            {"all-backends", "fjall", "test-util", "fts_indexed_long"},
        ),
        ("loom", "--features loom", {"all-backends", "fjall", "loom"}),
    ]

    def test_every_cell_passes(self):
        for cell, flags, expected in self.CASES:
            with self.subTest(cell=cell):
                code, out, _ = run_main(
                    ["--expected", cell, "--flags=" + flags, "--manifest-path", "m.toml"]
                )
                self.assertEqual(code, 0, out)
                self.assertEqual(fmc.EXPECTED[cell], expected)

    def test_mismatch_fails_with_exit_1(self):
        code, _, err = run_main(
            [
                "--expected",
                "no-default",
                "--flags=--features fjall",
                "--manifest-path",
                "m.toml",
            ]
        )
        self.assertEqual(code, 1)
        self.assertIn("mismatch", err)
        self.assertIn("'fjall'", err)


class MainErrorTests(unittest.TestCase):
    def test_unknown_cell_id_exits_2(self):
        code, _, err = run_main(
            ["--expected", "bogus", "--manifest-path", "m.toml"]
        )
        self.assertEqual(code, 2)
        self.assertIn("unknown cell id", err)

    def test_malformed_manifest_exits_2(self):
        code, _, err = run_main(
            ["--expected", "fjall", "--manifest-path", "m.toml"],
            manifest=b"[features\n",
        )
        self.assertEqual(code, 2)
        self.assertIn("features_matrix_check.py:", err)

    def test_manifest_without_features_table_exits_2(self):
        code, _, err = run_main(
            ["--expected", "fjall", "--manifest-path", "m.toml"],
            manifest=b'[package]\nname = "shamir-engine"\n',
        )
        self.assertEqual(code, 2)
        self.assertIn("[features]", err)

    def test_wrong_package_exits_2(self):
        code, _, err = run_main(
            ["--expected", "fjall", "--manifest-path", "m.toml"],
            manifest=b'[package]\nname = "other"\n\n[features]\nfjall = []\n',
        )
        self.assertEqual(code, 2)
        self.assertIn("shamir-engine", err)


if __name__ == "__main__":
    unittest.main()
