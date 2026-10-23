"""unittest suite for scripts/ci/gates/native_code_check.py.

All ``cargo tree`` output is INJECTED through the ``runner`` argument (a
fake ``subprocess.run``), so the production ``query_graph`` /
``run_cargo_tree`` path is exercised end to end with no cargo, no network
and no workspace.  The canned stdout mirrors the real captured shape of
``cargo tree --locked --workspace --all-features --target all
-e normal,build,dev -i <pkg> --depth 1 --prefix none --format "{p}"``:
the queried package's own line first, then its direct dependents (at
depth 1), some with the ``(*)`` repeat marker and a registry source.
"""

import contextlib
import io
import os
import subprocess
import sys
import unittest

sys.path.insert(
    0,
    os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "gates"),
)

import native_code_check as ncc  # noqa: E402

REGISTRY = "registry+https://github.com/rust-lang/crates.io-index"


def line(name, version="1.2.3", source="", repeat=False):
    """One captured-shape ``--format "{p}"`` line."""
    out = "%s v%s" % (name, version)
    if source:
        out += " (%s)" % source
    if repeat:
        out += " (*)"
    return out


def absent(package):
    """cargo's failure for a package that is not in the graph."""
    return (
        101,
        "",
        "error: package ID specification `%s` did not match any packages\n" % package,
    )


def present(package, dependents=()):
    """cargo's success output: the package itself, then its dependents."""
    lines = [line(package, repeat=True)]
    lines += [line(dep, source=REGISTRY) for dep in dependents]
    return (0, "\n".join(lines) + "\n", "")


def allowed_state():
    """Every build tool present with exactly its allowlisted dependents."""
    canned = {tool: present(tool, sorted(ncc.ALLOWLIST[tool])) for tool in ncc.BUILD_TOOLS}
    for crate in ncc.DENY_LIST:
        canned[crate] = absent(crate)
    return canned


class FakeRunner:
    """Stands in for ``subprocess.run``: answers by the ``-i <package>`` argument."""

    def __init__(self, canned):
        self.canned = canned

    def __call__(self, cmd, **_kwargs):
        package = cmd[cmd.index("-i") + 1]
        code, out, err = self.canned[package]
        return subprocess.CompletedProcess(cmd, code, stdout=out, stderr=err)


def run_main(canned):
    """Run ncc.main against canned output; return (exit_code, stdout, stderr)."""
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        code = ncc.main([], runner=FakeRunner(canned))
    return code, out.getvalue(), err.getvalue()


class AllowedStateTests(unittest.TestCase):
    def test_full_allowlist_state_is_exit_0(self):
        code, out, _ = run_main(allowed_state())
        self.assertEqual(code, 0)
        self.assertIn("PASSED", out)

    def test_absent_tool_is_stale_entries_not_a_tooling_error(self):
        canned = allowed_state()
        canned["bindgen"] = absent("bindgen")
        code, _, err = run_main(canned)
        self.assertEqual(code, 1)
        self.assertIn("stale allowlist entry", err)
        self.assertIn("libproc", err)
        self.assertNotIn("tooling error", err)


class ViolationTests(unittest.TestCase):
    def test_new_cc_dependent_fails_and_names_the_crate(self):
        canned = allowed_state()
        canned["cc"] = present("cc", sorted(ncc.ALLOWLIST["cc"]) + ["evil-native-crate"])
        code, _, err = run_main(canned)
        self.assertEqual(code, 1)
        self.assertIn("evil-native-crate", err)
        self.assertIn("'cc'", err)

    def test_stale_allowlist_entry_fails(self):
        canned = allowed_state()
        canned["cc"] = present("cc", sorted(ncc.ALLOWLIST["cc"] - {"blake3"}))
        code, _, err = run_main(canned)
        self.assertEqual(code, 1)
        self.assertIn("stale allowlist entry", err)
        self.assertIn("blake3", err)

    def test_deny_listed_crate_without_dependents_fails(self):
        canned = allowed_state()
        canned["ring"] = present("ring")
        code, _, err = run_main(canned)
        self.assertEqual(code, 1)
        self.assertIn("'ring'", err)
        self.assertIn("deny-listed", err)

    def test_deny_listed_crate_with_dependents_fails(self):
        canned = allowed_state()
        canned["ring"] = (0, "ring v0.17.14\naws-lc-rs v1.14.0 (*)\n", "")
        code, _, err = run_main(canned)
        self.assertEqual(code, 1)
        self.assertIn("'ring'", err)


class ToolingErrorTests(unittest.TestCase):
    def test_cargo_failure_is_exit_2(self):
        canned = allowed_state()
        canned["cmake"] = (101, "", "error: the lock file needs to be updated\n")
        code, _, err = run_main(canned)
        self.assertEqual(code, 2)
        self.assertIn("tooling error", err)

    def test_unknown_stderr_is_tooling_error_not_absence(self):
        canned = allowed_state()
        canned["bindgen"] = (101, "", "error: failed to load workspace\n")
        code, _, _ = run_main(canned)
        self.assertEqual(code, 2)

    def test_package_id_specification_alone_is_not_absence(self):
        canned = allowed_state()
        canned["ring"] = (
            101,
            "",
            "error: package ID specification `ring` is ambiguous\n",
        )
        code, _, _ = run_main(canned)
        self.assertEqual(code, 2)

    def test_missing_cargo_binary_is_exit_2(self):
        def runner(_cmd, **_kwargs):
            raise FileNotFoundError("cargo")

        err = io.StringIO()
        with contextlib.redirect_stderr(err), contextlib.redirect_stdout(io.StringIO()):
            code = ncc.main([], runner=runner)
        self.assertEqual(code, 2)


class ParserUnitTests(unittest.TestCase):
    def test_parse_packages_handles_markers_and_sources(self):
        text = "\n".join(
            [
                line("cc"),
                line("aws-lc-sys", source=REGISTRY),
                line("blake3", repeat=True),
                "    Blocking waiting for file lock",
            ]
        )
        self.assertEqual(ncc.parse_packages(text), {"cc", "aws-lc-sys", "blake3"})

    def test_every_build_tool_is_allowlisted(self):
        self.assertEqual(set(ncc.ALLOWLIST), set(ncc.BUILD_TOOLS))


if __name__ == "__main__":
    unittest.main()
