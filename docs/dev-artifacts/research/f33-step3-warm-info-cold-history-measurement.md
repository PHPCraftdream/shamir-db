# F-33 Step 3 (#837) — measuring `TableManager::create`'s open path against a hybrid repo

## Summary

**No gap was found.** `TableManager::create`'s existing open sequence
(`crates/shamir-engine/src/table/table_manager.rs:180-330`) already
produces a fully coherent result when handed stores from a
`BoxRepo::Hybrid` repo across a simulated process restart. All 5 numbered
assertions from the brief hold, plus the optional 6th (vector index2)
scenario, which turned out to be straightforward to wire directly via
`TableManager::create_index_v2` (no DDL surface needed). No production
file was touched.

## What was built

A new test file,
`crates/shamir-engine/src/repo/tests/hybrid_table_open_tests.rs`, wired
into `crates/shamir-engine/src/repo/tests/mod.rs` under the same
`#[cfg(all(test, feature = "fjall"))]` gate as `hybrid_repo_tests`. Six
`#[tokio::test]`s, each following the same shape:

1. Build a hybrid repo at a fresh `tempfile::tempdir()` path
   (`BoxRepoFactory::hybrid(path).create()`).
2. Open a table by hand via a local `open_table()` helper that mirrors
   `RepoInstance::create_table_context`'s exact `store_get` call shape
   (`__data__<table>`, `__info__<table>`, `__history__<table>`,
   `__interner__`) and then `TableManager::create(...).with_interner(...)`
   — no `RepoInstance`/DDL surface involved, per the brief's scope.
3. Exercise the table (index/validator/interner/row/vector-index setup as
   needed per scenario).
4. **Simulate a restart**: drop every handle (`TableManager`, stores, the
   hybrid `BoxRepo` itself — releasing fjall's exclusive directory lock),
   then build a **fresh** `BoxRepoFactory::hybrid(same path)` and repeat
   step 2.
5. Assert on the reopened `TableManager`.

## Per-assertion evidence

### 1. Index definitions survive, postings don't
`index_definition_survives_but_postings_do_not`: created a hash index
`by_x` on field `x`, inserted a row with `x = 42`, confirmed
`lookup_by_index` finds 1 hit pre-restart. Post-restart: the index
definition is still present (`iter_indexes().next()` yields a def with
the SAME `name_interned`), but `lookup_by_index` on the identical value
returns 0 hits. Confirms the index-definition/posting split from the
brief: `IndexManager::new` reloads defs from `__info__` (mirrored,
durable), while postings are `__info__`-resident *derived* keys whose
underlying row no longer exists in the now-empty `__data__`/`__history__`
— so the postings themselves come back empty because `create_index`
never re-derives them and the physical posting keys written before the
restart never round-tripped through the classifier in the first place
(the previous process's postings lived in a store instance that no
longer exists; the fresh hybrid repo's `__info__` MirroredStore only
replays what `is_durable_table_config` accepted, and posting keys are a
different key shape entirely — never 16 bytes, so they never even reach
the tag comparison).

### 2. Validator bindings survive
`validator_bindings_survive_restart`: bound a validator on `accounts`,
confirmed `validator_bindings().len() == 1` pre-restart. Post-restart:
`validator_bindings().len() == 1` again, with the same `validator_id`.
`load_validators_metadata` reads from `__info__`'s `_m.val` tag, which is
in the `ALLOWLIST` — confirmed empirically, not just by reading the
allowlist.

### 3. Interner coherence (most safety-critical)
`interner_resolves_same_field_to_same_id_after_restart`: interned
`"email"` pre-restart via `Interner::touch_ind`, captured the assigned id,
persisted the new key via `InternerManager::save_new_keys`. Post-restart:
touching `"email"` again on the FRESH repo-level interner (built from
`__interner__`, allow-ALL mirrored) returns `TouchInd::Exists` (not
`TouchInd::New`) with the SAME id. This is the assertion the brief calls
out as most safety-critical — a mismatch here would silently corrupt
every persisted index definition that references the old id. Confirmed:
`__interner__`'s allow-ALL classifier round-trips every chunk, and
`InternerManager::get()`'s chunk-scan boot path reconstructs the exact
same `(InternerKey, UserKey)` mapping.

### 4. Record counter reads 0 post-restart
`record_counter_reads_zero_after_restart`: inserted 3 rows, confirmed
`counter().get() == 3` pre-restart (after `flush_metadata()` persists it).
Post-restart: `counter().get() == 0`. `MetaKey::Count`'s tag (`"count"`)
is explicitly NOT in `ALLOWLIST` (cross-checked directly in
`crates/shamir-storage/src/storage_mirrored.rs`), so the counter's
persisted key never mirrors to disk — `RecordCounter::new` lazily
hydrates from a fresh, empty `__info__`-adjacent read that finds nothing,
starting at 0. Class B (derived-from-data) behaves exactly as designed.

### 5. `create()` does not error or panic
Every scenario above already exercises this implicitly (a panic on
`.unwrap()` around `TableManager::create` would fail the test outright).
`create_does_not_error_or_panic_across_restart` additionally isolates the
`legacy_indexes_need_rebuild` / `_m.idx.lfv` marker path called out in the
brief's point 7: the marker is stamped to `2` on first open (with a
legacy hash index present), survives the restart (its tag `_m.idx.lfv` is
in `ALLOWLIST`), stays `2` after reopen (no wasted re-stamp), and
`verify()` reports healthy post-restart — confirming empirically that 0
real postings against 0 rows is already a consistent state and `create()`
does NOT wrongly trigger a `repair()` pass that would have nothing
correct to reconcile against (there is nothing in `__data__`/`__history__`
that `repair()` could rebuild from, so it correctly writes 0 postings,
matching the already-0 posting count in `__info__`).

### 6. Vector index2 (optional, included)
`vector_index_does_not_resurrect_after_restart`: wiring a vector index2
backend directly turned out to be straightforward without any DDL-surface
machinery — `TableManager::create_index_v2` (a plain `TableManager`
method, already used this way by
`crates/shamir-engine/src/table/tests/index2_persistence_tests.rs`) is
sufficient. Created a `vector` index on `embedding` (dim 3, cosine),
inserted 3 vectors, confirmed a similarity search returns 2 ranked hits
pre-restart. Post-restart: the index2 *descriptor* survives (1 backend
registered, `IndexKind::Vector`), but the same similarity search returns
`Ranked([])` — 0 hits. This directly confirms the brief's concern about
`VectorBackend::restore_on_open` preferring a persisted HNSW snapshot: no
such snapshot exists in the hybrid repo's `__info__` (its keys, per
`crates/shamir-index/src/vector/snapshot.rs`'s `chunk_key` /`sidecar_key`
/ `manifest_key` / `delta_chunk_key`, are ASCII strings like
`"<keyspace>.g0.data.000000"` — never the 16-byte system-record shape
`is_durable_table_config` checks — so they can never match the
allowlist and are correctly never mirrored), so `restore_on_open` falls
through to its full data-store-scan rebuild path, which correctly yields
an empty graph against the empty `__data__`.

## Why no fix was needed

The hybrid repo's per-store routing table (Step 2) and the `__info__`
classifier allowlist (Step 1) were already precise enough that every
"config vs. derived-from-data" distinction `TableManager::create` relies
on lines up exactly with what actually survives a hybrid-repo restart:

- Config-shaped `RecordId::system(...)` keys with an allowlisted tag
  (`indexes`, `indexes_uniq`, `sorted_index`, `_m.val`, `_m.idx.lfv`,
  interner chunks `i.d*`, etc.) mirror to disk and come back.
- Data-derived state (`MetaKey::Count`, postings, HNSW snapshots) either
  has a non-allowlisted tag or, in the posting/snapshot case, never even
  has the 16-byte system-record shape the classifier checks — so it's
  excluded by construction, not by an easily-missed tag omission.

No `MirroredStore`, `HybridRepoComposite`, `is_durable_table_config`, or
`TableManager::create` production code was modified as part of this
task. The only new artifacts are
`crates/shamir-engine/src/repo/tests/hybrid_table_open_tests.rs` and the
one-line module registration in
`crates/shamir-engine/src/repo/tests/mod.rs`.

## Test evidence

```
./scripts/test.sh -p shamir-engine -- hybrid_table_open
     Summary [0.461s] 6 tests run: 6 passed, 1684 skipped

./scripts/test.sh -p shamir-engine -- repo
     Summary [3.046s] 101 tests run: 101 passed, 1589 skipped
```

`cargo fmt -p shamir-engine -- --check` and
`cargo clippy -p shamir-engine --all-targets -- -D warnings` are both
clean.

## Scope confirmation

Per the brief's constraints: hybrid was NOT wired into `shamir-db`'s DDL
surface (that remains Step 4 / #838); `MirroredStore` /
`HybridRepoComposite` / `is_durable_table_config` were not touched;
`TableManager::create` was not modified. Only the new test file + the
`tests/mod.rs` module registration are new production-side (test-only)
artifacts.
