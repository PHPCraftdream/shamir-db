"""Inventory test: ci.yml's `merge-gate` must need exactly REQUIRED_JOBS and every
other job of ci.yml, so a new gating job cannot be forgotten by the aggregate
(and therefore by the required-check rule).

Stdlib only; the workflow is read as text (no YAML parser): top-level job keys
are the two-space-indented keys under `jobs:`, the `needs:` list is the single
inline list on the `merge-gate` job.
"""

import os
import re
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(HERE))))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(HERE)), "gates"))

import merge_gate_check as mgc  # noqa: E402

CI = os.path.join(ROOT, ".github", "workflows", "ci.yml")

# Jobs that exist on purpose outside the aggregate: the draft-PR mirror of
# `engine-features` (runs on drafts only; the full tier covers the same cells)
# and the aggregate itself.
OUTSIDE_THE_AGGREGATE = {"engine-features-fast", "merge-gate"}


def read_ci():
    with open(CI, encoding="utf-8") as handle:
        return handle.read()


def job_keys(text):
    jobs = text[text.index("\njobs:\n") :]
    return re.findall(r"^  ([A-Za-z0-9_-]+):\s*$", jobs, re.M)


def merge_gate_needs(text):
    block = text[text.index("\n  merge-gate:\n") :]
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
        missing = [
            job
            for job in job_keys(text)
            if job not in needed and job not in OUTSIDE_THE_AGGREGATE
        ]
        self.assertEqual(missing, [], "gating jobs missing from merge-gate.needs")

    def test_exactly_one_job_carries_the_required_check_name(self):
        names = re.findall(r"^    name: merge gate \(required\)\s*$", read_ci(), re.M)
        self.assertEqual(len(names), 1)

    def test_aggregate_has_no_unknown_dependency(self):
        keys = set(job_keys(read_ci()))
        unknown = [job for job in merge_gate_needs(read_ci()) if job not in keys]
        self.assertEqual(unknown, [])


if __name__ == "__main__":
    unittest.main()
