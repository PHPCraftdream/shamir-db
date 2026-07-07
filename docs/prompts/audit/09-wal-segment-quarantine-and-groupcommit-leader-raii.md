Task: two independent HIGH-severity durability fixes from
`docs/audits/2026-07-06-durability-storage-wal-tx.md` (top-5 items #3
and #5). Fix BOTH in this pass — they touch different crates
(`shamir-wal` vs `shamir-engine`) and are unrelated to each other, but
both are top-priority HIGH findings from the same audit.

---

## Fix A — WAL segment poisoning after a partial write / failed fsync
(audit §1.3, top-5 #3)

### Where

`crates/shamir-wal/src/wal_segment.rs:112-147` (`append_batch`).

```rust
pub async fn append_batch(&self, payloads: Vec<Vec<u8>>, max_version: u64) -> DbResult<u64> {
    // ...
    let n = payloads.len() as u64;
    let last_seq = self.next_seq.fetch_add(n, Ordering::AcqRel) + n - 1;
    self.max_committed.fetch_max(max_version, Ordering::AcqRel);
    let file = Arc::clone(&self.file);
    let frame_bytes = spawn_blocking(move || -> DbResult<u64> {
        // ... builds `buf` (all frames coalesced)
        let mut f = file.lock().expect("WalSegment file mutex poisoned");
        f.write_all(&buf)
            .map_err(|e| DbError::Storage(format!("WalSegment append: {e}")))?;
        // <-- on error here, `buf` may be PARTIALLY written to the file.
        //     No rollback, no truncation back to the last good boundary.
        Ok(buf.len() as u64)
    })
    .await
    .map_err(|e| DbError::Internal(format!("spawn_blocking join: {e}")))??;
    self.bytes_written.fetch_add(frame_bytes, Ordering::AcqRel);
    Ok(last_seq)
}
```

Also relevant: `crates/shamir-wal/src/wal_group_commit.rs:264-271` — the
circuit breaker on a failed append only releases leadership; the NEXT
leader keeps writing to the SAME (already-poisoned) segment file.

And the replay side, `wal_segment.rs:217-241` — replay stops at the
first torn/CRC-mismatched frame and silently discards everything after
it (this part is a SEPARATE, already-somewhat-intentional design for
the torn-tail-at-end-of-active-segment case; do not touch replay logic
in this fix — the fix here is entirely about PREVENTING the poisoned
state from being appended to in the first place, not about changing
recovery's read-side tolerance).

### Why this is HIGH

Scenario: ENOSPC mid-`write_all` for window N → the file has a truncated
frame at the end. Space frees up; window N+1 appends successfully AND
gets fsync'd (a `Synced`-durability commit acks the caller). Power loss.
On replay, the reader walks frames in order, hits the torn frame from
window N first, and (per the existing, separate replay design) stops
there — **all of window N+1's acked records are silently lost**, even
though they were themselves written correctly and fsync'd, because
they're unreachable behind the torn tail from the EARLIER failed write.

The same class of loss applies to a failed `fsync` (not just
`write_all`): if fsync fails (or the notorious fsyncgate scenario where
the kernel marks dirty pages clean without actually persisting them), a
subsequent "successful" fsync does not retroactively persist window N —
window N+1 is still stranded behind a hole.

### Fix

On ANY error appending to (or syncing) a segment: **quarantine that
segment** — do not allow further appends to it. Concretely:

1. In `append_batch`'s `spawn_blocking` closure, on `write_all` failure:
   determine how many bytes were actually written before the failure
   (if `write_all` doesn't tell you the partial count directly, you may
   need to switch to a loop of `write` calls tracking bytes-written, or
   use `f.metadata()?.len()` before/after to detect the file grew by a
   partial amount) — then `f.set_len(<last known-good byte offset>)?`
   to truncate the file back to the boundary before this batch started
   (the byte offset recorded via `self.bytes_written.load(...)` BEFORE
   this call, which is already tracked). This removes the torn/partial
   frame(s) from the file entirely.
2. Mark the segment as poisoned (a new `AtomicBool` or similar flag on
   `WalSegment`) so that any FUTURE `append_batch` call on this segment
   returns an error immediately instead of writing to a known-bad file.
   The caller (wal_group_commit.rs's leader logic) must react to this
   error by rotating to a brand-new segment rather than retrying on the
   same fd — check how segment rotation is currently triggered
   elsewhere (`segment_set.rs`) and reuse that path; do not invent a
   new rotation mechanism if one already exists for other reasons (e.g.
   size-based rotation).
3. On an fsync failure specifically (wherever that's plumbed — likely
   `wal_group_commit.rs`'s background fsync path, cross-reference with
   audit finding 1.5 if you find the same code, but 1.5 itself is OUT
   OF SCOPE for this task — only touch what's needed to make an fsync
   failure ALSO poison the segment / trigger rotation, don't fix 1.5's
   separate dirty-flag-loss bug here): treat a failed fsync as "do not
   trust this file anymore" — same quarantine/rotate reaction as a
   write failure, with a loud (not `log::warn!`, use `log::error!` or
   equivalent severity already used elsewhere in this crate for fatal
   conditions) log message, since silent retry-on-same-fd is exactly
   the bug.

### TDD requirement

1. **Red**: write a test that simulates a partial write (you likely need
   to inject a fault — check if `WalSegment`/`append_batch` already has
   any test seam for fault injection in `crates/shamir-wal/src/tests/`
   or similar; if not, the most direct approach is a unit test that
   calls the private truncation-repair logic directly with a
   deliberately-corrupted file state, or a test that opens a segment,
   writes a good batch, then manually truncates the file to simulate a
   torn write, and asserts that a subsequent open/validate call detects
   and repairs it — read the existing test file(s) in
   `crates/shamir-wal/src/tests/` first to match the existing fault-
   injection idiom rather than inventing a new one).
2. **Green**: implement the fix.
3. Confirm existing WAL tests still pass — `./scripts/test.sh -p shamir-wal`.

---

## Fix B — `GroupCommit::run` hangs forever if the leader future is cancelled
(audit §2.1, top-5 #5, independently confirmed by the concurrency-engine
audit as finding A7 — double-confirmed, treat as high-confidence)

### Where

`crates/shamir-engine/src/repo/group_commit/mod.rs:44-76`
(`GroupCommit::run`).

```rust
pub async fn run<F, Fut>(&self, flush: F) -> DbResult<()>
where
    F: Fn() -> Fut,
    Fut: Future<Output = DbResult<()>>,
{
    let rx = {
        let mut s = self.state.lock().await;
        let (tx, rx) = oneshot::channel();
        s.waiters.push(tx);
        if s.leader_busy {
            drop(s);
            return recv(rx).await;
        }
        s.leader_busy = true; // I am the leader.
        rx
    };

    loop {
        let batch: Vec<_> = {
            let mut s = self.state.lock().await;
            std::mem::take(&mut s.waiters)
        };

        let res = flush().await;   // <-- if THIS future is cancelled
                                    //     (caller's task dropped — e.g.
                                    //     client disconnect, tokio::select!
                                    //     race, shutdown), everything below
                                    //     never runs: `leader_busy` stays
                                    //     `true` FOREVER.
        // ... sends results to `batch`, eventually `s.leader_busy = false`
    }
}
```

### Why this is HIGH

Once `leader_busy` is stuck `true`, every subsequent call to
`GroupCommit::run` (any `synced_flush` in this repo) takes the
`if s.leader_busy { return recv(rx).await; }` branch and parks forever
— no leader will ever run again to serve them (there's no timeout, no
retry-elect-new-leader path). This is a durability-flush DoS: any
Synced-durability request for this repo hangs until process restart,
triggered simply by one caller's future being dropped mid-flush (a
client disconnecting under load, a `tokio::select!` timeout racing the
flush, or graceful shutdown cancelling in-flight requests).

### Fix

`tokio::sync::Mutex`-guarded state means a synchronous `Drop` impl
cannot reliably reset `leader_busy` (Drop can't `.await` to acquire the
lock). The audit suggests two options — **prefer the second one**
(cleaner, avoids the async-Drop problem entirely):

> RAII-guard on leader_busy (reset in Drop) + fail waiters with an
> error; **or** run the flush in a separate spawned task, not in the
> body of a cancellable request.

Concretely: when a caller is elected leader, instead of running the
leader loop inline (where `flush().await` is subject to the CALLING
task's cancellation), spawn the entire leader loop as its own detached
`tokio::task` (`tokio::spawn`) so it is NOT tied to the lifetime of the
original caller's future. The elected caller then just does
`recv(rx).await` like every other (non-leader) caller — if the caller's
own future is dropped, only ITS wait on `rx` is abandoned; the spawned
leader task keeps running to completion regardless, correctly resetting
`leader_busy` and serving every other waiter (including ones that
arrive after the original caller is gone).

Sketch (adapt to fit the actual types/lifetimes — `F: Fn() -> Fut` where
`Fut` may not be `'static`/`Send` as currently written; check whether
`flush`'s closure needs an `Arc`/owned-capture adjustment to be
spawnable, and handle that adjustment as part of this fix):

```rust
if s.leader_busy {
    drop(s);
    return recv(rx).await;
}
s.leader_busy = true;
drop(s);

// Spawn the leader loop so caller cancellation can't abandon it mid-flush.
let state = /* whatever handle self.state needs — Arc<Self> or similar */;
tokio::spawn(async move {
    loop {
        let batch: Vec<_> = { /* take waiters under lock */ };
        let res = flush().await;
        // ... send to batch ...
        // ... check waiters.is_empty(), else loop ...
    }
});

recv(rx).await
```

If `GroupCommit` isn't already held behind an `Arc` by its callers,
you may need to change its API to require `self: Arc<Self>` for `run`,
or restructure so the spawned task can safely reference `self`'s state
without a lifetime conflict — check how `GroupCommit` is constructed
and held by its caller(s) (likely `repo_instance.rs`) before deciding
the exact shape.

### TDD requirement

1. **Red**: write a test in `crates/shamir-engine/src/repo/group_commit/tests/`
   (a `tests` module already exists per the `#[cfg(test)] mod tests;` at
   the bottom of `mod.rs`) that:
   - Spawns a leader call whose `flush` future is designed to be
     cancelled mid-flight (e.g. wrap the flush in a future that you
     drop/abort partway through — `tokio::select!` racing against a
     short sleep, or an explicit `JoinHandle::abort()` on the task
     driving the leader call).
   - Asserts that a SUBSEQUENT `run()` call (simulating another waiter
     arriving after the cancellation) still completes successfully
     within a bounded time (use a `tokio::time::timeout` in the test
     assertion) — before the fix, this should hang/timeout because
     `leader_busy` is stuck.
2. **Green**: implement the fix.
3. Confirm existing group_commit tests still pass:
   `./scripts/test.sh -p shamir-engine -- group_commit`

---

## Gate (must be clean before finishing, for BOTH fixes)

```
cargo fmt -p shamir-wal -p shamir-engine -- --check
cargo clippy -p shamir-wal -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-wal
./scripts/test.sh -p shamir-engine -- group_commit
```

If clippy flags PRE-EXISTING lints in code you did not touch, do not
fix them here — note them in your final report instead.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

For EACH fix (A and B), report:
- The failing test you wrote and what it asserted before/after.
- The exact mechanism of the fix (quarantine/truncation logic for A;
  spawn-detach structure for B).
- Any API/type changes required (e.g. if `GroupCommit::run` needed
  `Arc<Self>` or similar) and why.
- Gate results (exact commands + pass/fail).
- Any pre-existing clippy/fmt issues found but not touched.
