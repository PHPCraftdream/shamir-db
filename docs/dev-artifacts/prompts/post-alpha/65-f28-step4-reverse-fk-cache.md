# Brief for F-28 Step 4 (#831, P1) — cached per-repo reverse-FK map + O(1) role flags

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

`discover_restrict_refs` (`crates/shamir-engine/src/query/batch/
fk_restrict.rs` ~line 151-186) and `discover_action_refs`
(`crates/shamir-engine/src/query/batch/fk_actions.rs` ~line 908-950ish)
both do the SAME thing on EVERY delete/update: `resolver.resolve_repo(..)`
→ `repo.list_table_names()` → `resolver.resolve(..)` for EVERY table in
the repo → `table.collect_fk_refs()` on each. This is O(tables) per
operation, repeated at EVERY level of cascade recursion
(`plan_cascade_recursive`/`plan_cascade_for_ids` in `fk_actions.rs`).
Violates this workspace's `O(x → 0)` ideology pillar, and F-28 Step 3's
spike (`docs/dev-artifacts/research/f28-s3-mechanism-decision.md`)
recommends S3-C, which needs an O(1) per-table flag ("is this table an
FK parent with a non-`NoAction` action, or an FK child requiring a
Serializable-isolation upgrade / footprint token") — that flag is what
this task builds.

## What `collect_fk_refs()` actually depends on

`crates/shamir-engine/src/table/table_manager_validators.rs` ~line
378-399 — `collect_fk_refs()` reads `self.validator_bindings` (an
`ArcSwap`-backed list on the `TableManager`) and, for each bound
validator id, resolves it via `ValidatorRegistry::get_by_id` and calls
`.fk_refs()` on the compiled validator. So the data driving reverse-FK
discovery changes ONLY at these mutation points:
- `TableManager::add_validator_binding`/`remove_validator_binding` (bindings).
- `ValidatorRegistry::register`/`replace_artifact`/`remove` for a
  validator that has non-empty `fk_refs()` (the compiled artifact).
- Table creation/deletion (`repo.list_table_names()`'s membership).

**All of the validator-registry mutation points are already funneled
through `ShamirDb::compile_table_schema`**
(`crates/shamir-db/src/shamir_db/shamir_db/schema_management.rs`
~line 486-540ish, already touched by F-24/#817 and F-27b/#827 — read
those commits first, this task builds directly on that same function)
— it is the SOLE place a table's validator binding + registry entry
change together, for BOTH the success path and the (already-correct,
post-F-27b) rollback-on-failure path. This makes it the single, correct
hook point for cache invalidation — you do NOT need to chase every DDL
handler in `admin_schema.rs` separately.

## Design

Add a per-repo cache: `TFxMap<String /* parent table name */,
Vec<ReverseFkEntry>>` where `ReverseFkEntry { child_table: String,
child_field: String, action: FkAction }` (unify `fk_restrict.rs`'s
`RestrictRef` and `fk_actions.rs`'s `DiscoveredRef` shapes if they're
close enough already — check both structs first; a single shared type
used by both discovery functions is cleaner than two near-duplicate
ones, but only merge them if it's a small, surgical change, not a big
refactor of either file's existing structure).

- **Storage**: `arc_swap::ArcSwap<TFxMap<String, Vec<ReverseFkEntry>>>`
  (RCU — read-heavy, rare invalidation, matches this workspace's
  concurrency ideology) on whichever type naturally owns "one cache per
  repo" — investigate whether `RepoInstance` (already has one instance
  per repo, already holds other per-repo shared state like
  `per_table_mvcc`) is the right home, or whether a new small struct
  living alongside it is cleaner. Use judgment; prefer the smaller diff.
- **O(1) role flags**: derive these FROM the same cache rather than a
  second independent structure — e.g. "is table X an FK parent with a
  non-`NoAction` action" is `cache.get(X).map(|v| v.iter().any(|e| e.action != FkAction::NoAction)).unwrap_or(false)`,
  and "is table X an FK child" requires a SEPARATE reverse-index (child →
  set of parent tables it references) OR a second small cache — investigate
  which shape Step 5 (#832, not yet briefed in detail) will actually need
  for its Serializable-upgrade decision (per the Step 3 memo: at
  implicit-delete-begin time on table X, decide "is X a parent worth
  upgrading"; at insert/update-staging time on table Y, decide "is Y a
  child requiring `require_footprint_for`") and build BOTH lookups the
  cache needs to serve in O(1), not just the parent-side one used by
  today's discovery functions.
- **Population**: lazy (build-on-first-miss, a.k.a. cache-aside) is
  simpler and safer than eager population at every schema-mutation point
  — a miss just means "run the existing O(tables) discovery once, cache
  the result." Rebuilding the WHOLE repo's map on any invalidation
  (rather than a surgical single-entry update) is the correct tradeoff
  here — DDL is rare, deletes/cascades are comparatively frequent, and a
  full rebuild is still just the existing O(tables) discovery, now paid
  once per DDL mutation instead of once per delete.
- **Invalidation**: clear the WHOLE repo's cache (`ArcSwap::store` a
  fresh empty map) from within `compile_table_schema`, at the point where
  it's known the live registry state has settled (i.e., AFTER any F-27b
  restore-on-failure has already run, so the cache reflects whatever
  ACTUALLY ended up live, success or rollback). Also invalidate on table
  create/drop (find the relevant `RepoInstance`/`TableManager` lifecycle
  methods — check whichever admin path adds/removes a table from
  `list_table_names()`'s result).

## Tests

**MANDATORY, test-then-fix in the same commit**:

1. Cache correctness after a schema mutation: add a `foreign_key` schema
   rule creating a NEW reverse-FK reference — confirm a subsequent delete
   on the parent table correctly discovers and enforces it (i.e., the
   cache doesn't serve a STALE "no such reference" from before the DDL).
2. Cache correctness after REMOVING a schema rule that declared an FK —
   confirm the reference is no longer discovered/enforced.
3. Cache correctness after a ROLLED-BACK schema mutation (force
   `compile_table_schema`'s activation to fail, per F-24/F-27b's existing
   test patterns in `schema_rollback_tests.rs` — reuse that file's
   fixtures) — confirm the cache reflects the RESTORED (old) state, not a
   stale view of the failed attempt.
4. A test demonstrating the O(1)/cached-vs-uncached distinction is
   meaningful — e.g. instrument (via a counter, or by checking this
   crate's existing convention for asserting call counts on a mock/spy
   resolver) that repeated deletes on the same parent table, with NO
   intervening schema mutation, do NOT re-run the O(tables) discovery
   scan each time.

## Constraints

- Do NOT implement Step 5's actual Serializable-isolation-upgrade or
  `require_footprint_for`-wiring logic — that is F-28 Step 5 (#832),
  which will consume the O(1) flags this task builds but is out of scope
  here.
- Do NOT touch `compile_table_schema`'s F-24/F-27b rollback logic itself
  — only ADD a cache-invalidation call at its natural completion point(s).
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine -p shamir-db -- --check` and
  `cargo clippy -p shamir-engine -p shamir-db --all-targets -- -D warnings`
  must be clean.
- Follow the workspace's concurrency ideology: `ArcSwap` for this
  snapshot-style RCU read pattern, `THasher`-keyed maps, no
  `std::sync::Mutex`/`RwLock` on this path.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -p shamir-db -- --check
cargo clippy -p shamir-engine -p shamir-db --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -- fk
./scripts/test.sh -p shamir-db -- schema
./scripts/test.sh @engine
./scripts/test.sh @e2e
```
