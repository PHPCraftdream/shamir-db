# TOKEN-AUTH1 tails 4 - pin budget by unique buckets (third attempt, design fixed by the orchestrator)

Base: master. Two earlier attempts were reverted (see `docs/dev-artifacts/audits/2026-11-02-token-auth1/DESIGN.md`, "Tails 2 review corrections" and "Tails 3 review corrections" - read both first): the first counted leases instead of buckets and refused legitimate traffic at about 512 concurrent logins; the second passed two closures to `AtomicUsize::fetch_update` (a compile error), decided the reservation from a separate marker load (a race against `BudgetPin::drop`), rolled back by a per-call flag and left tests with impossible expectations. Files: `crates/shamir-server/src/security/perimeter/http_login/admission/{table.rs,state.rs}`, `service/begin.rs` (caller of `pin_source`), tests under `admission/tests/`. CLAUDE.md applies (layout: new test code goes into existing test files or a new subfolder - `admission/tests/` entries must stay at most 7; at most 1000 lines per file; no raw std `HashMap/HashSet`; clippy `unused_io_amount`, `type_complexity`, `needless_borrows_for_generic_args`, `result_large_err`).

## Goal

The number of UNIQUE pinned buckets (input pins plus pending-entry pins) of one admission table can never exceed 3,072 of its 4,096 buckets, so a flood of slow requests can never pin the whole table and switch the per-source rate limit off. A refusal fails closed (`ServerBusy`, existing mapping) and records `record_budget_drop("pin_budget")`. A source that already pins its bucket must never be refused (its FINISH costs nothing).

## Design (follow it exactly; deviations only with evidence in the audit)

Each bucket has a marker `AtomicUsize` (pin count); the table owns a `pinned_buckets: AtomicUsize` (units) and `const PIN_BUDGET: usize = 3072`. There is NO lease object.

`try_pin_marker(marker, budget) -> bool` (the only place that raises a marker):
```
loop {
    let current = marker.load(Acquire);
    if current > 0 {
        // the unit of this bucket is already reserved by an existing pin
        if marker.compare_exchange(current, current + 1, AcqRel, Acquire).is_ok() { return true; }
        continue;
    }
    // 0 -> 1: reserve the unit BEFORE publishing the marker
    if budget.fetch_update(AcqRel, Acquire, |u| (u < PIN_BUDGET).then_some(u + 1)).is_err() { return false; }
    if marker.compare_exchange(0, 1, AcqRel, Acquire).is_ok() { return true; }
    budget.fetch_sub(1, AcqRel);   // lost the race: give the unit back and retry
}
```
`unpin_marker(marker, budget)`: `if marker.fetch_sub(1, AcqRel) == 1 { budget.fetch_sub(1, AcqRel); }` - used by `BudgetPin::drop` for every marker it holds. `Clone for BudgetPin` only increments each held marker with `fetch_add(1)` (a clone never changes the 0/1 state: the original keeps the marker above zero). A pin over several buckets (arrival and credit tables, parent tiers) that is refused halfway releases the markers it already raised through `unpin_marker` (the same function as Drop - build the `BudgetPin` incrementally and drop it on refusal). Orderings: the budget counter and the markers use AcqRel/Acquire as above; this is conservative: the budget may exceed the true count by one for a moment between a marker reaching zero and its unit being released, never fall below it.

`pin_source` returns `Option<(BudgetPin, BudgetPin)>` (None = refused); `begin.rs` maps None to its existing `busy()` reply, the admission `admit` path maps a refusal to `AdmissionError::ServerBusy`. The existing fail-closed `Dropped -> ServerBusy` of the debit stays.

## Tests (they must really exercise the cap; write a helper `pin_n_distinct_buckets(table, n)` that returns the pins)

1. Exactly 3,072 distinct buckets can be pinned; the 3,073rd is refused; the counters stay unchanged after a refusal.
2. Pinning the same bucket many times costs one unit; dropping all but one keeps the unit; dropping the last releases it (`pinned_buckets` returns to the previous value).
3. `Clone` then drop of the original and of the clone releases exactly once (no underflow: assert the counter is back to 0).
4. At the cap, a source that already holds a pin on a bucket is still admitted for another pin on the same bucket (no refusal); a new bucket is refused.
5. A multi-bucket pin refused halfway leaves every counter and marker as before.
6. A loom-free stress test: 8 threads pin and drop random buckets 10,000 times each; at the end the counter is 0 and it never exceeded the cap (record the maximum with `fetch_max` in a shadow counter read after each successful pin).
7. The existing full-table and eviction tests keep their meaning: after your change read each and make sure its expectation still follows from the real code (a full table no longer implies that no bucket is evictable; unpinned buckets are candidates).

## Rules

- NO LOCAL COMPILATION OR TESTS: the only commands allowed are `cargo fmt -p shamir-server` (and `-- --check`) and `python -I scripts/ci/check_layout.py`. No `cargo check/clippy/build/test`, no rustc. A child that runs them violates the policy (three children already did and left 2 GB `target/` directories): say so in every child prompt. No `Cargo.toml`, lockfile, workflow edits.
- Nothing is compiled locally: finish with a compiler-style read of every file you touched, especially the exact signature of `AtomicUsize::fetch_update(set_order, fetch_order, FnMut(usize) -> Option<usize>)` (ONE closure, two orderings first), `compare_exchange(current, new, success, failure)`, ownership of values moved into closures, struct literals with all fields in test constructors, and the callers of every changed signature across `crates/`.
- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index (read-only `git show`/`git log`/`git diff` is fine). Only edit files; the orchestrator commits. No machine-local absolute paths. Do not leave stray empty or status files in the tree.
- Children only edit files, are awaited, every touched file re-read as the compiler would. Never finish with a child outstanding. The session has a hard 150-minute limit: stop editing 40 minutes before the time in the run file, never leave a half-edited item (revert it by editing back and record it as BLOCKED), append the per-item report to the audit under "Tails 4".
