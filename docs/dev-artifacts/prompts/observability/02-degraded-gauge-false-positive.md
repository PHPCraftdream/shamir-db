# Brief — #1003 (part of #1001): fix #984's degraded-index gauge false-positive on a healthy in-progress CREATE INDEX

Task: #1003 in the session TaskList (a decomposed leaf of the former #1001
umbrella). Follow-up from `@oh`'s adversarial review of the #983/#984/#997/
#998 batch. #984 itself (`docs/dev-artifacts/prompts/observability/01-degraded-index-gauge.md`)
is DONE and merged — this is a correctness refinement on top of it, not a
redo.

## The bug, precisely

`TableManager::degraded_index_count()`
(`crates/shamir-engine/src/table/degraded_index_count.rs`) counts every
index whose `.state != IndexState::Ready` across all four families
(regular, unique, sorted, index2) — i.e. every index currently `Building`.

`IndexState::Building` is set at the START of `CREATE INDEX` (before the
backfill scan begins) and flipped to `Ready` only once the backfill fully
completes (F-72, #899 — see `crate::index2::state::IndexState`'s doc and
`create_index`'s call sites in
`crates/shamir-engine/src/table/table_manager_index_mgmt.rs`). A backfill
on a large table can legitimately run for minutes (see
`docs/guide-docs/KNOWN_LIMITATIONS.md` §3's measured numbers — a 100k-row
table's build takes ~140-160s).

So: **a completely healthy, currently-in-progress `CREATE INDEX` makes
`degraded_index_count()` — and therefore the `shamir_degraded_indexes_total`
Prometheus gauge — read non-zero for the ENTIRE duration of the build.** The
gauge's own help text
(`crates/shamir-server/src/observability.rs:404-413`) says:

```
"Count of indexes NOT in Ready state (stuck-Building) across currently-open
tables only. … Non-zero means an operator should run doctor::repair() to
rebuild the stuck index(es)."
```

This would page an operator (or trigger an alert) on a completely normal
DDL operation, telling them to run a repair tool against an index that is
not stuck at all — a real false-positive, not a cosmetic wording issue.

## Read these first

1. `crates/shamir-engine/src/table/degraded_index_count.rs` — the counting
   method itself (unchanged shape today: walk 4 families, count
   non-`Ready`).
2. `crates/shamir-server/src/observability.rs:400-413` — the gauge
   registration + help text.
3. `crates/shamir-engine/src/table/tests/degraded_index_count_tests.rs` —
   existing tests, including the `ReadCountingStore` double proving zero
   store reads (your fix must NOT reintroduce store reads into the hot
   count path — any new in-flight-tracking state must be in-memory/atomic,
   same as the existing state).
4. `crates/shamir-index/src/base_index/index_manager.rs`'s
   `create_index_backfill_hook` field (~line 162) + `set_create_index_backfill_hook`
   (~line 439) — the EXISTING test-only pause-hook mechanism used
   elsewhere in this codebase to park a real backfill mid-flight for a
   live-simulation test. You'll want this to construct a genuine
   "CREATE INDEX is currently running" test scenario (not a hand-seeded
   `Building` definition, which is indistinguishable from a truly stuck one
   — see point below).
5. `crates/shamir-engine/src/table/table_manager_index_mgmt.rs`'s
   `create_index` / `create_unique_index` / `create_sorted_index_with_include`
   / index2's create path (`create_index_v2` or equivalent) — the call
   sites where a build genuinely starts and ends.

## The fix — recommended approach (justify if you deviate)

**Track in-flight CREATE INDEX operations and exclude them from the count**
(option (b) from the original task description) — this is a REAL fix, not
just a documentation band-aid, and this codebase already has the RAII-guard
convention for exactly this shape of "in flight" tracking (see
`begin_write_barrier`'s guard types for the established pattern).

Concretely:
- Add a per-`TableManager` (or per-`IndexManager`, whichever is the
  natural owner given where `create_index`'s call sites live) `Arc<AtomicU64>`
  in-flight-create counter.
- Increment it at the very start of each of the four families' create path
  (`create_index`, `create_unique_index`, `create_sorted_index_with_include`,
  the index2 create path), decrement it via an RAII guard so a panic or
  early error return still decrements (mirroring how this codebase already
  guards barrier acquisition — do NOT hand-roll a decrement at every return
  site, that's exactly the bug class RAII guards exist to prevent here).
- `degraded_index_count()` becomes: count non-`Ready` indexes as today,
  then `saturating_sub` the in-flight-create counter's current value from
  the total (clamped at zero — never negative).
- This correctly still reports a GENUINELY stuck index (one left `Building`
  by a crash in a PAST process — the in-flight counter is process-local and
  starts at 0 on every restart, so a crash-orphaned `Building` state from
  before this boot is NOT masked) while excluding a currently-live create in
  THIS process.

If, after reading the actual code, you find a cleaner or more accurate
approach — e.g. splitting into a separate `shamir_building_indexes_total`
gauge (option (c)) instead of subtracting — that's an acceptable deviation,
but justify it in your report and make sure the SAME test scenario below
(a live in-progress create must not look "stuck") is provable either way.
Do NOT settle for option (a) (help-text-only) unless you find a concrete
reason the counter approach is unsafe or infeasible — a pure docs fix
leaves the underlying false-positive live, which is a weaker outcome.

## Required new test

Using the existing `create_index_backfill_hook` pause-hook mechanism:
1. Start a table, seed enough rows that a backfill is genuinely observable
   (doesn't need to be large — the pause hook makes it deterministic
   regardless of row count).
2. Install the pause hook, spawn `create_index(...)` as a background task,
   park it mid-backfill (same `tokio::select!` + `wait_until_parked()`
   pattern used throughout this codebase's other pause-hook tests — see
   `crates/shamir-engine/src/table/tests/p997_hash_rename_durability_tests.rs`
   for the exact idiom).
3. While parked (create genuinely in-flight, index in `Building` state):
   assert `degraded_index_count()` is **zero** — this is the exact
   false-positive scenario, now proven fixed.
4. Let the create finish (resume the hook / let the parked task complete).
   Assert the index is `Ready` and `degraded_index_count()` is still zero.
5. Separately (can reuse or add to the EXISTING
   `degraded_index_count_tests.rs` file's pattern of hand-constructing a
   `Building` definition WITHOUT going through `create_index` at all —
   simulating a crash-orphaned index from a past process, i.e. `in_flight`
   counter is 0 but the definition is genuinely stuck): assert
   `degraded_index_count()` still correctly reports it as degraded. This is
   the regression guard proving your fix didn't just always return zero.

## Scope discipline

- Do not change `/readyz`'s behavior or doc — that decision (data-quality
  vs boot-readiness) is deliberate and out of scope, per #984's original
  brief.
- Do not change the gauge's name or remove it — only its accuracy /
  optionally its help text.
- Keep the fix's cost genuinely O(1) amortized / no new store reads — the
  `ReadCountingStore` test double in the existing test file must still
  report zero reads for the count path.

## Gate (MANDATORY)

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -- degraded
./scripts/test.sh -p shamir-server --full
```

⚠️ Raw `cargo test` is BLOCKED by this repo's perimeter guard. Use
`./scripts/test.sh` (`-p <crate>`, `-- <substring>` for a narrow run).

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Do NOT run
`git commit` or `git add` — the orchestrator verifies your diff and the
test run, then commits. Only edit/create files and run read-only / test /
gate commands.

## What to report back

- Which option you chose ((b) in-flight counter, (c) split gauge, or a
  justified alternative) and why.
- Confirmation the new live-in-progress-create test passes, and that the
  existing "genuinely stuck (crash-orphaned)" scenario still correctly
  reports degraded (paste both test names + pass/fail).
- Confirmation the `ReadCountingStore` zero-reads test still passes
  unmodified.
- Exact gate command output.
