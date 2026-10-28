BINDING DESIGN (from @oh design review, read-only, file:line verified against this worktree):

Verdict: lazy, probe-time catch-up per-field index in a `scc::HashMap` on `StagingStore`. Mutators never decode; they only log overwritten positions. Stale postings are tolerated (they only add false positives that the call sites' existing exact recheck filters). No reverse map. Sound because `writes` is an append-only-position `IndexMap` (set/set_many/remove all `insert`, `staging_store.rs:181,192,199`; no `swap_remove`).

ProbeKey lives at `crates/shamir-types/src/record_view/probe_key.rs` (already in this worktree, uncommitted; exported `shamir_types::record_view::ProbeKey`, `record_view/mod.rs:54`). Keep it.

1. Data structure — NEW `crates/shamir-tx/src/tx/support/staged_field_index.rs` (support/ goes 6→7; register `pub(crate) mod staged_field_index;` in support/mod.rs):
```rust
#[derive(Default)]
pub(crate) struct StagedFieldIndex {
    walked: usize,                              // writes positions folded
    overwrites_seen: usize,                     // cursor into StagingStore::overwritten
    postings: TFxMap<ProbeKey, TSet<usize>>,    // key -> writes positions
    undecodable: TSet<usize>,                   // decode failed: always candidates
}
impl StagedFieldIndex {
    pub(crate) fn catch_up<'a>(&mut self, field: &InternerKey, len: usize,
        overwritten: &[usize], bytes_at: impl Fn(usize) -> Option<&'a Bytes>);
    pub(crate) fn candidates(&self, keys: &[ProbeKey]) -> TSet<usize>; // postings first, then undecodable
}
```
(use whatever Fx map/set aliases shamir_collections actually exports.)
`catch_up` folds positions `walked..len`, then `overwritten[overwrites_seen..]`. Fold one position: `bytes_at(pos)` None (staged Remove) → skip; else decode exactly like `validator_db.rs:103-123` (RecordView::new → scalar_at(from_ref(field)) via trait RecordRef `record_ref.rs:169`, fallback `InnerValue::from_bytes(..).scalar_at`): scalar → `ProbeKey::for_scalar` → insert pos into each posting; missing/non-scalar → nothing; both decodes fail → `undecodable` (keeps `fk_staged_corrupt_parent_fails_closed`, `validator_db_tests.rs:445`, fail-closed). Keep `StagedOp` private — pass the `bytes_at` closure (do not widen visibility: rust-guard).

New StagingStore fields (after `:94`):
```rust
probe_index: scc::HashMap<u64, StagedFieldIndex, THasher>, // keyed by field id; built on first probe
overwritten: Vec<usize>,
probe_built: AtomicBool,
```
`new()`: `scc::HashMap::with_hasher(THasher::default())` (`::new` banned by clippy.toml). No Mutex, no `len()`.

2. Mutation paths:
- `set` `:181`, `set_many` `:192`: `let (i, old) = self.writes.insert_full(..)`; `if old.is_some() && *self.probe_built.get_mut() { self.overwritten.push(i) }`.
- `remove` `:199`: same.
- `rewrite_set_bytes` `:329`: at the top `self.probe_index.clear_sync(); self.overwritten.clear(); *self.probe_built.get_mut() = false;` (field ids change; index rebuilds lazily).
- `drain` `:267`: nothing.

3. Probe API on StagingStore:
```rust
/// Staged Set rows that MAY have `field == value` (superset; callers recheck exactly).
pub fn probe_field(&self, field: &InternerKey, value: &QueryValue) -> Vec<(RecordKey, Bytes)>
```
`ProbeKey::for_query(value)` empty (NaN/arrays/maps) → return every Set row (old linear behaviour). Else `probe_built.store(true)`; `entry_sync(field.id()).or_insert_with(Default::default)` then `get_mut().catch_up(field, writes.len(), &overwritten, |i| ..get_index(i)..)` (entry pattern: `repo_tx_gate.rs:294`); map `candidates` through `writes.get_index` keeping Set rows only.

Call-site swaps (round 2), each keeping the existing exact recheck:
- `validator_db.rs:381-387`: `for (_, bytes) in staging.probe_field(field_id, value) { if record_field_matches_by_id(&bytes, field_id, value)? { return Ok(true); } }`
- `validator_db.rs:501-514`: same with `exclude_rid` skip on `key`.
- `fk_restrict.rs:502-511`: same loop setting `matched`, with `?`.
- `fk_on_update/scan.rs:350-355`: `staging.probe_field(field_id, value).iter().any(|(_, b)| record_field_matches_by_id(b, field_id, value))`.
- Update only the "P1 perf fix: iter_ops()" comments at those four sites.

4. Dependencies: shamir-tx already depends on shamir-types, scc, shamir-collections (`Cargo.toml:16,18,20`). Add nothing.

5. Tests:
- `#[cfg(test)] thread_local! { static FOLD_DECODES: Cell<u64> }` in staged_field_index.rs with pub(crate) read/reset (precedent `record_view/mod.rs:51-52`).
- Parity: NEW `crates/shamir-tx/src/tests/gate_tests/staged_field_index_tests.rs` (gate_tests 6→7; add to its mod.rs). InMemoryStore as `staging_store_tests.rs:7`. Seeded loop of random set/set_many/remove/overwrite/rewrite_set_bytes over {Int, F64, numeric Str, Str, Null, Bool, corrupt b"\xc4"}; after each step assert probe_field+exact predicate == linear iter_ops scan for several query values. Name `rv_engine_0db9257b_index_matches_linear_scan`. Plus `rv_engine_0db9257b_fold_decodes_linear`: 200 probe-then-set cycles, FOLD_DECODES ≤ 200.
- Round 3: `#[cfg(test)] thread_local! STAGED_RECHECKS` in validator_db.rs incremented in `record_field_matches_by_id` (`:103`); NEW `crates/shamir-engine/src/validator/tests/registry/staged_probe_tests.rs` (registry tests 4→5) `rv_engine_0db9257b_staged_probe_is_not_linear`: 200× `exists_in_self("uid", Int(i), None)` then staging insert i in one tx (reuse setup/execute_insert_tx `validator_db_tests.rs:153,258`), assert count ≤ 400 (old ≈ 19,900).

6. Rounds: R1 shamir-tx only (probe_key.rs + index + fields + mutators + probe_field + both shamir-tx tests); R2 four engine call sites; R3 engine counter + regression test. Gates per round: fmt -p, clippy -p <crate> --all-targets -D warnings (R1 also shamir-engine), check_layout.
