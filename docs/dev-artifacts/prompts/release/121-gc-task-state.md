# Brief: GC-TASK-STATE (item 19 of SHUTDOWN-ORACLES, #2008), done by hand

Found by the review of the flake branch (FLAKE #2009): the token-GC join is now bounded in the public shutdown path, and an expiry abandons the join. The state check that follows lies.

## Defect

`TokenService::shutdown_gc` cancels the stop token, takes the join handle out of the `gc_task` slot and awaits it. A shutdown that abandons this await at its deadline leaves the collector task running (parked behind the blocking pool or on the clock) with the slot already empty. `gc_task_finished` then read `gc_shutdown.is_cancelled()` for an empty slot, so it reported true for a collector that had not ended. Anything that gates on it (the shutdown report, the boot tests) accepts a running collector as stopped.

## Change

* `TokenService` counts the live collector tasks itself: `start_gc` increments `gc_live` under the slot lock before it spawns, and the task owns a drop guard (`LiveCollector`) that decrements it, so a normal return, a panic and an abort all count the task out. `gc_spawned` marks that a collector was ever started.
* `gc_task_finished` is the live count after a spawn (zero means every collector ended) and the old stop-token answer before any spawn. A counter, not one exit flag, because a collector restarted after an abandoned or completed `shutdown_gc` (the deadline tests do exactly that) is a new live task and must not inherit the first one's end.
* Tests (`tokens/tests/gc_task_tests.rs`, new file because `service_tests.rs` is near the file limit): an abandoned `shutdown_gc` over a collector parked in the clock keeps `gc_task_finished` false until the clock is released; a collector restarted after a completed shutdown is not reported finished while its pass is parked.

## Non-goals

No change to `shutdown_gc` semantics, to the join bound in `ServerHandle`, or to the scheduled-worker join (item 18).

## Acceptance

`fmt`, clippy `-D warnings` for `shamir-server-security`, the layout gate and the visibility check exit 0; full-tier CI green on the head; the two new tests pass on both OS and the existing boot and deadline tests that read `gc_task_finished` stay green.
