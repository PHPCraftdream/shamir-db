#!/usr/bin/env python3
"""Select active rust-guard conservation campaigns touched by a PR diff.

Why
    The round-1 inline CI step built package dirs from ABSOLUTE
    ``cargo metadata`` manifest paths but compared them against
    REPO-RELATIVE ``git diff --name-only`` paths, so
    ``f.startswith(d + "/")`` never matched and every campaign was
    silently skipped.  This script centralises the selection logic,
    normalises ALL separators to ``/`` and strips the repo root from the
    metadata capture (``--root``, default ``.``) so both sides are
    repo-relative posix paths.

CLI
    ``rust_guard_select.py --stages .rust-guard/stages.toml
    --metadata metadata.json [--root .]``

    Changed files are read from stdin, one per line (the natural output
    of ``git diff --name-only``).  For every campaign in ``stages.toml``
    whose packages (restricted to those known to the metadata) are
    touched by the diff, one line is printed::

        <name>\t<delta>\t<comma-separated package names>

    Nothing is printed (exit 0) when nothing matches.

Exit codes: 0 selection done (possibly no output), 2 usage/parse error
(malformed stages TOML or campaign entry, unreadable metadata file, bad
JSON, missing arguments) — the CI step fails closed on 2.
Python 3 standard library only; ``python3 -I`` compatible.
"""

from __future__ import annotations

import argparse
import json
import sys
import tomllib
from pathlib import Path, PurePosixPath

sys.path.insert(
    0,
    str(Path(__file__).resolve().parent),
)


def norm(rel: str) -> str:
    """Normalise separators to ``/`` (Windows captures use ``\\``)."""
    return rel.replace("\\", "/")


def strip_root(path: str, root: str) -> str:
    """Repo-relative posix form of ``path`` given repo root ``root``.

    Both inputs may use any separator mix; ``root`` may be ``.``.
    """
    p = PurePosixPath(norm(path))
    r = PurePosixPath(norm(root))
    try:
        return p.relative_to(r).as_posix()
    except ValueError:
        # Root not a prefix (e.g. metadata captured elsewhere): keep the
        # normalised path; matching against repo-relative diffs will then
        # simply not match, which is the fail-safe direction.
        return p.as_posix()


def load_pkg_dirs(metadata_text: str, root: str) -> dict[str, str]:
    """package name -> repo-relative source dir, from cargo metadata JSON."""
    meta = json.loads(metadata_text)
    dirs: dict[str, str] = {}
    for pkg in meta.get("packages", []):
        manifest = pkg.get("manifest_path")
        if not manifest:
            continue
        manifest_rel = strip_root(manifest, root)
        dirs.setdefault(pkg["name"], norm(str(Path(manifest_rel).parent)))
    return dirs


def select(
    campaigns: list[dict],
    pkg_dirs: dict[str, str],
    changed: list[str],
) -> list[tuple[str, str, list[str]]]:
    """Pure selection: campaigns whose existing packages' dirs are touched.

    Returns ``(name, delta, [package names known to metadata])`` tuples.
    A changed file (repo-relative posix) matches a campaign iff it starts
    with ``<pkg-dir>/`` for one of the campaign's packages present in
    ``pkg_dirs``.
    """
    changed_norm = [norm(f) for f in changed]
    out: list[tuple[str, str, list[str]]] = []
    for c in campaigns:
        known = [name for name in c["packages"] if name in pkg_dirs]
        dirs = [pkg_dirs[name] for name in known]
        touched = any(
            f.startswith(d + "/") for f in changed_norm for d in dirs
        )
        if touched:
            out.append((c["name"], c["delta"], known))
    return out


def load_campaigns(stages_text: bytes) -> list[dict]:
    """Parse and validate the stages TOML (fail closed on malformed input)."""
    data = tomllib.loads(stages_text.decode("utf-8"))
    campaigns = data.get("campaigns", [])
    if not isinstance(campaigns, list):
        raise ValueError("campaigns must be a list")
    for c in campaigns:
        if not isinstance(c, dict):
            raise ValueError("campaign entries must be tables")
        for key in ("name", "delta"):
            if not isinstance(c.get(key), str):
                raise ValueError("campaign missing string %r" % key)
        pkgs = c.get("packages")
        if (
            not isinstance(pkgs, list)
            or not pkgs
            or not all(isinstance(p, str) for p in pkgs)
        ):
            raise ValueError("campaign %r needs non-empty packages list" % c.get("name"))
    return campaigns


def main(
    argv: list[str] | None = None,
    *,
    stages_text: bytes | None = None,
    metadata_text: str | None = None,
    changed: list[str] | None = None,
) -> int:
    """Entry point; keyword args inject file content / stdin in tests."""
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--stages", required=True, help="path to stages.toml")
    parser.add_argument("--metadata", required=True, help="captured cargo metadata JSON")
    parser.add_argument("--root", default=".", help="repo root to strip from manifest paths")
    args = parser.parse_args(argv)

    try:
        if stages_text is None:
            stages_text = Path(args.stages).read_bytes()
        campaigns = load_campaigns(stages_text)
        if metadata_text is None:
            metadata_text = Path(args.metadata).read_text(encoding="utf-8")
        pkg_dirs = load_pkg_dirs(metadata_text, args.root)
        if changed is None:
            changed = sys.stdin.read().splitlines()
        for name, delta, pkgs in select(campaigns, pkg_dirs, changed):
            print("%s\t%s\t%s" % (name, delta, ",".join(pkgs)))
    except (OSError, ValueError, KeyError, tomllib.TOMLDecodeError) as exc:
        print("rust_guard_select.py: %s" % exc, file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
