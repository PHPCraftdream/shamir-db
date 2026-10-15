# Round 8 — subscription bridge quarantine delivery

## Constraints and ownership

- Scope: `shamir-server/src/subscriptions/{bridge.rs,push.rs,journal_drain.rs,journal_source.rs,db_journal_source.rs}` as needed, subscription tests/quarantine, and only necessary manifests. Other workers own DB quarantine/cache/facade receiver implementation and replication handlers/bench.
- No builds, tests, clippy, rustc, nextest, or Cargo commands. No git writes or commits. The orchestrator commits; edits must use file editing tools.
- Keep source/layout limits at or below 1000 lines and 7 direct folder entries; do not add a dependency/config or raw query construction.

## DB delivery-gate API coordination

The DB worker is implementing `shamir_db::shamir_db::{ChangelogReceiver, ChangelogDeliveryGate}`. `subscribe_changelog` returns `Result<Option<ChangelogReceiver>>`; receiver `recv`/`try_recv` preserve Tokio broadcast errors; `delivery_gate()` clones an Arc-backed gate; `closed().await` is cancel-safe and wakes on quarantine. `try_with_delivery` is synchronous and executes sink acceptance only while an active lease is valid; `None` means closed. Transition marks closed and waits active synchronous leases to drain before acknowledging. No await may occur inside a lease.

Bridge requirements: capture one gate per attached repo and retain it independently of JournalDrain's mutable receiver slice; recheck quarantine after every journal page read; protect each actual PushSink acceptance attempt, not an earlier check; select `closed()` against the backpressure retry delay so idle bridges terminate promptly. Apply the same gate after materialized snapshot reads and before snapshot acceptance. Closed notifications may be best-effort/ungated terminal control frames.

## Required repairs

- Fix imports for `ReplDirection`/`ReplMode`, owned row id cloning, and ensure the same registry subscription id is reserved and passed into `bridge_task`.
- Make live bridge test wait for `Ready` before its live write.
- Add deterministic in-flight page-read and push-backpressure quarantine tests: gate/hook pauses after page read or before acceptance, test flips quarantine, then resumes and requires `Closed` with no quarantined event. No sleeps as synchronization; use explicit oneshot/barrier handshakes, join every task, and assert observed state. Include snapshot protection if the existing deterministic snapshot hooks can support the real persisted quarantine transition.
- Tests must have a negative-control rationale: the current probe at event-delivery entry is before awaited work/retry sleeps and snapshot delivery currently bypasses it, so each test must fail on the old path.

## JournalDrain receiver seam

Inspect actual receiver APIs and callers first. Prefer a local receiver trait in `journal_source.rs` only if it can represent async `recv` with a `Send` future and synchronous `try_recv`, implemented for raw Tokio receivers and the new wrapper. This preserves existing raw test fakes without asking the DB worker for a public raw-receiver constructor. If wrapper error semantics prevent this, adapt tests locally and report the exact minimal DB-side test support required; do not invent an API silently.

## Verification/report

Source inspect actual callers and types. Tests are for CI only and must not be run here. Final report must identify exact changed files, required DB interface (including any test support), explain why lease linearization prevents check/send TOCTOU and note any remaining race boundary, and name the deterministic test oracles and old-code negative controls. Explicitly state no validation commands were run.