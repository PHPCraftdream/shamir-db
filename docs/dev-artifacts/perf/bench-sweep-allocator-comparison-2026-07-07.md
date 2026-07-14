# Allocator sweep comparison — 2026-07-07

## Context

After normalizing every `bench-scale-tool` bench across the workspace to
cost ≤10ms per single call (see
`docs/dev-artifacts/checkpoints/2026-07-07-bench-normalization-crush-fanout.md` and
commit `9c6ba170`), a full workspace `sweep` completes in ~60-80s. This
made it cheap to run the same 47-target sweep under four different
global-allocator configurations and compare wall-clock totals directly.

**Important scope caveat.** This sweep is a fleet of small, cheap,
mixed-workload micro-benches (msgpack encode/decode, filter eval, WAL
append, subscription fan-out, etc.) — it does **not** stress large
allocations or multi-thread allocator contention. It answers "does the
allocator choice regress typical per-request workloads?", not "which
allocator wins under a large-alloc / heavy-concurrency profile". The
latter question belongs to `sefer-alloc`'s own benches
(`large_realloc`, `heap_xthread`, `malloc_macro`) — see the follow-up
task where those are run instead.

**Build profile.** All bench binaries in this comparison were built
under `[profile.bench]` (`Cargo.toml` root, inherits `release`, but
`opt-level = 1`, `lto = false`) — **not** `opt-level = 3`. This is a
deliberate project convention (fast bench rebuilds during iterative
`/opti` work), documented at `Cargo.toml:64-87`. Numbers below are NOT
representative of a full `opt-level = 3` release build; they are
internally comparable to each other (same profile across all four
variants) but not to production release-binary throughput claims.

## Results — 47-target sweep, `scale=1`, two runs per allocator

| Allocator | Source | Run 1 | Run 2 | Failed |
|---|---|---|---|---|
| sefer-alloc 0.2.1 | crates.io | 71.2s | 69.5s | 0/47 |
| sefer-alloc 0.3.0 | local path `D:/dev/rust/sefer-alloc` | 82.1s | 82.2s | 0/47 |
| mimalloc | crates.io (`mimalloc` crate) | 74.3s | 73.1s | 0/47 |
| system default | Windows HeapAlloc (no `#[global_allocator]`) | 63.6s | 67.5s | 0/47 |

All four configurations passed 0 failures across every one of the 47
targets. Sums are wall-clock totals for the whole 47-target sweep (each
target itself runs its own calibrated iteration count from
`bench-iters.txt`, unchanged across all four runs — only the allocator
varied).

**Reading this table.** System default was fastest in this specific
mixed micro-bench fleet, then sefer-alloc 0.2.1, then mimalloc, then
sefer-alloc 0.3.0 (local, still in development). The gap between the
two sefer-alloc versions (69.5-71.2s vs 82.1-82.2s) is larger than
run-to-run noise (~2-4s) and warrants investigation on the sefer-alloc
side — 0.3.0 added `alloc-decommit` + `fastbin` to its `production`
feature bundle (0.2.1 did not have decommit), which is a plausible
source of the regression on this workload shape (many short-lived
small segments cycling quickly).

**Caveat on "who wins" claims.** This sweep intentionally exercises
cheap, mostly single-threaded, small-allocation workloads (the whole
point of this session's normalization pass was ≤10ms/call, i.e. small
N). It is not the regime sefer-alloc is designed to win in — the
project's own rollout note
(`docs/dev-artifacts/perf/sefer-alloc-rollout-2026-06-30.md`) attributes sefer-alloc's
17-22× advantage over mimalloc specifically to **alloc-heavy setup
workloads** (bulk record construction, `engine_perf --test`, 103
variants) at release opt-levels, not to this micro-bench fleet. A
proper "sefer-alloc's real strength" comparison needs the crate's own
large-allocation and multi-thread-contention benches (see below).

## How the switch was implemented

Before this session, allocator selection was:
- `shamir-db`: already feature-gated (`bench-sefer` /
  `bench-sefer-tuned` / `bench-mimalloc`, default = system) via a
  shared `crates/shamir-db/benches/bench_allocator.rs` included from
  every bench file.
- `shamir-server`: **hardcoded** — every one of its 4 bench files
  (`db_handler_rps.rs`, `duplex_throughput.rs`, `wire_latencies.rs`,
  `wire_pipelining.rs`) declared its own
  `#[global_allocator] static GLOBAL: sefer_alloc::SeferAlloc = ...;`
  directly, with no way to switch without editing every file.

This session centralized `shamir-server`'s switch to mirror
`shamir-db`'s pattern:

1. Added a new shared file
   `crates/shamir-server/benches/bench_allocator.rs`:

   ```rust
   #[cfg(all(feature = "bench-sefer", not(feature = "bench-sefer-tuned")))]
   #[global_allocator]
   static GLOBAL: sefer_alloc::SeferAlloc = sefer_alloc::SeferAlloc::new();

   #[cfg(feature = "bench-sefer-tuned")]
   #[global_allocator]
   static GLOBAL: sefer_alloc::SeferAlloc = sefer_alloc::SeferAlloc::with_config({
       sefer_alloc::LargeCacheConfig::new()
           .budget_bytes(2 * 1024 * 1024 * 1024)
           .headroom_bytes(512 * 1024 * 1024)
           .decay_interval_ms(500)
           .decay_rate_percent(25)
           .mode(sefer_alloc::LargeCacheMode::Lazy)
   });

   #[cfg(feature = "bench-mimalloc")]
   #[global_allocator]
   static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;
   ```

2. Replaced the 4 hardcoded `#[global_allocator]` blocks in
   `db_handler_rps.rs` / `duplex_throughput.rs` / `wire_latencies.rs` /
   `wire_pipelining.rs` with a single `include!("bench_allocator.rs");`
   each.

3. Added to `crates/shamir-server/Cargo.toml`:

   ```toml
   [features]
   bench-sefer       = []
   bench-sefer-tuned = []
   bench-mimalloc    = ["dep:mimalloc"]

   [dependencies]
   # ... sefer-alloc stays a required (non-optional) dep — src/main.rs
   # (the production bin) always uses it regardless of these features.
   mimalloc = { version = "0.1", default-features = false, optional = true }
   ```

4. `src/main.rs` (the actual production server binary) was **not**
   touched — it keeps its own hardcoded, production-tuned
   `sefer_alloc::SeferAlloc::with_config(ALLOCATOR_CONFIG)` regardless
   of these bench features. The switch only affects the 4 bench
   binaries.

With this in place, `cargo bench -p shamir-server --features
bench-mimalloc` (or `--features bench-sefer-tuned`, or no features at
all for system default) picks the allocator per-binary, same as
`shamir-db` already did.

## Exact commands run, in order

### 1. Baseline: sefer-alloc 0.2.1 (crates.io), 2 runs

No feature toggling needed — this was the pre-existing default
(hardcoded in the 4 shamir-server bench files at the time, and
`shamir-db`'s existing default via its own feature gate resolving to
system... — for this run shamir-db's own benches ran at system default
since no `bench-mimalloc`/`bench-sefer*` feature was active; only
shamir-server's 4 benches used sefer-alloc, since it was still
hardcoded then).

```bash
CARGO_TARGET_DIR=D:/dev/rust/.cargo-target-bench \
  cargo bench-tool calibrate-to-budget 60 --force   # first run, recalibrates + sweeps
CARGO_TARGET_DIR=D:/dev/rust/.cargo-target-bench \
  cargo bench-tool sweep                            # subsequent runs, reuse bench-iters.txt
```

### 2. Switch to sefer-alloc 0.3.0 (local path), 2 runs

Edited `crates/shamir-db/Cargo.toml` and
`crates/shamir-server/Cargo.toml`:

```toml
# before
sefer-alloc = { version = "0.2.1", features = ["production"] }
# after
sefer-alloc = { path = "D:/dev/rust/sefer-alloc", features = ["production"] }
```

```bash
cargo check -p shamir-server                        # confirm it resolves + builds
CARGO_TARGET_DIR=D:/dev/rust/.cargo-target-bench \
  cargo bench-tool sweep                            # run 1
CARGO_TARGET_DIR=D:/dev/rust/.cargo-target-bench \
  cargo bench-tool sweep                            # run 2
```

### 3. Revert to sefer-alloc 0.2.1 (crates.io), 2 runs (re-confirmation)

```bash
git checkout 6daacdce~1 -- crates/shamir-db/Cargo.toml \
  crates/shamir-server/Cargo.toml Cargo.lock
cargo check -p shamir-server
CARGO_TARGET_DIR=D:/dev/rust/.cargo-target-bench cargo bench-tool sweep   # run 1
CARGO_TARGET_DIR=D:/dev/rust/.cargo-target-bench cargo bench-tool sweep   # run 2
```

### 4. Centralize the allocator switch in shamir-server

(See "How the switch was implemented" above — added
`bench_allocator.rs` + `[features]` + `mimalloc` optional dep, migrated
the 4 bench files to `include!(...)`.)

```bash
cargo check -p shamir-server --benches                        # system default (no feature)
cargo check -p shamir-server --benches --features bench-mimalloc
cargo check -p shamir-server --benches --features bench-sefer
cargo fmt -p shamir-server -- --check
cargo clippy -p shamir-server --benches -- -D warnings
```

### 5. mimalloc, 2 runs

Since `bench-cli`'s workspace build (`cargo bench --workspace
--benches --no-run`) does not accept per-crate `--features`, the
switch for a *workspace-wide* sweep is done via each crate's `default`
feature set (toggled temporarily, then reverted):

```toml
# crates/shamir-db/Cargo.toml
default = ["all-backends", "bench-mimalloc"]   # was: ["all-backends"]

# crates/shamir-server/Cargo.toml
[features]
default = ["bench-mimalloc"]                   # added
```

```bash
cargo check -p shamir-server -p shamir-db --benches
CARGO_TARGET_DIR=D:/dev/rust/.cargo-target-bench cargo bench-tool sweep   # run 1
CARGO_TARGET_DIR=D:/dev/rust/.cargo-target-bench cargo bench-tool sweep   # run 2
```

### 6. system default, 2 runs

Reverted the `default` feature toggles from step 5:

```toml
# crates/shamir-db/Cargo.toml
default = ["all-backends"]

# crates/shamir-server/Cargo.toml
[features]
# default = [...] line removed entirely
```

```bash
CARGO_TARGET_DIR=D:/dev/rust/.cargo-target-bench cargo bench-tool sweep   # run 1
CARGO_TARGET_DIR=D:/dev/rust/.cargo-target-bench cargo bench-tool sweep   # run 2
```

### 7. Final state (this commit)

Reverted `sefer-alloc` back to the crates.io `0.2.1` release in both
`shamir-db` and `shamir-server` (the local `D:/dev/rust/sefer-alloc`
checkout is not yet published — a path dependency would break the
build for anyone without that exact local checkout). The new
centralized allocator-switch infrastructure in `shamir-server`
(`bench_allocator.rs` + `[features]` + optional `mimalloc` dep) is
kept — it's a pure dev/bench convenience, doesn't touch the production
binary, and makes future allocator comparisons a one-line feature
toggle instead of a 4-file edit.

```bash
# crates/shamir-db/Cargo.toml, crates/shamir-server/Cargo.toml:
sefer-alloc = { version = "0.2.1", features = ["production"] }   # (back to crates.io)

cargo check -p shamir-server -p shamir-db --benches
cargo fmt --all -- --check
cargo clippy -p shamir-server -p shamir-db --benches -- -D warnings
```

## Follow-up: testing sefer-alloc's actual claimed strengths

This sweep does not test large allocations or multi-thread contention.
`sefer-alloc`'s own repo (`D:/dev/rust/sefer-alloc`) ships dedicated
Criterion benches for exactly that:

| Bench | What it measures | Command |
|---|---|---|
| `large_realloc` | Multi-MiB alloc+free, geometric `realloc` growth, adversarial-neighbour realloc pressure — SeferAlloc vs mimalloc vs System | `cargo bench --bench large_realloc --features alloc-global` |
| `malloc_macro` (example) | Multi-thread macro-benchmark (larson + mstress sweep) at T=1/2/4 threads | `cargo run --release --example malloc_macro --features "alloc-global alloc-xthread"` |
| `heap_xthread` | Cross-thread free (`RemoteFreeRing`) micro-bench, low-noise flamegraph target | `cargo bench --bench heap_xthread --features "alloc-core alloc-xthread"` |
| `global_alloc` | General Vec/Box churn, SeferAlloc vs mimalloc vs System | `cargo bench --bench global_alloc --features alloc-global` |

These run in the `sefer-alloc` repo directly (Criterion-based, not
migrated to `bench-scale-tool`), independent of the shamir-db
workspace sweep documented above.
