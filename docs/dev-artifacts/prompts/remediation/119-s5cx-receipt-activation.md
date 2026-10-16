# 119 — S5c-X PR6b-2c: receipt apply activation on the merged foundations

## Context

PR6b-2 (draft PR #180, branch `feat/s5cx-follower-receipt-2`) implemented
receipt-based follower apply/resume but failed five review rounds. A design
review split it into three merges:

1. **Repository write-admission domain** — merged (#183): `RepoWriteDomain`
   and the `RepoWritePermit` capability (`crates/shamir-engine/src/repo/changelog_store/write_domain.rs`,
   `write_domain_membership.rs`, `repo_membership_impl.rs`); every table
   membership, DDL, migration, recovery and catalogue mutation enters the
   domain; lock order is permit → marker → per-table DDL admission →
   barrier → `bindings_lock` / `persist_lock`; writable `TableManager`
   getters are crate-private, other crates use read-only views or the
   admitted APIs.
2. **Structured receipt worker lifecycle** — merged (#182), inactive:
   `crates/shamir-engine/src/tx/cold_reconcile/lifecycle/` (owner holds the
   sender and join handle; execution state holds the receiver and the
   lease; jobs carry payload and reply only; states Open → Closing →
   Joined; unconditional quarantine on timeout or panic).
3. **This PR: activation.** Materialization, absolute counters, journal
   writer coordination, publication, paused reservations and cold recovery
   activate **together**, on top of (1) and (2).

The binding design is `docs/dev-artifacts/prompts/remediation/106-s5cx-follower-receipt-2.md`.
Invariants: I1 an event is derived from its WAL entry; I2 the WAL is
retained until the journal is checkpointed; I3 followers are served only
recoverable versions. Receipts are durable redo records with phases
Prepared → Materialized → Checkpointed, written in one atomic repository
batch; publication happens only after durability.

## Task

Implement the activation unit from origin/master. The old branch is a
SOURCE OF PARTS, not a base: read it with `git show
feat/s5cx-follower-receipt-2:<path>` and `git diff
origin/master...feat/s5cx-follower-receipt-2` (read-only git only). Reuse
its receipt staging, apply, checkpoint, lookup, counter and test code where
it is correct, but:

- replace its own receipt worker with the merged lifecycle foundation (2);
  the worker must not hold its own sender, must terminate on repository
  close/drop through the owner, and shutdown must reach publication or a
  safe durable pause, reporting failures to the caller (never log-and-go);
- replace its wrapper-based admission with `RepoWritePermit` (1): the
  receipt pipeline takes the repository permit and passes it to admitted
  APIs; no second admission mechanism.

## Outstanding review findings that must be resolved (from the last review of #180)

- Materialized resume must not repeat an already-durable phase: both the
  inline retry and the ordinary outstanding-receipt path go straight to
  checkpoint for a stored Materialized receipt; identity checking compares
  event bytes, bindings, lineage and epoch, not only ranges.
- The Materialized-pause test must actually hit its injected failure and
  assert the paused state and replay; the counter oracle must
  discriminate (a recomputation must not produce the same value by
  coincidence).
- Old-history regression coverage must exercise the cold read branch and a
  real `AS OF` read.
- Crash-boundary proof: the parent harness must require the child's
  phase marker, so an unrelated child failure cannot satisfy the death
  check; the harness reopens twice and replays twice, covering exact
  allocation, history and timestamps, counters, downstream bytes and
  snapshots.
- Cancellation tests must not depend on a fixed yield budget.
- The activation must not write a receipt format that the same merged
  version cannot recover.

## Rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
  `rm`, or any git command that mutates the working tree or index. Only
  edit files; the orchestrator commits.
- No local builds, tests or clippy; `cargo fmt -p <crate>` for touched
  crates is the only allowed command besides read-only git. CI is the gate
  (tests run only through `./scripts/test.sh` in CI).
- Layout ratchet: at most 7 entries per folder, at most 1000 lines per
  file; files and folders already over a limit must not grow.
- Imports at the top of the file; `mod.rs` files only re-export; tests in
  `tests/` folders, one topic per file; queries only through the builder.
- No machine-specific absolute paths in any file you write.
- Futures on deep inline chains (nested Batch, ALTER SUBSCRIPTION RESUME,
  seed install) must not grow: heap-pin new heavy awaits there (two stack
  overflows came from exactly this in #181 and #183).
- Before finishing, run one verifier child agent over EVERY file changed
  in the whole PR (`git diff --name-only origin/master` plus untracked),
  checking compile-level consistency (signatures, visibility, `Send`
  bounds, every caller in benches/examples/tests of every crate).

## Final message

Per area: what was reused from the old branch, what was rebuilt on the
foundations, file:line for each review finding above, and measured line
counts for every over-limit file touched vs origin/master.
