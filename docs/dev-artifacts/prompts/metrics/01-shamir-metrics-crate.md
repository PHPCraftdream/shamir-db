# Brief: the `shamir-metrics` crate (release-1 R4a-1)

Design: `docs/dev-artifacts/design/engine-metrics.md`, sections 3 (the primitive), 5 (cost proof plan), 6 (tests, R4a-1 item a),
7 (step PR-1) and 9 (open questions, defaults apply). Read them in full, plus
`docs/dev-artifacts/prompts/release/04-observability.md` section 2 (engine keeps state, no metrics library in the engine).

## Why

Every later R4a step records latencies into one shared histogram type. The type must sit below every engine layer and every
future split crate, so it is a new leaf crate with no dependency except `std`. This step adds only that crate; the engine,
the server and the exporter are not touched.

## What

1. **New crate** `crates/foundation/shamir-metrics` (package `shamir-metrics`). It is the 7th and last slot of the
   `foundation` group (cap 7). Copy the manifest shape of `crates/foundation/shamir-numa/Cargo.toml` (`publish = false`,
   edition, license, `[lib] doctest = false`, `[lints] workspace = true`). No production dependencies at all. The workspace
   `members` glob `crates/*/*` already covers it.
2. **API exactly as design 3.1**: `Histogram::new(bounds: &'static [u64]) -> Self` (a `const fn`), `observe(&self, v: u64)`,
   `snapshot(&self) -> HistogramSnapshot { bounds, buckets: Vec<u64>, count, sum, torn }`. Cell layout, inclusive `le`
   semantics, the implicit overflow cell, integer `sum`: design 3.2. Update: design 3.3 (linear scan with early exit, three
   Relaxed `fetch_add`s). Snapshot: the seqlock epoch of design 3.4 (writer bumps the epoch before and after, reader retries
   a bounded number of times, then returns a best-effort copy with `torn: true`). Use a fixed array of atomic cells sized
   for the largest bound set (19 latency bounds + 1 overflow cell) and reject a larger `bounds` slice at construction with a
   const assertion; do not allocate in `new` or `observe`.
3. **The two static bound sets** as public constants of the crate: `LATENCY_NS_BOUNDS` (the 19 values of design 3.2) and
   `BYTES_BOUNDS` (12 powers of two, 1 KiB to 1 GiB).
4. **Atomics through one alias module** so the loom model can swap them: under `cfg(loom)` the atomics come from `loom`
   (a `[target.'cfg(loom)'.dependencies]` entry, the same way the precedent does it; read how
   `crates/engine/shamir-engine/src/table/support/guards/tests/loom_model.rs` and `WriterDrainBarrier` gate it). A `const fn new`
   cannot build `loom` atomics, so follow the precedent for that too (a separate non-const constructor under `cfg(loom)`).
5. **Tests**, in `src/tests/` (a `tests/mod.rs` manifest plus one file per topic, no inline test modules, no local `use`):
   bound mapping at every edge (value equal to a bound, one above, zero, `u64::MAX`), the overflow cell, `count` and `sum`,
   a consistent snapshot after many observations, the `torn` path (a child test module may reach the private epoch field to
   hold it odd and prove the bounded retry returns `torn: true` instead of blocking), and the `loom_model` test of design 3.5
   (writer thread observes two values, reader snapshots, the snapshot is a consistent prefix). The loom test function names
   contain `loom_model`.
6. **Bench** `benches/histogram_observe.rs` on `bench_scale_tool::Harness`, by copying the shape of an existing small bench
   (for example `crates/storage/shamir-wal/benches/wal_append.rs` and its `[[bench]]` entries). Cells: `observe` on the latency
   set at a low, a middle and an overflow value, and a bare `AtomicU64::fetch_add` as the reference. Acceptance of design 5:
   at most 50 ns per `observe`. Find out how other crates treat `bench-iters.txt` for a new bench and follow that; do not run
   the bench.
7. **Registries of crates.** Grep (outside `docs/dev-artifacts` and `docs/checkpoints`) for the places that enumerate crates,
   using a sibling such as `shamir-numa` as the probe, and add the new crate consistently: at least the crate table and the
   crate count in `CLAUDE.md` and the same lines in `AGENTS.md` if present, and any CI matrix or edge file under `scripts/ci/`
   that lists crates. A crate README is optional; if a sibling has one, add a short one. `Cargo.lock` gets the new entry by
   `cargo check -p shamir-metrics --all-targets` (allowed locally; do not edit the lock by hand).
8. **No rust-guard campaign.** No `shamir-engine` source changes, so do not touch `.rust-guard/stages.toml` or
   `.rust-guard/stages/`. This overrides the corresponding bullet of the common tail below.

## Non-goals

No engine, server, WAL or index change; no exporter or text rendering (that is PR-2); no counter or gauge types (only the
histogram); no metrics library; no `std::sync::Mutex`/`RwLock`; no CI workflow edits (if the existing loom gate cannot pick up
the new `loom_model` test without a workflow change, say so in the final report with the exact job and selection line, and
do not edit the workflow); no version change.

## Acceptance (observed in CI)

fmt, clippy and layout clean; the crate's lib tests green on all three operating systems; `cargo nextest list -E
'test(loom_model)'` with the loom cfg lists the model; the bench compiles. State in the final report exactly which of these
you could verify locally (only fmt, clippy, check, layout) and which CI must show.
