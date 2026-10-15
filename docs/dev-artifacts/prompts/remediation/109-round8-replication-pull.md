# Round 8 — isolated scope worker brief

Scope: `crates/shamir-server/src/db_handler/repl_handler.rs`, `crates/shamir-server/src/replication/in_process.rs`, topical replication regression tests/manifests if needed, and `crates/shamir-db/benches/engine_perf.rs` only. Do not edit shamir-db implementation or bridge/push/subscription tests; another worker owns quarantine cache and receiver facade.

Fix long-poll bypass: both wire and in-process currently retain raw RepoInstance and reread `read_changelog_for_replication` after sleeps. Route every page/retry through `ShamirDb::read_changelog_for_replication`, preserve `downstream_resync_required`, missing-repo/error semantics, limits and metadata. Recheck gate after each async page read and before returning output, including after metadata awaits. Acknowledgment can stop an in-flight result only at an explicit gate observation; do not claim stronger atomic exclusion absent synchronization.

Regression coverage should deterministically pause a real long-poll request via an actual handshake/hook, flip quarantine, resume, and assert wire refusal for the pre-flip pending request; cover in-process too. No sleep-based synchronization. Tests must fail against raw-repo reread implementation. Inspect signatures/callers/Send guards before edits.

Benchmark layout: `engine_perf.rs` currently 1367 lines vs origin/master 1365. Retain existing subscribe failure `expect`, not `.ok()`, and compensate exactly two lines in this file such that rustfmt does not undo the reduction. No dependency/version/config changes.

Constraints: no local builds/tests/clippy; no cargo invocation; no git writes/commit. Edit files only. Orchestrator performs formatting. Keep layout ratchet: existing >1000-line files net <= master; <=7 new folder entries; do not grow an existing over-limit folder. Query builder only, imports header. Report files, line locations, tests and exact mutation each catches.
