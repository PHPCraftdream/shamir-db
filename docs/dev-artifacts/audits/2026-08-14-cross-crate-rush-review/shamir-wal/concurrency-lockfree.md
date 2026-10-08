<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-wal — concurrency-lockfree independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

The normal append path keeps guards out of sink awaits. Its progress failure, cancellation exposure and dirty-clear race remain; architectural single-appender reasoning must not be generalized to all metadata and sync access.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 9 | 5 | 0 | 1 | 1 | 0 | 2 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — Circuit-breaker exit strands parked waiters indefinitely (no wakeup, no self-rescue)

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

Followers admitted after mem::take fail their only CAS and remain uncompleted when the failing leader returns. Background fsync does not drain pending.

Evidence: [crates/shamir-wal/src/wal_group_commit.rs:277](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_group_commit.rs#L277); [crates/shamir-wal/src/wal_group_commit.rs:327](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_group_commit.rs#L327).

Grouping/duplicate: [correctness-tdd.md#1](correctness-tdd.md#review-1). This is not an additional independent defect.

<a id="review-2"></a>

### Claim 2 — WalSegment::append_batch rollback and bytes_written accounting are unsafe under concurrent callers, while the bench claims correctness coverage of exactly that shape

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

B can capture offset zero before A succeeds; B's later failed write truncates A's acknowledged bytes to zero. Accounting resumes outside serialization and rollback drops the guard. This bypasses the documented production single-appender model; the benchmark neither injects failure nor checks replay.

Evidence: [crates/shamir-wal/src/wal_segment.rs:220](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_segment.rs#L220); [crates/shamir-wal/src/wal_segment.rs:248](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_segment.rs#L248); [crates/shamir-wal/src/wal_segment.rs:270](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_segment.rs#L270); [crates/shamir-wal/benches/segment_set_lock.rs:100](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/benches/segment_set_lock.rs#L100).

<a id="review-3"></a>

### Claim 3 — Leader tenure is unbounded under sustained arrival — the leader's own append future cannot resolve

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The leader checks its completed waiter only after drain-until-empty returns. Sustained arrivals keeping each pending observation nonempty delay that caller indefinitely while followers make progress.

Evidence: [crates/shamir-wal/src/wal_group_commit.rs:185](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_group_commit.rs#L185); [crates/shamir-wal/src/wal_group_commit.rs:273](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_group_commit.rs#L273).

<a id="review-4"></a>

### Claim 4 — dirty_since_sync boolean has a clear-race against concurrent Buffered completions

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

After fsync releases the file mutex, a new Buffered write and its true publication can finish before sync_now resumes and stores false. Quiet-state retry is then lost; the page-cache acknowledgement itself remains valid.

Evidence: [crates/shamir-wal/src/wal_group_commit.rs:300](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_group_commit.rs#L300); [crates/shamir-wal/src/wal_group_commit.rs:355](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_group_commit.rs#L355); [crates/shamir-wal/src/wal_group_commit.rs:393](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_group_commit.rs#L393).

<a id="review-5"></a>

### Claim 5 — MemSink holds a std::sync::Mutex across O(N) CPU work inside async fns

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Replay decodes the corpus under frames; retain and any also scan under it. There is no guard across await, but work and worker blocking scale with backlog. No measured contention claim follows.

Evidence: [crates/shamir-wal/src/wal_sink.rs:162](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_sink.rs#L162); [crates/shamir-wal/src/wal_sink.rs:188](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_sink.rs#L188); [crates/shamir-wal/src/wal_sink.rs:205](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_sink.rs#L205).

<a id="review-verified-lock-placement-and-map-pillars"></a>

### Claim Verified/lock-placement-and-map-pillars — Lock placement, CAS/Notify ordering, and inapplicable Fx/scc rules

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

File guards are in blocking closures; metadata/pending guards end before sink awaits. Waiter outcome publication and CAS ordering are suitable. No production hash-map or scc length site exists here.

Evidence: [crates/shamir-wal/src/wal_segment.rs:235](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_segment.rs#L235); [crates/shamir-wal/src/wal_group_commit.rs:103](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/wal_group_commit.rs#L103); [crates/shamir-wal/src/segment_set.rs:224](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/segment_set.rs#L224).

<a id="review-verified-all-lock-sites-o1"></a>

### Claim Verified/all-lock-sites-O1 — All SegmentSet lock sites are O(1)

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

Sealed-vector clone, retain, any and maximum computation are linear. The ordinary active-Arc clone alone is constant work.

Evidence: [crates/shamir-wal/src/segment_set.rs:427](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/segment_set.rs#L427); [crates/shamir-wal/src/segment_set.rs:470](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/segment_set.rs#L470); [crates/shamir-wal/src/segment_set.rs:587](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/segment_set.rs#L587).

<a id="review-verified-concurrent-replay-truncation-tolerance"></a>

### Claim Verified/concurrent-replay-truncation-tolerance — Claim-then-delete and replay provide Windows delete-pending tolerance

Status: `partially-fixed`. Current risk: `medium`.

Prior-cycle decision: `partially-fixed`.

Claiming prevents two truncators targeting one tracked segment. Actual replay instead create-opens the snapshot path and uses strict startup replay; live drainer recover calls make its race protection incomplete.

Evidence: [crates/shamir-wal/src/segment_set.rs:470](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/segment_set.rs#L470); [crates/shamir-wal/src/segment_set.rs:435](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/segment_set.rs#L435); [crates/shamir-engine/src/tx/drainer.rs:368](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/tx/drainer.rs#L368).

Grouping/duplicate: [error-handling-lifecycle.md#5](error-handling-lifecycle.md#review-5). This is not an additional independent defect.

<a id="review-verified-rotation-double-guards"></a>

### Claim Verified/rotation-double-guards — Rotation rechecks active sequence before publishing

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Both rotations recheck active_seq before publishing. This protects publication against a completed competing rotation, not concurrent writes or a captured stale maximum.

Evidence: [crates/shamir-wal/src/segment_set.rs:319](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/segment_set.rs#L319); [crates/shamir-wal/src/segment_set.rs:393](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wal/src/segment_set.rs#L393).

## Evidence and recipe corrections

- Single-leader appends do not imply zero contention from background sync, drainer probes, truncation or replay.
- A rollback-safe concurrent implementation must serialize boundary capture, writes, rollback, accounting and poison admission, not move just one load.
- A fair handoff must guarantee a successor for already-queued work; clearing leadership and waiting for an unspecified future pusher is insufficient.
- Mem replay must snapshot non-destructively; mem::take would remove the WAL.
- The proposed epoch recipe must snapshot a write epoch before synchronization and conservatively advance only that epoch on success, including concurrent sync calls.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-wal -- Concurrency & lock-free invariants

## Summary

The crate's three locks are each inline-justified and match CLAUDE.md's closed adjudications: `SegmentSet::inner` (O(1) critical section, single-leader model, per the #1095/#1109 entry), `WalSegment::file` (locked only inside `spawn_blocking`, never across `.await`), and `WalGroupCommit::pending` (sanctioned `tokio::sync::Mutex`). No lock is ever held across an `.await`, there is no `parking_lot`/`RwLock` usage, atomics are ordered correctly (CAS leader election, `fetch_max` watermarks, enable-before-check `Notify` park), and pillars 4/5 are N/A (no hash-keyed or concurrent-map structures in the crate; no `scc::*::len()` anywhere). The one substantive defect is a liveness hole in `WalGroupCommit`'s error path: the circuit-breaker exit can strand already-parked waiters indefinitely, which falls under the repo's "hangs are BUGS" rule. Remaining findings are lower-severity races and accounting weaknesses that are safe only under the crate's documented single-writer model — a model its own bench deliberately violates while claiming correctness coverage it does not have.

## Findings

### 1. Circuit-breaker exit strands parked waiters indefinitely (no wakeup, no self-rescue)
**File:** `crates/shamir-wal/src/wal_group_commit.rs:324-330` (park loops at `:189-197` and `:249-257`; `mem::take` at `:277`)
**Severity:** high

**Issue.** When the leader's window fails (write or fsync error), `lead_until_drained` completes only the waiters whose entries were in the *taken* batch (`:302-306`, `:319-323`), then does `flushing.store(false)` and returns — leaving any entries pushed *after* the `mem::take` sitting in `pending` with their owners parked on `waiter.notify.notified()`. Those owners already failed their `flushing` CAS (it was still `true` when they tried), so they will not re-attempt leadership; they only observe completion when some *future* `append`/`append_many` caller wins the CAS and drains the queue. Note the exit at `:328` also releases leadership *outside* the `pending` lock, unlike the normal-exit path at `:273-275` that upholds the L1 invariant.

**Failure scenario.** Leader L takes batch [E1]; committer T2 pushes E2 and fails its CAS (L still holds `flushing`), then parks. L's `append_batch` fails (ENOSPC/EIO → segment poisoned); L completes E1's waiter with `Err` and exits via the circuit breaker. E2 remains in `pending`; T2 is parked with no notifier. If no further append ever arrives (a poisoned WAL is exactly when the engine may stop committing), T2's commit future never resolves — an unbounded hang, which CLAUDE.md classifies as a bug to fix, never tolerate. The existing failure test (`append_many_write_failure_leaves_zero_entries`, `wal_group_commit_tests.rs:377-424`) is single-caller and does not cover this.

**Suggested fix.** On the circuit-breaker path, re-acquire the `pending` lock, `mem::take` the remainder, complete every remaining waiter with `Err` (mirroring the taken batch's outcome), and only then `store(false)` — this preserves L3's "no task spins on a dead segment" while unblocking everyone. Add a regression test: arm `arm_fail_next_append`, race a follower `append` against a leader whose window fails, and assert the follower resolves (Err) within a bounded wait rather than parking forever.

### 2. `WalSegment::append_batch` rollback and `bytes_written` accounting are unsafe under concurrent callers, while the bench claims correctness coverage of exactly that shape
**File:** `crates/shamir-wal/src/wal_segment.rs:213-220` (seq `fetch_add` + `pre_batch_offset` load before the blocking write), `:248-252` (`set_len(pre_batch_offset)` rollback), `:270` (`bytes_written.fetch_add`); bench `crates/shamir-wal/benches/segment_set_lock.rs:100-153, 235-308`
**Severity:** low

**Issue.** `pre_batch_offset` is loaded *before* the `spawn_blocking` write, so under two concurrent appenders on one segment, B's failed `write_all` rolls the file back to an offset captured before A's concurrent successful write — `set_len` would truncate away A's already-acked frames. `bytes_written` interleavings can likewise produce a wrong rollback boundary. This is safe today only because of the documented single-leader model (`wal_segment.rs:82-94`; CLAUDE.md's closed `SegmentSet::inner` entry). However, `benches/segment_set_lock.rs` deliberately runs N concurrent `append_batch` callers (`raw_append`, `sustained_append`, `append_truncate_concurrent`) against the shared active segment and labels `raw_append` a "lock CORRECTNESS stress test… proving `append_batch` stays correct under worst-case, all-N-contend concurrency" — a claim the bench never actually verifies: it injects no failures, so the racy rollback path is never exercised, and there is no post-run replay/CRC check of the written frames.

**Failure scenario.** Only if the single-writer model is ever relaxed (e.g., someone cites the bench as evidence that concurrent `SegmentSet::append_batch` is supported): a mid-bench ENOSPC under `sustained_append` would silently discard other tasks' acked frames.

**Suggested fix.** Either (a) document on `WalSegment::append_batch` that concurrent calls are correctness-unsupported (single-leader contract) and soften the bench's "correctness stress test" claim to "no-failure concurrency smoke", or (b) make the path actually safe — e.g., read the offset and `fetch_add` `bytes_written` inside the file-lock critical section on the blocking thread (keeping the lock held only there, never across `.await`). If (a), a fault-injecting concurrent test would still be worthwhile to pin the boundary.

### 3. Leader tenure is unbounded under sustained arrival — the leader's own append future cannot resolve
**File:** `crates/shamir-wal/src/wal_group_commit.rs:269-331`
**Severity:** low

**Issue.** `lead_until_drained` returns only when it observes `pending` empty. The leader task's *own* waiter may have been completed after its first window, but the task is inside `lead_until_drained`, not the park loop — it cannot return until the queue drains. Under sustained load (arrival rate ≥ service rate; the doc itself notes fsync dominates a durable append ~63×), every empty-check can see new entries, so the current leader is pinned indefinitely and its caller's commit future never resolves even though its own entry is physically durable. This is degraded service time for one committer at a time, not a deadlock (progress continues), hence low.

**Suggested fix.** Bound consecutive windows per tenure and hand leadership off — but only *after* finding 1 is fixed, since a non-draining handoff would strand waiters exactly the same way (a handoff must either drain or complete the leftovers). Alternatively, once the leader's own waiter is `done`, release leadership under the `pending` lock with the same leftover-completion guard, letting a fresh pusher re-elect.

### 4. `dirty_since_sync` boolean has a clear-race against concurrent Buffered completions
**File:** `crates/shamir-wal/src/wal_group_commit.rs:299-301` (set per Buffered window), `:355-362` (`sync_now`: fsync then `store(false)`), `:386-410` (timer `take_dirty` → `sync_now`)
**Severity:** nit

**Issue.** The background timer's `sync_now` does `sink.sync().await` and only then `store(false)`; a Buffered window completing between the fsync's completion and the `store(false)` (or between `take_dirty()` and the fsync, when the fsync misses the new bytes) has its dirty flag wiped without its bytes being flushed. The timer's promised "bounded power-loss window" silently reopens until the next Buffered append re-sets the flag. The `Buffered` tier's ack contract (page-cache only) is not violated, so this is a nit.

**Suggested fix.** Replace the boolean with an epoch pair: `write_epoch: AtomicU64` (`fetch_add` on each Buffered window) and `synced_epoch: AtomicU64` (`fetch_max(epoch_at_fsync_start)` on successful fsync); dirty ⇔ `write_epoch > synced_epoch`. This is race-free without any lock.

### 5. `MemSink` holds a `std::sync::Mutex` across O(N) CPU work inside async fns
**File:** `crates/shamir-wal/src/wal_sink.rs:161-169` (`replay` decodes every frame under the lock), `:185-191` (`truncate_below` `retain`), `:203-207` (`has_truncatable` scan)
**Severity:** nit

**Issue.** `WalSink::replay`'s `Mem` arm holds `frames` across bincode-decoding the entire WAL (O(total bytes) CPU) inside an async fn. The lock is never held across an `.await` (compliant with the hard rule), but a concurrent append on a multi-threaded runtime blocks a worker thread on a `std::sync::Mutex` for the whole replay. These are rare paths (recovery / drainer gate) on the in-memory sink only, and the struct's inline comment sanctions the lock, hence nit.

**Suggested fix.** In `replay`, clone (or `mem::take` into a fresh Vec) the frames under the lock and decode after releasing it — replay is a snapshot anyway; the decode outcome per frame is unchanged.

---

**Verified consistent with CLAUDE.md (no action):** `SegmentSet::inner` (`segment_set.rs:50-60`, matches the closed #1095/#1109 adjudication; all 13 lock sites are O(1) and never span an await); `WalSegment::file` (`wal_segment.rs:82-107`, locked only inside `spawn_blocking` closures); `WalGroupCommit::pending` (sanctioned async exception, O(1) push/`mem::take`); claim-then-delete truncation vs concurrent replay with Windows delete-pending tolerance (`segment_set.rs:455-548`, `wal_segment.rs:428-450`); rotation double-guard re-checks (`segment_set.rs:319-322, 393-397`); pillars 4/5 N/A — no hash-keyed or `scc`/`DashMap` structures exist in this crate, so the Fx-hash and `scc::*::len()` rules have no applicable sites.

</details>
