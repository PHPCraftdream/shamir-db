# Brief — R0-C: registry insert atomicity + cross-family index namespace (#1009 + #1010)

## Context

S.H.A.M.I.R. Database, `crates/shamir-engine` + `crates/shamir-index` +
`crates/shamir-db`. Part of the release-blocker execution map
(`docs/dev-artifacts/roadmap/2026-08-05-release-blocker-execution-map.md` §R0-C) —
read that section first.

**Prerequisite work already landed, use it — do not re-derive it:**

- **R0-D** (commit `5935b346`): added `IndexState::Failed` (planner-invisible,
  append-safe enum variant) and `IndexRegistry::set_failed`/`failure_reason_of`.
  Reuse this — do not invent a parallel failure-signaling mechanism.
- **R0-A** (commit `125b7981`): every CREATE/DROP/RENAME across all four index
  families (regular, unique, sorted, index2) now holds
  `TableManager::ddl_admission` (via `begin_write_barrier`) for its ENTIRE
  critical section — meaning **at most one registry-mutating DDL op is ever in
  flight per table**. This is the load-bearing invariant both tasks in this
  brief build on. Read `begin_write_barrier`'s doc
  (`crates/shamir-engine/src/table/table_manager.rs:937-965`) if you haven't
  already.

## Part 1 — registry insert atomicity (#1009)

### The defect (verify by reading, current line numbers may have drifted
slightly since the source reviews)

`crates/shamir-index/src/registry.rs`'s `insert()` (~`:130-200` post R0-A —
search for `pub async fn insert`): publishes to `by_id` first, `by_name`
second. If `by_name.insert_async` fails (name already taken), the function
returns `Err` **without rolling back `by_id`** — the backend ends up
registered by id (visible to `all_backends()`/`backends_newer_than()` and any
planner path that iterates by id) but unreachable by name (a `DROP` by that
name won't find it).

Three call sites in `crates/shamir-engine/src/table/table_manager.rs`
(currently around `:517`, `:527`, `:536` — grep `index2_registry.insert` to
confirm) swallow this error entirely: `let _ = mgr.index2_registry.insert(backend).await;`.
These are all inside the OPEN-PATH recovery loop that replays PERSISTED
descriptors — i.e., this failure mode fires when **on-disk metadata contains
two descriptors with the same name** (corruption, or a bug elsewhere that let
it happen before this fix existed).

### The fix

1. **Make `insert()` check-before-mutate, not mutate-then-maybe-rollback.**
   Add a cheap `by_name.contains_async(&name_interned)` check FIRST; if the
   name is taken, return `Err` WITHOUT touching `by_id` at all. Only proceed
   to the `by_id` → `by_name` publish sequence if the name is free. This is
   safe from races on the SAME table specifically BECAUSE of R0-A: `insert()`
   is only ever called while the caller holds `ddl_admission` for that table
   (client-driven CREATE paths after R0-A; open-path recovery is single-task
   sequential by construction, so no concurrent caller exists there either) —
   so "check then act" cannot be beaten by a second concurrent `insert()` on
   the same table. Document this precondition on `insert()`'s doc comment
   (mirror how R0-A's brief documented the equivalent invariant on
   `IndexRegistry`'s counter merge).
2. **Fix the three open-path call sites to fail closed, not silently
   continue.** Once (1) lands, `insert()` failing during open-path recovery
   means the ON-DISK METADATA is genuinely contradictory (two persisted
   descriptors claim the same name) — there is no single "broken backend" to
   mark `Failed` the way R0-D's fix does (that mechanism is for a backend that
   registered fine but failed to RESTORE its content; this is a backend that
   can't even be assigned a name slot). The correct response is stronger:
   **`TableManager::create()`'s open path must return an `Err` for the WHOLE
   table open**, not silently drop one of the two colliding descriptors and
   proceed with a partially-loaded table. Silently picking a winner is exactly
   the "molча выбирать семейство нельзя" failure mode the execution map
   warns about. Write a clear error message naming both colliding descriptor
   ids/names and instructing the operator to resolve the on-disk duplicate
   before the table can be reopened (this is a genuinely rare, corruption-only
   path — fine for it to require manual intervention).
3. **Add a consistency check to `doctor::verify()`**: `by_id ↔ by_name ↔
   persisted descriptors` — even though (2) makes duplicate-name corruption
   fail the table open outright (so an already-open table cannot currently
   have this problem), a defense-in-depth check in `verify()` costs little and
   catches any future code path that might reintroduce the hazard. Keep this
   cheap (no full re-scan of postings, just cross-referencing the three
   registry-level structures already in memory).

### Tests (must fail against the reverted code)

- `insert()` with a colliding name returns `Err` and `by_id` contains NO entry
  for the failed insert (not just "eventually consistent" — check immediately
  after the failed call).
- Open-path recovery with two persisted index2 descriptors sharing a name:
  `TableManager::create()` returns `Err`, not a partially-loaded table.
- `doctor::verify()`'s new consistency check flags a synthetically-constructed
  `by_id`/`by_name` mismatch (if you can construct one through the public API
  post-fix — if the fix makes such a mismatch genuinely unreachable through
  normal use, say so and test the check's logic directly instead via
  whatever internal seams are available).

## Part 2 — cross-family index name uniqueness (#1010)

### The defect (verify by reading)

Four existence-check methods already exist and are individually correct:
`crates/shamir-engine/src/table/table_manager_index_mgmt.rs` — `index_exists`
(~`:1379`), `unique_index_exists` (~`:1392`), `sorted_index_exists` (~`:1309`),
`index2_exists` (~`:1326`).

**CREATE never combines them.** `crates/shamir-db/src/shamir_db/execute/admin_table_index.rs`'s
`handle_create_index` (~`:352-357`) checks ONLY `unique_index_exists` OR
`index_exists`, based on `op.unique` — it never checks `sorted_index_exists`
or `index2_exists`, regardless of which family is being created. Nothing
stops `CREATE INDEX foo ...` (regular) from succeeding when a sorted or index2
index named `foo` already exists on the same table.

**DROP and RENAME already check all four for "does the name exist anywhere"**
(`admin_table_index.rs` ~`:563-576` for drop, ~`:676-685` for rename, both via
an `||` chain across all four `*_exists` calls) — that part is fine. The
actual defect is downstream: once existence is confirmed, the code that
performs the operation (DROP: `admin_table_index.rs` ~`:593-618`; RENAME:
`table_manager_index_mgmt.rs` ~`:1437-1690`) resolves WHICH family to act on
via ANOTHER short-circuit `||` chain (calling `drop_unique_index`/
`drop_index`/`drop_sorted_index`/`drop_index2` in sequence, stopping at the
first that returns `true`) — so if two families share a name, DROP silently
removes only the first match and RENAME may touch only one family, leaving
inconsistent siblings behind.

### The fix

1. **Preflight check at CREATE, done under admission (not at the handler
   layer where it currently sort-of-exists for DROP/RENAME).** The
   handler-layer checks in `admin_table_index.rs` run BEFORE the eventual
   `TableManager` method acquires `begin_write_barrier` — a TOCTOU gap
   (another CREATE for a different family could register the same name
   between the handler's check and the actual registration). The check must
   move INSIDE each family's `TableManager` create method (`create_index`,
   `create_unique_index`, `create_unique_index_from_records`/
   `create_unique_index_body`, `create_index_v2`, `create_sorted_index_with_include`
   — grep for all CREATE entry points, there may be more than these), executed
   AFTER `begin_write_barrier` is acquired (or, if there's a cheap fast-reject
   opportunity analogous to R0-A's tombstone-check-before-barrier fix, note
   that as an option — but the correctness-critical placement is "while
   holding admission", not "before it", since only the admission-guarded
   window guarantees no other family's CREATE can interleave). Add a single
   shared helper (e.g. `TableManager::any_index_exists(name) -> bool`
   combining all four `*_exists` calls) so every CREATE path uses the SAME
   check rather than four independent call sites drifting apart.
2. **Startup doctor check for PRE-EXISTING collisions.** Existing tables may
   already have cross-family name collisions from before this fix (this fix
   only prevents NEW ones). Add a `doctor::verify()` check that walks all four
   families' names and reports any name present in more than one family —
   this must NOT silently resolve/repair automatically (there's no safe
   default choice of which family "wins"); it must surface as an operator-
   visible unhealthy condition requiring explicit repair/rename.
3. **Leave DROP/RENAME's existing short-circuit resolution as-is for THIS
   brief** unless you can do it with low risk: once (1) prevents NEW
   collisions, the short-circuit-picks-first-match behavior in DROP/RENAME
   only remains a problem for tables with PRE-EXISTING collisions (surfaced by
   (2)). Fixing DROP/RENAME to act on all matching families (or to require an
   explicit family disambiguator when a collision is detected) is legitimate
   scope for this brief if it's straightforward given what you find, but do
   not let it balloon — the execution map treats "unified DROP INDEX without
   a family disambiguator" as a SEPARATE, later task (#1025, blocked on this
   one). If you touch DROP/RENAME's resolution logic, keep it to "detect a
   collision and refuse with a clear error instead of silently picking one",
   not a full redesign.

### Tests (must fail against the reverted code)

- `CREATE INDEX foo` (regular) after a sorted (or index2) index named `foo`
  already exists on the same table → rejected, for every ordered pair of the
  four families (12 directed pairs, or at minimum one representative pair per
  family as CREATE-target with a different family as the pre-existing
  occupant — cover all four as the NEW one being created).
  `doctor::verify()` correctly reports a synthetically pre-seeded cross-family collision.
- If you touched DROP/RENAME's resolution: a test that a collision is
  detected and refused rather than silently resolved to one family.

## Constraints

- Follow `CLAUDE.md`: reuse `tokio::sync::Mutex`/`ddl_admission` precedent
  (already in place, no new primitive needed here). Test files under the
  crate's existing `tests/` directory convention.
- Gate: `cargo fmt -p shamir-index -p shamir-engine -p shamir-db`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `./scripts/test.sh -p shamir-index -p shamir-engine -p shamir-db --full`,
  and `./scripts/test.sh @oracle` must all be clean.
- Do NOT touch R0-B (#1007/#1008 — sorted rename generation bump,
  reconcile/ABA) — separate brief, not yet dispatched. Do NOT touch
  `IndexState`/`degraded_index_count.rs` beyond what Part 1 explicitly asks
  (reusing, not modifying, R0-D's `Failed` machinery). Do NOT implement #1025
  (unified DROP INDEX without a family-disambiguator flag) — that is
  explicitly out of scope, see Part 2 point 3.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or
any git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## Definition of done

- [ ] `IndexRegistry::insert` checks name availability before mutating either
      map; a colliding insert leaves `by_id` untouched.
- [ ] Open-path recovery on duplicate persisted index2 metadata fails the
      WHOLE table open with a clear, actionable error — not a silently
      partial table.
- [ ] `doctor::verify()` gains a `by_id ↔ by_name ↔ persisted` consistency
      check.
- [ ] CREATE for any of the four families rejects a name already used by ANY
      other family, checked while holding `ddl_admission` (not just at the
      handler layer).
- [ ] `doctor::verify()` (or an equivalent startup/doctor path) reports
      pre-existing cross-family name collisions as an unhealthy condition.
- [ ] New tests for all of the above, each confirmed to fail against the
      pre-fix code.
- [ ] fmt/clippy/tests green (report exact commands and pass/fail).
