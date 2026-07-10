Task: #524 — prototype a sharded worker-loop / MPSC-batching design for
fjall point-ops, to amortize the per-op `spawn_blocking` dispatch cost
(audit finding 3.3, `docs/audits/2026-07-06-perf-radical-o-notation.md`).
This is explicitly a PROTOTYPE / EXPERIMENT task — read the "Honest
reporting requirement" section below before writing any code, it governs
how this task closes.

## Context

`crates/shamir-storage/src/storage_fjall.rs`'s `get`/`set`/`remove`/
`insert` each dispatch their own `tokio::task::spawn_blocking` call (fjall
is a synchronous crate, confirmed in task #502's investigation — no
non-blocking API exists anywhere in fjall's public surface). Each
dispatch costs ~1-5µs of threadpool hand-off + task migration, on top of
the actual fjall operation cost.

Baseline numbers (already measured, `crates/shamir-storage/benches/storage_fjall_pump.rs`,
`CARGO_TARGET_DIR=/d/dev/rust/.cargo-target-bench cargo bench -p
shamir-storage --bench storage_fjall_pump`):
```
insert:        46,095 ns/op
get:           29,887 ns/op
set_existing:  50,443 ns/op
scan_prefix:  525,893 ns/op
```

## The prototype

Design a sharded worker-loop: one (or a small, fixed number of) dedicated
OS thread(s) that owns a `fjall::Keyspace` handle and receives point-ops
(get/set/remove/insert) via an MPSC channel from async callers, batching
multiple concurrently-arriving ops into fewer `spawn_blocking`-equivalent
dispatches (or, if the worker loop itself runs OUTSIDE tokio's blocking
pool as a dedicated thread with its own channel receive loop, it may not
need `spawn_blocking` at all for the steady-state case — investigate
which shape fits this codebase's existing patterns better, e.g. how
`shamir-wal`'s group-commit leader/follower pattern
(`crates/shamir-wal/src/wal_group_commit.rs`) already solves an
analogous "batch concurrent async callers into fewer synchronous I/O
calls" problem — that's a good structural template to study, even though
it's solving a slightly different problem (write batching, not
read+write point-ops)).

Each caller sends its op + a oneshot response channel; the worker thread
drains what's currently queued, executes each op against the real fjall
`Keyspace` synchronously (no per-op dispatch — the thread is ALREADY the
blocking-safe context), and replies via each op's oneshot. This amortizes
the "cross into a safe-to-block context" cost over a batch instead of
paying it per op.

Investigate whether this can be layered UNDERNEATH the existing `Store`
trait implementation for `FjallStore` transparently (so nothing calling
code needs to change), or whether it requires a new opt-in variant.
Prefer transparent if it's clean; don't force it if it requires
invasive restructuring.

## Honest reporting requirement (READ BEFORE STARTING)

This is a PROTOTYPE. Per this campaign's established discipline (never
fabricate perf results), you MUST:

1. Implement the design.
2. Bench BOTH:
   a. **Throughput under contention** — many concurrent callers hammering
      point-ops simultaneously (the audit's claimed +10-30% win scenario).
   b. **p99 latency under LOW/NO contention** — a single isolated caller
      doing a point-op with nothing else happening. This is the
      "did we regress the common case by making it wait for a batch
      window" check — batching amortization schemes classically trade
      throughput-under-load for latency-in-the-uncontended-case, and this
      audit finding's own text warned about exactly this trade-off.
3. Compare against the baseline numbers above (same bench file, same
   methodology, same `CARGO_TARGET_DIR` isolation).
4. **If EITHER metric shows a regression** (uncontended latency gets
   measurably worse, OR contended throughput doesn't actually improve),
   **do NOT keep the change** — revert your implementation, and instead
   write up what you tried, what you measured, and why it didn't pan out,
   as the final report. This is a fully legitimate, expected outcome for
   an experimental prototype task — reporting "I tried X, it made Y
   worse, here's the data, recommend NOT pursuing this further" is
   exactly as valuable as a working implementation, and is what this
   brief wants if that's what the data shows. Do not tune the benchmark
   or cherry-pick numbers to make a regression look like a win.
5. If BOTH metrics genuinely improve (or throughput improves with no
   measurable uncontended-latency regression), keep the implementation,
   write the regression tests (existing `Store` trait behavior for
   `FjallStore` must be unchanged — same `./scripts/test.sh -p
   shamir-storage` suite must stay green), and report the real numbers.

## TDD (only if the prototype is kept)

Existing `storage_fjall_tests.rs` and any `Store`-trait-conformance
tests must stay green — the worker-loop must be invisible to correctness,
only affecting latency/throughput characteristics. Add a test for the
worker-loop's own batching behavior if it introduces new internal
structure worth testing directly (e.g. a batch actually contains
multiple ops when submitted concurrently).

## Verification (lighter per-task gate, agreed this session)

```
cargo check --workspace --all-targets
./scripts/test.sh -p shamir-storage
```
Do NOT run the full fmt/clippy/test --full gate — that's FINAL-GATE's job.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

```
[Prototype] Status: kept / reverted-with-findings
  > Design implemented (worker-loop shape, batching mechanism)
  > Bench: throughput-under-contention before/after
  > Bench: p99 latency under low contention before/after
  > If reverted: honest explanation of what regressed and why
  > If kept: confirm existing test suite stays green

[Verification]
  cargo check --workspace --all-targets: pass/fail
  ./scripts/test.sh -p shamir-storage: pass/fail
```
