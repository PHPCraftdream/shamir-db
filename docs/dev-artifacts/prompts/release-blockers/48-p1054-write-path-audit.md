# Brief 48 — #1054: exhaustive audit of write paths mutating regular/hash postings

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

This is a **READ-ONLY investigation task**. Do not edit any production code.
The only file you write is the new research document described below.

## Why this matters

The online-CREATE-INDEX redesign (RFC at
`docs/dev-artifacts/research/2026-08-07-online-index-build-rfc.md`, slice 1
tracked as tasks #1055-#1062) captures concurrent writes during backfill via
a "dirty-set" — the set of `RecordId`s touched while a build is in flight,
later re-read and re-indexed. **A missed write path = a permanently missing
posting = silent index corruption that no test catches except a targeted
one.** This task's only job is to make the list of capture points provably
exhaustive before any of #1055-#1062 starts.

## What's already found (starting point, not the answer — re-verify it)

1. **tx-staged path**: `crates/shamir-engine/src/table/table_manager_tx_ops.rs`
   plans regular/unique index-ops at STAGE time against
   `index_manager.generation()` — 6 sites (`:451, :592, :785, :924, :999, :1147`),
   each calling `tx.note_base_index_stage_gen(token, base_index_gen)`
   (`:479, :677, :854, :968, :1094, :1193`). At commit,
   `crates/shamir-engine/src/tx/pre_commit.rs::rederive_base_index_ops_post_stage`
   (`:1345`) compares `mgr.generation()` against the staged value from
   `tx.base_index_stage_gens` (`crates/shamir-tx/src/tx_context.rs:157, :595`)
   and re-derives ops on mismatch.
2. **non-tx path**: `crates/shamir-engine/src/table/table_manager_crud.rs`
   calls `index_manager.on_record_created/on_record_deleted/on_record_updated`
   DIRECTLY, bypassing the generation-gate — `insert` (~`:142-215`, call at
   `:211`), `delete` (~`:452-457`), upsert/update (~`:515-570`, calls at
   `:550/:562/:565`).
3. **doctor::repair() rebuild path**: `crates/shamir-engine/src/table/doctor.rs`
   calls `on_record_created` in a rebuild loop at `:608, :620, :626` — a
   THIRD structurally distinct path, found during this same investigation on
   2026-08-09 but not yet written up.

## What to do

1. Enumerate EVERY path that can mutate the regular/hash posting keyspace.
   Check, beyond the three above:
   - `DROP TABLE CASCADE`
   - replication apply (`crates/shamir-engine/src/tx/apply_replicated.rs` —
     confirmed during a prior grep to touch `index_write_set`/WAL replay
     machinery; trace whether it independently mutates postings or only
     replays already-planned ops)
   - WAL recovery/replay (`crates/shamir-engine/src/tx/recovery.rs`'s
     `replay_v2_op` — confirmed to apply `IndexPut`/`IndexDel` directly to
     `info_store.set`/`.remove`; determine whether this replays
     ALREADY-COMPUTED posting keys captured at original commit time (in
     which case it needs NO separate dirty-set capture — the original
     commit already went through one of paths 1-3) or independently derives
     new ones)
   - migration coordinator (grep found nothing at
     `crates/shamir-engine/src/migration/coordinator.rs` in a prior pass —
     confirm this file exists and re-check; migration APIs may live
     elsewhere in the tree)
   - any batch/bulk insert paths distinct from the ones above
   - any table-open recovery path that re-derives postings (distinct from
     `doctor::repair()`)
2. For EACH path found, record: file:line of the mutation point, whether it
   passes through `ddl_admission`/the write barrier, whether it can see
   `IndexState::Building` indexes, and where physically a dirty-set capture
   call would go if this path needs one.
3. For paths that only REPLAY already-computed keys (like WAL recovery
   likely does), explicitly argue why they do NOT need independent dirty-set
   capture — the argument should be "the original write already went
   through a capturing path", not an assumption.
4. State the METHOD used to claim exhaustiveness — e.g. "grep for every
   caller of `set_many`/`remove_many` on the posting keyspace, cross-checked
   against every caller of `on_record_*`, cross-checked against every
   construction site of `IndexWriteOp::SetPosting`/`RemovePosting` for the
   regular family" — not "grepped a few likely names".

## Output

Write `docs/dev-artifacts/research/2026-08-09-p1054-write-path-audit.md`
with: a table of all paths found (file:line, barrier coverage, Building
visibility, capture point), the exhaustiveness method used, and an explicit
statement of which paths need NEW dirty-set capture (for #1058) versus which
are already covered transitively. This document will be cited directly from
the RFC revision (#1055) — write it for that audience.

Do not modify `docs/dev-artifacts/research/2026-08-07-online-index-build-rfc.md`
itself — that's #1055's job, not this one.

## Gate before you report done

No code changes are expected, so `cargo fmt`/`clippy`/tests should be a
no-op. Run them anyway to confirm nothing was accidentally touched:

```
git status --short
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

Report the exact path list you found, not a summary claiming "found all
paths" — list them.
