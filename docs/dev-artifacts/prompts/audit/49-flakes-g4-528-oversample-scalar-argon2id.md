Task: G4 (task #528) — three independent test flakes/failures found during
earlier verification passes this campaign. Investigate each, reproduce,
root-cause, and either fix the underlying bug or harden the test — per
this repo's standing rule, hangs/flakes are BUGS to hunt down, never
papered over with a raised timeout.

## Issue 1 — `trusted_pure_scalar_backs_functional_index` (STABLE FAILURE, not a flake)

`crates/shamir-db/src/shamir_db/tests/user_scalar_tests.rs::trusted_pure_scalar_backs_functional_index`
(currently ~line 99) fails DETERMINISTICALLY (not intermittently — it has
failed identically across at least 5 independent verification runs this
session, in under 0.1s each time, with no other test in its crate
affected). It panics at the assertion `assert_eq!(resp.results["result"].records.len(), 1, ...)`
(currently line ~161).

The test: registers a `trusted_pure()` custom scalar `my_upper`, inserts a
record `{name: "alice"}` BEFORE creating a functional index on
`my_upper(name)`, creates the functional index, inserts a SECOND record
`{name: "bob"}` AFTER the index exists (exercising the live
`FunctionalBackend` eval path via `IndexExpr::Scalar`), then queries with
`Filter::Computed { expr_op: "my_upper", field: ["name"], cmp: "eq", value: "ALICE" }`
and expects exactly 1 matching record (`alice`, since `my_upper("alice") ==
"ALICE"`).

Investigate: does the functional index actually get built/populated
correctly for the PRE-EXISTING record (`alice`) when `create_index` runs
(the backfill path), or does it only correctly index records inserted
AFTER the index exists (`bob`)? Or is the bug in the QUERY side — does
`Filter::Computed`'s eval path correctly route through the functional
index at all, or does it fall back to something that doesn't match? Add
diagnostic assertions/prints as needed during investigation (remove them
before finishing) to determine WHICH of these it is: (a) backfill-on-create
doesn't populate the trusted_pure-scalar functional index correctly, (b)
the live incremental-update path (on `bob`'s insert) doesn't work, (c) the
query/filter-eval path doesn't correctly use the functional index, or (d)
something else entirely. Root-cause it precisely before fixing — this is
a genuine functional-index/trusted-pure-scalar bug, not a test problem
(unless investigation proves otherwise, in which case document why and
fix the test instead).

## Issue 2 — `oversample_higher_yields_at_least_as_many` (intermittent, ANN/vector)

`crates/shamir-engine/src/table/tests/filtered_ann_tests.rs::oversample_higher_yields_at_least_as_many`
(currently ~line 400) inserts 300 "common" + 6 "rare" vectors (all
scattered near the same query point) using a deterministically-seeded PCG
RNG (`Pcg(7)`), then compares a low-oversample (1×) ANN query against a
high-oversample (10×) query, asserting the high-oversample result count is
`>=` the low-oversample count. Despite the deterministic RNG seed, this
test is intermittent.

Investigate: is the underlying HNSW/ANN index construction itself
non-deterministic (e.g. does `hnsw_rs` or this codebase's HNSW adapter use
any unseeded randomness internally — level assignment, tie-breaking during
graph construction — that isn't controlled by the test's `Pcg(7)` seed)?
If so, the test's premise ("deterministic seed → deterministic ANN
result") is false, and the fix is either: (a) seed whatever internal
randomness source is actually responsible (if the codebase's HNSW wrapper
exposes a way to do so), or (b) redesign the assertion to be robust to the
ACTUAL non-determinism (e.g. run N times and require the property to hold
in aggregate / on average, or widen the tolerance, or use a query
construction that doesn't depend on ANN-approximation edge cases at all —
document your reasoning for whichever you choose). Do NOT simply retry
the assertion in a loop hoping it passes — that's a randomly-passing test,
not a fixed one. If the test's own comment ("may miss rare records
initially" — an inherent approximation-search property) is really just
describing NORMAL ANN behavior and the assertion is fundamentally
comparing two runs that could legitimately tie or invert due to
approximate-search noise, consider whether the assertion should be
loosened (e.g. compare against an expected LOWER BOUND derived from the
algorithm's actual guarantees, not a strict inequality between two
essentially-random-order-dependent runs).

## Issue 3 — `argon2id_concurrency_cap_bounds_parallel_calls` (intermittent under load)

`crates/shamir-funclib/src/crypto/tests/crypto_tests.rs::argon2id_concurrency_cap_bounds_parallel_calls`
(currently ~line 186) spawns `2 × ARGON2ID_CONCURRENCY_CAP` OS threads, each
calling the `argon2id` scalar function, and asserts the OBSERVED PEAK
concurrent in-flight count (`A2_PEAK_IN_FLIGHT`, incremented/decremented
around each call) reaches exactly `cap` — proving the semaphore correctly
caps concurrency AND that the test setup achieved genuine overlap. This has
failed intermittently under system load with e.g. "expected peak==16, got
peak=10" — the KDF calls didn't actually overlap enough in real wall-clock
time to prove the cap was ever contended, because thread scheduling under
load doesn't guarantee `2×cap` threads all reach the KDF call at
overlapping instants.

This is a real-time-measurement flake, not a semaphore-correctness bug
(the semaphore itself is presumably fine — the TEST's proof technique is
timing-fragile). Redesign the test's synchronization to prove overlap
WITHOUT relying on wall-clock scheduling luck: use an explicit barrier —
e.g. a `std::sync::Barrier` (or an atomic counter + spin/park) that each
worker thread signals BEFORE starting its argon2id call and that blocks
all `2×cap` threads until they've ALL signaled "ready to call now",
guaranteeing every thread enters the semaphore-gated call region at
approximately the same instant regardless of system load. This should
make the "the peak reaches `cap`" assertion deterministic instead of
probabilistic. Confirm the semaphore's cap-enforcement logic itself
(`ARGON2ID_CONCURRENCY_CAP`, the actual gating mechanism in
`crates/shamir-funclib/src/crypto/`) doesn't need any change — this is
purely a test-hardening fix, unless investigation reveals otherwise.

## General guidance for all three

- Reproduce each BEFORE fixing (isolated run + a few repeated runs) to
  confirm you understand the actual failure mode, not just the symptom.
- For Issue 1 (stable failure), root-cause via actual debugging, not
  guesswork — this may require reading `FunctionalBackend`'s
  create-time backfill path and its live-update path side by side to spot
  the divergence.
- For Issues 2 and 3 (flakes), the fix must make the test's PASS/FAIL
  determination robust — either by removing the source of non-determinism
  the test wasn't accounting for, or by redesigning what's being measured/
  asserted so genuine non-determinism (approximate search, thread
  scheduling) can't produce a false failure.
- Do not raise any test timeout to paper over a hang — none of these three
  are hang-class issues, but this rule is restated per this repo's
  standing policy.
- If, after genuine investigation, any of these three turns out to be
  unfixable within reasonable scope (e.g. Issue 1 reveals a much larger
  functional-index architectural gap), STOP and document the specific
  finding + a scoped-down follow-up task, per this campaign's established
  pattern — don't force a fix that papers over a deeper issue.

## Test scope

```
./scripts/test.sh -p shamir-db -p shamir-engine -p shamir-funclib
```

Run each fixed test repeatedly in isolation (5-10x) to build confidence
it's no longer flaky/failing, not just a single lucky pass.

## Verification (lighter per-task gate, agreed this session)

```
cargo check --workspace --all-targets
./scripts/test.sh -p shamir-db -p shamir-engine -p shamir-funclib
```

Do NOT run the full fmt/clippy/test --full gate — that's FINAL-GATE's job.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

```
[Issue 1] Status: fixed / scoped-down-with-followup
  > Root cause found (backfill vs live-update vs query-eval, or other)
  > Fix applied + confirmation it now passes reliably (repeated runs)

[Issue 2] Status: fixed / scoped-down-with-followup
  > Root cause (unseeded internal randomness, assertion too strict, etc.)
  > Fix applied + confirmation

[Issue 3] Status: fixed / scoped-down-with-followup
  > Synchronization redesign (barrier mechanism used)
  > Confirmation the peak-reaches-cap assertion is now deterministic
    (repeated runs, ideally under simulated load if feasible)

[Verification]
  cargo check --workspace --all-targets: pass/fail
  ./scripts/test.sh -p shamir-db -p shamir-engine -p shamir-funclib: pass/fail
```
