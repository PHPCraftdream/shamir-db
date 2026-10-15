You are a Rust fix worker in worktree D:/dev/rust/shamir-db/worktrees/s5cx-follower-receipt-2 (branch feat/s5cx-follower-receipt-2, clean tree @ 0721b609).

⛔ NEVER run git reset / checkout / clean / stash / restore / rm, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
Do NOT run cargo/rustc/nextest or any build or test command. Only the orchestrator runs `cargo fmt` at the very end.
Read and follow the hard rules in C:/Users/Computer/.agents/skills/rust-intel/SKILL.md if readable.
FIRST: save this brief verbatim to docs/dev-artifacts/prompts/replication/107-s5cx-receipt-3-compile-blockers.md (create the file).

TASK: fix six static compile blockers from the round-2 review. Fix ONLY these; surgical edits, no refactors, imports at file top, rustfmt-clean (4 spaces, trailing commas).

1. `crates/shamir-engine/src/tx/follower_durability/receipt/stage.rs` around lines 670-684, fn `table_rows`: `table: String` is moved into the pushed tuple and then referenced by two `format!("...{table}")` expressions in the same tuple literal — use-after-move. Fix minimally, e.g. push `table.clone()` as the first element (verify the tuple type `Vec<(String, String, String, u64)>` stays satisfied).

2. `crates/shamir-engine/src/tx/follower_durability/receipt/tests/crash_subprocess_tests.rs` fn `Scenario::expected_history` (~lines 179-192): it declares return type `Vec<(Vec<u8>, u64, Bytes)>` but pushes `(change.key.clone(), ...)` where `RecordChange.key` is `bytes::Bytes` (see `crates/shamir-tx/src/changefeed.rs` ~line 82). Fix with `change.key.to_vec()` so the element type matches. Verify the consumer `history_rows` (returns `Vec<(Vec<u8>, u64, Bytes)>` building `key.to_vec()`) and the `assert_eq!` at ~line 217 compare equal types.

3. `crates/shamir-engine/src/tx/cold_reconcile/tests/reconcile_tests.rs` line 6 has `use super::reconcile_with_hook;` but `crates/shamir-engine/src/tx/cold_reconcile/tests/mod.rs` no longer re-exports it (the round-2 commit removed `pub(super) use crate::tx::cold_reconcile::algorithm::reconcile_with_hook;`). Restore that re-export line at the top of `cold_reconcile/tests/mod.rs`. First grep `crates/shamir-engine/src/tx/cold_reconcile/algorithm.rs` for the exact visibility of `reconcile_with_hook` (expected `pub(super)`, visible throughout `cold_reconcile`) and confirm the tests module can see it through the re-export path used by other lines in that file.

4. `crates/shamir-engine/src/meta/repl/tests/bookmark_v3_tests.rs`: the test `legacy_v2_loaders_read_v3_records_and_saves_refuse_downgrade` (~line 154-193) calls `load_fence_record` (~line 174) but nothing imports it. The file header line 2 imports from `crate::meta::repl_bookmark`: add `load_fence_record` to that existing header import list (do NOT add a mid-function import unless the header would create a conflict; prefer the header).

5. `crates/shamir-engine/src/tx/follower_durability/receipt/tests/ddl_interlock_tests.rs` line 53: `tables.iter().map(|t| TableConfig::new(t))` passes `&&str` where `TableConfig::new` takes `impl Into<String>` (see `crates/shamir-engine/src/table/table_config.rs` line 8 — `&&str` does not implement `Into<String>`). Fix to `TableConfig::new(*t)`.

6. Same file, ~lines 106-111: `Box::pin(apply_replicated_batch(&repo, &[event(1, 1)], 0, ApplyPolicy::Trusted))` — the temporary array `[event(1, 1)]` is borrowed by the pinned future but dropped at the end of the `let` statement (E0716 when polled later). Bind it first: `let events = [event(1, 1)];` (or `vec![...]`) and pass `&events`. Check the rest of the file for the same pattern (only fix sites where the future is stored across statements; a directly-`.await`ed temporary is fine).

7. `crates/shamir-engine/src/tx/cold_reconcile/tests/receipt_recovery_tests.rs` line 11: `use futures::StreamExt;` is unused in this file (verify with grep that no `iter_stream`/`.next().await` stream use exists) — remove the import.

FINALLY: re-read every edited file section against the real definitions (grep the actual types/signatures: `RecordChange.key`, `TableConfig::new`, `reconcile_with_hook` visibility, `load_fence_record` export path in `crates/shamir-engine/src/meta/repl_bookmark.rs` or its module tree) and confirm each fix compiles as the compiler would see it (no builds allowed — pure re-reading). Report: each fix with file:line before/after, and anything you could not verify.
