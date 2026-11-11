"""Self-check coverage and CLI failure atomicity."""
import contextlib
import io
import json
import os
import sys
import tempfile
import unittest
from unittest import mock

MATRIX = os.path.abspath(os.path.join(os.path.dirname(__file__), "../../matrix"))
sys.path.insert(0, MATRIX)

import checks
import test_matrix as cli
from selection import select_matrix


ROOT = "/repo"


def package(name, directory, root=ROOT):
    return {"id": name, "name": name, "version": "1.0.0",
            "manifest_path": os.path.join(root, directory, "Cargo.toml"),
            "dependencies": [],
            "targets": [{"name": name, "kind": ["lib"], "test": True,
                         "src_path": os.path.join(root, directory, "src/lib.rs")}]}


class UnitPathGuardTest(unittest.TestCase):
    def test_guard_accepts_cfg_test_and_rejects_ungated_module(self):
        with tempfile.TemporaryDirectory() as root:
            source = os.path.join(root, "crates/a/src/lib.rs")
            child = os.path.join(root, "crates/a/src/tests/mod.rs")
            os.makedirs(os.path.dirname(child))
            with open(child, "w", encoding="utf-8") as handle:
                handle.write("// test module\n")
            metadata = {"packages": [package("a", "crates/a", root)]}
            source_paths = ["crates/a/src/lib.rs", "crates/a/src/tests/mod.rs"]
            contents = {
                "crates/a/src/lib.rs": "#[cfg(test)]\nmod tests;\n",
                "crates/a/src/tests/mod.rs": "// test module\n",
            }
            content_provider = lambda path: contents.get(path)
            with open(source, "w", encoding="utf-8") as handle:
                handle.write(contents["crates/a/src/lib.rs"])
            findings, production = checks.unit_path_audit(
                root, metadata, source_paths, content_provider, {"edge": []})
            self.assertEqual(findings, [])
            self.assertNotIn("crates/a/src/tests/mod.rs", production)

            contents["crates/a/src/lib.rs"] = "mod tests;\n"
            with open(source, "w", encoding="utf-8") as handle:
                handle.write(contents["crates/a/src/lib.rs"])
            findings, production = checks.unit_path_audit(
                root, metadata, source_paths, content_provider, {"edge": []})
            self.assertTrue(any("cfg(test)" in item and "crates/a/src/lib.rs" in item
                                for item in findings))
            self.assertIn("crates/a/src/tests/mod.rs", production)

            downstream = package("downstream", "crates/downstream", root)
            downstream["dependencies"] = [{"name": "a", "kind": None}]
            metadata["packages"].append(downstream)
            selected = select_matrix(["crates/a/src/tests/mod.rs"], metadata, root,
                                     content_provider, {"edge": []},
                                     source_paths=source_paths,
                                     production_overrides=production)
            self.assertIn("downstream", {item["name"] for item in selected["crates"]})

    def test_guard_accepts_test_gated_path_attribute_and_flags_negative_fallback(self):
        with tempfile.TemporaryDirectory() as root:
            source = os.path.join(root, "crates/a/src/lib.rs")
            child = os.path.join(root, "crates/a/src/tests/helper.rs")
            os.makedirs(os.path.dirname(child))
            with open(child, "w", encoding="utf-8") as handle:
                handle.write("// test helper\n")
            metadata = {"packages": [package("a", "crates/a", root)]}
            source_paths = ["crates/a/src/lib.rs", "crates/a/src/tests/helper.rs"]
            contents = {
                "crates/a/src/lib.rs": '#[cfg(test)]\n#[path = "tests/helper.rs"]\nmod helper;\n',
                "crates/a/src/tests/helper.rs": "// test helper\n",
            }
            content_provider = lambda path: contents.get(path)
            with open(source, "w", encoding="utf-8") as handle:
                handle.write(contents["crates/a/src/lib.rs"])
            findings, production = checks.unit_path_audit(
                root, metadata, source_paths, content_provider, {"edge": []})
            self.assertEqual(findings, [])
            self.assertNotIn("crates/a/src/tests/helper.rs", production)

            contents["crates/a/src/lib.rs"] = '#[path = "tests/helper.rs"]\nmod helper;\n'
            with open(source, "w", encoding="utf-8") as handle:
                handle.write(contents["crates/a/src/lib.rs"])
            findings, production = checks.unit_path_audit(
                root, metadata, source_paths, content_provider, {"edge": []})
            self.assertTrue(any("cfg(test)" in item and "crates/a/src/lib.rs" in item
                                for item in findings))
            self.assertIn("crates/a/src/tests/helper.rs", production)


class ExternalEdgeCheckTest(unittest.TestCase):
    def test_actual_changed_hidden_target_requires_declared_reader_edge(self):
        with tempfile.TemporaryDirectory() as root:
            reader_dir = os.path.join(root, "crates/a")
            target_dir = os.path.join(root, "crates/b/src")
            os.makedirs(os.path.join(reader_dir, "src"))
            os.makedirs(target_dir)
            source = os.path.join(reader_dir, "src/lib.rs")
            target = os.path.join(target_dir, "data.json")
            with open(source, "w", encoding="utf-8") as handle:
                handle.write('const DATA: &str = include_str!("../../b/src/data.json");\n')
            with open(target, "w", encoding="utf-8") as handle:
                handle.write("{}\n")
            metadata = {"packages": [package("reader", "crates/a", root), package("data", "crates/b", root)]}
            edge = ('[[edge]]\npaths=["crates/b/src/data.json"]\ncrates=["reader"]\n'
                    'propagate="prod"\nreason="reader consumes hidden data file"\n')
            source_paths = ["crates/a/src/lib.rs"]
            with mock.patch.object(checks, "_top_entries", return_value=["crates"]), \
                    mock.patch.object(checks, "_tracked_rust", return_value=source_paths), \
                    mock.patch.object(checks, "_ts_external_refs", return_value=[]):
                missing = checks.run_checks(root, metadata, "", source_paths=source_paths)
                accepted = checks.run_checks(root, metadata, edge, source_paths=source_paths)
            self.assertTrue(any("reader reads external target crates/b/src/data.json without Cargo edge or target-matched reader edge" in item for item in missing))
            self.assertFalse(any("reader reads external target crates/b/src/data.json without Cargo edge or target-matched reader edge" in item for item in accepted))
            selected = select_matrix(["crates/b/src/data.json"], metadata, root,
                                     edge_config=checks.load_edges(edge))
            self.assertIn("reader", {item["name"] for item in selected["crates"]})

    def test_check_reports_findings_to_stderr_without_partial_stdout(self):
        with tempfile.TemporaryDirectory() as root:
            metadata = os.path.join(root, "metadata.json")
            with open(metadata, "w", encoding="utf-8") as handle:
                json.dump({"packages": []}, handle)
            stdout, stderr = io.StringIO(), io.StringIO()
            with mock.patch.object(cli, "load_metadata", return_value={"packages": []}), \
                    mock.patch.object(cli, "run_checks", return_value=["bad edge at crates/a/src/lib.rs:1"]), \
                    contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                code = cli.main(["--check", "--metadata", metadata])
            self.assertEqual(code, 1)
            self.assertEqual(stdout.getvalue(), "")
            self.assertIn("bad edge", stderr.getvalue())


class CliValidationAndFormatTest(unittest.TestCase):
    def test_invalid_args_leave_stdout_empty(self):
        with tempfile.TemporaryDirectory() as root:
            metadata = os.path.join(root, "metadata.json")
            changed = os.path.join(root, "changed.txt")
            with open(metadata, "w", encoding="utf-8") as handle:
                json.dump({"packages": []}, handle)
            with open(changed, "w", encoding="utf-8") as handle:
                handle.write("docs/readme.md\n")
            stdout, stderr = io.StringIO(), io.StringIO()
            with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                code = cli.main(["--metadata", metadata, "--changed", changed,
                                 "--head", "A", "--format", "explain"])
            self.assertEqual(code, 1)
            self.assertEqual(stdout.getvalue(), "")
            self.assertTrue(stderr.getvalue())

    def test_github_output_rejects_malformed_args_without_partial_stdout(self):
        with tempfile.TemporaryDirectory() as root:
            metadata = os.path.join(root, "metadata.json")
            changed = os.path.join(root, "changed.txt")
            with open(metadata, "w", encoding="utf-8") as handle:
                json.dump({"packages": []}, handle)
            with open(changed, "w", encoding="utf-8") as handle:
                handle.write("docs/readme.md\n")
            result = {"version": 1, "base": None, "head": None, "scope": "bounded",
                      "all_reasons": [], "crates": [], "flags": {}, "args": "-p demo;echo bad"}
            stdout, stderr = io.StringIO(), io.StringIO()
            with mock.patch.object(cli, "select_matrix", return_value=result), \
                    contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                code = cli.main(["--metadata", metadata, "--changed", changed,
                                 "--format", "github-output"])
            self.assertEqual(code, 1)
            self.assertEqual(stdout.getvalue(), "")
            self.assertIn("malformed args", stderr.getvalue())

    def test_explain_and_json_formats_include_same_selection(self):
        result = select_matrix(["crates/a/src/lib.rs"], {"packages": [package("a", "crates/a")]}, ROOT)
        self.assertIn("a | lib", cli._explain(result))
        rendered = cli._github_output(result)
        self.assertTrue(rendered.startswith("matrix={"))
        self.assertIn("args=-p a", rendered)

    def test_dispatch_and_schedule_shadow_real_head_range(self):
        with tempfile.TemporaryDirectory() as root:
            metadata_path = os.path.join(root, "metadata.json")
            with open(metadata_path, "w", encoding="utf-8") as handle:
                json.dump({"packages": [package("a", "crates/a", root)]}, handle)
            for event in ("workflow_dispatch", "schedule"):
                with self.subTest(event=event), mock.patch.object(cli, "ROOT", root), \
                        mock.patch.object(cli, "_changed_paths", return_value=["crates/a/src/lib.rs"]) as diff, \
                        mock.patch.object(cli, "_tracked_paths", return_value=[]), \
                        mock.patch.object(cli, "unit_path_audit", return_value=([], set())), \
                        mock.patch.object(cli, "_snapshot", return_value=None), \
                        contextlib.redirect_stdout(io.StringIO()) as stdout:
                    code = cli.main(["--event", event, "--range", "HEAD^..HEAD",
                                     "--metadata", metadata_path, "--format", "json"])
                    self.assertEqual(code, 0)
                    result = json.loads(stdout.getvalue())
                    self.assertEqual(result["scope"], "bounded")
                    self.assertEqual({item["name"] for item in result["crates"]}, {"a"})
                    diff.assert_called_once_with("HEAD^..HEAD", root)


if __name__ == "__main__":
    unittest.main()
