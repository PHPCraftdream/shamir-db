בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Task #524: fjall worker-loop batching prototype — findings and revert

Audit finding 3.3 (`docs/audits/2026-07-06-perf-radical-o-notation.md`),
following up on task #502's investigation. This documents an
implemented-then-reverted prototype and the redesign it should be
replaced with.

## What was prototyped

A single dedicated OS worker thread per `FjallStore`, owning one clone
of the `fjall::Keyspace` handle. Every point-op (`get`/`set`/`remove`/
`insert`) was routed to this one thread via an MPSC channel + oneshot
reply, executed synchronously there, coalescing concurrently-arriving
ops into fewer wake cycles — eliminating the per-op `spawn_blocking`
dispatch cost entirely (no per-op cross-into-blocking-context hop).

## Why it was reverted (not committed)

The implementing agent's own bench (32 concurrent callers, 2,000 ×
512-byte records — 1 MB total, entirely memtable-resident, zero disk
I/O) showed BOTH throughput-under-contention and uncontended latency
improving versus the `spawn_blocking`-per-op baseline. Per this task's
own "honest reporting" rule, a genuine regression on either axis would
have meant reverting — but the bench itself passed. **An `@fl`
adversarial review caught what the bench could not measure at all.**

Verified from source (`~/.cargo/registry/.../lsm-tree-3.1.6/src/tree/mod.rs`,
fjall's own README "Multithreading" section): **fjall reads are
explicitly designed for genuine multi-threaded concurrent execution.**
`Tree::get` takes a brief shared `RwLock::read()` only to clone an
immutable `SuperVersion` snapshot, then proceeds lock-free (memtable is
a `crossbeam_skiplist::SkipMap`; sealed segments are immutable) — many
threads can call `get()` on the same keyspace truly in parallel, and
fjall's own docs say exactly this is the intended usage
("internally synchronized for multi-threaded access ... without
needing to lock yourself"). Writes are different: `Keyspace::insert`
holds a journal-writer `Mutex` for the whole operation — writes already
serialize inside fjall regardless of which thread calls them.

**Funneling every READ through one dedicated worker thread therefore
discards real, fjall-designed read parallelism**, capping a single hot
table's point-read throughput at whatever ONE OS thread's serial
throughput can achieve — no matter how many CPU cores are available —
while the OLD `spawn_blocking`-per-op design could scale reads toward
the core count via tokio's blocking pool.

**Why the bench couldn't see this:** `FjallStore` sits underneath
`CachedStore`/`MemBufferStore` in this codebase's repo stack
(`crates/shamir-engine/src/repo/repo_types.rs`) — the reads that
actually reach fjall in production are predominantly CACHE MISSES, the
reads most likely to do real disk I/O (bloom filter + index block +
data block, tens of µs to low ms on a cold cache). The prototype's
worker drain loop executes queued jobs strictly sequentially — one
slow cache-miss read stalls every OTHER queued op on that same worker,
including unrelated writes and the `contains_key` probes inside `set`/
`remove`. A bench with an entirely memtable-resident 1 MB dataset can
never exercise this: every "read" is a sub-microsecond skiplist lookup,
so one thread trivially keeps up with 32 concurrent callers and the
measured win is ONLY the (real, but narrower) `spawn_blocking` dispatch
overhead — not a signal about the design's behavior under realistic
cold-cache, high-fan-out, hot-table production load.

**Concrete exposure scenario** (per `@fl`'s analysis): a 16+ core
machine, ≥64 concurrent connections reading a hot table whose working
set exceeds the cache layer — the worker path collapses to fully
serialized single-threaded disk I/O; the old `spawn_blocking` path
overlaps those reads across the blocking pool. This is a >10×
regression risk in that scenario, not a narrow edge case.

## Recommended redesign (NOT implemented — filed as follow-up)

Split routing by operation kind rather than funneling everything
through one worker:

1. **Writes** (`insert`/`set`/`remove`) → KEEP the single worker thread.
   fjall already serializes writes on its own journal mutex, so this
   loses no real parallelism — it's a clean, safe win (amortized
   dispatch, and as a bonus, the worker already-serialized nature
   incidentally closes the `contains_key`-then-`insert` TOCTOU window
   audit finding §1.2/§B13 discusses, since all point-writes now run on
   one thread in submission order).
2. **Reads** (`get`) → a SHARDED POOL of N reader worker threads
   (N ≈ physical core count, keys routed by hash or round-robin), each
   running the same drain-loop shape, OR simply keep reads on the
   existing per-op `spawn_blocking` path (the conservative,
   already-correct fallback) if the sharded-pool design proves not
   worth the added complexity once actually measured against a
   realistic cold-cache bench.
3. **Bench requirement for any successor attempt:** the bench MUST
   include a dataset variant several times larger than the block/cache
   layer (or otherwise force real cache misses) so cold-read
   parallelism-under-contention is actually exercised — the
   memtable-resident-only bench in this reverted attempt is
   insufficient evidence for a production concurrency decision and
   should not be reused as-is; extend it, don't just rerun it.

## Disposition

No code change landed for task #524 — the prototype was fully reverted
from the working tree (never committed). This document + the follow-up
task capture the investigation so a future attempt at the redesign
doesn't have to re-derive the read/write asymmetry from scratch.
