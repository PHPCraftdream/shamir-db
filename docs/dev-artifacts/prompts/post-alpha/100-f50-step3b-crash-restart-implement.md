# Brief for F-50 Step 3b (#873, P0, implement) — land the persisted index2 lifecycle state + crash/restart continuation

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

**Read `docs/dev-artifacts/research/f50-step3-crash-restart-spike.md` in
full first** — Step 3a's decision memo (commit `d4f159fb`). It settled four
design questions with reasoning; this brief transcribes its §5
implementation plan into concrete instructions and adds the exact current
line/shape references verified this session. Do not re-derive the design —
implement what the memo decided.

**Already landed by Step 3a (do not re-touch):**
- `crates/shamir-index/src/state.rs` — `IndexState` enum (`Ready` default,
  `Building`).
- `crates/shamir-index/src/descriptor.rs` — `IndexDescriptor.state: IndexState`
  field.
- `crates/shamir-index/src/persistence.rs` — `decode_persisted_indexes`
  (try-current-then-fallback-legacy load path), wired into
  `load_index2_metadata`. **This is the ONLY safe way old on-disk
  descriptors forward-compat — do not add a second mechanism.**

**The four settled decisions this step implements:**
1. Forward-compat: already done (Step 3a). Nothing to do here.
2. Crash-restart continuation: **restart-from-scratch** — a `Building`
   backend found on table-open is dropped (`IndexBackend::drop_all()`) and
   its backfill re-run from scratch, then flipped to `Ready` and re-persisted.
3. Persist point: `Building` **piggybacks on `create_index_v2`'s EXISTING
   first `save_index2_metadata` call** (`table_manager_index_mgmt.rs:118-119`)
   — no third persist point.
4. Doctor extension: **designed, not yet implemented** — this step
   implements it per the memo's §4 design.

## What to implement (memo §5, items 4-10)

### 4. `create_index_v2` — Building-at-start, Ready-at-finish

In `table_manager_index_mgmt.rs::create_index_v2`:
- Construct the new descriptor with `state: IndexState::Building` (not the
  default `Ready`).
- At the FIRST `save_index2_metadata` call (`:118-119`, already there for
  the #534 id-reuse fix), make the in-flight `Building` descriptor visible
  to that persist. The memo recommends **option (A) — surgical**: give
  `save_index2_metadata` (or add a new `save_index2_metadata_with_pending`
  variant) an `Option<IndexDescriptor>` parameter for the in-flight
  descriptor, so the persisted set is `registry.all_descriptors() ∪
  {pending}` without inserting the backend into the LIVE registry yet
  (preserving the existing backfill-before-register invariant — the live
  write-hook must not route to an unregistered backend, per
  `backfill_index2_backend`'s own doc comment). Read that doc comment
  before changing this call site.
- After `backfill_index2_backend` completes and `index2_registry.insert`
  (`:322-325`) runs, flip the descriptor's state to `Ready` before the
  FINAL `save_index2_metadata` call (`:327-328`).

### 5. `registry.rs` — track authoritative state

Per the memo's recommendation: extend `IndexRegistry`'s `by_id` tuple from
`(Arc<dyn IndexBackend>, u64 gen)` (landed by F-50 Step 1) to
`(Arc<dyn IndexBackend>, u64 gen, IndexState)`. Add
`registry.set_state(id: u32, state: IndexState)`. `all_descriptors()`
(already merges backend + gen into a cloned descriptor) additionally
overwrites the cloned descriptor's `state` field from the tuple's
authoritative value — this keeps `IndexDescriptor.state` as read from disk
a pure serialization carrier, with the registry as the single source of
truth for a LIVE backend's current state. Do not add per-backend interior
mutability (the memo explicitly rejected this as more invasive) — this is
the F-50 Step 1 generation-tag pattern, extended by one field.

**Do not touch the generation-bumping logic itself** (`insert`/
`remove_by_id`'s `fetch_add` calls) — this task only adds the state slot
alongside it.

### 6. Planner Ready-gate

`read_planner.rs::try_plan_index2` (`:32-104`, confirmed this session —
currently gates only on `registry.is_empty()` then dispatches by
`find_by_field_and_kind`/`get_by_name`). Add a `state == Ready` check so a
`Building` backend is invisible to read planning — either by having
`find_by_field_and_kind`/`get_by_name`-equivalent lookups filter on state,
or by checking `backend`'s state (via the registry, not the descriptor
clone) after resolution and returning `None` if `Building`. Pick whichever
is less invasive to the existing lookup helpers; state your choice in the
final summary.

### 7. Self-healing restart-from-scratch on table open

In `table_manager.rs`'s open path (`:296-310`, where `load_index2_metadata`
results are consumed and backends are reconstructed): for each descriptor
loaded with `state == Building`, after its backend is built (before
`restore_on_open` runs, per the memo's §2.3): call `drop_all()` on it,
re-run `backfill_index2_backend` (you may need to extract/reuse the
existing backfill logic — check whether it's already a standalone
callable function or is private to `create_index_v2` and needs a small
signature adjustment to be callable from the open path too), flip its
state to `Ready`, register it, and re-persist via `save_index2_metadata`.
This makes the recovery automatic — no operator action needed.

### 8. Doctor extension

`table/doctor.rs` — add:
```rust
pub struct Index2Health {
    pub id: u32,
    pub name: String,
    pub state: IndexState,
    pub healthy: bool,   // false iff state == Building
}
```
Add `index2_backends: Vec<Index2Health>` to `VerifyReport` (currently has
`regular_indexes`/`unique_indexes`/`sorted_indexes: Vec<IndexHealth>` —
follow that existing shape/naming convention). `verify()` gains a loop over
`self.index2_registry().all_backends()` (or an equivalent that also
surfaces state — you may need a registry accessor beyond
`all_descriptors()` if that clones the state field usefully already, check
first) populating this and folding `Building` backends into
`is_healthy()`/`all_indexes_healthy()`'s existing AND-chain (rename that
method or its body appropriately — follow existing naming). Give each
unhealthy entry a clear message per the memo ("index2 backend '{name}'
(id={id}) is in Building state — build was interrupted; reopen the table
or run repair").

`repair()` may OPTIONALLY re-trigger item 7's restart-from-scratch routine
for any `Building` backend found — a thin wrapper, not new logic. This
part is explicitly marked optional in the memo; implement it if it's a
small addition given how you structured item 7, otherwise note in your
summary why you deferred it (do not force it if it creates awkward
coupling).

### 9. Tests

At minimum:
- **Crash/restart simulation**: persist a `Building` descriptor directly
  (bypassing the normal `create_index_v2` flow, mirroring how you'd
  hand-construct one — or by pausing `create_index_v2` mid-sequence if a
  test seam is easy to add, your call on the simplest deterministic way to
  get a `Building` descriptor on disk with a live table not yet knowing
  about it), then reopen the table (fresh `TableManager`/`RepoInstance`
  against the same store) and assert: the backend is re-backfilled,
  reaches `Ready`, and is queryable.
- **Planner Ready-gate test**: a `Building` backend is invisible to
  `try_plan_index2` (returns `None`/falls through to full scan) even
  though it exists in the registry.
- **Doctor `Building`-detection test**: `verify()` reports a `Building`
  backend as unhealthy with the `index2_backends` entry present.

### 10. `KNOWN_LIMITATIONS.md`

Update `docs/guide-docs/KNOWN_LIMITATIONS.md` to document: the
crash-restart gap is now closed via restart-from-scratch (cite this
commit), and the residual cost (a crash during an index build always
re-does the O(N) backfill on next open, even if the crash happened at 99%
completion — accepted per the memo's reasoning that resume would need a
persisted cursor + per-backend idempotency, unjustified for a rare DDL
event).

## What NOT to do

- Do NOT implement DDL cancellation (unrelated to crash-restart; #872 now
  provides a real DROP path, but cancellation of an IN-PROGRESS build is
  still a separate, un-scoped concern — do not fold it in here).
- Do NOT touch F-50 Step 1/2's generation-gate mechanism
  (`IndexRegistry::generation`, `rederive_index2_ops_post_stage`,
  `SortedIndexManager::generation`) beyond the tuple-widening in item 5
  above (which is additive, not a change to the generation logic itself).
- Do NOT touch #872's `drop_index2`/`drop_sorted_index`/`sorted_index_exists`/
  `index2_exists` — landed, unrelated.
- Do NOT re-implement or second-guess Step 3a's forward-compat mechanism —
  it is proven and landed; just use `IndexState`/`IndexDescriptor.state` as
  they exist.

## Constraints

- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine -p shamir-index -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` must be clean.
- This is a genuinely large task touching `create_index_v2`,
  `IndexRegistry`, the planner, the table-open path, and the doctor —
  timebox it. If the self-healing restart-on-open (item 7) proves
  substantially harder than expected (e.g. `backfill_index2_backend` is
  too entangled with `create_index_v2`'s other state to extract cleanly),
  stop, document precisely what's hard, and land items 4-6 + 8 with a
  clear note on what's deferred — a partial, honestly-scoped landing is
  better than an untested rush through all 7 items. Say so explicitly in
  your final summary if you timebox out of anything.
- Clean up any scratch/debug log files you create in the repo root before
  finishing.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -p shamir-index -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -p shamir-index --full
```

When done, give your final summary as plain text: exactly which of items
4-10 you completed vs. deferred (and why, if any), the registry state-
tracking design you landed, the planner-gate mechanism you chose, the
self-healing restart implementation and its test's actual output, the
doctor extension and its test's actual output, and confirmation
fmt/clippy are clean.
