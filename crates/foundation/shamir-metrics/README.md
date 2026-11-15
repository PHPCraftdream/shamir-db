# shamir-metrics

Fixed-bucket atomic histogram with a torn-free seqlock snapshot for
S.H.A.M.I.R. (design: `docs/dev-artifacts/design/engine-metrics.md` §3).

A foundation leaf crate with **no production dependencies beyond `std`**, so
every engine layer and future split crate can record into it without a cycle.
The engine keeps state; the server polls snapshots — no metrics library, no
locks on engine hot paths.

## What's here

| Item | Role |
|---|---|
| `Histogram` | Fixed static bounds, inclusive `le` semantics, implicit overflow cell, integer `sum`; `observe` = linear scan + three Relaxed `fetch_add`s inside a `started`/`completed` counter pair |
| `Histogram::snapshot` | Two-counter seqlock reader: bounded retries, then a best-effort copy with `torn: true` (the exporter drops torn samples instead of blocking) |
| `HistogramSnapshot` | `bounds`, `buckets` (`len + 1`, last = overflow), `count`, `sum`, `torn` |
| `LATENCY_NS_BOUNDS` | 19 latency bounds, 100 µs … 60 s |
| `BYTES_BOUNDS` | 12 power-of-two byte bounds, 1 KiB … 1 GiB |
| `MAX_BOUNDS` | Fixed cell-array capacity; larger bound slices are rejected at construction |

No allocation in `new` (a `const fn`) or `observe`. Under the `loom` feature
the atomics swap to `loom` through `src/atomics.rs` and the two-counter
seqlock snapshot is model-checked in `src/tests/loom_model.rs`:

```
cargo test -p shamir-metrics --features loom --lib tests::loom_model
```

The `observe` micro-bench lives in
`benches/histogram_observe.rs` (acceptance: ≤ 50 ns/op).
