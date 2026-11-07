"""unittest suite for scripts/ci/affected_crates.py.

Pure-function tests over a hand-built `cargo metadata` dict and file lists —
no cargo, no git, no filesystem.
"""

import contextlib
import io
import json
import os
import sys
import tempfile
import unittest

sys.path.insert(
    0,
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
)

import affected_crates as ac  # noqa: E402


def pkg(name, deps=(), manifest=None):
    manifest = manifest or "/repo/crates/%s/Cargo.toml" % name
    return {
        "name": name,
        "manifest_path": manifest,
        "dependencies": [{"name": d} for d in deps],
    }


METADATA = {
    "packages": [
        pkg("shamir-types"),
        pkg("shamir-collections"),
        pkg("shamir-engine", deps=["shamir-types", "shamir-collections"]),
        pkg("shamir-db", deps=["shamir-engine"]),
        pkg("shamir-cli", deps=["shamir-db"]),
        pkg("external-only", deps=[]),
    ]
}


class DirectDepTest(unittest.TestCase):
    def test_changed_leaf_marks_it_and_dependents(self):
        # A leaf change marks the leaf plus every workspace package that
        # depends on it (transitive dependent closure).
        self.assertEqual(
            ac.select_crates(["crates/shamir-collections/src/lib.rs"], METADATA),
            ["shamir-cli", "shamir-collections", "shamir-db", "shamir-engine"],
        )

    def test_output_is_sorted_p_args(self):
        # format_args sorts defensively so output order never depends on
        # the caller's input order.
        self.assertEqual(ac.format_args(["b", "a"]), "-p a -p b")


class TransitiveDependentTest(unittest.TestCase):
    def test_dependents_of_changed_crate_are_included(self):
        # shamir-db depends on shamir-engine (transitively on shamir-types);
        # shamir-cli depends on shamir-db; external-only depends on nothing.
        self.assertEqual(
            ac.select_crates(["crates/shamir-types/src/x.rs"], METADATA),
            ["shamir-cli", "shamir-db", "shamir-engine", "shamir-types"],
        )
        self.assertEqual(
            ac.select_crates(["crates/shamir-engine/src/y.rs"], METADATA),
            ["shamir-cli", "shamir-db", "shamir-engine"],
        )


class DevDependencyTest(unittest.TestCase):
    def test_dev_dependency_edge_counts(self):
        meta = {
            "packages": [
                pkg("core"),
                {
                    "name": "it",
                    "manifest_path": "/repo/crates/it/Cargo.toml",
                    "dependencies": [{"name": "core", "kind": "dev"}],
                },
            ]
        }
        self.assertEqual(
            ac.select_crates(["crates/core/src/lib.rs"], meta),
            ["core", "it"],
        )


class GlobalFileFallbackTest(unittest.TestCase):
    def test_global_paths_yield_workspace(self):
        for path in (
            "Cargo.lock",
            "Cargo.toml",
            "rust-toolchain.toml",
            "rust-toolchain",
            "deny.toml",
            ".cargo/config.toml",
            ".config/nextest.toml",
            "scripts/test.sh",
            ".github/workflows/ci.yml",
            "README.txt",  # outside crates/ and docs/, not *.md
        ):
            self.assertEqual(ac.select_crates([path], METADATA), ac.WORKSPACE, path)


class DocsOnlyTest(unittest.TestCase):
    def test_docs_and_markdown_only_diff_selects_nothing(self):
        self.assertEqual(
            ac.select_crates(
                ["docs/guide/x.md", "CLAUDE.md", "docs/img.png".replace(".png", ".md")],
                METADATA,
            ),
            [],
        )

    def test_format_args_empty_for_no_crates(self):
        self.assertEqual(ac.format_args([]), "")


class UnknownCrateFolderTest(unittest.TestCase):
    def test_unknown_crates_folder_falls_back_to_workspace(self):
        self.assertEqual(
            ac.select_crates(["crates/shamir-ghost/src/lib.rs"], METADATA),
            ac.WORKSPACE,
        )


class MarkdownUnderGlobalDirTest(unittest.TestCase):
    def test_markdown_under_global_dirs_is_still_global(self):
        # The global check takes precedence over the markdown/docs
        # exemption: a .md file under scripts/, .github/, .cargo/ or
        # .config/ cannot be assumed harmless.
        for path in (
            "scripts/example.md",
            ".github/example.md",
            ".cargo/notes.md",
            ".config/notes.md",
        ):
            self.assertEqual(ac.select_crates([path], METADATA), ac.WORKSPACE, path)


class FormatArgsTest(unittest.TestCase):
    def test_workspace_sentinel_passes_through(self):
        # The --workspace sentinel is passed through format_args verbatim.
        self.assertEqual(ac.format_args(ac.WORKSPACE), ac.WORKSPACE)
        # A global path through select_crates yields the sentinel.
        self.assertEqual(
            ac.select_crates([".github/workflows/ci.yml"], METADATA), ac.WORKSPACE
        )


class NearestManifestMappingTest(unittest.TestCase):
    def _meta(self):
        return {
            "packages": [
                pkg("shamir-x", manifest="/repo/crates/g/x/Cargo.toml"),
                pkg("a", manifest="/repo/crates/a/Cargo.toml"),
                pkg("sub", manifest="/repo/crates/a/sub/Cargo.toml"),
                pkg("hnsw_rs", manifest="/repo/vendor/hnsw_rs/Cargo.toml"),
            ]
        }

    def test_grouped_layout_leaf(self):
        meta = self._meta()
        self.assertEqual(
            ac.select_crates(["crates/g/x/src/lib.rs"], meta), ["shamir-x"]
        )
        self.assertEqual(
            ac.select_crates(["crates/g/x/src/deep/file.rs"], meta), ["shamir-x"]
        )

    def test_nested_manifest_wins_over_parent(self):
        # Nearest manifest dir wins: files under crates/a/sub/ map to "sub",
        # not to the package at crates/a (the closure is exactly {sub}).
        meta = self._meta()
        self.assertEqual(
            ac.select_crates(["crates/a/sub/src/lib.rs"], meta), ["sub"]
        )

    def test_parent_manifest_for_its_own_files(self):
        meta = self._meta()
        self.assertEqual(ac.select_crates(["crates/a/src/lib.rs"], meta), ["a"])

    def test_unmapped_folder_falls_back_to_workspace(self):
        meta = self._meta()
        self.assertEqual(
            ac.select_crates(["crates/g/notes.txt"], meta), ac.WORKSPACE
        )

    def test_vendor_layout(self):
        meta = self._meta()
        self.assertEqual(
            ac.select_crates(["vendor/hnsw_rs/src/lib.rs"], meta), ["hnsw_rs"]
        )


class ClientTsFlagTest(unittest.TestCase):
    def test_client_ts_change_selects_no_crates(self):
        self.assertEqual(
            ac.select_crates(["crates/client/shamir-client-ts/src/core/query.ts"], METADATA),
            [],
        )

    def test_client_ts_plus_real_crate(self):
        changed = [
            "crates/client/shamir-client-ts/src/core/query.ts",
            "crates/shamir-types/src/lib.rs",
        ]
        crates = ac.select_crates(changed, METADATA)
        self.assertEqual(
            crates,
            ["shamir-cli", "shamir-db", "shamir-engine", "shamir-types"],
        )
        flags = ac.compute_flags(crates, changed)
        self.assertTrue(flags["ts"])

    def test_client_ts_only_flags(self):
        changed = ["crates/client/shamir-client-ts/src/main.ts"]
        crates = ac.select_crates(changed, METADATA)
        flags = ac.compute_flags(crates, changed)
        self.assertTrue(flags["ts"])
        self.assertFalse(flags["docs_only"])
        self.assertFalse(flags["engine"])
        self.assertFalse(flags["node"])


class EventRuleTest(unittest.TestCase):
    def test_workspace_forcing_events(self):
        for event in ("push", "workflow_dispatch", "schedule"):
            self.assertTrue(ac.event_forces_workspace(event), event)
        self.assertFalse(ac.event_forces_workspace("pull_request"))

    def test_workspace_selection_overrides_flags(self):
        flags = ac.compute_flags(ac.WORKSPACE, [])
        self.assertTrue(flags["engine"])
        self.assertTrue(flags["ts"])
        self.assertTrue(flags["node"])
        self.assertFalse(flags["docs_only"])


class FlagsTest(unittest.TestCase):
    def test_marked_engine(self):
        flags = ac.compute_flags(
            ac.select_crates(["crates/shamir-engine/src/x.rs"], METADATA), []
        )
        self.assertTrue(flags["engine"])

    def test_closure_reaches_engine(self):
        crates = ac.select_crates(["crates/shamir-types/src/x.rs"], METADATA)
        self.assertIn("shamir-engine", crates)
        self.assertTrue(ac.compute_flags(crates, [])["engine"])

    def test_marked_client(self):
        meta = {"packages": list(METADATA["packages"]) + [pkg("shamir-client")]}
        self.assertTrue(
            ac.compute_flags(
                ac.select_crates(["crates/shamir-client/src/lib.rs"], meta), []
            )["node"]
        )

    def test_marked_client_node(self):
        meta = {"packages": list(METADATA["packages"]) + [pkg("shamir-client-node")]}
        self.assertTrue(
            ac.compute_flags(
                ac.select_crates(["crates/shamir-client-node/src/lib.rs"], meta), []
            )["node"]
        )

    def test_marked_types_only_no_node(self):
        self.assertFalse(
            ac.compute_flags(
                ac.select_crates(["crates/shamir-types/src/lib.rs"], METADATA), []
            )["node"]
        )

    def test_marked_query_types_sets_ts(self):
        meta = {"packages": list(METADATA["packages"]) + [pkg("shamir-query-types")]}
        self.assertTrue(
            ac.compute_flags(
                ac.select_crates(["crates/shamir-query-types/src/lib.rs"], meta), []
            )["ts"]
        )

    def test_docs_only_diff(self):
        changed = ["docs/x.md", "README.md"]
        crates = ac.select_crates(changed, METADATA)
        self.assertEqual(crates, [])
        flags = ac.compute_flags(crates, changed)
        self.assertTrue(flags["docs_only"])
        self.assertFalse(flags["engine"])
        self.assertFalse(flags["ts"])
        self.assertFalse(flags["node"])

    def test_workspace_selection(self):
        flags = ac.compute_flags(ac.WORKSPACE, [])
        self.assertTrue(flags["engine"])
        self.assertTrue(flags["ts"])
        self.assertTrue(flags["node"])
        self.assertFalse(flags["docs_only"])


class OptionalDependencyTest(unittest.TestCase):
    def test_optional_dependency_edge_counts(self):
        meta = {
            "packages": [
                pkg("core"),
                {
                    "name": "it",
                    "manifest_path": "/repo/crates/it/Cargo.toml",
                    "dependencies": [{"name": "core", "optional": True}],
                },
            ]
        }
        self.assertEqual(
            ac.select_crates(["crates/core/src/lib.rs"], meta),
            ["core", "it"],
        )


class GithubOutputFormatTest(unittest.TestCase):
    def test_exact_literal(self):
        self.assertEqual(
            ac.format_github_output(
                "-p a -p b", {"engine": True, "ts": True, "node": False, "docs_only": False}
            ),
            "args=-p a -p b\nengine=true\nts=true\nnode=false\ndocs_only=false",
        )

    def test_workspace_case(self):
        self.assertEqual(
            ac.format_github_output(
                ac.WORKSPACE, {"engine": True, "ts": True, "node": True, "docs_only": False}
            ),
            "args=--workspace\nengine=true\nts=true\nnode=true\ndocs_only=false",
        )

    def test_docs_only_case(self):
        self.assertEqual(
            ac.format_github_output(
                "", {"engine": False, "ts": False, "node": False, "docs_only": True}
            ),
            "args=\nengine=false\nts=false\nnode=false\ndocs_only=true",
        )


class MarkdownInPackageTest(unittest.TestCase):
    def test_markdown_inside_a_package_marks_it(self):
        # A docs file living inside a package still selects that package
        # and its dependents; only markdown outside any package is exempt.
        self.assertEqual(
            ac.select_crates(["crates/shamir-engine/docs/LAYERS.md"], METADATA),
            ["shamir-cli", "shamir-db", "shamir-engine"],
        )

    def test_markdown_outside_any_package_is_exempt(self):
        self.assertEqual(ac.select_crates(["docs/x.md", "README.md"], METADATA), [])


class MainEventCliTest(unittest.TestCase):
    def _run(self, argv):
        out = io.StringIO()
        with tempfile.TemporaryDirectory() as tmp:
            meta = os.path.join(tmp, "meta.json")
            with open(meta, "w", encoding="utf-8") as f:
                json.dump(METADATA, f)
            with contextlib.redirect_stdout(out):
                code = ac.main(argv + ["--metadata", meta])
        return code, out.getvalue()

    def test_push_event_selects_workspace_without_a_diff(self):
        # `--base` points at a ref that does not exist: a diff attempt would
        # fail, so success proves the event rule short-circuits it.
        code, out = self._run(
            ["--event", "push", "--base", "refs/none", "--format", "github-output"]
        )
        self.assertEqual(code, 0)
        self.assertEqual(
            out.strip(),
            "args=--workspace\nengine=true\nts=true\nnode=true\ndocs_only=false",
        )

    def test_schedule_and_dispatch_events_select_workspace(self):
        for event in ("schedule", "workflow_dispatch"):
            code, out = self._run(["--event", event, "--base", "refs/none"])
            self.assertEqual((code, out.strip()), (0, ac.WORKSPACE), event)


class ValidateArgsTest(unittest.TestCase):
    def test_valid_lines(self):
        for line in ("", ac.WORKSPACE, "-p a", "-p a -p b"):
            ac.validate_args(line)  # must not raise

    def test_invalid_lines(self):
        for line in (
            "-p a b",
            "a",
            "-p a extra",
            "--workspace -p a",
            "-p a b -p c",
        ):
            with self.assertRaises(RuntimeError):
                ac.validate_args(line)


if __name__ == "__main__":
    unittest.main()
