#!/usr/bin/env python3
"""Count matching tests in a nextest list JSON; fail closed when none.

Why
    The ``loom-model`` job's preflight (and the ``test-discovery`` format
    smoke) must prove the produced ``cargo nextest list`` document really
    contains runnable tests under the given filter — an empty or
    malformed list must fail the gate instead of silently running a job
    over zero tests.

Document shape (real nextest output)
    ``{"rust-build-meta": ..., "test-count": N, "rust-suites":
    {"<binary-id>": {..., "testcases": {"<name>": {"ignored": bool,
    "filter-match": {"status": "matches"|"mismatches", ...}}}}}}``

    A testcase COUNTS when its ``filter-match.status`` is ``"matches"``
    and — when ``--name-filter <substring>`` is given — its name
    contains the substring.

CLI
    ``loom_preflight_check.py --list-file <path> [--name-filter <substring>]``

Exit codes: 0 count > 0 (count printed to stdout), 1 count == 0 (clear
message on stderr), 2 parse error (unreadable file, bad JSON, missing
``rust-suites``) — the CI step fails closed on 2.
Python 3 standard library only; ``python3 -I`` compatible.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


def count_matching_tests(document: dict, name_filter: str) -> int:
    """Count testcases matching the filter across every rust-suite."""
    count = 0
    for suite in document.get("rust-suites", {}).values():
        if not isinstance(suite, dict):
            continue
        testcases = suite.get("testcases")
        if not isinstance(testcases, dict):
            continue
        for name, info in testcases.items():
            if not isinstance(info, dict):
                continue
            match = info.get("filter-match")
            status = match.get("status") if isinstance(match, dict) else None
            if status != "matches":
                continue
            if name_filter and name_filter not in name:
                continue
            count += 1
    return count


def main(
    argv: list[str] | None = None,
    *,
    list_text: str | None = None,
) -> int:
    """Entry point; ``list_text`` injects the list file content in tests."""
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--list-file",
        required=True,
        help="path to the nextest list JSON document",
    )
    parser.add_argument(
        "--name-filter",
        default="",
        help="only count testcases whose name contains this substring",
    )
    args = parser.parse_args(argv)

    try:
        if list_text is None:
            list_text = Path(args.list_file).read_text(encoding="utf-8")
        document = json.loads(list_text)
        if not isinstance(document, dict):
            raise ValueError("list JSON is not an object")
        if not isinstance(document.get("rust-suites"), dict):
            raise ValueError("list JSON has no rust-suites object")
    except (OSError, ValueError, UnicodeDecodeError) as exc:
        print("loom_preflight_check.py: unreadable nextest list: %s" % exc, file=sys.stderr)
        return 2

    count = count_matching_tests(document, args.name_filter)
    if count == 0:
        print(
            "loom_preflight_check.py: no tests matched in %s"
            " (name filter %r): preflight FAILED" % (args.list_file, args.name_filter),
            file=sys.stderr,
        )
        return 1
    print("loom_preflight_check.py: %d matching test(s) in %s" % (count, args.list_file))
    return 0


if __name__ == "__main__":
    sys.exit(main())
