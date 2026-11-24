# Brief: REPL-STOP-FENCE (item 17 of SHUTDOWN-ORACLES, #2008), done by hand

Found by the review of the flake branch (FLAKE #2009) and by the landing review of BACKUP-TAILS-SERVER (item 3 there).

## Defect

`ServerHandle` shutdown stops the follower-replication supervisor with `SubscriptionSupervisor::stop_all`, which takes the reconcile lock BEFORE it signals any follower. A reconcile pass in flight holds that lock across its catalogue read (a blocking-pool read), so under a stalled pool `stop_all` waits for the lock. On the deadline path the stage cap abandons it, and nothing has been signalled: every follower keeps running (each has its own cancellation token; the root shutdown token only stops the reconcile ticker), the repository flush still runs (`quiesced` stays true), and `shutdown_clean` requires commit and apply admission to be stopped. On the public path (no outer deadline) the same wait is unbounded: it was left unbounded in the flake fix precisely because bounding it without this fix would abandon the stop before any signal.

## Change

* `SubscriptionSupervisor` owns a stop token. Every follower's cancellation token is a CHILD of it (`start_subscription`), and `begin_stop` (new) cancels the parent: one synchronous cancel reaches every running follower with no lifecycle lock, no registry walk and no await, and a follower started afterwards is born cancelled. `reconcile` returns at once once the token is cancelled. `stop_all` is unchanged (it cancels the children one by one and never the parent; many tests call it mid-test and keep using the supervisor).
* The follower loop's Hello and Pull are cancellation-aware (`tokio::select!` on the token), and a cancellation seen after the pull reply admits no apply (an apply already admitted always finishes). Without this a stop waited for a silent upstream up to the production request timeout (35 s), which the new stage cap would turn into an unclean report and a skipped flush. Dropping a pull before anything is applied is safe: the next start pulls again from the durable bookmark.
* `ServerHandle` shutdown: `begin_stop()` first, then the join is bounded by `CONN_DRAIN_TIMEOUT` with OR without an outer deadline; an expiry reports `replication supervisor shutdown exceeded` and sets `quiesced = false`, so the repository flush is skipped (the followers were already signalled and exit on their own). The flush-skipped message names replication. The `shutdown_with_report` rustdoc drops the replication exception.
* Tests (`multi_target_exit_tests.rs`): a running two-target follower exits after `begin_stop` alone, and a later `reconcile` does not restart the still-active row (asserted active); a follower whose start is parked before its spawn (start race hook, under the lifecycle lock) while `begin_stop` runs ends up with no running follower and no `stop_all`; a follower that waits on a silent Hello or Pull exits on `begin_stop`.

## Review result (independent review, verdict FAIL, all three P2 fixed)

* The first version cancelled by walking the registry with a fence pairing and a post-insert recheck: the review showed that the synchronous `scc` walk can itself block on a bucket lock before the stage timer starts, that the late-registration race was untested although a seam existed (the start race hook), and that a 10 s cap turns a follower that only waits for the network into an unclean shutdown with a skipped flush. The parent/child token design removes the walk and the fences; the late-registration test uses the existing hook; Hello and Pull are cancellation-aware.
* Noted and left as it is: the rustdoc of `shutdown_with_report` is not an inventory of every unbounded wait (the public path still awaits the two reaper joins directly; their ticks are in-memory), and a reconcile pass already past its stop check can still spawn a follower, which is born cancelled.

## Non-goals

No change to `stop_all` semantics, to reconcile outside the early return, or to the scheduled-worker join (item 18) and the GC handle (item 19, its own branch).

## Acceptance

`fmt`, clippy `-D warnings` for `shamir-sync` and `shamir-server`, the layout gate and the visibility check exit 0; full-tier CI green on the head; the four new tests pass on both OS.
