# Brief 49 — #1054 round 2: correct the audit's Path 1 "already covered" claim

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

This is a **READ-ONLY investigation task**. Do not edit any production code.
The only file you edit is the research document from round 1.

## What round 1 got right

`docs/dev-artifacts/research/2026-08-09-p1054-write-path-audit.md` correctly
found 7 write paths and correctly identified that Path 2 (non-tx CRUD) needs
new dirty-set capture, Path 3-6 don't (rebuilds/backfills/migration), and
Path 7 (WAL recovery) is a safe replay. Keep all of that.

## What round 1 got wrong — Path 1 is NOT "already covered"

The document's §Path 1 claims: "Dirty-set capture point: **Already
covered** — the generation gate at commit time is the canonical capture
mechanism for this path" and describes only the RE-DERIVATION branch
(`rederive_base_index_ops_post_stage` firing when `mgr.generation() !=
stage_gen`). This is incomplete and the conclusion is wrong. Here is the
actual mechanism, verified by reading the code directly (2026-08-09):

1. **`IndexManager::plan_record_created`/`plan_record_updated`/
   `plan_record_deleted`** (`crates/shamir-index/src/base_index/index_manager.rs:2013-2038,
   2105-2130, 2200+`) iterate `self.indexes.iter()` with **NO filter on
   `IndexState`** — every registered def, `Building` or `Ready`, gets
   planned and produces `IndexWriteOp::SetPosting`.

2. **`self.indexes.add_index(index_def)` registers the def at `Building`
   BEFORE the backfill scan starts** — see `create_index_from_stream`'s
   Phase 1 (`index_manager.rs:1615-1620`, comment: "register the definition
   FIRST, at Building"). From that instant, ANY subsequent call to
   `plan_record_created`/etc. includes this Building def.

3. **This is called from ORDINARY stage-time planning, not just
   re-derivation.** `crates/shamir-engine/src/table/table_manager_tx_ops.rs::plan_base_index_insert_ops`
   (`:280-293`) — called at STAGE time for every normal `insert_tx`
   (`:456-457`), not gated on any generation check — calls
   `self.index_manager.plan_record_created(&rid, rec)` directly (`:285`).
   The generation-gate re-derivation path round 1 focused on is a SEPARATE,
   ADDITIONAL mechanism for the case where an index was registered AFTER
   stage but BEFORE commit — it is not the only way a tx-staged write
   reaches a Building index's postings. The much MORE COMMON case is: index
   already registered Building, tx stages AND commits entirely after that
   (no generation change at all), and ordinary stage-time planning still
   writes directly into the Building index's posting keyspace via the exact
   same `plan_record_created` call.

4. **Why this is safe TODAY and unsafe under online-build.** Today,
   `TableManager::create_index` holds `begin_write_barrier` across the
   ENTIRE Phase 1→2→3 sequence (documented at
   `index_manager.rs:1501-1519`'s own doc comment, cited in the RFC's §1.1).
   So nothing can reach `plan_base_index_insert_ops`/commit while a build is
   in flight — the barrier blocks ALL writers, tx-staged or not, for the
   whole duration. This is EXACTLY the "gets delta catch-up for free"
   mechanism the RFC's own source citations describe. Once slice 1 removes
   the barrier for Phase A/B/C (the entire point of the online-build
   redesign), this same code path will execute CONCURRENTLY with Phase A's
   snapshot scan — landing directly on the same posting keyspace Phase A is
   also writing to. This is EXACTLY the hazard the original RFC's Claim 2
   (§3) describes: "Phase A's own read of a row the hook has ALSO logged a
   delta for... Phase A's posting write for R... and Phase C's replay of
   R's post-V delta must not race each other destructively" — except
   without dirty-set interception, Path 1's direct write isn't even logged
   anywhere Phase C can find it; it just silently races Phase A's write with
   no ordering guarantee at all.

## What to fix in the document

1. Correct §Path 1's "Dirty-set capture point" from **Already covered** to
   **NEW CAPTURE NEEDED** — same conclusion as Path 2, same underlying
   reason (both funnel through `IndexManager::plan_record_*`, which writes
   directly to Building-index postings with no state check).
2. Note explicitly that Path 1 and Path 2 are NOT independent write paths at
   the mechanism level — they are two different CALLERS
   (`table_manager_tx_ops.rs`'s stage-time/re-derivation planning vs.
   `table_manager_crud.rs`'s direct `on_record_*` calls) of the exact SAME
   underlying `IndexManager`/`SortedIndexManager` planning methods. This
   means the capture point most likely belongs INSIDE those shared methods
   (`IndexManager::plan_record_created`/`updated`/`deleted`,
   `SortedIndexManager`'s equivalents) — checking per-def `IndexState` and,
   for a `Building` def with an active in-flight online-build registry entry
   (per #1058's planned registry), routing to dirty-set capture INSTEAD OF
   (or ahead of) producing a direct `SetPosting`/`RemovePosting` for that
   specific def — rather than at each of the ~15 scattered call sites in
   `table_manager_tx_ops.rs` and `table_manager_crud.rs` individually. Argue
   for or against this single-choke-point design explicitly; if you disagree
   after checking, say why with citations.
3. Re-check whether `SortedIndexManager::plan_record_created`/etc. (used at
   `table_manager_tx_ops.rs:291` and elsewhere) has the same no-state-filter
   behavior — round 1 didn't check this for the sorted family specifically
   (it's out of scope for slice 1's regular/hash-only target, but the
   architectural finding about "capture point should live in the shared
   planning method, not at scattered call sites" may generalize, and it's
   worth one grep to confirm or refute before this document is cited by the
   RFC revision).
4. Update the "Paths Requiring NEW Dirty-Set Capture" and "Paths Already
   Covered Transitively" sections and the summary table to reflect the
   correction. Update the "Conclusion" section's counts (currently says "1
   path already covered, 1 path needs new capture" — should become "2 paths
   need new capture, sharing one underlying mechanism").
5. Leave everything else in the document as-is — Paths 2-7's analysis holds.

## Gate before you report done

```
git status --short
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

No production code should be touched — confirm `git status --short` shows
only the research document changed.
