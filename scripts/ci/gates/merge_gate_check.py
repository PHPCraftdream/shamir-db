#!/usr/bin/env python3
"""Aggregate merge gate, including fail-closed matrix and tree skip proofs.

The merge-gate job passes GitHub's ``toJSON(needs)`` object on stdin. A
successful ``affected`` result may authorize a matrix-empty skip only when its
``outputs.matrix`` is the complete version-1 test-matrix JSON contract and
passes the validation below. An arbitrary reason map or partial output is not
an authorization. Tree-equality skips are a separate, push-only proof from
``affected.outputs.tree_green == 'true'`` and are limited to TREE_SKIP_JOBS.

``--draft true`` means the fast tier was selected (not merely that the PR is
a draft); the planner passes false for a draft whose head carries ``[ci full]``.
Python 3 standard library only; ``python3 -I`` compatible.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from typing import Any

REQUIRED_JOBS = [
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
]

# Only these explicitly named jobs may be skipped for push-tree equality.
# Keep this aligned with workflow-level `if:` guards; source-integrity,
# affected, matrix-self-check, formatting and dependency gates always run.
TREE_SKIP_JOBS = frozenset(
    {
        "test-build",
        "test",
        "integration",
        "ts-unit",
        "loom-model",
        "test-discovery",
        "rust-guard-selftest",
        "node-binding",
    }
)

MATRIX_FLAGS = frozenset(
    {"engine", "ts", "node", "wasm32", "deny", "pipeline", "docs_only"}
)
CRATE_NAME = re.compile(r"^[A-Za-z0-9_-]+$")
VALID_RESULTS = frozenset(("success", "failure", "cancelled", "skipped"))
DRAFT_MESSAGE = "draft PR: full matrix not run, not merge-ready"

# Jobs omitted by tier or by design. Matrix-self-check and rust-guard
# safety jobs are deliberate exceptions and still run on every PR.
FULL_TIER_ONLY_JOBS = frozenset(
    {
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
        "rustdoc",
        "engine-features",
        "node-binding",
    }
)


def _valid_matrix(value: Any) -> tuple[dict[str, Any] | None, str | None]:
    """Validate the complete stage-1 matrix JSON contract; return error text."""
    if not isinstance(value, dict):
        return None, "matrix must be an object"
    expected_keys = {
        "version", "base", "head", "scope", "all_reasons", "crates", "flags", "args"
    }
    if set(value) != expected_keys:
        return None, "matrix keys must be exactly %s" % ", ".join(sorted(expected_keys))
    if type(value.get("version")) is not int or value["version"] != 1:
        return None, "matrix version must be integer 1"
    if not isinstance(value.get("base"), str) or not isinstance(value.get("head"), str):
        return None, "matrix base and head must be strings"
    scope = value.get("scope")
    if not isinstance(scope, str) or scope not in {"none", "bounded", "all"}:
        return None, "matrix scope must be none, bounded, or all"
    all_reasons = value.get("all_reasons")
    if not isinstance(all_reasons, list) or any(not isinstance(reason, str) for reason in all_reasons):
        return None, "matrix all_reasons must be a list of strings"
    args = value.get("args")
    if not isinstance(args, str):
        return None, "matrix args must be a string"

    flags = value.get("flags")
    if not isinstance(flags, dict) or set(flags) != MATRIX_FLAGS:
        return None, "matrix flags must contain exactly %s" % ", ".join(sorted(MATRIX_FLAGS))
    if any(type(flags[name]) is not bool for name in MATRIX_FLAGS):
        return None, "matrix flags must all be booleans"

    crates = value.get("crates")
    if not isinstance(crates, list):
        return None, "matrix crates must be a list"
    seen: set[str] = set()
    for index, row in enumerate(crates):
        prefix = "matrix crates[%d]" % index
        if not isinstance(row, dict):
            return None, "%s must be an object" % prefix
        if set(row) != {"name", "lib", "integration", "check_only", "reasons"}:
            return None, "%s has invalid fields" % prefix
        name = row.get("name")
        if not isinstance(name, str) or not CRATE_NAME.fullmatch(name):
            return None, "%s name must match [A-Za-z0-9_-]+" % prefix
        if name in seen:
            return None, "%s duplicate crate name %r" % (prefix, name)
        seen.add(name)
        if type(row["lib"]) is not bool or type(row["check_only"]) is not bool:
            return None, "%s lib and check_only must be booleans" % prefix
        integration = row.get("integration")
        if integration != "all" and not isinstance(integration, list):
            return None, "%s integration must be 'all' or a list" % prefix
        if isinstance(integration, list) and any(
            not isinstance(binary, str) or not CRATE_NAME.fullmatch(binary)
            for binary in integration
        ):
            return None, "%s integration names must match [A-Za-z0-9_-]+" % prefix
        if isinstance(integration, list) and len(set(integration)) != len(integration):
            return None, "%s integration names must be unique" % prefix
        reasons = row.get("reasons")
        if not isinstance(reasons, list) or not reasons or any(
            not isinstance(reason, str) or not reason.strip() for reason in reasons
        ):
            return None, "%s reasons must be a non-empty list of strings" % prefix
        if row["check_only"] and (row["lib"] or integration == "all" or bool(integration)):
            return None, "%s check_only row cannot select tests" % prefix
        if not row["lib"] and not row["check_only"] and integration != "all" and not integration:
            return None, "%s has no selected checks or tests" % prefix

    tokens = args.split()
    if tokens == ["--workspace"]:
        if scope != "all":
            return None, "matrix --workspace args require scope all"
    else:
        if len(tokens) % 2 or any(tokens[index] != "-p" for index in range(0, len(tokens), 2)):
            return None, "matrix args must be -p package pairs"
        names = tokens[1::2]
        if any(not CRATE_NAME.fullmatch(name) for name in names):
            return None, "matrix args contain an invalid package name"
        if len(set(names)) != len(names) or set(names) != seen:
            return None, "matrix args do not match validated crate rows"
    if scope == "none" and crates:
        return None, "matrix scope none cannot contain crate rows"
    return value, None


def _affected_outputs(needs: dict[str, Any]) -> dict[str, Any] | None:
    info = needs.get("affected")
    if not isinstance(info, dict) or info.get("result") != "success":
        return None
    outputs = info.get("outputs")
    return outputs if isinstance(outputs, dict) else None


def _matrix_from_needs(needs: dict[str, Any]) -> tuple[dict[str, Any] | None, str | None]:
    outputs = _affected_outputs(needs)
    if outputs is None:
        return None, "affected job did not succeed with outputs"
    encoded = outputs.get("matrix")
    if not isinstance(encoded, str) or not encoded:
        return None, "affected outputs.matrix is missing"
    try:
        value = json.loads(encoded)
    except (TypeError, ValueError, json.JSONDecodeError) as exc:
        return None, "affected outputs.matrix is invalid JSON: %s" % exc
    matrix, error = _valid_matrix(value)
    if error:
        return None, "affected outputs.matrix is invalid: %s" % error
    return matrix, None


def _matrix_skip_reason(job: str, matrix: dict[str, Any]) -> str | None:
    """Return why a valid matrix proves this job has no selected work."""
    flags = matrix["flags"]
    crates = matrix["crates"]
    if flags["pipeline"]:
        return None
    if job == "clippy":
        return "matrix selected no crate rows" if not crates else None
    has_lib = any(row["lib"] for row in crates)
    if job == "wasm32-guest-check" and not flags["wasm32"]:
        return "matrix wasm32 flag is false"
    if job == "test-build" and not has_lib:
        return "matrix selected no lib tests"
    if job == "test" and not has_lib and not flags["engine"]:
        return "matrix selected no lib tests or engine doctests"
    if job == "integration" and not any(
        row["integration"] == "all" or bool(row["integration"]) for row in crates
    ):
        return "matrix selected no integration binaries"
    if job in {"rustdoc", "engine-features", "loom-model"} and not flags["engine"]:
        return "matrix engine flag is false"
    if job == "ts-unit" and not flags["ts"]:
        return "matrix ts flag is false"
    if job == "node-binding" and not flags["node"]:
        return "matrix node flag is false"
    if job == "deny" and not flags["deny"]:
        return "matrix deny flag is false"
    return None


def evaluate(
    needs: dict[str, Any],
    *,
    event: str,
    draft: bool,
    notices: list[str] | None = None,
) -> list[str]:
    """Pure gate logic: return failure reasons (empty means merge-ready)."""
    reasons: list[str] = []
    is_draft_pr = event == "pull_request" and draft
    affected_outputs = _affected_outputs(needs)
    tree_green = (
        event == "push"
        and affected_outputs is not None
        and affected_outputs.get("tree_green") == "true"
    )
    matrix: dict[str, Any] | None = None
    matrix_error: str | None = None
    if isinstance(needs.get("affected"), dict) and needs["affected"].get("result") == "success":
        matrix, matrix_error = _matrix_from_needs(needs)
    if matrix_error:
        reasons.append("affected outputs.matrix invalid: %s" % matrix_error)
    if (
        event == "workflow_dispatch"
        and affected_outputs is not None
        and affected_outputs.get("manual_partial") == "true"
    ):
        reasons.append(
            "manual run with an explicit range or crates selection is not merge-ready "
            "(use scope=workspace or the default branch range)"
        )

    for job, info in needs.items():
        if job not in REQUIRED_JOBS:
            reasons.append("unknown job in needs: %s" % job)
            continue
        result = info.get("result") if isinstance(info, dict) else None
        if not isinstance(result, str) or result not in VALID_RESULTS:
            reasons.append(
                "job %s: invalid result %r (must be one of %s)"
                % (job, result, ", ".join(sorted(VALID_RESULTS)))
            )
        elif result in ("failure", "cancelled"):
            # Failed/cancelled jobs are never excused by tier, matrix, or tree.
            reasons.append("job %s: %s" % (job, result))
        elif result == "skipped":
            if job == "matrix-self-check" and event != "pull_request":
                if notices is not None:
                    notices.append(
                        "matrix-self-check skipped: event %s is not pull_request" % event
                    )
                continue

            # The fast tier is explicitly not merge-ready. Only full-tier jobs
            # may be absent on an ordinary draft; affected and safety jobs run.
            if is_draft_pr and job in FULL_TIER_ONLY_JOBS:
                continue
            # `fast` is the only job omitted by design in the full tier.
            if not is_draft_pr and job == "fast":
                continue

            if tree_green and job in TREE_SKIP_JOBS:
                if notices is not None:
                    notices.append(
                        "tree skip: job %s (affected.outputs.tree_green=true on push)" % job
                    )
                continue

            # A matrix skip needs the successful affected result and its full,
            # validated JSON contract. Do not consult output reasons as proof.
            if job in REQUIRED_JOBS and job not in {
                "plan",
                "affected",
                "fast",
                "fmt",
                "cooldown",
                "source-integrity",
                "matrix-self-check",
            }:
                if matrix is None and matrix_error is None:
                    matrix, matrix_error = _matrix_from_needs(needs)
                if matrix is not None:
                    reason = _matrix_skip_reason(job, matrix)
                    if reason is not None:
                        if notices is not None:
                            notices.append("matrix skip: job %s (%s)" % (job, reason))
                        continue
            reasons.append("job %s unexpectedly skipped" % job)

    for job in REQUIRED_JOBS:
        if job not in needs:
            reasons.append("missing required job: %s" % job)

    if is_draft_pr:
        reasons.append(DRAFT_MESSAGE)
    return reasons


def main(argv: list[str] | None = None, *, needs_text: str | None = None) -> int:
    """Entry point; ``needs_text`` injects stdin for unit tests."""
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--event",
        required=True,
        choices=["pull_request", "push", "workflow_dispatch", "schedule"],
        help="github.event_name",
    )
    parser.add_argument(
        "--draft",
        required=True,
        choices=["true", "false"],
        help="whether the fast draft tier was selected",
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

    notices: list[str] = []
    reasons = evaluate(
        needs,
        event=args.event,
        draft=args.draft == "true",
        notices=notices,
    )
    for notice in notices:
        print("::notice::%s" % notice)
    if reasons:
        for reason in reasons:
            print("merge gate FAILED: %s" % reason, file=sys.stderr)
        return 1
    print("merge gate PASSED: all %d required jobs green" % len(REQUIRED_JOBS))
    return 0


if __name__ == "__main__":
    sys.exit(main())
