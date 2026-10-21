#!/usr/bin/env python3
"""npm license gate: every package of every committed npm lockfile needs an allowed license.

Why
    ``cargo deny check`` (deny.toml) gates the licenses of the Rust graph; the
    npm dependencies of the TypeScript client (``crates/shamir-client-ts``)
    had no license gate at all.  This script reads the committed
    ``package-lock.json`` files only (lockfileVersion 2/3 ``packages`` map): no
    ``npm install``, no network, hermetic like ``cargo deny`` -- the lockfile
    already records the ``license`` of every registry package.

Policy
    * ``ALLOWED`` is the permissive, OSI-approved list of deny.toml
      ``[licenses].allow``; keep the two lists in sync.  The project itself is
      ``MIT OR Apache-2.0``, so every permissive license is compatible.
    * A package's license is an SPDX expression (``OR`` / ``AND`` /
      ``WITH`` / parentheses).  It is accepted when it can be satisfied from
      ``ALLOWED``: ``A OR B`` needs one operand, ``A AND B`` needs both,
      ``X WITH Y`` is accepted when ``X WITH Y`` or ``X`` is allowed.
    * ``EXCEPTIONS`` allows extra licenses for ONE named package (the analogue
      of deny.toml ``exceptions``); never a repo-wide allowance.
    * Fail closed: a package with no ``license`` field, a non-string field,
      an unparsable expression, ``UNLICENSED`` / ``SEE LICENSE IN ...`` or a
      license outside the list is a violation.
    * The lockfile root entry (``""``) and workspace links are skipped.
    * Tracked ``package.json`` files WITHOUT a lockfile cannot be checked
      hermetically; they are listed as notes, not failures.

Usage
    ``npm_licenses_check.py [--root DIR] [LOCKFILE ...]``  -- without
    arguments every tracked ``package-lock.json`` under ``--root`` (default
    ``.``) is checked.

Exit codes: 0 all packages allowed, 1 violations or an unreadable lockfile.
Python 3 standard library only; ``python3 -I`` compatible.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

# Keep in sync with deny.toml [licenses].allow (compared case-insensitively).
ALLOWED = (
    "MIT",
    "MIT-0",
    "Apache-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "BSD-1-Clause",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "0BSD",
    "ISC",
    "Zlib",
    "Unicode-3.0",
    "Unicode-DFS-2016",
    "Unlicense",
    "CC0-1.0",
    "BSL-1.0",
)

# Per-package extra allowances: {npm package name: (license id, ...)}.  Each
# entry needs a comment naming the package, the license and why it is fine.
EXCEPTIONS: dict[str, tuple[str, ...]] = {}

_TOKEN = re.compile(r"\(|\)|[A-Za-z0-9][A-Za-z0-9.+\-]*")


class ExpressionError(ValueError):
    """The SPDX expression cannot be parsed."""


def _allowed_set(extra: tuple[str, ...] = ()) -> frozenset[str]:
    return frozenset(x.lower() for x in ALLOWED + extra)


def satisfied(expression: str, allowed: frozenset[str]) -> bool:
    """True when the SPDX ``expression`` can be satisfied from ``allowed``."""
    tokens = _TOKEN.findall(expression)
    if not tokens or "".join(tokens) != re.sub(r"\s+", "", expression):
        raise ExpressionError("unparsable license expression %r" % expression)
    pos = 0

    def peek() -> str | None:
        return tokens[pos] if pos < len(tokens) else None

    def take() -> str:
        nonlocal pos
        tok = tokens[pos]
        pos += 1
        return tok

    def expr() -> bool:
        value = term()
        while peek() is not None and peek().upper() == "OR":
            take()
            rhs = term()  # always consume the operand, never short-circuit
            value = value or rhs
        return value

    def term() -> bool:
        value = factor()
        while peek() is not None and peek().upper() == "AND":
            take()
            rhs = factor()
            value = value and rhs
        return value

    def factor() -> bool:
        tok = peek()
        if tok is None:
            raise ExpressionError("unexpected end of %r" % expression)
        if tok == "(":
            take()
            value = expr()
            if peek() != ")":
                raise ExpressionError("missing ')' in %r" % expression)
            take()
            return value
        if tok == ")" or tok.upper() in ("AND", "OR", "WITH"):
            raise ExpressionError("unexpected %r in %r" % (tok, expression))
        ident = take().lower()
        if peek() is not None and peek().upper() == "WITH":
            take()
            if peek() is None or peek() in ("(", ")"):
                raise ExpressionError("WITH without an exception in %r" % expression)
            exception = take().lower()
            return ("%s with %s" % (ident, exception)) in allowed or ident in allowed
        return ident in allowed

    value = expr()
    if pos != len(tokens):
        raise ExpressionError("trailing %r in %r" % (tokens[pos], expression))
    return value


def package_name(key: str) -> str:
    """npm package name of a lockfile ``packages`` key (``node_modules/...``)."""
    return key.split("node_modules/")[-1]


def check_lockfile(path: Path) -> tuple[int, int, list[str]]:
    """Returns (runtime packages, dev packages, violation lines) of one lockfile."""
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        return 0, 0, ["%s: unreadable lockfile: %s" % (path, exc)]
    packages = data.get("packages")
    if not isinstance(packages, dict):
        return 0, 0, ["%s: no `packages` map (lockfileVersion 2 or 3 required)" % path]
    runtime = dev = 0
    violations: list[str] = []
    for key, info in sorted(packages.items()):
        if key == "" or not isinstance(info, dict) or info.get("link"):
            continue
        name = package_name(key)
        scope = "dev" if info.get("dev") else "runtime"
        if scope == "dev":
            dev += 1
        else:
            runtime += 1
        label = "%s: %s@%s [%s]" % (path, name, info.get("version", "?"), scope)
        lic = info.get("license")
        if lic is None:
            violations.append("%s: no license field" % label)
            continue
        if not isinstance(lic, str) or not lic.strip():
            violations.append("%s: license field is not a non-empty string: %r" % (label, lic))
            continue
        try:
            ok = satisfied(lic, _allowed_set(EXCEPTIONS.get(name, ())))
        except ExpressionError as exc:
            violations.append("%s: %s" % (label, exc))
            continue
        if not ok:
            violations.append("%s: license %r is not allowed" % (label, lic))
    return runtime, dev, violations


def _git_ls(root: Path, pattern: str) -> list[Path]:
    out = subprocess.run(
        ["git", "-C", str(root), "ls-files", "-z", "--", pattern],
        capture_output=True,
        check=True,
    ).stdout.decode("utf-8")
    return [
        root / rel
        for rel in out.split("\0")
        if rel and "node_modules/" not in rel
    ]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--root", default=".", help="repository root (default: .)")
    parser.add_argument("lockfiles", nargs="*", help="lockfiles to check (default: all tracked)")
    args = parser.parse_args(argv)
    root = Path(args.root)

    if args.lockfiles:
        lockfiles = [Path(p) for p in args.lockfiles]
        manifests: list[Path] = []
    else:
        try:
            lockfiles = _git_ls(root, "*package-lock.json")
            manifests = _git_ls(root, "*package.json")
        except (OSError, subprocess.CalledProcessError) as exc:
            print("npm_licenses_check.py: cannot list tracked files: %s" % exc, file=sys.stderr)
            return 1
    if not lockfiles:
        print("npm_licenses_check.py: no package-lock.json to check", file=sys.stderr)
        return 1

    locked_dirs = {p.parent for p in lockfiles}
    for manifest in manifests:
        if manifest.parent not in locked_dirs:
            print("note: %s has no package-lock.json; its licenses are not checked" % manifest)

    total_runtime = total_dev = 0
    violations: list[str] = []
    for lockfile in lockfiles:
        runtime, dev, found = check_lockfile(lockfile)
        total_runtime += runtime
        total_dev += dev
        violations.extend(found)
    if violations:
        for line in violations:
            print("npm license gate FAILED: %s" % line, file=sys.stderr)
        return 1
    print(
        "npm license gate PASSED: %d packages (%d runtime, %d dev) in %d lockfile(s)"
        % (total_runtime + total_dev, total_runtime, total_dev, len(lockfiles))
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
