# Brief: shamir-metrics, revision 1 (multi-writer soundness, loom wiring)

Follow-up to `01-shamir-metrics-crate.md`, same worktree, same rules (the constraints of the original prompt apply unchanged:
no builds or tests, only fmt, clippy, `cargo check`, `check_layout`; no git mutation; final report required). The crate you
wrote stays; this revision fixes one protocol flaw found in review and one wiring problem. Read your own
`histogram.rs`, `atomics.rs`, `tests/torn_epoch.rs`, `tests/loom_model.rs` again before editing.

## 1. The single odd/even epoch is unsound with more than one writer

`observe` is called concurrently from many tasks. With two writers A and B the epoch goes 0 -> 1 (A starts) -> 2 (B starts):
it is EVEN while both are mid-update, so a reader sees an even epoch, copies half-updated cells, sees the same even epoch
again and returns `torn: false` for a snapshot whose `count`, `sum` and buckets disagree. The design text (3.4) has the same
flaw because it assumes one writer. Replace the single epoch with two counters, keeping the cost at two extra RMWs per
`observe`:

* Writer: `started.fetch_add(1, Relaxed)`; `fence(Release)`; the three Relaxed `fetch_add`s (cell, count, sum);
  `completed.fetch_add(1, Release)`.
* Reader attempt: `c1 = completed.load(Acquire)`; `s1 = started.load(Relaxed)`; if `s1 != c1` a write is in flight: retry.
  Otherwise copy cells, count and sum with Relaxed loads; `fence(Acquire)`; `s2 = started.load(Relaxed)`; accept only if
  `s2 == s1`, else retry. Keep the bounded retry count (8) and the best-effort copy with `torn: true`.
* Argument to put in a short comment above `snapshot`: equal counters mean no writer in flight at the moment of the `started`
  read; an unchanged `started` after the copy means no writer began during it; the release fence before the data writes and
  the acquire fence before the final `started` read make any visible data write of a late writer force `s2 != s1`.

Export `fence` through the `atomics.rs` alias module (`core::sync::atomic::fence`, and `loom::sync::atomic::fence` under
`cfg(loom)`). Update the memory arithmetic comment if you state one. Adjust `tests/torn_epoch.rs` (rename the file to
`torn_snapshot.rs` and the manifest): an in-flight writer is simulated by bumping `started` once without bumping `completed`
-> `torn: true` with the pre-existing state in the best-effort copy; balanced counters -> `torn: false`. Add a test with a
real multi-writer burst only if it can be written without threads racing on timing; otherwise leave concurrency to the loom
model.

## 2. The loom model must have two writers

Rewrite `tests/loom_model.rs`: two writer threads each observe one value (different buckets), one reader snapshots; assert
that a non-torn snapshot has `count == sum(buckets)`, `sum` equal to the sum of the observed values that landed in the
buckets, and that `count` is 0, 1 or 2 with the matching bucket set (so a snapshot that saw one writer's cell but not its
count would fail). Keep the state space small: use a private `static` bound set of two bounds for the model (not the
latency set), and `loom::model::Builder` with a preemption bound (for example 3) if the plain model is too large. The test
function name still contains `loom_model`.

## 3. Loom activation: follow the engine precedent

The engine does not use `[target.'cfg(loom)'.dependencies]`; it has an optional dependency `loom`, a cargo feature
`loom = ["dep:loom"]` and a `build.rs` that emits `cfg(loom)` only for that crate (see `crates/engine/shamir-engine/Cargo.toml`
and its `build.rs`). Do the same: optional dep, feature `loom`, a small `build.rs`, and declare the cfg the way the engine
does so `unexpected_cfgs` stays quiet under `clippy -D warnings` (check how the engine and the workspace lints handle it).
Remove the target-cfg dependency. Add the exact command to the crate README. The workflow is still not edited here; the CI
gate is wired in a separate step.

## 4. Design text sync (same change)

Edit `docs/dev-artifacts/design/engine-metrics.md`: section 3.4 describes the two-counter protocol and says why one odd/even
epoch is unsound with concurrent writers; the memory line of 3.3 and the loom sentence of 3.4 (narrowed epoch width, now
meaningless) follow; section 3.2 lists the twelve byte bounds you chose (1, 2, 4 KiB, then steps of x4 to 1 GiB) instead of the
ellipsis. Do not touch any other section. Add one line to the section 10 verification notes that the protocol was revised
after review.

## Acceptance

fmt, clippy (`-p shamir-metrics --all-targets`, also with `--features loom`) and layout clean; every `epoch` reference gone;
report what CI must show: lib tests green on three operating systems, `cargo nextest list -p shamir-metrics --features loom
--lib -E 'test(loom_model)'` lists the model and the model passes.
