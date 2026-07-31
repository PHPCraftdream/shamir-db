# F-75 (#902) — fix F-65's grandchild test: invalid oracle leaves sites 2/3 unverified

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Only edit files;
the orchestrator commits.

## The bug — a defect in a test WE shipped, not in production code

`cascade_grandchild_recursion_propagates_read_error`
(`crates/shamir-engine/src/query/batch/tests/fk_indexed_action_read_error_tests.rs`,
currently around line 398) is meant to prove F-65 (#891, commit
`28d39f31`) closed fast-path re-read **sites 2 and 3** (the
grandchild-recursion index fast path in `plan_cascade_for_ids`, and the
grandchild ref-field-value collection loop — both in
`crates/shamir-engine/src/query/batch/fk_actions.rs`).

The test builds a 3-level self-referential hierarchy (`employees.manager_id
-> employees.id`, CEO(1) <- Mgr(2) <- Worker(3)) and calls
`arm_failure_for_all_rows(&resolver, "employees").await` — which arms a
one-shot injected `read_one_tx_bytes` failure for **every row in the
`employees` table**, including the CEO/Mgr rows that the CASCADE's
**direct-child re-read (site 1)** touches FIRST, before recursion into
sites 2/3 ever happens. Deleting the CEO triggers site 1's re-read of Mgr
— which is already armed — so the whole operation aborts with `Err` right
there. The test's `assert!(result.is_err(), ...)` passes, but sites 2 and
3 are never reached. The test's name and its module doc both claim
coverage they do not actually provide.

**This is the exact same class of defect** caught and fixed during F-65
itself in `on_update_index_fast_path_propagates_read_error` (a test
passing for the wrong reason — an armed-everywhere injector makes
`is_err()` true regardless of which specific site actually failed). That
fix was applied to one test and missed this sibling.

**The module doc itself (top of the test file, `## Fault-injection
strategy` section) currently asserts the arm-everything strategy is
sound**: "Each scenario enumerates every `RecordId` currently in the
target child table... and arms all of them, so whichever candidate id(s)
the index fast-path selects for re-read are guaranteed to hit the injected
failure — no need to predict which specific id the fast path will pick."
That reasoning is exactly what produced the invalid oracle for THIS
specific test (a self-referential, multi-level recursion where "the
target table" is touched at every recursion depth, not just once) — it
must be corrected, not just this one test's body.

## The fix

1. Make the oracle discriminating for the grandchild-recursion test.
   Either:
   - Arm the injected failure ONLY for the specific record id(s) that
     sites 2/3 actually re-read (the Worker row, id 3, or whichever id(s)
     `plan_cascade_for_ids`'s recursion step and the ref-field collection
     loop touch) — leave the direct-child (site 1) id(s) unarmed so the
     delete can genuinely proceed past site 1 and reach the recursive
     call; or
   - Assert on the returned error's message/code to prove WHICH site
     failed, not merely that something did. The production error strings
     are already site-specific and exist verbatim today:
     `"fk_actions: grandchild index fast-path re-read failed: {e}"` (site
     2, `fk_actions.rs` ~line 796) and `"fk_actions: grandchild ref_field
     collection re-read failed: {e}"` (site 3, `fk_actions.rs` ~line 651)
     — asserting the returned error's `to_string()`/`Display` contains the
     expected substring is straightforward and turns the oracle honest.
   You may combine both (arm only the relevant id AND assert the message)
   for the strongest proof; at minimum do one.
2. **Split into two tests, one per site** (site 2's grandchild
   index-fast-path re-read, site 3's ref-field collection re-read) since
   they have distinct messages and, in general, may need distinct setups
   to isolate deterministically (verify whether both can actually be
   provoked from the SAME 3-level hierarchy with different armed ids, or
   whether site 3 needs its own scenario — read `plan_cascade_for_ids`
   and the ref-field collection loop in full to determine this before
   writing the tests, don't guess).
3. Correct the module doc's `## Fault-injection strategy` section: the
   "arm every row in the target table" strategy is sound ONLY for a
   single-level (non-recursive) scenario where the whole table's rows are
   candidates for exactly one re-read site. State explicitly that a
   multi-level self-referential recursion (this grandchild scenario)
   needs per-id or per-message discrimination instead, and why.
4. Do not touch `fk_actions.rs`/`fk_on_update.rs` production code — this
   task is test-only. If reading the recursion logic reveals the
   production fix ITSELF has a gap (not just the test), stop and flag it
   explicitly in your final report rather than silently expanding scope.

## Definition of done — the check the ORIGINAL test could not pass

- With the shipped fix reverted at **site 2 ONLY** (temporarily changing
  that one `.map_err`/propagation back to the pre-F-65 `_ => continue`
  shape, leaving site 1 and site 3 fixed), the NEW site-2 test must FAIL.
  Restore and confirm it passes again.
- Repeat independently for **site 3 ONLY**.
- Do this sabotage-then-restore cycle yourself and report exactly what
  you did and observed for each site — this is the proof that was missing
  before (the old test could not distinguish "site 1 caught it" from
  "site 2/3 caught it," so it could never have failed this way).
- `cargo fmt -p shamir-engine -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `./scripts/test.sh -p shamir-engine --full` green.
- Cheap, narrowly-scoped task: do not restructure the rest of the test
  file, do not touch sites 1/4's tests, do not touch production code.

Commit with `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>` per
repo convention.
