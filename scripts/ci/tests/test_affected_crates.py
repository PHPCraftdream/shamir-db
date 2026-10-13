"""unittest suite for scripts/ci/affected_crates.py.

Pure-function tests over a hand-built `cargo metadata` dict and file lists —
no cargo, no git, no filesystem.
"""

import os
import sys
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


if __name__ == "__main__":
    unittest.main()
