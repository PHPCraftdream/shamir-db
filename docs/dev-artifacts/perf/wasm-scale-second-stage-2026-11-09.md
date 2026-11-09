# WASM procedure path at scale: second-stage measurements (2026-11-09)

Numbers for the decisions of the second stage of the compiler plan
(`docs/dev-artifacts/design/shamir-lang-design.md`,
`docs/dev-artifacts/design/wasm-two-tier-compilation.md`). Produced by the bench
`crates/wasm/shamir-wasm-host/benches/wasm_scale` (release profile, manual runs only) and by
the existing `wasm_procedures` bench for the per-call costs.

## Conditions and caveats

* Windows, one run each, release profile, the process tree limited to 25% CPU on 4 logical
  processors at idle priority (the machine was shared with other work). Absolute numbers are
  therefore pessimistic and the tails (p999, max) are dominated by the CPU quota: even the
  idle row has a 410 ms maximum. Read p50/p99 and ratios, not the tails.
* Cells with one or two iterations (`compile/100k_*`, `compile/1m_*`, `cold/compile_fn512_*`)
  are estimates.
* `compute/wasm_fast/*` against `compute/wasm/*` is not monotonic (`fib`, `f64_poly` came out
  faster on the fast tier); treat the per-kernel call gains below as a range, not a constant.

## Module size to compile time (generated module, host engine)

| module | wasm bytes | Winch (fast) | Cranelift (optimized) | optimized − fast |
|---|---|---|---|---|
| 1k | 1 021 | 0.97 ms | 1.98 ms | 1.0 ms |
| 10k | 10 205 | 1.60 ms | 7.27 ms | 5.7 ms |
| 100k | 102 338 | 16.1 ms | 53.0 ms | 36.9 ms |
| 1m | 1 048 577 | 424 ms | 1.66 s | 1.23 s |

Existing kernels (`wasm_procedures`): `int_sum` 1.83 / 4.48 ms, `fn64` 29.0 / 33.2 ms,
`fn512` 198 / 285 ms (fast / optimized).

## Load cost

| module | admission | module hash |
|---|---|---|
| 1k | 11 µs | 44 ns |
| 10k | 67 µs | 0.33 µs |
| 100k | 0.88 ms | 4.6 µs |
| 1m | 63 ms | 38 µs |

Admission grows about linearly (roughly 60 ns per byte); hashing is negligible.

## Memory per loaded module

| module | fast code | optimized code | fast image | optimized image | heap held | heap peak while compiling |
|---|---|---|---|---|---|---|
| 1k | 4 KiB | 4 KiB | 14 KB | 14 KB | 4.6 KB | 80 KB |
| 10k | 16 KiB | 20 KiB | 32 KB | 37 KB | 14.6 KB | 0.28 MB |
| 100k | 144 KiB | 160 KiB | 216 KB | 245 KB | 116 KB | 2.4 MB |
| 1m | 1.40 MiB | 1.56 MiB | 2.1 MB | 2.4 MB | 1.14 MB | 23.4 MB |

The compile-time heap peak is about 22 times the module size; the held heap about 1.1 times.
The tier-up swaps the slot, it does not add a second resident copy once the fast form is
dropped.

## Calls while a compile runs in the background

100 000 calls of one warm procedure, the compile load being 1 or 4 background compiles of a
100 KiB module in a loop:

| load | p50 | p99 |
|---|---|---|
| idle | 33 µs | 94 µs |
| 1 compile | 64 µs | 174 µs |
| 4 compiles | 68 µs | 503 µs |

p50 doubles and p99 grows 1.8× (one compile) to 5.3× (four) on a quota-limited machine;
`background_jobs = max(1, cores / 4)` keeps the realistic case at the one-compile row.

## Tier-up break-even

Break-even calls = (optimized − fast compile) / (fast call − optimized call).

* `int_sum`: 2.65 ms / 0.15 ms ≈ 17 calls.
* `mem_scan`: 2.65 ms / 0.34 ms ≈ 8 calls (compile delta taken from `int_sum`, a module of
  the same size).
* `collatz`: 2.65 ms / 1.56 ms ≈ 2 calls.
* `fib`, `f64_poly`: no gain measured on this run, never repaid.
* Database-bound procedure (`host/async_get_x100`, 3.4 ms per call, all of it in the host):
  the tier does not change the call cost, so the optimized compile is never repaid.

The default `hot_calls = 64` therefore stays above every measured compute break-even and
costs one background compile for the procedures that never repay it. No change proposed.

## shamir-lang source compile

| source | source bytes | module bytes | compile time |
|---|---|---|---|
| scalar | 110 | 22 555 | 0.50 ms |
| validator | 326 | 24 557 | 0.58 ms |
| procedure | 226 | 24 301 | 0.58 ms |
| builder batch | 268 | 25 597 | 5.5 ms |
| 16 helper functions | 1 639 | 27 746 | 1.06 ms |
| 64 helper functions | 6 295 | 42 866 | 2.6 ms |
| 100 helper functions | 9 787 | 54 242 | 3.2 ms |

* Every measured source compiles in under 10 ms, the design target.
* The embedded runtime gives every module a floor of about 22 KB; each generated helper adds
  about 0.3 KB.
* The compile output is capped at 64 KiB (`B001`): 200 generated helpers (85 KB) are rejected,
  100 helpers (54 KB) pass. That cap is the effective size limit of a `shamir` module.
* The builder batch is about ten times slower than the other small sources (5.5 ms); it is the
  cell to watch.

## Thresholds proposed

* Source compile: at most 10 ms for a source whose module stays under the 64 KiB cap;
  regression gate at 2 times the numbers above on the same cells.
* Rust-compiled wasm reference: not measured here (needs the wasm32 CI target, part B of the
  CI-WASM32 task), so no numeric gap to compiled Rust is proposed yet.
