# Brief: REPL-STOP-FENCE (item 17 of SHUTDOWN-ORACLES, #2008), done by hand

Found by the review of the flake branch (FLAKE #2009) and by the landing review of BACKUP-TAILS-SERVER (item 3 there).

## Defect

`ServerHandle` shutdown stops the follower-replication supervisor with `SubscriptionSupervisor::stop_all`, which takes the reconcile lock BEFORE it signals any follower. A reconcile pass in flight holds that lock across its catalogue read (a blocking-pool read), so under a stalled pool `stop_all` waits for the lock. On the deadline path the stage cap abandons it, and nothing has been signalled: every follower keeps running (each has its own cancellation token; the root shutdown token only stops the reconcile ticker), the repository flush still runs (`quiesced` stays true), and `shutdown_clean` requires commit and apply admission to be stopped. On the public path (no outer deadline) the same wait is unbounded: it was left unbounded in the flake fix precisely because bounding it without this fix would abandon the stop before any signal.

## Change

* `SubscriptionSupervisor` owns a stop token. Every follower's cancellation token is a CHILD of it (`start_subscription`), and `begin_stop` (new) cancels the parent: one synchronous cancel reaches every running follower with no lifecycle or registry lock and no await (the token's own cancel visits its children synchronously, which is the signal delivery, not part of the bounded join), and a follower started afterwards is born cancelled. A new `reconcile` returns once the token is cancelled, but it checks the token only after it took the lifecycle lock, so it can still wait behind an admitted pass. `stop_all` is unchanged (it cancels the children one by one and never the parent; many tests call it mid-test and keep using the supervisor).
* The follower loop's Hello and Pull are cancellation-aware (`tokio::select!` on the token), and a cancellation seen after the pull reply, and again right after the apply gate was taken, admits no apply (an apply already entered always finishes). Without this a stop waited for a silent upstream up to the production request timeout (35 s), which the new stage cap would turn into an unclean report and a skipped flush. Dropping a pull before anything is applied is safe: the next start pulls again from the durable bookmark.
* A start that registers its handle after every follower already exited (a stop signalled between the spawn and the insert) sweeps the dead handle itself: the last exit invalidates the lease before it deregisters, so either that removal sees the handle or the post-insert check sees the invalid lease.
* `ServerHandle` shutdown: `begin_stop()` first, then the asynchronous join is bounded by `CONN_DRAIN_TIMEOUT` with OR without an outer deadline; an expiry reports `replication supervisor shutdown exceeded` and sets `quiesced = false`, so the repository flush is skipped (the followers were already signalled and exit at their next cancellation checkpoint; a follower can still wait on loading its receipt or on the replication control lock before it reaches one). The flush-skipped message names replication. The `shutdown_with_report` rustdoc drops the replication exception.
* Tests (`multi_target_exit_tests.rs`): a running two-target follower exits after `begin_stop` alone, and a later `reconcile` does not restart the still-active row (asserted active); a follower whose start is parked before its spawn (start race hook, under the lifecycle lock) while `begin_stop` runs ends up with no running follower, no registry entry, no lease and no `stop_all`; both followers of a start exit while the start is parked between the spawn and the registration (registration hook), and the released start leaves no registered handle; a follower that waits on a silent Hello or Pull exits on `begin_stop`; a stop seen while a received batch waits for the apply gate (held by the test) applies nothing.

## Review result (independent review, verdict FAIL, all three P2 fixed)

* The first version cancelled by walking the registry with a fence pairing and a post-insert recheck: the review showed that the synchronous `scc` walk can itself block on a bucket lock before the stage timer starts, that the late-registration race was untested although a seam existed (the start race hook), and that a 10 s cap turns a follower that only waits for the network into an unclean shutdown with a skipped flush. The parent/child token design removes the walk and the fences; the late-registration test uses the existing hook; Hello and Pull are cancellation-aware.
* Noted and left as it is: the rustdoc of `shutdown_with_report` is not an inventory of every unbounded wait (the public path still awaits the two reaper joins directly; their ticks are in-memory), and a reconcile pass already past its stop check can still spawn a follower, which is born cancelled.

## Review result, round 2 (verdict FAIL against the redesign, three P2 and one P3, all fixed)

* A born-cancelled follower could finish before its handle was registered (the spawn precedes the insert, the last exit then found nothing to deregister and the dead handle stayed registered until `stop_all`; the race predates the branch but the new path makes it likely): the start sweeps the handle after the insert when its lease is already invalid, and a registration hook between the spawn and the insert pins it with a test.
* The cancellation check sat before the identity fence and the apply gate, so a reply could still be applied after a stop that arrived while it waited there: the check is repeated right after the gate was taken, and a test holds the gate while the stop arrives.
* The late-start test counted yields before its seam was reached: it now spawns the reconcile, waits for the hook under a timeout, fails at once if the reconcile returns first, and asserts registry and lease absence.
* Wording: "no lock" and "at once" now say what is true (no lifecycle or registry lock; signal delivery versus task exit; the cap bounds the asynchronous join).
* Left as it is, with the reason: no regression isolates the server-side expiry and flush decision (the join expiry needs a follower that ignores its token, which the supervisor cannot be made to host without a new seam in the follower loop; the decision itself is one `quiesced = false` assignment read by the existing flush gate).

## Review result, round 3 (verdict FAIL against the round 2 fixes: two P2 and one P3, all addressed)

* The post-insert sweep treated an invalid lease alone as proof of exit, but DROP, PAUSE and terminal marking clear the lease while followers still run, so the sweep could detach live followers (and `stop_all` would then find nothing to join). The sweep now requires the matching lease, an invalid lease AND every target's done flag (all wrappers reached exit housekeeping); a negative-control test invalidates the lease inside the registration hook while the followers wait on a silent Hello and asserts the handle stays registered.
* The apply-gate test announced every pull result, including an empty batch or an error, and its 300 ms pause did not prove that a follower reached the gate: the source now announces only a non-empty batch (an empty one encodes as the one-byte msgpack array 0x90). Known limit, accepted: arrival at the gate itself is still covered by the pause, which can only weaken the check on a slow runner and never fail it (an acknowledgement of the pending gate read would need a new seam inside the loop).
* Wording: a born-cancelled follower exits at its FIRST CANCELLATION CHECKPOINT (it may first load its receipt or prepare the identity fence), and an expiry of the replication join means follower quiescence was not established (it can also come from a reconcile stalled on the catalogue with no follower at all).

## Non-goals

No change to `stop_all` semantics, to reconcile outside the early return, or to the scheduled-worker join (item 18) and the GC handle (item 19, its own branch).

## Acceptance

`fmt`, clippy `-D warnings` for `shamir-sync` and `shamir-server`, the layout gate and the visibility check exit 0; full-tier CI green on the head; the four new tests pass on both OS.
