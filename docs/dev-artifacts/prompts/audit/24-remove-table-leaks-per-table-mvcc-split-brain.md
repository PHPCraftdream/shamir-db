Task: MEDIUM concurrency — `remove_table` does not clean up the
`per_table_mvcc` registry entry, so a DROP followed by a CREATE of the
SAME table name creates a split-brain: the `TableManager` reads
through a NEW `MvccStore` while the commit pipeline/SSI
provider/drainer keep writing through the OLD (stale) one, since
`per_table_mvcc.insert` silently no-ops when the token already exists
(audit finding A13, `docs/dev-artifacts/audits/2026-07-06-concurrency-engine.md`).

## Where

- `crates/shamir-engine/src/repo/repo_instance.rs`, `remove_table`
  (~line 375-389, confirm current lines):
  ```rust
  pub fn remove_table(&self, table_name: &str) -> bool {
      let removed = self.configs.remove(table_name).is_some();
      if removed {
          self.tables.remove(table_name);
          let token = table_token_for(table_name);
          let _ = self
              .token_names
              .remove_if(&token, |existing| existing.as_str() == table_name);
      }
      removed
  }
  ```
  This removes `configs` (the catalogue), `tables` (the `OnceCell`-style
  `TableManager` cache), and the `token_names` reverse-index entry — but
  it does **NOT** call `self.per_table_mvcc.remove(&token)`. The old
  `MvccStore` Arc for this token stays registered in `per_table_mvcc`
  indefinitely.
- `crates/shamir-engine/src/repo/repo_instance.rs`, `create_table_context`
  (~line 303-348, confirm current lines): `let _ =
  self.per_table_mvcc.insert(token, Arc::clone(&mvcc));` (~line 321) —
  `scc::HashMap::insert` returns `Err` (silently discarded via `let _
  =`) if the key ALREADY EXISTS; it does NOT overwrite. So when a table
  is dropped and immediately recreated under the SAME name (same
  `table_token_for` result, since the token is a deterministic hash of
  the name), `create_table_context` builds a brand-NEW `MvccStore` (a
  fresh `history_store`/gate-wired instance) and tries to register it
  — but the STALE entry from the dropped table is still present in
  `per_table_mvcc`, so the insert silently fails and the OLD `MvccStore`
  remains the one other subsystems resolve through.
  - The `TableManager` returned by `create_table_context` DOES hold the
    new `mvcc` directly (via `.with_mvcc_store(mvcc)`), so reads/writes
    issued THROUGH THE TABLEMANAGER go to the NEW store.
  - But other subsystems resolve the MvccStore by looking it up in
    `per_table_mvcc` BY TOKEN, independently of the `TableManager`
    instance — per the audit: `commit_phases.rs:511`,
    `pre_commit.rs:63`, `drainer.rs:410` (confirm exact current line
    numbers — these may have shifted). These all resolve to the OLD,
    STALE `MvccStore` still sitting in `per_table_mvcc` under this
    token.
- `crates/shamir-engine/src/repo/repo_instance.rs`,
  `rename_table_stores` (~line 401-450+, confirm current lines):
  the module doc/comments reference "renaming" clearing the FROM
  token's registration too — confirm whether `rename_table_stores`
  ALSO fails to clean up `per_table_mvcc` for the `from` token (it
  currently reads `self.per_table_mvcc.get(&from_token)` to drain, but
  does it ever REMOVE that entry after the rename completes, or does
  the old token's entry leak the same way?). If it shares the same gap,
  fix both call sites; if `rename_table_stores` already handles this
  correctly (e.g. by re-registering the mvcc under the new token AND
  removing the old one), confirm and note in your report.

## Why this is MEDIUM

**Concrete interleaving from the audit:**
1. `DROP TABLE t` → `remove_table("t")` runs: `configs`, `tables`,
   `token_names` are cleared, but `per_table_mvcc[token_for("t")]`
   STILL holds the OLD `MvccStore` Arc.
2. `CREATE TABLE t` (same name) → eventually triggers
   `create_table_context("t")`: builds a NEW `MvccStore` (fresh
   `history_store` handle, wired to the SAME shared `gate`), tries
   `per_table_mvcc.insert(token, new_mvcc)` — **silently fails** (key
   already present from step 1's leaked entry) — the NEW `TableManager`
   holds `new_mvcc` directly, but `per_table_mvcc[token]` still points
   at the OLD `mvcc`.
3. A client issues a WRITE through the new `TableManager` (e.g. via a
   transaction). The tx commit pipeline's internals
   (`commit_phases.rs`/`pre_commit.rs`) resolve the table's MvccStore
   by looking it up in `per_table_mvcc` BY TOKEN — they get the OLD
   `mvcc`, not the `new_mvcc` the `TableManager` actually holds. The
   committed tx's overlay/version data lands in the OLD MvccStore's
   in-memory overlay — the OLD store's `history_store` handle may point
   at an already-orphaned/deleted physical store (since DROP typically
   orphans or deletes the underlying `__data__`/`__history__` stores
   for the dropped name — confirm this against `remove_table`'s actual
   disposition, noted in the codebase as "same disposition as
   `drop_table`, which orphans `__data__`").
4. A client then READS through the new `TableManager` — reads resolve
   via the `TableManager`'s own held `new_mvcc` reference directly
   (bypassing the `per_table_mvcc` lookup, since the `TableManager`
   already has its own `Arc<MvccStore>` field) — so the just-committed
   write (landed in the OLD store per step 3) is **INVISIBLE** to this
   read. **Committed transactions silently vanish** from the new
   table's perspective — a genuine data-loss / split-brain bug, not
   just a resource leak.
5. Background machinery (the drainer, per the audit) ALSO resolves via
   `per_table_mvcc` by token — so it keeps draining the OLD, orphaned
   MvccStore's overlay (which may reference deleted physical stores),
   potentially erroring or silently doing nothing useful, while the
   NEW table's `TableManager`-held MvccStore's overlay is NEVER
   drained by the background drainer at all (the drainer never sees it,
   since it's not registered in `per_table_mvcc` under this token).

## Fix

Per the audit's fix sketch: **`remove_table` MUST also
`per_table_mvcc.remove(&token)`** (and `rename_table_stores` must do
the equivalent for the FROM token, if it currently doesn't already).

Concretely:
1. In `remove_table`, after computing `token` (or reuse the existing
   `token` binding already computed for the `token_names` cleanup),
   add: `let _ = self.per_table_mvcc.remove(&token);` — this ensures a
   subsequent `CREATE TABLE` under the same name's `create_table_context`
   will find NO stale entry and its `insert` will succeed, registering
   the genuinely-new `MvccStore`.
2. Consider (and report on) whether removing the `per_table_mvcc` entry
   needs any additional care — e.g., should any IN-FLIGHT transaction
   still holding a reference to the OLD `MvccStore` (via its own
   `TableManager` instance, captured before the drop) be allowed to
   finish draining/settling first? Since `Arc<MvccStore>` reference
   counting means the OLD store object itself isn't freed until the
   last `Arc` clone (held by any in-flight `TableManager`/tx) drops —
   removing it from `per_table_mvcc` only stops NEW lookups from
   finding it; anything ALREADY holding a clone keeps working against
   it (whether that's correct or itself a separate concern is worth
   noting, but the audit's core ask is specifically about the registry
   leak enabling the NEW table's create to silently fail to register —
   fix that; flag any deeper in-flight-transaction-drain concern as a
   follow-up if it's out of scope for a surgical fix here).
3. Check `rename_table_stores` (the "2. Drop the old live registration"
   comment at ~line 440+ suggests it may already call `remove_table`
   internally for the `from` name, in which case fixing `remove_table`
   automatically fixes the rename path too — confirm this via the
   actual call graph rather than assuming).

Do NOT change the token-collision-safety check in `token_names.remove_if`
(the `existing.as_str() == table_name` guard) — that's correct and
unrelated; only add the missing `per_table_mvcc.remove(&token)` call
(and confirm/fix the rename path).

## TDD requirement

1. **Red**: write `#[tokio::test]`s (check
   `crates/shamir-engine/src/repo/tests/` or wherever
   `remove_table`/table-lifecycle tests live, and follow established
   patterns) that:
   - Reproduce the exact split-brain: create table `t`, commit a write
     to it, drop `t`, create a NEW table `t` (same name), commit a
     DIFFERENT write through the new `TableManager`, then READ through
     the new `TableManager` and assert the just-committed write IS
     visible. This should FAIL before the fix (the write silently lands
     in the stale old MvccStore per the interleaving above, so the read
     through the new TableManager's own held store sees nothing) and
     PASS after the fix (the new create's `per_table_mvcc.insert`
     genuinely succeeds since the stale entry was removed, so the
     commit pipeline resolves the SAME new store the TableManager
     holds).
   - A more direct/unit-level assertion if feasible: after
     `remove_table("t")`, assert
     `repo.per_table_mvcc().get(&token_for("t"))` returns `None` (the
     registry entry is genuinely gone) — this is the most direct,
     minimal proof of the fix, complementing the end-to-end
     drop-then-recreate-then-verify-visibility test above.
   - A regression test confirming normal drop (no recreate) still works
     (table genuinely gone, `has_table` returns false, no panic/error
     from any background machinery that might have held a reference).
2. **Green**: apply the fix.
3. Confirm existing table-lifecycle tests (create/drop/rename) still
   pass.

## Test scope command

```
./scripts/test.sh -p shamir-engine -- remove_table
./scripts/test.sh -p shamir-engine -- repo
```

## Gate (must be clean before finishing)

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
```

If clippy flags PRE-EXISTING lints in code you did not touch, do not
fix them here — note them in your final report instead.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

When done, report exactly:
- The exact `per_table_mvcc.remove` call added to `remove_table`.
- Whether `rename_table_stores` already handled the FROM-token cleanup
  correctly (e.g. by delegating to `remove_table`) or needed an
  analogous fix, with evidence from the actual call graph.
- The failing-then-passing test evidence for the drop-then-recreate
  split-brain reproduction, plus the direct registry-emptiness
  assertion.
- Confirmation existing table lifecycle tests (create/drop/rename)
  still pass.
- Full test/gate results (exact commands + pass/fail).
