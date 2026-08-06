# Brief — R0-B: instance provenance for tx-plan reconcile + sorted rename generation (#1007 + #1008)

## Context

S.H.A.M.I.R. Database, `crates/shamir-index` + `crates/shamir-engine` +
`crates/shamir-tx`. Part of the release-blocker execution map
(`docs/dev-artifacts/roadmap/2026-08-05-release-blocker-execution-map.md`
§R0-B) — read that section first. **This is the largest and riskiest task in
the R0 wave** — take it carefully, in the staged order below, and do not rush
Part 2.

**Prerequisite work already landed, use it, do not re-derive it:** R0-D
(`5935b346`, `IndexState::Failed`), R0-A (`125b7981`, full `ddl_admission`
coverage + `IndexRegistry`'s merged generation counter), R0-C (`6602ea4e`,
registry insert atomicity + cross-family namespace). R0-B does not depend on
R0-C's specific changes but touches adjacent code — re-read current line
numbers, they have shifted since the source reviews.

## Pre-verified: this does NOT touch the WAL wire format

The execution map flagged "verify WAL impact before starting" as a risk gate.
**Already checked, verified safe** — do not re-litigate this, just be aware
of it:

- `crates/shamir-tx/src/index_write_op.rs`'s `IndexWriteOp` enum is `Debug,
  Clone` only — NOT `Serialize`/`Deserialize`. It is a pure in-process
  transaction-staging construct; it never touches disk directly.
- The actual WAL-serialized type is `WalOpV2` in
  `crates/shamir-wal/src/wal_entry_v2.rs`. Its `IndexPut`/`IndexDel` variants
  already carry an `idx_id: u32` field — **already anticipating exactly this
  kind of identity work** (see the doc comment at `wal_entry_v2.rs:69-79`:
  "Stage 5 reconciliation may... thread `idx_id` through `IndexWriteOp` and
  emit it here" — option (a), explicitly deferred). Currently `idx_id` is
  always emitted as `0`; recovery decodes the real index id from the
  posting key's byte prefix instead.
- Conclusion: adding an identity field to `IndexWriteOp` and to
  `IndexDefinition`/`SortedIndexDefinition` (both described below) is a
  **normal in-memory Rust struct change**, not a wire-format migration. If
  you add the field to `IndexDefinition`/`SortedIndexDefinition` (which ARE
  `Serialize`/`Deserialize` — they're persisted metadata), mark it
  `#[serde(skip)]` and give it a fresh in-memory-only value on construction —
  there is DIRECT precedent for this exact pattern already in both structs
  (`SortedIndexDefinition::included_fields_interned` at
  `crates/shamir-index/src/base_index/sorted_index_definition.rs:44-49` is
  `#[serde(skip)]`, populated at registration/load time, never persisted —
  copy this pattern, do not invent a new one).

## Part 1 — sorted RENAME must bump generation (#1007, do this FIRST, it's small)

**Code:** `crates/shamir-index/src/base_index/sorted_index_manager.rs` —
`rename_definition` (search for `pub async fn rename_definition`). Compare
against `register` and `drop_index`, which both already do
`self.generation.fetch_add(1, Ordering::AcqRel)` on every successful mutation
(grep `generation.fetch_add` in this file for both call sites). `rename_definition`
does the RCU swap + epoch-carry + persist but never bumps `generation`.

**Fix:** add the same `self.generation.fetch_add(1, Ordering::AcqRel)` call
to `rename_definition`, in the same relative position (after the RCU commit
succeeds, mirroring the existing two call sites' placement). This alone makes
`crates/shamir-engine/src/tx/pre_commit.rs`'s sorted rederive gate
(`if sorted_mgr.generation() == stage_gen { continue }`) correctly detect that
something changed after a rename and re-run re-derivation — necessary but
**not sufficient by itself**: once the gate fires, sorted's rederive
currently re-plans against ALL current defs (adding correct new ops under the
new name) but never removes the STALE ops already staged under the old
name/id — that's Part 2's job. Do not consider #1007 "done" until Part 2 also
lands; they must ship together for the sorted-rename scenario to actually be
fixed end-to-end (Part 2's test in this brief covers the combination).

**Test:** a registry-level test analogous to the existing generation-bump
tests for `register`/`drop_index` in this file's test module — rename a
definition, assert `generation()` advanced. Confirm it fails (no advance)
against the reverted code.

## Part 2 — typed instance provenance for tx-plan reconcile (#1008, the substantial part)

### The defect, precisely

`crates/shamir-engine/src/tx/pre_commit.rs` has three rederive functions
(`rederive_index2_ops_post_stage`, the sorted block, `rederive_base_index_ops_post_stage`
— grep for these names, they're all in this file). Every one of them can ADD
ops for definitions that changed since stage. Only the base-index one
attempts to REMOVE stale ops, and it does so by an identity heuristic —
`(is_unique, name_interned)` — that does not survive a DROP-then-CREATE of
the same name with a different definition (ABA). Index2 and sorted don't
retract stale ops AT ALL: `rederive_index2_ops_post_stage` only calls
`backends_newer_than(stage_gen)` and appends; the sorted block re-plans
against all current defs and appends, never removing.

**Concrete failure modes this causes** (from the execution map, verify by
tracing the code — do not just take this list on faith, confirm each against
current source before treating it as ground truth for your tests):

1. `stage → DROP sorted index → commit`: the stale staged sorted ops (never
   retracted) get applied in the commit's physical-write phase, resurrecting
   postings for an index that no longer exists.
2. `stage → DROP index2 backend → commit`: same class, orphan postings under
   the old numeric id.
3. Base-index ABA: `stage (index x on field a) → DROP x → CREATE x on field
   b → commit`: old ops (hashed on field `a`'s value) are retained because
   `(is_unique, name_interned)` still matches — contaminating the NEW index x
   (built on field `b`) with postings computed against the wrong field.
4. Combined with Part 1: `stage sorted op (targets old_id) → RENAME → commit`:
   even with Part 1's generation bump making the gate fire, the stale
   old-`id`-targeted op is never removed, so it still gets applied — orphan
   postings under the old namespace, on top of whatever the (now-firing)
   rederive correctly adds under the new name.

### The fix

**One unifying mechanism across all three reconcile functions**, replacing
the byte/name heuristics: every definition (`IndexDefinition` for
regular/unique, `SortedIndexDefinition` for sorted) gets an in-memory-only
**instance epoch** — a `u64` (or `u32`, match what's locally idiomatic)
bumped every time that NAME gets a fresh definition: on CREATE (fresh value)
and on RENAME (bump, since Part 1 already established rename changes
identity for generation-gating purposes — the epoch bump is the SAME
semantic, at finer grain). `#[serde(skip)]`, mirroring
`SortedIndexDefinition::included_fields_interned`'s existing pattern (see
above) — do NOT persist this, it only needs to survive within one process's
uptime, same as index2's existing `BackendEntry.gen` (see below).

- **Index2 already has this** — `BackendEntry.gen` (fixed correctly by R0-A,
  `crates/shamir-index/src/registry.rs`) IS this exact epoch, already
  correctly maintained. Index2 does NOT need a new field. It needs its
  reconcile function to ALSO retract: for every op staged against an index2
  backend whose CURRENT `entry.gen` no longer matches what was true at stage
  time (i.e., the backend was removed, or replaced), remove that op from
  `tx.index_write_set` before the physical-write phase — mirroring the shape
  of base-index's EXISTING 2c-retain filter (`pre_commit.rs`, grep
  `p02c_retain` for the existing test file naming this pattern — reuse the
  general shape, just key it correctly).
- **Sorted and base-index need the new field.** Add it to
  `SortedIndexDefinition` and `IndexDefinition`. Stamp it into every
  `IndexWriteOp` these two families' `plan_*` methods produce (find every
  `IndexBackend`/manager method that constructs an `IndexWriteOp::SetPosting`/
  `RemovePosting` for these two families — they already have access to the
  definition they're planning against, so they already have the epoch value
  available; this is a threading exercise, not a new lookup).
- **`IndexWriteOp` needs a provenance field** to carry `(family,
  name_interned, instance_epoch)` (or however you choose to represent
  "family" — could be implicit from context if `IndexWriteOp` stays
  per-family-typed, check how it's currently used before deciding to add an
  explicit family tag; the goal is that reconcile can unambiguously match a
  staged op back to "is the definition I was planned against still the SAME
  instance"). This is a shape change to a `Debug, Clone`-only enum — safe,
  confirmed above, but touches every construction site; let the compiler find
  them all (do not grep-and-hope).
- **Reconcile logic, all three functions**: for every currently-live
  definition whose epoch is NOT already covered by a staged op with the
  SAME `(name_interned, epoch)`, derive fresh ops (mirrors existing
  "newer than stage" logic, just keyed more precisely). For every STAGED op
  whose `(name_interned, epoch)` does NOT match ANY currently-live
  definition's CURRENT epoch, remove it from `tx.index_write_set` before the
  physical-write phase. This single rule, applied uniformly, closes all four
  failure modes in one mechanism — do not implement per-family bespoke logic
  if you can help it; the whole point of this task is replacing three
  divergent heuristics with one correct one.

### Tests (must fail against the reverted code — this is the highest-value
part of this brief, budget real time for it)

- `stage → DROP → commit` for EACH of the four index kinds (regular, unique,
  sorted, index2/functional-or-fts pick one representative index2 kind):
  postings are NOT resurrected.
- `stage (field a) → DROP → CREATE same-name (field b) → commit` for base
  regular AND unique: the new index only contains postings derived from
  field `b`, never field `a`'s stale ops. For unique specifically, confirm no
  false conflict from the stale op's key.
- `stage sorted op → RENAME → commit`: combines Part 1 + Part 2 — the row
  ends up indexed under the NEW name, and the OLD namespace has no orphan
  posting. This is the direct end-to-end proof that #1007 and #1008 together
  close NP-2.
- Multiple lifecycle transitions between stage and commit (e.g. CREATE →
  RENAME → DROP → CREATE-same-name, all before one commit) for at least one
  family — confirms the epoch mechanism handles more than one transition, not
  just the single-transition cases above.
- Rollback: a tx that stages against one epoch, sees the definition rotate
  twice more before commit, then the COMMIT ITSELF fails for an unrelated
  reason (e.g. inject a failure) — confirm no partial/inconsistent state is
  left in the registries themselves (the tx's own atomicity, not the
  reconcile logic, should own this — but verify the reconcile changes don't
  accidentally weaken it).

## Constraints

- Follow `CLAUDE.md`: no new `std::sync::Mutex`/`RwLock`, Fx-hash collections
  if you need a lookup structure, `Result`/`thiserror` for new fallible paths.
  Test files under the crate's existing `tests/` directory convention.
- Gate: `cargo fmt -p shamir-index -p shamir-engine -p shamir-tx`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `./scripts/test.sh -p shamir-index -p shamir-engine -p shamir-tx --full`,
  and `./scripts/test.sh @oracle` must all be clean. This touches WAL-adjacent
  and commit-critical code — also run
  `./scripts/test.sh -- crash_recovery` (or the closest matching filter) to
  catch any crash-recovery regression, and read (don't skip) any SLOW/loom
  output if `@oracle`'s scope includes loom tests.
- Do NOT touch `WalOpV2`/`wal_entry_v2.rs` — populating the currently-`0`
  `idx_id` field with real data is explicitly OUT of scope for this brief
  (it's the "Stage 5 reconciliation... may thread idx_id through" work the
  existing comment defers — a separate future task, not this one). This
  brief's identity concept lives entirely in-memory, pre-WAL.
- Do NOT touch R0-C's work (registry insert atomicity, cross-family
  namespace) or #1025 (unified DROP INDEX). Do NOT expand into #1011 (DROP
  vs active readers — a separate decision-task, not code).
- If, while implementing, you find the "one unifying mechanism" genuinely
  cannot cover all three families without excessive complexity, STOP and
  report the specific obstacle rather than shipping three divergent
  heuristics again — that would defeat this task's purpose. A smaller,
  correct, uniform fix beats a larger, inconsistent one.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or
any git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## Definition of done

- [ ] `rename_definition` bumps `SortedIndexManager.generation`, matching
      `register`/`drop_index`'s existing pattern.
- [ ] `IndexDefinition` and `SortedIndexDefinition` carry an in-memory-only
      (`#[serde(skip)]`) instance epoch, bumped on CREATE (fresh) and RENAME.
- [ ] `IndexWriteOp` carries enough provenance for reconcile to match a
      staged op back to "is this still the same instance".
- [ ] All three `pre_commit.rs` rederive functions both ADD ops for
      newly-live definitions AND REMOVE ops for no-longer-matching ones,
      via the same mechanism.
- [ ] `stage → DROP → commit` does not resurrect postings, for all four
      index kinds.
- [ ] `stage (field a) → DROP → CREATE same-name (field b) → commit` does
      not contaminate the new index with field-`a`-derived postings (base
      regular and unique).
- [ ] `stage sorted op → RENAME → commit` lands the row under the new name
      with no orphan in the old namespace (Part 1 + Part 2 combined proof).
- [ ] Multi-transition and rollback tests pass.
- [ ] fmt/clippy/tests green, including the crash-recovery-focused run
      called out above (report exact commands and pass/fail).
