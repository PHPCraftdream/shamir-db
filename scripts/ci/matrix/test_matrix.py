#!/usr/bin/env python3
"""Compute the explainable test matrix for a ref range or injected path list."""
from __future__ import annotations

import argparse
import fnmatch
import importlib.util
import io
import json
import os
import re
import subprocess
import sys
import tarfile
import tomllib
from pathlib import PurePosixPath

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from checks import run_checks, unit_path_audit
from selection import (WORKSPACE, edge_matches, load_edges, manifest_dirs,
                       production_paths, select_matrix, nearest_package, _has_test_targets)

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, "../../.."))
OLD_PATH = os.path.join(HERE, "affected_crates.py")
EDGE_PATH = os.path.join(HERE, "edges.toml")


def _git(args: list[str], root: str = ROOT, binary: bool = False):
    proc = subprocess.run(["git", *args], cwd=root, capture_output=True, check=False)
    if proc.returncode:
        raise RuntimeError(f"git {' '.join(args)} failed ({proc.returncode}): {proc.stderr.decode('utf-8', 'replace').strip()}")
    return proc.stdout if binary else proc.stdout.decode("utf-8", "replace")


def load_metadata(path: str | None, root: str = ROOT) -> dict:
    if path:
        with open(path, encoding="utf-8") as stream:
            return json.load(stream)
    proc = subprocess.run(["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"],
                          cwd=root, capture_output=True, text=True, check=False)
    if proc.returncode:
        raise RuntimeError(f"cargo metadata failed ({proc.returncode}): {proc.stderr.strip()}")
    return json.loads(proc.stdout)


def _changed_paths(spec: str, root: str = ROOT) -> list[str]:
    raw = _git(["diff", "--name-status", "-z", "-M", spec], root, binary=True)
    fields, paths, index = raw.split(b"\0"), [], 0
    while index < len(fields) and fields[index]:
        status = fields[index].decode("ascii", "replace")
        index += 1
        count = 2 if status.startswith(("R", "C")) else 1
        if index + count > len(fields):
            raise RuntimeError("truncated git name-status record")
        paths.extend(item.decode("utf-8", "replace") for item in fields[index:index + count])
        index += count
    return sorted(set(paths))


def _range_input(args, root: str):
    if args.changed:
        with open(args.changed, encoding="utf-8") as stream:
            return [line.rstrip("\r\n") for line in stream if line.strip()], None, None, None, None
    if args.event == "push":
        if args.range or args.base or args.head:
            raise ValueError("push uses --before/--after, not --range/--base/--head")
        if not args.before or not args.after:
            raise ValueError("--event push requires --before and --after")
        return (_changed_paths(f"{args.before}..{args.after}", root), args.before, args.after,
                args.before, args.after)
    if args.before or args.after:
        raise ValueError("--before/--after are valid only with --event push")
    if args.range:
        if args.base or args.head:
            raise ValueError("--range cannot be combined with --base/--head")
        if args.range.count("..") != 1 or "..." in args.range:
            raise ValueError("--range must be exactly A..B (two-dot)")
        base, head = args.range.split("..")
        if not base or not head:
            raise ValueError("--range requires both refs")
        return _changed_paths(f"{base}..{head}", root), base, head, base, head
    base = args.base or "HEAD^"
    head = args.head or "HEAD"
    if args.base:
        merge_base = _git(["merge-base", base, head], root).strip()
        if merge_base == _git(["rev-parse", head], root).strip():
            # head is already on the base (a run on master itself): the last commit only
            return _changed_paths(f"{head}^..{head}", root), f"{head}^", head, f"{head}^", head
        return _changed_paths(f"{merge_base}...{head}", root), merge_base, head, merge_base, head
    # A default event uses a real previous first-parent snapshot, never an
    # implicit workspace sentinel. HEAD^ is a ref expression; no checkout.
    return _changed_paths(f"{base}..{head}", root), base, head, base, head


def _snapshot(rev: str, path: str, root: str = ROOT) -> str | None:
    proc = subprocess.run(["git", "show", f"{rev}:{path}"], cwd=root, capture_output=True, check=False)
    if proc.returncode:
        return None
    return proc.stdout.decode("utf-8", "replace")


def _provider(root: str, rev: str | None = None, cache: dict | None = None):
    cache = cache if cache is not None else {}
    def read(path: str):
        key = (rev, path)
        if key in cache:
            return cache[key]
        if rev:
            value = _snapshot(rev, path, root)
        else:
            try:
                with open(os.path.join(root, path), encoding="utf-8") as stream:
                    value = stream.read()
            except OSError:
                value = None
        cache[key] = value
        return value
    return read


def _tracked_paths(root: str = ROOT) -> list[str]:
    raw = _git(["ls-files", "-z"], root, binary=True)
    return sorted({item.decode("utf-8", "replace") for item in raw.split(b"\0") if item})


def _load_old_selector():
    spec = importlib.util.spec_from_file_location("affected_crates_old", OLD_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError("cannot load moved selector core")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class GitSnapshot:
    """Cached read-only view of a Git tree; blob reads are grouped with cat-file."""
    def __init__(self, rev: str, root: str):
        self.rev, self.root = rev, root
        self.paths = {}
        raw = _git(["ls-tree", "-r", "-z", "--full-tree", rev], root, binary=True)
        for record in raw.split(b"\0"):
            if not record:
                continue
            metadata, path = record.split(b"\t", 1)
            fields = metadata.decode("ascii").split()
            if len(fields) == 3 and fields[1] == "blob":
                self.paths[path.decode("utf-8", "replace")] = fields[2]
        self._cache = {}

    def read_many(self, paths: list[str]) -> dict[str, str | None]:
        missing = sorted({path for path in paths if path not in self._cache})
        oids = [(path, self.paths[path]) for path in missing if path in self.paths]
        if oids:
            payload = "".join(oid + "\n" for _, oid in oids).encode("ascii")
            proc = subprocess.run(["git", "cat-file", "--batch"], cwd=self.root,
                                  input=payload, capture_output=True, check=False)
            if proc.returncode:
                raise RuntimeError("git cat-file --batch failed: " + proc.stderr.decode("utf-8", "replace"))
            stream = io.BytesIO(proc.stdout)
            for path, oid in oids:
                header = stream.readline().decode("ascii", "replace").split()
                if len(header) != 3 or header[0] != oid or header[1] != "blob":
                    raise RuntimeError(f"invalid cat-file response for {path}")
                size = int(header[2])
                data = stream.read(size)
                if len(data) != size or stream.read(1) != b"\n":
                    raise RuntimeError(f"truncated cat-file blob for {path}")
                self._cache[path] = data.decode("utf-8", "replace")
        for path in missing:
            self._cache.setdefault(path, None)
        return {path: self._cache.get(path) for path in paths}

    def read(self, path: str):
        return self.read_many([path])[path]


def _historical_metadata(snapshot: GitSnapshot) -> tuple[dict, dict[str, str]]:
    paths = [path for path in snapshot.paths if path.endswith("Cargo.toml") and
             (path.startswith("crates/") or path.startswith("vendor/"))]
    blobs = snapshot.read_many(paths)
    root_text = snapshot.read("Cargo.toml") or ""
    try:
        root_doc = tomllib.loads(root_text)
    except tomllib.TOMLDecodeError:
        root_doc = {}
    workspace = root_doc.get("workspace", {})
    root_dependencies = workspace.get("dependencies", {})
    workspace_package = workspace.get("package", {})
    manifests, by_dir = {}, {}
    for path in paths:
        text = blobs[path]
        if text is None:
            continue
        try:
            doc = tomllib.loads(text)
            package = doc.get("package", {})
            name = package.get("name")
            if not name:
                continue
            version = package.get("version", "0.0.0")
            if isinstance(version, dict) and version.get("workspace") is True:
                version = workspace_package.get("version", "0.0.0")
            pkg = {"name": name, "version": version, "manifest_path": os.path.join(snapshot.root, path),
                   "dependencies": [], "targets": [], "id": path, "manifest_text": text}
            section_kinds = (("dependencies", None), ("dev-dependencies", "dev"),
                             ("build-dependencies", "build"))
            sections = [(doc.get(section, {}), kind) for section, kind in section_kinds]
            for target_sections in doc.get("target", {}).values():
                sections.extend((target_sections.get(section, {}), kind) for section, kind in section_kinds)
            for entries, kind in sections:
                for dep_name, value in entries.items():
                    if not isinstance(value, dict):
                        continue
                    inherited = value.get("workspace") is True
                    config = root_dependencies.get(dep_name, {}) if inherited else value
                    if not isinstance(config, dict):
                        config = {}
                    actual = value.get("package", config.get("package", dep_name))
                    pkg["dependencies"].append({"name": actual, "kind": kind,
                                                 "path": config.get("path", value.get("path"))})
            manifests[path] = pkg
            by_dir[os.path.dirname(path)] = pkg
        except (tomllib.TOMLDecodeError, AttributeError, TypeError):
            continue

    all_paths = snapshot.paths
    for path, pkg in manifests.items():
        directory = os.path.dirname(path)
        doc = tomllib.loads(pkg["manifest_text"])
        package = doc.get("package", {})
        explicit_lib = doc.get("lib")
        targets = []

        def add_target(kind, name, local, test=True):
            if local in all_paths:
                targets.append({"kind": [kind], "name": name,
                                "src_path": os.path.join(snapshot.root, local), "test": test})

        if explicit_lib is not None:
            lib = explicit_lib if isinstance(explicit_lib, dict) else {}
            local = os.path.normpath(os.path.join(directory, lib.get("path", "src/lib.rs"))).replace("\\", "/")
            target_name = lib.get("name", pkg["name"]).replace("-", "_")
            kind = "proc-macro" if lib.get("proc-macro") else "lib"
            add_target(kind, target_name, local, lib.get("test", True))
        elif package.get("autolib", True):
            local = directory + "/src/lib.rs"
            add_target("lib", pkg["name"].replace("-", "_"), local)

        explicit_bins = doc.get("bin", [])
        configured_bins = set()
        for item in explicit_bins:
            local = os.path.normpath(os.path.join(directory, item.get("path", f"src/bin/{item.get('name', pkg['name'])}.rs"))).replace("\\", "/")
            target_name = item.get("name", os.path.basename(local)[:-3])
            configured_bins.add(local)
            add_target("bin", target_name, local, item.get("test", True))
        if package.get("autobins", True):
            for candidate in all_paths:
                if candidate.startswith(directory + "/src/bin/") and candidate.endswith(".rs"):
                    local = candidate[len(directory) + 1:]
                    if local.count("/") == 2 and local.endswith("/main.rs"):
                        local = candidate
                    elif local.count("/") != 2:
                        continue
                    if local not in configured_bins:
                        stem = os.path.basename(local)[:-3] if not local.endswith("/main.rs") else local.split("/")[-2]
                        add_target("bin", stem, local)
            main = directory + "/src/main.rs"
            if main not in configured_bins:
                add_target("bin", pkg["name"].replace("-", "_"), main)

        explicit_tests = doc.get("test", [])
        configured_tests = set()
        for item in explicit_tests:
            default_path = f"tests/{item.get('name', pkg['name'])}.rs"
            local = os.path.normpath(os.path.join(directory, item.get("path", default_path))).replace("\\", "/")
            target_name = item.get("name", os.path.basename(local)[:-3])
            configured_tests.add(local)
            add_target("test", target_name, local, item.get("test", True))
        if package.get("autotests", True):
            for candidate in all_paths:
                if not candidate.startswith(directory + "/tests/") or not candidate.endswith(".rs"):
                    continue
                local = candidate[len(directory) + 1:]
                if (local.count("/") == 1 or local.endswith("/main.rs")) and local not in configured_tests:
                    target_name = os.path.basename(local)[:-3] if not local.endswith("/main.rs") else local.split("/")[-2]
                    add_target("test", target_name, candidate)
        pkg["targets"] = targets

    members = set()
    member_patterns = workspace.get("members", ())
    excluded = workspace.get("exclude", ())
    for path, pkg in manifests.items():
        directory = os.path.dirname(path)
        included = any(fnmatch.fnmatchcase(directory, pattern.rstrip("/")) for pattern in member_patterns)
        omitted = any(fnmatch.fnmatchcase(directory, pattern.rstrip("/")) for pattern in excluded)
        if included and not omitted:
            members.add(pkg["id"])
    metadata = {"packages": list(manifests.values()), "workspace_members": sorted(members)}
    return metadata, {path: pkg["manifest_text"] for path, pkg in manifests.items()}


def _historical_guard(snapshot: GitSnapshot, metadata: dict, changed: list[str]):
    source_paths = sorted(path for path in snapshot.paths
                          if path.endswith(".rs") and path.startswith(("crates/", "vendor/")))
    source_map = snapshot.read_many(source_paths)
    findings, production = unit_path_audit(snapshot.root, metadata, source_paths, source_map.get,
                                            {"edge": []})
    return findings, production, source_map


def _independent_production_own(changed: list[str], metadata: dict, root: str,
                                provider, guard_overrides: set[str]) -> set[str]:
    """Oracle from path classes, distinct from selector result traversal."""
    dirs = manifest_dirs(metadata, root)
    selected = set()
    for path in changed:
        pkg, _ = nearest_package(path, dirs)
        if not pkg:
            continue
        package_dir = os.path.relpath(os.path.dirname(pkg["manifest_path"]), root).replace("\\", "/")
        local = PurePosixPath(path).relative_to(PurePosixPath(package_dir)).as_posix()
        if path in guard_overrides:
            selected.add(pkg["name"])
        elif local.startswith(("tests/", "benches/", "examples/")):
            continue
        elif local.endswith(".md"):
            text = provider(path)
            if text is None or _include_matches(pkg, local, metadata, provider, root):
                selected.add(pkg["name"])
        elif "src" in PurePosixPath(local).parts and (
                "tests" in PurePosixPath(local).parts or PurePosixPath(local).name.endswith("_tests.rs")):
            continue
        else:
            selected.add(pkg["name"])
    return selected


def _include_matches(pkg: dict, local: str, metadata: dict, provider, root: str) -> bool:
    basename = PurePosixPath(local).name
    package_dir = PurePosixPath(os.path.relpath(os.path.dirname(pkg["manifest_path"]), root).replace("\\", "/"))
    for path in metadata.get("source_paths", ()):
        if not path.endswith(".rs"):
            continue
        source = PurePosixPath(path)
        try:
            source.relative_to(package_dir)
        except ValueError:
            continue
        text = provider(path) or ""
        if re.search(r"(?:include_str|include_bytes)!\s*\([^)]*['\"][^'\"]*" + re.escape(basename) + r"['\"]", text):
            return True
    return False


def _commit_paths(parent: str, commit: str, root: str) -> list[str]:
    return _changed_paths(f"{parent}..{commit}", root)


def _validate_ref(ref: str, root: str) -> str:
    if not ref or ref.startswith("-"):
        raise ValueError(f"invalid empty or option-like Git ref: {ref!r}")
    return _git(["rev-parse", "--verify", "--end-of-options", f"{ref}^{{commit}}"], root).strip()


def _validate_range(spec: str, root: str, label: str = "range") -> tuple[str, str]:
    if spec.count("..") != 1 or "..." in spec:
        raise ValueError(f"{label} must be exactly A..B (two-dot)")
    first, second = spec.split("..")
    if not first or not second:
        raise ValueError(f"{label} requires both refs")
    return _validate_ref(first, root), _validate_ref(second, root)


def _backtest(revision_range: str, edge_config: dict, root: str):
    base_ref, head_ref = _validate_range(revision_range, root, "--backtest")
    commits = _git(["rev-list", "--first-parent", "--reverse", f"{base_ref}..{head_ref}"], root).splitlines()
    old_selector = _load_old_selector()
    rows, misses = [], []
    old_all_count = new_all_count = 0
    old_reason_counts, new_reason_counts, old_trigger_paths = {}, {}, {}
    size_old, size_new = [], []
    snapshots = {}

    def snapshot(rev):
        if rev not in snapshots:
            snapshots[rev] = GitSnapshot(rev, root)
        return snapshots[rev]

    for commit in commits:
        parent_line = _git(["rev-list", "--parents", "-n", "1", commit], root).split()
        if len(parent_line) < 2:
            continue
        parent = parent_line[1]
        changed = _commit_paths(parent, commit, root)
        head_snapshot, base_snapshot = snapshot(commit), snapshot(parent)
        historical, _ = _historical_metadata(head_snapshot)
        historical["source_paths"] = [path for path in head_snapshot.paths if path.endswith(".rs")]
        manifest_paths = [path for path in head_snapshot.paths if path.endswith("Cargo.toml")]
        paths_to_read = (set(changed) | set(historical["source_paths"]) | set(manifest_paths) |
                         {"Cargo.lock", "Cargo.toml", ".config/nextest.toml"})
        head_text = head_snapshot.read_many(sorted(paths_to_read))
        base_text = base_snapshot.read_many(["Cargo.lock", "Cargo.toml", ".config/nextest.toml"])
        provider = lambda path, _map=head_text: _map.get(path)
        old = old_selector.select_crates(changed, historical, root=root)
        old_all = old == old_selector.WORKSPACE
        old_names = {p["name"] for p in historical["packages"]} if old_all else set(old)
        for path in changed:
            if old_selector._is_global(path):
                old_trigger_paths[path] = old_trigger_paths.get(path, 0) + 1
        if old_all:
            old_all_count += 1
            reason = "old selector global/unbounded sentinel"
            old_reason_counts[reason] = old_reason_counts.get(reason, 0) + 1
        guard_findings, guard_overrides, _ = _historical_guard(head_snapshot, historical, changed)
        base_historical, _ = _historical_metadata(base_snapshot) if "Cargo.toml" in changed else ({}, {})
        base_packages = {pkg["name"]: pkg for pkg in base_historical.get("packages", ())}
        all_manifest_text = dict(head_text)
        if "Cargo.toml" in changed:
            base_manifest_paths = [path for path in base_snapshot.paths if path.endswith("Cargo.toml")]
            all_manifest_text.update(base_snapshot.read_many(base_manifest_paths))
        new = select_matrix(changed, historical, root, provider, edge_config,
                            lock_base=base_text.get("Cargo.lock"), lock_head=head_text.get("Cargo.lock"),
                            manifest_base=base_text.get("Cargo.toml"), manifest_head=head_text.get("Cargo.toml"),
                            nextest_base=base_text.get(".config/nextest.toml"), nextest_head=head_text.get(".config/nextest.toml"),
                            manifest_provider=lambda path, _map=all_manifest_text: _map.get(path),
                            source_paths=historical["source_paths"], production_overrides=guard_overrides,
                            base_metadata=base_historical if base_historical else None)
        new_names = {crate["name"] for crate in new["crates"]}
        if new["scope"] == "all":
            new_names = {pkg["name"] for pkg in historical["packages"]}
        own = _independent_production_own(changed, historical, root, provider, guard_overrides)
        missing = sorted(own - new_names)
        selected_by_name = {crate["name"]: crate for crate in new["crates"]}
        test_missing = []
        for name in own:
            pkg = next(p for p in historical["packages"] if p["name"] == name)
            enabled_lib = any(set(target.get("kind", ())) & {"lib", "proc-macro"} and target.get("test", True)
                              for target in pkg.get("targets", ()))
            enabled_tests = {target.get("name") for target in pkg.get("targets", ())
                             if "test" in target.get("kind", ()) and target.get("test", True)}
            selection = selected_by_name.get(name)
            if name not in new_names or (enabled_lib and not (selection or {}).get("lib", False)):
                test_missing.append(name)
            elif enabled_tests:
                chosen = (selection or {}).get("integration", [])
                if chosen != "all" and not enabled_tests.issubset(set(chosen)):
                    test_missing.append(name)
        if missing or test_missing:
            misses.append({"commit": commit, "production_own_missing": missing,
                           "test_target_missing": sorted(test_missing)})
        if new["scope"] == "all":
            new_all_count += 1
        for reason in new["all_reasons"]:
            new_reason_counts[reason] = new_reason_counts.get(reason, 0) + 1
        old_size, new_size = len(old_names), len(new_names)
        size_old.append(old_size)
        size_new.append(new_size)
        rows.append({"commit": commit, "changed": len(changed), "old_scope": "all" if old_all else "bounded",
                     "old_size": old_size, "new_scope": new["scope"], "new_size": new_size,
                     "old_reasons": (["old selector global/unbounded sentinel"] if old_all else []),
                     "all_reasons": new["all_reasons"], "crate_reasons": {c["name"]: c["reasons"] for c in new["crates"]},
                     "old": old, "new": new["args"], "guard_findings": guard_findings,
                     "production_own_missing": missing, "test_target_missing": sorted(test_missing)})
    average_old = sum(size_old) / len(size_old) if size_old else 0
    average_new = sum(size_new) / len(size_new) if size_new else 0
    summary = {"commits": len(rows), "old_sizes": size_old, "new_sizes": size_new,
               "old_total_size": sum(size_old), "new_total_size": sum(size_new),
               "old_mean_size": average_old, "new_mean_size": average_new,
               "old_all": old_all_count, "new_all": new_all_count,
               "old_all_reasons": old_reason_counts, "old_trigger_paths": old_trigger_paths,
               "new_all_reasons": new_reason_counts, "production_coverage_failures": misses,
               "historical_metadata": "Cargo.toml/tree-derived metadata per commit; Cargo target defaults and explicit overrides reconstructed"}
    return rows, summary


def _explain(result: dict) -> str:
    lines = [f"Scope: {result['scope']}  Args: {result['args']}"]
    lines.extend("ALL: " + reason for reason in result["all_reasons"])
    lines.append("crate | tests/checks | reasons")
    for item in result["crates"]:
        runs = (["lib"] if item["lib"] else [])
        if item["integration"] == "all":
            runs.append("integration:all")
        elif item["integration"]:
            runs.append("integration:" + ",".join(item["integration"]))
        if item["check_only"]:
            runs.append("check-only")
        lines.append(f"{item['name']} | {', '.join(runs) or 'none'} | {'; '.join(item['reasons'])}")
    lines.append("flags: " + ", ".join(f"{key}={str(value).lower()}" for key, value in result["flags"].items()))
    return "\n".join(lines)


def _github_output(result: dict) -> str:
    args = result["args"]
    if not re.fullmatch(r"(?:|--workspace|-p [A-Za-z0-9_-]+(?: -p [A-Za-z0-9_-]+)*)", args):
        raise ValueError("malformed args line: " + repr(args))
    lines = ["matrix=" + json.dumps(result, separators=(",", ":"), sort_keys=True), "args=" + args]
    lines.extend(f"{key}={'true' if value else 'false'}" for key, value in result["flags"].items())
    return "\n".join(lines)


def build_parser():
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group()
    source.add_argument("--range", help="explicit two-dot A..B range")
    source.add_argument("--base", help="merge-base base ref (default previous first-parent commit)")
    parser.add_argument("--head", help="head ref, defaults to HEAD")
    parser.add_argument("--event", default="pull_request", choices=("pull_request", "push", "workflow_dispatch", "schedule"))
    parser.add_argument("--before")
    parser.add_argument("--after")
    parser.add_argument("--metadata")
    parser.add_argument("--changed")
    parser.add_argument("--edge-config")
    parser.add_argument("--format", choices=("json", "github-output", "explain"), default="json")
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--backtest")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        if args.head and not args.base:
            raise ValueError("--head requires --base")
        if args.range and args.head:
            raise ValueError("--range cannot be combined with --head")
        if args.event == "push" and (args.range or args.base or args.head):
            raise ValueError("push uses --before/--after, not --range/--base/--head")
        if args.changed and args.metadata is None:
            raise ValueError("--changed requires --metadata for git/cargo-free core input")
        if args.changed is None:
            metadata = load_metadata(args.metadata, ROOT)
        else:
            with open(args.metadata, encoding="utf-8") as stream:
                metadata = json.load(stream)
        with open(args.edge_config or EDGE_PATH, encoding="utf-8") as stream:
            edge_text = stream.read()
        edges = load_edges(edge_text)
        if args.check:
            problems = run_checks(ROOT, metadata, edge_text)
            if problems:
                print("\n".join(problems), file=sys.stderr)
                return 1
            if not args.backtest:
                return 0
        if args.backtest:
            rows, summary = _backtest(args.backtest, edges, ROOT)
            for row in rows:
                print(json.dumps(row, separators=(",", ":"), sort_keys=True))
            print("summary " + json.dumps(summary, separators=(",", ":"), sort_keys=True))
            return int(bool(summary["production_coverage_failures"]))
        if args.event not in {"push", "pull_request"} and (args.before or args.after):
            raise ValueError("--before/--after are valid only for push")
        if args.event in {"workflow_dispatch", "schedule"} and not args.changed and not args.range and not args.base and not args.head:
            args.range = "HEAD^..HEAD"
        if args.changed and (args.range or args.base or args.head or args.before or args.after):
            raise ValueError("--changed cannot be combined with refs/ranges")
        if args.changed:
            with open(args.changed, encoding="utf-8") as stream:
                changed = [line.rstrip("\r\n") for line in stream if line.strip()]
            base = head = diff_base = diff_head = None
            tracked_paths = []
            source_provider = _provider(ROOT)
        else:
            if args.event == "push":
                if not args.before or not args.after:
                    raise ValueError("push requires --before and --after")
                changed = _changed_paths(f"{args.before}..{args.after}", ROOT)
                base, head = args.before, args.after
                diff_base, diff_head = base, head
            else:
                if args.range:
                    if args.range.count("..") != 1 or "..." in args.range:
                        raise ValueError("--range must be exactly A..B")
                    base, head = args.range.split("..")
                    if not base or not head:
                        raise ValueError("--range requires A..B")
                    spec = f"{base}..{head}"
                    diff_base, diff_head = base, head
                elif args.base:
                    head = args.head or "HEAD"
                    base = _git(["merge-base", args.base, head], ROOT).strip()
                    spec = f"{base}...{head}"
                    diff_base, diff_head = base, head
                else:
                    head = args.head or "HEAD"
                    base = _git(["rev-parse", f"{head}^1"], ROOT).strip()
                    spec = f"{base}..{head}"
                    diff_base, diff_head = base, head
                changed = _changed_paths(spec, ROOT)
            tracked_paths = _tracked_paths(ROOT)
            if tracked_paths:
                snapshot = GitSnapshot(head, ROOT)
                tracked_paths = list(snapshot.paths)
                source_map = snapshot.read_many([path for path in tracked_paths if path.endswith(".rs")])
                source_provider = source_map.get
            else:
                source_provider = _provider(ROOT, head)
        source_paths = [path for path in (tracked_paths or []) if path.endswith(".rs")]
        if args.changed:
            guard_findings, production_override = [], set()
        else:
            guard_findings, production_override = unit_path_audit(ROOT, metadata, source_paths, source_provider, edges)
        if args.check and guard_findings:
            print("\n".join(guard_findings), file=sys.stderr)
            return 1
        extras = {}
        if "Cargo.lock" in changed and diff_base and diff_head:
            extras.update(lock_base=_snapshot(diff_base, "Cargo.lock", ROOT), lock_head=_snapshot(diff_head, "Cargo.lock", ROOT))
        if "Cargo.toml" in changed and diff_base and diff_head:
            base_meta, _ = _historical_metadata(GitSnapshot(diff_base, ROOT))
            extras.update(manifest_base=_snapshot(diff_base, "Cargo.toml", ROOT), manifest_head=_snapshot(diff_head, "Cargo.toml", ROOT),
                          manifest_provider=lambda path: _snapshot(diff_head, path, ROOT), base_metadata=base_meta)
        if ".config/nextest.toml" in changed and diff_base and diff_head:
            extras.update(nextest_base=_snapshot(diff_base, ".config/nextest.toml", ROOT), nextest_head=_snapshot(diff_head, ".config/nextest.toml", ROOT))
        result = select_matrix(changed, metadata, ROOT, source_provider, edges,
                               source_paths=source_paths, production_overrides=production_override, **extras)
        result["base"], result["head"] = base, head
        rendered = json.dumps(result, separators=(",", ":"), sort_keys=True) if args.format == "json" else _github_output(result) if args.format == "github-output" else _explain(result)
        print(rendered)
        return 0
    except (OSError, RuntimeError, ValueError, KeyError, TypeError, json.JSONDecodeError) as exc:
        print(f"test_matrix.py: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
