#!/usr/bin/env python3
"""Manual CI-only paired benchmarks; no third-party Python dependencies."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import re
import shutil
import signal
import statistics
import subprocess
import sys
import time
import tomllib

BASELINE = "e11064772960e06c24b97379ea637335a286382e"
ROOT = Path(__file__).resolve().parent.parent
SHA = re.compile(r"[0-9a-f]{40}\Z")


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def save(path, value):
    Path(path).write_text(json.dumps(value, indent=2, allow_nan=False) + "\n")


def load(path):
    return json.loads(Path(path).read_text())


def git(repo, *args):
    return subprocess.check_output(["git", "-C", str(repo), *args], text=True).strip()


def validate_sha(value):
    if not SHA.fullmatch(value):
        raise ValueError("Revision must be an exact lowercase 40-character SHA")
    return value


def load_counts(path):
    counts = {}
    for line in Path(path).read_text().splitlines():
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        match = re.fullmatch(r"\s*(\S+::\S+)\s*=\s*([0-9]+)\s*", line)
        if not match or match[1] in counts or int(match[2]) <= 0:
            raise ValueError(f"Invalid/duplicate iteration entry: {line}")
        counts[match[1]] = int(match[2])
    return counts


def load_manifest(path):
    with Path(path).open("rb") as stream:
        manifest = tomllib.load(stream)
    if manifest["version"] != 1 or type(manifest["scale"]) is not int or manifest["scale"] < 1:
        raise ValueError("Unsupported manifest version/scale")
    seen = set()
    for bench in manifest["benches"]:
        if bench["class"] not in manifest["thresholds"] or bench["rank"] not in range(1, 8):
            raise ValueError("Invalid class/rank")
        for cell, patterns in bench.get("source_prerequisites", {}).items():
            if cell not in bench["cells"] or not patterns:
                raise ValueError("Invalid source prerequisite selection")
            for pattern in patterns:
                re.compile(pattern)
        for cell in bench["cells"]:
            key = bench["bench"] + "::" + cell
            if key in seen or not re.fullmatch(r"[A-Za-z0-9_/=.-]+", cell):
                raise ValueError("Duplicate/invalid selected cell")
            seen.add(key)
    for thresholds in manifest["thresholds"].values():
        for mode in ("hosted", "dedicated"):
            if not 0 < thresholds[mode] < 100:
                raise ValueError("Invalid margin")
    return manifest


def preflight_cell(bench, cell, sources, counts, operation):
    """Pure static optional-cell decision; cached counts never prove registration.

    Prerequisite regexes are reviewed narrow source-shape checks, not a Rust
    interpreter. Unknown shapes stay unsupported until the manifest is reviewed.
    Both immutable checkouts must support the exact requested cell.
    """
    key = bench["bench"] + "::" + cell
    if bench.get("optional"):
        patterns = bench.get("source_prerequisites", {}).get(cell, [])
        if not patterns:
            return "Optional cell has no reviewed exact source prerequisite"
        # Comments are not registration evidence. These checks intentionally only
        # recognize the simple, unconditional registrations declared in the TOML.
        code = [re.sub(r"/\*.*?\*/|//[^\n]*", "", source, flags=re.S)
                for source in sources]
        if not code or not all(all(re.search(pattern, source, re.S) for pattern in patterns)
                               for source in code):
            return "Optional exact cell is not registered by reviewed source prerequisites on both revisions"
    if key not in counts and operation != "calibrate":
        if bench.get("optional"):
            return "Optional cell lacks reviewed iteration count; use separate calibration dispatch"
        raise ValueError(f"Selected required cell lacks reviewed iterations: {key}")
    return None


def unmeasured_rows(result):
    measured = {cell["key"] for cell in result["cells"]}
    reason = "; ".join(result["errors"]) or (
        "Calibration only; no comparison observations" if
        result.get("plan", {}).get("operation") == "calibrate" else
        "Build/run did not produce complete paired observations")
    return [{"cell": cell["key"], "reason": reason}
            for cell in result.get("selected", []) if cell["key"] not in measured]


def parse_output(text, expected):
    """Reject malformed data, calibration, duplicates, stray and missing cells."""
    rows = {}
    if re.search(r"calibrat|manifest (?:updated|written)", text, re.I):
        raise ValueError("Unexpected calibration/manifest mutation")
    for line in text.splitlines():
        tokens = line.split()
        if not tokens:
            continue
        if tokens[0] not in expected and not any(t in tokens for t in ("iters", "ns/op")):
            continue
        if len(tokens) != 7 or tokens[2::2] != ["iters", "ms", "ns/op"]:
            raise ValueError(f"Malformed measurement: {line}")
        cell, count, _, ms, _, ns, _ = tokens
        if cell not in expected or cell in rows or not re.fullmatch(r"[0-9]+", count):
            raise ValueError(f"Unexpected/duplicate cell or count: {line}")
        count, ms, ns = int(count), float(ms), float(ns)
        if count != expected[cell] or not math.isfinite(ms) or ms < 0 or \
                not math.isfinite(ns) or ns <= 0:
            raise ValueError(f"Invalid timing/count: {line}")
        # Harness prints milliseconds to 3 decimals, ns/op to 2 decimals.
        if abs(ms * 1e6 - ns * count) > 501 + 0.0051 * count:
            raise ValueError("Inconsistent elapsed time and ns/op")
        rows[cell] = {"iters": count, "ms": ms, "ns": ns}
    if set(rows) != set(expected):
        raise ValueError(f"Missing cells: {set(expected) - set(rows)}")
    return rows


def paired(a, b, margin):
    if len(a) != 5 or len(b) != 5:
        raise ValueError("Exactly five complete pairs required per batch")
    ratios = []
    for left, right in zip(a, b):
        if left["iters"] != right["iters"]:
            raise ValueError("Unequal A/B iteration counts")
        if not all(math.isfinite(v) and v > 0 for v in (left["ns"], right["ns"])):
            raise ValueError("Invalid paired timing")
        ratios.append(right["ns"] / left["ns"])
    median = statistics.median(ratios)
    spread = 100 * 1.4826 * statistics.median(abs(r - median) for r in ratios)
    change = 100 * (median - 1)
    status = "inconclusive" if spread > margin / 2 else (
        "flagged" if change > margin else "within margin")
    return {"a_ns": statistics.median(x["ns"] for x in a),
            "b_ns": statistics.median(x["ns"] for x in b), "ratios": ratios,
            "change_pct": change, "spread_pct": spread, "pairs": 5, "status": status}


def policy(batches, mode):
    if mode == "dedicated" and len(batches) != 2:
        return "inconclusive"
    statuses = [b["status"] for b in batches]
    if "inconclusive" in statuses:
        return "inconclusive"
    if all(s == "flagged" for s in statuses):
        return "flagged"
    if all(s == "within margin" for s in statuses):
        return "within margin"
    return "inconclusive"


def prepare(args):
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    validate_sha(args.baseline)
    if args.baseline != BASELINE:
        raise ValueError("Baseline is fixed by the reviewed experiment")
    # Read this trusted ref exactly once. The workflow fetches master, not fork refs.
    master = validate_sha(git(ROOT, "rev-parse", "refs/remotes/origin/master"))
    candidate = validate_sha(os.environ.get(args.candidate_env, "") or master)
    for revision in (args.baseline, candidate):
        if git(ROOT, "rev-parse", revision + "^{commit}") != revision:
            raise ValueError("Revision is not an exact commit identity")
        subprocess.run(["git", "-C", str(ROOT), "merge-base", "--is-ancestor",
                        revision, master], check=True)
    manifest_path = ROOT / "scripts/bench_ab.toml"
    manifest = load_manifest(manifest_path)
    counts_path = ROOT / "bench-iters.txt"
    counts = load_counts(counts_path)
    shutil.copyfile(manifest_path, out / "bench_ab.toml")
    shutil.copyfile(counts_path, out / "iterations.txt")
    plan = {"base_sha": args.baseline, "candidate_sha": candidate, "master_sha": master,
            "control_sha": git(ROOT, "rev-parse", "HEAD"), "mode": args.mode,
            "operation": args.operation, "batches": 2 if args.mode == "dedicated" else 1,
            "manifest": manifest, "counts": counts, "scale": manifest["scale"],
            "manifest_digest": digest(out / "bench_ab.toml"),
            "iteration_digest": digest(out / "iterations.txt"), "suite": "owed-small"}
    save(out / "plan.json", plan)
    if os.environ.get("GITHUB_OUTPUT"):
        with open(os.environ["GITHUB_OUTPUT"], "a") as stream:
            for key in ("base_sha", "candidate_sha"):
                stream.write(f"{key}={plan[key]}\n")


def execute(argv, cwd, env, prefix, timeout):
    """Bound the entire process group, retaining output even on timeout."""
    started = time.monotonic()
    with prefix.with_suffix(".stdout.log").open("w") as stdout, \
            prefix.with_suffix(".stderr.log").open("w") as stderr:
        process = subprocess.Popen(argv, cwd=cwd, env=env, stdout=stdout, stderr=stderr,
                                   start_new_session=True)
        try:
            code = process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
            save(prefix.with_suffix(".command.json"),
                 {"argv": [str(x) for x in argv], "seconds": time.monotonic() - started,
                  "exit": process.returncode, "timeout": True, "cwd": str(cwd)})
            raise ValueError(f"Timeout: {prefix.name}")
    save(prefix.with_suffix(".command.json"),
         {"argv": [str(x) for x in argv], "seconds": time.monotonic() - started,
          "exit": code, "cwd": str(cwd)})
    if code:
        raise ValueError(f"Process failed ({code}): {prefix.name}")
    return prefix.with_suffix(".stdout.log").read_text()


def run(args):
    out = args.plan.resolve().parent
    plan = load(args.plan)
    result = {"plan": plan, "cells": [], "errors": [], "complete": False,
              "unsupported": list(plan["manifest"]["unsupported"])}
    save(out / "results.json", result)
    try:
        repos = {"A": args.a.resolve(), "B": args.b.resolve()}
        targets = {side: args.target_root.resolve() / side for side in repos}
        env = {k: v for k, v in os.environ.items() if not k.startswith("BENCH_")}
        env.update(CARGO_TERM_COLOR="never", RUST_BACKTRACE="1", RUSTFLAGS="",
                   TOKIO_WORKER_THREADS="2", RAYON_NUM_THREADS="2",
                   RUSTUP_TOOLCHAIN="1.95.0")
        metadata = {"platform": platform.platform(), "cpu_count": os.cpu_count(),
                    "cpuinfo": Path("/proc/cpuinfo").read_text(),
                    "meminfo": Path("/proc/meminfo").read_text(),
                    "image": os.environ.get("ImageVersion"), "profile": "release",
                    "features": "crate defaults", "threads": 2, "rustflags": "",
                    "compiler": execute(["rustc", "-Vv"], ROOT, env, out / "compiler", 30)}
        save(out / "machine.json", metadata)
        for side, repo in repos.items():
            expected_sha = plan["base_sha" if side == "A" else "candidate_sha"]
            if git(repo, "rev-parse", "HEAD") != expected_sha:
                raise ValueError("Checkout identity mismatch")
            shutil.copyfile(out / "iterations.txt", repo / "bench-iters.txt")
        selected = []
        result["selected"] = selected
        for bench in sorted(plan["manifest"]["benches"], key=lambda b: b["rank"]):
            name = bench["bench"]
            if bench.get("coverage_only"):
                result["unsupported"].append({"cell": name + "::future fixtures",
                                              "reason": "Optional snapshot/vector or rejecting-sink family needs reviewed frozen adapters and explicit cell/count selection"})
                continue
            paths = [repo / "crates" / bench["crate"] / "benches" / (name + ".rs")
                     for repo in repos.values()]
            cells = bench["cells"]
            # Never overlay candidate production code on A. A differing measurement
            # fixture needs a separately reviewed frozen adapter, outside this helper.
            reason = None
            if not all(p.is_file() for p in paths):
                reason = "Benchmark fixture absent on one or both immutable revisions"
            elif digest(paths[0]) != digest(paths[1]):
                reason = "Fixture source differs; no reviewed frozen cross-revision adapter"
            elif not cells:
                reason = "No explicit reviewed cell/source selection for optional future fixture"
            if reason:
                for cell in cells or ["future fixture"]:
                    result["unsupported"].append({"cell": name + "::" + cell, "reason": reason})
                continue
            sources = [path.read_text() for path in paths]
            for cell in cells:
                key = name + "::" + cell
                reason = preflight_cell(bench, cell, sources, plan["counts"], plan["operation"])
                if reason:
                    result["unsupported"].append({"cell": key, "reason": reason})
                    continue
                selected.append({**bench, "cell": cell, "key": key,
                                 "fixture_digest": digest(paths[0])})
        result["selected"] = selected
        for side, repo in repos.items():
            metadata[side] = {"source_sha": git(repo, "rev-parse", "HEAD"),
                              "lockfile_digest": digest(repo / "Cargo.lock")
                              if (repo / "Cargo.lock").exists() else None,
                              "profile_manifest_digest": digest(repo / "Cargo.toml")}
        save(out / "machine.json", metadata)
        save(out / "results.json", result)
        binaries = {}
        # Finish ALL A and B release builds before any warmup or timed process.
        for side, repo in repos.items():
            side_env = {**env, "CARGO_TARGET_DIR": str(targets[side])}
            for crate, name in sorted({(b["crate"], b["bench"]) for b in selected}):
                text = execute(["cargo", "bench", "--locked", "--profile=release", "--no-run",
                                "-p", crate, "--bench", name, "--message-format=json"],
                               repo, side_env, out / f"build-{side}-{name}", 2400)
                artifacts = [json.loads(line) for line in text.splitlines() if line.startswith("{")]
                executables = {x["executable"] for x in artifacts
                               if x.get("reason") == "compiler-artifact" and x.get("executable")
                               and x["target"]["name"] == name and "bench" in x["target"]["kind"]}
                if len(executables) != 1:
                    raise ValueError(f"Missing/ambiguous built executable: {side}/{name}")
                binaries[side, name] = executables.pop()
        result["selected"] = selected
        save(out / "results.json", result)
        for index, bench in enumerate(selected):
            key, name, cell = bench["key"], bench["bench"], bench["cell"]
            if plan["operation"] == "calibrate":
                text = execute([binaries["B", name], "--calibrate", "1", key], repos["B"], env,
                               out / f"calibrate-{index}", 600)
                # Cached stale counts cannot hide a no-match calibration process.
                if len(re.findall(r"^\s*" + re.escape(key) + r" -> [1-9][0-9]* iters\s*$",
                                  text, re.M)) != 1:
                    raise ValueError(f"Calibration did not produce the selected exact cell: {key}")
                if key not in load_counts(repos["B"] / "bench-iters.txt"):
                    raise ValueError(f"Calibration did not save the selected count: {key}")
                continue
            # The real harness uses SUBSTRING filtering, not exact equality.
            # Validate every matched cached row, including /1000 when selecting /100.
            expected = {k.split("::", 1)[1]: n * plan["scale"]
                        for k, n in plan["counts"].items()
                        if k.startswith(name + "::") and key in k}
            observations = []
            for batch in range(plan["batches"]):
                samples = {"A": [], "B": []}
                for pair in range(-1, 5):
                    order = "AB" if pair < 0 or pair % 2 == 0 else "BA"
                    for side in order:
                        repo = repos[side]
                        if digest(repo / "bench-iters.txt") != plan["iteration_digest"]:
                            raise ValueError("Iteration digest changed before invocation")
                        text = execute([binaries[side, name], "--scale", str(plan["scale"]), key],
                                       repo, env, out / f"cell-{index}-batch-{batch}-pair-{pair}-{side}", 600)
                        if digest(repo / "bench-iters.txt") != plan["iteration_digest"]:
                            raise ValueError("Iteration digest changed after invocation")
                        rows = parse_output(text, expected)
                        if pair >= 0:
                            samples[side].append(rows[cell])
                margin = plan["manifest"]["thresholds"][bench["class"]][plan["mode"]]
                observations.append({**paired(samples["A"], samples["B"], margin),
                                     "observations": samples, "batch": batch + 1})
            status = policy(observations, plan["mode"])
            # Frozen one-iteration counts are smoke, not tight certification evidence.
            if expected[cell] < 2:
                status = "inconclusive"
            result["cells"].append({**bench, "batches": observations, "status": status,
                                    "actual_iters": expected[cell], "scale": plan["scale"]})
            save(out / "results.json", result)
        if plan["operation"] == "calibrate":
            shutil.copyfile(repos["B"] / "bench-iters.txt", out / "candidate-iterations.txt")
            load_counts(out / "candidate-iterations.txt")
            result["calibration"] = "Candidate only: human review/commit required; not comparison evidence"
        if digest(out / "iterations.txt") != plan["iteration_digest"] or \
                digest(out / "bench_ab.toml") != plan["manifest_digest"]:
            raise ValueError("Frozen manifest digest changed")
        if not selected:
            raise ValueError("No compatible selected cells; coverage blocked")
        result["complete"] = True
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        result["errors"].append(str(error))
    finally:
        result["unmeasured"] = unmeasured_rows(result)
        if plan["operation"] == "calibrate" and args.b.joinpath("bench-iters.txt").is_file():
            shutil.copyfile(args.b / "bench-iters.txt", out / "candidate-iterations.txt")
        save(out / "results.json", result)
    return 0 if result["complete"] else 1


def report(args):
    out = args.results
    result = load(out / "results.json") if (out / "results.json").exists() else {
        "cells": [], "unsupported": [], "errors": ["Preparation/build did not produce results"],
        "complete": False}
    result["unmeasured"] = unmeasured_rows(result)
    lines = ["# Hot-path A/B", "", "Aggregate ns/op, not request latency percentiles.",
             "Hosted flags are advisory. Dedicated confirmation requires two predeclared batches;",
             "unsupported/noisy/smoke cells and unvalidated storage stability remain open debts.", "",
             "Identity: " + json.dumps({k: result.get("plan", {}).get(k) for k in
                                       ("base_sha", "candidate_sha", "master_sha", "mode", "operation",
                                        "iteration_digest", "manifest_digest", "scale")}), "",
             "| Cell | A median ns/op | B median ns/op | Change % | MAD spread % | Pairs | Status |",
             "|---|---:|---:|---:|---:|---:|---|"]
    for cell in result["cells"]:
        for batch in cell["batches"]:
            lines.append(f"| {cell['key']} (batch {batch['batch']}) | {batch['a_ns']:.2f} | "
                         f"{batch['b_ns']:.2f} | {batch['change_pct']:+.2f} | "
                         f"{batch['spread_pct']:.2f} | 5 | {cell['status']} |")
    for item in result["unmeasured"]:
        lines.append(f"| {item['cell']} | — | — | — | — | 0 | unmeasured: {item['reason']} |")
    for item in result["unsupported"]:
        lines.append(f"| {item['cell']} | — | — | — | — | 0 | unsupported: {item['reason']} |")
    lines += ["", "## Measurement failures"] + result["errors"]
    lines += ["", result.get("calibration", ""), "## Validation still required in real dispatch",
              "Common locked release builds, harness/runtime compatibility, useful timed intervals,",
              "runner noise/storage stability and future frozen fixture adapters. No local tests run."]
    text = "\n".join(lines) + "\n"
    out.mkdir(parents=True, exist_ok=True)
    (out / "report.md").write_text(text)
    if args.summary:
        with args.summary.open("a") as stream:
            stream.write(text)
    save(out / "report.json", result)
    return 0


def verdict(args):
    result = load(args.results / "results.json")
    if result["errors"] or not result["complete"]:
        return 1
    if result["plan"]["operation"] == "calibrate":
        return 0
    if result["plan"]["mode"] == "dedicated":
        if any(c["status"] == "flagged" for c in result["cells"]):
            return 2
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    p = sub.add_parser("prepare")
    p.add_argument("--baseline", default=BASELINE)
    p.add_argument("--candidate-env", default="REQUESTED_CANDIDATE")
    p.add_argument("--mode", choices=("hosted", "dedicated"), default="hosted")
    p.add_argument("--operation", choices=("compare", "calibrate"), default="compare")
    p.add_argument("--output", type=Path, required=True)
    p = sub.add_parser("run")
    for name in ("plan", "a", "b", "target-root"):
        p.add_argument("--" + name, type=Path, required=True)
    p = sub.add_parser("report")
    p.add_argument("--results", type=Path, required=True)
    p.add_argument("--summary", type=Path)
    p = sub.add_parser("verdict")
    p.add_argument("--results", type=Path, required=True)
    args = parser.parse_args()
    try:
        return {"prepare": prepare, "run": run, "report": report,
                "verdict": verdict}[args.command](args) or 0
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        print(f"Hard measurement failure: {error}", file=sys.stderr)
        if args.command == "prepare":
            args.output.mkdir(parents=True, exist_ok=True)
            save(args.output / "results.json", {"complete": False, "errors": [str(error)],
                                               "cells": [], "unsupported": []})
        return 1


if __name__ == "__main__":
    sys.exit(main())
