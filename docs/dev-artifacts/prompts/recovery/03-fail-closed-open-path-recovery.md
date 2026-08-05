# Brief — R0-D: fail-closed open-path index recovery (#1013 + #1023)

## Context

S.H.A.M.I.R. Database, `crates/shamir-engine` + `crates/shamir-index`. Two
independent readonly reviews of the `#957-#1005` wave (both committed at
`docs/dev-artifacts/research/2026-08-05-new-wave-readonly-review*.md`) found the
same defect class, cross-checked against current source and recorded in
`docs/dev-artifacts/roadmap/2026-08-05-release-blocker-execution-map.md` §R0-D:
**open-path recovery is fail-open where it must be fail-closed.**

For a database, a silently-wrong result is worse than refusing to open. Today two
code paths log a warning and continue as if recovery succeeded.

## Task 1 — startup recovery must not leave a broken backend `Ready` (#1013)

### The two fail-open sites (verified, read them before touching anything)

1. `crates/shamir-engine/src/table/table_manager.rs:483-585` — inside the index2
   Building self-heal loop:
   - `:491-503`: if `backend.drop_all().await` fails during the "was Building —
     restart from scratch" self-heal, the code logs a warning
     ("partial postings may persist") and proceeds to backfill anyway.
   - `:579-585`: in the general `restore_on_open` loop, if `b.restore_on_open(...)`
     returns `Err`, the code logs a warning and moves on — the backend stays
     registered with whatever in-memory state it had (for a freshly-constructed
     adapter, that's empty/incomplete), and nothing downstream knows it failed.
2. Contract: `crates/shamir-index/src/backend.rs:250-276` (`restore_on_open`'s
   doc comment — read it, your fix must honor whatever contract already exists
   there and update it if the contract itself needs to state the new behavior).
3. `crates/shamir-index/src/vector/vector_backend.rs:637` — vector's
   `restore_on_open` override: if HNSW snapshot restore fails and it falls back to
   a full rebuild, and THAT also fails, the backend can end up registered but
   effectively empty, silently returning incomplete search results forever.

### Why this matters concretely

For FTS specifically, a partial `drop_all` failure followed by backfill means
stats are not a clean overwrite — the index can end up double-counting
statistics. For vector, an empty adapter marked `Ready` returns empty/incomplete
search results with no error signal at all — a client cannot tell the difference
between "no matches" and "the index silently failed to load".

### Fix

1. Add a `Failed` variant to `IndexState`
   (`crates/shamir-index/src/state.rs:50-60`). **This is safe**: the existing doc
   comment on the enum already says appending a variant is backward-compatible
   (bincode tags variants by ordinal; only reordering is breaking) — confirmed by
   reading the comment, do not second-guess it, just add the variant with a doc
   comment explaining when it's set.
2. On a failed `drop_all` during Building self-heal: do NOT proceed to backfill
   and flip to `Ready`. Set the backend's state to `Failed` instead (same
   mechanism `set_state` already uses for `Building -> Ready`, e.g.
   `registry.rs`'s `set_state`/`state_of` — reuse it, do not add a parallel path).
3. On a failed `restore_on_open`: same — the backend's tuple-slot state becomes
   `Failed`, not left at whatever it was.
4. `Failed` MUST be planner-invisible, exactly like `Building` already is
   (find the existing `state != IndexState::Ready` gates the planner uses for
   `Building` and extend them to also exclude `Failed` — do not special-case
   `Building` and forget `Failed` in any of those gates).
5. **Reuse, do not rebuild, the existing degraded-index observability**: the
   `shamir_degraded_indexes_total` gauge (`crates/shamir-engine/src/table/degraded_index_count.rs`,
   built in #984/#1003/#1005) already counts any definition whose state is not
   `Ready` (excluding a specific in-flight CREATE via `InFlightCreateSet`). A
   `Failed` index is not in-flight, so it will automatically be counted as
   degraded once you set the state correctly — you should NOT need to touch
   `degraded_index_count.rs` at all. If you find yourself editing it, stop and
   re-check your assumption; that's a signal you've misunderstood the existing
   gate.
6. `doctor::verify()` (wherever it inspects index state — find it, it's the same
   contour the degraded gauge's poller reads from) should report the `Failed`
   reason if one is available (propagate the underlying error string into the
   entry, not just the enum variant).

### Tests (write these — they must fail against the reverted code)

- A deterministic test that makes `drop_all` fail during Building self-heal
  (inject via whatever test-double/fault-injection mechanism the existing
  `index_manager` tests already use for backend failures — check
  `crates/shamir-index/src/tests/` and `crates/shamir-engine/src/table/tests/`
  for an existing pattern before inventing a new one) and asserts the resulting
  state is `Failed`, NOT `Ready`.
- A deterministic test that makes `restore_on_open` fail (vector AND a
  non-vector family, e.g. FTS) and asserts the index is NOT planner-visible
  (i.e. a query that would use it does not select it, or an explicit
  `state_of()` check returns `Failed`).
- A test that a `Failed` index is counted by `degraded_index_count()` (reusing
  the existing test file `crates/shamir-engine/src/table/tests/degraded_index_count_tests.rs`
  — add a case, don't duplicate the harness).

## Task 2 — unique-index backfill must not silently skip corrupt records (#1023)

### The site (verified)

`crates/shamir-index/src/base_index/index_manager_unique.rs:372-380` — during
`create_unique_index`'s backfill, a record whose key is not exactly 16 bytes, or
whose value fails to decode, is skipped with a `continue`. That row never gets a
unique posting. Its "occupied" state is invisible to later duplicate-detection —
a later insert with a colliding value may be wrongly accepted because the
original occupant's posting was never written.

This directly contradicts the fail-closed policy `#960` already established
elsewhere in this same file for a corrupt EXISTING unique posting (length != 16
bytes there returns a typed `DbError::Codec`, not `Ok(None)` — read that code
right above/near the backfill loop as your template for the fix's error type and
tone).

### Fix

Minimum viable, and what to actually ship given this is alpha and backfill
already returns a `DbResult`: turn the silent `continue` into a typed error that
aborts the backfill (same `DbError::Codec` variant `#960`'s fix already uses one
data-fetch aisle over — do not invent a new error variant if that one already
fits). If a genuine reason exists to keep tolerating some skips (check whether
any existing test or caller relies on lenient backfill before deciding), fall
back to a skip counter + `log::warn!` whose count is surfaced through
`verify()` — but the typed-error path is the default expectation unless you find
a concrete reason it breaks an existing caller.

### Tests

- Backfill over a table containing one record with a malformed key (not 16
  bytes) — assert the backfill returns an error (or, if you took the
  skip-counter fallback, assert the skip is visible in `verify()`'s output) and
  does NOT silently produce a unique index missing that row's posting.
- Backfill over a table containing one record whose value fails to decode —
  same assertion.

## Constraints (both tasks)

- Follow `CLAUDE.md`'s code ideology: no new `std::sync::Mutex`/`RwLock` on hot
  paths, `Result`/`?`/`thiserror`, no new files unless genuinely needed, tests
  live under the crate's existing `tests/` directory convention (check
  `crates/shamir-index/src/tests/mod.rs` and
  `crates/shamir-engine/src/table/tests/mod.rs` for how to wire a new test file
  in — manifest-only `mod.rs`, no inline `#[cfg(test)] mod tests { ... }`).
- Gate: `cargo fmt -p shamir-index -p shamir-engine`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `./scripts/test.sh @oracle @storage` (or the narrower `-p shamir-index -p shamir-engine --full`
  if that's faster) must be green before you report done.
- Do not touch `degraded_index_count.rs` unless you can point to exactly why the
  existing gate doesn't already cover `Failed` — it almost certainly does once
  the state is set correctly.
- Do not expand scope into R0-A/B/C (registry generation, rename, DDL admission)
  — those are separate tasks (#1006-#1010, #1012) with their own briefs. Stay
  inside index-state/recovery/backfill.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or
any git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## Definition of done

- [ ] `IndexState::Failed` added, doc-commented, backward-compatible (append only).
- [ ] Both fail-open sites in `table_manager.rs` set `Failed` instead of
      continuing to `Ready`.
- [ ] Planner-visibility gates exclude `Failed` everywhere they already exclude
      `Building`.
- [ ] `degraded_index_count()` counts `Failed` indexes with NO changes to
      `degraded_index_count.rs` itself (or a one-line justified change if truly
      needed — explain why in the final report).
- [ ] Unique backfill aborts (or, with justification, counts+warns) on a
      malformed key/undecodable record instead of silently skipping.
- [ ] New deterministic tests for all of the above, each shown to fail against
      the pre-fix code (describe how you confirmed this in your final report).
- [ ] fmt/clippy/tests green.
