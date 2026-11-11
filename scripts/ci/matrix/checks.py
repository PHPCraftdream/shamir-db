"""Textual external-edge and test-path audits.

This scanner resolves literal Rust includes and `concat!(env!("CARGO_MANIFEST_DIR"), ...)` paths, and literal TS filesystem reads. It cannot prove macro expansion, build-script/proc-macro generated inputs, symlinks, cfg-specific expansion, arbitrary computed or non-literal concat paths, or dynamic TS paths. Literal repository-root discovery alone is not a data read; downstream dynamic binary paths are out of scope. Recognizable but unresolved reads fail closed. Scanning `concat!` may duplicate diagnostics for nested includes. An edge is valid only when its glob matches the target and its crates contains the reader; TypeScript readers may instead use flags=["ts"].
"""
from __future__ import annotations
import os
import re
import subprocess
import tomllib
from pathlib import PurePosixPath
from selection import edge_matches, load_edges, manifest_dirs, nearest_package, norm


def _edge_covers(edges, target, reader=None, ts=False):
    return any(edge_matches(edge, target) and
               ((reader is not None and reader in edge.get("crates", ())) or
                (ts and "ts" in edge.get("flags", ()))) for edge in edges)


def _macro_args(text, name):
    marker = re.compile(r"\b" + re.escape(name) + r"!\s*\(")
    for match in marker.finditer(text):
        depth, quote, escaped, index = 1, None, False, match.end()
        while index < len(text) and depth:
            char = text[index]
            if quote:
                if escaped: escaped = False
                elif char == "\\": escaped = True
                elif char == quote: quote = None
            elif char in "'\"": quote = char
            elif char == "(": depth += 1
            elif char == ")": depth -= 1
            index += 1
        yield (text[match.end():index - 1] if depth == 0 else None), match.start(), index


def _iter_macro_args(text, code, name):
    for expr, offset, end in _macro_args(text, name):
        if code[offset:offset + len(name)] == name:
            yield (expr, offset), end


def _include_path(expr):
    manifest = "CARGO_MANIFEST_DIR" in expr
    literal = re.compile(r'"((?:\\.|[^"\\])*)"')
    if manifest:
        marker = re.search(r'env!\s*\(\s*"CARGO_MANIFEST_DIR"\s*\)', expr)
        if not marker:
            return None, True
        prefix = "".join(literal.findall(expr[:marker.start()]))
        suffix = "".join(literal.findall(expr[marker.end():]))
        # The env! argument names the variable; it is not a path component.
        return prefix + suffix, True
    strings = literal.findall(expr)
    return ("".join(strings), False) if strings else (None, False)


def _code_mask(text, mask_literals=True):
    """Blank comments and optionally strings/chars while preserving offsets."""
    chars = list(text)
    i, n = 0, len(text)
    while i < n:
        if text.startswith("//", i):
            end = text.find("\n", i)
            end = n if end < 0 else end
            for j in range(i, end): chars[j] = " "
            i = end
        elif text.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if text.startswith("/*", j): depth += 1; j += 2
                elif text.startswith("*/", j): depth -= 1; j += 2
                else: j += 1
            for k in range(i, j):
                if chars[k] != "\n": chars[k] = " "
            i = j
        elif text[i] in "rbc" and (text[i] == "r" or text[i:i + 2] in {"br", "cr"}):
            prefix_end = i + (2 if text[i:i + 2] in {"br", "cr"} else 1)
            hashes = 0
            while prefix_end + hashes < n and text[prefix_end + hashes] == "#": hashes += 1
            quote = prefix_end + hashes
            if quote < n and text[quote] == '"':
                terminator = '"' + "#" * hashes
                end = text.find(terminator, quote + 1)
                end = n if end < 0 else end + len(terminator)
                for k in range(i, end):
                    if mask_literals and chars[k] != "\n": chars[k] = " "
                i = end
            else: i += 1
        elif text[i] == '"' or (text[i] == "'" and re.match(r"'(?:[^\\'\r\n]|\\(?:['\"\\nrt0]|x[0-9A-Fa-f]{2}|u\{[0-9A-Fa-f_]+\}))'", text[i:])):
            quote, j, escaped = text[i], i + 1, False
            while j < n:
                ch = text[j]
                if escaped: escaped = False
                elif ch == "\\": escaped = True
                elif ch == quote:
                    j += 1; break
                j += 1
            for k in range(i, j):
                if chars[k] != "\n": chars[k] = " "
            i = j
        else: i += 1
    return "".join(chars)


def _tracked_rust(root):
    raw = subprocess.run(["git", "ls-files", "-z", "--", "crates"], cwd=root, capture_output=True, check=False)
    if raw.returncode:
        raise RuntimeError("git ls-files failed: " + raw.stderr.decode("utf-8", "replace"))
    return sorted(item.decode("utf-8", "replace") for item in raw.stdout.split(b"\0") if item.endswith(b".rs"))


def _rust_external_refs(root, metadata, edges, content_provider=None, source_paths=None):
    dirs = manifest_dirs(metadata, root)
    members = {p["name"] for p in metadata.get("packages", ())}
    deps = {p["name"]: {d.get("name") for d in p.get("dependencies", ())}
            for p in metadata.get("packages", ()) if p["name"] in members}
    sources = source_paths if source_paths is not None else _tracked_rust(root)
    errors = []
    for source in sources:
        reader, _ = nearest_package(source, dirs)
        if not reader:
            continue
        text = content_provider(source) if content_provider else None
        if text is None and content_provider:
            errors.append(f"{source}:0: Rust source missing/unreadable in supplied snapshot")
            continue
        if text is None:
            try:
                with open(os.path.join(root, source), encoding="utf-8") as f: text = f.read()
            except OSError as exc:
                errors.append(f"{source}:0: cannot read Rust source: {exc}")
                continue
        code = _code_mask(text)
        for macro in ("include_str", "include_bytes", "concat"):
            for (expr, offset), _ in _iter_macro_args(text, code, macro):
                if macro == "concat" and (expr is None or "CARGO_MANIFEST_DIR" not in expr):
                    continue
                line = text.count("\n", 0, offset) + 1
                if expr is None:
                    errors.append(f"{source}:{line}: unterminated {macro}! target")
                    continue
                path, manifest_rel = _include_path(expr)
                if path is None:
                    errors.append(f"{source}:{line}: non-literal {macro}! target cannot be audited")
                    continue
                base = os.path.dirname(reader["manifest_path"]) if manifest_rel else os.path.dirname(os.path.join(root, source))
                resolved_path = base + path if manifest_rel else os.path.join(base, path)
                target_path = os.path.abspath(os.path.normpath(resolved_path))
                target = norm(os.path.relpath(target_path, root))
                try:
                    in_repo = os.path.commonpath([os.path.abspath(root), target_path]) == os.path.abspath(root)
                except ValueError:
                    in_repo = False
                target_pkg = nearest_package(target, dirs)[0] if in_repo else None
                external = (not in_repo or target_pkg is None or
                            (target_pkg["name"] != reader["name"] and target_pkg["name"] not in deps[reader["name"]]))
                if not os.path.isfile(target_path):
                    if external:
                        errors.append(f"{source}:{line}: external include target missing/unreadable: {target}")
                    continue
                if external and not _edge_covers(edges, target, reader["name"]):
                    errors.append(f"{source}:{line}: {reader['name']} reads external target {target} without Cargo edge or target-matched reader edge")
    return list(dict.fromkeys(errors))


def _ts_external_refs(root, edges):
    package_root = os.path.abspath(os.path.join(root, "crates/client/shamir-client-ts"))
    findings, seen = [], set()
    calls = re.compile(r"\b(?:readFileSync|readFile|existsSync|accessSync|resolve|join)\s*\(")
    literals = re.compile(r"(['\"`])((?:\\.|[^\\])*?)\1")
    for current, dirs, files in os.walk(package_root):
        dirs[:] = [d for d in dirs if d not in {"node_modules", "dist", "target"}]
        for filename in files:
            if not filename.endswith((".ts", ".tsx", ".js")): continue
            full = os.path.join(current, filename)
            source = norm(os.path.relpath(full, root))
            try: text = open(full, encoding="utf-8").read()
            except OSError as exc:
                finding = f"{source}:0: cannot read TS source: {exc}"
                if finding not in seen: findings.append(finding); seen.add(finding)
                continue
            for match in calls.finditer(text):
                depth, quote, escaped, end = 1, None, False, match.end()
                while end < len(text) and depth:
                    ch = text[end]
                    if quote:
                        if escaped: escaped = False
                        elif ch == "\\": escaped = True
                        elif ch == quote: quote = None
                    elif ch in "'\"`": quote = ch
                    elif ch == "(": depth += 1
                    elif ch == ")": depth -= 1
                    end += 1
                args = text[match.end():end - 1]
                clean_args, quote, escaped, cursor = [], None, False, 0
                while cursor < len(args):
                    ch = args[cursor]
                    if quote:
                        clean_args.append(ch)
                        if escaped: escaped = False
                        elif ch == "\\": escaped = True
                        elif ch == quote: quote = None
                        cursor += 1
                    elif ch in "'\"`":
                        quote = ch; clean_args.append(ch); cursor += 1
                    elif args.startswith("//", cursor):
                        newline = args.find("\n", cursor)
                        cursor = len(args) if newline < 0 else newline + 1
                        clean_args.append("\n") if newline >= 0 else None
                    else:
                        clean_args.append(ch); cursor += 1
                refs = [value for _, value in literals.findall("".join(clean_args))]
                first = next((index for index, value in enumerate(refs) if value.startswith("../")), None)
                if first is None: continue
                target_path = os.path.abspath(os.path.normpath(os.path.join(os.path.dirname(full), refs[first])))
                for suffix in refs[first + 1:]:
                    if suffix in {"utf8", "utf-8", "ascii", "buffer"}: continue
                    if not suffix or "/" in suffix or "\\" in suffix: continue
                    target_path = os.path.abspath(os.path.normpath(os.path.join(target_path, suffix)))
                try: inside = os.path.commonpath([os.path.normcase(package_root), os.path.normcase(target_path)]) == os.path.normcase(package_root)
                except ValueError: inside = False
                if inside: continue
                target = norm(os.path.relpath(target_path, root))
                if target == ".": continue
                if not os.path.exists(target_path) or not _edge_covers(edges, target, ts=True):
                    line = text.count("\n", 0, match.start()) + 1
                    status = "missing/unreadable " if not os.path.exists(target_path) else "undeclared "
                    finding = f"{source}:{line}: {status}external TS target {target}"
                    if finding not in seen: findings.append(finding); seen.add(finding)
    return findings


def _attrs_before(lines, index):
    attrs, cursor = [], index - 1
    while cursor >= 0:
        line = lines[cursor].strip()
        if not line or line.startswith(("//", "/*", "*", "*/")):
            cursor -= 1; continue
        if line.startswith("#[") or line.endswith("]"):
            attrs.append(line); cursor -= 1; continue
        break
    text = " ".join(reversed(attrs))
    # cfg(loom) is never active in a production build, like cfg(test)
    cfgs = re.findall(r"#\s*\[cfg\s*\(([^]]*)\)\s*\]", text)
    return attrs, any(re.search(r"\b(?:test|loom)\b", c) and "not" not in c for c in cfgs)


def _module_decl(text):
    for match in re.finditer(r"(?m)^[ \t]*(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;", text):
        yield match.group(1), match.start()


def _resolve_child(source_path, name, attrs, root, available=None):
    joined = " ".join(attrs)
    match = re.search(r'#\s*\[path\s*=\s*"([^"]+)"\s*\]', joined)
    if match:
        return os.path.normpath(os.path.join(os.path.dirname(source_path), match.group(1))).replace("\\", "/")
    directory = os.path.dirname(source_path)
    if PurePosixPath(source_path).name not in {"lib.rs", "main.rs", "mod.rs"}:
        directory = os.path.join(directory, PurePosixPath(source_path).stem)
    candidates = [os.path.join(directory, name + ".rs"), os.path.join(directory, name, "mod.rs")]
    candidates = [os.path.normpath(candidate).replace("\\", "/") for candidate in candidates]
    exists = (lambda candidate: candidate in available) if available is not None else (lambda candidate: os.path.isfile(os.path.join(root, candidate)))
    return next((candidate for candidate in candidates if exists(candidate)), None)


def unit_path_audit(root, metadata, source_paths, content_provider, edge_config):
    """Return actual path-gate errors and source files to classify as production."""
    findings, production, test_paths, visited = [], set(), set(), set()
    roots = []
    for pkg in metadata.get("packages", ()):
        for target in pkg.get("targets", ()):
            if target.get("src_path"):
                roots.append((target["src_path"], "test" in target.get("kind", ())))
    available = {norm(source) for source in source_paths}
    allowed = {norm(path) for path in edge_config.get("production_in_test_path", ())}
    def read(path):
        path = norm(os.path.relpath(path, root)) if os.path.isabs(path) else norm(path)
        return content_provider(path)
    def visit(path, inherited=False, unsafe=False):
        absolute = os.path.abspath(path if os.path.isabs(path) else os.path.join(root, path))
        state = (absolute, inherited, unsafe)
        if state in visited: return
        visited.add(state)
        rel = norm(os.path.relpath(absolute, root))
        if unsafe: production.add(rel)
        text = read(absolute)
        if text is None:
            findings.append(f"{rel}:0: module source missing/unreadable")
            return
        lines = text.splitlines()
        for name, offset in _module_decl(text):
            prefix = text[:offset].splitlines()
            attrs, own_test = _attrs_before(prefix, len(prefix))
            line = text.count("\n", 0, offset) + 1
            path_attr = next((m.group(1) for attr in attrs if (m := re.search(r'#\s*\[path\s*=\s*"([^"]+)"', attr))), None)
            child = _resolve_child(rel, name, attrs, root, available=available)
            looks_test = (name == "tests" or name.endswith("_tests") or
                          bool(path_attr and (
                              "tests" in path_attr.replace("\\", "/").split("/") or
                              PurePosixPath(path_attr.replace("\\", "/")).name.endswith("_tests.rs"))))
            cfg_test = inherited or own_test
            child_unsafe = looks_test and not cfg_test
            if child_unsafe:
                if not (child and norm(child) in allowed):
                    findings.append(f"{rel}:{line}: module {name} targets test-like path without cfg(test); treated production")
                production.add(rel)
            if child:
                if cfg_test: test_paths.add(norm(child))
                visit(child, cfg_test, unsafe or looks_test and not cfg_test)
    for root_source, inherited in roots:
        visit(root_source, inherited)
    return findings, production


def _top_entries(root):
    proc = subprocess.run(["git", "ls-files", "-z"], cwd=root, capture_output=True, check=False)
    if proc.returncode:
        raise RuntimeError("git ls-files failed: " + proc.stderr.decode("utf-8", "replace"))
    return sorted({item.split(b"/", 1)[0].decode("utf-8", "replace") for item in proc.stdout.split(b"\0") if item})


def run_checks(root, metadata, edge_text, content_provider=None, source_paths=None):
    """Return actual scanner failures; top-root fallback classifications are informational."""
    config = load_edges(edge_text)
    edges = config.get("edge", ())
    findings = []
    members = {pkg["name"] for pkg in metadata.get("packages", ())}
    for edge in edges:
        unknown = set(edge.get("crates", ())) - members
        findings.extend(f"edges.toml names unknown workspace crate {name}" for name in sorted(unknown))
        unsupported = set(edge.get("flags", ())) - {"ts"}
        findings.extend(f"edges.toml has unsupported edge flag {flag}" for flag in sorted(unsupported))
    sources = source_paths if source_paths is not None else _tracked_rust(root)
    if content_provider is None:
        def content_provider(path):
            try: return open(os.path.join(root, path), encoding="utf-8").read()
            except OSError: return None
    findings.extend(_rust_external_refs(root, metadata, edges, content_provider, sources))
    findings.extend(_ts_external_refs(root, edges))
    guard, _ = unit_path_audit(root, metadata, sources, content_provider, config)
    findings.extend(guard)
    known = {".cargo", ".config", ".github", ".rust-guard", "crates", "deploy", "docs", "scripts", "vendor", ".githooks", ".gitignore", ".gitattributes", ".git-blame-ignore-revs", "Cargo.lock", "Cargo.toml", "CLAUDE.md", "AGENTS.md", "README.md", "CHANGELOG.md", "CODE_OF_CONDUCT.md", "CONTRIBUTING.md", "LICENSE-APACHE", "LICENSE-MIT", "SECURITY.md", "deny.toml", "clippy.toml", "bench-iters.txt", "cooldown.toml", "cooldown-exceptions.toml", "rust-toolchain.toml", "rust-toolchain", ".claude", ".crush", "examples", "tests", "tools"}
    for entry in _top_entries(root):
        if entry not in known:
            findings.append(f"unclassified tracked top-level entry; selector must use all: {entry}")
    return findings
