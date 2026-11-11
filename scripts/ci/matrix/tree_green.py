#!/usr/bin/env python3
"""Skip a push only when its tree has a confirmed green full PR run.

The run query is limited to the newest 30 successful ``ci.yml`` pull-request
runs. A successful ``merge gate (required)`` job proves the run was full tier;
a workflow's overall success alone is not sufficient. Every uncertainty is a
false result so the caller runs the computed matrix.

``--format github-output`` prints ``tree_green=true|false`` and a fixed,
safe ``reason=...`` value on separate lines.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys

RUN_LIMIT = 30
_REPO_RE = re.compile(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+\Z")
_SHA_RE = re.compile(r"[0-9a-fA-F]{40}\Z")
_REQUIRED_GATE = "merge gate (required)"
_CONCLUSIONS = frozenset(
    (
        "action_required",
        "cancelled",
        "failure",
        "neutral",
        "skipped",
        "stale",
        "success",
        "timed_out",
        "startup_failure",
    )
)


class _ApiError(Exception):
    pass


class _MalformedResponse(Exception):
    pass


def _api_json(repo: str, endpoint: str, runner) -> object:
    try:
        result = runner(
            ["gh", "api", "repos/%s/%s" % (repo, endpoint)],
            capture_output=True,
            text=True,
            check=False,
        )
    except Exception as exc:
        raise _ApiError from exc
    if getattr(result, "returncode", 1) != 0:
        raise _ApiError
    try:
        return json.loads(result.stdout)
    except (AttributeError, TypeError, ValueError) as exc:
        raise _MalformedResponse from exc


def _tree_for_commit(repo: str, sha: str, runner) -> str:
    data = _api_json(repo, "commits/%s" % sha, runner)
    actual_sha = data.get("sha") if isinstance(data, dict) else None
    if not isinstance(actual_sha, str) or actual_sha.lower() != sha.lower():
        raise _MalformedResponse
    commit = data.get("commit")
    tree = commit.get("tree") if isinstance(commit, dict) else None
    tree_sha = tree.get("sha") if isinstance(tree, dict) else None
    if not isinstance(tree_sha, str) or not _SHA_RE.fullmatch(tree_sha):
        raise _MalformedResponse
    return tree_sha.lower()


def _successful_required_gate(repo: str, run_id: int, runner) -> bool:
    data = _api_json(
        repo,
        "actions/runs/%d/jobs?per_page=100" % run_id,
        runner,
    )
    jobs = data.get("jobs") if isinstance(data, dict) else None
    if not isinstance(jobs, list):
        raise _MalformedResponse
    for job in jobs:
        if not isinstance(job, dict) or not isinstance(job.get("name"), str):
            raise _MalformedResponse
        if job["name"] == _REQUIRED_GATE:
            return job.get("conclusion") == "success"
    return False


def _head_contains(repo: str, before: str, head_sha: str, runner) -> bool:
    """A PR run tests head merged into master, which equals head only when head holds `before`."""
    data = _api_json(repo, "compare/%s...%s" % (before, head_sha), runner)
    status = data.get("status") if isinstance(data, dict) else None
    if not isinstance(status, str):
        raise _MalformedResponse
    return status in ("ahead", "identical")


def check_tree_green(repo: str, after: str, runner=None, before: str | None = None) -> tuple[bool, str]:
    """Return whether a confirmed full PR run has the pushed commit's tree."""
    if not isinstance(repo, str) or not _REPO_RE.fullmatch(repo):
        return False, "invalid_input"
    if not isinstance(after, str) or not _SHA_RE.fullmatch(after):
        return False, "invalid_input"
    if not isinstance(before, str) or not _SHA_RE.fullmatch(before):
        return False, "invalid_input"
    if runner is None:
        runner = subprocess.run

    try:
        data = _api_json(
            repo,
            "actions/workflows/ci.yml/runs?event=pull_request&status=success&per_page=%d"
            % RUN_LIMIT,
            runner,
        )
        runs = data.get("workflow_runs") if isinstance(data, dict) else None
        if not isinstance(runs, list) or len(runs) > RUN_LIMIT:
            raise _MalformedResponse
        for run in runs:
            if not isinstance(run, dict):
                raise _MalformedResponse
            run_id = run.get("id")
            head_sha = run.get("head_sha")
            if (
                isinstance(run_id, bool)
                or not isinstance(run_id, int)
                or run_id <= 0
                or not isinstance(head_sha, str)
                or not _SHA_RE.fullmatch(head_sha)
                or run.get("event") != "pull_request"
                or run.get("status") != "completed"
                or run.get("conclusion") not in _CONCLUSIONS
            ):
                raise _MalformedResponse

        if not runs:
            return False, "no_matching_full_run"

        after_tree = _tree_for_commit(repo, after, runner)
        for run in runs:
            if run["conclusion"] != "success":
                continue
            head_sha = run["head_sha"]
            head_repo = run.get("head_repository")
            if not isinstance(head_repo, dict) or head_repo.get("full_name") != repo:
                continue
            if _tree_for_commit(repo, head_sha, runner) != after_tree:
                continue
            if not _head_contains(repo, before, head_sha, runner):
                continue
            if _successful_required_gate(repo, run["id"], runner):
                return True, "matching_full_run"
        return False, "no_matching_full_run"
    except _MalformedResponse:
        return False, "malformed_response"
    except _ApiError:
        return False, "api_error"


def main(argv: list[str] | None = None, *, runner=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--repo", required=True, help="owner/repository")
    parser.add_argument("--before", required=True, help="previous tip of the pushed branch")
    parser.add_argument("--after", required=True, help="pushed commit SHA")
    parser.add_argument(
        "--format",
        choices=("github-output",),
        default="github-output",
        help="output format (default: github-output)",
    )
    args = parser.parse_args(argv)
    tree_green, reason = check_tree_green(args.repo, args.after, runner=runner, before=args.before)
    print("tree_green=%s" % ("true" if tree_green else "false"))
    print("reason=%s" % reason)
    return 0


if __name__ == "__main__":
    sys.exit(main())
