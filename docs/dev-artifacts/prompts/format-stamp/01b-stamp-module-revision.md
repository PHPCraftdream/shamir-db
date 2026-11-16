# Brief: format-stamp module, revision 1 (review findings)

Follow-up to `01-stamp-module.md`, same worktree and rules (no builds or tests; only fmt, clippy `-p shamir-sync
--all-targets`, `check_layout`; no git mutation; final report; no `#[allow]`/`#[expect]`, no function-local `use`, mod.rs
manifests only). Three review findings to fix:

1. **`tests/table/table_invariants_tests.rs`, `table_cross_product_is_total`** asserts `outcome.is_ok() || outcome.is_err()`,
   a tautology. Rewrite it so that each of the 18 combinations (stamp state x directory state x epoch relation, Writer mode)
   carries an EXPECTED result taken from the decision table of design 3.4 (the row number in a comment, the expected `Ok`
   outcome kind or the exact `StampError` variant), and assert that expected result; a combination the table marks impossible
   is listed with an explicit marker and asserted unreachable by construction (for example by the epoch policy constructor
   refusing it) rather than skipped. Check the epoch relations against `EpochPolicy::new` and the fixture `policy` helper: a
   combination the policy cannot express must be handled explicitly, not silently passed. Keep the test deterministic and
   OS-independent.
2. **`epoch_invariants_hold`**: the change-log id uniqueness check compares `ids.len()` with `FORMAT_CHANGES.len()`, which is
   always true. Replace it with a real uniqueness check (collect into a set and compare lengths, with a `TSet` or sorted
   adjacent comparison; no `std::collections::HashMap`/`HashSet` per the workspace hasher rule).
3. **Log lines of design 3.4**: rows 3, 5a and 15 specify an info line on stamp creation and warn lines for adopted
   stampless data and for a replaced corrupt stamp. The module has no logging. Add them inside `check_data_dir` with
   `tracing` (a dependency of the crate already) using the message texts of the design table; they carry no secrets and no
   absolute machine paths beyond what the design table prints. Add no new test that asserts on log output unless a log-capture
   helper already exists in the crate's tests; otherwise state in the report that the log lines are untested.

No other change. Report what you changed, and what CI must show.
