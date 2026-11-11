"""Unit tests for the fail-closed matrix-aware merge gate."""

import contextlib
import copy
import io
import json
import os
import sys
import unittest

sys.path.insert(
    0,
    os.path.join(
        os.path.dirname(
            os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))),
        "gates",
    ),
)

import merge_gate_check as mgc  # noqa: E402


def matrix(*, rows=None, flags=None, scope=None, args=None):
    """Build the complete stage-1 JSON contract, encoded as affected output."""
    rows = copy.deepcopy(rows or [])
    flags_override = flags or {}
    flags = {
        "engine": False,
        "ts": False,
        "node": False,
        "wasm32": False,
        "deny": False,
        "pipeline": False,
        "docs_only": False,
    }
    flags.update(flags_override)
    if "docs_only" not in flags_override:
        flags["docs_only"] = not rows and not any(
            flags_override.get(name, False) for name in flags if name != "docs_only"
        )
    if scope is None:
        scope = (
            "bounded"
            if rows or any(flag for name, flag in flags.items() if name != "docs_only")
            else "none"
        )
    if args is None:
        args = "--workspace" if scope == "all" else " ".join("-p " + row["name"] for row in rows)
    return {
        "version": 1,
        "base": "base-sha",
        "head": "head-sha",
        "scope": scope,
        "all_reasons": [],
        "crates": rows,
        "flags": flags,
        "args": args,
    }


def row(name="shamir-types", *, lib=False, integration=None, check_only=False):
    return {
        "name": name,
        "lib": lib,
        "integration": integration if integration == "all" else list(integration or []),
        "check_only": check_only,
        "reasons": ["changed: crates/example/src/lib.rs (+1)"],
    }


def needs(**results):
    """Build needs JSON; double underscores in names become hyphens."""
    result = {
        job.replace("__", "-"): {"result": res, "outcome": res}
        for job, res in results.items()
    }
    result["affected"] = {
        "result": "success",
        "outcome": "success",
        "outputs": {
            "matrix": json.dumps(
                matrix(
                    rows=[row(lib=True, integration="all")],
                    flags={
                        "engine": True,
                        "ts": True,
                        "node": True,
                        "wasm32": True,
                        "deny": True,
                        "pipeline": False,
                    },
                    scope="bounded",
                )
            )
        },
    }
    return result


def NEEDS(**results):
    return needs(**results)


FULL_OK = NEEDS(
    plan="success",
    affected="success",
    fast="skipped",
    fmt="success",
    clippy="success",
    wasm32__guest__check="success",
    test__build="success",
    test="success",
    integration="success",
    ts__unit="success",
    cooldown="success",
    deny="success",
    loom__model="success",
    source__integrity="success",
    rust__guard__selftest="success",
    rustdoc="success",
    engine__features="success",
    test__discovery="success",
    node__binding="success",
    matrix__self__check="success",
)

DRAFT_OK = NEEDS(
    plan="success",
    affected="success",
    fast="success",
    fmt="skipped",
    clippy="skipped",
    wasm32__guest__check="skipped",
    test__build="skipped",
    test="skipped",
    integration="skipped",
    ts__unit="skipped",
    cooldown="skipped",
    deny="skipped",
    loom__model="skipped",
    source__integrity="success",
    rust__guard__selftest="success",
    rustdoc="skipped",
    engine__features="skipped",
    test__discovery="success",
    node__binding="skipped",
    matrix__self__check="success",
)

READY = dict(event="pull_request", draft=False)
DRAFT = dict(event="pull_request", draft=True)


def set_matrix(data, value):
    data["affected"]["outputs"]["matrix"] = json.dumps(value)


def run_main(payload, argv_extra=(), *, event="pull_request"):
    out, err = io.StringIO(), io.StringIO()
    argv = ["--event", event, "--draft", "false", *argv_extra]
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        code = mgc.main(argv, needs_text=payload)
    return code, out.getvalue(), err.getvalue()


class EvaluateTests(unittest.TestCase):
    def test_all_success_non_draft_passes(self):
        self.assertEqual(mgc.evaluate(FULL_OK, **READY), [])

    def test_full_matrix_ok_has_selected_test_rows_and_feature_flags(self):
        selected, error = mgc._valid_matrix(
            json.loads(FULL_OK["affected"]["outputs"]["matrix"])
        )
        self.assertIsNone(error)
        self.assertEqual(selected["crates"], [row(lib=True, integration="all")])
        self.assertEqual(
            selected["flags"],
            {
                "engine": True,
                "ts": True,
                "node": True,
                "wasm32": True,
                "deny": True,
                "pipeline": False,
                "docs_only": False,
            },
        )

    def test_draft_keeps_exact_non_merge_ready_reason(self):
        notices = []
        reasons = mgc.evaluate(DRAFT_OK, **DRAFT, notices=notices)
        self.assertEqual(reasons, [mgc.DRAFT_MESSAGE])
        self.assertEqual(reasons, ["draft PR: full matrix not run, not merge-ready"])
        self.assertEqual(notices, [])
        self.assertEqual(
            set(mgc.FULL_TIER_ONLY_JOBS),
            {
                "fmt", "clippy", "wasm32-guest-check", "test-build", "test",
                "integration", "ts-unit", "cooldown", "deny", "loom-model",
                "rustdoc", "engine-features", "node-binding",
            },
        )
        for job in ("source-integrity", "test-discovery", "rust-guard-selftest"):
            data = copy.deepcopy(DRAFT_OK)
            data[job] = {"result": "skipped", "outcome": "skipped"}
            self.assertIn("job %s unexpectedly skipped" % job, mgc.evaluate(data, **DRAFT))

    def test_draft_fast_failure_and_full_failure_still_fail(self):
        for job, result in (("fast", "failure"), ("test", "cancelled")):
            with self.subTest(job=job):
                data = copy.deepcopy(DRAFT_OK)
                data[job] = {"result": result, "outcome": result}
                reasons = mgc.evaluate(data, **DRAFT)
                self.assertIn("job %s: %s" % (job, result), reasons)
                self.assertIn(mgc.DRAFT_MESSAGE, reasons)

    def test_non_draft_failure_and_cancellation_fail(self):
        for job, result in (("clippy", "failure"), ("integration", "cancelled")):
            with self.subTest(job=job):
                data = copy.deepcopy(FULL_OK)
                data[job] = {"result": result, "outcome": result}
                self.assertEqual(
                    mgc.evaluate(data, **READY), ["job %s: %s" % (job, result)]
                )

    def test_full_tier_fast_skip_is_allowed(self):
        notices = []
        self.assertEqual(mgc.evaluate(FULL_OK, **READY, notices=notices), [])
        self.assertEqual(notices, [])

    def test_unexpected_skip_fails_with_selected_matrix(self):
        data = copy.deepcopy(FULL_OK)
        data["rustdoc"] = {"result": "skipped", "outcome": "skipped"}
        self.assertIn("job rustdoc unexpectedly skipped", mgc.evaluate(data, **READY))
        data["cooldown"] = {"result": "skipped", "outcome": "skipped"}
        self.assertIn("job cooldown unexpectedly skipped", mgc.evaluate(data, **READY))

    def test_unknown_and_missing_jobs_fail(self):
        data = copy.deepcopy(FULL_OK)
        data["deploy-prod"] = {"result": "success", "outcome": "success"}
        self.assertIn("unknown job in needs: deploy-prod", mgc.evaluate(data, **READY))
        del data["deploy-prod"]
        del data["cooldown"]
        self.assertEqual(
            mgc.evaluate(data, **READY), ["missing required job: cooldown"]
        )

    def test_invalid_and_missing_results_fail(self):
        cases = [
            ({"result": "weird", "outcome": "weird"}, "invalid result 'weird'"),
            ({"outcome": "success"}, "invalid result None"),
            ({"result": None, "outcome": None}, "invalid result None"),
            ("success", "invalid result None"),
        ]
        for info, expected in cases:
            with self.subTest(info=info):
                data = copy.deepcopy(FULL_OK)
                data["test"] = info
                self.assertTrue(any(expected in item for item in mgc.evaluate(data, **READY)))

    def test_skipped_plan_and_affected_are_never_by_design(self):
        for scenario, source in ((READY, FULL_OK), (DRAFT, DRAFT_OK)):
            for job in ("plan", "affected"):
                with self.subTest(scenario=scenario, job=job):
                    data = copy.deepcopy(source)
                    data[job] = {"result": "skipped", "outcome": "skipped"}
                    self.assertTrue(
                        any("job %s unexpectedly skipped" % job in reason for reason in mgc.evaluate(data, **scenario))
                    )

    def test_draft_full_marker_is_judged_as_full_tier(self):
        self.assertEqual(mgc.evaluate(FULL_OK, event="pull_request", draft=False), [])

    def test_matrix_self_check_is_required_and_pr_skip_is_never_exempt(self):
        self.assertIn("matrix-self-check", mgc.REQUIRED_JOBS)
        data = copy.deepcopy(FULL_OK)
        del data["matrix-self-check"]
        self.assertEqual(mgc.evaluate(data, **READY), ["missing required job: matrix-self-check"])
        data["matrix-self-check"] = {"result": "skipped", "outcome": "skipped"}
        self.assertIn("job matrix-self-check unexpectedly skipped", mgc.evaluate(data, **READY))

    def test_matrix_self_check_non_pr_skip_is_event_only_and_noticed(self):
        data = copy.deepcopy(FULL_OK)
        data["matrix-self-check"] = {"result": "skipped", "outcome": "skipped"}
        notices = []
        self.assertEqual(
            mgc.evaluate(data, event="push", draft=False, notices=notices), []
        )
        self.assertEqual(
            notices,
            ["matrix-self-check skipped: event push is not pull_request"],
        )

        data["matrix-self-check"] = {"result": "success", "outcome": "success"}
        data["test-discovery"] = {"result": "skipped", "outcome": "skipped"}
        self.assertEqual(
            mgc.evaluate(data, **READY), ["job test-discovery unexpectedly skipped"]
        )

    def test_draft_test_discovery_still_runs_and_notices_matrix_empty(self):
        data = copy.deepcopy(DRAFT_OK)
        data["test-discovery"] = {"result": "success", "outcome": "success"}
        self.assertEqual(
            mgc.evaluate(data, **DRAFT), [mgc.DRAFT_MESSAGE]
        )

    def test_tree_equality_is_push_only_and_exactly_bounded(self):
        tree_skipped = copy.deepcopy(FULL_OK)
        tree_skipped["affected"]["outputs"]["tree_green"] = "true"
        for job in mgc.TREE_SKIP_JOBS:
            tree_skipped[job] = {"result": "skipped", "outcome": "skipped"}
        notices = []
        self.assertEqual(
            mgc.evaluate(tree_skipped, event="push", draft=False, notices=notices), []
        )
        self.assertEqual(
            set(notices),
            {
                "tree skip: job %s (affected.outputs.tree_green=true on push)" % job
                for job in mgc.TREE_SKIP_JOBS
            },
        )
        for job in (
            "clippy",
            "rustdoc",
            "engine-features",
            "deny",
            "source-integrity",
            "affected",
            "cooldown",
        ):
            outside = copy.deepcopy(FULL_OK)
            outside["affected"]["outputs"]["tree_green"] = "true"
            outside[job] = {"result": "skipped", "outcome": "skipped"}
            self.assertIn(
                "job %s unexpectedly skipped" % job,
                mgc.evaluate(outside, event="push", draft=False),
            )

        bounded_skips = copy.deepcopy(tree_skipped)
        set_matrix(bounded_skips, matrix(rows=[row(lib=True)], scope="bounded"))
        for event in ("pull_request", "workflow_dispatch", "schedule"):
            with self.subTest(event=event):
                self.assertIn(
                    "job test unexpectedly skipped",
                    mgc.evaluate(bounded_skips, event=event, draft=False),
                )
        self.assertNotIn("source-integrity", mgc.TREE_SKIP_JOBS)
        self.assertIn("test-discovery", mgc.TREE_SKIP_JOBS)
        self.assertIn("rust-guard-selftest", mgc.TREE_SKIP_JOBS)
        for job in set(mgc.REQUIRED_JOBS) - mgc.TREE_SKIP_JOBS - {
            "plan", "affected", "fast", "fmt", "cooldown", "source-integrity", "matrix-self-check"
        }:
            outside = copy.deepcopy(FULL_OK)
            outside["affected"]["outputs"]["tree_green"] = "true"
            outside[job] = {"result": "skipped", "outcome": "skipped"}
            self.assertIn(
                "job %s unexpectedly skipped" % job,
                mgc.evaluate(outside, event="push", draft=False),
            )

    def test_tree_api_fallback_and_boolean_true_do_not_authorize(self):
        data = copy.deepcopy(FULL_OK)
        set_matrix(data, matrix(rows=[row(lib=True)], scope="bounded"))
        for invalid in (None, "false", "", True, 1, {}, []):
            with self.subTest(invalid=invalid):
                data["affected"]["outputs"]["tree_green"] = invalid
                data["test"] = {"result": "skipped", "outcome": "skipped"}
                self.assertIn(
                    "job test unexpectedly skipped",
                    mgc.evaluate(data, event="push", draft=False),
                )
                data["test"] = {"result": "success", "outcome": "success"}

    def test_tree_never_excuses_cancelled_or_failed_job(self):
        data = copy.deepcopy(FULL_OK)
        set_matrix(data, matrix(rows=[row(lib=True)], scope="bounded"))
        data["affected"]["outputs"]["tree_green"] = "true"
        data["test"] = {"result": "cancelled", "outcome": "cancelled"}
        self.assertIn("job test: cancelled", mgc.evaluate(data, event="push", draft=False))

    def test_valid_matrix_empty_can_skip_only_selected_job_kinds(self):
        data = copy.deepcopy(FULL_OK)
        set_matrix(data, matrix(rows=[], scope="none", args=""))
        for job in (
            "clippy",
            "wasm32-guest-check",
            "test-build",
            "test",
            "integration",
            "ts-unit",
            "deny",
            "loom-model",
            "rustdoc",
            "engine-features",
            "node-binding",
        ):
            data[job] = {"result": "skipped", "outcome": "skipped"}
        notices = []
        self.assertEqual(mgc.evaluate(data, **READY, notices=notices), [])
        expected_notices = {
            "matrix skip: job clippy (matrix selected no crate rows)",
            "matrix skip: job wasm32-guest-check (matrix wasm32 flag is false)",
            "matrix skip: job test-build (matrix selected no lib tests)",
            "matrix skip: job test (matrix selected no lib tests or engine doctests)",
            "matrix skip: job integration (matrix selected no integration binaries)",
            "matrix skip: job ts-unit (matrix ts flag is false)",
            "matrix skip: job deny (matrix deny flag is false)",
            "matrix skip: job loom-model (matrix engine flag is false)",
            "matrix skip: job rustdoc (matrix engine flag is false)",
            "matrix skip: job engine-features (matrix engine flag is false)",
            "matrix skip: job node-binding (matrix node flag is false)",
        }
        self.assertEqual(set(notices), expected_notices)

    def test_matrix_version_keys_and_check_only_are_strict(self):
        invalid = [
            matrix() | {"unexpected": True},
            {**matrix(), "version": True},
            {**matrix(), "version": 2},
            matrix(rows=[row(lib=True, check_only=True)]),
            matrix(rows=[row(integration=["roundtrip"], check_only=True)]),
        ]
        for value in invalid:
            with self.subTest(value=value):
                validated, error = mgc._valid_matrix(value)
                self.assertIsNone(validated)
                self.assertIsNotNone(error)

    def test_valid_matrix_scope_all_with_explicit_workspace_args(self):
        value = matrix(rows=[], scope="all", args="--workspace")
        validated, error = mgc._valid_matrix(value)
        self.assertIsNotNone(validated)
        self.assertIsNone(error)

    def test_invalid_matrix_scope_all_with_mismatched_args_fails(self):
        value = matrix(rows=[], scope="all", args="-p shamir-types")
        validated, error = mgc._valid_matrix(value)
        self.assertIsNone(validated)
        self.assertEqual(error, "matrix args do not match validated crate rows")

    def test_scope_all_accepts_explicit_package_args(self):
        selected = matrix(
            rows=[row("shamir-types", lib=True, integration=["roundtrip"])],
            scope="all",
            args="-p shamir-types",
        )
        validated, error = mgc._valid_matrix(selected)
        self.assertIsNone(error)
        self.assertEqual(validated, selected)

        data = copy.deepcopy(FULL_OK)
        set_matrix(data, selected)
        data["test"] = {"result": "skipped", "outcome": "skipped"}
        self.assertIn("job test unexpectedly skipped", mgc.evaluate(data, **READY))

    def test_all_scope_skips_follow_selected_rows_and_flags(self):
        selected = matrix(
            rows=[row("shamir-types", check_only=True)],
            scope="all",
            args="-p shamir-types",
        )
        data = copy.deepcopy(FULL_OK)
        set_matrix(data, selected)
        for job in ("test-build", "test", "integration", "ts-unit", "node-binding", "deny"):
            data[job] = {"result": "skipped", "outcome": "skipped"}
        self.assertEqual(mgc.evaluate(data, **READY), [])
        data["clippy"] = {"result": "skipped", "outcome": "skipped"}
        self.assertIn("job clippy unexpectedly skipped", mgc.evaluate(data, **READY))

    def test_safety_not_matrix_skippable(self):
        for pipeline in (False, True):
            with self.subTest(pipeline=pipeline):
                data = copy.deepcopy(FULL_OK)
                selected = matrix(rows=[row(lib=True)], flags={"pipeline": pipeline})
                set_matrix(data, selected)
                data["test-discovery"] = {"result": "skipped", "outcome": "skipped"}
                self.assertIsNone(mgc._matrix_skip_reason("test-discovery", selected))
                self.assertIn(
                    "job test-discovery unexpectedly skipped", mgc.evaluate(data, **READY)
                )

    def test_clippy_can_skip_only_without_selected_crates(self):
        data = copy.deepcopy(FULL_OK)
        set_matrix(data, matrix(rows=[], scope="none", args=""))
        data["clippy"] = {"result": "skipped", "outcome": "skipped"}
        self.assertEqual(mgc.evaluate(data, **READY), [])
        data = copy.deepcopy(FULL_OK)
        set_matrix(data, matrix(rows=[row(check_only=True)]))
        data["clippy"] = {"result": "skipped", "outcome": "skipped"}
        self.assertIn("job clippy unexpectedly skipped", mgc.evaluate(data, **READY))

    def test_matrix_empty_test_must_still_run_engine_doctests(self):
        data = copy.deepcopy(FULL_OK)
        set_matrix(
            data,
            matrix(rows=[], flags={"engine": True}, scope="bounded", args=""),
        )
        data["test-build"] = {"result": "skipped", "outcome": "skipped"}
        self.assertEqual(mgc.evaluate(data, **READY), [])
        data["test"] = {"result": "skipped", "outcome": "skipped"}
        self.assertIn("job test unexpectedly skipped", mgc.evaluate(data, **READY))

    def test_lib_selection_requires_test_build_and_test(self):
        data = copy.deepcopy(FULL_OK)
        set_matrix(data, matrix(rows=[row(lib=True)]))
        data["test-build"] = {"result": "skipped", "outcome": "skipped"}
        data["test"] = {"result": "skipped", "outcome": "skipped"}
        reasons = mgc.evaluate(data, **READY)
        self.assertIn("job test-build unexpectedly skipped", reasons)
        self.assertIn("job test unexpectedly skipped", reasons)

    def test_integration_selection_requires_integration_job(self):
        data = copy.deepcopy(FULL_OK)
        set_matrix(data, matrix(rows=[row(integration=["query_roundtrip"])]))
        data["test-build"] = {"result": "skipped", "outcome": "skipped"}
        data["test"] = {"result": "skipped", "outcome": "skipped"}
        self.assertEqual(mgc.evaluate(data, **READY), [])
        data["integration"] = {"result": "skipped", "outcome": "skipped"}
        self.assertIn("job integration unexpectedly skipped", mgc.evaluate(data, **READY))

    def test_integration_all_must_run_for_runtime_target_discovery(self):
        data = copy.deepcopy(FULL_OK)
        set_matrix(data, matrix(rows=[row(integration="all")]))
        data["test-build"] = {"result": "skipped", "outcome": "skipped"}
        data["test"] = {"result": "skipped", "outcome": "skipped"}
        data["integration"] = {"result": "skipped", "outcome": "skipped"}
        self.assertIn("job integration unexpectedly skipped", mgc.evaluate(data, **READY))

    def test_check_only_row_runs_clippy_but_can_skip_test_jobs(self):
        data = copy.deepcopy(FULL_OK)
        set_matrix(data, matrix(rows=[row(check_only=True)]))
        for job in ("test-build", "test", "integration"):
            data[job] = {"result": "skipped", "outcome": "skipped"}
        self.assertEqual(mgc.evaluate(data, **READY), [])
        data = copy.deepcopy(FULL_OK)
        set_matrix(data, matrix(rows=[row(lib=True)], scope="bounded"))
        data["clippy"] = {"result": "skipped", "outcome": "skipped"}
        self.assertIn("job clippy unexpectedly skipped", mgc.evaluate(data, **READY))

    def test_per_feature_flags_and_pipeline_canary_override(self):
        cases = [
            ("wasm32-guest-check", {"wasm32": True}),
            ("rustdoc", {"engine": True}),
            ("engine-features", {"engine": True}),
            ("loom-model", {"engine": True}),
            ("ts-unit", {"ts": True}),
            ("node-binding", {"node": True}),
            ("deny", {"deny": True}),
        ]
        for job, flag in cases:
            with self.subTest(job=job):
                data = copy.deepcopy(FULL_OK)
                set_matrix(data, matrix(flags=flag))
                data[job] = {"result": "skipped", "outcome": "skipped"}
                self.assertIn("job %s unexpectedly skipped" % job, mgc.evaluate(data, **READY))

        data = copy.deepcopy(FULL_OK)
        set_matrix(data, matrix(flags={"pipeline": True}))
        jobs = (
            "clippy",
            "wasm32-guest-check",
            "test-build",
            "test",
            "integration",
            "ts-unit",
            "deny",
            "loom-model",
            "rustdoc",
            "engine-features",
            "node-binding",
        )
        for job in jobs:
            data[job] = {"result": "skipped", "outcome": "skipped"}
        reasons = mgc.evaluate(data, **READY)
        for job in jobs:
            self.assertIn("job %s unexpectedly skipped" % job, reasons)


    def test_pipeline_canary_requires_jobs_even_with_empty_rows(self):
        data = copy.deepcopy(FULL_OK)
        set_matrix(
            data,
            matrix(
                rows=[],
                scope="all",
                args="--workspace",
                flags={"pipeline": True},
            ),
        )
        for job in (
            "clippy",
            "wasm32-guest-check",
            "test-build",
            "test",
            "integration",
            "ts-unit",
            "deny",
            "loom-model",
            "rustdoc",
            "engine-features",
            "node-binding",
        ):
            data[job] = {"result": "skipped", "outcome": "skipped"}
        reasons = mgc.evaluate(data, **READY)
        for job in (
            "clippy",
            "wasm32-guest-check",
            "test-build",
            "test",
            "integration",
            "ts-unit",
            "deny",
            "loom-model",
            "rustdoc",
            "engine-features",
            "node-binding",
        ):
            self.assertIn(
                "job %s unexpectedly skipped" % job, reasons
            )

    def test_invalid_or_missing_matrix_never_authorizes_skip(self):
        invalid_matrices = [
            {"reasons": {"clippy": "not needed"}},
            matrix(flags={"engine": "false"}),
            matrix(rows=[row("bad crate name!")]),
            matrix(rows=[row(integration=["bad/name"])]),
            matrix(rows=[row()], scope="none"),
            matrix() | {"unexpected": True},
        ]
        for invalid in invalid_matrices:
            with self.subTest(invalid=invalid):
                data = copy.deepcopy(FULL_OK)
                data["affected"]["outputs"]["matrix"] = json.dumps(invalid)
                data["clippy"] = {"result": "skipped", "outcome": "skipped"}
                reasons = mgc.evaluate(data, **READY)
                self.assertTrue(any("affected outputs.matrix invalid" in r for r in reasons))
                self.assertIn("job clippy unexpectedly skipped", reasons)

    def test_missing_matrix_never_authorizes_skip(self):
        data = copy.deepcopy(FULL_OK)
        del data["affected"]["outputs"]["matrix"]
        data["rustdoc"] = {"result": "skipped", "outcome": "skipped"}
        reasons = mgc.evaluate(data, **READY)
        self.assertIn("affected outputs.matrix invalid: affected outputs.matrix is missing", reasons)
        self.assertIn("job rustdoc unexpectedly skipped", reasons)

    def test_invalid_matrix_fails_even_when_no_consumer_skips(self):
        data = copy.deepcopy(FULL_OK)
        set_matrix(data, matrix(rows=[row(lib=True)], scope="bounded"))
        data["affected"]["outputs"]["matrix"] = "{"
        self.assertTrue(
            any("affected outputs.matrix invalid" in r for r in mgc.evaluate(data, **READY))
        )

    def test_unsuccessful_affected_cannot_authorize_matrix_skip(self):
        data = copy.deepcopy(FULL_OK)
        set_matrix(data, matrix(rows=[row(lib=True)], scope="bounded"))
        data["affected"]["result"] = "failure"
        data["rustdoc"] = {"result": "skipped", "outcome": "skipped"}
        reasons = mgc.evaluate(data, **READY)
        self.assertIn("job affected: failure", reasons)
        self.assertIn("job rustdoc unexpectedly skipped", reasons)

    def test_required_jobs_inventory_stays_explicit(self):
        self.assertEqual(
            set(mgc.REQUIRED_JOBS),
            {
                "plan",
                "affected",
                "fast",
                "fmt",
                "clippy",
                "wasm32-guest-check",
                "test-build",
                "test",
                "integration",
                "ts-unit",
                "cooldown",
                "deny",
                "loom-model",
                "source-integrity",
                "rust-guard-selftest",
                "rustdoc",
                "engine-features",
                "test-discovery",
                "node-binding",
                "matrix-self-check",
            },
        )


class MainTests(unittest.TestCase):
    def test_pass_exit_0(self):
        code, out, err = run_main(json.dumps(FULL_OK))
        self.assertEqual((code, err), (0, ""))
        self.assertIn("merge gate PASSED", out)

    def test_fail_exit_1_with_reason(self):
        data = copy.deepcopy(FULL_OK)
        data["test"] = {"result": "failure", "outcome": "failure"}
        code, _, err = run_main(json.dumps(data))
        self.assertEqual(code, 1)
        self.assertIn("job test: failure", err)

    def test_matrix_skip_emits_github_notice(self):
        data = copy.deepcopy(FULL_OK)
        set_matrix(data, matrix(rows=[], scope="none", args=""))
        data["clippy"] = {"result": "skipped", "outcome": "skipped"}
        code, out, err = run_main(json.dumps(data))
        self.assertEqual((code, err), (0, ""))
        self.assertIn("::notice::matrix skip: job clippy", out)

    def test_tree_skip_notice_and_non_push_rejection(self):
        data = copy.deepcopy(FULL_OK)
        set_matrix(data, matrix(rows=[row(lib=True)], scope="bounded"))
        data["affected"]["outputs"]["tree_green"] = "true"
        data["test"] = {"result": "skipped", "outcome": "skipped"}
        code, out, err = run_main(json.dumps(data), event="push")
        self.assertEqual((code, err), (0, ""))
        self.assertIn("::notice::tree skip: job test", out)
        code, _, err = run_main(json.dumps(data), event="pull_request")
        self.assertEqual(code, 1)
        self.assertIn("job test unexpectedly skipped", err)

    def test_partial_manual_run_is_not_merge_ready(self):
        data = copy.deepcopy(FULL_OK)
        data["affected"]["outputs"]["manual_partial"] = "true"
        code, _, err = run_main(json.dumps(data), event="workflow_dispatch")
        self.assertEqual(code, 1)
        self.assertIn("explicit range or crates selection is not merge-ready", err)
        data["affected"]["outputs"]["manual_partial"] = "false"
        code, _, err = run_main(json.dumps(data), event="workflow_dispatch")
        self.assertEqual((code, err), (0, ""))

    def test_non_pr_matrix_self_check_skip_emits_notice(self):
        data = copy.deepcopy(FULL_OK)
        data["matrix-self-check"] = {"result": "skipped", "outcome": "skipped"}
        code, out, err = run_main(json.dumps(data), event="schedule")
        self.assertEqual((code, err), (0, ""))
        self.assertIn("::notice::matrix-self-check skipped: event schedule", out)

    def test_malformed_needs_json_fails_closed(self):
        code, _, err = run_main("{not json")
        self.assertEqual(code, 1)
        self.assertIn("unreadable needs JSON", err)


if __name__ == "__main__":
    unittest.main()
