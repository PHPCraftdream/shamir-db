# Brief — P0-3b: extend DROP INDEX crash-recovery tombstone to sorted + index2 families

Task: #972 in the session TaskList. Follows #959 (P0-3), which already fixed the legacy (regular + unique) `IndexManager` family in `crates/shamir-index/src/legacy/index_manager.rs`/`index_manager_unique.rs` — read that commit first (`git log --oneline | grep 959` or search the diff for "P0-3 (#959)") to reuse its exact pattern, naming conventions, and lessons learned rather than reinventing anything.

**Priority: sorted index family FIRST (confirmed real gap, no prior deferral), index2 (fts/functional/vector) SECOND (a pre-existing, deliberately-deferred gap per its own doc comment — see below). If time runs out, sorted alone with index2 explicitly left as a documented, cross-referenced deferral is an acceptable stopping point — do not rush a shaky index2 fix.**

## Confirmed current behavior

### Sorted index family — confirmed real gap, same shape as #959's legacy bug

`crates/shamir-index/src/legacy/sorted_index_manager.rs::drop_index` (~line 462):

```rust
pub async fn drop_index(&self, name_interned: u64) -> DbResult<bool> {
    // ... rcu-remove from self.indexes (retire) ...
    self.generation.fetch_add(1, Ordering::AcqRel);
    // ... sweep entry_prefix via scan_prefix_stream + remove_many ...
    self.persist_defs().await?;
    Ok(true)
}
```

Same ordering as legacy's OLD (pre-#959) code: retire → sweep → persist. A crash between sweep and `persist_defs()` leaves the on-disk sorted-index definitions listing the index as present while its entries are gone — same silent-wrong-results resurrection bug #959 fixed for legacy.

### Index2 family — a PRE-EXISTING, EXPLICITLY DEFERRED gap (not new, not silent)

`crates/shamir-engine/src/table/table_manager_index_mgmt.rs::drop_index2`'s own doc comment (~line 713) ALREADY states, in the codebase's own words: *"crash-safety [...] #873, deliberately out of scope here"* — i.e. this gap was known and intentionally deferred to a specific future task (F-50 Step 3b / #873) BEFORE this review, not discovered by it. Read that doc comment in full before touching this path. Decide, and clearly justify in your report, one of:

- (a) Apply the SAME tombstone pattern from #959/sorted to index2 now, closing #873 as a side effect (preferred if it fits cleanly without fighting the existing deferral's stated reasoning) — search git history/comments for "#873" to see if there's a more specific plan you'd be overriding; if the existing deferral note describes a DIFFERENT, more complete design than the simple tombstone, don't silently discard that design — either implement IT instead, or explain in your report why the simpler tombstone is preferable for this release.
- (b) Leave it deferred, but upgrade the doc comment to explicitly cross-reference this task's sorted-family fix (so a future reader sees the pattern to copy) and confirm in your report that this decision was deliberate, not a time-out.

## Required fix (sorted family, mandatory)

Mirror #959's exact approach for the legacy family:

1. A durable tombstone (`Vec<u64>` of in-progress-drop names, bincode-serialized) persisted under a `system:*` key BEFORE the sweep. **Critical, hard-won lesson from #959**: `RecordId::system(name)` truncates `name` to 12 bytes (see `crates/shamir-types/src/types/record_id.rs::system`) — before picking a key name, compute what EVERY existing sorted-index system key already truncates to (grep `RecordId::system(` in `sorted_index_manager.rs` for the existing persistence key(s), e.g. whatever `persist_defs` writes under) and verify your new tombstone key name's first-12-bytes does NOT collide. #959 discovered a real collision this way (`"indexes_unique_dropping"` and `"indexes_unique"` both truncate to `"indexes_uniq"`) — do the same verification here, don't assume it's fine.
2. Extract the sweep into its own idempotent method (mirror `sweep_index_postings` from #959) so recovery can re-run it safely.
3. Persist the tombstone → retire def (existing) → sweep (idempotent method) → persist reduced defs → clear tombstone (persist-first ordering, mirroring `clear_from_dropping`'s doc in #959's diff).
4. Open-time recovery: find wherever `SortedIndexManager` loads its persisted state on construction (mirror `IndexManager::new`'s pattern from #959 — search for the sorted manager's equivalent constructor) and add a `recover_in_progress_drops`-equivalent call there, following the exact crash-state matrix #959 documented (crash-before-sweep / crash-after-sweep-before-persist / crash-after-persist-before-clear).
5. Namespace-reuse guard: reject `create_sorted_index*` for a name currently in the tombstone set (mirror #959's guard in `create_index`/`create_index_from_records`/`create_index_from_stream`).

## Required tests

Mirror #959's `p03_drop_durability_tests.rs` test shape (same crate, `crates/shamir-index/src/legacy/tests/` — check whether a new file or extending that one is more appropriate given the existing `tests/mod.rs` wiring) for the sorted family: crash-before-sweep, crash-after-sweep-before-persist, crash-after-persist-before-clear, idempotent double-recovery, namespace-reuse rejection, and a "live drop paused at post-sweep hook, manager dropped (simulated crash), fresh construction recovers" test using the SAME `BackfillPauseHook`-style seam #959 introduced (check whether `SortedIndexManager` already has an analogous pause hook from F-76, or whether you need to add one mirroring `drop_index_post_sweep_hook`).

If you also fix index2 (option (a) above), add the equivalent matrix for it too, in whatever test module index2 already uses.

## Scope discipline

- Do NOT touch legacy regular/unique (`index_manager.rs`/`index_manager_unique.rs` — #959, already done) except to READ it as a reference.
- Do NOT touch RENAME INDEX (index2's rename is #961, sorted's is #962 — separate tasks, not yet started, will build on whatever tombstone infrastructure you add here if relevant — keep that in mind for naming/shape consistency but do not implement rename logic yourself).
- Run ONLY the centralized test entry point: `./scripts/test.sh -p shamir-index` (and `-p shamir-engine` if you touch anything there). Raw `cargo test` is blocked.
- `cargo fmt` and `cargo clippy --workspace --all-targets -- -D warnings` must be clean before you declare done.

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Do NOT run `git commit` or `git add` — the orchestrator verifies your diff and test run, then commits. Only edit files and run read-only/build/test commands. Delete stray log files you create yourself; mention it if you leave any.

## What to report back

State clearly: whether you fixed sorted only or both sorted+index2, the exact tombstone key name(s) chosen and the collision check you ran for each, what each test proves, and the exact `cargo fmt`/`cargo clippy`/`./scripts/test.sh` commands with real pass/fail counts and exit codes. If index2 was left deferred, quote the decision and reasoning explicitly.
