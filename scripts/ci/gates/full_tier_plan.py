#!/usr/bin/env python3
"""Tier planner: decide whether a CI run executes the FULL matrix.

The ``plan`` job of ci.yml runs this script and exposes its single output
``full=true|false``; every full-tier job gates on it, every fast-tier job on
its negation, and the merge gate reads it to pick the tier it evaluates.

Rules (implemented exactly)
    * any event other than ``pull_request`` (master push, workflow_dispatch,
      schedule)                                  -> full
    * a pull request that is not a draft (opened ready, or promoted with
      ``gh pr ready``)                           -> full
    * a DRAFT pull request                       -> full ONLY when the message
      of the PR head commit contains the explicit marker on a line of its
      own::

          [ci full]

      Otherwise the draft runs the fast tier.  Only the head commit is read,
      so one marker buys exactly one full run; the next push without it is
      fast again.

The marker must be the whole line (surrounding blanks and a CR are
tolerated): prose that merely mentions it never triggers a full run.  The
test selection inside the fast tier is NOT driven by the message; it stays
diff-based (scripts/ci/affected_crates.py).

Usage
    full_tier_plan.py --event <event> --draft true|false --message-file <path>

Writes ``full=<bool>`` to stdout and, when set, to ``$GITHUB_OUTPUT``.
Exit code 0 always on a valid invocation; an unreadable message file exits 1
(the ``plan`` job fails and the merge gate fails closed).
Python 3 standard library only; ``python3 -I`` compatible.
"""

from __future__ import annotations

import argparse
import os
import re
import sys

MARKER_LINE = re.compile(r"^[ \t]*\[ci full\][ \t]*\r?$", re.MULTILINE)


def has_marker(message: str) -> bool:
    """True when ``message`` carries the marker alone on one of its lines."""
    return MARKER_LINE.search(message) is not None


def decide(event: str, draft: bool, message: str) -> bool:
    """Pure tier decision: True = run the full matrix."""
    if event != "pull_request" or not draft:
        return True
    return has_marker(message)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--event", required=True, help="github.event_name")
    parser.add_argument(
        "--draft",
        required=True,
        choices=["true", "false"],
        help="github.event.pull_request.draft == true",
    )
    parser.add_argument(
        "--message-file",
        required=True,
        help="file holding the PR head commit message",
    )
    args = parser.parse_args(argv)

    try:
        with open(args.message_file, encoding="utf-8") as handle:
            message = handle.read()
    except OSError as exc:
        print("full_tier_plan.py: unreadable message file: %s" % exc, file=sys.stderr)
        return 1

    line = "full=%s" % ("true" if decide(args.event, args.draft == "true", message) else "false")
    print(line)
    output = os.environ.get("GITHUB_OUTPUT")
    if output:
        with open(output, "a", encoding="utf-8") as handle:
            handle.write(line + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
