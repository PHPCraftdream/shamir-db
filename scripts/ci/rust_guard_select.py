#!/usr/bin/env python3
"""Select rust-guard campaigns or filter packages against cargo metadata.

CLI:
    rust_guard_select.py --stages stages.toml --metadata metadata.json [--root .]
    rust_guard_select.py --present-in metadata.json --packages a,b

Selection reads changed paths from stdin. A campaign is evaluated only when
its own delta or discovery_delta is touched AND a campaign package is touched;
landed campaigns are inert until removed by the next campaign. Output columns:
    <name>\t<delta>\t<packages>\t<dirs>\t<discovery_delta>
Packages and directories are comma-separated. The present-in mode prints the
requested packages found in metadata, in input order. Exit 2 on invalid input,
missing arguments, unreadable files, or an empty present-in result.
Python standard library only; python3 -I compatible.
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


def resolve_root(root: str) -> str:
    """Absolute, separator-normalised repo root from a possibly relative one.

    The workflow passes ``--root .`` (relative), while ``cargo metadata``
    manifest paths are ABSOLUTE.  Resolving once here makes ``strip_root``
    a pure string operation on comparable absolute posix paths; with the
    literal ``"."`` the ``PurePosixPath(".")`` prefix never matched and no
    path was ever stripped (the round-2 regression this fixes).
    """
    return norm(str(Path(root).resolve()))


def strip_root(path: str, root: str) -> str:
    """Repo-relative posix form of ``path`` given repo root ``root``.

    Both inputs may use any separator mix; ``root`` should already be
    RESOLVED (absolute) via :func:`resolve_root` — ``strip_root`` itself
    stays pure and does no filesystem access.
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


def touches_own_delta(campaign: dict, changed: list[str]) -> bool:
    """Whether the diff touches either campaign delta, independent of separators."""
    paths = {norm(path) for path in changed}
    return any(norm(campaign[key]) in paths for key in ("delta", "discovery_delta"))


def select(
    campaigns: list[dict],
    pkg_dirs: dict[str, str],
    changed: list[str],
) -> list[tuple[str, str, list[str], list[str]]]:
    """Pure selection: campaigns whose existing packages' dirs are touched.

    Returns ``(name, delta, [package names], [package directories])`` tuples.
    A changed file (repo-relative posix) matches a campaign iff it starts
    with ``<pkg-dir>/`` for one of the campaign's packages present in
    ``pkg_dirs``.
    """
    changed_norm = [norm(f) for f in changed]
    out: list[tuple[str, str, list[str], list[str]]] = []
    for c in campaigns:
        known = [name for name in c["packages"] if name in pkg_dirs]
        dirs = [pkg_dirs[name] for name in known]
        touched = any(
            f.startswith(d + "/") for f in changed_norm for d in dirs
        )
        if touched:
            out.append((c["name"], c["delta"], known, dirs))
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
        for key in ("name", "delta", "discovery_delta"):
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
    parser.add_argument("--stages", help="path to stages.toml")
    parser.add_argument("--metadata", help="captured cargo metadata JSON")
    parser.add_argument("--present-in", help="filter packages against this metadata JSON")
    parser.add_argument("--packages", help="comma-separated package names for --present-in")
    parser.add_argument("--root", default=".", help="repo root to strip from manifest paths")
    args = parser.parse_args(argv)
    if args.present_in is not None:
        if not args.packages or args.stages is not None or args.metadata is not None:
            parser.error("--present-in requires --packages and cannot be combined with selection")
    elif not args.stages or not args.metadata or args.packages is not None:
        parser.error("selection requires --stages and --metadata")

    try:
        if args.present_in is not None:
            if metadata_text is None:
                metadata_text = Path(args.present_in).read_text(encoding="utf-8")
            meta = json.loads(metadata_text)
            if not isinstance(meta, dict) or not isinstance(meta.get("packages"), list):
                raise ValueError("metadata must contain a packages list")
            names = {pkg["name"] for pkg in meta["packages"]}
            present = [name for name in args.packages.split(",") if name in names]
            if not present:
                raise ValueError("none of the requested packages are present")
            print(",".join(present))
            return 0
        if stages_text is None:
            stages_text = Path(args.stages).read_bytes()
        campaigns = load_campaigns(stages_text)
        if metadata_text is None:
            metadata_text = Path(args.metadata).read_text(encoding="utf-8")
        pkg_dirs = load_pkg_dirs(metadata_text, resolve_root(args.root))
        if changed is None:
            changed = sys.stdin.read().splitlines()
        active = [c for c in campaigns if touches_own_delta(c, changed)]
        for name, delta, pkgs, dirs in select(active, pkg_dirs, changed):
            campaign = next(c for c in active if c["name"] == name and c["delta"] == delta)
            print("%s\t%s\t%s\t%s\t%s" % (
                name, delta, ",".join(pkgs), ",".join(dirs), campaign["discovery_delta"]
            ))
    except (OSError, ValueError, KeyError, TypeError, tomllib.TOMLDecodeError) as exc:
        print("rust_guard_select.py: %s" % exc, file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
