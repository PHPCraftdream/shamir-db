# Brief: wake the replication long poll on a journal event (LONGPOLL-NOTIFY, design only)

## Why

`crates/server/shamir-server/src/engine/db_handler/handlers/repl_handler.rs` (around lines 405-440): when the journal
tail is empty, the `Pull.wait_ms` handler loops on `sleep(min(remaining, POLL_STEP = 50 ms))` and re-reads the journal.
With N followers that is N x 20 reads per second on an empty tail and up to 50 ms of extra delivery latency. The header
of the file deliberately calls this "not a subscription": the leader keeps no per-follower state, never blocks writers
(REPLICATION section 5.6). Any replacement must keep that property.

## What (a design document, NO production code)

Write a short design section, `docs/dev-artifacts/design/longpoll-notify.md`, that:

1. Describes the current loop precisely (file and line), the guarantees the file header states, and the deadline rule.
2. Surveys the signals that already exist: the live changefeed broadcast of `RepoInstance` (and where it fires relative
   to publication), the bounded confirmed-read position, the journal append notification if any, the drainer wake.
   For each: when it fires, what it carries, whether a late subscriber misses it, its cost for N followers.
3. Chooses the MINIMAL mechanism: a shared wake signal (for example `tokio::sync::Notify` or a `watch` of the confirmed
   version) that is NOT per follower, with `select!(notified, deadline)`. Prove the lost-wakeup race closed: the waiter
   must subscribe BEFORE its final re-read of the tail, so an event between the read and the subscription is seen.
4. States and justifies the invariants: no per-follower state on the leader, no lock a writer can block on, the
   deadline still bounds every wait, no change of the wire protocol, behaviour for a follower that connects while the
   leader is idle, and for a leader restart.
5. Lists the tests the implementation must carry (race between read and subscription, deadline expiry with no event,
   many waiters woken by one event, no per-follower registry), and the failure-only diagnostics the repo requires for a
   test that could flake (`shamir-test-diag` `FlakeGuard`).
6. Gives the implementation plan in steps with files and risk, and what review it needs.

Cite file and line for every claim about existing code. Do not change any source file. Do not touch
`.rust-guard/stages.toml`.

## Observable acceptance

The design document exists and is self-contained; `cargo` is not needed (docs only); layout is OK. Final report: the
chosen mechanism in five lines, the open questions for the owner, and the review you recommend.
