#!/usr/bin/env python3
"""Affected-crates selector for the CI fast tier.

Reads the files changed between a base ref and HEAD (``git diff --name-only
<base>...HEAD``) and prints, on ONE line, the sorted ``-p <crate>`` arguments
limiting cargo / ./scripts/test.sh to the crates a change can affect:

  * a changed file under ``crates/<dir>/`` marks the workspace package whose
    manifest lives in that folder;
  * every workspace package that depends on a marked package (directly or
    transitively, across normal / dev / build deps among workspace members)
    is added too;
  * if any changed path is ``Cargo.lock``, the root ``Cargo.toml``,
    ``rust-toolchain*``, ``deny.toml``, anything under ``.cargo/`` /
    ``.config/`` / ``scripts/`` / ``.github/``, or any file outside
    ``crates/`` / ``docs/`` that is not ``*.md``, the ``WORKSPACE``
    sentinel (the single word ``--workspace``) is returned;
  * a diff touching ONLY ``docs/`` or ``*.md`` returns an EMPTY list — the
    fast tier then runs only ``cargo fmt`` and this script's unit tests;
  * a ``crates/<dir>/`` folder that is not a workspace package manifest dir
    falls back to the ``WORKSPACE`` sentinel.

Exit code is non-zero (with a message on stderr) on any failure; a partial
list is never printed. Python 3 standard library only.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import PurePosixPath

WORKSPACE = "--workspace"

_GLOBAL_PREFIXES = (".cargo/", ".config/", "scripts/", ".github/")
_GLOBAL_NAMES = ("Cargo.lock", "Cargo.toml", "deny.toml")


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


def _is_global(path: str) -> bool:
    p = _norm(path)
    if p in _GLOBAL_NAMES:
        return True
    if p.startswith("rust-toolchain"):
        return True
    return any(p.startswith(prefix) for prefix in _GLOBAL_PREFIXES)


def _crate_dir(path: str) -> str | None:
    """Second path segment of a `crates/<dir>/...` path, else None."""
    p = PurePosixPath(_norm(path))
    parts = p.parts
    if len(parts) >= 2 and parts[0] == "crates":
        return parts[1]
    return None


def _manifest_dirs(metadata: dict) -> dict[str, str]:
    """Map `crates/<dir>` folder name -> workspace package name."""
    dirs: dict[str, str] = {}
    for pkg in metadata.get("packages", []):
        manifest = _norm(pkg.get("manifest_path", ""))
        parent = PurePosixPath(manifest).parent
        if len(parent.parts) >= 2 and parent.parts[-2] == "crates":
            dirs[parent.parts[-1]] = pkg["name"]
    return dirs


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


def select_crates(changed: list[str], metadata: dict) -> list[str] | str:
    """Pure selection core: changed files + metadata -> WORKSPACE or crate names.

    Returns the ``WORKSPACE`` sentinel (``"--workspace"``) when the diff
    cannot be bounded to specific crates; a sorted list of crate names
    otherwise (``[]`` = docs-only diff, no crates affected).
    """
    workspace = {pkg["name"] for pkg in metadata.get("packages", [])}
    dirs = _manifest_dirs(metadata)

    marked: set[str] = set()
    fallback_workspace = False

    for path in changed:
        crate_dir = _crate_dir(path)
        if crate_dir is not None:
            name = dirs.get(crate_dir)
            if name is None:
                # Unknown folder under crates/ — cannot bound the blast radius.
                fallback_workspace = True
            else:
                marked.add(name)
        elif _is_global(path):
            # Global build config cannot be bounded to specific crates.
            # Checked BEFORE the markdown/docs exemption so files like
            # scripts/example.md or .github/x.md still fall back.
            fallback_workspace = True
        elif path.endswith(".md") or _norm(path).startswith("docs/"):
            continue
        else:
            # Anything outside crates/ and docs/ can affect the build.
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
    args = parser.parse_args(argv)

    try:
        metadata = load_metadata(args.metadata)
        files = changed_files(args.base)
        crates = select_crates(files, metadata)
    except (OSError, RuntimeError, ValueError, KeyError) as exc:
        print("affected_crates.py: %s" % exc, file=sys.stderr)
        return 1

    print(format_args(crates))
    return 0


if __name__ == "__main__":
    sys.exit(main())
