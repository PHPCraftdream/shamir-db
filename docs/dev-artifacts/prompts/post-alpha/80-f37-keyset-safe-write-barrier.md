# Brief for F-37 (#845, P0) — `keyset_safe` activation must use the same write barrier as unique-index DDL

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

A readonly review (`docs/dev-artifacts/research/2026-07-27-new-wave-readonly-review.md`,
finding P0-3) found a real writer race in F-17's `keyset_safe` proof.
**Read `crates/shamir-db/src/shamir_db/execute/admin_schema.rs`'s
`stamp_keyset_safe` function (~line 296-341) and `handle_set_table_schema`
(~line 344 onward) in full first** — this brief describes the exact gap in
that code, not a hypothetical.

`stamp_keyset_safe` reads `table_handle.count().await? == 0` and, for any
genuinely new/type-changed schema rule, sets `keyset_safe = true` iff the
table was empty at that read. `handle_set_table_schema` wraps the whole
DDL in `lock_schema_rmw` (`admin_schema.rs:84-97`) — but **confirmed by
reading the code**: `lock_schema_rmw` acquires a lock keyed only by
`(db, repo, table)` in `shamir.admin_user_locks()`, a map that ONLY schema
DDL handlers touch. It does **not** intersect with any lock a plain
INSERT/UPDATE/write path acquires — those go through
`TableManager::insert`/`set`/etc. in
`crates/shamir-engine/src/table/table_manager_crud.rs`, which know nothing
about `admin_user_locks`.

**The race**: (1) table is empty; (2) DDL reads `count() == 0`, decides
`keyset_safe = true` for a new rule; (3) BEFORE the DDL persists+activates,
a concurrent INSERT/UPDATE lands (fully legal — nothing blocks it) writing
a value under whatever schema/no-schema was active a moment ago, possibly
of a type incompatible with the about-to-be-activated rule; (4) the DDL
persists and activates the new schema with `keyset_safe = true` anyway;
(5) the cursor gate (`crates/shamir-server/src/db_handler/cursor_handlers.rs:418-493`)
now trusts `keyset_safe = true` for a table whose full row history was
never actually proven homogeneous — the schema-typed keyset gate F-17 built
can silently misbehave against that one racy row.

## The existing pattern to mirror

This EXACT class of problem — "a DDL sequence needs writers to be blocked
for its snapshot→persist→activate window, but only for the tables/rules
that actually need it" — is already solved for unique-index creation.
**Read this in full before writing any code**:

- `TableManager.unique_write_lock: Arc<tokio::sync::Mutex<()>>`
  (`crates/shamir-engine/src/table/table_manager.rs:49`, public accessor
  `unique_write_lock()` at line 526).
- `TableManager.index2_create_barrier: Arc<std::sync::atomic::AtomicBool>`
  (line 63) — set `true` (Release, under `unique_write_lock`) for the
  duration of a `create_index_v2` backfill→register sequence, so every
  concurrent writer ALSO acquires `unique_write_lock` during that window,
  even for tables with no legacy unique index.
- `TableManager::needs_write_barrier()` (line 551-580) — the O(1) predicate
  every writer consults: `self.index_manager.has_unique_indexes() ||
  self.index2_create_barrier.load(Acquire)`.
- `table_manager_crud.rs`'s `insert`/`insert_many_returning_version`/
  `delete_returning_version`/`set` (4 call sites, lines ~90/180/323/390) —
  each conditionally acquires `unique_write_lock` iff `needs_write_barrier()`
  is true: `let _guard = if self.needs_write_barrier() { Some(self.unique_write_lock.lock().await) } else { None };`
- `table_manager_index_mgmt.rs`'s `create_unique_index`/`create_index_v2`
  (the actual barrier-setters) — hold `unique_write_lock` across their
  ENTIRE snapshot→backfill→register sequence, and set/clear
  `index2_create_barrier` (Release-ordered) while holding it.

## What to build

### 1. A new barrier flag for schema activation

Add a parallel `AtomicBool` field to `TableManager`, e.g.
`schema_activation_barrier: Arc<std::sync::atomic::AtomicBool>` (same
shape/Arc-sharing/clone-propagation as `index2_create_barrier` — check
every place `index2_create_barrier` is threaded through `TableManager`'s
`Clone` impl, `new`/`create` constructors, and mirror each one for the new
field).

Extend `needs_write_barrier()`'s OR-condition to also check this new flag,
so a writer acquires `unique_write_lock` whenever EITHER a legacy unique
index exists, OR an index2 create is in flight, OR a schema activation
that needs the barrier is in flight.

Add public accessors on `TableManager` for `shamir-db`'s DDL handler to
drive this flag and the shared lock from outside the engine crate — mirror
`unique_write_lock()`'s existing public-accessor style exactly (e.g.
`pub fn set_schema_activation_barrier(&self, on: bool)` using the same
`Ordering::Release` the index2 create path uses, or whatever shape fits
this codebase's existing conventions best — look at how `index2_create_barrier`
is set/cleared for the exact ordering to replicate).

### 2. Wire the barrier into `admin_schema.rs`'s DDL sequence

In `handle_set_table_schema` (and `AddSchemaRule`/`RemoveSchemaRule` if they
ALSO call `stamp_keyset_safe` or an equivalent count-based proof — check
this; the brief's context section only quotes `handle_set_table_schema`,
confirm whether the other two DDL ops share this exposure before deciding
scope), hold `TableManager::unique_write_lock()` across the ENTIRE
"read count() -> stamp_keyset_safe -> persist catalogue -> activate
validator" sequence, with the barrier flag set for that same window:

1. Acquire `unique_write_lock` (via the table handle's accessor).
2. Set `schema_activation_barrier = true` (Release) while holding it —
   NOW any writer landing after this point blocks on `unique_write_lock`
   until this DDL releases it, so `stamp_keyset_safe`'s `count()==0` read
   is genuinely a snapshot no concurrent write can invalidate before this
   DDL commits+activates.
3. Do the existing sequence: `stamp_keyset_safe` → persist catalogue →
   `compile_table_schema`/activate.
4. Clear the barrier flag and release the lock (in that order, or whatever
   order mirrors the existing unique-index DDL's own barrier-clear
   sequence — check it precisely rather than guessing) — on EVERY exit
   path, success or error (use a guard/RAII pattern if the existing
   `unique_write_lock` call sites already establish one, to avoid a
   forgotten-unlock bug on an early error return; check
   `create_unique_index`'s existing error-path handling for the pattern to
   copy).

This closes the race precisely: any INSERT/UPDATE that would otherwise land
between the `count()==0` read and the schema's activation now blocks on
`unique_write_lock` until the DDL is fully committed+activated (at which
point it either sees the new schema's validation rules applied to itself,
or — if the write started before this DDL's lock acquisition and merely
queued behind it — it's now provably ordered AFTER the point the count was
proven, meaning the count proof and this write's presence are correctly
ordered, no longer racing).

### 3. Do NOT extend `keyset_safe` eligibility beyond `count()==0`

The review separately suggests (as an "even better" enhancement, not part
of this P0's scope) doing a full snapshot-validation scan of all existing
rows to allow `keyset_safe = true` for a non-empty table. **Do not do
this as part of F-37** — this task is scoped to closing the RACE around
the existing `count()==0` proof, not expanding what counts as proof. File
a note in your summary if you think a follow-up task for that enhancement
is warranted, but do not implement it here.

## Tests — MANDATORY, in the same commit

1. A deterministic paused-DDL/concurrent-write test: pause the DDL's
   `stamp_keyset_safe`/persist sequence at a controllable point AFTER the
   `count()==0` read but BEFORE the barrier would otherwise release
   (inject a gate the same way this session's other race-closure tests do
   — check `fk_race_closure_tests.rs`/`fk_reverse_cache_race_tests.rs` for
   the established deterministic-injection style in this codebase, prefer
   a `Notify`-based handshake over any sleep-based timing), attempt a
   concurrent INSERT from another task, and assert it BLOCKS until the DDL
   finishes (does not interleave) — the core proof that the write barrier
   actually serializes against writers now, not just against other DDL.
2. A regression: the existing non-racing case (DDL completes with no
   concurrent writer) still activates `keyset_safe = true` for a genuinely
   empty table exactly as before — no functional regression to the
   already-correct non-racing path.
3. Check whether an existing F-17 test suite
   (`crates/shamir-db`'s keyset/schema tests — grep for `keyset_safe`)
   already covers the non-racing case; if so, just confirm it still
   passes rather than duplicating it.

## Constraints

- Reuse `unique_write_lock` — do NOT introduce a second, independent
  per-table mutex for this. The whole point of mirroring the unique-index
  pattern is that writers already know to check ONE barrier predicate; a
  second independent lock would require writers to acquire two locks and
  reason about their ordering, a much bigger and riskier change.
- Do NOT touch `index2_create_barrier`'s own semantics — add a sibling
  flag, don't overload the existing one (they represent different
  in-flight conditions and should be independently settable/clearable).
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine -p shamir-db -- --check` and
  `cargo clippy -p shamir-engine -p shamir-db --all-targets -- -D warnings`
  must both be clean.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -p shamir-db -- --check
cargo clippy -p shamir-engine -p shamir-db --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -- barrier
./scripts/test.sh -p shamir-db -- keyset
./scripts/test.sh -p shamir-engine --full
./scripts/test.sh -p shamir-db --full
```
