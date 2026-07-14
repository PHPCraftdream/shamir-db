Task #534 — close two related gaps in `TableManager::create_index_v2`'s
index2 (fts/functional/vector) CREATE INDEX pipeline, found during G4/#528's
`@fl` review of the `backfill_index2_backend` fix.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Context (confirmed by reading the current code — re-confirm line numbers,
## they may have shifted)

`crates/shamir-engine/src/table/table_manager_index_mgmt.rs::create_index_v2`
(~line 28-266) builds a new index2 backend, then:
```rust
self.backfill_index2_backend(backend.as_ref()).await?;   // ~255
self.index2_registry.insert(backend).await?;              // ~257
crate::index2::persistence::save_index2_metadata(...).await?; // ~262
```
No lock is held across this sequence. Compare with `create_unique_index`
(~line 344-352 in the same file), which holds
`self.unique_write_lock: Arc<tokio::sync::Mutex<()>>`
(declared `crates/shamir-engine/src/table/table_manager.rs:49`, a sanctioned
`tokio::sync::Mutex` per this repo's concurrency rules — guards across
`.await`, low-frequency DDL contention) across the ENTIRE
snapshot→backfill→register sequence (see that function's own doc comment,
"audit A9 — write-barrier for unique CREATE"). This SAME lock is already
acquired by every non-tx writer (`table_manager_crud.rs`'s insert path) and
by the tx commit pipeline's Phase 2.5, so it already serializes ALL writer
classes against DDL — `create_index_v2` currently does not participate in
this barrier at all.

## Finding 1 — lost-write race (the correctness bug)

Without a write-barrier, a row written by a concurrent writer AFTER
`backfill_index2_backend`'s `list_stream` cursor has passed that row's key
position, but BEFORE `index2_registry.insert()` completes, is observed by
NEITHER the backfill (already past it) NOR the live `index2_on_insert`
write-hook (backend not registered yet) — permanently missing from the new
index until a future write happens to touch that row again.

The legacy btree path solved the identical class of gap (audit A9) via
register-first + idempotent double-write absorption — NOT portable here
as-is because `IndexKind::Fts`'s backend applies non-idempotent
`BumpFtsStats` counter-bump ops (`crates/shamir-index/src/write_ops.rs`)
via `apply_in_memory`; a naive register-first flip would double-count
stats for any row racing during backfill.

### Fix: write-barrier lock (Option B, matching `create_unique_index`'s own precedent)

Acquire `self.unique_write_lock.lock().await` at the START of
`create_index_v2` (for the index2 branch only — the `btree`/`unique`
early-return above already has its own locking via
`create_index`/`create_unique_index`, do not double-lock those), and hold
the guard across `backfill_index2_backend` → `index2_registry.insert` →
`save_index2_metadata` (see Finding 2 below for one more call inside this
same held region). This works uniformly for ALL index2 backend kinds
(fts/functional/vector) regardless of idempotency — no writer can insert
ANY row while the new backend is between "not yet registered" and
"registered", so the backfill's snapshot is guaranteed complete and
consistent. This is the same trade-off `create_unique_index` already
accepted: CREATE INDEX is a low-frequency DDL operation, so serializing it
against concurrent writers for its duration is an acceptable cost — it is
NOT free-standing new design, it is applying the exact pattern already in
this file to a class of index that didn't get it originally.

Double-check: `create_index_v2` must not already be called from within a
context that holds `unique_write_lock` (would deadlock — `tokio::sync::Mutex`
is NOT reentrant). Grep all callers of `create_index_v2` to confirm none do
today.

## Finding 2 — crash-orphan-postings window (lower severity, narrower)

`self.index2_registry.allocate_id()` (~line 64) is a plain in-memory
`AtomicU32::fetch_add` (`crates/shamir-index/src/registry.rs:29-30`) with
**no durability at allocation time** — the id only becomes durable when
`save_index2_metadata` (~line 262, the LAST step) succeeds. `IndexRegistry`'s
`next_id` is restored from persisted metadata on reopen
(`set_next_id`, `registry.rs:82-84`, called from wherever
`load_index2_metadata` is consumed at table-open time — grep for it).

So: a crash between `allocate_id()` and the final `save_index2_metadata()`
leaves postings written under an id that was NEVER persisted. On restart,
`next_id` resets to the last successfully-persisted watermark — meaning
the SAME id can be allocated again to a genuinely DIFFERENT index
definition later, which would then "inherit" the crashed attempt's stale
orphan postings (requires a value-hash collision between the old crashed
definition and the new one to actually manifest as a phantom match at
query time — narrow, but a real correctness hazard, not just wasted space).

### Fix: durably reserve the id BEFORE backfill (cheap, reuses existing machinery)

`save_index2_metadata` (`crates/shamir-index/src/persistence.rs:53-71`)
already persists `PersistedIndexes { next_id: registry.peek_next_id(),
descriptors: registry.all_descriptors().await }` in one `MetaEnvelope`
write. Call this SAME function ONCE immediately after `allocate_id()`
(~line 64-65), BEFORE `backfill_index2_backend` runs — at that point
`registry.all_descriptors()` still returns the PRE-existing descriptor set
(the new backend hasn't been inserted yet), so this call durably advances
the persisted `next_id` watermark past the newly-allocated id WITHOUT
exposing the new (not-yet-backfilled) index to anything that reads
`descriptors`. If the process then crashes mid-backfill, restart restores
`next_id` past the dead id — it can never be reallocated. The orphan
postings under that dead, never-reused id become permanently unreachable
garbage (inert, not a correctness hazard — no future index can ever collide
with an id nothing will ever allocate again). This requires ONE added call
to an already-tested function, no new persistence format.

Net effect combining both fixes: hold `unique_write_lock` across
[persist-reserved-next_id → backfill → registry.insert → final
save_index2_metadata]. The reserved-id persist happens under the SAME lock
guard (no extra lock acquisition needed) since it's all inside one held
critical section.

## Test coverage (write these — TDD)

1. **Finding 1 regression test**: a concurrent-writer test proving a row
   inserted DURING a `create_index_v2` call (fts or functional — pick
   whichever is easiest to drive concurrently in a test) is NOT lost —
   i.e. it ends up queryable via the new index afterward. Use a
   synchronization primitive (e.g. a test-only hook, or a `tokio::sync::Barrier`,
   mirroring the style of `argon2id_concurrency_cap_bounds_parallel_calls`'s
   redesign in G4/#528) to force the writer's insert to land inside the
   race window that existed before this fix — the test should FAIL against
   the pre-fix code (verify this red state mentally or by temporarily
   reverting your own lock addition) and PASS after.
2. **Finding 2 regression test**: simulate the crash-then-restart sequence —
   allocate an id, do NOT complete `save_index2_metadata`'s final call
   (simulate the crash by stopping short), then simulate the reopen path
   (`load_index2_metadata` → `set_next_id`) and assert the next `allocate_id()`
   call does NOT return the same id as the one from the interrupted attempt.
3. Existing test coverage from G4/#528
   (`trusted_pure_scalar_backs_functional_index` and siblings) must stay
   green — this is additive locking + one added persist call, not a
   behavior change to the backfill logic itself.
4. If convenient, add one direct backfill regression test each for `fts` and
   `vector` index types (only `functional` has one today per the G4/#528
   task notes) — optional, do it if it doesn't balloon scope.

## Test scope

```
./scripts/test.sh -p shamir-engine -p shamir-index
```

## Verification (lighter per-task gate, agreed this session)

```
cargo check --workspace --all-targets
./scripts/test.sh -p shamir-engine -p shamir-index
```
Do NOT run the full fmt/clippy/test --full gate — that's FINAL-GATE's (#529's) job.

## Report format

```
[Implementation] Status: fixed / scoped-down-with-followup
  > Finding 1: lock acquired where, what it's held across, confirmed no
    reentrancy/deadlock with existing callers
  > Finding 2: where the extra save_index2_metadata call was added, confirmed
    it doesn't prematurely expose the new (unbackfilled) index to readers
  > New tests added, and confirmation the Finding-1 test fails against the
    pre-fix code (how you verified this — temporarily reverting the lock,
    or reasoning through the interleaving)
[Verification]
  cargo check --workspace --all-targets: pass/fail
  ./scripts/test.sh -p shamir-engine -p shamir-index: pass/fail
```
