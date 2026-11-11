import json
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "matrix"))
import filterset


NAMES = ("shamir-server", "shamir-e2e", "shamir-lib", "shamir-check")


def metadata():
    return {
        "packages": [
            {
                "name": name,
                "id": name,
                "targets": [
                    {"name": name + "_unit", "kind": ["lib"]},
                    {"name": name + "_tests", "kind": ["test"]},
                    {"name": name + "_other", "kind": ["test"]},
                ],
            }
            for name in NAMES
        ]
    }


def matrix(rows, flags=None):
    names = sorted(row["name"] for row in rows)
    return {
        "version": 1,
        "base": "base",
        "head": "head",
        "scope": "bounded",
        "all_reasons": [],
        "crates": rows,
        "flags": flags or {name: False for name in filterset.FLAG_NAMES},
        "args": " ".join("-p " + name for name in names),
    }


def row(name, lib=False, integration=None, check_only=False):
    return {
        "name": name,
        "lib": lib,
        "integration": [] if integration is None else integration,
        "check_only": check_only,
        "reasons": ["fixture"],
    }


class ConsumeTests(unittest.TestCase):
    def test_lib_integration_and_check_only_are_separated(self):
        source = matrix([
            row("shamir-lib", lib=True, integration=["shamir-lib_tests"]),
            row("shamir-check", check_only=True),
        ])
        result = filterset.consume(source, metadata())
        self.assertEqual(result["lib_args"], "-p shamir-lib")
        self.assertEqual(result["integration_args"], "-p shamir-lib")
        self.assertEqual(result["check_args"], "-p shamir-check -p shamir-lib")
        self.assertEqual(result["integration_filter"],
                         "(kind(test) and package(=shamir-lib) and binary(=shamir-lib_tests))")
        self.assertTrue(result["run_lib"])
        self.assertTrue(result["run_integration"])
        self.assertTrue(result["run_check"])

    def test_all_selects_every_test_target_and_exact_names(self):
        result = filterset.consume(matrix([row("shamir-server", lib=True, integration="all")]), metadata())
        self.assertEqual(result["integration_filter"], " or ".join((
            "(kind(test) and package(=shamir-server) and binary(=shamir-server_other))",
            "(kind(test) and package(=shamir-server) and binary(=shamir-server_tests))",
        )))
        self.assertNotIn("shamir-e2e", result["integration_filter"])

    def test_check_only_has_no_test_jobs_even_with_all_targets(self):
        result = filterset.consume(matrix([row("shamir-check", check_only=True)]), metadata())
        self.assertEqual(result["lib_args"], "")
        self.assertEqual(result["integration_args"], "")
        self.assertEqual(result["integration_filter"], "")
        self.assertEqual(result["check_args"], "-p shamir-check")
        self.assertFalse(result["run_lib"])
        self.assertFalse(result["run_integration"])
        self.assertTrue(result["run_check"])
        self.assertFalse(result["run_engine"])

    def test_all_accepts_zero_target_and_unordered_package_args(self):
        data = metadata()
        data["packages"][3]["targets"] = []
        source = matrix([row("shamir-check", integration="all"), row("shamir-lib", lib=True, integration=[])])
        source["args"] = "-p shamir-lib -p shamir-check"
        result = filterset.consume(source, data)
        self.assertEqual(result["args"], "-p shamir-check -p shamir-lib")
        self.assertEqual(result["integration_args"], "")
        self.assertEqual(result["integration_filter"], "")

    def test_rejects_unvalidated_matrix_and_names(self):
        bad = matrix([row("shamir-lib;echo", lib=True)])
        with self.assertRaisesRegex(ValueError, "invalid crate"):
            filterset.consume(bad, metadata())
        bad = matrix([row("shamir-lib", integration=["shamir-lib_tests sibling"])])
        with self.assertRaisesRegex(ValueError, "invalid integration target"):
            filterset.consume(bad, metadata())
        bad = matrix([row("shamir-lib", integration=["shamir-e2e_tests"])])
        with self.assertRaisesRegex(ValueError, "unknown integration target"):
            filterset.consume(bad, metadata())
        bad = matrix([row("shamir-lib", lib=True)])
        bad["args"] = "--workspace"
        with self.assertRaisesRegex(ValueError, "args"):
            filterset.consume(bad, metadata())

    def test_version_must_be_exact_integer(self):
        for version in (True, 1.0, "1"):
            with self.subTest(version=version):
                bad = matrix([row("shamir-lib", lib=True)])
                bad["version"] = version
                with self.assertRaisesRegex(ValueError, "version"):
                    filterset.consume(bad, metadata())

    def test_flags_require_all_seven_boolean_contract_fields(self):
        complete = {name: False for name in filterset.FLAG_NAMES}
        for omitted in filterset.FLAG_NAMES:
            with self.subTest(omitted=omitted):
                flags = dict(complete)
                del flags[omitted]
                bad = matrix([row("shamir-lib", lib=True)], flags=flags)
                with self.assertRaisesRegex(ValueError, "flags have missing or unknown keys"):
                    filterset.consume(bad, metadata())

    def test_flags_reject_unknown_keys(self):
        flags = {name: False for name in filterset.FLAG_NAMES}
        flags["future_flag"] = False
        bad = matrix([row("shamir-lib", lib=True)], flags=flags)
        with self.assertRaisesRegex(ValueError, "flags have missing or unknown keys"):
            filterset.consume(bad, metadata())

    def test_workspace_members_restrict_validation_set(self):
        data = metadata()
        data["packages"].append({"id": "external", "name": "external-pkg", "targets": []})
        data["workspace_members"] = [name for name in NAMES]
        with self.assertRaisesRegex(ValueError, "unknown workspace crate"):
            filterset.consume(matrix([row("external-pkg", lib=True)]), data)

    def test_github_output_is_canonical_and_complete(self):
        source = matrix([row("shamir-lib", lib=True)])
        result = {"matrix_data": source, **filterset.consume(source, metadata())}
        output = filterset.github_output(result)
        self.assertIn("matrix={", output)
        for key in (*filterset.FLAG_NAMES, "lib_args", "integration_args", "integration_filter",
                    "check_args", "run_lib", "run_integration", "run_check"):
            self.assertIn(key + "=", output)
        self.assertEqual(json.loads(result["matrix"])["crates"][0]["name"], "shamir-lib")


class PlanTests(unittest.TestCase):
    def stage1(self, *args, **kwargs):
        self.argv = args[0]
        data = matrix([row("shamir-lib", lib=True, integration=["shamir-lib_tests"])])
        stdout = "matrix=" + json.dumps(data, separators=(",", ":")) + "\nargs=" + data["args"]
        stdout += "\n" + "\n".join(name + "=false" for name in filterset.FLAG_NAMES)
        return SimpleNamespace(returncode=0, stdout=stdout, stderr="")

    def test_pr_uses_merge_base_contract_and_github_output(self):
        result = filterset.plan("pull_request", {}, metadata(), runner=self.stage1)
        self.assertIn("--event", self.argv)
        self.assertIn("pull_request", self.argv)
        self.assertIn("--base", self.argv)
        self.assertIn("origin/master", self.argv)
        self.assertIn("--head", self.argv)
        self.assertIn("HEAD", self.argv)
        self.assertEqual(self.argv[-2:], ["--format", "github-output"])
        self.assertIn("-p shamir-lib", result["lib_args"])

    def test_push_uses_exact_event_range(self):
        before, after = "a" * 40, "b" * 40
        filterset.plan("push", {"before": before, "after": after}, metadata(), runner=self.stage1)
        pairs = [self.argv[i:i + 2] for i in range(len(self.argv) - 1)]
        self.assertIn(["--event", "push"], pairs)
        self.assertIn(["--before", before], pairs)
        self.assertIn(["--after", after], pairs)

    def test_dispatch_computed_inputs_and_default_range(self):
        event = {"inputs": {"range": "", "scope": "computed", "crates": ""}}
        filterset.plan("workflow_dispatch", event, metadata(), runner=self.stage1)
        pairs = [self.argv[i:i + 2] for i in range(len(self.argv) - 1)]
        self.assertIn(["--range", "HEAD^..HEAD"], pairs)
        event["inputs"]["range"] = "v1..HEAD"
        filterset.plan("workflow_dispatch", event, metadata(), runner=self.stage1)
        pairs = [self.argv[i:i + 2] for i in range(len(self.argv) - 1)]
        self.assertIn(["--range", "v1..HEAD"], pairs)

    def test_dispatch_crates_and_workspace_are_metadata_fixtures(self):
        never_run = lambda *args, **kwargs: self.fail("manual override invoked stage1")
        result = filterset.plan("workflow_dispatch", {"inputs": {"crates": "shamir-lib shamir-check"}},
                                metadata(), runner=never_run)
        self.assertEqual(result["args"], "-p shamir-check -p shamir-lib")
        package_names = [package["name"] for package in metadata()["packages"]]
        workspace_metadata = {
            "packages": [dict(package, id=package["name"]) for package in metadata()["packages"]],
            "workspace_members": package_names,
        }
        all_result = filterset.plan("workflow_dispatch", {"inputs": {"scope": "workspace"}},
                                    workspace_metadata, runner=never_run)
        self.assertEqual(all_result["args"], " ".join("-p " + name for name in sorted(NAMES)))
        for key in ("run_pipeline", "run_engine", "run_ts", "run_node", "run_wasm32", "run_deny"):
            self.assertTrue(all_result[key])

    def test_dispatch_rejects_untrusted_and_unknown_crates(self):
        for name in ("shamir-lib;echo", "not-a-package"):
            with self.subTest(name=name), self.assertRaises(ValueError):
                filterset.plan("workflow_dispatch", {"inputs": {"crates": name}}, metadata())

    def test_bin_only_manual_package_has_no_lib_tests(self):
        data = {"packages": [{"id": "bin-only-id", "name": "bin-only", "targets": [
            {"name": "binary_test", "kind": ["test"]}, {"name": "app", "kind": ["bin"]},
        ]}]}
        selected = filterset.plan("workflow_dispatch", {"inputs": {"crates": "bin-only"}}, data,
                                  runner=lambda *args, **kwargs: self.fail("manual crates invoked stage1"))
        self.assertEqual(selected["lib_args"], "")
        self.assertEqual(selected["integration_args"], "-p bin-only")
        self.assertEqual(selected["integration_filter"],
                         "(kind(test) and package(=bin-only) and binary(=binary_test))")

    def test_schedule_is_only_top_crates_lib_and_integration(self):
        result = filterset.plan("schedule", {}, metadata())
        self.assertEqual(result["args"], "-p shamir-e2e -p shamir-server")
        source = json.loads(result["matrix"])
        self.assertTrue(all(row["lib"] and row["integration"] == "all" for row in source["crates"]))

    def test_malformed_stage1_output_is_rejected(self):
        def runner(*args, **kwargs):
            return SimpleNamespace(returncode=0, stdout="args=-p x\n", stderr="")
        with self.assertRaisesRegex(ValueError, "matrix contract"):
            filterset.plan("pull_request", {}, metadata(), runner=runner)

    def test_push_requires_both_event_shas(self):
        with self.assertRaisesRegex(ValueError, "before and after"):
            filterset.plan("push", {"before": "a" * 40}, metadata(), runner=self.stage1)


if __name__ == "__main__":
    unittest.main()
