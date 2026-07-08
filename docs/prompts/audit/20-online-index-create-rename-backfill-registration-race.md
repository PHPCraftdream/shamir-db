Task: MEDIUM-HIGH concurrency — online CREATE/RENAME INDEX loses a
concurrent write in the window between the backfill snapshot and index
definition registration; unique-index RENAME has an additional window
where duplicate values can slip through and permanently destroy the
unique index (audit finding A9,
`docs/audits/2026-07-06-concurrency-engine.md`).

## Where

- `crates/shamir-index/src/legacy/index_manager.rs`,
  `create_index_from_records` (~line 241-279): builds ALL posting
  entries from a snapshot of records (`records: Vec<(RecordId,
  InnerValue)>`, collected by the caller BEFORE this function runs —
  see `collect_all_current_records` at the caller), writes them via
  `info_store.set_many` (line 262-264), and only THEN registers the
  definition: `self.indexes.add_index(index_def)` (line 269). Any
  concurrent write that lands AFTER the snapshot was taken but BEFORE
  `add_index` runs is invisible to the write-hook that decides whether
  to maintain a posting for a given index (the hook checks the
  registered index set, which doesn't yet include this index) — so
  that record is silently never indexed. It stays unindexed forever
  (until a manual `repair()`), because after `add_index` runs, the
  write-hook only maintains postings for writes that happen AFTER
  registration; it has no mechanism to notice a gap.
- `crates/shamir-index/src/legacy/index_manager_unique.rs`,
  `create_unique_index_from_records` (~line 368, confirm current
  line): same drop→backfill→register ordering issue, but WORSE for
  unique indexes — see the "Why this is bad" interleaving below for
  the rename case specifically, where the SAME structural bug produces
  a harder failure (the whole index build fails, not just one record
  missed).
- `crates/shamir-engine/src/table/table_manager_index_mgmt.rs`,
  `rename_index` (~line 405-510, confirm current lines — this file may
  have shifted since 2026-07-06):
  - Regular (hash) index rename (~line 443-458): `drop_index(old_id)`
    THEN `create_index(new_name, ...)` — between the drop and the
    create-from-snapshot-then-register, writes are invisible to BOTH
    the old index (dropped) and the new one (not yet registered) —
    same class of gap as `create_index_from_records`, but now the
    WHOLE index (not just a fresh CREATE) has a window where writes
    are silently unindexed.
  - Unique index rename (~line 461-476): `drop_unique_index(old_id)`
    THEN `create_unique_index(new_name, ...)`. In the drop→create
    window, ordinary non-unique writes (no uniqueness check active,
    since the unique index is dropped) can insert DUPLICATE values for
    the field that's supposed to be unique. When
    `create_unique_index_from_records` later tries to backfill+register
    the new unique index from a snapshot that (depending on timing) may
    now contain those duplicates, the unique-constraint validation
    during backfill fails → the whole rename aborts (or worse, leaves
    the table with NO unique index at all if the failure happens after
    the drop but the create never successfully completes) — the table
    that started with a working unique constraint can end up with NONE.
  - `rekey_sorted_prefix` (~line 524-560+, confirm current end line):
    scans and copies posting entries under the old-id prefix to the
    new-id prefix WITHOUT any write-blocking — a write landing mid-scan
    could be copied under the OLD id (already scanned past) and never
    appear under the new id, or vice versa depending on scan order
    (same class of lost-write race, applied to the rekey operation
    itself rather than to backfill).

## Why this is MEDIUM-HIGH

**Concrete interleaving from the audit (regular CREATE INDEX):**
1. Admin runs `CREATE INDEX idx ON t (field)`. The table manager calls
   `collect_all_current_records()` — a snapshot of all records AT THIS
   MOMENT (the definition is NOT yet registered).
2. Concurrently, a writer inserts record R with `field = "x"`. The
   write-hook checks the registered index set — `idx` is not there yet
   — so no posting is created for R under `idx`.
3. The admin's `create_index_from_records` finishes building postings
   from the (R-less) snapshot, writes them, and THEN calls
   `self.indexes.add_index(index_def)` — `idx` is now registered.
4. **R is now permanently missing from `idx`.** Any query using `idx`
   to filter/lookup on `field = "x"` will never find R, until an
   explicit `repair()` rebuild. This is silent — no error, no log
   (beyond the normal "Created index" info line), and no test would
   catch it without deliberately racing a write against a CREATE INDEX.

**Concrete interleaving (unique RENAME, worse case):**
1. Table `t` has a working unique index `uniq` on `email`. Admin runs
   `RENAME INDEX uniq TO uniq2`.
2. `drop_unique_index(old_id)` runs — `uniq`'s uniqueness enforcement
   is now GONE.
3. In the window before `create_unique_index(new_name, ...)`
   registers the new definition, TWO writers insert records with the
   SAME `email` value — no unique check blocks either (the index is
   dropped).
4. `create_unique_index_from_records` backfills from a snapshot that
   includes both duplicate records, hits the uniqueness violation
   during backfill validation, and the whole `create_unique_index`
   call fails.
5. `rename_index` propagates the error. **The table now has NO unique
   index on `email` at all** — the old one was dropped in step 2 and
   the new one never successfully finished creating. The uniqueness
   guarantee the schema was supposed to enforce is silently gone until
   an admin notices and manually re-runs index creation (after first
   deduplicating the offending rows by hand).

## Fix

Per the audit's fix sketch — two complementary approaches, pick
whichever fits each code path (confirm your choice in the report):

**Option A — register-definition-first, backfill-second (preferred for
plain CREATE and for the non-unique parts of rename):**
1. Register the index definition (`self.indexes.add_index(index_def)`
   or equivalent) BEFORE taking the backfill snapshot / before writing
   any posting entries. Once registered, the write-hook starts
   maintaining postings for ALL new/concurrent writes against this
   index immediately.
2. Take the backfill snapshot and write postings for the
   pre-registration records AFTERWARD. Because postings are idempotent
   (the posting key is derived deterministically from
   `(index_key, record_id)` — check `build_posting_key`/
   `build_index_key_from_record` to confirm this holds), a record that
   was BOTH captured in the snapshot AND concurrently indexed live by
   the write-hook (a narrow overlap window right at registration time)
   produces the SAME posting key twice — writing it twice is a safe,
   idempotent no-op, not a duplicate/corruption.
3. This inverts the risk: instead of "a write in the gap is silently
   lost", the (much narrower, and safe-by-idempotency) risk becomes "a
   write right at the registration boundary is indexed twice
   redundantly" — which is harmless.

**Option B — hold a table-wide write barrier for the whole DDL
duration (simpler, more disruptive — use only where Option A's
idempotent-double-write reasoning doesn't cleanly apply, e.g. the
unique-index uniqueness VALIDATION during backfill, which is NOT safe
to just redo/double-apply):**
1. For the UNIQUE index specifically (create AND rename), hold the
   existing `unique_write_lock` (used elsewhere in the commit pipeline
   per HIGH-A, cross-reference `pre_commit.rs`'s Phase 2.5 acquisition
   pattern if useful context) across the ENTIRE drop→backfill→register
   sequence for a unique-index rename, so no writer can insert ANY row
   (let alone a duplicate) while the unique index is between its old
   and new registered states. This closes the "duplicates slip through
   the gap" failure mode at its root (no gap = no duplicates possible),
   at the cost of blocking writers table-wide for the rename's
   duration (acceptable for a low-frequency DDL operation).
2. For `rekey_sorted_prefix`: either apply the same write-barrier
   approach for its duration, or (if barrier-free is preferred for
   performance) make the rekey itself resilient to concurrent writes by
   scanning old-id postings and ALSO re-checking for any not-yet-seen
   entries after the initial scan pass completes (a short, bounded
   "settle" re-scan) — pick whichever is simpler given the existing
   code shape; justify the choice in your report.

Apply **Option A** to `create_index_from_records` (plain CREATE INDEX,
non-unique) and to the non-unique halves of `rename_index`. Apply
**Option B** (or an equivalent write-barrier) specifically to the
UNIQUE index create/rename path, since a duplicate slipping through
during backfill is NOT safely idempotent-double-writable — it's a
correctness violation of the uniqueness guarantee itself, not a
harmless double-write.

Do NOT change `rekey_sorted_prefix`'s physical key-rewrite logic itself
(the byte-level prefix substitution) — only the concurrency envelope
around when it's safe to run relative to concurrent writers.

## TDD requirement

1. **Red**: write `#[tokio::test]`s (check
   `crates/shamir-index/src/legacy/tests/` and
   `crates/shamir-engine/src/table/tests/` for the existing test module
   layout covering `create_index_from_records`/`rename_index`, and
   follow established patterns) that:
   - Reproduce the plain-CREATE lost-write race: spawn a concurrent
     writer racing `create_index_from_records`'s snapshot-then-register
     window (you may need to inject a controlled delay/yield point, or
     structure the test to directly call the lower-level pieces in the
     "wrong" order to deterministically force the race rather than
     relying on real scheduler timing — use your judgment on the
     cleanest deterministic reproduction given the existing code's
     testability). Assert that, post-fix, a write landing in this
     window IS indexed (present in postings after registration) — this
     should FAIL pre-fix (record silently missing) and PASS post-fix.
   - Reproduce the unique-rename duplicate-slip-through race
     (deterministically: drop the unique index, insert two duplicate
     rows, THEN attempt create_unique_index_from_records / the full
     rename) and assert the fix's chosen approach (write-barrier)
     prevents the duplicates from ever being insertable in the first
     place OR fails cleanly leaving the OLD unique index still in
     place (whichever invariant your Option B implementation actually
     provides — pick the achievable, correct one and assert it
     precisely; do not assert an invariant the design doesn't actually
     guarantee).
   - A regression test confirming plain (non-racing) CREATE INDEX and
     RENAME INDEX still work correctly end-to-end (existing behavior
     for the common case).
2. **Green**: apply the fix(es).
3. Confirm existing index-management tests
   (create/drop/rename/rebuild/repair) still pass.

## Test scope command

```
./scripts/test.sh -p shamir-index
./scripts/test.sh -p shamir-engine -- index
```

## Gate (must be clean before finishing)

```
cargo fmt -p shamir-index -p shamir-engine -- --check
cargo clippy -p shamir-index -p shamir-engine --all-targets -- -D warnings
```

If clippy flags PRE-EXISTING lints in code you did not touch, do not
fix them here — note them in your final report instead.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

When done, report exactly:
- Which approach (Option A register-first, Option B write-barrier, or
  a hybrid) was applied to each of: plain CREATE INDEX
  (`create_index_from_records`), regular-index RENAME, unique-index
  CREATE (`create_unique_index_from_records`), unique-index RENAME,
  and `rekey_sorted_prefix` — with a one-line justification each.
- Confirmation that posting writes are genuinely idempotent (cite the
  posting-key derivation) wherever Option A's "safe double-write"
  reasoning is relied upon.
- The failing-then-passing test evidence for both the plain-CREATE
  lost-write race and the unique-rename duplicate-slip-through race.
- Confirmation existing index create/drop/rename/repair tests still
  pass.
- Full test/gate results (exact commands + pass/fail).
