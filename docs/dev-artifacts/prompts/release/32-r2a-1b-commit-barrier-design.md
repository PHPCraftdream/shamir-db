# Brief: R2a step 1b — review and settle the commit barrier for the online backup facade (design only)

Design: `docs/dev-artifacts/design/online-backup.md`, especially sections 4.2 (protocol steps A, B, E, G), 4.3, 4.4 and the
new "Step 1 findings (2026-11-17)" subsection of the Review section. The WAL truncation pin has landed
(`crates/engine/shamir-engine/src/tx/drain/truncation_pin*.rs`). The copy engine in `shamir-sync` has landed
(`crates/server/shamir-sync/src/sync/durability/backup/online/`, port trait `OnlineBackupPort` in `wal_capture.rs`).

## Why

The capture/seal/replay facade cannot be built safely until two hazards are settled: capture/seal draining while holding
repository admission can self-deadlock, and interner-only leader commits pass every table freeze. A tx-layer concurrency
change needs a reviewed design before code.

## What

Revise the "Step 1 findings" subsection into a settled design (still in `online-backup.md`, OBSERVED vs PROPOSED, path:line
for every code claim — re-verify each cited line against the current code, since the engine was split into
`shamir-engine-repo` / `shamir-engine-table` crates):

1. For each hazard: the exact call chain with current paths, a minimal reproduction scenario in words, and why the pin
   alone does not help.
2. The commit admission barrier: the primitive (lock-free preferred; if a `tokio` primitive, the contention model), where
   it is entered and left on every commit path (table commits, interner-only commits, DDL, replicated apply on a
   follower), how it interacts with the drainer and the truncation pin, and how a cancelled or crashed capture/seal
   releases it (fail-closed recovery).
3. The facade API (`capture_point`, `seal_point`, `replay_to`) in the `db_instance` layer, mapped one-to-one onto the
   `OnlineBackupPort` methods, and which layer owns each piece (no new backward edges in `.rust-guard/layers.toml`).
4. The CI tests that prove: no self-deadlock under concurrent commits, an interner-only commit cannot slip past the
   seal, cancellation at each step releases the barrier and the pin, replay to `V*` reproduces exactly the sealed state.
5. An implementation plan of at most two PR-sized steps, and whether each must land before the tx layer moves into its
   own crate.

## Do not

Change any code, test or CI file. Add no dependency (if an option needs one, write it as an owner decision with the
alternative that needs none).

## Policy (verbatim, binding)

Compilation, builds and tests run on GitHub CI, not on this machine. Banned locally: `cargo test`, `cargo nextest`,
`./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, npm/tsc builds, any build or test run.
Allowed locally only: `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`,
`cargo fmt -p <crate>` (and `-- --check`), `python -I scripts/ci/check_layout.py`. This task needs none of them.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working
tree or index. Only edit files; the orchestrator commits.

## Observable acceptance

Only `docs/dev-artifacts/design/online-backup.md` changes; every code claim carries a current path and line; the open
questions have defaults. The final report lists the decisions taken and the open questions.
