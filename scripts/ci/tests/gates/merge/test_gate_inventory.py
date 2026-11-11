"""Inventory checks for the CI merge gate and its narrowly allowed tree skips."""

import os
import re
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(HERE)))))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.dirname(HERE))), "gates"))

import merge_gate_check as mgc  # noqa: E402

CI = os.path.join(ROOT, ".github", "workflows", "ci.yml")
OUTSIDE_THE_AGGREGATE = {"engine-features-fast", "merge-gate"}
EXPECTED_TREE_SKIP_JOBS = frozenset(
    {
        "test-build",
        "test",
        "integration",
        "ts-unit",
        "loom-model",
        "node-binding",
        "test-discovery",
        "rust-guard-selftest",
    }
)


def read_ci():
    with open(CI, encoding="utf-8") as handle:
        return handle.read()


def job_keys(text):
    jobs = text[text.index("\njobs:\n") :]
    return re.findall(r"^  ([A-Za-z0-9_-]+):\s*$", jobs, re.M)


def job_block(text, name):
    marker = "\n  %s:\n" % name
    start = text.index(marker) + len(marker)
    match = re.search(r"^  [A-Za-z0-9_-]+:\s*$", text[start:], re.M)
    end = start + match.start() if match else len(text)
    return text[start:end]


def merge_gate_needs(text):
    block = job_block(text, "merge-gate")
    match = re.search(r"^    needs: \[([^\]]*)\]\s*$", block, re.M)
    if not match:
        raise AssertionError("merge-gate has no inline `needs: [...]` list")
    return [item.strip() for item in match.group(1).split(",") if item.strip()]


class GateInventoryTests(unittest.TestCase):
    def test_merge_gate_needs_equals_required_jobs(self):
        self.assertEqual(sorted(merge_gate_needs(read_ci())), sorted(mgc.REQUIRED_JOBS))

    def test_every_other_job_is_in_the_aggregate(self):
        text = read_ci()
        needed = set(merge_gate_needs(text))
        missing = [job for job in job_keys(text) if job not in needed and job not in OUTSIDE_THE_AGGREGATE]
        self.assertEqual(missing, [], "gating jobs missing from merge-gate.needs")

    def test_exactly_one_job_carries_the_required_check_name(self):
        names = re.findall(r"^    name: merge gate \(required\)\s*$", read_ci(), re.M)
        self.assertEqual(len(names), 1)

    def test_aggregate_has_no_unknown_dependency(self):
        keys = set(job_keys(read_ci()))
        unknown = [job for job in merge_gate_needs(read_ci()) if job not in keys]
        self.assertEqual(unknown, [])

    def test_affected_always_runs_and_publishes_matrix(self):
        text = read_ci()
        block = job_block(text, "affected")
        self.assertNotRegex(block, r"(?m)^    if:")
        self.assertIn("matrix:", block)
        self.assertIn("affected", merge_gate_needs(text))

    def test_matrix_self_check_is_required_and_pull_request_only(self):
        text = read_ci()
        block = job_block(text, "matrix-self-check")
        self.assertRegex(block, r"(?m)^    if:.*github\.event_name\s*==\s*'pull_request'")
        self.assertIn("test_matrix.py", block)
        self.assertIn("matrix-self-check", merge_gate_needs(text))

    def test_tree_skip_inventory_is_exact(self):
        self.assertEqual(mgc.TREE_SKIP_JOBS, EXPECTED_TREE_SKIP_JOBS)
        self.assertTrue(
            {
                "clippy",
                "rustdoc",
                "engine-features",
                "deny",
                "source-integrity",
                "affected",
                "cooldown",
                "matrix-self-check",
            }.isdisjoint(mgc.TREE_SKIP_JOBS)
        )
        self.assertNotIn("wasm32-guest-check", mgc.TREE_SKIP_JOBS)

    def test_workflow_tree_job_guards_are_only_on_tree_skip_jobs(self):
        text = read_ci()
        guard = re.compile(r"(?m)^    if:.*tree_green")
        guarded_jobs = {
            job for job in job_keys(text) if guard.search(job_block(text, job))
        }
        self.assertLessEqual(guarded_jobs, EXPECTED_TREE_SKIP_JOBS)
        self.assertNotIn("wasm32-guest-check", guarded_jobs)


if __name__ == "__main__":
    unittest.main()
