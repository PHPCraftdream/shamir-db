#!/usr/bin/env python3
"""Native-code gate: the built dependency graph may compile C only for allowlisted crates.

Why
    Owner ruling: the workspace goes Rust-only where that comes easily;
    where no real Rust alternative exists the C dependency stays and is
    documented (``docs/native-code.md``).  This gate makes that state
    enforceable: a NEW crate that compiles C/C++/assembly (they depend on
    the build tools ``cc`` / ``cmake`` / ``bindgen`` / ``pkg-config``)
    fails CI until an owner decision adds it to the allowlist AND the
    document — in the same PR.  A stale allowlist entry (the crate no
    longer uses the tool) also fails, so the allowlist and the document
    stay truthful.

How
    For every build tool T the script runs::

        cargo tree --locked --workspace --all-features --target all \
            -e normal,build,dev -i T --depth 1 --prefix none --format "{p}"

    ``-i`` inverts the graph, so the lines at depth 1 are the DIRECT
    dependents of T — exactly the crates whose build runs T.  A tool that
    is not in the graph at all (cargo reports it does not match any
    package) is an empty dependent set, not an error.  The hard deny list
    (``ring``, ``libmimalloc-sys``, ``zstd-sys``,
    ``wasmtime-internal-jit-debug``, ``openssl-sys``) is asserted with the
    same ``cargo tree -i``: the crate being PRESENT in the built graph is
    a failure, even though it may still sit in ``Cargo.lock`` unused.

Exit codes: 0 clean, 1 policy violation (unknown dependent, stale
allowlist entry, or a deny-listed crate in the graph), 2 tooling error
(cargo failed for any other reason).
Python 3 standard library only; ``python3 -I`` compatible.
See ``docs/native-code.md`` for the per-entry rationale.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys

BUILD_TOOLS = ("cc", "cmake", "bindgen", "pkg-config")

# Direct dependents per build tool. Every entry is documented in
# docs/native-code.md; add an entry ONLY together with a document entry,
# and only after an owner decision.
ALLOWLIST: dict[str, frozenset[str]] = {
    "cc": frozenset(
        {
            "aws-lc-sys",
            "cmake",
            "blake3",
            "generator",
            "iana-time-zone-haiku",
            "wasmtime",
            "wasmtime-internal-fiber",
        }
    ),
    "cmake": frozenset({"aws-lc-sys"}),
    "pkg-config": frozenset({"aws-lc-sys"}),
    "bindgen": frozenset({"libproc"}),
}

# Crates that must NEVER be in the built graph again (they may remain in
# Cargo.lock unused; the graph is what is built).
DENY_LIST = (
    "ring",
    "libmimalloc-sys",
    "zstd-sys",
    "wasmtime-internal-jit-debug",
    "openssl-sys",
)

TREE_TIMEOUT_SECONDS = 300

# e.g.  aws-lc-sys v1.12.4 (*)   |  libz-sys v1.1.20
#       wasmtime v36.0.2 (registry+https://github.com/rust-lang/crates.io-index)
PKG_LINE_RE = re.compile(r"^(\S+)\s+v\S")

# cargo tree -i <absent-package> fails with exactly this wording.
ABSENT_MARKERS = ("did not match any packages",)

DEFAULT_RUNNER = subprocess.run  # replaced by tests


def run_cargo_tree(package: str, runner=DEFAULT_RUNNER) -> tuple[int, str, str]:
    """Run the inverted ``cargo tree`` query for ``package``.

    Returns ``(returncode, stdout, stderr)``; the single place where cargo
    is invoked, so tests can inject canned output via ``runner``.
    """
    cmd = [
        "cargo",
        "tree",
        "--locked",
        "--workspace",
        "--all-features",
        "--target",
        "all",
        "-e",
        "normal,build,dev",
        "-i",
        package,
        "--depth",
        "1",
        "--prefix",
        "none",
        "--format",
        "{p}",
    ]
    try:
        proc = runner(
            cmd,
            capture_output=True,
            text=True,
            timeout=TREE_TIMEOUT_SECONDS,
        )
    except subprocess.TimeoutExpired:
        return 124, "", "cargo tree timed out after %ds" % TREE_TIMEOUT_SECONDS
    except OSError as exc:
        return 127, "", "cannot run cargo: %s" % exc
    return proc.returncode, proc.stdout or "", proc.stderr or ""


def parse_packages(tree_text: str) -> set[str]:
    """Bare crate names of every package line in ``--format "{p}"`` output.

    Lines are ``<name> v<version>[ (<source>)] [ (*)]``; the first line is
    the queried package itself, the rest are its direct dependents.
    Unparsable lines are ignored (cargo progress noise).
    """
    names: set[str] = set()
    for line in tree_text.splitlines():
        match = PKG_LINE_RE.match(line.strip())
        if match:
            names.add(match.group(1))
    return names


def query_graph(package: str, runner) -> tuple[int, set[str], str]:
    """Query ``package`` in the built graph; return (exit_code, names, error).

    ``names`` holds the queried package (when present) and its direct
    dependents; empty means the package is not in the graph.  exit_code 2
    is a cargo/tooling failure (the error string says why).
    """
    rc, out, err = run_cargo_tree(package, runner)
    if rc == 0:
        return 0, parse_packages(out), ""
    if any(marker in err for marker in ABSENT_MARKERS):
        return 0, set(), ""
    excerpt = " | ".join(err.strip().splitlines()[-3:]) or "(no stderr)"
    return 2, set(), "cargo tree -i %s failed (exit %d): %s" % (package, rc, excerpt)


def main(argv: list[str] | None = None, *, runner=DEFAULT_RUNNER) -> int:
    """Entry point; tests inject canned cargo output through ``runner``."""
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.parse_args(argv)

    violations: list[str] = []
    tooling_errors: list[str] = []

    for tool in BUILD_TOOLS:
        code, names, error = query_graph(tool, runner)
        if code == 2:
            tooling_errors.append(error)
            continue
        dependents = names - {tool}
        for name in sorted(dependents - ALLOWLIST[tool]):
            violations.append(
                "new dependent of build tool %r: %r is not in the allowlist"
                " (see docs/native-code.md; adding it needs an owner decision"
                " + allowlist and doc entry in the same PR)" % (tool, name)
            )
        for name in sorted(ALLOWLIST[tool] - dependents):
            violations.append(
                "stale allowlist entry: %r no longer depends on build tool %r"
                " — remove it from the allowlist and docs/native-code.md" % (name, tool)
            )

    for crate in DENY_LIST:
        code, names, error = query_graph(crate, runner)
        if code == 2:
            tooling_errors.append(error)
            continue
        if crate in names:
            violations.append(
                "deny-listed native crate %r is in the built graph" % crate
            )

    if tooling_errors:
        for line in tooling_errors:
            print("native_code_check.py: tooling error: %s" % line, file=sys.stderr)
        return 2
    if violations:
        for line in violations:
            print("native code gate FAILED: %s" % line, file=sys.stderr)
        return 1
    print("native code gate PASSED: build-tool dependents match the allowlist;")
    print("no deny-listed native crate in the graph (see docs/native-code.md)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
