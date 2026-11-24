# Brief: STORE-RELEASE-BARRIER (items 12 and 13 of SHUTDOWN-ORACLES, #2008), done by hand

Both defects were found by the landing review of BACKUP-TAILS-SERVER and deliberately left out of that landing; they are in `crates/server/shamir-server/src/engine/server/store_release_barrier.rs`.

## Defects

1. **A discovered lock marker that vanished counts as unlocked.** `open_lock` mapped a `NotFound` open of a marker that discovery had classified (and that is now absent per `symlink_metadata`) to `LockState::Unlocked`. On Unix an unlinked lock file can still be held through an open descriptor, so absence proves nothing: the barrier could certify a release for a store that is still locked.
2. **A probe that completes after its absolute deadline can certify release.** `tokio::time::timeout_at` polls the wrapped receiver before its timer, and the blocking job sent the bare result. A job that finishes after the deadline but before the async side polls again won the race, and a late empty held set returned `Released` (in `ServerHandle` this can happen during the 250 ms report reserve, and `Released` appends no error).

## Change

* `open_lock`: the vanished marker is `LockState::DiscoveryGap` (UNKNOWN, reported as `store discovery was incomplete`), the same class as a dangling marker. The `Unlocked` doc no longer mentions a removed lock file.
* Report text (`server_handle.rs`): the discovery-gap detail names both causes, an incomplete walk and a discovered marker that could no longer be verified.
* `bounded_probe`: the blocking job stamps its own completion on the timer's clock (`tokio::time::Instant`, runtime-entered inside the blocking job) right after the probe returns, against the absolute deadline; a result completed at or after the deadline is `PoolStarved`, so the first look is `Unverified(PoolStarved)` and a late RE-probe keeps the last in-budget held observation (`StillLocked`).
* Tests: the seam test now asserts that a vanished marker is a gap (it asserted the opposite on purpose before). Two deterministic tests of the completion stamp run on a current-thread runtime with a PAUSED clock and ONE blocking worker (no real-time sleeps, no race with the scheduler): `a_probe_finishing_after_the_deadline_is_not_an_observation` holds the job on a channel, advances the virtual clock past the deadline, releases the job and drains the worker, so the result and the expired timer are both ready at the next poll (a bare-value mutant returns `Ok`); `a_probe_finishing_before_the_deadline_is_an_observation_even_when_polled_late` lets the job finish in budget, advances the clock past the deadline and only then polls (a stamp taken on the receiving side returns `PoolStarved`). The runtime thread waits on a sentinel job queued behind the probe, so the timer cannot fire in between. New test seam `bounded_probe_for_test` (cfg(test)).

## Review result (round 1 FAIL, round 2 FAIL on the tests only, all fixed)

* Round 1: the late-probe test could pass vacuously, the std/tokio clocks of the stamp and of the timer disagreed, one wording was off: fixed by the sentinel, the tokio clock and the text.
* Round 2 (production code verified correct): the test did not catch a stamp moved to the async side and its first-poll assertion depended on real-time scheduling; the wording of the gap detail and of this brief was stale. Fixed by the two paused-clock tests above and the two texts.

## Not in this branch (stay in SHUTDOWN-ORACLES)

Item 11 (a database named like a system store, e.g. `users`, nests its repository `data_dir/users/main` INSIDE the system store directory, which the marker walk never visits): the fix needs either reserved database names (a product decision) or a walk over authoritative store paths, and descending into fjall store directories would hit the depth cap on every healthy shutdown.

## Acceptance

`fmt`, clippy `-D warnings` for `shamir-server`, the layout gate and the visibility check exit 0; full-tier CI green on the head; the three new or changed tests pass on both OS (the seam test is Unix-only).
