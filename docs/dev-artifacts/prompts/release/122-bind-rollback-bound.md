# Brief: BIND-ROLLBACK-BOUND (item 14 of SHUTDOWN-ORACLES, #2008), done by hand

Found by the landing review of BACKUP-TAILS-SERVER and deliberately left out of that landing.

## Defect

`ServerLauncher::launch_inner` rolls a failed launch back at two stages (listener bind and observability bind). Both rollbacks ended with `let _ = snap.handle.await` on the periodic meta-snapshot task. The task writes one final lockout and rate-limit snapshot after the root token is cancelled, and that write is a `spawn_blocking` job. With the blocking pool saturated or stalled the job never starts, the join never completes, and the launch never returns its boot error: the process hangs where it should report the bind failure.

## Change

* `support/rollback_cleanup.rs`: `join_meta_snapshot(stage, task)` joins the task under a 5 s bound (the value the graceful shutdown uses for the same task). An expiry is logged at error level through the same `launch rollback cleanup error` event as every other cleanup failure and the task is left to finish on its own (it may hold the `server_meta` store lock until then). The boot error still wins.
* `launch.rs`: both rollbacks call it instead of awaiting the handle. The expiry text states only what is observed (the task did not stop within 5 s, it may still hold the `server_meta` store lock, an immediate retry on the same data directory can fail); the `meta_sinks.rs` comment no longer claims nothing is ever detached.
* The executable: a failed run no longer drops its Tokio runtime unbounded. Dropping a runtime waits for running blocking jobs without a limit, and a stalled final snapshot would keep the process alive before the boot error is printed, so `main.rs` and the Windows service entry call `shutdown_timeout(5 s)` on the error path only (the graceful path is unchanged).
* Test (`rollback_cleanup_tests.rs`): the pool's only blocking thread is parked, a real meta-snapshot task is spawned, told to stop and given scheduler turns to queue its final job; with the clock paused and advanced explicitly the join must be pending just before 5 s, return after it, log the expiry for its stage (the `error="..."` field is checked), and return while the pool is still occupied.

## Review result (round 1, verdict FAIL: one P1, two P2, all addressed)

* P1: the launcher returned the bind error but the foreground process could still hang before printing it (runtime drop): fixed by the bounded runtime shutdown above.
* P2: the log asserted a cause it cannot observe (queued behind a saturated pool): reworded to the observed facts. P2: the test did not prove the bound or the submission of the final job: replaced by the explicit-clock test above (a mutant bound fails the middle or the last poll; the task runs to its queued job before the first poll).
* Left as it is, with the reason: no test drives either real bind-failure branch of `launch_inner` with a saturated pool (it needs a seam that parks the pool between the bind failure and the rollback; the helper both branches call is covered, and the call sites are two one-line calls read in the review).

## Non-goals

The two reaper joins in the rollbacks (their ticks are in-memory and do not touch the pool), the GC join (`shutdown_gc` before `start_gc` has nothing to join at the observability stage), `gate.shutdown()` and `teardown_audit` (both have their own bounds), the graceful shutdown path, and the "real failed launch" cleanup test (item 4 of the task, a separate seam).

## Acceptance

`fmt`, clippy `-D warnings` for `shamir-server`, the layout gate and the visibility check exit 0; full-tier CI green on the head; the new test passes on both OS.
