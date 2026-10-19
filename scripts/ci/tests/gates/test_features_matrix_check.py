"""unittest suite for scripts/ci/gates/features_matrix_check.py.

Tests the cargo-metadata resolver with CAPTURED-SHAPE fixtures: a
realistic minimal ``cargo metadata`` JSON (``packages[].id/name`` and
``resolve.nodes[].id/features``) injected via the ``metadata_text``
keyword, plus an injected subprocess runner for the cargo-failure path —
no real cargo, no filesystem fixture, no network.
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

import features_matrix_check as fmc  # noqa: E402

ENGINE_ID = "shamir-engine 0.1.0-alpha.1 (path+file:///D/dev/rust/shamir-db/crates/shamir-engine)"
STORAGE_ID = "shamir-storage 0.1.0-alpha.1 (path+file:///D/dev/rust/shamir-db/crates/shamir-storage)"


def metadata_json(engine_features):
    """Build a realistic minimal cargo metadata document."""
    return json.dumps(
        {
            "packages": [
                {"id": ENGINE_ID, "name": "shamir-engine"},
                {"id": STORAGE_ID, "name": "shamir-storage"},
            ],
            "resolve": {
                "nodes": [
                    {
                        "id": STORAGE_ID,
                        "features": ["fjall"],
                    },
                    {
                        "id": ENGINE_ID,
                        "features": engine_features,
                    },
                ]
            },
        }
    )


def runner_ok(engine_features):
    """A subprocess.run stand-in returning captured-shape metadata."""

    class Proc:
        returncode = 0
        stderr = ""
        stdout = metadata_json(engine_features)

    def run(cmd, capture_output=True, text=True):
        return Proc()

    return run


def runner_fail():
    """A subprocess.run stand-in simulating a cargo failure."""

    class Proc:
        returncode = 101
        stdout = ""
        stderr = "error: failed to parse manifest\nCaused by: bad table\n"

    def run(cmd, capture_output=True, text=True):
        return Proc()

    return run


def run_main(argv=(), metadata=None, runner=None):
    """Run fmc.main with injected metadata; return (exit_code, stdout, stderr)."""
    kwargs = {}
    if metadata is not None:
        kwargs["metadata_text"] = metadata
    if runner is not None:
        kwargs["runner"] = runner
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        code = fmc.main(list(argv), **kwargs)
    return code, out.getvalue(), err.getvalue()


class ExpectedSetsTests(unittest.TestCase):
    def test_default_is_included_in_expected_sets(self):
        self.assertEqual(fmc.EXPECTED["no-default"], set())
        self.assertEqual(fmc.EXPECTED["fjall"], {"fjall"})
        self.assertEqual(fmc.EXPECTED["test-util"], {"test-util"})
        self.assertEqual(fmc.EXPECTED["fjall-test-util"], {"fjall", "test-util"})
        self.assertEqual(
            fmc.EXPECTED["default-plus"],
            {"default", "all-backends", "fjall", "test-util", "fts_indexed_long"},
        )
        self.assertEqual(
            fmc.EXPECTED["loom"], {"default", "all-backends", "fjall", "loom"}
        )

    def test_parse_flags_passthrough(self):
        self.assertEqual(
            fmc.parse_flags("--no-default-features --features fjall,test-util"),
            ["--no-default-features", "--features", "fjall,test-util"],
        )
        self.assertEqual(
            fmc.parse_flags("--features=test-util --locked"),
            ["--features", "test-util"],
        )
        with self.assertRaises(ValueError):
            fmc.parse_flags("--features")


class MainCellTests(unittest.TestCase):
    """Every cell id matches the captured cargo-resolved feature set."""

    CASES = [
        ("no-default", "--no-default-features", []),
        ("fjall", "--no-default-features --features fjall", ["fjall"]),
        ("test-util", "--no-default-features --features test-util", ["test-util"]),
        (
            "fjall-test-util",
            "--no-default-features --features fjall,test-util",
            ["fjall", "test-util"],
        ),
        (
            "default-plus",
            "--features test-util,fts_indexed_long",
            ["default", "all-backends", "fjall", "test-util", "fts_indexed_long"],
        ),
        (
            "loom",
            "--features loom",
            ["default", "all-backends", "fjall", "loom"],
        ),
    ]

    def test_every_cell_matches(self):
        for cell, flags, features in self.CASES:
            with self.subTest(cell=cell):
                code, out, _ = run_main(
                    ["--expected", cell, "--flags=" + flags],
                    metadata=metadata_json(features),
                    runner=runner_ok(features),
                )
                self.assertEqual(code, 0, out)
                self.assertIn("OK", out)

    def test_mismatch_exits_1(self):
        code, _, err = run_main(
            ["--expected", "no-default", "--flags=--features fjall"],
            metadata=metadata_json(["fjall"]),
        )
        self.assertEqual(code, 1)
        self.assertIn("mismatch", err)
        self.assertIn("'fjall'", err)


class MainErrorTests(unittest.TestCase):
    def test_unknown_cell_exits_2(self):
        code, _, err = run_main(["--expected", "bogus"], metadata=metadata_json([]))
        self.assertEqual(code, 2)
        self.assertIn("unknown cell id", err)

    def test_missing_resolve_exits_2(self):
        code, _, err = run_main(
            ["--expected", "fjall"],
            metadata=json.dumps({"packages": [{"id": ENGINE_ID, "name": "shamir-engine"}]}),
        )
        self.assertEqual(code, 2)
        self.assertIn("resolve", err)

    def test_package_absent_exits_2(self):
        code, _, err = run_main(
            ["--expected", "fjall"],
            metadata=json.dumps(
                {
                    "packages": [{"id": STORAGE_ID, "name": "shamir-storage"}],
                    "resolve": {"nodes": [{"id": STORAGE_ID, "features": []}]},
                }
            ),
        )
        self.assertEqual(code, 2)
        self.assertIn("shamir-engine", err)

    def test_resolve_node_missing_engine_exits_2(self):
        code, _, err = run_main(
            ["--expected", "fjall"],
            metadata=json.dumps(
                {
                    "packages": [{"id": ENGINE_ID, "name": "shamir-engine"}],
                    "resolve": {"nodes": [{"id": STORAGE_ID, "features": []}]},
                }
            ),
        )
        self.assertEqual(code, 2)
        self.assertIn("resolve graph", err)

    def test_malformed_json_exits_2(self):
        code, _, err = run_main(["--expected", "fjall"], metadata="{not json")
        self.assertEqual(code, 2)
        self.assertIn("malformed", err)

    def test_injected_cargo_failure_exits_2(self):
        code, _, err = run_main(
            ["--expected", "fjall"], runner=runner_fail()
        )
        self.assertEqual(code, 2)
        self.assertIn("cargo metadata failed", err)
        self.assertIn("bad table", err)

    def test_node_without_features_list_exits_2(self):
        code, _, err = run_main(
            ["--expected", "fjall"],
            metadata=json.dumps(
                {
                    "packages": [{"id": ENGINE_ID, "name": "shamir-engine"}],
                    "resolve": {"nodes": [{"id": ENGINE_ID}]},
                }
            ),
        )
        self.assertEqual(code, 2)
        self.assertIn("features list", err)


if __name__ == "__main__":
    unittest.main()
