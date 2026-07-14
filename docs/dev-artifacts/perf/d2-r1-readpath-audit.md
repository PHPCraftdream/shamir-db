# D2 / R1 — Read-path coherence audit (overlay precondition)

**Scope.** Prove the invariant required before inserting a versioned
in-memory OVERLAY into `MvccStore::resolve_read` / `get_at`:

> **Every read of committed user-DATA row values must resolve through
> `MvccStore::get_at` → `resolve_read` (or its siblings `get_current` /
> `current_stream`, which read the same `history` log).** Any reader that
> reaches committed row bytes by directly hitting `Store::get` /
> `transact` / `iter_stream` / range-scan on `history` / `base` /
> `data_store()` BYPASSES the overlay and would miss not-yet-drained
> versions after D2.

**Method.** Grep'd every data-read primitive (`.get(`, `.get_at(`,
`.get_current(`, `current_stream`, `iter_stream`, `data_store()`,
`history_store()`, `.transact(`, `.base()`) across `shamir-engine`,
`shamir-tx`, `shamir-db`. Classified each production site.

**READ-ONLY audit — no code changed, no tests run.**

---

## Canonical readers (the overlay insertion point — all OK)

All live in `crates/shamir-tx/src/mvcc_store/mod.rs`:

- `get_at` (:377) → `resolve_read` (:215) — snapshot point read.
- `get_current` (:398) — current point read (has R3 pre-floor fallback to
  `get_at`).
- `current_stream` (:435) — current full-scan stream.
- `version_of` (:508) / `live_version` (:517) — version-only, no value.

D2 overlay goes into `resolve_read`; `get_current` and `current_stream`
must be made overlay-aware in the same slice (they read the log directly,
not via `resolve_read`).

---

## Site table (production read sites)

| site (file:line) | what it reads | path | verdict | D2 |
|---|---|---|---|---|
| `engine/table/table_manager_crud.rs:502` `get` | row value (point) | `mvcc.get_current` | OK | overlay via get_current |
| `engine/table/table_manager_crud.rs:523` `get_many` | row values (point) | `mvcc.get_current` | OK | overlay via get_current |
| `engine/table/table_manager_crud.rs:508` else-branch | row value | `table.get` (non-MVCC table) | EXEMPT (no MvccStore attached) | n/a |
| `engine/table/table_manager_streaming.rs:35,73` `list_stream*` | row values (scan) | `mvcc.current_stream` | OK | overlay via current_stream |
| `engine/table/table_manager_streaming.rs:392` `read_one_tx` | row value (tx point) | `mvcc.get_at` (after staged_op overlay probe) | OK | overlay via get_at |
| `engine/table/read_temporal.rs:97` `read_as_of` | as-of value | `mvcc.get_at` | OK | overlay via get_at |
| `engine/table/read_temporal.rs:278` `read_history` | timeline values | `mvcc.history_of` (log) | OK (historical, see note) | overlay irrelevant (asks log timeline) |
| `engine/table/read_index_scan.rs:143,217,329,394` | row values (index fetch) | `self.get_many` → `get_current` | OK | inherits get_current |
| `engine/table/read_index_scan.rs:107-129` covering index-only | row value FROM POSTING | posting + `mvcc.live_version` freshness gate; fallback `get_many` | OK-with-caveat (see Index section) | needs overlay-aware live_version check |
| `engine/table/read_exec.rs:502,619,710` `read_*` full-scan | row values (scan) | `list_stream*` → `current_stream` | OK | inherits current_stream |
| `engine/migration/coordinator.rs:158,326` snapshot/drain | row values (scan) | `mvcc.current_stream` | OK | inherits current_stream |
| `engine/table/table_manager_replication.rs:190` `bulk_populate_index2` | row values (scan) | `self.list_stream` → current_stream | OK | inherits current_stream |
| `engine/table/doctor.rs:96,221,235` | row values (scan) | `self.list_stream` → current_stream | OK | inherits current_stream |

---

## НАРУШЕНИЯ (bypass of committed user-data) — **NONE on a live read path**

No production reader of committed user-data row values bypasses the
`get_at` / `get_current` / `current_stream` seam. **Count of live
bypass violations: 0.**

The candidates examined and cleared:

1. **`engine/tx/commit_phases.rs:376` `apply_data_batch` → `base.transact(ops)`**
   (the anchor "suspect"). This is a **WRITE** path (Phase 5a
   materialize), not a read, and the `base.transact` arm fires **only for
   non-MVCC tables** (`per_table_mvcc` miss). It writes durable data for
   tables that have no `MvccStore` and hence no overlay. **Not a read
   bypass.** It IS the place D2 reroutes the durable materialize from, but
   that is the write side, out of R1's scope.

2. **`tx/staging_store.rs:151` `StagingStore::get` → `self.base.get(k)`**
   (read-through that hits `data_store()` directly). This would be a
   committed-data read bypass IF on a live path — but the production tx
   point-read path (`read_one_tx`, streaming.rs:371-408) uses
   `staging.staged_op()` (overlay-only probe, never falls through) and
   then `mvcc.get_at`. `StagingStore::get` is called **only from tests +
   one bench** (`tx_overhead.rs:100`, `staging_store_tests.rs`,
   `tx_context_tests.rs:94`). **Latent foot-gun, not a live bypass.**
   *Recommendation:* leave as-is for D2, but add a doc-note that
   `StagingStore::get`'s base fall-through is non-overlay-aware and must
   not be wired into any future read path (or route it through the seam).

3. **`tx/recovery.rs:55,85` `data_store().set/remove`** and
   **`tx/recovery.rs:381` `history_store().set`** — recovery WRITE
   replay, not reads. EXEMPT.

4. **`table_manager_replication.rs:152` `data_store().iter_stream`**
   inside `seed_log_from_data_store` — a migration-cutover WRITE-seeding
   read of the RAW data_store, used to push raw migration-copied bytes
   INTO the log via `set_versioned_many`. This reads `data_store()`
   directly, but its purpose is precisely to MATERIALIZE the log from
   data_store at cutover (a one-shot seeding op, not a user read). EXEMPT
   (bootstrap/migration plumbing). After D2 the values it seeds land in
   the log normally; nothing to change.

---

## EXEMPT (metadata / recovery / durability — not committed user-data reads)

- `db/shamir_db/system_store.rs:*` `data_store().flush()` (×17) — durability
  flush, no read.
- `engine/repo/repo_instance.rs:879` `data_store().flush()` — flush.
- `engine/table/table_manager_buffer.rs:26`, `table_manager.rs:191,215,356`
  `data_store()` — buffer-config / store-handle plumbing, not value reads.
- `db/shamir_db/execute/admin_migration.rs:86,112` — clones the
  `data_store()` Arc to hand to the migration coordinator, which reads via
  `current_stream` (:256 routes to `seed_log_from_data_store`). Plumbing.
- `tx/mvcc_store/mvcc_gc.rs:85,208,294` `current_version` + GC history
  range scans — GC/vacuum over the version log, not a user read.
- `tx/mvcc_store/mod.rs:541` `lookup_ts` / `record_ts` — per-version commit
  timestamp metadata (TS_TAG keyspace), not row data.
- `engine/tx/recovery.rs:*` — WAL replay writes + version-cache seeding.
- Interner / counter / index-posting `info_store` reads — metadata, never
  the row value (interner replication: `table_manager_replication.rs:27,46`).
- `tx/staging_store.rs:151` — see НАРУШЕНИЯ #2 (test/bench-only).

---

## Index / HNSW read path (separate layer — needs D2 work)

**Covering index-only read** (`read_index_scan.rs:107-150`,
`read_sorted_index_scan`): for a covering index with no residual /
group-by / order-by, the row value is reconstructed **from the index
posting itself**, NOT fetched from `history`. Freshness is validated by
`mvcc.live_version(&id.to_bytes())`: the posting carries the version it was
written at (`decode_covering_projection` → `(v, proj)`), and the row is
served from the posting **only if** `live_version` is `None` (no in-process
mutation) **or** equals `v` (posting is current). On mismatch / corrupt
posting / `None`-with-deleted it falls back to `self.get_many` → `get_current`.

- This is **NOT a data-store bypass**: it never reads `history`/`data_store`
  for the value; it reads a derived projection gated by the version cell.
- **D2 implication:** `live_version` reads `cells[key].version` — the same
  cell the overlay/floor advance. After D2, a committed-but-not-yet-drained
  version bumps the cell (commit publishes the cell version before/at ack),
  so the covering check would see `hwm = new_v` while the posting carries an
  older `v` → version mismatch → fallback to `get_current`, which the
  overlay covers. **Provided the cell version is published at commit-ack
  (not at drain).** This must hold in D2: the freshness gate is correct only
  if `cells[key].version` reflects the committed (ack'd) version, not the
  durable-drained one. Action: D2 must keep `publish_cell` / cell-version
  advance on the ACK path (it already is, per `apply_committed_ops` /
  `set_versioned`), and the covering fallback then routes through the
  overlay-aware `get_current`. **No data-bypass; an overlay-aware
  `get_current` fallback is the only requirement.**

**HNSW / vector read:** vectors are a DERIVED accelerator, rebuilt from the
data store on open (`commit_phases.rs:287` `promote_vectors` doc). Vector
search returns RecordIds; the actual row values are then materialized via
the normal `get_many` / `get_current` seam. No value-bearing bypass.

---

## Verdict — GO for overlay insertion into `resolve_read`

The read-path is **clean** for inserting a versioned overlay into
`MvccStore::resolve_read`. Every live reader of committed user-data row
values already funnels through `get_at` (→ `resolve_read`),
`get_current`, or `current_stream`. There are **zero live bypass
violations**.

**Preconditions / co-requisites for D2 (not blockers, but must land in the
same slice):**

1. **`get_current` (mod.rs:398) and `current_stream` (mod.rs:435) read the
   `history` log directly, NOT via `resolve_read`.** Putting the overlay only
   in `resolve_read` would make point `get` (crud.rs:502) and every
   full-scan / list / migration / doctor / index-fetch path miss undrained
   versions. **All three (`resolve_read`, `get_current`, `current_stream`)
   must consult the overlay.** This is the single most important fact: the
   overlay is NOT "one insertion point" — it is three (point-snapshot,
   point-current, scan-current).

2. **Cell version must be published on commit-ACK**, so the covering
   index-only freshness gate (`live_version`) keeps falling back to the
   overlay-aware `get_current` for not-yet-drained versions. (Already the
   case today.)

3. **`StagingStore::get` (staging_store.rs:151) base fall-through is
   non-overlay-aware** — keep it test/bench-only; do not wire it into any
   read path.

No site needs to be pre-migrated under `get_at` before the overlay lands;
the seam is already universal. The work is to make the overlay visible from
all three seam entry points, not to re-route stray bypass readers (there are
none).
