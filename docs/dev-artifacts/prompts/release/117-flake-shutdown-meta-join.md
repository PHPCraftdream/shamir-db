# Brief: FLAKE `public_shutdown_bounds_meta_snapshot_join_without_outer_deadline` (task #2009), done by hand

## Symptom

On the Ubuntu lib run of an unrelated branch the test failed with `public shutdown must settle with the blocking pool parked: Elapsed(())` after its own 180 s bound; it passed on rerun (a green run takes about 30 s). The failing log has no stage evidence.

## Evidence and inference

* The panic is the test's own `timeout`, so the runtime thread was free: an ASYNC wait never completed.
* The public `ServerHandle::shutdown()` has no outer deadline. Its late stages are bounded (audit 10 s, repository flush 15 s, observability 5 s, meta snapshot 5 s, store release 10 s). Three early joins were awaited UNBOUNDED in that path: the REST gate maintenance worker (own 2 s drain, not pool-bound), the token GC task and the replication supervisor stop. The GC pass and a reconcile pass submit blocking jobs, and the test parks the pool's only thread for the whole shutdown, so a job queued behind it never completes.
* Which of the two joins hung is INFERRED, not observed (the launcher's own first GC pass lands before or behind the parked job depending on scheduling).

## Change

1. `server_handle.rs`: the token-GC join and the replication-supervisor join are bounded by `CONN_DRAIN_TIMEOUT` in the no-deadline path too, reported as `token GC shutdown exceeded` / `replication supervisor shutdown exceeded`; one `shutdown stage` debug event per teardown stage.
2. The test restarts a GC pass with no await before parking the pool (deterministic queued job), asserts the GC expiry line, adopts `FlakeGuard` with a 150 s watchdog and the tracing ring.
3. Registered in `docs/dev-artifacts/design/flake-failure-logs.md`.

## Non-goals

No timeout is raised, no assertion of the meta-snapshot stage changes, the gate and reaper joins stay as they are (the gate worker has its own bound, the reapers run in-memory ticks).

## Acceptance

`fmt`, clippy `-D warnings` for `shamir-server`, the layout gate and the visibility check exit 0; CI full tier green on the head, and the test passes with the GC expiry line present.
