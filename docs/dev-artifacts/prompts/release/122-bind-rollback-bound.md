# Brief: BIND-ROLLBACK-BOUND (item 14 of SHUTDOWN-ORACLES, #2008), done by hand

Found by the landing review of BACKUP-TAILS-SERVER and deliberately left out of that landing.

## Defect

`ServerLauncher::launch_inner` rolls a failed launch back at two stages (listener bind and observability bind). Both rollbacks ended with `let _ = snap.handle.await` on the periodic meta-snapshot task. The task writes one final lockout and rate-limit snapshot after the root token is cancelled, and that write is a `spawn_blocking` job. With the blocking pool saturated or stalled the job never starts, the join never completes, and the launch never returns its boot error: the process hangs where it should report the bind failure.

## Change

* `support/rollback_cleanup.rs`: `join_meta_snapshot(stage, task)` joins the task under a 5 s bound (the value the graceful shutdown uses for the same task). An expiry is logged at error level through the same `launch rollback cleanup error` event as every other cleanup failure and the task is left to finish on its own (it may hold the `server_meta` store lock until then). The boot error still wins.
* `launch.rs`: both rollbacks call it instead of awaiting the handle.
* Test (`rollback_cleanup_tests.rs`): the pool's only blocking thread is parked, a real meta-snapshot task is spawned and told to stop, and the join must return, log the expiry for its stage, and return while the pool is still occupied.

## Non-goals

The two reaper joins in the rollbacks (their ticks are in-memory and do not touch the pool), the GC join (`shutdown_gc` before `start_gc` has nothing to join at the observability stage), the graceful shutdown path, and the "real failed launch" cleanup test (item 4 of the task, a separate seam).

## Acceptance

`fmt`, clippy `-D warnings` for `shamir-server`, the layout gate and the visibility check exit 0; full-tier CI green on the head; the new test passes on both OS.
