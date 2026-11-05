#!/usr/bin/env python3
"""Markdown summary of one `wasm-bench` job.

Usage: wasm_procedures_report.py <bench-out-dir> <strategy>

Reads `wasm_procedures.log`, `wasm_db_calls.log` and `elapsed.txt` written by the workflow
and prints the cell table plus the derived numbers the measurements exist for: the ratio to
native per kernel, the cost of one guest->host call, the metering overhead and what a
database call from a guest adds over the same call without a guest.
"""

import re
import sys
from pathlib import Path

CELL = re.compile(r"^\s*(\S+)\s+(\d+)\s+iters\s+([\d.]+)\s+ms\s+([\d.]+)\s+ns/op")
HOST_CALLS = 100
DB_CALLS = 20


def parse(path: Path) -> dict:
    cells = {}
    if not path.is_file():
        return cells
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        match = CELL.match(line)
        if match:
            name = match.group(1).split("::", 1)[-1]
            cells[name] = (int(match.group(2)), float(match.group(4)))
    return cells


def fmt(ns: float) -> str:
    if ns >= 1e6:
        return f"{ns / 1e6:.2f} ms"
    if ns >= 1e3:
        return f"{ns / 1e3:.1f} us"
    return f"{ns:.0f} ns"


def ratio(a: float, b: float) -> str:
    return f"{a / b:.2f}x" if b else "n/a"


def skipped(path: Path) -> str:
    if not path.is_file():
        return ""
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        if line.startswith("SKIP"):
            return line
    return ""


def main() -> int:
    out_dir, strategy = Path(sys.argv[1]), sys.argv[2]
    log = out_dir / "wasm_procedures.log"
    cells = parse(log)
    db_cells = parse(out_dir / "wasm_db_calls.log")
    elapsed = (out_dir / "elapsed.txt").read_text().strip() if (out_dir / "elapsed.txt").is_file() else "n/a"

    print(f"## wasm procedures - strategy `{strategy}`")
    print(f"\n{elapsed}\n")
    reason = skipped(log)
    if reason:
        print(f"**{reason}**\n")
        return 0
    if not cells:
        print("No cells were produced; see the job log.\n")
        return 0

    print("| cell | iters | per op |\n|---|---:|---:|")
    for name, (iters, ns) in {**cells, **db_cells}.items():
        print(f"| `{name}` | {iters} | {fmt(ns)} |")

    print("\n### Compute: wasm vs native\n\n| kernel | wasm | native | ratio |\n|---|---:|---:|---:|")
    for name, (_, wasm) in cells.items():
        if name.startswith("compute/wasm/"):
            kernel = name.rsplit("/", 1)[1]
            native = cells.get(f"compute/native/{kernel}", (0, 0.0))[1]
            print(f"| {kernel} | {fmt(wasm)} | {fmt(native)} | {ratio(wasm, native)} |")

    base = cells.get("warm/call_identity", (0, 0.0))[1]
    print(f"\n### Guest -> host call (per call, over the {fmt(base)} warm-call floor)\n")
    for name, (_, ns) in cells.items():
        if name.startswith("host/"):
            print(f"- `{name}`: {fmt(max(ns - base, 0.0) / HOST_CALLS)} per host call")

    print("\n### Metering overhead (relative to no metering)\n\n| kernel | mode | per call | vs none |\n|---|---|---:|---:|")
    for name, (_, ns) in cells.items():
        parts = name.split("/")
        if parts[0] == "metering" and len(parts) == 3:
            none = cells.get(f"metering/none/{parts[2]}", (0, 0.0))[1]
            print(f"| {parts[2]} | {parts[1]} | {fmt(ns)} | {ratio(ns, none)} |")

    if db_cells:
        direct = db_cells.get(f"direct/execute_read_x{DB_CALLS}", (0, 0.0))[1]
        print(f"\n### Database calls from a guest (over {fmt(direct)} for {DB_CALLS} direct reads)\n")
        for name, (_, ns) in db_cells.items():
            if name.startswith("wasm/"):
                calls = int(name.rsplit("_x", 1)[1])
                print(f"- `{name}`: {fmt(ns / calls)} per call")
    return 0


if __name__ == "__main__":
    sys.exit(main())
