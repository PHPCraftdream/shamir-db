# Brief: REPL-STOP-FENCE (item 17 of SHUTDOWN-ORACLES, #2008), done by hand

Found by the review of the flake branch (FLAKE #2009) and by the landing review of BACKUP-TAILS-SERVER (item 3 there).

## Defect

`ServerHandle` shutdown stops the follower-replication supervisor with `SubscriptionSupervisor::stop_all`, which takes the reconcile lock BEFORE it signals any follower. A reconcile pass in flight holds that lock across its catalogue read (a blocking-pool read), so under a stalled pool `stop_all` waits for the lock. On the deadline path the stage cap abandons it, and nothing has been signalled: every follower keeps running (each has its own cancellation token; the root shutdown token only stops the reconcile ticker), the repository flush still runs (`quiesced` stays true), and `shutdown_clean` requires commit and apply admission to be stopped. On the public path (no outer deadline) the same wait is unbounded: it was left unbounded in the flake fix precisely because bounding it without this fence would abandon the stop before any signal.

## Change

* `SubscriptionSupervisor::begin_stop` (new): latches `stopping` and cancels EVERY registered follower with no lock and no await (the followers exit gracefully at their next await; `stop_all` still joins them).
* `reconcile` returns at once once `stopping` is set; `start_subscription` re-checks the flag after it registers a handle and cancels it at once (SeqCst fences on both sides so that either the `begin_stop` walk sees the new handle or the start path sees the flag). `stop_all` itself is unchanged (many tests call it mid-test and keep using the supervisor).
* `ServerHandle` shutdown: `begin_stop()` first, then the join is bounded by `CONN_DRAIN_TIMEOUT` with OR without an outer deadline; an expiry reports `replication supervisor shutdown exceeded` and sets `quiesced = false`, so the repository flush is skipped (the followers were already signalled and exit on their own). The flush-skipped message now names replication. The `shutdown_with_report` rustdoc drops the replication exception.
* Test `begin_stop_cancels_followers_without_the_lock_and_fences_later_starts`: a running two-target follower exits after `begin_stop` alone (no `stop_all`), and a later `reconcile` does not restart the still-active row.

## Not covered by a test (said, not hidden)

The register-then-recheck window inside `start_subscription` (a follower registered after the `begin_stop` walk) is closed by the fences and argued in the comments; there is no seam between the spawn and the registry insert to force it. A server-level test with a parked reconcile and an active follower is not added: the supervisor-level test covers the mechanism and the server change is mechanical.

## Non-goals

No change to `stop_all` semantics, to reconcile outside the early return, or to the scheduled-worker join (item 18) and the GC handle (item 19).

## Acceptance

`fmt`, clippy `-D warnings` for `shamir-sync` and `shamir-server`, the layout gate and the visibility check exit 0; full-tier CI green on the head; the new test passes on both OS.
