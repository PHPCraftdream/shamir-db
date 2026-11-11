"""Pure crate/test selection from change paths and injected source snapshots.

All repository contents, source-file lists, lock/root-manifest snapshots and
hidden-edge configuration enter through arguments. This module never invokes
Git/Cargo or walks the filesystem. Callers may inject a source reader and paths
for Markdown include checks and the textual unit-path guard.
"""
from __future__ import annotations

import fnmatch
import os
import re
import tomllib
from pathlib import PurePosixPath

WORKSPACE = "--workspace"
TS_DIR = "crates/client/shamir-client-ts"
DEPENDENCY_INPUTS = {"Cargo.toml", "Cargo.lock", "deny.toml", "package.json", "package-lock.json"}
DEPENDENCY_SCRIPTS = {"scripts/ci/gates/npm_licenses_check.py", "scripts/ci/gates/native_code_check.py",
                      "scripts/ci/gates/dependency_admission_check.py"}
WASM_GUESTS = {"shamir-sdk", "shamir-query-builder", "shamir-query-types", "shamir-types"}


def norm(path: str) -> str:
    return path.replace("\\", "/").removeprefix("./")


def relative(path: str, root: str) -> str:
    path, root = norm(path), norm(root).rstrip("/")
    if path == root:
        return "."
    if path.lower().startswith((root + "/").lower()):
        return path[len(root) + 1:]
    if re.match(r"^[A-Za-z]:/", path) and re.match(r"^[A-Za-z]:/", root):
        try:
            return norm(os.path.relpath(path, root))
        except ValueError:
            pass
    return path


def manifest_dirs(metadata: dict, root: str) -> dict[str, dict]:
    member_ids = set(metadata.get("workspace_members", ()))
    dirs = {}
    for pkg in metadata.get("packages", ()):
        if member_ids and pkg.get("id") not in member_ids:
            continue
        manifest = pkg.get("manifest_path")
        if manifest:
            parent = PurePosixPath(relative(manifest, root)).parent.as_posix()
            if parent != ".":
                dirs[parent] = pkg
    return dirs


def nearest_package(path: str, dirs: dict[str, dict]) -> tuple[dict | None, str]:
    for ancestor in PurePosixPath(norm(path)).parents:
        key = ancestor.as_posix()
        if key == ".":
            break
        if key in dirs:
            return dirs[key], key
    return None, ""


def _member_names(metadata: dict) -> set[str]:
    members = set(metadata.get("workspace_members", ()))
    return {pkg["name"] for pkg in metadata.get("packages", ())
            if not members or pkg.get("id") in members}


def _dependency_maps(metadata: dict, names: set[str]):
    packages = {p["name"]: p for p in metadata.get("packages", ()) if p["name"] in names}
    reverse = {kind: {} for kind in ("normal", "build", "dev")}
    for name, pkg in packages.items():
        for dep in pkg.get("dependencies", ()):
            target = dep.get("name")
            if target not in names:
                continue
            kind = dep.get("kind") or "normal"
            if kind in reverse:
                reverse[kind].setdefault(target, set()).add(name)
    return packages, reverse


def _target_rel(pkg: dict, target: dict, root: str) -> str:
    package_dir = PurePosixPath(relative(pkg["manifest_path"], root)).parent
    source = PurePosixPath(relative(target.get("src_path", ""), root))
    try:
        return source.relative_to(package_dir).as_posix()
    except ValueError:
        return source.as_posix()


def _integration_targets(pkg: dict, root: str) -> dict[str, str]:
    return {t.get("name", ""): _target_rel(pkg, t, root)
            for t in pkg.get("targets", ()) if "test" in t.get("kind", ())}

def _pkg_local(pkg: dict, path: str, root: str) -> str:
    local = relative(path, root)
    package_dir = PurePosixPath(relative(pkg["manifest_path"], root)).parent
    try:
        return PurePosixPath(local).relative_to(package_dir).as_posix()
    except ValueError:
        return local


def _markdown_is_production(pkg: dict, path: str, root: str,
                            content_provider, source_paths: list[str]) -> bool:
    basename = PurePosixPath(path).name
    package_dir = PurePosixPath(relative(pkg["manifest_path"], root)).parent
    for source in source_paths:
        if not source.endswith(".rs"):
            continue
        source_local = PurePosixPath(relative(source, root))
        try:
            source_local.relative_to(package_dir)
        except ValueError:
            continue
        text = content_provider(source) if content_provider else None
        if text and re.search(r"(?:include_str|include_bytes)!\s*\([^;]*?\b" + re.escape(basename) + r"\b", text):
            return True
    return False


def role(pkg: dict, path: str, root: str, content_provider=None,
         source_paths: list[str] | None = None) -> tuple[str, str | None]:
    """Classify a package path as production, unit, integration, check or none."""
    local = _pkg_local(pkg, path, root)
    parts = PurePosixPath(local).parts
    if local.endswith(".md"):
        sources = source_paths or []
        if content_provider and _markdown_is_production(pkg, path, root, content_provider, sources):
            return "production", None
        return "none", None
    targets = _integration_targets(pkg, root)
    if parts and parts[0] == "tests":
        for name, source in targets.items():
            source_path = PurePosixPath(source)
            if local == source or local.startswith(source.rstrip("/") + "/"):
                return "integration", name
            # Cargo target `tests/foo.rs` may itself be the bin root; a target
            # declared at `tests/foo/main.rs` also owns its companion tree.
            if source_path.parent.as_posix() == "tests" and source_path.suffix == ".rs" and source_path.name != "main.rs" and local.startswith(f"tests/{source_path.stem}/"):
                return "integration", name
            parent = source_path.parent.as_posix()
            if source_path.name == "main.rs" and parent != "." and local.startswith(parent + "/"):
                return "integration", name
        return "integration_all", None
    if "src" in parts and ("tests" in parts or PurePosixPath(local).name.endswith("_tests.rs")):
        return "unit", None
    if parts and parts[0] in {"benches", "examples"}:
        return "check", None
    return "production", None


def load_edges(text: str) -> dict:
    config = tomllib.loads(text)
    for edge in config.get("edge", ()):
        if not edge.get("paths") or not edge.get("reason"):
            raise ValueError("each [[edge]] requires paths and reason")
        if edge.get("propagate") not in {None, "tests", "prod"}:
            raise ValueError("edge propagate must be tests or prod")
    return config


def edge_matches(edge: dict, path: str) -> bool:
    return any(fnmatch.fnmatchcase(norm(path), pattern) for pattern in edge.get("paths", ()))


def _has_test_targets(pkg: dict, root: str) -> tuple[bool, bool]:
    library = any(set(t.get("kind", ())) & {"lib", "proc-macro"} and t.get("test", True)
                  for t in pkg.get("targets", ()))
    return library, bool(_integration_targets(pkg, root))


def _lock_data(text: str):
    try:
        data = tomllib.loads(text)
    except tomllib.TOMLDecodeError as exc:
        raise ValueError(f"invalid lockfile: {exc}") from exc
    packages = data.get("package")
    if not isinstance(packages, list):
        raise ValueError("lockfile lacks package array")
    by_key, by_name = {}, {}
    for item in packages:
        try:
            key = (item["name"], item["version"], item.get("source"))
        except (KeyError, TypeError) as exc:
            raise ValueError("malformed lock package record") from exc
        if key in by_key:
            raise ValueError(f"duplicate lock package {key[0]} {key[1]}")
        by_key[key] = item
        by_name.setdefault(key[0], []).append(key)
    graph = {}
    for key, item in by_key.items():
        edges = set()
        for raw in item.get("dependencies", ()):
            if not isinstance(raw, str):
                raise ValueError(f"invalid dependency record in {key[0]} {key[1]}")
            if " " not in raw:
                candidates = by_name.get(raw, ())
                if len(candidates) != 1:
                    raise ValueError("ambiguous bare locked dependency: " + raw)
                edges.add(candidates[0])
                continue
            match = re.match(r"^(.+?) ([^ ]+)(?: \((.*)\))?$", raw)
            if not match:
                raise ValueError("unrecognized locked dependency: " + raw)
            name, version, source = match.groups()
            candidates = [k for k in by_name.get(name, ()) if k[1] == version and (not source or k[2] == source)]
            if len(candidates) != 1:
                raise ValueError("unresolved or ambiguous locked dependency: " + raw)
            edges.add(candidates[0])
        graph[key] = edges
    return by_key, graph, by_name


def _reach(graph: dict, start):
    visited, stack = set(), [start]
    while stack:
        node = stack.pop()
        if node in visited:
            continue
        visited.add(node)
        stack.extend(graph.get(node, ()))
    return visited


def lockfile_changes(base_text: str, head_text: str, packages: dict[str, dict]) -> dict[str, str]:
    old_nodes, old_graph, old_names = _lock_data(base_text)
    new_nodes, new_graph, new_names = _lock_data(head_text)
    changed = {key for key in set(old_nodes) | set(new_nodes) if old_nodes.get(key) != new_nodes.get(key)}
    removed = set(old_nodes) - set(new_nodes)
    added_or_changed = changed - removed
    result = {}
    for name, pkg in packages.items():
        version = pkg.get("version")
        # Historical member identity is the package name/version/path. Match
        # the lock node by name/source first, then fall back to version so a
        # member-version update is not mistaken for an external dependency.
        old_member = [k for k in old_names.get(name, ()) if k[2] is None]
        new_member = [k for k in new_names.get(name, ()) if k[2] is None]
        own_old = next((k for k in old_member if k[1] == version), old_member[0] if len(old_member) == 1 else None)
        own_new = next((k for k in new_member if k[1] == version), new_member[0] if len(new_member) == 1 else None)
        old_deps = old_nodes[own_old].get("dependencies", []) if own_old else []
        new_deps = new_nodes[own_new].get("dependencies", []) if own_new else []
        if own_old and own_new and old_deps != new_deps:
            result[name] = "lock: own dependency list changed"
            continue
        reached_old = _reach(old_graph, own_old) if own_old else set()
        reached_new = _reach(new_graph, own_new) if own_new else set()
        old_hits = sorted(changed & removed & reached_old)
        new_hits = sorted(changed & added_or_changed & reached_new)
        hits = old_hits + new_hits
        if hits:
            labels = sorted({f"{key[0]} {key[1]}" for key in hits})
            result[name] = "lock: " + ", ".join(labels)
        elif own_old is None or own_new is None:
            if name in old_names or name in new_names:
                result[name] = "lock: workspace member package record changed"
    return result


def _manifest(text: str, label: str):
    try:
        return tomllib.loads(text)
    except tomllib.TOMLDecodeError as exc:
        raise ValueError(f"invalid {label}: {exc}") from exc


def root_manifest_changes(base_text: str, head_text: str, packages: dict[str, dict], root: str,
                          manifest_provider=None, base_packages: dict[str, dict] | None = None) -> dict[str, str]:
    before, after = _manifest(base_text, "base manifest"), _manifest(head_text, "head manifest")
    bw, aw = before.get("workspace", {}), after.get("workspace", {})
    bd, ad = bw.get("dependencies", {}), aw.get("dependencies", {})
    def non_dependency_config(data):
        copy = dict(data)
        ws = dict(copy.get("workspace", {}))
        for key in ("dependencies", "members", "exclude"):
            ws.pop(key, None)
        copy["workspace"] = ws
        return copy
    if non_dependency_config(before) != non_dependency_config(after):
        return {WORKSPACE: "root Cargo.toml build configuration changed"}
    dependency_changes = {name for name in set(bd) | set(ad) if bd.get(name) != ad.get(name)}
    if not dependency_changes:
        # Membership-only workspace edits do not change compilation and need
        # no member-manifest snapshots.
        return {}
    old_packages = base_packages or packages
    union = {**old_packages, **packages}
    used_by = {}
    for name, pkg in union.items():
        manifest_path = pkg.get("manifest_path")
        rel = norm(os.path.relpath(manifest_path, root)) if manifest_path else ""
        source = manifest_provider(rel) if manifest_provider and rel else None
        if source is None and manifest_provider:
            # The head may have removed a package manifest; use the historical
            # package record's injected manifest when available, never skip it.
            source = pkg.get("manifest_text")
        if source is None:
            raise ValueError(f"cannot read member manifest for {name}: {rel or '<missing path>'}")
        manifest = _manifest(source, f"member manifest {rel}")
        used = set()
        sections = [manifest.get(key, {}) for key in ("dependencies", "dev-dependencies", "build-dependencies")]
        for target in manifest.get("target", {}).values():
            sections.extend(target.get(key, {}) for key in ("dependencies", "dev-dependencies", "build-dependencies"))
        for section in sections:
            for dep_name, value in section.items():
                if isinstance(value, dict) and value.get("workspace") is True:
                    used.add(dep_name)
        used_by[name] = used
    result = {}
    for dep_name in sorted(dependency_changes):
        members = sorted(name for name, used in used_by.items() if dep_name in used)
        for name in members:
            result[name] = f"workspace dependency changed: {dep_name}"
    return result


def nextest_overrides(before: str, after: str) -> tuple[bool, set[str]]:
    import difflib
    old, new = before.splitlines(), after.splitlines()
    changed_lines = []
    for tag, i1, i2, j1, j2 in difflib.SequenceMatcher(a=old, b=new, autojunk=False).get_opcodes():
        if tag != "equal":
            changed_lines.extend((old, idx) for idx in range(i1, i2))
            changed_lines.extend((new, idx) for idx in range(j1, j2))
    if not changed_lines:
        return True, set()
    selected = set()
    for lines, idx in changed_lines:
        start = idx
        while start >= 0 and not re.match(r"^\s*\[\[?[^]]+\]\]?\s*$", lines[start]):
            start -= 1
        if start < 0 or not re.match(r"\s*\[\[profile\..*\.overrides\]\]\s*$", lines[start]):
            return False, set()
        end = start + 1
        while end < len(lines) and not re.match(r"^\s*\[\[?[^]]+\]\]?\s*$", lines[end]):
            end += 1
        block = "\n".join(lines[start:end])
        filters = re.findall(r"(?m)^\s*filter\s*=\s*([\"'])(.*?)\1\s*$", block)
        if len(filters) != 1:
            return False, set()
        expr = filters[0][1]
        # Only positive package predicates joined by conjunction are a safe
        # bounded set. Negation/alternation can broaden selection unexpectedly.
        matches = re.findall(r"package\(([A-Za-z0-9_-]+)\)", expr)
        scrubbed = re.sub(r"package\([A-Za-z0-9_-]+\)", "PACKAGE", expr)
        if not matches or re.search(r"[!|]|\bor\b", scrubbed) or "PACKAGE" not in scrubbed:
            return False, set()
        selected.update(matches)
    return True, selected


def _explicit_classification(path: str) -> str | None:
    """Return table row's classification; None means fail-safe all."""
    path = norm(path)
    if path in {"Cargo.lock", "Cargo.toml", "deny.toml", "clippy.toml", ".config/nextest.toml", ".github/workflows/ci.yml"}:
        return "special"
    if path.startswith("rust-toolchain") or path.startswith(".cargo/") or path == "scripts/test.sh":
        return "all"
    if path.startswith((".github/", "scripts/", ".rust-guard/", "tools/rust-guard/", ".githooks/", ".claude/", ".crush/", "docs/")) or path in {"bench-iters.txt", ".gitignore", ".gitattributes", ".git-blame-ignore-revs"} or path.startswith("LICENSE") or path.endswith(".md"):
        return "none"
    if path.startswith(TS_DIR + "/"):
        return "ts"
    return None


def select_matrix(changed: list[str], metadata: dict, root: str, content_provider=None,
                  edge_config: dict | None = None, lock_base: str | None = None,
                  lock_head: str | None = None, manifest_base: str | None = None,
                  manifest_head: str | None = None, nextest_base: str | None = None,
                  nextest_head: str | None = None, manifest_provider=None,
                  source_paths: list[str] | None = None,
                  production_overrides: set[str] | None = None,
                  base_metadata: dict | None = None) -> dict:
    """Pure matrix API. External contents and scanner findings are injected."""
    edge_config = edge_config or {"edge": [], "canary": []}
    members = _member_names(metadata)
    packages, reverse = _dependency_maps(metadata, members)
    dirs = manifest_dirs(metadata, root)
    edges = edge_config.get("edge", ())
    production_overrides = production_overrides or set()
    source_paths = source_paths or []
    changed = sorted({norm(path) for path in changed if path})
    jobs, all_reasons = {}, []
    flags = {key: False for key in ("engine", "ts", "node", "wasm32", "deny", "pipeline", "docs_only")}

    def add(name, mode, reason, target=None):
        if name not in members:
            raise ValueError(f"selection names non-workspace crate {name}")
        item = jobs.setdefault(name, {"lib": False, "integration": [], "check_only": False, "reasons": []})
        item["reasons"].append(reason)
        if mode in {"all_tests", "all_local_tests"}:
            item["check_only"] = False
            lib, integrations = _has_test_targets(packages[name], root)
            item["lib"] |= lib
            if integrations:
                item["integration"] = "all"
        elif mode == "unit":
            item["check_only"] = False
            item["lib"] |= _has_test_targets(packages[name], root)[0]
        elif mode == "integration":
            item["check_only"] = False
            if item["integration"] != "all" and target not in item["integration"]:
                item["integration"].append(target)
        elif mode == "integration_all":
            item["check_only"] = False
            if _integration_targets(packages[name], root):
                item["integration"] = "all"
        elif mode == "check" and not item["lib"] and not item["integration"]:
            item["check_only"] = True

    def production(seed, reason):
        closure, pending = {seed}, [seed]
        add(seed, "all_tests", reason)
        while pending:
            target = pending.pop()
            for kind in ("normal", "build"):
                for reader in reverse[kind].get(target, ()):
                    if reader not in closure:
                        closure.add(reader)
                        pending.append(reader)
                        add(reader, "all_tests", f"depends on {target} ({kind})")
        for target in sorted(closure):
            for reader in reverse["dev"].get(target, ()):
                if reader not in closure:
                    add(reader, "all_tests", f"dev-depends on {target}")

    def all_scope(reason):
        all_reasons.append(reason)

    for path in changed:
        pkg, _ = nearest_package(path, dirs)
        matching = [edge for edge in edges if edge_matches(edge, path)]
        if (PurePosixPath(path).name in DEPENDENCY_INPUTS or path in DEPENDENCY_SCRIPTS):
            flags["deny"] = True
        for edge in matching:
            for flag in edge.get("flags", ()):
                if flag not in flags:
                    raise ValueError(f"unknown edge flag {flag}")
                flags[flag] = True
            reason = f"declared edge: {edge['reason']}"
            for name in edge.get("crates", ()):
                if edge.get("propagate", "tests") == "prod":
                    production(name, reason)
                else:
                    add(name, "all_tests", reason)
        if path == "Cargo.lock":
            if lock_base is None or lock_head is None:
                all_scope("Cargo.lock snapshots unavailable")
            else:
                try:
                    for name, reason in lockfile_changes(lock_base, lock_head, packages).items():
                        production(name, reason)
                except (ValueError, KeyError, TypeError) as exc:
                    all_scope(f"Cargo.lock unparsable: {exc}")
            continue
        if path == "Cargo.toml":
            if manifest_base is None or manifest_head is None or manifest_provider is None:
                all_scope("root Cargo.toml snapshots or member manifests unavailable")
            else:
                try:
                    base_packages = {p["name"]: p for p in (base_metadata or metadata).get("packages", ())}
                    for name, reason in root_manifest_changes(manifest_base, manifest_head, packages, root, manifest_provider, base_packages).items():
                        if name == WORKSPACE:
                            all_scope(reason)
                        elif name in members:
                            production(name, reason)
                except (ValueError, KeyError, TypeError) as exc:
                    all_scope(f"root Cargo.toml unparsable: {exc}")
            continue
        if path == ".config/nextest.toml":
            if nextest_base is None or nextest_head is None:
                all_scope("nextest snapshots unavailable")
            else:
                scoped, names = nextest_overrides(nextest_base, nextest_head)
                if not scoped or names - members:
                    all_scope("nextest change outside package-specific override filters")
                else:
                    for name in sorted(names):
                        add(name, "all_tests", "nextest package override changed")
            continue
        if path == "deny.toml":
            flags["deny"] = True
            continue
        if path == "clippy.toml":
            for name in sorted(members):
                add(name, "check", "clippy.toml affects lint checks")
            continue
        if (path == ".github/workflows/ci.yml" or path.startswith(".github/actions/")
                or (path.startswith("scripts/ci/") and not path.startswith("scripts/ci/tests/"))):
            flags["pipeline"] = True
            for name in edge_config.get("canary", ()):
                add(name, "all_tests", "CI pipeline canary")
            continue
        if pkg and path.endswith(".md"):
            if _markdown_is_production(pkg, path, root, content_provider, source_paths):
                production(pkg["name"], f"changed: {path} (+1)")
            continue
        classification = _explicit_classification(path)
        if classification == "all":
            all_scope(f"global build/test configuration: {path}")
            continue
        if classification == "none":
            continue
        if classification == "ts":
            flags["ts"] = True
            continue
        if classification == "none":
            continue
        if classification == "special":
            continue
        if path.startswith(("examples/", "tests/", "tools/")):
            all_scope(f"unclassified path: {path}")
            continue
        if path.startswith("vendor/") and pkg:
            production(pkg["name"], f"changed: {path} (+1)")
            continue
        if path in production_overrides:
            if pkg:
                production(pkg["name"], f"test-path guard failed; production source: {path}")
            else:
                all_scope(f"unclassified guarded path: {path}")
            continue
        if path.endswith(".md"):
            if pkg and _markdown_is_production(pkg, path, root, content_provider, source_paths):
                production(pkg["name"], f"changed: {path} (+1)")
            continue
        if pkg:
            kind, target = role(pkg, path, root, content_provider, source_paths)
            if kind == "production":
                production(pkg["name"], f"changed: {path} (+1)")
            elif kind == "unit":
                add(pkg["name"], "unit", f"unit test changed: {path}")
            elif kind == "integration":
                add(pkg["name"], "integration", f"integration test changed: {path}", target)
            elif kind == "integration_all":
                add(pkg["name"], "integration_all", f"integration support changed: {path}")
            elif kind == "check":
                add(pkg["name"], "check", f"check target changed: {path}")
            continue
        if not matching:
            all_scope(f"unclassified path: {path}")

    if all_reasons:
        result_crates = []
        for name in sorted(members):
            lib, integrations = _has_test_targets(packages[name], root)
            result_crates.append({"name": name, "lib": lib, "integration": "all" if integrations else [],
                                  "check_only": False, "reasons": ["workspace selected"]})
        scope = "all"
    else:
        result_crates = []
        for name, item in sorted(jobs.items()):
            if item["integration"] != "all":
                item["integration"] = sorted(item["integration"])
            item["reasons"] = sorted(set(item["reasons"]))
            result_crates.append({"name": name, **item})
        scope = "bounded" if jobs else "none"
    names = {crate["name"] for crate in result_crates}
    flags["engine"] = scope == "all" or "shamir-engine" in names
    flags["deny"] |= scope == "all"
    flags["ts"] |= scope == "all" or "shamir-query-types" in names
    flags["node"] = scope == "all" or bool(names & {"shamir-client", "shamir-client-node"})
    flags["wasm32"] = scope == "all" or bool(names & WASM_GUESTS)
    flags["docs_only"] = scope == "none" and bool(changed) and not flags["ts"] and not flags["deny"] and not flags["pipeline"]
    args = WORKSPACE if scope == "all" else " ".join(f"-p {name}" for name in sorted(names))
    return {"version": 1, "base": None, "head": None, "scope": scope,
            "all_reasons": sorted(set(all_reasons)), "crates": result_crates,
            "flags": flags, "args": args}


def production_paths(changed: list[str], metadata: dict, root: str,
                     content_provider=None, source_paths: list[str] | None = None,
                     production_overrides: set[str] | None = None) -> set[str]:
    dirs = manifest_dirs(metadata, root)
    production_overrides = production_overrides or set()
    selected = set()
    for path in changed:
        path = norm(path)
        pkg, _ = nearest_package(path, dirs)
        if path in production_overrides and pkg:
            selected.add(pkg["name"])
            continue
        if pkg and role(pkg, path, root, content_provider, source_paths)[0] == "production":
            selected.add(pkg["name"])
    return selected


def lockfile_data(text: str):
    return _lock_data(text)
