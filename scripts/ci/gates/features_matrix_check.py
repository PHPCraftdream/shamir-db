#!/usr/bin/env python3
"""Assert the enabled feature set of shamir-engine matches a matrix cell.

Why
    The ``engine-features`` CI job compiles one isolated cell per feature
    combination, but ``cargo check`` passing does not PROVE which features
    were enabled ("no defaults" could silently hide a default that crept
    back in).  This script recomputes the enabled feature set the same way
    cargo resolves it — pure ``tomllib`` parsing of the manifest's
    ``[features]`` table, no ``cargo`` call, fully deterministic — and
    fails when it differs from the cell's expected set.

CLI
    ``features_matrix_check.py --expected <cell-id>
    [--manifest-path crates/shamir-engine/Cargo.toml] [--flags "<flags>"]``

    ``--flags`` takes the same feature flags the CI cell passes to cargo
    (``--no-default-features``, ``--features a,b``); the resolution is
    derived from them, so the CI job and this assertion cannot drift.

Cells and their expected sets (the ``default`` name itself is the
activation label, not a resolved feature; dep-qualified references like
``shamir-storage/fjall`` are not engine features):

    no-default      {}
    fjall           {fjall}
    test-util       {test-util}
    fjall-test-util {fjall, test-util}
    default-plus    {all-backends, fjall, test-util, fts_indexed_long}
    loom            {all-backends, fjall, loom}

Exit codes: 0 resolved set matches, 1 mismatch (resolved vs expected
printed to stderr), 2 usage/parse error (unknown cell id, unreadable or
malformed manifest, missing arguments) — the CI step fails closed on 2.
Python 3 standard library only; ``python3 -I`` compatible.
"""

from __future__ import annotations

import argparse
import sys
import tomllib
from pathlib import Path

PACKAGE_NAME = "shamir-engine"

EXPECTED: dict[str, set[str]] = {
    "no-default": set(),
    "fjall": {"fjall"},
    "test-util": {"test-util"},
    "fjall-test-util": {"fjall", "test-util"},
    "default-plus": {"all-backends", "fjall", "test-util", "fts_indexed_long"},
    "loom": {"all-backends", "fjall", "loom"},
}


def parse_flags(flags: str) -> tuple[bool, list[str]]:
    """Split CI feature flags into (no_default, [requested feature lists]).

    Understands ``--no-default-features`` and ``--features <list>`` /
    ``--features=<list>`` (lists comma-separated, possibly repeated, like
    cargo).  Any other token is ignored — this only resolves features.
    """
    no_default = False
    requested: list[str] = []
    tokens = flags.split()
    i = 0
    while i < len(tokens):
        tok = tokens[i]
        if tok == "--no-default-features":
            no_default = True
        elif tok == "--features" or tok.startswith("--features="):
            if tok == "--features":
                i += 1
                if i >= len(tokens):
                    raise ValueError("--features given without a value")
                value = tokens[i]
            else:
                value = tok.split("=", 1)[1]
            requested.extend(f.strip() for f in value.split(",") if f.strip())
        i += 1
    return no_default, requested


def resolve_features(manifest: dict, no_default: bool, requested: list[str]) -> set[str]:
    """Compute the engine's enabled feature set like cargo resolves flags.

    Starts from the ``default`` list (unless ``--no-default-features``),
    unions the requested features, then transitively expands every enabled
    feature's own list from ``[features]``.  Dep-qualified references
    (``some-crate/some-feature``) are forwarded to dependencies, not
    engine features, and are excluded from the result.
    """
    table = manifest.get("features")
    if not isinstance(table, dict):
        raise ValueError("manifest has no [features] table")
    local = {name for name, deps in table.items() if isinstance(deps, list)}

    def expand(name: str, seen: set[str]) -> set[str]:
        """Names activated by `name`, transitively, cycles tolerated."""
        if name in seen or "/" in name or name not in local:
            return set()
        seen.add(name)
        out = {name}
        for dep in table[name]:
            out |= expand(dep, seen)
        return out

    enabled: set[str] = set()
    if not no_default:
        enabled |= expand("default", set())
    for feat in requested:
        enabled |= expand(feat, set())
    # `default` is the activation label, not a resolved feature — cargo's
    # own feature lists show the expanded set without it.
    enabled.discard("default")
    return enabled


def main(
    argv: list[str] | None = None,
    *,
    manifest_text: bytes | None = None,
) -> int:
    """Entry point; ``manifest_text`` injects file content in tests."""
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--expected",
        required=True,
        metavar="CELL-ID",
        help="matrix cell id to assert (one of: %s)" % ", ".join(sorted(EXPECTED)),
    )
    parser.add_argument(
        "--manifest-path",
        default="crates/shamir-engine/Cargo.toml",
        help="path to the engine manifest",
    )
    parser.add_argument(
        "--flags",
        default="",
        help="the cell's cargo feature flags (e.g. '--no-default-features --features fjall')",
    )
    args = parser.parse_args(argv)

    try:
        if args.expected not in EXPECTED:
            raise ValueError(
                "unknown cell id %r (expected one of: %s)"
                % (args.expected, ", ".join(sorted(EXPECTED)))
            )
        no_default, requested = parse_flags(args.flags)
        if manifest_text is None:
            manifest_text = Path(args.manifest_path).read_bytes()
        manifest = tomllib.loads(manifest_text.decode("utf-8"))
        name = manifest.get("package", {}).get("name")
        if name != PACKAGE_NAME:
            raise ValueError(
                "manifest %s is package %r, expected %r"
                % (args.manifest_path, name, PACKAGE_NAME)
            )
        resolved = resolve_features(manifest, no_default, requested)
    except (
        OSError,
        ValueError,
        KeyError,
        AttributeError,
        tomllib.TOMLDecodeError,
        UnicodeDecodeError,
    ) as exc:
        print("features_matrix_check.py: %s" % exc, file=sys.stderr)
        return 2

    expected = EXPECTED[args.expected]
    if resolved != expected:
        print(
            "features_matrix_check.py: feature resolution mismatch for cell "
            "%r (flags: %s)\n  resolved: %s\n  expected: %s"
            % (
                args.expected,
                args.flags or "(none)",
                sorted(resolved),
                sorted(expected),
            ),
            file=sys.stderr,
        )
        return 1
    print(
        "features_matrix_check.py: cell %r OK (resolved %s)"
        % (args.expected, sorted(resolved))
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
