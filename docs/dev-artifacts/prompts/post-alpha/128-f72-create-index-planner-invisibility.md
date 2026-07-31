# F-72 (#899) — make legacy regular/sorted CREATE INDEX planner-invisible until backfill completes

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Only edit files;
the orchestrator commits.

## The bug

F-57 (#883, commit `fcaae001`) and F-70 (#897) serialise CREATE INDEX
against WRITERS (write-barrier + drain + `unique_write_lock`). That lock is
held by writers, not readers — it does nothing to hide a partially-built
index from the PLANNER. No race window is needed: the exposure lasts the
**entire backfill**, which streams the whole table.

**Sorted** (`crates/shamir-engine/src/table/table_manager_sorted_index.rs`,
`create_sorted_index_with_include`): `self.sorted_indexes.register(def)`
publishes the definition into the RCU-shared vec BEFORE the streamed
backfill loop that follows it writes any postings.
`crates/shamir-engine/src/table/read_planner.rs`'s
`try_plan_sorted_index_scan` calls `mgr.find_by_field(&field_path)`
unconditionally — no state check exists on `SortedIndexDefinition` at all —
so a concurrent range/`Between`/`Gte`/`Lte` query can be planned against a
half-populated index and silently return fewer rows than actually exist.

**Regular hash** (`crates/shamir-index/src/legacy/index_manager.rs`,
`create_index_from_records`): `add_index` publishes the definition
(`indexes.add_index` + `has_indexes.store(true)`) BEFORE `set_many` writes
postings — deliberately, per the "audit A9" doc comment there, to close a
DIFFERENT concern (a lost-write race against concurrent WRITERS). That
concern is legitimate and must NOT be undone by this task. But
`read_planner.rs`'s `find_single_field_index` calls
`self.index_manager_ref().iter_indexes()` unconditionally, so the same
half-built-index-visible-to-readers gap exists here too. Separately,
`save_index_info().await?` runs AFTER live publication — a persist failure
leaves a published in-memory index while returning `Err` to the caller;
fold this into the same fix (publish-then-persist is backwards).

**Do not confuse this with F-71 (#898, already shipped).** F-71 added a
`ready_at_version: u64` field to `SortedIndexDefinition` — that is an AsOf
**epoch** (governs the seek-vs-full-scan safety gate for a specific pinned
version), not a lifecycle **state**. This task adds an orthogonal concept:
whether the index exists to the planner AT ALL. Both fields may end up
living on the same struct; do not merge their semantics or reuse one to
imply the other.

## The reference shape — index2 already does this right

`crates/shamir-index/src/state.rs` defines `IndexState { Ready, Building }`
(two-variant, `#[default] Ready`). `crates/shamir-index/src/registry.rs`'s
`find_by_field_and_kind` (the index2 planner lookup) skips any backend
whose state is not `Ready` — a `Building` backend is invisible to that
lookup but still reachable by name via `get_by_name` (used by DDL paths
like `drop_index2` that must be able to see and act on a `Building`
backend). `create_index_v2` registers at `Building`, backfills, then calls
`set_state(Ready)` once the backfill fully completes — a single atomic
flip. Use this exact split (state-filtered planner lookup vs.
name-keyed DDL lookup) as the model for both legacy families.

**Read `crates/shamir-index/src/state.rs`'s module doc in full before
touching persistence** — it documents a proven, non-obvious bincode
landmine: `#[serde(default)]` on a new trailing field does NOT rescue a
read of OLD on-disk bytes for this workspace's pinned bincode (positional,
non-self-describing) — decoding old bytes against a struct with a new
field fails with `UnexpectedEof`, not a default-filled decode.
`crates/shamir-index/src/persistence.rs`'s `load_index2_metadata` is the
proven fix: try the current (with-state) shape first, and on decode
failure fall back to decoding the legacy pre-state shape, lifting every
legacy descriptor to `Ready` (a pre-state on-disk index was, definitionally,
always fully built — `Building` could never have been persisted before the
field existed). Reuse this exact fallback-decode pattern for BOTH
`IndexDefinition` (regular) and `SortedIndexDefinition` (sorted) — do not
reach for `#[serde(default)]` alone and assume it round-trips old data,
that assumption was tested and disproven for this exact struct shape.

## The fix — minimum viable lifecycle slice

For BOTH the regular-hash and sorted index families:

1. Add a `state: IndexState` field (reuse `shamir_index::state::IndexState`
   directly — do not invent a parallel enum) to `IndexDefinition` and
   `SortedIndexDefinition`. New registrations during CREATE INDEX start at
   `Building`.
2. Backfill happens exactly as today (register-first is fine — it is what
   closes the lost-write race against concurrent writers; do not change
   that ordering). What changes is that the freshly-registered definition
   must be **invisible to planner lookups** while `Building`.
3. Add the state-filter to every PLANNER-facing lookup: `read_planner.rs`'s
   `try_plan_sorted_index_scan` (via `SortedIndexManager::find_by_field`,
   or a new `find_by_field` variant/parameter that filters — your call, but
   the planner's actual call site must end up state-filtered) and
   `find_single_field_index`/`try_plan_and_index_scan`'s traversal of
   `IndexManager::iter_indexes()`. Enumerate every OTHER caller of
   `iter_indexes`/`find_by_field` (grep both — there are ~18 call sites
   across `doctor.rs`, `admin_describe.rs`, `admin_list.rs`,
   `admin_table_index.rs`, tests, etc.) and classify each as "planner read
   path" (must gate) vs. "DDL/introspection/doctor path" (must NOT gate —
   these legitimately need to see `Building` entries, mirroring index2's
   `get_by_name` staying unfiltered). Do not blanket-filter
   `iter_indexes()` itself if that would break a legitimate
   introspection/doctor caller — prefer a separate filtered accessor (or an
   explicit parameter) over silently changing the existing method's
   contract, exactly as index2 keeps `find_by_field_and_kind` (filtered)
   and `get_by_name` (unfiltered) as two distinct methods.
4. On successful backfill completion, flip `Building → Ready` as a single
   atomic publication (mirror index2's `set_state`) — this is the ONLY
   point a concurrent planner read may start observing the index.
5. Fix the regular-hash publish-then-persist inversion noted above:
   `save_index_info().await?` must run before (or as part of the same
   atomic step as) the state flip to `Ready`, not after — so a persist
   failure never leaves a `Ready`, queryable index whose definition failed
   to durably save. Decide and document: does a persist failure at this
   point roll back the registration entirely, or leave it durably
   `Building` for the next restart/doctor pass to reconcile? Either is
   acceptable — pick one, document why, and test it.
6. On backfill error or cancellation (the loop's `?` returns early): the
   definition must never end up `Ready` — either explicitly deregister it
   (clean rollback) or leave it `Building` (self-heals via the existing
   restart-from-scratch reconciliation path, IF one already exists for
   these legacy families — check `doctor.rs` for legacy-family coverage;
   if no such reconciliation exists yet for legacy indexes, that's out of
   scope for THIS task — in that case an abandoned `Building` entry must
   at minimum stay permanently planner-invisible and not silently
   resurrect as queryable, and this limitation must be stated explicitly
   in the commit message, not silently left as a gap).
7. Persistence round-trip: a definition written by a build BEFORE this fix
   (no `state` field on disk) must decode as `Ready` (matching index2's
   legacy-lift semantics) — write an explicit compat test proving this,
   mirroring `crates/shamir-index/src/tests/index_state_compat_tests.rs`.

## Definition of done

- A red-then-green (or failing-then-passing) pause-seam test for BOTH
  families independently (sorted AND regular hash), using this codebase's
  existing `TEST_*` hook conventions (no `sleep`-based timing — grep for
  the pattern used by `TEST_POST_BARRIER_PRE_WRITE_HOOK` and similar seams
  in `table_manager_sorted_index.rs`/`table_manager_index_mgmt.rs` for the
  house style): park CREATE mid-backfill, issue a concurrent read that
  would use the index if it were visible, and assert it instead falls back
  to a full scan / the prior complete state and returns the COMPLETE,
  correct row set — not a truncated one.
- A persistence compat test per family proving a pre-fix on-disk definition
  (no `state` field) decodes as `Ready`, not a decode failure and not
  `Building`.
- A test proving the regular-hash publish-then-persist fix: a simulated
  `save_index_info` failure never leaves a `Ready`, queryable, durably-
  unsaved index behind.
- `cargo fmt -p shamir-index -p shamir-engine -p shamir-db -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `./scripts/test.sh -p shamir-index -p shamir-engine -p shamir-db --full`
  green.
- Do not touch DROP/RENAME INDEX lifecycle semantics — that is F-76
  (#903), a separate, later task. This task only needs `Building`/`Ready`
  to exist and be correctly gated for CREATE.
- Do not run this task concurrently with any other task touching
  `table_manager_index_mgmt.rs`, `table_manager_sorted_index.rs`, or
  `index_manager.rs`'s create paths — overlapping DDL/lock surface.

Commit with `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>` per
repo convention.
