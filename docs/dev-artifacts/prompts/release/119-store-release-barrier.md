# Brief: STORE-RELEASE-BARRIER (items 12 and 13 of SHUTDOWN-ORACLES, #2008), done by hand

Both defects were found by the landing review of BACKUP-TAILS-SERVER and deliberately left out of that landing; they are in `crates/server/shamir-server/src/engine/server/store_release_barrier.rs`.

## Defects

1. **A discovered lock marker that vanished counts as unlocked.** `open_lock` mapped a `NotFound` open of a marker that discovery had classified (and that is now absent per `symlink_metadata`) to `LockState::Unlocked`. On Unix an unlinked lock file can still be held through an open descriptor, so absence proves nothing: the barrier could certify a release for a store that is still locked.
2. **A probe that completes after its absolute deadline can certify release.** `tokio::time::timeout_at` polls the wrapped receiver before its timer, and the blocking job sent the bare result. A job that finishes after the deadline but before the async side polls again won the race, and a late empty held set returned `Released` (in `ServerHandle` this can happen during the 250 ms report reserve, and `Released` appends no error).

## Change

* `open_lock`: the vanished marker is `LockState::DiscoveryGap` (UNKNOWN, reported as `store discovery was incomplete`), the same class as a dangling marker. The `Unlocked` doc no longer mentions a removed lock file.
* `bounded_probe`: the blocking job stamps its own completion against the std form of the absolute deadline; a result completed at or after the deadline is `PoolStarved`, so the first look is `Unverified(PoolStarved)` and a late RE-probe keeps the last in-budget held observation (`StillLocked`).
* Tests: the seam test now asserts that a vanished marker is a gap (it asserted the opposite on purpose before); `a_probe_finishing_after_the_deadline_is_not_an_observation` blocks the runtime thread past the deadline so that both the late result and the expired timer are ready at the next poll (a bare-value mutant returns `Ok`). New test seam `bounded_probe_for_test` (cfg(test)).

## Not in this branch (stay in SHUTDOWN-ORACLES)

Item 11 (a database named like a system store, e.g. `users`, nests its repository `data_dir/users/main` INSIDE the system store directory, which the marker walk never visits): the fix needs either reserved database names (a product decision) or a walk over authoritative store paths, and descending into fjall store directories would hit the depth cap on every healthy shutdown.

## Acceptance

`fmt`, clippy `-D warnings` for `shamir-server`, the layout gate and the visibility check exit 0; full-tier CI green on the head; the two new or changed tests pass on both OS (the seam test is Unix-only).
