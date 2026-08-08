# Brief — #1038: P0-3a Slice 3 — lease-based drain guard for index2 (`IndexRegistry`)

## Context

S.H.A.M.I.R. Database. Final slice of the P0-3a effort. Slice 1 (#1011)
closed a DROP-vs-reader race for the regular hash-index family using a
chokepoint `ReaderDrainGate`. Slice 2 (#1037, just landed) mirrored that
for the sorted family — but its FIRST attempt shipped a serious bug (back-off
signal indistinguishable from "genuinely empty," caught by zero-trust
review before commit, fixed by introducing `DbError::IndexDrainInProgress`
in `shamir-storage` and updating every real caller to fall back correctly).
**Read that fix and its commit message in full before starting this task**
— `git log --oneline --all | grep 1037` to find it, then `git show <sha>`.
The exact same failure class is possible here; this brief is written to
prevent a repeat.

index2 (`crates/shamir-index/src/registry.rs`, `IndexRegistry`) is
structurally different from the other two families: resolve and read are
ALREADY connected through an `Arc<dyn IndexBackend>` handle (`find_by_field_and_kind`/
`get_by_name` hand callers a clone of the `Arc`, not a fresh lookup each
time) — so the fix here is a LEASE (the `Arc` bundled with a drain-guard
that outlives the caller's read), not a chokepoint-scan gate like the other
two families.

## Already investigated — three findings, verify before implementing

1. **The task's own proposed scope (`get_by_name` also renamed to
   `lease_by_name`) is likely WRONG — I checked every real (non-test)
   caller and found none of them are read-dispatch.** All 5 non-test call
   sites of `get_by_name` are DDL-writer or introspection/existence-check
   style, not "resolve a backend to read its live data":
   - `table_manager_index_mgmt.rs:973` — inside `drop_index2` itself,
     resolving which backend to drop WHILE ALREADY HOLDING
     `begin_write_barrier` exclusivity. This is the WRITER side — gating
     it with a reader-lease would be nonsensical (at best redundant, at
     worst self-deadlock-adjacent).
   - `table_manager_index_mgmt.rs:1372`, `:1559`, `:1598` — `.is_some()`
     existence/classification checks (DROP admission's `index2_exists`,
     RENAME's family classification, RENAME's destination-occupied
     check). These need the TRUE existence state regardless of a
     concurrent drop's drain window — gating them could make DDL
     admission/collision logic reach a WRONG conclusion (e.g. RENAME
     believing an index2 backend doesn't exist because it happens to be
     mid-drain, when it structurally still does).
   - `doctor.rs:402` — a registry CONSISTENCY check (`by_id`↔`by_name`
     round-trip), touches only `.descriptor().id`, never reads index
     data. Mirrors the exact reason `begin_write_barrier` was reverted
     from `doctor::verify()` (#1011's own history) — a read-only
     diagnostic must not be blocked by an unrelated in-flight DDL op.

   **Verify this classification yourself** (don't take my word for it —
   re-read each call site) before deciding. If confirmed: leave
   `get_by_name` completely untouched, do NOT introduce a `lease_by_name`
   that nothing calls. Only `find_by_field_and_kind` has genuine
   read-dispatch callers (`read_exec.rs:1760` — vector similarity lookup;
   `read_planner.rs:48/72/90` — fts/vector/functional backend acquisition
   for a real read) and needs the lease treatment / rename to
   `lease_by_field_and_kind`.

2. **The "blocked" signal needs THREE states here, not two — reuse
   `DbError::IndexDrainInProgress` from #1037, don't invent a parallel
   mechanism.** `find_by_field_and_kind` currently returns
   `Option<Arc<dyn IndexBackend>>` where `None` has a PRE-EXISTING, real
   meaning: "no fts/vector/functional index registered for this field" —
   every caller already treats that as "fall through to a different plan"
   (confirmed: `read_exec.rs`'s vector case explicitly documents "No
   vector index on this field — fall through to the legacy full-scan
   path"; `read_planner.rs`'s three arms are `?`-chained inside a
   `try_X`-shaped function whose own `None` result means "this filter
   shape isn't index2-servable, try something else"). Collapsing "gate
   blocked" into that SAME `None` would repeat #1037's round-1 mistake —
   BUT reusing `None` might ALSO be fine if you can show every real
   caller's fallback path is still CORRECT (not silently wrong) when
   triggered by a blocked-but-actually-existing index, the same rigor
   #1037 required. **Recommended design** (verify/adjust, don't blindly
   copy): change the signature to `DbResult<Option<BackendLease>>` —
   `Ok(None)` keeps its pre-existing "genuinely no such index" meaning,
   `Err(DbError::IndexDrainInProgress(_))` is the NEW distinguishable
   "found it, but don't trust a read right now" signal, `Ok(Some(lease))`
   is the success path. This is the same pattern #1037 used, extended by
   one state. Trace EVERY real caller (the 4 named above) to confirm each
   one's `Err(IndexDrainInProgress)` handling lands on a genuinely correct
   fallback (full scan, or the documented "unranked residual filter"
   degradation for vector — re-verify that degradation is actually
   correct-but-slower, not silently-wrong, before relying on it).

3. **`doctor.rs:402`'s `get_by_name` call is unaffected by finding 1**
   (it's not being changed), but double check there isn't a SEPARATE
   index2 consistency check elsewhere that DOES read backend data through
   `find_by_field_and_kind` — if so it needs the same read-only-diagnostic
   exemption `verify()`'s regular/sorted entry_count paths already
   establish (don't gate diagnostic reads; #1011/#1037 precedent).

## What to implement

Per the task's own design (verify against the current tree, some details
above already update it):

1. **`BackendLease` type** — bundles `backend: Arc<dyn IndexBackend>` with
   an RAII drain-guard (`_guard: OwnedReadGuard` or whatever the actual
   gate primitive's guard type ends up being called — check
   `crate::reader_drain_gate::ReaderDrainGate`'s existing guard type from
   slices 1/2 first, reuse it rather than inventing a new one if the
   shape fits).
2. **`IndexRegistry` gets its own `reader_gate: ReaderDrainGate` field**
   (same construction/clone pattern as `IndexManager`/`SortedIndexManager`).
3. **`find_by_field_and_kind` → `lease_by_field_and_kind`**, gated as
   described in finding 2. Rename makes the drain-awareness visible at
   every call site (per the task's own stated rationale) — update the 4
   real callers accordingly.
4. **`drop_index2` writer-side wiring**: `begin_drop()` before
   `remove_by_id`, `wait_for_drain().await` before whatever bulk-sweep
   step corresponds to `drop_all`/postings removal — mirror slices 1/2's
   2.5/3.5/4.5 placement exactly (raise BEFORE the RCU-equivalent
   registry mutation, drain BEFORE the physical sweep, release after).
5. **`all_backends()`/`backends_newer_than()` stay UNGATED** — held for a
   whole transaction's duration; leasing those would stall every DROP
   INDEX on the table for as long as the longest concurrent tx. This is
   explicit, load-bearing, per the task's own description — do not gate
   these.
6. **Test pause hooks may stay `#[cfg(test)]`-gated** (per the task
   description) — index2's hooks are not a cross-crate test consumer,
   unlike `shamir-index`'s hooks used by `shamir-engine`'s test suite.
7. **Close the "KNOWN GAP" doc comment** in
   `crates/shamir-index/src/base_index/index_manager.rs` (~line
   1723-1746, `drop_index`'s doc comment) — it currently reads "SORTED
   family (#1037) and index2 (#1038) — STILL OPEN". After this slice,
   update it to reflect all four families closed (find the exact wording
   convention slice 2 used for its own "CLOSED" update, mirror it for
   index2).

## Tests

Mirror slices 1/2's test shape (`p1011_reader_drain_tests.rs`/
`p1037_sorted_reader_drain_tests.rs` as templates), adapted for the lease
API:

1. **Proof test**: parked read holds the lease → spawned `drop_index2`
   blocks in `wait_for_drain` → release read → drop completes → sweep
   verified to run only after.
2. **Distinguishable-signal test** (THE lesson from #1037's round 1): a
   `lease_by_field_and_kind` call during the drain window must return
   `Err(DbError::IndexDrainInProgress(_))`, NOT `Ok(None)` — write this
   test to explicitly panic with a message naming the bug class if it
   ever regresses to the wrong signal, same style as #1037's rewritten
   test.
3. **Back-off pairing**: `drain_waits() == 0` uncontended /
   `drain_waits() == 1` racing, paired (lone `==0` is vacuous — #1005's
   defect class, don't repeat it).
4. **Guard-release-on-error test**: force a lease-holding read to error,
   confirm RAII release.
5. **Caller-side regression tests** for at least the vector-similarity
   path (`read_exec.rs`) and one `read_planner.rs` arm (fts or
   functional): prove the query-level fallback during a drain window is
   CORRECT (degrades to the documented residual-filter/full-scan
   behavior), not silently wrong — this is the caller-verification #1037
   required, adapted to index2's specific fallback semantics.
6. Regression sweep: full `shamir-index`/`shamir-engine` suites.

## Constraints

- Follow `CLAUDE.md`: `Result<T, E>` conventions, tests in `tests/`
  directories, imports at top of file, one-file-one-primary-export.
- Gate: `cargo fmt -p shamir-index -p shamir-engine -- --check`, `cargo
  clippy --workspace --all-targets -- -D warnings`, `./scripts/test.sh -p
  shamir-index -p shamir-engine --full`. Use the wrapper, never raw
  `cargo test`/`cargo nextest run`.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.
⛔ Do not create scratch files at the repo root.

## Definition of done

- [ ] Verified (or refuted) finding 1 — `get_by_name`'s callers are all
      non-read-dispatch; `lease_by_name` NOT introduced unless refuted
      with evidence.
- [ ] Verified (or refuted) finding 2 — every real caller of
      `lease_by_field_and_kind` traced to a genuinely correct fallback on
      `Err(IndexDrainInProgress)`, not just "compiles and doesn't panic."
- [ ] `IndexRegistry` has its own `reader_gate`; `lease_by_field_and_kind`
      returns `DbResult<Option<BackendLease>>` (or a justified alternative
      if you disagree with this shape after investigation).
- [ ] `drop_index2` wired with the 2.5/3.5/4.5 pattern.
- [ ] `all_backends()`/`backends_newer_than()` explicitly left ungated.
- [ ] "KNOWN GAP" doc comment in `index_manager.rs` updated to reflect all
      four families closed.
- [ ] Full test suite per the "Tests" section, including the
      distinguishable-signal regression test naming the #1037 bug class.
- [ ] fmt/clippy/test gates green, real output reported.
