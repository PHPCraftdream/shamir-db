# Brief: FLAKE `public_shutdown_bounds_meta_snapshot_join_without_outer_deadline` (task #2009), done by hand

## Symptom

On the Ubuntu lib run of an unrelated branch the test failed with `public shutdown must settle with the blocking pool parked: Elapsed(())` after its own 180 s bound; it passed on rerun (a green run took about 30 s). The failing log has no stage evidence.

## Evidence and inference

* The panic is the test's own `timeout`, so the runtime thread was free: an ASYNC wait never completed.
* The public `ServerHandle::shutdown()` has no outer deadline. Its late stages are bounded (audit 10 s, repository flush 15 s, observability 5 s, meta snapshot 5 s, store release 10 s). The token-GC join was awaited UNBOUNDED in that path, and the GC pass submits a blocking job while the test parks the pool's only thread for the whole shutdown, so a pass queued behind it never completes.
* Which stage hung is INFERRED, not observed. A static trace of the fixture (no scheduled backup, no follower, reapers with in-memory ticks, REST gate worker with its own 2 s drain) finds no other unbounded pool-dependent wait; the launcher's own first GC pass lands before or behind the parked job depending on scheduling.

## Change

1. `server_handle.rs`: the token-GC join is bounded by `CONN_DRAIN_TIMEOUT` in the no-deadline path too, reported as `token GC shutdown exceeded`; one `shutdown stage` debug event marks the start of each phase that can wait without a bound (token GC, replication, scheduled worker, reapers).
2. The test restarts a GC pass with no await before parking the pool (deterministic queued job), asserts the GC expiry line, adopts `FlakeGuard` with a 150 s watchdog and the tracing ring.
3. Registered in `docs/dev-artifacts/design/flake-failure-logs.md`.

## Review result (independent review, verdict FAIL on scope, re-scoped)

* The replication stop is NOT bounded in the public path (a first version did): `stop_all` takes the reconcile lock BEFORE it signals any follower, so abandoning it on expiry would let the flush run beside live followers. Fencing new follower starts and signalling followers independently of the lock is tracked in SHUTDOWN-ORACLES; it cannot be the cause in this fixture (no follower, the periodic reconcile skips its first tick and shutdown cancels the root before its second).
* The scheduled-worker join stays unbounded in the public path by design (cleanup and instance-lock ordering); a scheduled backup that reaches storage while the pool is starved can still stall it. Tracked in SHUTDOWN-ORACLES; the shutdown rustdoc states the exception.
* After a GC join expiry the collector task is detached and `gc_task_finished()` reads true (handle absent plus cancelled). Tracked in SHUTDOWN-ORACLES.

## Non-goals

No timeout is raised, no assertion of the meta-snapshot stage changes, the gate and reaper joins stay as they are.

## Acceptance

`fmt`, clippy `-D warnings` for `shamir-server`, the layout gate and the visibility check exit 0; CI full tier green on the head, and the test passes with the GC expiry line present.
