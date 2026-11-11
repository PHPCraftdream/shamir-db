#!/usr/bin/env python3
"""Legacy affected-crates selector for the CI fast tier.

Reads the files changed between a base ref and HEAD (``git diff --name-only
<base>...HEAD``) and prints, on ONE line, the sorted ``-p <crate>`` arguments
limiting cargo / ./scripts/test.sh to the crates a change can affect:

  * a changed file is mapped to the workspace package whose manifest lives
    in the NEAREST ancestor directory of the file (``crates/g/x/src/lib.rs``
    -> the package at ``crates/g/x``; also covers non-``crates/`` layouts
    like ``vendor/hnsw_rs``);
  * every workspace package that depends on a marked package (directly or
    transitively, across normal / dev / optional / build deps among
    workspace members) is added too;
  * if any changed path is ``Cargo.lock``, the root ``Cargo.toml``,
    ``rust-toolchain*``, ``deny.toml``, anything under ``.cargo/`` /
    ``.config/`` / ``scripts/`` / ``.github/``, or any file outside
    ``crates/`` / ``docs/`` that is not ``*.md``, the ``WORKSPACE``
    sentinel (the single word ``--workspace``) is returned;
  * a diff touching ONLY ``docs/`` or ``*.md`` returns an EMPTY list — the
    fast tier then runs only ``cargo fmt`` and this script's unit tests;
  * an unknown folder (no workspace package manifest in any ancestor)
    falls back to the ``WORKSPACE`` sentinel;
  * a change under ``crates/client/shamir-client-ts/`` affects NO Rust crate (no
    workspace fallback) but sets the ``ts`` flag.

The full CI tier calls the same selector with ``--format github-output``,
which additionally prints four boolean flags:

  * ``engine``   — the workspace or the crate set includes ``shamir-engine``;
  * ``ts``       — the workspace, a ``crates/client/shamir-client-ts/`` change, or
    the crate set includes ``shamir-query-types``;
  * ``node``     — the workspace or the crate set includes ``shamir-client``
    or ``shamir-client-node``;
  * ``docs_only`` — a bounded diff that selected no crates and touched no
    ``crates/client/shamir-client-ts/`` file (false in workspace mode).

Event rule: ``pull_request`` diffs ``origin/master...HEAD``;
``push`` / ``workflow_dispatch`` / ``schedule`` always select the whole
workspace (no git diff is run).

Exit code is non-zero (with a message on stderr) on any failure; a partial
list is never printed. Python 3 standard library only.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import PurePosixPath

WORKSPACE = "--workspace"

CLIENT_TS_DIR = "crates/client/shamir-client-ts"

WORKSPACE_EVENTS = frozenset(("push", "workflow_dispatch", "schedule"))

_GLOBAL_PREFIXES = (".cargo/", ".config/", "scripts/", ".github/")
_GLOBAL_NAMES = ("Cargo.lock", "Cargo.toml", "deny.toml")

_ARGS_LINE_RE = re.compile(r"^(-p \S+)( -p \S+)*$")


def _norm(path: str) -> str:
    return path.replace("\\", "/")


def load_metadata(metadata_file: str | None) -> dict:
    """Load `cargo metadata --format-version 1 --no-deps` JSON."""
    if metadata_file is not None:
        with open(metadata_file, "r", encoding="utf-8") as f:
            return json.load(f)
    raw = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        capture_output=True,
        text=True,
        check=False,
    )
    if raw.returncode != 0:
        raise RuntimeError(
            "cargo metadata failed (exit %d): %s" % (raw.returncode, raw.stderr.strip())
        )
    return json.loads(raw.stdout)


def changed_files(base: str, repo_dir: str | None = None) -> list[str]:
    """List files changed between `base` and HEAD (merge-base three-dot diff)."""
    raw = subprocess.run(
        ["git", "diff", "--name-only", "%s...HEAD" % base],
        capture_output=True,
        text=True,
        check=False,
        cwd=repo_dir,
    )
    if raw.returncode != 0:
        raise RuntimeError(
            "git diff failed (exit %d): %s" % (raw.returncode, raw.stderr.strip())
        )
    return [line for line in raw.stdout.splitlines() if line.strip()]


def event_forces_workspace(event: str) -> bool:
    """True when the event type always selects the whole workspace."""
    return event in WORKSPACE_EVENTS


def _is_global(path: str) -> bool:
    p = _norm(path)
    if p in _GLOBAL_NAMES:
        return True
    if p.startswith("rust-toolchain"):
        return True
    return any(p.startswith(prefix) for prefix in _GLOBAL_PREFIXES)


def _repo_relative(path: str, root: str) -> str:
    p = _norm(path)
    r = _norm(root)
    if p == r:
        return "."
    if p.startswith(r + "/"):
        return p[len(r) + 1:]
    if p.startswith("./"):
        return p[2:]
    return p


def _manifest_dirs(metadata: dict, root: str) -> dict[str, str]:
    """Map repo-relative manifest PARENT dir -> workspace package name."""
    dirs: dict[str, str] = {}
    for pkg in metadata.get("packages", []):
        name = pkg.get("name")
        manifest = pkg.get("manifest_path")
        if not name or not manifest:
            continue
        rel = _repo_relative(_norm(manifest), root)
        parent = PurePosixPath(rel).parent
        parent_str = parent.as_posix()
        if parent_str == ".":
            # The repo root manifest itself is never a mapping target.
            continue
        dirs[parent_str] = name
    return dirs


def _nearest_package(path: str, dirs: dict[str, str]) -> str | None:
    """Package whose manifest dir is the deepest ancestor of `path`."""
    p = PurePosixPath(_norm(path))
    for ancestor in p.parents:
        s = ancestor.as_posix()
        if s == ".":
            break
        name = dirs.get(s)
        if name is not None:
            return name
    return None


def _reverse_deps(metadata: dict, workspace: set[str]) -> dict[str, set[str]]:
    """dep -> set of workspace packages depending on it (any dep kind)."""
    rev: dict[str, set[str]] = {}
    for pkg in metadata.get("packages", []):
        name = pkg["name"]
        if name not in workspace:
            continue
        for dep in pkg.get("dependencies", []):
            dep_name = dep.get("name")
            if dep_name in workspace:
                rev.setdefault(dep_name, set()).add(name)
    return rev


def select_crates(
    changed: list[str], metadata: dict, root: str = "/repo"
) -> list[str] | str:
    """Pure selection core: changed files + metadata -> WORKSPACE or crate names.

    Returns the ``WORKSPACE`` sentinel (``"--workspace"``) when the diff
    cannot be bounded to specific crates; a sorted list of crate names
    otherwise (``[]`` = docs-only diff, no crates affected).
    """
    workspace = {pkg["name"] for pkg in metadata.get("packages", [])}
    dirs = _manifest_dirs(metadata, root)

    marked: set[str] = set()
    fallback_workspace = False

    for path in changed:
        if _is_global(path):
            # Global build config cannot be bounded to specific crates.
            # Checked BEFORE the markdown/docs exemption so files like
            # scripts/example.md or .github/x.md still fall back.
            fallback_workspace = True
        elif path == CLIENT_TS_DIR or _norm(path).startswith(CLIENT_TS_DIR + "/"):
            # The TypeScript client sets the ts flag only; no Rust crate.
            continue
        else:
            name = _nearest_package(path, dirs)
            if name is not None:
                # A file inside a package (markdown included) marks it.
                marked.add(name)
            elif path.endswith(".md") or _norm(path).startswith("docs/"):
                continue
            else:
                # Unknown folder — cannot bound the blast radius.
                fallback_workspace = True

    if fallback_workspace:
        return WORKSPACE
    if not marked:
        return []

    rev = _reverse_deps(metadata, workspace)
    closure = set(marked)
    frontier = list(marked)
    while frontier:
        current = frontier.pop()
        for dependent in rev.get(current, ()):
            if dependent not in closure:
                closure.add(dependent)
                frontier.append(dependent)

    return sorted(closure)


def format_args(crates: list[str] | str) -> str:
    """Render the selection as one CLI line: ``-p a -p b`` / ``--workspace`` / ````."""
    if crates == WORKSPACE:
        return WORKSPACE
    if not crates:
        return ""
    return " ".join("-p %s" % name for name in sorted(crates))


def compute_flags(crates: list[str] | str, changed: list[str]) -> dict[str, bool]:
    """Derive the boolean CI flags from the selection and the changed files."""
    workspace_mode = crates == WORKSPACE
    names = set() if workspace_mode else set(crates)
    ts_changed = any(
        p == CLIENT_TS_DIR or _norm(p).startswith(CLIENT_TS_DIR + "/") for p in changed
    )
    return {
        "engine": workspace_mode or "shamir-engine" in names,
        "ts": workspace_mode or ts_changed or "shamir-query-types" in names,
        "node": workspace_mode or bool(names & {"shamir-client", "shamir-client-node"}),
        "docs_only": (not workspace_mode) and (not names) and (not ts_changed),
    }


def format_github_output(args_line: str, flags: dict[str, bool]) -> str:
    """Render the five `key=value` lines consumed via GITHUB_OUTPUT."""
    keys = ("engine", "ts", "node", "docs_only")
    lines = ["args=%s" % args_line]
    lines.extend("%s=%s" % (k, "true" if flags[k] else "false") for k in keys)
    return "\n".join(lines)


def validate_args(line: str) -> None:
    """Raise unless `line` is empty, --workspace, or a well-formed -p list."""
    if line == "" or line == WORKSPACE:
        return
    if _ARGS_LINE_RE.match(line):
        return
    raise RuntimeError("malformed args line: %r" % line)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--base",
        default="origin/master",
        help="base git ref for the three-dot diff (default: origin/master)",
    )
    parser.add_argument(
        "--metadata",
        default=None,
        help="read a pre-captured `cargo metadata` JSON file instead of invoking cargo",
    )
    parser.add_argument(
        "--event",
        default="pull_request",
        choices=("pull_request", "push", "workflow_dispatch", "schedule"),
        help="triggering workflow event (default: pull_request)",
    )
    parser.add_argument(
        "--format",
        default="args",
        choices=("args", "github-output"),
        help="output format (default: args)",
    )
    args = parser.parse_args(argv)

    try:
        metadata = load_metadata(args.metadata)
        if event_forces_workspace(args.event):
            changed: list[str] = []
            crates: list[str] | str = WORKSPACE
        else:
            changed = changed_files(args.base)
            crates = select_crates(changed, metadata, root=_norm(os.getcwd()))
        args_line = format_args(crates)
        validate_args(args_line)
        flags = compute_flags(crates, changed)
    except (OSError, RuntimeError, ValueError, KeyError) as exc:
        print("affected_crates.py: %s" % exc, file=sys.stderr)
        return 1

    if args.format == "github-output":
        print(format_github_output(args_line, flags))
    else:
        print(args_line)
    return 0


if __name__ == "__main__":
    sys.exit(main())
