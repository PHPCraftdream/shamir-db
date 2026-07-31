# F-76 (#903) — unified DROP/RENAME INDEX lifecycle + per-family error/cancellation semantics

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Only edit files;
the orchestrator commits.

## Problem 1 — DROP exposes the SAME partial-visibility window F-72 (#899) just closed for CREATE

F-72 added a `state: IndexState` (`Building`/`Ready`, reused from
`shamir_index::state::IndexState`) to `IndexDefinition`/
`SortedIndexDefinition` and gated every PLANNER-facing lookup so a
`Building` (not-yet-ready) index is invisible to query planning. **DROP
has the mirror-image bug**: it deletes POSTINGS first, while the
definition is still `Ready` and planner-visible, and only removes the
definition/registry entry LAST. A concurrent reader can select the index
(it's still `Ready`) while its postings are mid-sweep, and observe a
registered-but-emptying index — silently wrong results, the same failure
shape F-72 closed for the opposite (build) direction.

Personally confirmed in TWO places (verify the rest yourself — the task
requires an exhaustive audit, not just these two):

- **index2 DROP** (`crates/shamir-engine/src/table/table_manager_index_mgmt.rs`,
  `drop_index2`, currently ~line 670): calls `backend.drop_all()` (sweeps
  postings — can be long) WHILE the backend is still in the live
  `index2_registry`, and only calls `registry.remove_by_id(id)`
  AFTERWARDS. The existing comment claims this order "prevents a reader
  seeing a registered backend without postings" — that reasoning is
  backwards; it's exactly the window that creates that exposure.
- **Regular-hash DROP** (`crates/shamir-index/src/legacy/index_manager.rs`,
  `drop_index`): scans and removes every posting entry via a
  `scan_prefix_stream` + `remove_many` BEFORE calling `self.indexes.
  remove_index(name_interned)` — same shape, same bug.

Audit and fix ALL drop paths (regular, unique, sorted, index2) — grep for
every `drop_*`/`remove_*` DDL method in `index_manager.rs`,
`table_manager_index_mgmt.rs`, `table_manager_sorted_index.rs`. **Correct
order**: atomically remove/flip the index out of the planner-visible
snapshot FIRST (mirroring how F-72's `Ready`-gate hides a `Building`
entry — consider whether DROP needs its own `Dropping` state, reusing the
same enum-extension approach the F-72 brief anticipated, or whether
simply removing the definition entry FIRST and sweeping postings SECOND
is sufficient for each family — decide per family and justify), THEN
delete postings asynchronously. A reader that already holds an `Arc`/
snapshot from before the removal keeps working against its own consistent
view (RCU semantics already guarantee this for the definition-holding
structures) — the fix is about NEW readers after the removal point, not
about disturbing in-flight ones.

## Problem 2 — no single error/cancellation contract across index families

Today each family's CREATE/DROP/RENAME sequences differ and each defines
"what happens on error" independently, inconsistently:

- **regular**: live definition published before postings; a persist or
  backfill error can leave it live with missing/partial postings.
- **unique**: postings written before the live definition, THEN live
  publication before metadata persist succeeds — a metadata-persist
  failure can leave a live index whose durable record never actually
  landed.
- **sorted**: live definition before streamed backfill; the existing code
  is explicitly commented `cancel-safe: NO`.
- **index2**: durable `Building` → private backfill → live `Ready` → final
  metadata persist; a failure of that LAST persist returns `Err` while the
  index is already live `Ready` in memory.
- **DROP/RENAME**: different, partly best-effort sequences per family
  (see Problem 1).

Define, for EVERY family and EVERY transition (Create, Drop, Rename), a
single documented contract answering:
- What does `Ok` mean — is it durable, is it planner-visible, both?
- What does `Err` mean — what state is left behind (nothing published,
  something durably `Building`/`Dropping`, or a genuine inconsistency)?
- Is the operation safely retryable (idempotent) after an `Err`?
- Exactly when does the index become planner-visible (the precise line/
  call, not "eventually")?
- On cancellation (task dropped mid-await) or crash (process dies
  mid-sequence), who is responsible for cleaning up orphan postings or an
  abandoned `Building`/`Dropping` entry — an explicit reconciliation path
  (like index2's restart-from-scratch self-heal), an operator-invoked
  `doctor::repair()` pass, or an accepted permanent-gap limitation (as
  F-72 documented for legacy CREATE)? State this per family; do not leave
  any family's answer implicit.

Write this contract as a single documented state machine — extend the
`Absent → Building → Ready → Dropping → Absent` shape (plus `Failed`)
that F-72's brief already anticipated needing an extension for DROP/
RENAME — as a doc comment in a sensible shared location (e.g. alongside
`shamir_index::state::IndexState`, or a new adjacent module if the
existing one is too narrow; your call, but make it discoverable from all
four families' code, not buried in one family's file).

## Definition of done

- Concurrent-reader tests for DROP, per family: a reader concurrently
  querying during a DROP must observe EITHER the complete index's correct
  result OR a full-scan fallback as if the index did not exist — NEVER a
  registered-but-partially-emptied index returning wrong/incomplete
  results. Use this codebase's established `TEST_*` pause-seam convention
  (no sleeps) to park the DROP mid-postings-sweep and issue the
  concurrent read deterministically.
- A red-then-green repro of the DROP visibility window for AT LEAST index2
  and regular-hash (the two confirmed above) — revert the reorder, show
  the concurrent reader test fails, restore, show it passes. Extend to
  sorted/unique if you find the same pattern there (very likely, per the
  task's own description).
- Crash/cancel-safety tests for each family's CREATE, DROP, and RENAME
  transition per the state machine you write — at minimum, prove that an
  error/cancellation at each documented failure point leaves the state
  the contract promises (never a silently-live index the metadata never
  actually recorded, never an orphan posting set with no definition
  anyone can find to clean up).
- The state-machine doc itself, reviewable independently of the code
  changes — a future reader must be able to answer "what does Err mean
  for a sorted-index RENAME" from this doc alone.
- `cargo fmt -p shamir-index -p shamir-engine -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `./scripts/test.sh -p shamir-index -p shamir-engine --full` green.
- Do not change CREATE's behavior beyond what's needed to fit the unified
  state-machine doc (F-72 already fixed CREATE's planner-visibility bug —
  don't re-litigate that work, only document/reference it as part of the
  unified contract).
- Do not run this task concurrently with any other task touching
  `index_manager.rs`, `sorted_index_manager.rs`,
  `table_manager_index_mgmt.rs`, or `table_manager_sorted_index.rs`.

This is a substantial P1 task (broader than the P0 items already shipped
this wave) — if the full per-family audit + fix + test matrix proves too
large for one pass, it is acceptable to land it in a tightly-scoped first
slice (e.g. index2 + regular-hash DROP fully fixed and tested, sorted/
unique DROP + all RENAME paths explicitly documented as a follow-up gap
in the state-machine doc) rather than a rushed, under-tested attempt at
everything. State clearly in the commit message exactly what was
completed versus explicitly deferred, and why.

Commit with `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>` per
repo convention.
