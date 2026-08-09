# Brief 61 — #1088: Phase C (catch-up loop) + Phase D (publish barrier)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## Context — read this before touching code

This is slice 1d-2 of online CREATE INDEX (RFC v3,
`docs/dev-artifacts/research/2026-08-07-online-index-build-rfc.md` —
**read §2.2, §2.4, §2.5, §3 Claim 2, and the "Revision log (v3)" section at
the top before writing any code**; the v3 revision fixed a real correctness
gap that changes this task's design from what v2/the original task
description said).

`#1087` (Phase B + Phase A) already landed
(`crates/shamir-engine/src/table/table_manager_index_mgmt.rs`,
`phase_b_a_backfill`): it raises the write barrier, opens a
`SnapshotGuard` (pinning a version), registers the index at `Building`,
marks it in-flight (activates dirty-set capture on every write path via
`IndexManager::is_build_in_flight`), drops the barrier, then scans the
pinned snapshot via `MvccStore::snapshot_stream` and writes postings for
every row that existed at the pin. **This brief includes a small,
necessary change to that function's contract (Part 0 below) — do not
skip it, the rest of the task depends on it.**

## Part 0 — extend `phase_b_a_backfill`'s contract (prerequisite)

Currently `phase_b_a_backfill` drops its `SnapshotGuard` at the end (comment:
"SnapshotGuard dropped here (RAII)") and returns `DbResult<bool>`. RFC v3
requires the guard to stay alive through Phase C too (Phase C needs
`MvccStore::get_at(key, pin)` — the pin-time read — which is only valid
while the guard holds `active_snapshots` open against GC). Change the
signature to hand the guard (and the pin version) to the caller instead of
dropping it:

```rust
pub(crate) struct PhaseBAResult {
    pub guard: shamir_tx::SnapshotGuard,
    pub pin: u64,
}

pub(crate) async fn phase_b_a_backfill(
    &self,
    index_def: crate::index::index_definition::IndexDefinition,
    batch_size: usize,
) -> DbResult<Option<PhaseBAResult>>
```

- `Ok(None)` — changefeed not wired, online build unavailable (same
  fallback signal as today's `Ok(false)`).
- `Ok(Some(PhaseBAResult { guard, pin }))` — success. `pin` is exactly
  `guard.version()` (already computed inside the function as `let pin =
  guard.version();` — just don't drop `guard` at the end, return it
  instead).
- Remove the "SnapshotGuard dropped here (RAII)" comment block and the
  implicit drop — `guard` must be MOVED into the returned `PhaseBAResult`,
  not dropped.

Update `phase_b_a_backfill`'s 3 existing tests in
`crates/shamir-engine/src/table/tests/p1087_phase_b_a_tests.rs` to match:
- `p1087_phase_b_a_correctness_no_concurrency`: `let result =
  tbl.phase_b_a_backfill(...).await.unwrap(); let result =
  result.expect("online build should succeed");` then use `result.guard`/
  `result.pin` if needed, or just let `result` drop at the end of the test
  (fine — the test doesn't need the guard, it only checks postings).
- `p1087_phase_b_a_concurrent_write_captured_in_dirty_set`: same shape.
- `p1087_phase_b_a_fallback_when_changefeed_absent`: `assert!(result.is_none())`.

Re-run `./scripts/test.sh -p shamir-engine -- p1087_phase_b_a` after this
change and confirm all 3 still pass before moving to Part 1.

## Part 1 — `IndexManager::apply_catchup_batch` (bypass helper)

**Why existing methods don't work here.** `plan_record_created`/
`plan_record_updated`/`plan_record_deleted` are gated by `is_build_in_flight`
(`crates/shamir-index/src/base_index/index_manager.rs:2223-2224` and
siblings) — when a def is `Building` AND in-flight, those methods route the
write INTO the dirty-set instead of producing `SetPosting`/`RemovePosting`.
Calling them from Phase C would just re-capture the same id forever, never
actually writing. Phase C needs a helper that writes DIRECTLY, bypassing
that gate, for exactly this one def during catch-up.

**Why the diff needs BOTH pin-time and current-time values.** The physical
posting key is `build_posting_key(index_key, record_id)` where `index_key`
is built from a hash of the indexed VALUE
(`crates/shamir-index/src/base_index/index_keys.rs:255-311`,
`build_index_key_from_record` → `IndexRecordKey::with_hash`). A row that
existed at the pin (Phase A wrote a posting for it, keyed by its pin-time
value) and was updated to a different value or deleted during the
barrier-free window needs its PIN-TIME posting key removed and its
CURRENT-state posting key (if any) written — reading only the current
state cannot reconstruct what the pin-time key was. See RFC v3's revision
log entry 6 and §2.4 for the full argument.

Add to `crates/shamir-index/src/base_index/index_manager.rs`, near
`write_postings_batch` (mirror its batching/cache-invalidation shape):

```rust
/// #1088: apply a batch of pin-vs-current posting diffs directly for a
/// specific Building+in-flight def, bypassing the in-flight dirty-set-
/// capture gate (`is_build_in_flight`). Called ONLY from Phase C/D's
/// catch-up loop (`TableManager::phase_c_d_catchup_and_publish`, a
/// different crate) — every other write path routes through
/// plan_record_created/updated/deleted, which correctly captures to the
/// dirty-set while a build is in-flight; calling those here would just
/// re-capture instead of writing.
///
/// `deltas`: one `(record_id, value_at_pin, value_now)` triple per drained
/// dirty-set id. `value_at_pin`/`value_now` are `None` when the row did not
/// exist at that version (pin: Phase A's scan never saw it and wrote no
/// posting for it; now: the row is deleted).
pub async fn apply_catchup_batch(
    &self,
    name_interned: u64,
    deltas: Vec<(RecordId, Option<InnerValue>, Option<InnerValue>)>,
) -> DbResult<()> {
    let Some(def) = self.indexes.get_index(name_interned) else {
        return Ok(());
    };

    let mut removes: Vec<RecordKey> = Vec::new();
    let mut sets: Vec<(RecordKey, Bytes)> = Vec::new();
    let mut cache_keys: Vec<Bytes> = Vec::new();

    for (record_id, old_value, new_value) in &deltas {
        let old_key = old_value.as_ref().and_then(|v| {
            build_index_key_from_record(false, name_interned, v, &def.paths)
        }).map(|irk| irk.to_bytes());
        let new_key = new_value.as_ref().and_then(|v| {
            build_index_key_from_record(false, name_interned, v, &def.paths)
        }).map(|irk| irk.to_bytes());

        if old_key == new_key {
            continue; // unchanged: both None, or same computed key
        }
        if let Some(ok) = &old_key {
            removes.push(build_posting_key(ok, record_id).into());
            cache_keys.push(ok.clone());
        }
        if let Some(nk) = &new_key {
            sets.push((build_posting_key(nk, record_id).into(), Bytes::new()));
            cache_keys.push(nk.clone());
        }
    }

    if !removes.is_empty() {
        self.info_store.remove_many(removes).await?;
    }
    if !sets.is_empty() {
        self.info_store.set_many(sets).await?;
    }
    for k in cache_keys {
        self.posting_cache.remove(&k);
    }
    Ok(())
}
```

Check the exact `Store::remove_many`/`set_many` signatures before writing
this (`crates/shamir-storage/src/types.rs` — `remove_many(keys: Vec<RecordKey>)
-> DbResult<Vec<bool>>`, `set_many(items: Vec<(RecordKey, Bytes)>) ->
DbResult<Vec<bool>>` — both already used elsewhere in this file, e.g.
`write_postings_batch`). `RecordKey` conversion from `Bytes` is `.into()`
(see `write_postings_batch`'s existing pattern).

## Part 2 — Phase C: catch-up loop (`TableManager`)

Add to `crates/shamir-engine/src/table/table_manager_index_mgmt.rs`, near
`phase_b_a_backfill`:

```rust
const CATCHUP_ITERATION_CAP: usize = 10; // RFC v3 §2.4/§6.2 — conservative
                                          // fixed cap, no tunables precedent
                                          // for this yet; local const is fine.

pub(crate) async fn phase_c_d_catchup_and_publish(
    &self,
    name_interned: u64,
    phase_ba: PhaseBAResult,
) -> DbResult<()> {
    let PhaseBAResult { guard, pin } = phase_ba;

    // ── Phase C: barrier-free catch-up loop ─────────────────────────────
    for _ in 0..CATCHUP_ITERATION_CAP {
        let dirty = self.index_manager.drain_dirty_set(name_interned);
        if dirty.is_empty() {
            break;
        }
        self.apply_catchup_for_ids(name_interned, &dirty, pin).await?;
    }

    // ── Phase D: short publish barrier ──────────────────────────────────
    let (_barrier, _uwl_guard) = self
        .begin_write_barrier(crate::index::write_barrier_flags::REGULAR_INDEX_CREATE)
        .await;

    // Final residual — whatever accumulated since the loop above's last
    // drain. Bounded by construction (the loop only exits on empty or cap).
    let final_dirty = self.index_manager.drain_dirty_set(name_interned);
    if !final_dirty.is_empty() {
        self.apply_catchup_for_ids(name_interned, &final_dirty, pin).await?;
    }

    // Flip Building -> Ready + persist — mirror index_manager.rs:1645-1673
    // (create_index_from_stream's Phase 3) EXACTLY: flip in-memory first,
    // then save_index_info(), matching the existing publish-then-persist
    // ordering invariant (F-72/#899) documented there. Add a
    // pub(crate)/pub method on IndexManager if the flip+persist isn't
    // already exposed as a callable unit — check first (grep
    // `index_manager.rs` for how `register_index_at_building`'s sibling
    // "flip to Ready" step is structured; #1087 already added
    // `register_index_at_building` for the mirror-image Phase 1 step, this
    // needs the Phase 3 equivalent, e.g. `flip_to_ready(name_interned)`).

    self.index_manager.clear_build_in_flight(name_interned);

    drop(guard); // release the pin — Phase C/D's last use of get_at(pin) was above.
    // _barrier / _uwl_guard drop via RAII at function end.

    Ok(())
}

/// Shared by Phase C's loop and Phase D's final residual: batched
/// pin-vs-current read for `ids`, then one `apply_catchup_batch` call.
async fn apply_catchup_for_ids(
    &self,
    name_interned: u64,
    ids: &[RecordId],
    pin: u64,
) -> DbResult<()> {
    let mvcc = self.mvcc_store().ok_or_else(|| {
        shamir_storage::error::DbError::Internal(
            "apply_catchup_for_ids: mvcc_store unavailable mid-catchup".to_string(),
        )
    })?;

    let keys: Vec<bytes::Bytes> = ids.iter().map(|id| id.to_bytes()).collect();
    let at_pin = mvcc.get_at_many(&keys, pin).await?;
    let at_now = self.get_many(ids).await?; // TableManager::get_many, already
                                             // decodes to InnerValue (table_manager_crud.rs:607)

    let mut deltas = Vec::with_capacity(ids.len());
    for i in 0..ids.len() {
        let old_value = at_pin[i]
            .as_ref()
            .map(|bytes| shamir_types::types::value::InnerValue::from_bytes(bytes))
            .transpose()
            .map_err(|e| {
                shamir_storage::error::DbError::Internal(format!(
                    "Phase C: failed to decode pin-time value: {e}"
                ))
            })?;
        deltas.push((ids[i], old_value, at_now[i].clone()));
    }

    self.index_manager
        .apply_catchup_batch(name_interned, deltas)
        .await
}
```

This is scaffolding, not gospel — check every method name/signature
against the actual code before using it (`get_many`'s exact return type is
`DbResult<Vec<Option<InnerValue>>>`, confirmed at
`table_manager_crud.rs:607`; `MvccStore::get_at_many`'s signature is at
`crates/shamir-tx/src/mvcc_store/mod.rs:1139`, returns
`DbResult<Vec<Option<Bytes>>>` — raw bytes, needs `InnerValue::from_bytes`
decode as shown above). Adjust as needed; the important invariants are:
(1) pin-time read uses `get_at_many(.., pin)`, NOT current-version reads;
(2) current-time read uses the ordinary `get_many` (or equivalent);
(3) the diff goes through `apply_catchup_batch`, never through
`plan_record_created`/`updated`/`deleted`; (4) the `SnapshotGuard` is held
until AFTER Phase D's final residual is applied, then dropped.

**Flip-to-Ready helper.** Check whether `IndexManager` already exposes a
callable "flip this def from Building to Ready + persist" unit, or whether
you need to add one (mirroring `index_manager.rs:1645-1673`'s inline logic
inside `create_index_from_stream`, adapted to a small
`pub(crate) async fn flip_to_ready(&self, name_interned: u64) -> DbResult<()>`
method reusable by both the existing inline call site and this new Phase D
path — REFACTOR the existing inline block to call the new method rather
than duplicating the logic, so there is exactly one flip-to-Ready
implementation).

**Invariant that must not break:** `Building` indexes are already
planner-invisible (`doctor.rs:97-101`). Phase D's flip is the ONLY place
`Ready` gets set for this build. If satisfying this requires touching the
planner, STOP and report as a finding — do not modify the planner (out of
scope).

## Tests (TDD) — write these BEFORE the implementation, in a new file
`crates/shamir-engine/src/table/tests/p1088_phase_c_d_tests.rs` (wire it in
via `tests/mod.rs`, `pub mod p1088_phase_c_d_tests;`)

1. **No concurrent writes.** Empty dirty-set immediately after
   `phase_b_a_backfill` → Phase C converges on its first empty drain, Phase D
   flips to `Ready`. Postings match exactly what Phase B+A wrote (no
   changes). Assert `IndexState::Ready` and that ordinary reads through the
   index return the expected rows.

2. **Insert during the window.** A row created AFTER the pin (so Phase A
   never saw it) lands in the dirty-set via the live write hook. Phase C's
   `old_value` (pin-time read) is `None`, `new_value` is `Some` → a new
   posting is written. Assert the new row is queryable through the index
   after Phase D.

3. **Update-to-different-value during the window — THE case v2 got wrong.**
   A row that EXISTED at the pin (Phase A already wrote its posting) is
   updated to a DIFFERENT indexed value during Phase A/B's window (use the
   pause-hook seam from `#1087`'s test file, same pattern as
   `p1087_phase_b_a_concurrent_write_captured_in_dirty_set`, or drive the
   write after `phase_b_a_backfill` returns but before calling
   `phase_c_d_catchup_and_publish` — either window exercises the same dirty-
   set path). Assert AFTER Phase D: (a) the OLD indexed value no longer
   matches this record (querying by the old value does NOT return this
   record — this is the exact bug the RFC v3 fix closes, so this assertion
   must be present and must have failed before Part 1's fix), (b) the NEW
   indexed value DOES match this record.

4. **Delete during the window.** A row that existed at the pin is deleted
   during the window. Assert AFTER Phase D: querying by its old indexed
   value returns nothing (no orphaned posting) and the record itself is
   gone.

5. **Hard iteration cap.** Simulate sustained dirty-set growth (e.g. a test
   hook or a loop that keeps writing new dirty records faster than
   `CATCHUP_ITERATION_CAP` iterations can drain) forces the loop to exit via
   the cap rather than an empty drain. Assert Phase D still correctly
   applies the final residual and flips to `Ready` — no data is silently
   dropped, just deferred to the (bounded) barrier-held final apply.

6. **Post-Phase-D normal writes.** After the index is `Ready`, an ordinary
   `plan_record_created`-driven write goes straight to `SetPosting` again
   (registry cleared, `is_build_in_flight` now `false`) — confirm via
   `index_manager_ref().is_build_in_flight(name_interned) == false` and a
   fresh insert being immediately queryable without any catch-up step.

## Gate before you report done

```
cargo check -p shamir-index --lib
cargo check -p shamir-engine --lib
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-index
./scripts/test.sh -p shamir-engine
```

Report exactly which tests you wrote, their pass/fail status individually
(not just a summary count), and any deviation from this brief with your
reasoning. If test 3 (update-to-different-value) or test 4 (delete) fail
even once during development, do not weaken the assertion to make them
pass — that is the exact bug this task exists to fix; report honestly if
you get stuck.
