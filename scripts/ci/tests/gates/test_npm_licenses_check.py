"""unittest suite for scripts/ci/gates/npm_licenses_check.py.

The gate reads committed ``package-lock.json`` files only, so every test builds a
tiny lockfile in a temp dir; discovery of tracked files is exercised against a
throwaway ``git init`` repo.  No npm, no network.
"""

import contextlib
import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(
    0,
    os.path.join(
        os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))),
        "gates",
    ),
)

import npm_licenses_check as nlc  # noqa: E402


def lockfile(packages, version=3):
    """A lockfile dict with the root entry plus ``packages`` (key -> info)."""
    return {
        "name": "fixture",
        "lockfileVersion": version,
        "packages": {"": {"name": "fixture"}, **packages},
    }


def write(dirpath, name, content):
    path = Path(dirpath) / name
    path.parent.mkdir(parents=True, exist_ok=True)
    text = content if isinstance(content, str) else json.dumps(content)
    path.write_text(text, encoding="utf-8")
    return path


def run_main(argv):
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        code = nlc.main(argv)
    return code, out.getvalue(), err.getvalue()


class SatisfiedTests(unittest.TestCase):
    def setUp(self):
        self.allowed = nlc._allowed_set()

    def ok(self, expression):
        return nlc.satisfied(expression, self.allowed)

    def test_single_allowed_and_unlisted(self):
        self.assertTrue(self.ok("MIT"))
        self.assertTrue(self.ok("Apache-2.0"))
        self.assertFalse(self.ok("GPL-3.0-only"))
        self.assertFalse(self.ok("MPL-2.0"))

    def test_case_is_ignored(self):
        self.assertTrue(self.ok("mit"))
        self.assertTrue(self.ok("apache-2.0"))

    def test_or_needs_one_operand(self):
        self.assertTrue(self.ok("GPL-3.0-only OR MIT"))
        self.assertTrue(self.ok("MIT OR GPL-3.0-only"))
        self.assertFalse(self.ok("GPL-3.0-only OR AGPL-3.0-only"))

    def test_and_needs_both_operands(self):
        self.assertTrue(self.ok("MIT AND ISC"))
        self.assertFalse(self.ok("MIT AND GPL-3.0-only"))
        self.assertFalse(self.ok("GPL-3.0-only AND MIT"))

    def test_precedence_and_parentheses(self):
        # AND binds tighter than OR.
        self.assertTrue(self.ok("GPL-3.0-only AND MIT OR ISC"))
        self.assertFalse(self.ok("GPL-3.0-only AND (MIT OR ISC)"))
        self.assertTrue(self.ok("(MIT OR GPL-3.0-only) AND (ISC OR AGPL-3.0-only)"))
        self.assertFalse(self.ok("(MIT OR GPL-3.0-only) AND (AGPL-3.0-only OR LGPL-3.0-only)"))

    def test_with_exception(self):
        self.assertTrue(self.ok("Apache-2.0 WITH LLVM-exception"))
        # An exception only adds permissions: the base license being allowed is enough.
        self.assertTrue(self.ok("Apache-2.0 WITH Classpath-exception-2.0"))
        self.assertFalse(self.ok("GPL-2.0-only WITH Classpath-exception-2.0"))

    def test_malformed_expressions_raise(self):
        for bad in ("", "   ", "MIT OR", "OR MIT", "(MIT", "MIT)", "MIT AND AND ISC",
                    "MIT*", "SEE LICENSE IN LICENSE.md", "MIT WITH", "MIT ISC"):
            with self.subTest(expression=bad):
                with self.assertRaises(nlc.ExpressionError):
                    nlc.satisfied(bad, self.allowed)


class CheckLockfileTests(unittest.TestCase):
    def check(self, packages, **kw):
        with tempfile.TemporaryDirectory() as tmp:
            path = write(tmp, "package-lock.json", lockfile(packages, **kw))
            return nlc.check_lockfile(path)

    def test_all_allowed_passes_and_counts_scopes(self):
        runtime, dev, violations = self.check(
            {
                "node_modules/a": {"version": "1.0.0", "license": "MIT"},
                "node_modules/@scope/b": {"version": "2.0.0", "license": "ISC"},
                "node_modules/c": {"version": "3.0.0", "license": "Apache-2.0", "dev": True},
            }
        )
        self.assertEqual((runtime, dev, violations), (2, 1, []))

    def test_unlisted_license_is_a_violation_naming_the_package(self):
        _, _, violations = self.check(
            {"node_modules/gpl": {"version": "1.2.3", "license": "GPL-3.0-only"}}
        )
        self.assertEqual(len(violations), 1)
        self.assertIn("gpl@1.2.3 [runtime]", violations[0])
        self.assertIn("'GPL-3.0-only' is not allowed", violations[0])

    def test_dev_packages_are_checked_too(self):
        _, _, violations = self.check(
            {"node_modules/d": {"version": "1.0.0", "license": "GPL-3.0-only", "dev": True}}
        )
        self.assertIn("[dev]", violations[0])

    def test_missing_license_fails_closed(self):
        _, _, violations = self.check({"node_modules/x": {"version": "1.0.0"}})
        self.assertIn("no license field", violations[0])

    def test_non_string_and_empty_license_fail_closed(self):
        _, _, violations = self.check(
            {
                "node_modules/legacy": {"version": "1.0.0", "license": {"type": "MIT"}},
                "node_modules/empty": {"version": "1.0.0", "license": " "},
            }
        )
        self.assertEqual(len(violations), 2)
        self.assertTrue(all("not a non-empty string" in v for v in violations))

    def test_unparsable_and_unlicensed_fail_closed(self):
        _, _, violations = self.check(
            {
                "node_modules/u": {"version": "1.0.0", "license": "UNLICENSED"},
                "node_modules/s": {"version": "1.0.0", "license": "SEE LICENSE IN LICENSE"},
            }
        )
        self.assertEqual(len(violations), 2)

    def test_root_entry_and_links_are_skipped(self):
        runtime, dev, violations = self.check(
            {"node_modules/w": {"resolved": "packages/w", "link": True}}
        )
        self.assertEqual((runtime, dev, violations), (0, 0, []))

    def test_nested_node_modules_use_the_innermost_name(self):
        _, _, violations = self.check(
            {"node_modules/a/node_modules/@s/b": {"version": "1.0.0", "license": "GPL-3.0-only"}}
        )
        self.assertIn("@s/b@1.0.0", violations[0])

    def test_per_package_exception_is_scoped_to_that_package(self):
        packages = {
            "node_modules/special": {"version": "1.0.0", "license": "MPL-2.0"},
            "node_modules/other": {"version": "1.0.0", "license": "MPL-2.0"},
        }
        with mock.patch.dict(nlc.EXCEPTIONS, {"special": ("MPL-2.0",)}):
            _, _, violations = self.check(packages)
        self.assertEqual(len(violations), 1)
        self.assertIn("other@1.0.0", violations[0])

    def test_unreadable_lockfile_and_missing_packages_map(self):
        with tempfile.TemporaryDirectory() as tmp:
            bad = write(tmp, "bad/package-lock.json", "{not json")
            self.assertIn("unreadable lockfile", nlc.check_lockfile(bad)[2][0])
            v1 = write(tmp, "v1/package-lock.json", {"lockfileVersion": 1, "dependencies": {}})
            self.assertIn("no `packages` map", nlc.check_lockfile(v1)[2][0])


class MainTests(unittest.TestCase):
    def test_explicit_lockfile_pass_and_fail(self):
        with tempfile.TemporaryDirectory() as tmp:
            good = write(tmp, "good/package-lock.json",
                         lockfile({"node_modules/a": {"version": "1.0.0", "license": "MIT"}}))
            bad = write(tmp, "bad/package-lock.json",
                        lockfile({"node_modules/b": {"version": "1.0.0", "license": "GPL-3.0-only"}}))
            code, out, _ = run_main([str(good)])
            self.assertEqual(code, 0)
            self.assertIn("PASSED: 1 packages (1 runtime, 0 dev) in 1 lockfile(s)", out)
            code, _, err = run_main([str(good), str(bad)])
            self.assertEqual(code, 1)
            self.assertIn("npm license gate FAILED", err)
            self.assertIn("b@1.0.0", err)

    def test_discovery_uses_tracked_lockfiles_and_notes_unlocked_manifests(self):
        with tempfile.TemporaryDirectory() as tmp:
            subprocess.run(["git", "init", "-q", tmp], check=True)
            write(tmp, "pkg/package.json", {"name": "pkg"})
            write(tmp, "pkg/package-lock.json",
                  lockfile({"node_modules/a": {"version": "1.0.0", "license": "MIT"}}))
            write(tmp, "unlocked/package.json", {"name": "unlocked"})
            # A vendored copy under node_modules must never be picked up.
            write(tmp, "pkg/node_modules/z/package-lock.json",
                  lockfile({"node_modules/q": {"version": "1.0.0", "license": "GPL-3.0-only"}}))
            subprocess.run(["git", "-C", tmp, "add", "-A", "-f"], check=True)
            code, out, err = run_main(["--root", tmp])
            self.assertEqual(code, 0, err)
            self.assertIn("PASSED: 1 packages", out)
            self.assertIn("unlocked", out)
            self.assertIn("has no package-lock.json", out)

    def test_no_lockfile_at_all_fails_closed(self):
        with tempfile.TemporaryDirectory() as tmp:
            subprocess.run(["git", "init", "-q", tmp], check=True)
            code, _, err = run_main(["--root", tmp])
            self.assertEqual(code, 1)
            self.assertIn("no package-lock.json to check", err)


if __name__ == "__main__":
    unittest.main()
