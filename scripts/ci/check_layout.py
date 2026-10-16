#!/usr/bin/env python3
"""Repository layout checker enforcing CLAUDE.md "Layout limits".

Enforces two rules over git-tracked files only:

File rule
    Every in-scope source file must have at most ``FILE_LIMIT`` (1000)
    physical lines.  A physical line is one ``b"\\n"`` byte (exact
    ``wc -l`` semantics).

Folder rule
    Every in-scope directory must have at most ``FOLDER_LIMIT`` (7)
    DIRECT tracked children.  A child is either a tracked file directly
    inside the directory, or a subdirectory containing at least one
    tracked descendant — of ANY type.  Data files (.json/.bin/.txt
    fixtures, lockfiles, .md) are never line-measured but DO occupy an
    entry slot.  Excluded-name subdirectories are never descended into
    for checking, but still count as one entry for their parent.

Scope
    Only files reported by ``git ls-files -z`` are considered at all;
    untracked or ignored paths (``target/``, scratch files) never count.
    Source extensions are ``SOURCE_EXTS``, matched by suffix (so
    ``x.d.ts`` counts, while ``ts`` without a dot, ``x.mjs`` and
    ``x.js`` do not).
    A directory is folder-checked iff (a) its subtree contains at least
    one tracked source file, (b) no segment of its path is in
    ``EXCLUDED_DIR_NAMES``, and (c) it is not a container root.
    Container roots are the repository root itself and the top-level
    ``crates/`` directory (see ``CONTAINER_DIR_NAMES``): they are exempt
    from the folder rule only, because the ``crates/*`` members glob
    makes regrouping impossible.  Source files directly inside a
    container root are still line-checked.

Exclusions
    ``EXCLUDED_DIR_NAMES`` lists directory names matched at ANY depth.
    A path under such a directory is entirely out of scope: it is not
    descended into, its files are not line-checked, and its own entry
    count is not evaluated — but it still counts as one entry for its
    parent.  Rationale, per name:
      vendor         vendored third-party code
      docs           documentation trees
      node_modules   npm dependency tree
      target         cargo build output
      dist           generated JS bundles
      coverage       coverage tool output
      fixtures       test fixture data
      testdata       test fixture data
      test_data      test fixture data
      __snapshots__  vitest snapshot files
      generated      codegen output
    Lockfiles never qualify as source files, so they need no separate
    exclusion list.

Ratchet baseline
    A committed, sorted text file (default
    ``<root>/scripts/ci/layout_baseline.txt``) with lines of the form
    ``dir <path> <count>`` / ``file <path> <count>``.  ``#`` comment
    lines and blank lines are ignored; parsing splits off the kind,
    then rsplit()s the trailing integer, so paths containing spaces
    work.  The baseline records every CURRENT violation.  The check
    fails when (a) a not-baselined folder/file violates a limit (a "new
    violation"), or (b) a baselined entry is above its recorded size
    ("grew").  A baselined entry that shrank (while still above the
    hard limit) or that is now within the hard limit produces a HINT,
    not a failure.  A missing baseline file in check mode behaves as an
    empty baseline.  ``--update-baseline`` never raises a recorded
    value and never adds a new violation; see the CLI help.

Exit codes: 0 pass, 1 any violation, 2 usage/internal error (missing
git repo, unreadable tracked file, malformed baseline line).
Python 3 standard library only.
"""

from __future__ import annotations

import argparse
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

SOURCE_EXTS: tuple[str, ...] = (".rs", ".ts", ".py", ".sh")
FOLDER_LIMIT = 7
FILE_LIMIT = 1000

# Directory names excluded from checking at ANY depth (see module docstring).
EXCLUDED_DIR_NAMES = frozenset(
    {
        "vendor",  # vendored third-party code
        "docs",  # documentation trees
        "node_modules",  # npm dependency tree
        "target",  # cargo build output
        "dist",  # generated JS bundles
        "coverage",  # coverage tool output
        "fixtures",  # test fixture data
        "testdata",  # test fixture data
        "test_data",  # test fixture data
        "__snapshots__",  # vitest snapshot files
        "generated",  # codegen output
    }
)

# Directories exempt from the folder rule only (workspace grouping).
CONTAINER_DIR_NAMES = frozenset({"crates"})

BASELINE_DEFAULT_REL = "scripts/ci/layout_baseline.txt"


def git_toplevel() -> Path:
    """Return the repository root (git run with cwd = this script's dir)."""
    raw = subprocess.run(
        ["git", "rev-parse", "--show-toplevel"],
        capture_output=True,
        text=True,
        check=False,
        cwd=Path(__file__).resolve().parent,
    )
    if raw.returncode != 0:
        raise RuntimeError(
            "not a git repository (git rev-parse --show-toplevel failed: %s)"
            % raw.stderr.strip()
        )
    return Path(raw.stdout.strip())


def git_tracked_files(root: Path) -> list[str]:
    """Sorted posix-style relative tracked paths under `root`."""
    raw = subprocess.run(
        ["git", "-C", str(root), "ls-files", "-z"],
        capture_output=True,
        check=False,
    )
    if raw.returncode != 0:
        raise RuntimeError(
            "git ls-files failed (exit %d): %s"
            % (raw.returncode, raw.stderr.decode("utf-8", "replace").strip())
        )
    files = raw.stdout.decode("utf-8", "surrogateescape").split("\0")
    return sorted(f for f in files if f)


def is_source_file(rel: str) -> bool:
    """True if `rel` has a source extension (suffix match on SOURCE_EXTS)."""
    return any(rel.endswith(ext) for ext in SOURCE_EXTS)


def is_excluded(rel: str) -> bool:
    """True if any DIRECTORY segment of `rel` is an excluded name.

    The filename itself is never matched.
    """
    parts = rel.split("/")
    return any(seg in EXCLUDED_DIR_NAMES for seg in parts[:-1])


def count_lines(root: Path, rel: str) -> int:
    """Physical line count of a tracked file (one b"\\n" byte = one line)."""
    try:
        data = (root / rel).read_bytes()
    except OSError as exc:
        raise RuntimeError("cannot read tracked file %r: %s" % (rel, exc)) from exc
    return data.count(b"\n")


@dataclass
class ScanResult:
    """Measured layout: direct-entry counts per dir, line counts per file."""

    dir_children: dict[str, int] = field(default_factory=dict)
    file_lines: dict[str, int] = field(default_factory=dict)


def scan(root: Path, tracked_files: list[str]) -> ScanResult:
    """Scan tracked files; collect in-scope dir entry counts and file lines."""
    result = ScanResult()
    # Pass A: every tracked file's parent counts it as a direct entry; source
    # files outside excluded trees additionally contribute line counts and
    # mark their ancestor directories as source-containing.
    dirs_with_source: set[str] = set()
    all_dirs: set[str] = set()
    for rel in tracked_files:
        parts = rel.split("/")
        parent = "/".join(parts[:-1])
        if parent:
            result.dir_children[parent] = result.dir_children.get(parent, 0) + 1
        for i in range(1, len(parts)):
            all_dirs.add("/".join(parts[:i]))
        if is_excluded(rel):
            continue
        if is_source_file(rel):
            for i in range(1, len(parts)):
                dirs_with_source.add("/".join(parts[:i]))
            result.file_lines[rel] = count_lines(root, rel)

    # Every distinct directory containing at least one tracked file anywhere
    # beneath it (directly or through file-free intermediate dirs) occupies
    # one entry slot in its own parent.
    for d in all_dirs:
        pp = "/".join(d.split("/")[:-1])
        if pp:
            result.dir_children[pp] = result.dir_children.get(pp, 0) + 1

    # Folder rule scope: has a tracked source file somewhere below, is not
    # an excluded-name directory (its own entry count is never evaluated;
    # it still counts as one entry for its parent), is not a container root.
    for d in list(result.dir_children):
        if any(seg in EXCLUDED_DIR_NAMES for seg in d.split("/")):
            del result.dir_children[d]
            continue
        segs = d.split("/")
        if len(segs) == 1 and segs[0] in CONTAINER_DIR_NAMES:
            del result.dir_children[d]
            continue
        if d not in dirs_with_source:
            del result.dir_children[d]
    return result


def load_baseline(path: Path) -> dict[tuple[str, str], int]:
    """Parse a baseline file into {(kind, path): count}."""
    entries: dict[tuple[str, str], int] = {}
    try:
        text = path.read_text(encoding="utf-8")
    except FileNotFoundError:
        return entries
    except OSError as exc:
        raise RuntimeError("cannot read baseline %s: %s" % (path, exc)) from exc
    for lineno, raw in enumerate(text.splitlines(), start=1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        pieces = line.split(None, 1)
        if len(pieces) != 2 or pieces[0] not in ("dir", "file"):
            raise RuntimeError(
                "malformed baseline line %d in %s: %r" % (lineno, path, raw)
            )
        kind, rest = pieces
        head, sep, tail = rest.rpartition(" ")
        if not sep or not tail.lstrip("-").isdigit():
            raise RuntimeError(
                "malformed baseline line %d in %s: %r" % (lineno, path, raw)
            )
        entries[(kind, head)] = int(tail)
    return entries


def format_entry(kind: str, path: str, n: int) -> str:
    """Render one baseline line: ``dir <path> <n>``."""
    return "%s %s %d" % (kind, path, n)


@dataclass
class Violation:
    kind: str
    path: str
    measured: int
    limit: int
    baselined: bool


@dataclass
class Hint:
    kind: str
    path: str
    measured: int
    baseline_value: int
    within_limit: bool


@dataclass
class Evaluation:
    violations: list[Violation] = field(default_factory=list)
    hints: list[Hint] = field(default_factory=list)
    baselined_ok: int = 0


def evaluate(
    report: ScanResult, baseline: dict[tuple[str, str], int]
) -> Evaluation:
    """Compare a scan against the baseline and hard limits."""
    ev = Evaluation()
    for kind, measured_map, hard in (
        ("dir", report.dir_children, FOLDER_LIMIT),
        ("file", report.file_lines, FILE_LIMIT),
    ):
        for path in sorted(measured_map):
            n = measured_map[path]
            base = baseline.get((kind, path))
            if n > hard:
                if base is None or base < n:
                    limit = base if base is not None and base >= hard else hard
                    ev.violations.append(
                        Violation(kind, path, n, limit, base is not None)
                    )
                else:  # n <= base: not a new violation
                    if n < base:
                        ev.hints.append(Hint(kind, path, n, base, False))
                        ev.baselined_ok += 1
                    else:  # n == base: still exactly at the ratchet point
                        ev.baselined_ok += 1
            else:
                if base is not None:
                    ev.hints.append(Hint(kind, path, n, base, True))
                    ev.baselined_ok += 1
    return ev


def _unit(kind: str) -> str:
    """Measured-unit words for one kind: ``direct entries`` or ``lines``."""
    return "direct entries" if kind == "dir" else "lines"


def render_lines(evaluation: Evaluation) -> list[str]:
    """Deterministic stdout lines for an evaluation (kind, then path)."""
    lines: list[str] = []
    for v in sorted(evaluation.violations, key=lambda v: (v.kind, v.path)):
        if v.baselined:
            hard = FOLDER_LIMIT if v.kind == "dir" else FILE_LIMIT
            lines.append(
                "VIOLATION %s %s: %d %s (limit %d [baselined; hard limit %d])"
                % (v.kind, v.path, v.measured, _unit(v.kind), v.limit, hard)
            )
        else:
            limit = FOLDER_LIMIT if v.kind == "dir" else FILE_LIMIT
            lines.append(
                "VIOLATION %s %s: %d %s (limit %d)"
                % (v.kind, v.path, v.measured, _unit(v.kind), limit)
            )
    for h in sorted(evaluation.hints, key=lambda h: (h.kind, h.path)):
        limit = FOLDER_LIMIT if h.kind == "dir" else FILE_LIMIT
        if h.within_limit:
            lines.append(
                "HINT %s %s: %d %s (baseline %d, limit %d) - within limits; "
                "run --update-baseline to remove"
                % (
                    h.kind,
                    h.path,
                    h.measured,
                    _unit(h.kind),
                    h.baseline_value,
                    limit,
                )
            )
        else:
            lines.append(
                "HINT %s %s: %d %s (baseline %d, limit %d) - shrank; "
                "run --update-baseline to lower to %d"
                % (
                    h.kind,
                    h.path,
                    h.measured,
                    _unit(h.kind),
                    h.baseline_value,
                    limit,
                    h.measured,
                )
            )
    return lines


def update_entries(
    report: ScanResult, baseline: dict[tuple[str, str], int]
) -> tuple[dict[tuple[str, str], int], list[str], bool]:
    """Ratchet the baseline against a scan.

    Returns (new_entries, log_lines, blocking).
    """
    new = dict(baseline)
    log: list[str] = []
    blocking = False
    measured: dict[tuple[str, str], int] = {}
    measured.update({("dir", p): n for p, n in report.dir_children.items()})
    measured.update({("file", p): n for p, n in report.file_lines.items()})
    limits = {"dir": FOLDER_LIMIT, "file": FILE_LIMIT}

    for key in sorted(baseline):
        kind, path = key
        base = baseline[key]
        n = measured.get(key)
        limit = limits[kind]
        if n is None:
            del new[key]
            log.append("baseline: removed %s %s (no longer measured)" % (kind, path))
            continue
        if n <= limit:
            del new[key]
            log.append("baseline: removed %s %s (now %d <= %d)" % (kind, path, n, limit))
        elif n < base:
            new[key] = n
            log.append("baseline: lowered %s %s %d -> %d" % (kind, path, base, n))
        elif n > base:
            log.append(
                "baseline: NOT raised %s %s (now %d > baseline %d; fix the regression)"
                % (kind, path, n, base)
            )
            blocking = True

    for key in sorted(measured):
        kind, path = key
        n = measured[key]
        if key in baseline:
            continue
        if n > limits[kind]:
            log.append(
                "baseline: NOT added %s %s (new violation; --update-baseline never adds)"
                % (kind, path)
            )
            blocking = True
    return new, log, blocking


def write_baseline(path: Path, entries: dict[tuple[str, str], int]) -> None:
    """Write a sorted baseline file with a short header comment."""
    lines = [
        "# layout baseline: current violations of the layout limits",
        "# format: dir <path> <n> | file <path> <n>; maintained by check_layout.py",
    ]
    for key in sorted(entries):
        lines.append(format_entry(key[0], key[1], entries[key]))
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")


def main(
    argv: list[str] | None = None,
    *,
    tracked_files: list[str] | None = None,
    root: Path | str | None = None,
) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--update-baseline",
        action="store_true",
        help="ratchet the baseline (never raises, never adds new violations)",
    )
    parser.add_argument("--baseline", default=None, help="baseline file path")
    parser.add_argument("--root", default=None, help="repository root (default: git toplevel)")
    args = parser.parse_args(argv)

    try:
        if root is None and args.root is not None:
            root = args.root
        root_path = Path(root) if root is not None else git_toplevel()
        if tracked_files is None:
            tracked_files = git_tracked_files(root_path)
        baseline_path = (
            Path(args.baseline)
            if args.baseline is not None
            else root_path / BASELINE_DEFAULT_REL
        )
        report = scan(root_path, tracked_files)
        if args.update_baseline:
            existed = baseline_path.exists()
            baseline = load_baseline(baseline_path)
            if existed:
                new, log, blocking = update_entries(report, baseline)
            else:
                # Bootstrap: a missing baseline is created with ALL current
                # violations (initial generation).
                new = {}
                log = []
                blocking = False
                for v in evaluate(report, {}).violations:
                    new[(v.kind, v.path)] = v.measured
                log.append(
                    "baseline: created (%d entries)" % len(new)
                )
            for line in log:
                print(line)
            if existed:
                serialized = {k: new[k] for k in sorted(new)}
                old_serialized = {k: baseline[k] for k in sorted(baseline)}
                if serialized != old_serialized:
                    write_baseline(baseline_path, serialized)
                    print("baseline: written (%d entries)" % len(serialized))
                else:
                    print("baseline: unchanged")
            else:
                write_baseline(baseline_path, dict(sorted(new.items())))
                print("baseline: written (%d entries)" % len(new))
            check_baseline = load_baseline(baseline_path)
            evaluation = evaluate(report, check_baseline)
            for line in render_lines(evaluation):
                print(line)
            n_v = len(evaluation.violations)
            n_h = len(evaluation.hints)
            if blocking or n_v:
                print("layout check: FAILED (%d violation(s), %d hint(s))" % (n_v, n_h))
                return 1
            print("layout check: OK (%d baselined, %d hint(s))" % (evaluation.baselined_ok, n_h))
            return 0
        baseline = load_baseline(baseline_path)
        evaluation = evaluate(report, baseline)
        for line in render_lines(evaluation):
            print(line)
        n_v = len(evaluation.violations)
        n_h = len(evaluation.hints)
        if n_v:
            print("layout check: FAILED (%d violation(s), %d hint(s))" % (n_v, n_h))
            return 1
        print(
            "layout check: OK (%d baselined, %d hint(s))"
            % (evaluation.baselined_ok, n_h)
        )
        return 0
    except RuntimeError as exc:
        print("check_layout.py: %s" % exc, file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
