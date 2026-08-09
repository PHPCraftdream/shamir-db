# Brief 54 — #1058: in-flight build registry + dirty-set capture (regular family)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## Context — read these two documents first, in full

1. `docs/dev-artifacts/research/2026-08-07-online-index-build-rfc.md` (RFC v2,
   approved) — §2.3 is your spec.
2. `docs/dev-artifacts/research/2026-08-09-p1054-write-path-audit.md` — the
   exhaustive write-path audit. Its conclusion is load-bearing: tx-staged and
   non-tx CRUD writes both funnel through the SAME shared planning methods —
   `IndexManager::plan_record_created`/`plan_record_updated`/
   `plan_record_deleted` (`crates/shamir-index/src/base_index/index_manager.rs:2013,2105,2200`).
   These iterate `self.indexes.iter()` with NO `IndexState` filter and
   produce `SetPosting`/`RemovePosting` directly, even for `Building` defs.
   **This is why the capture point belongs INSIDE these three methods, not
   at the ~15 call sites that invoke them** (`table_manager_tx_ops.rs`,
   `table_manager_crud.rs`, `pre_commit.rs::rederive_base_index_ops_post_stage`).

This slice (1c) targets the **regular/hash family only**
(`IndexManager`) — NOT `SortedIndexManager`, even though the audit found the
same pattern there too. Sorted is explicitly deferred (RFC v2 §5.3). Do not
touch `sorted_index_manager.rs`.

## Part 1 — in-flight build registry

Add a registry to `IndexManager` tracking which regular-family indexes have
an online build currently in Phase B/C/D (i.e., barrier-free, dirty-set-
capturing). Precedent for shape: `in_flight_creates`
(`crates/shamir-engine/src/table/table_manager_index_mgmt.rs:630`, used for
`degraded_index_count()`) and the EXISTING `dropping_regular`/
`dropping_unique` sets already living on `IndexManager` itself (check their
exact type and where they're declared — mirror that, since your registry is
the same kind of "index name → in-flight DDL state" bookkeeping, just for
CREATE instead of DROP).

Per this repo's concurrency idiom (`CLAUDE.md`): `scc::HashMap` with
`THasher`, not `Mutex`. One entry per `name_interned` currently undergoing
an online build. The registry answers exactly one question: "is `name_interned`
currently in an in-flight online build?" — RAII-guarded insertion/removal
(register on Phase B start, remove on Phase D completion or abort) is out of
scope for THIS task (that's #1059's job, wiring the registry into the actual
build lifecycle) — this task only needs the registry data structure itself
plus a way to query/insert/remove it, with unit tests proving it behaves
correctly in isolation. Expose whatever minimal API #1059 will need (e.g.
`fn mark_build_in_flight(&self, name_interned: u64)`,
`fn is_build_in_flight(&self, name_interned: u64) -> bool`,
`fn clear_build_in_flight(&self, name_interned: u64)`).

## Part 2 — dirty-set capture inside the shared planning methods

Add a dirty-set structure to `IndexManager` (in-memory — see rationale
below), keyed by `name_interned` (one dirty-set per in-flight build), storing
`RecordId`s only (no values — per RFC v2 §2.3's operator-decided design;
Phase C will re-read at current version and recompute, so the SET's job is
just "which ids were touched", nothing more).

Modify `plan_record_created`, `plan_record_updated`, `plan_record_deleted`
(`index_manager.rs:2013-2038, 2105-2130, 2200+`): inside each method's loop
over `self.indexes.iter()`, for EACH `def`:
- If `def.state == IndexState::Building` AND `is_build_in_flight(def.name_interned)`:
  add the `RecordId` to that def's dirty-set. Do NOT produce a
  `SetPosting`/`RemovePosting` op for this specific def (skip it in the
  returned `Vec<IndexWriteOp>` — other `Ready` defs in the SAME loop still
  produce their ops normally).
- Otherwise (def is `Ready`, or `Building` but NOT yet in-flight-registered):
  produce the op as today, unchanged.

**Why "Building AND in-flight" and not just "Building"**: a `Building` def
that hasn't reached Phase B yet (registered at Building, but Phase B's
barrier hasn't run) needs today's direct-write behavior (this mirrors the
"gets delta catch-up for free" mechanism that's still correct for the
narrow window before Phase B starts). Only once Phase B registers the
build as in-flight does dirty-set capture take over. (#1059 owns exactly
when that registration happens — this task just needs the conditional to be
correct given the registry's state, not to drive the registry itself.)

**Atomicity — the requirement that must not be silently violated.** If a
write commits but its `RecordId` doesn't land in the dirty-set, that
row's posting is permanently and silently missing from the built index.
Since the capture now lives INSIDE the single planning method that BOTH the
tx-commit path (`pre_commit.rs::rederive_base_index_ops_post_stage`, which
calls `plan_record_updated`/`plan_record_created` directly) and the ordinary
stage-time path (`table_manager_tx_ops.rs::plan_base_index_insert_ops`
etc., which ALSO call these same methods) and the non-tx path
(`table_manager_crud.rs`'s `on_record_created`/etc., which call the
`on_record_*` wrappers that internally call `plan_record_*`) all invoke —
the capture is atomic with respect to whichever caller triggers it, AS LONG
AS the caller applies the returned ops (or, for the dirty-set branch, the
capture already happened synchronously inside the call) atomically with the
record's own commit. Verify this holds for all three callers — if any
caller could theoretically call `plan_record_created` and then NOT actually
commit the underlying write (e.g., an aborted tx), confirm the dirty-set
entry for that record doesn't leak into a FUTURE successful write's data
(should be harmless either way, since Phase C just re-reads CURRENT state —
an over-inclusive dirty-set costs a wasted re-read, not a correctness bug;
state this explicitly rather than assuming it).

## Storage — in-memory, decided and documented, not left implicit

Per RFC v2 §2.3/§4.2: in-memory is sufficient for slice 1's conservative
restart-from-scratch crash-recovery policy (a crash loses the dirty-set, but
also always restarts Phase A from scratch per #1060's design, so nothing
needs the set to survive a crash in slice 1). Do NOT build a durable
`info_store`-backed version — that's explicitly deferred (resumable Phase-C
recovery is a slice-2+ optimization per the RFC). Write a one-line comment
on the dirty-set field stating this decision and citing RFC v2 §4.2.

## Tests (TDD)

1. Index not in the in-flight registry → dirty-set never grows, `SetPosting`/
   `RemovePosting` still produced exactly as before this change (regression
   guard against accidentally changing today's behavior for every index NOT
   mid-build).
2. Index IS in the in-flight registry (manually register it in the test) →
   a write touching that index's fields adds the `RecordId` to its
   dirty-set, and does NOT produce a `SetPosting`/`RemovePosting` for that
   specific def.
3. A write that does NOT touch the in-flight index's fields (e.g., the
   index is on field `a`, the write only sets field `b`) → the `RecordId`
   does NOT land in the dirty-set (otherwise Phase C degrades into a full
   rescan of every write, defeating the point of a *dirty* set).
4. Two indexes on the same table, ONE in-flight (Building + registered) and
   ONE `Ready` → a write touching both fields: the `Ready` index gets its
   normal `SetPosting`, the in-flight index's `RecordId` goes to its
   dirty-set — confirming a build in progress for one index does not
   degrade live support for a sibling.
5. Exercise this through at least TWO of the three callers found by the
   audit (pick, e.g., a stage-time `plan_base_index_insert_ops`-style path
   AND a direct `on_record_created`-style non-tx path) — not just
   unit-testing `plan_record_created` in isolation. Check existing test
   files in `crates/shamir-index/src/base_index/tests/`
   (`index_manager_tests/`) for the right level to add these, and
   `crates/shamir-engine/src/table/tests/` if a caller-level test fits
   better there instead.

## Boundaries

- Regular/hash family (`IndexManager`) only. Do NOT touch
  `SortedIndexManager`, unique-family-specific logic beyond what
  `plan_record_created`/`updated`/`deleted` already share with regular, or
  index2.
- Do NOT wire the registry into the actual `create_index`/Phase A-D
  lifecycle — that's #1059. This task delivers the registry + capture
  mechanism, proven in isolation.
- Do NOT change `SetPosting`/`RemovePosting`'s wire format or `Provenance`.

## Gate before you report done

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-index
./scripts/test.sh -p shamir-engine
```

Report the exact diff, the exact new test names, and confirm the 5 required
scenarios above are each covered by name — don't paraphrase.
