# Brief for F-33 Step 3 (#837, P1) — measure `TableManager::create`'s open path against a hybrid repo

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

F-33 Step 2 (#836, landed, commit `c32c4e3d`) added `BoxRepo::Hybrid` /
`BoxRepoFactory::Hybrid` to `crates/shamir-engine/src/repo/repo_types.rs`:
an opt-in repo backend where `__history__`/`__data__`/`__tx__`/
`__changelog__` are plain ephemeral in-memory, and `__info__`/`__interner__`
are `MirroredStore`s (memory-primary + durable fjall mirror, config-only
allowlist for `__info__`, allow-ALL for `__interner__`). This step does
**not** wire the hybrid backend into any DDL surface yet — it directly
constructs a hybrid repo via `BoxRepoFactory::hybrid(path)` and measures
whether `TableManager::create`'s existing OPEN SEQUENCE
(`crates/shamir-engine/src/table/table_manager.rs:180-330`) already
produces a coherent result when handed stores from such a repo, or whether
it has a genuine gap that needs fixing before Step 4 wires the DDL surface.

**This is a measurement task first, a fix task only if the measurement
finds something.** Do not "improve" `TableManager::create` speculatively.

## What `create()`'s open sequence does (read the real file first —
this is a summary to orient you, not a substitute)

In order, `TableManager::create(name, data_store, info_store)`:

1. `InternerManager::new(info_store)` — a per-table interner, later
   REPLACED (in `RepoInstance::create_table_context`,
   `repo_instance.rs:412-418`) by `.with_interner(repo_interner)` — the
   real, shared, per-repo interner backed by the **`__interner__`** store
   (unprefixed — confirmed via `repo_instance.rs:381-417`: the exact 4
   `store_get` calls a real table open makes are
   `__data__<table>`, `__info__<table>`, `__history__<table>`, and the
   repo-level `__interner__`, which is exactly the routing table Step 2
   implements).
2. `RecordCounter::new(info_store)` — reads a persisted row-count key.
   This is Class B (derived-from-data) — must read as 0 against a hybrid
   repo's fresh in-memory `__data__`/`__history__`, even though `__info__`
   survived. Confirm the counter's persisted key tag is genuinely excluded
   from `is_durable_table_config`'s allowlist (`crates/shamir-storage/src/storage_mirrored.rs`)
   — cross-check against `MetaKey::Count` in
   `crates/shamir-engine/src/meta/namespace.rs`.
3. `IndexManager::new` / `SortedIndexManager::new` — load persisted index
   DEFINITIONS (schema: which fields have indexes) from `__info__`. This
   is Class A (config) — must survive. The actual index POSTINGS (which
   record ids match which value) are derived-from-data and must NOT
   survive — verify they don't (different key shape, confirmed by Step 1's
   classifier design to fall outside the system-record shape entirely, or
   excluded by tag).
4. `load_validators_metadata(&info_store)` — persisted validator bindings.
   Class A — must survive.
5. `buffer_config::load` — persisted per-table buffer tuning. Class A —
   must survive.
6. `load_index2_metadata` + per-backend `restore_on_open` — most backends
   (Functional/FTS/Btree) fall through to a full **data-store** scan
   rebuild (correctly yields empty against a hybrid repo's empty
   `__data__`/`__history__`). `VectorBackend` OVERRIDES this to try a
   persisted HNSW snapshot from `__info__` FIRST — if that snapshot's key
   shape were ever misclassified as durable, a reopened hybrid table would
   load a STALE non-empty vector index pointing at rows that no longer
   exist. Confirm (empirically, not just by reading) that a vector
   snapshot chunk's key never matches `is_durable_table_config` (check
   `crates/shamir-index/src/vector/snapshot.rs`'s actual key construction).
7. `legacy_indexes_need_rebuild` — reads a persisted posting-format
   version stamp (`_m.idx.lfv`, which Step 1 correctly added to the
   allowlist as durable). Since this tag survives but the postings
   themselves don't, confirm this doesn't cause `create()` to WRONGLY skip
   `mgr.repair()` in a state where it should have rebuilt something (it
   shouldn't — 0 real postings should mean 0 indexed postings is already
   consistent — but verify this empirically, don't just trust the
   argument).

## What to build

A new integration-style test file,
`crates/shamir-engine/src/repo/tests/hybrid_table_open_tests.rs` (wire it
into `crates/shamir-engine/src/repo/tests/mod.rs` next to
`hybrid_repo_tests`, same `#[cfg(all(test, feature = "fjall"))]` gate).

For each scenario below: build a hybrid repo at a `tempfile::tempdir()`
path, call `store_get` for `__data__<table>`/`__info__<table>`/
`__history__<table>`/`__interner__` directly (mirroring
`create_table_context`'s exact call shape — you do NOT need the full
`RepoInstance`/DDL surface, this step is explicitly scoped to NOT touch
that), construct a `TableManager` via `TableManager::create(...)` +
`.with_interner(...)` manually, exercise it, then **simulate a restart**:
drop every handle (repo, stores, table manager — release fjall's
exclusive lock, same pattern as Step 2's tests), build a FRESH
`BoxRepoFactory::hybrid(same path)`, repeat the `store_get`/`create`
sequence, and assert on the reopened `TableManager`:

1. **Index definitions survive, postings don't.** Create a hash index on
   a field pre-restart, insert a row that would match it, confirm the
   index returns a hit pre-restart. Post-restart: the index DEFINITION is
   still visible (schema unchanged) but a lookup for the same value
   returns NO hit (data + postings are gone, not stale/wrong).
2. **Validator bindings survive.** Bind a validator pre-restart, confirm
   `validator_bindings`/`bindings_len` is non-empty post-restart.
3. **Interner coherence.** Intern a field name pre-restart, note its id.
   Post-restart, intern the SAME field name again through the repo-level
   interner and confirm it resolves to the SAME id (not a fresh id from
   an empty interner) — this is the single most safety-critical
   assertion in the whole campaign per the design memo.
4. **Record counter reads 0 post-restart**, not the pre-restart count.
5. **`create()` does not error or panic** on any of the above sequences —
   this alone is a meaningful signal that the open path tolerates
   warm-info/cold-history.
6. **If a vector index (index2) was created and had rows inserted
   pre-restart**: post-restart, `restore_on_open` must not resurrect a
   non-empty vector index — a search must return no results (only add
   this scenario if wiring a vector index2 backend directly, without the
   DDL surface, is straightforward with the tools already available in
   this crate's tests; if it requires DDL-surface machinery this step
   deliberately excludes, skip it and say so explicitly in your summary
   rather than half-wiring it).

## If the measurement finds a real gap

Fix it — minimally, in the actual production file the bug lives in (NOT
in `MirroredStore`, `HybridRepoComposite`, or `is_durable_table_config`,
all already-landed Step 1/2 work — a gap here would most likely mean a
genuinely missing/wrong classifier tag, in which case fixing it belongs in
`crates/shamir-storage/src/storage_mirrored.rs`'s `ALLOWLIST`, or an
actual `TableManager::create` sequencing bug). Document the found gap and
the fix in your summary as precisely as you'd document a new finding — do
not silently patch and move on.

If everything already works: do NOT touch any production file. Only the
new test file + `tests/mod.rs`'s module wiring are new production-side
artifacts. Additionally write a short memo,
`docs/dev-artifacts/research/f33-step3-warm-info-cold-history-measurement.md`,
recording what you measured and confirming (or refuting) each of the 5
numbered assertions above with the actual test evidence.

## Constraints

- Do NOT wire hybrid into `shamir-db`'s DDL surface
  (`extract_storage_type`/`factory_from_meta`/`handle_create_repo`) —
  that's Step 4 (#838), a separate task.
- Do NOT modify `MirroredStore`/`HybridRepoComposite`/
  `is_durable_table_config` unless the measurement finds a genuine,
  demonstrated gap in one of them.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine -- --check` and
  `cargo clippy -p shamir-engine --all-targets -- -D warnings` must be
  clean.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -- hybrid_table_open
./scripts/test.sh -p shamir-engine -- repo
```
