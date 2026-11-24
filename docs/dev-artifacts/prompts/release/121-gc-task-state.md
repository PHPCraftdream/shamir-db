# Brief: GC-TASK-STATE (item 19 of SHUTDOWN-ORACLES, #2008), done by hand

Found by the review of the flake branch (FLAKE #2009): the token-GC join is now bounded in the public shutdown path, and an expiry abandons the join. The state check that follows lies.

## Defect

`TokenService::shutdown_gc` cancels the stop token, takes the join handle out of the `gc_task` slot and awaits it. A shutdown that abandons this await at its deadline leaves the collector task running (parked behind the blocking pool or on the clock) with the slot already empty. `gc_task_finished` then read `gc_shutdown.is_cancelled()` for an empty slot, so it reported true for a collector that had not ended. Anything that gates on it accepts a running collector as stopped: the oracle's consumers are the token-service test after `shutdown_gc` and the boot test that polls it during a normal shutdown (the shutdown report itself reports a GC join expiry directly and does not read it).

## Change

* `TokenService` counts the live collector tasks itself: `start_gc` increments `gc_live` under the slot lock before it spawns, and the task owns a drop guard (`LiveCollector`) that decrements it, so a normal return, a panic and an abort all count the task out (a spawn that fails drops the never-polled future and the guard with it). `gc_spawned` marks that a collector was started and is published only after the spawn succeeded. The count can legitimately be zero by the time `start_gc` returns if the collector has already ended.
* `gc_task_finished` is the live count after a spawn (zero means every collector ended) and the old stop-token answer before any spawn; it takes the slot lock, so the two reads are one snapshot against `start_gc` (without it a first start plus a shutdown landing between the flag read and the token read could report a live collector as finished; review round 1). A counter, not one exit flag, because a collector restarted after an abandoned or completed `shutdown_gc` (the deadline tests do exactly that) is a new live task and must not inherit the first one's end.
* The count guard wraps the collector future from outside (`async move { let _live = live; collector.await }`), so the count reaches zero only after the collector and the store handles it captured are destroyed; a handle still in the slot must also report `is_finished()` (review round 2).
* Tests (`tokens/tests/gc_task_tests.rs`, new file because `service_tests.rs` is near the file limit; the clock announces every parked pass and each test waits for that signal before asserting): an abandoned `shutdown_gc` over a parked collector keeps `gc_task_finished` false until the clock is released; a collector restarted after a completed shutdown is not reported finished while its pass is parked; with two collectors alive (abandoned shutdown, then a restart) the first one ending does not report finished while the second is parked (the first collector's end is witnessed independently through the runtime's alive-task count before the answer is read).

## Non-goals

No change to `shutdown_gc` semantics, to the join bound in `ServerHandle`, or to the scheduled-worker join (item 18).

## Acceptance

`fmt`, clippy `-D warnings` for `shamir-server-security`, the layout gate and the visibility check exit 0; full-tier CI green on the head; the three new tests pass on both OS and the existing boot test that polls `gc_task_finished` and the deadline test that restarts a pass stay green.
