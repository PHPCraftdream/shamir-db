#!/usr/bin/env python3
"""Aggregate merge gate: decide from the workflow's needs JSON if a run is merge-ready.

Why
    CI-03b deliverable 4. The ``merge-gate`` job (``if: always()``,
    ``needs:`` every gating job) pipes ``toJSON(needs)`` — the object
    ``{<jobname>: {result, outcome}, ...}`` — into this script together
    with the event name and the draft flag.  The script is the ONLY thing
    that decides the gate, so its semantics are the branch-protection
    semantics and are unit-tested for every transition.

Semantics (implemented exactly)
    * ``result`` ``failure`` or ``cancelled``  -> FAIL, naming the job.
    * ``skipped`` -> FAIL ("unexpectedly skipped") UNLESS skipped by design
      for the tier.  ``plan`` (the tier planner, scripts/ci/gates/
      full_tier_plan.py) has no ``if`` and is NEVER skipped by design.
      - FAST tier (``--event pull_request --draft true``): every
        ``REQUIRED_JOBS`` entry except ``plan`` and ``fast`` is skipped by
        design.  Even in the only-passing shape (``fast`` == success,
        everything else skipped) the gate still FAILS with
        "draft PR: full matrix not run, not merge-ready" — draft success
        never authorises a merge.
      - FULL tier (ready PR / master push / workflow_dispatch / a draft PR
        whose head commit carries the ``[ci full]`` marker): ``fast`` is
        skipped by design; every other required job must be ``success``.
    * ``--draft`` therefore means "the FAST tier was selected": ci.yml passes
      ``draft == true AND plan.outputs.full != 'true'``, so a marker run on a
      draft is judged as the full tier it actually ran.
    * any ``result`` that is not EXACTLY one of ``success`` / ``failure``
      / ``cancelled`` / ``skipped`` -> FAIL ("invalid result", naming the
      job and the bad value; a MISSING ``result`` key counts as invalid),
      so a workflow-file regression can never silently widen the gate.
    * unknown job name in needs (not in ``REQUIRED_JOBS``) -> FAIL
      ("unknown job", naming it).
    * a required job missing from needs -> FAIL.

Exit codes: 0 merge-ready, 1 gate failure (reasons on stderr); exit 1 is
fail-closed for unreadable needs JSON too.
Python 3 standard library only; ``python3 -I`` compatible.
"""

from __future__ import annotations

import argparse
import json
import sys

REQUIRED_JOBS = [
    "plan",
    "fast",
    "fmt",
    "clippy",
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
]

DRAFT_MESSAGE = "draft PR: full matrix not run, not merge-ready"


VALID_RESULTS = frozenset(("success", "failure", "cancelled", "skipped"))


def evaluate(
    needs: dict[str, dict],
    *,
    event: str,
    draft: bool,
) -> list[str]:
    """Pure gate logic: return the list of failure reasons (empty = pass)."""
    reasons: list[str] = []
    is_draft_pr = event == "pull_request" and draft

    for job, info in needs.items():
        if job not in REQUIRED_JOBS:
            reasons.append("unknown job in needs: %s" % job)
            continue
        if isinstance(info, dict):
            result = info.get("result")
        else:
            result = None
        if result not in VALID_RESULTS:
            reasons.append(
                "job %s: invalid result %r (must be one of %s)"
                % (job, result, ", ".join(sorted(VALID_RESULTS)))
            )
        elif result in ("failure", "cancelled"):
            reasons.append("job %s: %s" % (job, result))
        elif result == "skipped":
            by_design = job != "plan" and (job == "fast") != is_draft_pr
            if not by_design:
                reasons.append("job %s unexpectedly skipped" % job)

    for job in REQUIRED_JOBS:
        if job not in needs:
            reasons.append("missing required job: %s" % job)

    if is_draft_pr:
        reasons.append(DRAFT_MESSAGE)
    return reasons


def main(
    argv: list[str] | None = None,
    *,
    needs_text: str | None = None,
) -> int:
    """Entry point; ``needs_text`` injects stdin in tests."""
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--event", required=True, help="github.event_name")
    parser.add_argument(
        "--draft",
        required=True,
        choices=["true", "false"],
        help="github.event.pull_request.draft == true",
    )
    args = parser.parse_args(argv)

    try:
        if needs_text is None:
            needs_text = sys.stdin.read()
        needs = json.loads(needs_text)
        if not isinstance(needs, dict):
            raise ValueError("needs JSON must be an object")
    except (OSError, ValueError, json.JSONDecodeError) as exc:
        print("merge_gate_check.py: unreadable needs JSON: %s" % exc, file=sys.stderr)
        return 1

    reasons = evaluate(
        needs,
        event=args.event,
        draft=args.draft == "true",
    )
    if reasons:
        for reason in reasons:
            print("merge gate FAILED: %s" % reason, file=sys.stderr)
        return 1
    print("merge gate PASSED: all %d required jobs green" % len(REQUIRED_JOBS))
    return 0


if __name__ == "__main__":
    sys.exit(main())
