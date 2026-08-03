# Brief — P0-5b: sorted RENAME INDEX cannot be resumed after a partial failure

Task: #962 in the session TaskList. Source: `docs/dev-artifacts/research/2026-08-03-new-wave-readonly-review.md` §P0-5b, RE-VERIFIED against the CURRENT source (the code has evolved since the review was written — read this brief carefully, it narrows and corrects the review's framing based on what's actually in the tree now). Tasks #957-#961, #972, #973 are already fixed/committed — #973 renamed `legacy` → `base_index` (so `SortedIndexManager` now lives at `crates/shamir-index/src/base_index/sorted_index_manager.rs`, not `legacy/`). #959/#972 established a durable-tombstone + open-time-recovery pattern for DROP INDEX (regular/unique/sorted) — REUSE that exact pattern here, it fits this bug almost perfectly.

## What the review got right, and what's already been fixed since

The review said `rekey_sorted_prefix` "comments a `transact()` call as atomic" and doesn't check `supports_atomic_transact()`. Reading the CURRENT code (`crates/shamir-engine/src/table/table_manager_index_mgmt.rs::rekey_sorted_prefix`, ~line 1177) shows this is PARTIALLY already addressed by a prior fix (task #616, referenced in the code's own comment) and by `Store::transact`'s doc (F-85/#913, `crates/shamir-storage/src/types.rs` ~line 179-253): `rekey_sorted_prefix` is now a **loop that re-scans the old-id prefix until nothing is left**, applying each batch as one `transact()` call — this "settle re-scan" design is EXPLICITLY documented in `Store::transact`'s own doc comment as the accepted, reviewed way production callers tolerate `supports_atomic_transact() == false` (a non-atomic backend's partial-batch visibility is fine because the NEXT loop iteration picks up whatever wasn't yet moved). **Do not re-litigate this design** — F-85 already decided callers self-heal via settle/re-scan instead of gating on the capability flag, and this settle loop already implements that for the "a concurrent writer races the rename and lands an entry under old_id mid-sweep" case.

## The REAL remaining gap (confirmed by reading the caller)

`crates/shamir-engine/src/table/table_manager_index_mgmt.rs`'s `rename_index` (~line 1107-1120):

```rust
if is_sorted {
    // Swap the in-memory definition old_id → new_id FIRST.
    self.sorted_indexes.rename_definition(old_id, new_id).await?;
    // Then sweep remaining old-id postings to new_id (with settle).
    rekey_sorted_prefix(&*self.info_store, old_id, new_id).await?;
}
```

`rename_definition` swaps (and — verify by reading `SortedIndexManager::rename_definition`, `crates/shamir-index/src/base_index/sorted_index_manager.rs` — likely persists) the definition FIRST, THEN `rekey_sorted_prefix` runs. If `rekey_sorted_prefix` returns `Err` at ANY point (e.g. `info_store.transact(ops).await?` genuinely fails — a real backend error, not just non-atomic partial-visibility), the `?` propagates the error all the way out of `rename_index`, but the definition has ALREADY been renamed to `new_id`. The client sees an error. **There is no way to resume the interrupted rekey**: a client retry of the SAME rename command would fail immediately because the source (`old_id`'s old name) no longer resolves — the catalog already thinks the rename succeeded, while some physical postings may still sit under the old `SORTED_TAG || old_id` prefix, permanently orphaned (never queried again under either name, silent storage leak, and if `old_id` is ever reused by a future index, ghost-posting collision).

This is the real, still-open bug: not the per-batch atomicity (already handled), but the **lack of a durable, resumable "rename in progress" record** that survives a crash or hard error between `rename_definition` and `rekey_sorted_prefix`'s completion.

## Required fix — reuse #959/#972's tombstone pattern, adapted for rename

1. Before calling `rename_definition`, persist a durable **"Renaming" tombstone** recording `(old_id, new_id)` for the sorted family — mirror `SortedIndexManager`'s `dropping_sorted`/`add_to_dropping_sorted`/`clear_from_dropping_sorted`/`system:sidx_drop` pattern from #972 exactly (same file, same crate, added THIS session — read that diff first: `git log --oneline | grep 972` then look at the sorted manager's drop-tombstone methods). Use a DIFFERENT key name (e.g. `system:sidx_ren` — **run the same 12-byte-truncation collision check #959/#972 established**: verify `"sidx_ren"`'s first 12 bytes don't collide with `"sorted_indexes"`→`"sorted_index"` or `"sidx_drop"`).
2. `rename_definition` proceeds as today (swap + persist).
3. `rekey_sorted_prefix` proceeds as today (settle loop).
4. On success, clear the "Renaming" tombstone (persist-first ordering, mirroring `clear_from_dropping_sorted`).
5. **Open-time recovery**: `SortedIndexManager::new()` (already calls `recover_in_progress_drops()` per #972 — add a sibling `recover_in_progress_renames()` call right after it) loads the "Renaming" tombstone; if present, RESUME `rekey_sorted_prefix`'s settle loop for that `(old_id, new_id)` pair (the loop is already idempotent — calling it again just finds nothing left to move if the rekey had actually finished, or resumes moving the remainder if it hadn't). Clear the tombstone after the resumed rekey completes.
6. Decide whether `rekey_sorted_prefix` itself needs to move from `table_manager_index_mgmt.rs` (engine crate) into `SortedIndexManager` (index crate) to be callable from the recovery path, OR whether the recovery path can stay in the engine crate and just needs its own open-time hook (check how `TableManager`/table open wiring calls into `SortedIndexManager::new` today, and whether `info_store` is available at that point — this determines where the recovery call naturally belongs; don't force it into the wrong crate just to reuse #972's exact structure if the dependency direction doesn't allow it).

## Required tests

Mirror #972's `p03b_sorted_drop_durability_tests.rs` shape (same test file directory) for rename:

- **Crash-resurrection-equivalent for rename**: seed a "Renaming" tombstone + a definition already swapped to `new_id` + postings STILL under `old_id`'s prefix (simulating a crash right after `rename_definition`, before `rekey_sorted_prefix` ran) — construct a fresh manager, assert the recovery path finishes the rekey (postings now under `new_id`'s prefix, tombstone cleared).
- **Idempotent resume**: run recovery twice — second run is a clean no-op.
- **Normal rename still works** (no tombstone left behind on a successful rename) — regression test.

## Scope discipline

- Do NOT touch index2 rename (#961, already done) or DROP INDEX (#959/#972, already done) except to READ their patterns as reference.
- Do NOT re-architect `Store::transact`/`supports_atomic_transact` (that's P1-3/#968, a separate, lower-priority task about API naming/splitting — not blocking this fix).
- Run ONLY the centralized test entry point: `./scripts/test.sh -p shamir-index -p shamir-engine`. Raw `cargo test` is blocked.
- `cargo fmt` and `cargo clippy --workspace --all-targets -- -D warnings` must be clean before you declare done.

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Do NOT run `git commit` or `git add` — the orchestrator verifies your diff and the test run, then commits. Only edit files and run read-only/build/test commands. Delete stray log files you create yourself; mention it if you leave any.

## What to report back

Confirm the exact tombstone key name chosen and its collision check, where the recovery hook ended up living (engine crate vs index crate) and why, what each test proves, and the exact `cargo fmt`/`cargo clippy`/`./scripts/test.sh` commands with real pass/fail counts and exit codes.
