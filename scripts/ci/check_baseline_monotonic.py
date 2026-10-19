#!/usr/bin/env python3
"""Baseline monotonicity checker for the layout ratchet (CLAUDE.md "Layout limits").

Why
    ``check_layout.py`` checks HEAD against the HEAD baseline.  That baseline
    is committed text, so a contributor can silently ENLARGE an entry to make
    a new violation pass — the ratchet then "protects" the enlarged number.
    This script closes that hole by comparing the baseline file itself
    between two revisions: it fails on (a) any NEW entry (present at HEAD,
    absent at base) and (b) any entry whose recorded number GREW.  Shrinking
    or removing an entry is fine — that is the ratchet tightening.

Baseline format
    Identical to ``check_layout.py``: lines ``dir <path> <n>`` /
    ``file <path> <n>``; ``#`` comment lines and blank lines are ignored;
    parsing splits off the kind, then rsplit()s the trailing integer, so
    paths containing spaces work.  Parsing is reused from
    ``check_layout.load_baseline`` (over a temp-file / in-memory text) to
    avoid parser drift.

CLI
    ``check_baseline_monotonic.py [--baseline <path>] [--base <rev>]
    [--head <rev>]``

    ``--baseline``  path of the baseline file INSIDE the repo (default:
                    ``<root>/scripts/ci/layout_baseline.txt``).
    ``--base``      base revision; REQUIRED for local use (there is no
                    reliable local CI detection).  CI passes
                    ``--base origin/master --head HEAD``; the workflow wiring
                    is done in a later chunk, not here.
    ``--head``      head revision, default ``HEAD``.  Always a git object:
                    the orchestrator commits before running this script;
                    the working tree is never read.

Contents of each revision come from immutable git objects
(``git show <rev>:<path>``).  A baseline missing at BASE is an empty
baseline at base (bootstrap).  A baseline missing at HEAD means there is
nothing to protect: exit 0 with a note.

Exit codes: 0 pass (or nothing to protect), 1 any new/grown entry,
2 usage/internal error (unknown --base/--head rev, unreadable object,
malformed baseline line).
Python 3 standard library only.
"""

from __future__ import annotations

import argparse
import subprocess
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path

# Sentinel: ``base_text=MISSING`` / ``head_text=MISSING`` means "the baseline
# file does not exist at this revision" (what ``git_show`` returning None
# means).  ``None`` keeps its original meaning of "read the content from git
# for real" — passing None no longer simulates a missing file (round-1 bug:
# the missing-baseline tests were silently running real git against a fake
# repo and testing nothing).
MISSING = object()

sys.path.insert(
    0,
    str(Path(__file__).resolve().parent),
)

import check_layout as cl  # noqa: E402

BASELINE_DEFAULT_REL = cl.BASELINE_DEFAULT_REL


@dataclass
class Finding:
    """One baseline regression, deterministic print order (kind, path)."""

    tag: str  # "NEW" or "GREW"
    kind: str  # "dir" or "file"
    path: str
    base_n: int | None
    head_n: int


def parse_baseline_text(text: str, origin: str) -> dict[tuple[str, str], int]:
    """Parse baseline CONTENT via check_layout's parser (no drift).

    ``origin`` is only for error messages (revision + path).
    """
    with tempfile.NamedTemporaryFile(
        "w", encoding="utf-8", suffix=".txt", delete=False
    ) as fh:
        fh.write(text)
        tmp = Path(fh.name)
    try:
        return cl.load_baseline(tmp)
    except RuntimeError as exc:
        raise RuntimeError("%s: %s" % (origin, exc)) from exc
    finally:
        tmp.unlink(missing_ok=True)


def git_show(rev: str, rel_path: str, root: Path) -> str | None:
    """Content of ``rel_path`` at ``rev``; None if the file does not exist.

    Any other git failure (unknown rev, not a repo) raises RuntimeError.
    """
    raw = subprocess.run(
        ["git", "-C", str(root), "show", "%s:%s" % (rev, rel_path)],
        capture_output=True,
        check=False,
    )
    if raw.returncode == 0:
        return raw.stdout.decode("utf-8", "surrogateescape")
    err = raw.stderr.decode("utf-8", "replace")
    if "exists on disk, but not in" in err or "path does not exist" in err or (
        "does not exist" in err
    ):
        return None
    raise RuntimeError(
        "git show %s:%s failed (exit %d): %s"
        % (rev, rel_path, raw.returncode, err.strip())
    )


def compare(
    base: dict[tuple[str, str], int], head: dict[tuple[str, str], int]
) -> list[Finding]:
    """Pure comparison: NEW and GREW findings, sorted by (kind, path)."""
    findings: list[Finding] = []
    for key in sorted(head):
        kind, path = key
        head_n = head[key]
        base_n = base.get(key)
        if base_n is None:
            findings.append(Finding("NEW", kind, path, None, head_n))
        elif head_n > base_n:
            findings.append(Finding("GREW", kind, path, base_n, head_n))
    return findings


def render_lines(findings: list[Finding]) -> list[str]:
    """Deterministic stdout lines mirroring check_layout.py's style."""
    lines: list[str] = []
    for f in findings:
        if f.tag == "NEW":
            lines.append("NEW %s %s %d" % (f.kind, f.path, f.head_n))
        else:
            lines.append(
                "GREW %s %s %d -> %d" % (f.kind, f.path, f.base_n, f.head_n)
            )
    return lines


def main(
    argv: list[str] | None = None,
    *,
    root: Path | str | None = None,
    base_text: str | object | None = None,
    head_text: str | object | None = None,
) -> int:
    """Entry point; ``base_text``/``head_text`` inject git content in tests.

    ``MISSING`` simulates the file not existing at that revision; ``None``
    means "invoke git normally".
    """
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--baseline", default=None, help="baseline path inside the repo")
    parser.add_argument("--base", default=None, help="base revision (required locally)")
    parser.add_argument("--head", default="HEAD", help="head revision (default: HEAD)")
    args = parser.parse_args(argv)

    try:
        root_path = Path(root) if root is not None else cl.git_toplevel()
        rel = args.baseline if args.baseline is not None else BASELINE_DEFAULT_REL
        if args.base is None:
            print(
                "check_baseline_monotonic.py: --base is required "
                "(CI passes --base origin/master --head HEAD)",
                file=sys.stderr,
            )
            return 2
        if base_text is None:
            base_text = git_show(args.base, rel, root_path)
        if head_text is None:
            head_text = git_show(args.head, rel, root_path)
        # Normalise the REAL git read to the same sentinel the injected
        # path uses: ``git_show`` returns None when the file is absent at
        # that revision.  Round-2 bug: a real None fell through to
        # ``parse_baseline_text``, whose temp-file write expects str and
        # raised TypeError instead of applying the missing-file rules.
        if base_text is None:
            base_text = MISSING
        if head_text is None:
            head_text = MISSING
        if base_text is MISSING:
            # Missing at base -> empty baseline (bootstrap): every HEAD
            # entry becomes NEW.
            base_text = ""
        if head_text is MISSING:
            print(
                "baseline monotonic check: OK (no baseline at %s; nothing to protect)"
                % args.head
            )
            return 0
        base_entries = parse_baseline_text(
            base_text or "", "%s:%s" % (args.base, rel)
        )
        head_entries = parse_baseline_text(head_text, "%s:%s" % (args.head, rel))
        findings = compare(base_entries, head_entries)
        for line in render_lines(findings):
            print(line)
        if findings:
            print(
                "baseline monotonic check: FAILED (%d new/grown entr(y|ies))"
                % len(findings)
            )
            return 1
        print(
            "baseline monotonic check: OK (%d entries, none new or grown)"
            % len(head_entries)
        )
        return 0
    except RuntimeError as exc:
        print("check_baseline_monotonic.py: %s" % exc, file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
