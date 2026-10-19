#!/usr/bin/env python3
"""Assert the enabled feature set of shamir-engine matches a matrix cell.

Why
    The ``engine-features`` CI job compiles one isolated cell per feature
    combination, but ``cargo check`` passing does not PROVE which features
    were enabled.  Static manifest parsing re-implements cargo's feature
    resolver and can drift from it (dep-qualified refs, weak deps,
    default activation).  This script instead asks cargo itself:
    ``cargo metadata --format-version 1 <cell flags>`` and reads the
    engine's node in ``resolve.nodes[]`` whose ``features`` array is the
    ACTUALLY enabled set (including the ``default`` pseudo-entry where
    defaults are on).  No drift is possible: the same resolver, the same
    invocation the CI cell uses.

CLI
    ``features_matrix_check.py --expected <cell-id>
    [--manifest-path crates/shamir-engine/Cargo.toml] [--flags "<flags>"]``

    ``--flags`` takes the same feature flags the CI cell passes to cargo
    (``--no-default-features``, ``--features a,b``); they are forwarded
    verbatim to ``cargo metadata``.

Cells and their expected sets (``default`` INCLUDED — cargo's resolved
``features`` list names it explicitly when defaults are on):

    no-default      {}
    fjall           {fjall}
    test-util       {test-util}
    fjall-test-util {fjall, test-util}
    default-plus    {default, all-backends, fjall, test-util, fts_indexed_long}
    loom            {default, all-backends, fjall, loom}

Exit codes: 0 resolved set matches, 1 mismatch (resolved vs expected
printed to stderr), 2 usage/cargo failure (unknown cell id, missing
``resolve`` graph, engine package absent from it, malformed metadata,
cargo subprocess failure — cargo's stderr excerpt is included) — the CI
step fails closed on 2.
Python 3 standard library only; ``python3 -I`` compatible.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys

PACKAGE_NAME = "shamir-engine"
METADATA_FLAGS = (
    "--format-version",
    "1",
    "--manifest-path",
)

EXPECTED: dict[str, set[str]] = {
    "no-default": set(),
    "fjall": {"fjall"},
    "test-util": {"test-util"},
    "fjall-test-util": {"fjall", "test-util"},
    "default-plus": {
        "default",
        "all-backends",
        "fjall",
        "test-util",
        "fts_indexed_long",
    },
    "loom": {"default", "all-backends", "fjall", "loom"},
}

DEFAULT_RUNNER = subprocess.run  # replaced by tests


def parse_flags(flags: str) -> list[str]:
    """Split the CI cell's flags into tokens ``cargo metadata`` accepts.

    ``--no-default-features`` and ``--features <list>`` /
    ``--features=<list>`` pass through verbatim (lists stay comma
    joined — cargo parses them).  Any other token is ignored: this only
    resolves features.
    """
    out: list[str] = []
    tokens = flags.split()
    i = 0
    while i < len(tokens):
        tok = tokens[i]
        if tok == "--no-default-features":
            out.append(tok)
        elif tok == "--features":
            i += 1
            if i >= len(tokens):
                raise ValueError("--features given without a value")
            out.extend([tok, tokens[i]])
        elif tok.startswith("--features="):
            out.extend(["--features", tok.split("=", 1)[1]])
        i += 1
    return out


def resolved_engine_features(
    metadata: dict, manifest_path: str
) -> set[str]:
    """Extract the engine's enabled feature set from cargo metadata JSON.

    Uses ``resolve.nodes[].features`` — the set cargo actually enabled
    (including the ``default`` pseudo-entry when defaults are on).  Fails
    closed (raises ``ValueError``) when the resolve graph is absent, the
    engine package is not in it, or the node carries no features list.
    """
    resolve = metadata.get("resolve")
    if not isinstance(resolve, dict) or not isinstance(
        resolve.get("nodes"), list
    ):
        raise ValueError(
            "cargo metadata has no resolve graph (cargo too old?)"
        )
    package_ids = {
        pkg.get("id"): pkg.get("name")
        for pkg in metadata.get("packages", [])
        if isinstance(pkg, dict)
    }
    engine_ids = {
        pid for pid, name in package_ids.items() if name == PACKAGE_NAME
    }
    if not engine_ids:
        raise ValueError(
            "package %r not found in cargo metadata for %s"
            % (PACKAGE_NAME, manifest_path)
        )
    for node in resolve["nodes"]:
        if not isinstance(node, dict):
            continue
        if node.get("id") in engine_ids:
            features = node.get("features")
            if not isinstance(features, list):
                raise ValueError(
                    "resolve node for %s has no features list" % PACKAGE_NAME
                )
            return set(features)
    raise ValueError(
        "resolve graph has no node for package %r" % PACKAGE_NAME
    )


def run_cargo_metadata(
    manifest_path: str, flags: list[str], runner
) -> str:
    """Run ``cargo metadata`` with the cell flags; return stdout text."""
    cmd = ["cargo", "metadata", *METADATA_FLAGS, manifest_path, *flags]
    try:
        proc = runner(cmd, capture_output=True, text=True)
    except OSError as exc:
        raise ValueError("cannot run cargo: %s" % exc) from exc
    if proc.returncode != 0:
        stderr = (proc.stderr or "").strip().splitlines()
        excerpt = " | ".join(stderr[-3:]) or "(no stderr)"
        raise ValueError(
            "cargo metadata failed (exit %d): %s" % (proc.returncode, excerpt)
        )
    return proc.stdout


def main(
    argv: list[str] | None = None,
    *,
    metadata_text: str | None = None,
    runner=DEFAULT_RUNNER,
) -> int:
    """Entry point; ``metadata_text`` and ``runner`` inject cargo output in tests."""
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
        flags = parse_flags(args.flags)
        if metadata_text is None:
            metadata_text = run_cargo_metadata(
                args.manifest_path, flags, runner
            )
        try:
            metadata = json.loads(metadata_text)
        except (json.JSONDecodeError, UnicodeDecodeError, TypeError) as exc:
            raise ValueError("malformed cargo metadata JSON: %s" % exc) from exc
        if not isinstance(metadata, dict):
            raise ValueError("cargo metadata JSON is not an object")
        resolved = resolved_engine_features(metadata, args.manifest_path)
    except (OSError, ValueError) as exc:
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
