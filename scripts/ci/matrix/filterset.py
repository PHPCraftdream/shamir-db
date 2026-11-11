#!/usr/bin/env python3
"""Plan CI consumers for the test-matrix contract."""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from typing import Any, Callable

NAME_RE = re.compile(r"[A-Za-z0-9_-]+")
RANGE_RE = re.compile(r"[A-Za-z0-9_./^~:@{}+%-]+\.\.[A-Za-z0-9_./^~:@{}+%-]+")
SHA_RE = re.compile(r"[0-9a-fA-F]{40}")
FLAG_NAMES = ("engine", "ts", "node", "wasm32", "deny", "pipeline", "docs_only")
CONSUMER_OUTPUTS = (
    "lib_args", "integration_args", "integration_filter", "check_args",
    "run_lib", "run_integration", "run_check", "run_engine", "run_ts",
    "run_node", "run_wasm32", "run_deny", "run_pipeline",
)


def _require_name(name: Any, what: str) -> str:
    if not isinstance(name, str) or NAME_RE.fullmatch(name) is None:
        raise ValueError("invalid %s name: %r" % (what, name))
    return name


def workspace_packages(metadata: dict[str, Any]) -> dict[str, dict[str, Any]]:
    packages = metadata.get("packages")
    if not isinstance(packages, list):
        raise ValueError("cargo metadata packages must be a list")
    workspace_ids = metadata.get("workspace_members")
    if workspace_ids is not None:
        if not isinstance(workspace_ids, list) or any(not isinstance(item, str) for item in workspace_ids):
            raise ValueError("cargo metadata workspace_members must be a list of package IDs")
        package_ids = [package.get("id") if isinstance(package, dict) else None for package in packages]
        if any(package_id is None for package_id in package_ids):
            raise ValueError("cargo metadata contains a malformed package id")
        if not set(workspace_ids) <= set(package_ids):
            raise ValueError("cargo metadata workspace_members references missing packages")
        member_ids = set(workspace_ids)
        packages = [package for package in packages if package["id"] in member_ids]
    result: dict[str, dict[str, Any]] = {}
    for package in packages:
        if not isinstance(package, dict):
            raise ValueError("cargo metadata contains a malformed package")
        name = _require_name(package.get("name"), "workspace package")
        if name in result:
            raise ValueError("duplicate workspace package: %s" % name)
        result[name] = package
    return result


def _matrix_args(names: list[str]) -> str:
    return " ".join("-p %s" % name for name in sorted(names))


def _target_names(package: dict[str, Any]) -> set[str]:
    targets = package.get("targets", [])
    if not isinstance(targets, list):
        raise ValueError("cargo metadata targets must be a list")
    names: set[str] = set()
    for target in targets:
        if not isinstance(target, dict):
            raise ValueError("cargo metadata contains a malformed target")
        kinds = target.get("kind", [])
        if not isinstance(kinds, list):
            raise ValueError("cargo metadata target kind must be a list")
        if "test" not in kinds:
            continue
        name = _require_name(target.get("name"), "integration target")
        if name in names:
            raise ValueError("duplicate integration target %s in %s" % (name, package["name"]))
        names.add(name)
    return names


def _has_lib_target(package: dict[str, Any]) -> bool:
    targets = package.get("targets", [])
    if not isinstance(targets, list):
        raise ValueError("cargo metadata targets must be a list")
    for target in targets:
        if not isinstance(target, dict) or not isinstance(target.get("kind", []), list):
            raise ValueError("cargo metadata contains a malformed target")
        if "lib" in target["kind"] or "proc-macro" in target["kind"]:
            return True
    return False


def validate_matrix(matrix: Any, metadata: dict[str, Any]) -> dict[str, Any]:
    if not isinstance(matrix, dict):
        raise ValueError("matrix must be a JSON object")
    if type(matrix.get("version")) is not int or matrix["version"] != 1:
        raise ValueError("unsupported matrix version")
    if matrix.get("scope") not in ("none", "bounded", "all"):
        raise ValueError("invalid matrix scope")
    for field in ("base", "head"):
        if not isinstance(matrix.get(field), str):
            raise ValueError("matrix %s must be a string" % field)
    all_reasons = matrix.get("all_reasons")
    if not isinstance(all_reasons, list) or any(not isinstance(item, str) for item in all_reasons):
        raise ValueError("matrix all_reasons must be a list of strings")
    packages = workspace_packages(metadata)
    rows = matrix.get("crates")
    if not isinstance(rows, list):
        raise ValueError("matrix crates must be a list")
    names: list[str] = []
    for row in rows:
        if not isinstance(row, dict):
            raise ValueError("matrix crate row must be an object")
        name = _require_name(row.get("name"), "crate")
        if name not in packages:
            raise ValueError("unknown workspace crate: %s" % name)
        if name in names:
            raise ValueError("duplicate matrix crate: %s" % name)
        names.append(name)
        if type(row.get("lib")) is not bool or type(row.get("check_only")) is not bool:
            raise ValueError("crate %s lib/check_only must be booleans" % name)
        integration = row.get("integration")
        if integration != "all" and not isinstance(integration, list):
            raise ValueError("crate %s integration must be 'all' or a list" % name)
        targets = _target_names(packages[name])
        if integration != "all":
            seen: set[str] = set()
            for target in integration:
                target = _require_name(target, "integration target")
                if target in seen:
                    raise ValueError("duplicate integration target %s in %s" % (target, name))
                if target not in targets:
                    raise ValueError("unknown integration target %s in %s" % (target, name))
                seen.add(target)
        if row["check_only"] and (row["lib"] or integration == "all" or bool(integration)):
            raise ValueError("crate %s check_only row must not select tests" % name)
        reasons = row.get("reasons")
        if not isinstance(reasons, list) or not reasons or any(not isinstance(item, str) or not item for item in reasons):
            raise ValueError("crate %s must have at least one reason" % name)
        if not row["lib"] and not row["check_only"] and integration != "all" and not integration:
            raise ValueError("crate %s has no selected checks or tests" % name)
    flags = matrix.get("flags")
    if not isinstance(flags, dict):
        raise ValueError("matrix flags must be an object")
    if set(flags) != set(FLAG_NAMES):
        raise ValueError("matrix flags have missing or unknown keys")
    if any(type(flags[name]) is not bool for name in FLAG_NAMES):
        raise ValueError("matrix flags must be booleans")
    matrix["flags"] = dict(flags)
    if matrix["scope"] == "all" and set(names) != set(packages):
        raise ValueError("scope=all matrix must contain every workspace crate")
    if matrix["scope"] == "none" and names:
        raise ValueError("scope=none matrix cannot contain crate rows")
    args = matrix.get("args")
    if not isinstance(args, str):
        raise ValueError("matrix args must be a string")
    if args == "--workspace":
        if matrix["scope"] != "all":
            raise ValueError("--workspace args require scope=all")
        matrix["args"] = _matrix_args(names)
        return matrix
    tokens = args.split()
    if len(tokens) % 2 or any(tokens[index] != "-p" for index in range(0, len(tokens), 2)):
        raise ValueError("matrix args must be a sequence of -p package pairs")
    arg_names = tokens[1::2]
    for name in arg_names:
        _require_name(name, "matrix package")
    if len(set(arg_names)) != len(arg_names) or set(arg_names) != set(names):
        raise ValueError("matrix args do not match its validated crate rows")
    return matrix


def integration_filter(matrix: dict[str, Any], metadata: dict[str, Any]) -> str:
    matrix = validate_matrix(matrix, metadata)
    packages = workspace_packages(metadata)
    terms: list[str] = []
    for row in matrix["crates"]:
        selected = _target_names(packages[row["name"]]) if row["integration"] == "all" else set(row["integration"])
        for target in sorted(selected):
            terms.append("(kind(test) and package(=%s) and binary(=%s))" % (row["name"], target))
    return " or ".join(terms)


def consume(matrix: dict[str, Any], metadata: dict[str, Any]) -> dict[str, Any]:
    matrix = validate_matrix(matrix, metadata)
    rows = matrix["crates"]
    all_names = sorted(row["name"] for row in rows)
    matrix = dict(matrix)
    matrix["args"] = _matrix_args(all_names)
    lib_names = sorted(row["name"] for row in rows if row["lib"])
    integration_names = sorted(
        row["name"] for row in rows if not row["check_only"] and (
            bool(_target_names(workspace_packages(metadata)[row["name"]]))
            if row["integration"] == "all" else bool(row["integration"])
        )
    )
    flags = dict(matrix["flags"])
    result: dict[str, Any] = {
        "matrix": json.dumps(matrix, sort_keys=True, separators=(",", ":")),
        "args": _matrix_args(all_names),
        "flags": flags,
        "lib_args": _matrix_args(lib_names),
        "integration_args": _matrix_args(integration_names),
        "integration_filter": integration_filter(matrix, metadata),
        "check_args": _matrix_args(all_names),
        "run_lib": bool(lib_names),
        "run_integration": bool(integration_names),
        "run_check": bool(all_names),
        "run_engine": flags["engine"],
        "run_ts": flags["ts"],
        "run_node": flags["node"],
        "run_wasm32": flags["wasm32"],
        "run_deny": flags["deny"],
        "run_pipeline": flags["pipeline"],
    }
    if flags["pipeline"]:
        result.update({"run_engine": True, "run_ts": True, "run_node": True,
                       "run_wasm32": True, "run_deny": True})
    return result


def _manual_flags(names: set[str], workspace: bool = False) -> dict[str, bool]:
    wasm_crates = {"shamir-sdk", "shamir-query-builder", "shamir-query-types", "shamir-types"}
    return {
        "engine": workspace or "shamir-engine" in names,
        "ts": workspace or bool(names & {"shamir-query-types", "shamir-client-ts"}),
        "node": workspace or bool(names & {"shamir-client", "shamir-client-node"}),
        "wasm32": workspace or bool(names & wasm_crates),
        "deny": workspace,
        "pipeline": workspace,
        "docs_only": False,
    }


def _fixture(names: list[str], metadata: dict[str, Any], reason: str, *, workspace: bool = False) -> dict[str, Any]:
    packages = workspace_packages(metadata)
    unknown = sorted(set(names) - set(packages))
    if unknown:
        raise ValueError("unknown workspace crate(s): %s" % ", ".join(unknown))
    selected = sorted(set(names))
    rows = []
    for name in selected:
        package = packages[name]
        targets = _target_names(package)
        has_lib = _has_lib_target(package)
        if reason.startswith("weekly schedule") and name in {"shamir-server", "shamir-e2e"}:
            if not has_lib:
                raise ValueError("scheduled crate %s has no lib target" % name)
            if not targets:
                raise ValueError("scheduled crate %s has no integration test targets" % name)
        rows.append({
            "name": name,
            "lib": has_lib,
            "integration": "all" if targets else [],
            "check_only": not has_lib and not targets,
            "reasons": [reason],
        })
    return {
        "version": 1,
        "base": "",
        "head": "HEAD",
        "scope": "all" if workspace else "bounded",
        "all_reasons": [reason] if workspace else [],
        "crates": rows,
        "flags": _manual_flags(set(selected), workspace),
        "args": _matrix_args(selected),
    }


def _dispatch_values(event_data: dict[str, Any], range_input: str | None,
                     crates_input: str | None, scope_input: str | None) -> tuple[str, str | None, str]:
    inputs = event_data.get("inputs", {})
    if not isinstance(inputs, dict):
        raise ValueError("workflow_dispatch inputs must be an object")
    range_value = range_input if range_input is not None else inputs.get("range", "")
    crates_value = crates_input if crates_input is not None else inputs.get("crates", "")
    scope_value = scope_input if scope_input is not None else inputs.get("scope", "computed")
    if not isinstance(range_value, str) or not isinstance(crates_value, str) or not isinstance(scope_value, str):
        raise ValueError("workflow_dispatch range, crates and scope inputs must be strings")
    range_value = range_value.strip() or "HEAD^..HEAD"
    if RANGE_RE.fullmatch(range_value) is None or range_value.count("..") != 1 or "..." in range_value or range_value.startswith("-"):
        raise ValueError("workflow_dispatch range must be a two-dot revision range")
    crates_value = crates_value.strip()
    scope_value = scope_value.strip() or "computed"
    if scope_value not in ("computed", "workspace"):
        raise ValueError("workflow_dispatch scope must be computed or workspace")
    if crates_value and scope_value == "workspace":
        raise ValueError("workflow_dispatch crates and scope=workspace are mutually exclusive")
    return range_value, crates_value or None, scope_value


def _run_stage1(event: str, event_data: dict[str, Any], metadata: dict[str, Any],
                runner: Callable[..., Any], *, range_input: str | None = None,
                metadata_file: str | None = None) -> dict[str, Any]:
    script = "scripts/ci/matrix/test_matrix.py"
    argv = [sys.executable, "-I", script]
    if metadata_file is not None:
        argv.extend(("--metadata", metadata_file))
    if event == "pull_request":
        argv.extend(("--event", "pull_request", "--base", "origin/master", "--head", "HEAD"))
    elif event == "push":
        before, after = event_data.get("before"), event_data.get("after")
        if not isinstance(before, str) or SHA_RE.fullmatch(before) is None or not isinstance(after, str) or SHA_RE.fullmatch(after) is None:
            raise ValueError("push event requires before and after commit SHAs")
        argv.extend(("--event", "push", "--before", before, "--after", after))
    elif event == "workflow_dispatch":
        argv.extend(("--event", "workflow_dispatch", "--range", range_input or "HEAD^..HEAD"))
    else:
        raise ValueError("stage1 is not invoked for schedule events")
    argv.extend(("--format", "github-output"))
    completed = runner(argv, capture_output=True, text=True, check=False)
    if completed.returncode != 0:
        raise RuntimeError("test_matrix.py failed (exit %d): %s" % (completed.returncode, completed.stderr.strip()))
    fields: dict[str, str] = {}
    for line in completed.stdout.splitlines():
        if not line or "=" not in line:
            raise ValueError("malformed test_matrix.py github-output line")
        key, value = line.split("=", 1)
        if key in fields:
            raise ValueError("duplicate test_matrix.py output: %s" % key)
        fields[key] = value
    expected_keys = {"matrix", "args", *FLAG_NAMES}
    if set(fields) != expected_keys:
        raise ValueError("test_matrix.py matrix contract github-output keys are incomplete")
    matrix = json.loads(fields["matrix"])
    if not isinstance(matrix, dict):
        raise ValueError("test_matrix.py matrix output must be a JSON object")
    if matrix.get("args") != fields["args"]:
        raise ValueError("test_matrix.py args output disagrees with matrix")
    for name in FLAG_NAMES:
        if name not in fields:
            continue
        value = fields[name]
        if value not in ("true", "false") or matrix.get("flags", {}).get(name) != (value == "true"):
            raise ValueError("test_matrix.py %s output disagrees with matrix" % name)
    return validate_matrix(matrix, metadata)


def plan(event: str, event_data: dict[str, Any], metadata: dict[str, Any], *,
         runner: Callable[..., Any] = subprocess.run, range_input: str | None = None,
         crates_input: str | None = None, scope_input: str | None = None,
         metadata_file: str | None = None) -> dict[str, Any]:
    if event not in ("pull_request", "push", "workflow_dispatch", "schedule"):
        raise ValueError("unsupported event: %s" % event)
    if not isinstance(event_data, dict):
        raise ValueError("event JSON must be an object")
    workspace = workspace_packages(metadata)
    if event == "schedule":
        matrix = _fixture(["shamir-server", "shamir-e2e"], metadata,
                          "weekly schedule: top-crate tests")
    elif event == "workflow_dispatch":
        dispatch_range, crates, scope = _dispatch_values(event_data, range_input, crates_input, scope_input)
        if scope == "workspace":
            matrix = _fixture(sorted(workspace), metadata, "manual scope=workspace", workspace=True)
        elif crates:
            names = crates.split()
            for name in names:
                _require_name(name, "manual crate")
            if len(set(names)) != len(names):
                raise ValueError("duplicate manual crate input")
            matrix = _fixture(names, metadata, "manual crate selection")
        else:
            matrix = _run_stage1(event, event_data, metadata, runner, range_input=dispatch_range,
                                 metadata_file=metadata_file)
    else:
        matrix = _run_stage1(event, event_data, metadata, runner, metadata_file=metadata_file)
    return {"matrix_data": matrix, **consume(matrix, metadata)}


def _load_metadata(path: str | None) -> dict[str, Any]:
    if path:
        with open(path, encoding="utf-8") as stream:
            return json.load(stream)
    completed = subprocess.run(
        ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"],
        capture_output=True, text=True, check=False,
    )
    if completed.returncode:
        raise RuntimeError("cargo metadata failed (exit %d): %s" % (completed.returncode, completed.stderr.strip()))
    return json.loads(completed.stdout)


def _read_event(path: str | None) -> dict[str, Any]:
    path = path or os.environ.get("GITHUB_EVENT_PATH")
    if not path:
        return {}
    with open(path, encoding="utf-8") as stream:
        event_data = json.load(stream)
    if not isinstance(event_data, dict):
        raise ValueError("event JSON must be an object")
    return event_data


def github_output(result: dict[str, Any]) -> str:
    lines = ["matrix=" + result["matrix"], "args=" + result["args"]]
    lines.extend("%s=%s" % (name, str(result["flags"][name]).lower()) for name in FLAG_NAMES)
    lines.extend("%s=%s" % (name, result[name]) for name in (
        "lib_args", "integration_args", "integration_filter", "check_args",
    ))
    lines.extend("%s=%s" % (name, str(result[name]).lower()) for name in CONSUMER_OUTPUTS[4:])
    return "\n".join(lines)


def _load_matrix(path: str) -> dict[str, Any]:
    with open(path, encoding="utf-8") as stream:
        text = stream.read()
    try:
        value = json.loads(text)
    except json.JSONDecodeError:
        fields: dict[str, str] = {}
        for line in text.splitlines():
            if "=" not in line:
                raise ValueError("malformed matrix input")
            key, value = line.split("=", 1)
            if key in fields:
                raise ValueError("duplicate matrix input key: %s" % key)
            fields[key] = value
        if "matrix" not in fields:
            raise ValueError("matrix input has no matrix field")
        value = json.loads(fields["matrix"])
    if isinstance(value, dict) and isinstance(value.get("matrix"), str) and "crates" not in value:
        value = json.loads(value["matrix"])
    return value


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    plan_parser = commands.add_parser("plan")
    plan_parser.add_argument("--event-file", default=None)
    plan_parser.add_argument("--event", default=os.environ.get("GITHUB_EVENT_NAME", "pull_request"),
                              choices=("pull_request", "push", "workflow_dispatch", "schedule"))
    plan_parser.add_argument("--metadata", default=None)
    plan_parser.add_argument("--range", dest="range_input", default=None)
    plan_parser.add_argument("--crates", default=None)
    plan_parser.add_argument("--scope", default=None, choices=("computed", "workspace"))
    plan_parser.add_argument("--format", choices=("github-output",), default="github-output")
    consume_parser = commands.add_parser("consume")
    consume_parser.add_argument("--matrix-file", required=True)
    consume_parser.add_argument("--metadata", default=None)
    consume_parser.add_argument("--format", choices=("github-output",), default="github-output")
    args = parser.parse_args(argv)
    try:
        metadata = _load_metadata(args.metadata)
        if args.command == "plan":
            result = plan(args.event, _read_event(args.event_file), metadata,
                          range_input=args.range_input, crates_input=args.crates,
                          scope_input=args.scope, metadata_file=args.metadata)
        else:
            matrix = validate_matrix(_load_matrix(args.matrix_file), metadata)
            result = {"matrix_data": matrix, **consume(matrix, metadata)}
        print(github_output(result))
    except (OSError, RuntimeError, ValueError, KeyError, json.JSONDecodeError) as exc:
        print("filterset.py: %s" % exc, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
