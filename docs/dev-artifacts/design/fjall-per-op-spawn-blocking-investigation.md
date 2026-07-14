# Investigation: fjall per-op `spawn_blocking` overhead (task #502, audit 3.3)

## Finding as stated

`docs/dev-artifacts/audits/2026-07-06-perf-radical-o-notation.md` finding 3.3 (deferred
once already from task #490, "структурное", "средняя-высокая" complexity):

Every `get`/`set`/`remove`/`insert` in `crates/shamir-storage/src/storage_fjall.rs`
is its own `tokio::task::spawn_blocking` call — a separate threadpool
dispatch + task migration (~1-5µs) per point-op, plus loss of any
data-locality between consecutive ops on the same connection/transaction.
The audit's own fix sketch:

- (a) for read-mostly ops, skip `spawn_blocking` entirely when the data is
  already in fjall's memtable/block-cache — "fjall 3 умеет неблокирующие
  пути? — проверить" (does fjall 3 have non-blocking paths? — verify).
- (b) a sharded worker-loop with MPSC batching of point-ops, amortizing
  dispatch cost over a batch.

## What's already NOT part of this finding (already fixed)

`get_many` (storage_fjall.rs:299-320) already batches: ALL keys in one
`get_many` call share a single `spawn_blocking` dispatch (a sequential loop
of point-gets inside one blocking closure), not one dispatch per key. The
audit's complaint about `get_many` was about missing *snapshot* consistency
across the batch (a correctness/isolation nuance, not a dispatch-overhead
one) — out of scope for 3.3, which is specifically about single-op
`get`/`set`/`remove`/`insert`/`contains_key`.

## Investigation: is approach (a) feasible?

**No — verified from fjall 3.1.6's actual source** (the version resolved in
`Cargo.lock`; local registry cache at
`~/.cargo/registry/src/index.crates.io-*/fjall-3.1.6/src/keyspace/mod.rs:623`):

```rust
pub fn get<K: AsRef<[u8]>>(&self, key: K) -> crate::Result<Option<lsm_tree::UserValue>> {
    Ok(self.tree.get(key, SeqNo::MAX)?)
}
```

fjall is a **purely synchronous** crate — there is no async variant, no
"try from cache only" API, no non-blocking peek. `Tree::get` may hit the
in-memory memtable/block-cache (fast) OR fall through to an actual
synchronous file read on a cache miss (a real blocking syscall, potentially
disk I/O) — the caller has no way to distinguish these cases up front, and
no way to safely bound/interrupt a blocking syscall from within an async
task without unsafe OS-level thread signaling (not something this
crate/task should attempt). Grepped the fjall source for any block-cache-
only or memtable-only lookup primitive — none exists in the public API.

**Conclusion: approach (a) requires forking/patching the `fjall` dependency
itself to expose a genuinely non-blocking or bounded-time read path. That
is out of scope for this task** (this campaign does not vendor/fork
third-party crates) and would itself be a much larger, separate body of
work with upstream-compatibility risk.

## Investigation: is approach (b) (sharded worker-loop + MPSC batching) low-risk enough to implement now?

**Assessment: no — defer, for three concrete reasons:**

1. **No baseline bench exists.** The audit's own gap list explicitly calls
   this out: "fjall-бэкенд вообще не бенчится" (the fjall backend isn't
   benchmarked at all — `membuffer_pump` and the engine benches only
   exercise the in-memory backend). This campaign's `/opti` methodology
   (see `CLAUDE.md`) requires baseline-then-fix-then-measure for every PERF
   change — there is currently no way to honestly report a before/after
   number for this specific finding, since the "before" doesn't exist as a
   bench yet. Implementing the worker-loop first and hand-waving the
   numbers would violate this campaign's "never fabricate results" rule.

2. **The architectural change touches the single most heavily-used storage
   backend's point-op path.** A shared MPSC-batching worker loop is not a
   local, easily-reverted change — every `get`/`set`/`remove`/`insert`
   across the whole engine (not just this crate) would route through a new
   shared queue + worker(s), changing the concurrency model for the
   fjall backend's read/write path wholesale. This is a materially
   different risk class from the tasks this campaign has completed so far
   (#499, #500, #501 were all either representation swaps behind an
   unchanged trait boundary, or an additive sidecar file, or a single-type
   internal fix) — this one changes how concurrent callers interact with
   the store itself.

3. **The win is directional, not proven, and could regress tail latency.**
   The audit's own estimate is "point-op latency floor −1–5µs; throughput
   mixed-нагрузки +10–30%" — a THROUGHPUT-under-contention win from
   amortizing dispatch cost over a batch. But batching point-ops through a
   shared channel classically trades average/throughput for **worst-case
   latency** (head-of-line blocking: an isolated, uncontended point-op now
   waits for a batching window or queue position instead of getting an
   immediate dedicated thread-pool dispatch). Without a bench that
   specifically measures p99/tail latency under both low-contention and
   high-contention load, this trade cannot be responsibly evaluated ahead
   of time.

## Decision

**Defer implementation of the worker-loop batching (approach b).** Approach
(a) is confirmed infeasible without forking fjall. Two follow-up tasks
filed (see TaskList) to make (b) tractable in a future pass:

- Add the missing fjall-backend bench first (point get/set/scan against a
  real tempdir-backed instance) — low-risk, immediately actionable, and the
  necessary prerequisite for ANY future work on this finding.
- Once that bench exists, prototype the sharded worker-loop / MPSC-batching
  design against it, with an explicit p99-latency-under-low-contention
  check alongside the throughput-under-contention number, before deciding
  whether to land it.

No code change in this task beyond this document — this finding remains
open, correctly scoped down rather than implemented on an unverified
premise.
