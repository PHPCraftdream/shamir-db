"""unittest suite for scripts/ci/gates/merge_gate_check.py.

Pure ``evaluate`` / ``main`` tests with the needs JSON injected as a
keyword argument (same injection style as test_rust_guard_select.py) —
no subprocess, no filesystem fixture, no network.  Covers every gate
transition: all-success non-draft, draft shapes, failure/cancelled,
by-design vs unexpected skips, unknown and missing jobs.
"""

import contextlib
import io
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

import merge_gate_check as mgc  # noqa: E402


def needs(**results):
    """Build a needs object from ``job_result=<result>`` kwargs.

    Hyphenated job names cannot be kwargs, so pass them via the helper
    below / plain dicts (``needs(fast="success")`` or ``NEEDS(...)``).
    """
    return {job.replace("_", "-"): {"result": res, "outcome": res} for job, res in results.items()}


def NEEDS(**results):
    """Alias keeping hyphens: double underscores become hyphens."""
    return {job.replace("__", "-"): {"result": res, "outcome": res} for job, res in results.items()}


FULL_OK = NEEDS(
    fast="skipped",
    fmt="success",
    clippy="success",
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
)

DRAFT_OK = NEEDS(
    fast="success",
    fmt="skipped",
    clippy="skipped",
    test="skipped",
    integration="skipped",
    ts__unit="skipped",
    cooldown="skipped",
    deny="skipped",
    loom__model="skipped",
    source__integrity="skipped",
    rust__guard__selftest="skipped",
    rustdoc="skipped",
    engine__features="skipped",
    test__discovery="skipped",
)

READY = dict(event="pull_request", draft=False)
DRAFT = dict(event="pull_request", draft=True)


def run_main(payload, argv_extra=()):
    """Run mgc.main with injected stdin; return (exit_code, stdout, stderr)."""
    out, err = io.StringIO(), io.StringIO()
    argv = ["--event", "pull_request", "--draft", "false", *argv_extra]
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        code = mgc.main(argv, needs_text=payload)
    return code, out.getvalue(), err.getvalue()


class EvaluateTests(unittest.TestCase):
    def test_all_success_non_draft_passes(self):
        data = dict(FULL_OK)
        data["fast"] = {"result": "skipped", "outcome": "skipped"}
        self.assertEqual(mgc.evaluate(data, **READY), [])

    def test_draft_all_skipped_but_fast_fails_with_exact_message(self):
        reasons = mgc.evaluate(DRAFT_OK, **DRAFT)
        self.assertEqual(reasons, [mgc.DRAFT_MESSAGE])
        self.assertEqual(
            reasons, ["draft PR: full matrix not run, not merge-ready"]
        )

    def test_draft_with_fast_failure_lists_it_and_still_fails(self):
        data = dict(DRAFT_OK)
        data["fast"] = {"result": "failure", "outcome": "failure"}
        reasons = mgc.evaluate(data, **DRAFT)
        self.assertIn("job fast: failure", reasons)
        self.assertIn(mgc.DRAFT_MESSAGE, reasons)

    def test_draft_with_full_tier_failure(self):
        data = dict(DRAFT_OK)
        data["test"] = {"result": "cancelled", "outcome": "cancelled"}
        reasons = mgc.evaluate(data, **DRAFT)
        self.assertIn("job test: cancelled", reasons)
        self.assertIn(mgc.DRAFT_MESSAGE, reasons)

    def test_non_draft_one_failure(self):
        data = dict(FULL_OK)
        data["clippy"] = {"result": "failure", "outcome": "failure"}
        reasons = mgc.evaluate(data, **READY)
        self.assertEqual(reasons, ["job clippy: failure"])

    def test_non_draft_one_cancelled(self):
        data = dict(FULL_OK)
        data["integration"] = {"result": "cancelled", "outcome": "cancelled"}
        reasons = mgc.evaluate(data, **READY)
        self.assertEqual(reasons, ["job integration: cancelled"])

    def test_non_draft_fast_skipped_by_design_passes(self):
        # FULL_OK already has fast skipped — the canonical passing shape.
        self.assertEqual(mgc.evaluate(FULL_OK, **READY), [])

    def test_non_draft_unexpectedly_skipped_required_job_fails(self):
        data = dict(FULL_OK)
        data["rustdoc"] = {"result": "skipped", "outcome": "skipped"}
        reasons = mgc.evaluate(data, **READY)
        self.assertEqual(reasons, ["job rustdoc unexpectedly skipped"])

    def test_non_draft_fast_success_also_passes(self):
        data = dict(FULL_OK)
        data["fast"] = {"result": "success", "outcome": "success"}
        self.assertEqual(mgc.evaluate(data, **READY), [])

    def test_unknown_job_name_fails(self):
        data = dict(FULL_OK)
        data["deploy-prod"] = {"result": "success", "outcome": "success"}
        reasons = mgc.evaluate(data, **READY)
        self.assertEqual(reasons, ["unknown job in needs: deploy-prod"])

    def test_missing_required_job_fails(self):
        data = {k: v for k, v in FULL_OK.items() if k != "cooldown"}
        reasons = mgc.evaluate(data, **READY)
        self.assertEqual(reasons, ["missing required job: cooldown"])

    def test_draft_skipped_fast_is_not_by_design(self):
        data = dict(DRAFT_OK)
        data["fast"] = {"result": "skipped", "outcome": "skipped"}
        reasons = mgc.evaluate(data, **DRAFT)
        self.assertIn("job fast unexpectedly skipped", reasons)
        self.assertIn(mgc.DRAFT_MESSAGE, reasons)

    def test_invalid_result_value_fails(self):
        data = dict(FULL_OK)
        data["rustdoc"] = {"result": "weird", "outcome": "weird"}
        reasons = mgc.evaluate(data, **READY)
        self.assertIn(
            "job rustdoc: invalid result 'weird' (must be one of "
            "cancelled, failure, skipped, success)",
            reasons,
        )

    def test_missing_result_key_fails(self):
        data = dict(FULL_OK)
        data["test"] = {"outcome": "success"}
        reasons = mgc.evaluate(data, **READY)
        self.assertIn(
            "job test: invalid result None (must be one of "
            "cancelled, failure, skipped, success)",
            reasons,
        )

    def test_null_result_fails(self):
        data = dict(FULL_OK)
        data["clippy"] = {"result": None, "outcome": None}
        reasons = mgc.evaluate(data, **READY)
        self.assertIn("job clippy: invalid result None", reasons[0])

    def test_non_dict_job_info_fails(self):
        data = dict(FULL_OK)
        data["fmt"] = "success"
        reasons = mgc.evaluate(data, **READY)
        self.assertTrue(r.startswith("job fmt: invalid result") for r in reasons)

    def test_required_jobs_list_excludes_fast_tier_only_job(self):
        self.assertNotIn("engine-features-fast", mgc.REQUIRED_JOBS)
        self.assertEqual(len(mgc.REQUIRED_JOBS), 14)
        self.assertIn("deny", mgc.REQUIRED_JOBS)


class MainTests(unittest.TestCase):
    def test_pass_exit_0(self):
        import json

        code, out, err = run_main(json.dumps(FULL_OK))
        self.assertEqual((code, err), (0, ""))
        self.assertIn("merge gate PASSED", out)

    def test_fail_exit_1_with_reason(self):
        import json

        data = dict(FULL_OK)
        data["test"] = {"result": "failure", "outcome": "failure"}
        code, out, err = run_main(json.dumps(data))
        self.assertEqual(code, 1)
        self.assertIn("job test: failure", err)

    def test_draft_main_prints_exact_message_and_exits_1(self):
        import json

        code, out, err = run_main(
            json.dumps(DRAFT_OK), argv_extra=["--draft", "true"]
        )
        self.assertEqual(code, 1)
        self.assertIn("draft PR: full matrix not run, not merge-ready", err)

    def test_malformed_json_fails_closed(self):
        code, out, err = run_main("{not json")
        self.assertEqual(code, 1)
        self.assertIn("unreadable needs JSON", err)


if __name__ == "__main__":
    unittest.main()
