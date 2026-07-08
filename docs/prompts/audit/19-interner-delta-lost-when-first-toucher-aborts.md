Task: MEDIUM-concurrency — interner-delta is lost when the "first
toucher" of a new field name aborts before WAL, leaving a later
committer's records referencing an id the durable interner delta never
recorded (audit finding A8, `docs/audits/2026-07-06-concurrency-engine.md`).

## Where

- `crates/shamir-tx/src/id_remap.rs` — has `remap_value`/
  `remap_inner_value_bytes` (recursively rewrites `InternerKey` ids used
  as `InnerValue::Map` keys according to an overlay→base remap) but NO
  function that just *collects* the referenced ids without rewriting —
  this fix needs one.
- `crates/shamir-tx/src/layered_interner.rs`, `commit_interner_overlay`
  (~line 176-202): builds `OverlayCommitResult { remap, delta }` where
  `delta` contains ONLY `(name, id)` pairs for entries this call
  discovered as `is_new` (i.e. this committer happened to be the FIRST
  to intern that name into base this merge). If a concurrent, EARLIER
  committer already interned the same name into base (so `base.get_ind`
  returns `Some`, `is_new = false` for THIS committer), this committer's
  `delta` does NOT include that `(name, id)` pair — even though this
  committer's staged bytes reference that id.
- `crates/shamir-engine/src/tx/pre_commit.rs`, `pre_commit_prelock`
  (~line 165-200): calls `commit_interner_overlay`, extends
  `tx.interner_deltas` with the returned `delta`, then rewrites every
  touched table's staging bytes via `remap_inner_value_bytes` (using the
  overlay→base `remap`). This is the natural place to add: after the
  remap is applied, additionally scan the (now base-id-referencing)
  staged bytes for ANY `InternerKey` id `>= repo_interner.persisted_high_water()`
  that is not already in `tx.interner_deltas`, look up its name via the
  base interner, and add `(name, id)` to `tx.interner_deltas`.
- `crates/shamir-engine/src/table/write_exec.rs` (~line 120-155): the C5
  base-intern path (`intern_to_base == true`, used outside a
  multi-statement tx) has the SAME class of gap — `new_base_keys` is
  only populated when `ti.is_new()`, i.e. only for entries THIS call
  created. If the write pipeline for base-intern also needs this fix
  (check whether base-intern writes go through `pre_commit_prelock` too,
  or whether they have their own delta-recording path that also needs
  the same "record ids referenced above persisted hwm" treatment) —
  confirm and fix both, or explain why only one path is affected.
- `crates/shamir-engine/src/table/interner_manager.rs`,
  `persisted_high_water()` (~line 259-264): returns
  `last_persisted_len.load(Ordering::Acquire)` — since interner ids are
  1-based and dense, this IS the correct "highest already-durable id"
  upper bound. Any id STRICTLY GREATER than this value is not yet
  durably recorded in the persisted interner chunk store and MUST be
  covered by SOME committer's WAL-carried `interner_deltas`, or a crash
  before the next checkpoint makes it unrecoverable.
- `crates/shamir-tx/src/core/interner/interner.rs` (module path may
  differ — it's `crates/shamir-types/src/core/interner/interner.rs`),
  `get_str(&self, id: &InternerKey) -> Option<Arc<str>>` (~line 157):
  the accessor to resolve an id back to its name for building the
  `(name, id)` delta tuple.

## Why this is MEDIUM

**Concrete interleaving from the audit:**
1. tx1 and tx2 concurrently intern the new field name `"foo"` (each
   builds its own per-tx `interner_overlay` — an overlay is
   tx-scoped/in-memory, assigning its OWN provisional overlay id to
   `"foo"` independently in each tx).
2. tx1 reaches `commit_interner_overlay` FIRST: `base.get_ind("foo")` is
   `None` → `base.touch_ind("foo")` creates it in base as id 42,
   `is_new() == true` → tx1's `delta = [("foo", 42)]`.
3. tx1 **aborts before its WAL write** (e.g. SSI conflict detected in a
   later commit phase, or any abort path after `pre_commit_prelock` but
   before the WAL entry is durably appended) — tx1's `delta` is
   discarded with the rest of tx1's aborted state; the `(name, id)` pair
   is now ONLY in-memory in `base` (via `touch_ind`), never WAL-recorded.
4. tx2 reaches `commit_interner_overlay` AFTER tx1's `touch_ind` already
   ran: `base.get_ind("foo")` returns `Some(42)` → `is_new() == false`
   for tx2 → tx2's `delta` does NOT include `("foo", 42)`, even though
   tx2's staged record bytes reference id 42 (map key = interned "foo").
5. tx2 commits successfully: its WAL entry carries `interner_deltas`
   that do NOT mention id 42. `interner_delta_max_id` for tx2's
   materialization is `None` (or excludes 42) → the periodic interner
   checkpoint gate (`materialize.rs:318`, `interner_delta_max_id.is_some()`)
   has nothing forcing a flush of id 42 specifically.
6. **Crash occurs before ANY checkpoint has persisted id 42** (either
   because no checkpoint interval elapsed, or the in-memory `base`
   interner — which DOES know about 42 via tx1's `touch_ind` — is lost
   on crash since it was never flushed to the durable chunk store).
7. On recovery: WAL replay only knows about `(name, id)` pairs that were
   actually recorded in some committed tx's `interner_deltas`. Since NO
   committed tx's delta ever mentioned `("foo", 42)`, the persistent
   interner after recovery has no entry for id 42 — but tx2's committed
   records (which DID make it into WAL/history) reference id 42 as a
   map key. **tx2's records are now undecodable** — any read that
   decodes the record's `InnerValue::Map` needs to resolve interner id
   42 back to a field name and cannot.

## Fix

Per the audit's fix sketch: **every committer must include in its own
`interner_deltas` ALL `(name, id)` pairs referenced by its own staged
bytes that are ABOVE the interner's `persisted_high_water()`** — not
just the ids it happened to be the first to create. Since
`touch_with_id` (the recovery replay entry point,
`interner.rs:250` — confirm exact current line) is idempotent (replaying
the same `(name, id)` pair twice is a harmless no-op), it is always safe
for MULTIPLE committers' deltas to redundantly include the same pair —
correctness only requires that AT LEAST ONE surviving (WAL-durable)
committer's delta includes every id its own records reference above the
persisted floor.

Concretely, in `pre_commit_prelock` (`crates/shamir-engine/src/tx/pre_commit.rs`),
after the existing remap step (staged bytes now reference BASE ids, not
overlay ids):

1. Add a new function (likely in `crates/shamir-tx/src/id_remap.rs`,
   sibling to `remap_value`) that recursively walks an `InnerValue` (the
   SAME traversal shape as `remap_value` — `Map` keys, `List` elements —
   but collecting rather than rewriting) and returns the set of
   `InternerKey` ids used as `Map` keys. Something like
   `collect_referenced_ids(value: &InnerValue, out: &mut HashSet<u64, S>)`
   or returning a `Vec`/`HashSet` directly — match the existing module's
   style (it already takes a generic `S: BuildHasher` for the caller-
   supplied `THasher`).
2. For each table's staged bytes (after the existing
   `rewrite_set_bytes(|b| remap_inner_value_bytes(...))` call), also scan
   with the new collector to gather every referenced id across ALL
   staged values for this tx.
3. Fetch `repo_interner.persisted_high_water()` (confirm the exact
   accessor path from `pre_commit_prelock`'s existing
   `repo.repo_interner()`/`repo_interner.get()` calls — it may need to
   go through the `InternerManager` wrapper rather than directly on
   `Interner`; check `interner_manager.rs`'s public API for how a caller
   reaches `persisted_high_water()`).
4. For every referenced id `> persisted_high_water()` NOT already present
   in `tx.interner_deltas` (build a `HashSet` of ids already in
   `tx.interner_deltas` first to dedup cheaply), resolve its name via
   `base_interner.get_str(&InternerKey::new(id))` (confirm exact
   constructor — `InternerKey::new` per `id_remap.rs:33`) and push
   `(name.to_string(), id)` onto `tx.interner_deltas`.
5. This must run for EVERY table's staged bytes this tx touches, not
   just the one whose overlay entry happened to be `is_new` — a tx with
   NO new overlay entries at all (its overlay was empty, or every name
   in its overlay already existed in base) can still reference an id
   that's above the persisted floor (created by some OTHER, possibly
   still-aborting, concurrent tx) and must record it.
6. Confirm whether the `write_exec.rs` C5 base-intern path
   (`intern_to_base == true`) reaches `pre_commit_prelock` at all, or is
   a separate non-tx write path with its own delta-recording. If
   separate: apply the analogous fix there too (scan the record's own
   staged bytes for referenced ids above the persisted floor, not just
   `new_base_keys` populated from `ti.is_new()`). If it turns out
   base-intern writes are single-record and therefore ALWAYS reference
   only ids they just interned themselves (no cross-tx sharing possible
   in that path) — confirm this reasoning explicitly in your report
   instead of silently skipping the path.

Do NOT change `commit_interner_overlay`'s `is_new`-based `delta`
computation itself — it is still correct and useful (it's the cheap,
common-case path for genuinely-new names). This fix ADDS a second,
broader pass that also captures ids the committer merely *references*
regardless of who created them.

## TDD requirement

1. **Red**: write `#[tokio::test]`s (find or create the appropriate test
   module — check `crates/shamir-engine/src/tx/tests/` for existing
   interner-overlay/pre_commit related tests, or
   `crates/shamir-tx/src/tests/` for `layered_interner`/`id_remap`
   related tests, and follow the established layout) that:
   - Reproduce the exact interleaving: tx1 interns "foo" into base
     (via `commit_interner_overlay` or the higher-level pre-commit path),
     but do NOT let tx1's delta ever reach WAL/persistence (simulate the
     "aborts before WAL" step by simply not persisting/using tx1's
     discarded delta — the bug is entirely about tx2's delta, so the
     test can directly construct the "some other id already exists in
     base above the persisted floor" precondition without needing to
     actually run a full abort machinery, if that's simpler and equally
     valid — use your judgment on the cleanest way to set up the
     precondition: `base` has an entry above `persisted_high_water()`
     that did NOT come from THIS tx's own newly-created delta).
   - tx2 interns/references the SAME name (already present in base,
     `is_new() == false` for tx2), stages a record referencing that id,
     and runs through `pre_commit_prelock` (or the relevant lower-level
     function under test).
   - Assert that `tx.interner_deltas` (or whatever the delta-producing
     function returns) DOES include `(name, id)` for that id — this
     should FAIL before the fix (delta is empty/missing that pair) and
     PASS after.
   - A second test confirming the existing `is_new`-based fast path
     still works: a tx that IS the first to intern a brand-new name
     still gets `(name, id)` in its delta (regression guard — should
     pass both before and after).
   - A third test confirming NO duplicate/spurious deltas: a tx whose
     staged bytes only reference ids ALREADY below
     `persisted_high_water()` (already durably persisted) does NOT
     grow `interner_deltas` for those — only ids above the floor are
     added (avoids bloating WAL entries with redundant already-durable
     mappings).
2. **Green**: apply the fix.
3. Confirm existing interner-overlay / pre-commit / WAL-recovery tests
   still pass — this touches a core commit-path function, so run the
   full `shamir-engine` and `shamir-tx` suites, not just the new tests.

## Test scope command

```
./scripts/test.sh -p shamir-tx
./scripts/test.sh -p shamir-engine
```

## Gate (must be clean before finishing)

```
cargo fmt -p shamir-tx -p shamir-engine -- --check
cargo clippy -p shamir-tx -p shamir-engine --all-targets -- -D warnings
```

If clippy flags PRE-EXISTING lints in code you did not touch, do not
fix them here — note them in your final report instead.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

When done, report exactly:
- The exact new id-collection function/logic added, and where.
- Confirmation of where in the commit pipeline the "scan staged bytes
  for ids above persisted_high_water() not already in interner_deltas"
  pass was inserted, and why that point is correct (after remap, before
  the WAL entry is built).
- Whether the `write_exec.rs` C5 base-intern path needed the same fix,
  and what you found/did.
- The failing-then-passing test evidence for the core bug reproduction,
  plus the two regression-guard tests (fast-path still works, no
  spurious deltas for already-persisted ids).
- Confirmation existing interner/pre-commit/recovery tests still pass.
- Full test/gate results (exact commands + pass/fail).
