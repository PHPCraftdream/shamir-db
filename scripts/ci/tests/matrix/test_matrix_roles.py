"""Briefed file-role classification for production and test targets."""
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(__file__), "../../matrix")))

from selection import role, select_matrix

ROOT = "/repo"


def package(name="demo", directory="crates/g/demo", targets=None, root=ROOT):
    return {"name": name, "manifest_path": f"{root}/{directory}/Cargo.toml",
            "version": "1.0.0", "dependencies": [], "targets": targets or [
                {"name": name.replace("-", "_"), "kind": ["lib"], "test": True,
                 "src_path": f"{root}/{directory}/src/lib.rs"},
                {"name": "one", "kind": ["test"], "test": True,
                 "src_path": f"{root}/{directory}/tests/one.rs"},
                {"name": "two", "kind": ["test"], "test": True,
                 "src_path": f"{root}/{directory}/tests/two/main.rs"}]}


class FileRoleTest(unittest.TestCase):
    def test_production_and_unit_roles_are_distinct(self):
        pkg = package()
        for path in ("crates/g/demo/src/lib.rs", "crates/g/demo/Cargo.toml", "crates/g/demo/build.rs"):
            self.assertEqual(role(pkg, path, ROOT)[0], "production", path)
        self.assertEqual(role(pkg, "crates/g/demo/src/tests/helper.rs", ROOT)[0], "unit")
        self.assertEqual(role(pkg, "crates/g/demo/src/feature_tests.rs", ROOT)[0], "unit")
        self.assertEqual(role(pkg, "crates/g/demo/src/tests.rs", ROOT)[0], "production")

    def test_integration_target_and_shared_support_roles(self):
        pkg = package()
        self.assertEqual(role(pkg, "crates/g/demo/tests/one.rs", ROOT), ("integration", "one"))
        self.assertEqual(role(pkg, "crates/g/demo/tests/two/helper.rs", ROOT), ("integration", "two"))
        self.assertEqual(role(pkg, "crates/g/demo/tests/shared/fixture.json", ROOT)[0], "integration_all")
        result = select_matrix(["crates/g/demo/tests/one.rs"], {"packages": [pkg]}, ROOT)
        self.assertEqual(result["crates"][0]["integration"], ["one"])
        support = select_matrix(["crates/g/demo/tests/shared/fixture.json"], {"packages": [pkg]}, ROOT)
        self.assertEqual(support["crates"][0]["integration"], "all")

    def test_benchmarks_and_examples_are_check_only(self):
        for path in ("crates/g/demo/benches/throughput.rs", "crates/g/demo/examples/basic.rs"):
            result = select_matrix([path], {"packages": [package()]}, ROOT)
            item = result["crates"][0]
            self.assertTrue(item["check_only"], path)
            self.assertFalse(item["lib"], path)
            self.assertEqual(item["integration"], [], path)

    def test_package_markdown_include_is_production(self):
        with tempfile.TemporaryDirectory() as root:
            directory = os.path.join(root, "crates", "g", "demo")
            os.makedirs(os.path.join(directory, "src"))
            pkg = package(root=root)
            pkg["manifest_path"] = os.path.join(directory, "Cargo.toml")
            source_path = "crates/g/demo/src/lib.rs"
            source = os.path.join(directory, "src", "lib.rs")
            with open(source, "w", encoding="utf-8") as handle:
                handle.write('const DOC: &str = include_str!("../guide.md");')
            doc = "crates/g/demo/guide.md"
            reader = lambda _: 'include_str!("../guide.md")'
            self.assertEqual(role(pkg, doc, root, reader, [source_path])[0], "production")
            self.assertEqual(role(pkg, doc, root, lambda _: "", [source_path])[0], "none")
            self.assertEqual(role(pkg, doc, root)[0], "none")
            result = select_matrix([doc], {"packages": [pkg]}, root,
                                   content_provider=reader, source_paths=[source_path])
            self.assertEqual([item["name"] for item in result["crates"]], ["demo"])

    def test_test_vectors_select_local_test_targets(self):
        pkg = package()
        pkg["targets"] = pkg["targets"][:1]
        result = select_matrix(["crates/g/demo/test-vectors/protocol.json"], {"packages": [pkg]}, ROOT)
        item = result["crates"][0]
        self.assertTrue(item["lib"])
        self.assertEqual(item["integration"], [])
        self.assertFalse(item["check_only"])


if __name__ == "__main__":
    unittest.main()
