"""Selection semantics, merge rules, graph closure and global inputs."""
import fnmatch
import json
import os
import random
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(__file__), "../../matrix")))

from selection import load_edges, lockfile_changes, root_manifest_changes, select_matrix

ROOT = "/repo"


def pkg(name, deps=(), directory=None, version="1.0.0", root=ROOT):
    directory = directory or f"crates/group/{name}"
    return {"id": name, "name": name, "version": version,
            "manifest_path": f"{root}/{directory}/Cargo.toml",
            "dependencies": [dict({"name": dep}, **(opts or {})) for dep, opts in deps],
            "targets": [{"name": name.replace("-", "_"), "kind": ["lib"], "test": True,
                         "src_path": f"{root}/{directory}/src/lib.rs"},
                        {"name": "it", "kind": ["test"], "test": True,
                         "src_path": f"{root}/{directory}/tests/it.rs"}]}


def select(paths, packages=None, edges=None, root=ROOT, **kwargs):
    metadata = {"packages": packages or [pkg("demo", root=root)]}
    return select_matrix(paths, metadata, root, edge_config=edges or {"edge": [], "canary": []}, **kwargs)


def crate(result, name):
    return next(item for item in result["crates"] if item["name"] == name)


class PropagationAndMergeTest(unittest.TestCase):
    def test_prod_closure_and_terminal_direct_dev_dependents(self):
        packages = [pkg("leaf"), pkg("normal", [("leaf", {})]),
                    pkg("build", [("normal", {"kind": "build"})]),
                    pkg("dev", [("build", {"kind": "dev"})]),
                    pkg("dev-of-dev", [("dev", {"kind": "dev"})]), pkg("sibling")]
        result = select(["crates/group/leaf/src/lib.rs"], packages)
        names = {item["name"] for item in result["crates"]}
        self.assertEqual(names, {"leaf", "normal", "build", "dev"})
        self.assertNotIn("dev-of-dev", names)
        self.assertNotIn("sibling", names)
        self.assertIn("depends on leaf (normal)", crate(result, "normal")["reasons"])
        self.assertIn("depends on normal (build)", crate(result, "build")["reasons"])
        self.assertTrue(all(crate(result, name)["lib"] for name in names))

    def test_random_graph_never_underselects_brief_production_minimum(self):
        rng = random.Random(1865)
        for case in range(30):
            names, packages = [f"c{i}" for i in range(8)], []
            edges = {kind: set() for kind in ("normal", "build", "dev")}
            for dependent in names:
                deps = []
                for target in names:
                    if target != dependent and rng.random() < 0.12:
                        kind = rng.choice(tuple(edges))
                        deps.append((target, {"kind": kind}))
                        edges[kind].add((target, dependent))
                packages.append(pkg(dependent, deps))
            seed = rng.choice(names)
            closure, frontier = {seed}, [seed]
            while frontier:
                current = frontier.pop()
                for kind in ("normal", "build"):
                    for source, target in edges[kind]:
                        if source == current and target not in closure:
                            closure.add(target)
                            frontier.append(target)
            required = closure | {target for source, target in edges["dev"] if source in closure}
            result = select([f"crates/group/{seed}/src/lib.rs"], packages)
            actual = {item["name"] for item in result["crates"]}
            self.assertTrue(required <= actual, (case, seed, required - actual))
            self.assertTrue(all(crate(result, name)["lib"] for name in required))

    def test_check_only_merges_with_tests_in_both_input_orders(self):
        paths = ["crates/group/demo/benches/bench.rs", "crates/group/demo/tests/it.rs"]
        results = [select(order) for order in (paths, list(reversed(paths)))]
        self.assertEqual(results[0], results[1])
        item = crate(results[0], "demo")
        self.assertFalse(item["check_only"])
        self.assertEqual(item["integration"], ["it"])
        self.assertFalse(item["lib"])

    def test_unit_and_integration_changes_do_not_propagate(self):
        packages = [pkg("core"), pkg("dependent", [("core", {})])]
        unit = select(["crates/group/core/src/tests/helpers.rs"], packages)
        self.assertEqual({item["name"] for item in unit["crates"]}, {"core"})
        self.assertTrue(crate(unit, "core")["lib"])
        integration = select(["crates/group/core/tests/it.rs"], packages)
        self.assertEqual({item["name"] for item in integration["crates"]}, {"core"})
        self.assertEqual(crate(integration, "core")["integration"], ["it"])

    def test_every_selected_crate_has_reasons(self):
        result = select(["crates/group/demo/src/lib.rs"])
        self.assertTrue(all(item["reasons"] for item in result["crates"]))


class DeclaredEdgeAndPathRuleTest(unittest.TestCase):
    def test_seeded_hidden_edges_and_canary(self):
        edges = load_edges('''canary=["demo"]\n\n[[edge]]\npaths=["private/**"]\ncrates=["demo"]\npropagate="prod"\nreason="test edge"\n''')
        hidden = select(["private/input.dat"], [pkg("demo")], edges)
        self.assertEqual(hidden["scope"], "bounded")
        self.assertTrue(crate(hidden, "demo")["lib"])
        self.assertIn("declared edge: test edge", crate(hidden, "demo")["reasons"])
        canary = select([".github/workflows/ci.yml"], [pkg("demo")], edges)
        self.assertTrue(canary["flags"]["pipeline"])
        self.assertEqual([item["name"] for item in canary["crates"]], ["demo"])
        self.assertIn("CI pipeline canary", crate(canary, "demo")["reasons"])

    def test_pipeline_canary_has_reasons_for_every_configured_crate(self):
        config = load_edges(open(os.path.join(os.path.dirname(__file__), "../../matrix/edges.toml"), encoding="utf-8").read())
        result = select([".github/workflows/ci.yml"], [pkg(name) for name in config["canary"]], config)
        self.assertTrue(result["flags"]["pipeline"])
        self.assertEqual({item["name"] for item in result["crates"]}, set(config["canary"]))
        self.assertTrue(all("CI pipeline canary" in item["reasons"] for item in result["crates"]))

    def test_seeded_hidden_paths_match_brief_roles(self):
        config = load_edges(open(os.path.join(os.path.dirname(__file__), "../../matrix/edges.toml"), encoding="utf-8").read())
        cases = [("crates/wasm/shamir-sdk/src/lib.rs", "shamir-wasm-host", "prod"),
                 ("crates/query/shamir-query-builder/tests/fixtures/wire_ops_matrix.json", None, "tests"),
                 ("crates/query/shamir-query-types/test-vectors/vector.json", None, "tests"),
                 ("deploy/server.example.ktav", "shamir-server", "tests")]
        for path, expected, mode in cases:
            matching = [edge for edge in config["edge"] if any(fnmatch.fnmatchcase(path, glob) for glob in edge["paths"])]
            self.assertTrue(matching, path)
            self.assertTrue(any(edge.get("propagate", "tests") == mode and
                                (expected is None or expected in edge.get("crates", [])) for edge in matching), path)

        query_types = select(["crates/query/shamir-query-types/src/lib.rs"],
                             [pkg("shamir-query-types")], config)
        self.assertEqual([item["name"] for item in query_types["crates"]], ["shamir-query-types"])
        self.assertTrue(query_types["flags"]["ts"])

    def test_path_table_flags_and_fail_safe_fallback(self):
        cases = [(["Cargo.lock"], "all", {}), (["Cargo.toml"], "all", {}),
                 (["rust-toolchain.toml"], "all", {}), ([".cargo/config.toml"], "all", {}),
                 (["scripts/test.sh"], "all", {}), ([".config/nextest.toml"], "all", {}),
                 ([".github/workflows/ci.yml"], "none", {"pipeline": True}),
                 (["scripts/other.py", "docs/guide.md"], "none", {"docs_only": True}),
                 (["scripts/ci/gates/any.py"], "none", {"pipeline": True, "docs_only": False}),
                 (["scripts/ci/tests/any.py"], "none", {"pipeline": False}),
                 ([".git-blame-ignore-revs", ".claude/x.md"], "none", {"docs_only": True}),
                 (["Cargo.lock"], "all", {"deny": True}),
                 (["package-lock.json"], "none", {"deny": True, "docs_only": False}),
                 (["scripts/ci/gates/npm_licenses_check.py"], "none", {"deny": True}),
                 (["deny.toml"], "none", {"deny": True}), (["clippy.toml"], "bounded", {}),
                 (["crates/client/shamir-client-ts/src/a.ts"], "none", {"ts": True, "docs_only": False}),
                 (["mystery/file.bin"], "all", {})]
        for paths, scope, flags in cases:
            result = select(paths)
            self.assertEqual(result["scope"], scope, paths)
            for key, value in flags.items():
                self.assertEqual(result["flags"][key], value, paths)
            if paths == ["clippy.toml"]:
                self.assertTrue(crate(result, "demo")["check_only"])
            if paths == ["deny.toml"]:
                self.assertEqual(result["crates"], [])

    def test_member_manifest_change_runs_the_dependency_gate(self):
        result = select(["crates/group/demo/Cargo.toml"])
        self.assertTrue(result["flags"]["deny"])
        self.assertFalse(select(["crates/group/demo/src/lib.rs"])["flags"]["deny"])

    def test_ci_change_canary_coexists_with_normal_changed_crate(self):
        result = select([".github/workflows/ci.yml", "crates/group/demo/src/lib.rs"])
        self.assertTrue(result["flags"]["pipeline"])
        self.assertTrue(crate(result, "demo")["lib"])
        self.assertIn("changed: crates/group/demo/src/lib.rs (+1)", crate(result, "demo")["reasons"])

    def test_old_and_new_rename_paths_are_both_classified(self):
        result = select(["crates/group/demo/src/old.rs", "crates/group/other/src/lib.rs"],
                        [pkg("demo"), pkg("other")])
        self.assertEqual({item["name"] for item in result["crates"]}, {"demo", "other"})
        self.assertTrue(all(item["reasons"] for item in result["crates"]))


class LockAndManifestDiffTest(unittest.TestCase):
    @staticmethod
    def lock(leaf_version="1.0.0", middle_dependencies=("leaf",),
             leaf_dependencies=(), include_leaf=True, extra=False,
             unrelated_dependencies=()):
        rows = [
            {"name": "demo", "version": "1.0.0", "dependencies": ["middle"]},
            {"name": "middle", "version": "1.0.0", "dependencies": list(middle_dependencies)},
            {"name": "unrelated", "version": "1.0.0", "dependencies": list(unrelated_dependencies)},
        ]
        if include_leaf:
            rows.append({"name": "leaf", "version": leaf_version,
                         "dependencies": list(leaf_dependencies)})
        if extra:
            rows.append({"name": "extra", "version": "1.0.0", "dependencies": []})
        return "\n".join(
            "[[package]]\nname = %s\nversion = %s\n%s" % (
                json.dumps(row["name"]), json.dumps(row["version"]),
                ("dependencies = [%s]\n" % ", ".join(json.dumps(dep) for dep in row["dependencies"])
                 if row["dependencies"] else ""))
            for row in rows)

    def test_lock_bump_added_removed_and_member_dependency_changes(self):
        packages = {"demo": pkg("demo"), "unrelated": pkg("unrelated")}
        # middle's dependency record is unchanged by the leaf version bump;
        # traversal of the head graph must still find demo's transitive leaf.
        bump = lockfile_changes(self.lock("1.0.0"), self.lock("2.0.0"), packages)
        self.assertIn("demo", bump)
        self.assertNotIn("unrelated", bump)
        added = lockfile_changes(self.lock("1.0.0", middle_dependencies=()),
                                 self.lock("1.0.0", middle_dependencies=("extra",), extra=True), packages)
        self.assertIn("demo", added)
        self.assertNotIn("unrelated", added)
        removed = lockfile_changes(self.lock("1.0.0"),
                                   self.lock(include_leaf=False, middle_dependencies=()), packages)
        self.assertIn("demo", removed)
        self.assertNotIn("unrelated", removed)
        member_dep = lockfile_changes(self.lock("1.0.0", middle_dependencies=()),
                                      self.lock("1.0.0", middle_dependencies=("leaf", "extra"), extra=True), packages)
        self.assertIn("demo", member_dep)
        with self.assertRaises(ValueError):
            lockfile_changes("not toml", self.lock(), packages)

    def test_root_workspace_dependency_and_other_config_changes(self):
        with tempfile.TemporaryDirectory() as root:
            packages = {"demo": pkg("demo", root=root)}
            packages["demo"]["manifest_path"] = os.path.join(root, "crates/group/demo/Cargo.toml")
            member_manifest = '[dependencies]\nshared = { workspace = true }\n'
            before = '[workspace]\ndependencies.shared = "1"\n'
            after = '[workspace]\ndependencies.shared = "2"\n'
            result = root_manifest_changes(before, after, packages, root, lambda _: member_manifest)
            self.assertIn("demo", result)
            profile = root_manifest_changes(before, before + '\n[profile.dev]\nopt-level = 1\n',
                                            packages, root, lambda _: member_manifest)
            self.assertIn("--workspace", profile)

    def test_package_nextest_override_is_bounded_but_global_change_is_all(self):
        with tempfile.TemporaryDirectory() as root:
            packages = [pkg("demo", root=root)]
            base = '[[profile.default.overrides]]\nfilter = "package(demo)"\nslow-timeout = "30s"\n'
            scoped = select([".config/nextest.toml"], packages, nextest_base=base,
                            nextest_head=base.replace("30s", "31s"), root=root)
            self.assertEqual(scoped["scope"], "bounded")
            self.assertEqual([item["name"] for item in scoped["crates"]], ["demo"])
            global_change = select([".config/nextest.toml"], packages,
                                   nextest_base="[profile.default]\n",
                                   nextest_head="[profile.default]\nretries = 2\n", root=root)
            self.assertEqual(global_change["scope"], "all")


if __name__ == "__main__":
    unittest.main()
