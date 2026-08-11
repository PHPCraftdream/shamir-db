# Brief 80 — #1080 (nit): `QueryResult { .., ..Default::default() }` disables exhaustive field-checking at ~133 sites

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## Background — already investigated, confirmed against the code

`crates/shamir-query-types/src/read/query_result.rs:112-202` — `QueryResult`
derives `Default` (F-10, #800), and its own doc comment (lines 114-118)
explains why: adding a new field (like `op_id`/`ddl_status`, added later by
#1015) shouldn't force hand-editing every existing `QueryResult { .. }`
construction site. That rationale is real and the derive itself is fine —
this task is NOT about removing `#[derive(Default)]`.

The problem: **~133 call sites** across `shamir-engine` (and a handful
elsewhere) spread `..Default::default()` across the CONSTRUCTION site
itself, e.g.:

```rust
return Ok(QueryResult {
    records: Vec::new(),
    stats: None,
    pagination: None,
    value: None,
    explain: Some(plan),
    skipped: false,
    versions: None,
    corrupt_records: Vec::new(),
    ..Default::default()   // <-- only op_id/ddl_status actually rely on this
});
```

Every field ALREADY has a natural default value (`None`/`false`/`Vec::new()`)
— today, `..Default::default()` at these sites is behaviorally identical to
spelling out the remaining fields. The problem is for the FUTURE: the next
person adding a field to `QueryResult` gets a struct-literal that still
compiles at all 133 sites even if the new field genuinely needs a non-default
value on SOME of those paths — the compiler no longer forces a decision at
each site, so a real omission becomes silent wrong behavior discovered only
at runtime, not a compile error.

**Investigated site-shape diversity yourself before writing this brief**: the
133 sites are NOT uniform. Sampled directly (`crates/shamir-engine/src/table/read_exec.rs`
lines ~308, ~407, ~446, ~588, ~659, ~753, ~1289, ~1428 — re-verify these line
numbers yourself, the file has likely moved since this brief was written):
some sites spell out 7-8 of the 10 fields explicitly and only rely on
`..Default::default()` for `op_id`/`ddl_status` (the two #1015 added); others
spell out far fewer fields. There is no single dominant "minimal" shape a
regex could safely rewrite — a blind mechanical substitution risks either
missing fields (compile error, fine, self-correcting) or silently duplicating
a field that's ALSO already set elsewhere in the same literal (compile error
too, so also self-correcting) — meaning this is compiler-verifiable but NOT
blindly scriptable without per-site attention when the compiler flags a
conflict.

## The fix — option (b) from the original task framing, confirmed as preferred

Two options were on the table; option (b) is preferred (smaller diff,
addresses the root cause instead of just the symptom) — confirm this
yourself against the actual site survey before committing to it, and switch
to option (a) for any sites option (b) doesn't cleanly fit:

**(a) Brute-force**: replace `..Default::default()` with explicit
`op_id: None, ddl_status: None,` (or whichever fields are ACTUALLY still
omitted at that specific site — some sites may omit more) at all ~133
sites — restores full exhaustive-checking everywhere, but every EXISTING
site pays the diff cost, and adding an 11th field in the future STILL means
touching all 133 sites (the exact churn #800's `Default` derive existed to
avoid).

**(b) Helper constructors (preferred)**: add one or a small handful of named
constructors on `QueryResult` for the genuinely common shapes seen across the
133 sites, and migrate call sites that match one of those shapes to use the
helper instead of a raw struct literal. This keeps `..Default::default()`
usage down to a SMALL, easily-reviewed number of places (inside the helpers
themselves), not scattered across 133 call sites — a future field addition
only needs review at the helpers + whatever handful of sites still use a raw
literal, not all 133.

Concretely for (b): survey the actual shapes across all ~133 sites (grep
`QueryResult {` across `crates/shamir-engine/src/table/` and
`crates/shamir-engine/src/query/`, read enough context at each to classify
the shape) and design constructors that cover the highest-frequency shapes.
Candidates likely worth adding (confirm against your own survey, don't just
copy this list blindly):
- `QueryResult::rows(records: Vec<QueryRecord>) -> Self` — records only,
  everything else default. Likely the single most common shape for simple
  scan paths.
- `QueryResult::with_stats(records: Vec<QueryRecord>, stats: QueryStats) -> Self`
  — records + stats, everything else default. Very common in the sampled
  `read_exec.rs` sites (records + `Some(QueryStats { .. })`, pagination
  usually `None` at those specific sites — verify per-site before assuming).
- Any other shape you find repeated ≥ ~10 times is worth its own
  constructor; anything rarer should just stay a raw literal with EVERY
  field spelled out explicitly (no `..Default::default()`) per option (a).

For sites that don't cleanly fit any helper (genuinely one-off combinations,
or where readability would suffer from forcing an awkward helper call),
apply option (a) directly: spell out every field, drop
`..Default::default()`. Every one of the 133 sites must end up EITHER using
a named helper OR having zero `..Default::default()` in it — none should be
left as a raw literal that still uses `..Default::default()`.

**Do not change behavior anywhere.** Every field's value at every site must
be IDENTICAL before and after this change — this is a purely structural
refactor. If you find a site where you're unsure whether the pre-existing
`..Default::default()`-implied value is actually correct (i.e. you suspect a
LATENT bug this task's premise warns about), do NOT silently "fix" it —
flag it explicitly in your final report as a separate finding, leave the
behavior unchanged in this commit, and let the orchestrator decide whether
it's a real bug worth a follow-up task.

## Scope

Cover every `QueryResult { .. ..Default::default() .. }` site the review
found (~133, concentrated in `shamir-engine`, primarily
`src/table/read_exec.rs`, `src/table/read_index_scan.rs`, and
`src/query/batch/query_runner.rs` per the original grep, but the review's
count may be stale — re-grep yourself for the authoritative current list;
some sites will be in `#[cfg(test)]` code, which are lower priority but
should still be migrated for consistency if the sweep is cheap once the
helpers exist). Non-`shamir-engine` sites (a handful in `shamir-query-types`,
`shamir-query-builder`, `shamir-db`, `shamir-server`, `shamir-transport-ws`
test/bench files per a broader grep) are in scope too if they match the same
pattern — sweep the whole workspace, not just `shamir-engine`.

## Tests

No new tests required beyond the existing suite — this is a structural
refactor with no behavior change (per the task's own framing). The existing
test suite is the regression guard: if any site's effective field values
changed, an existing test should catch it. Do not add tests just to pad
coverage for a no-behavior-change refactor.

## Gate before you report done

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine
./scripts/test.sh -p shamir-engine --full
./scripts/test.sh -p shamir-db --full
./scripts/test.sh -p shamir-server --full
./scripts/test.sh -p shamir-query-types
./scripts/test.sh -p shamir-query-builder --full
```

(Run the full set — this refactor's blast radius touches many files across
several crates; a narrow scope check would miss a regression in an
untouched-looking corner.)

Paste the actual final summary line from every command — literal output, not
a paraphrase. Report: how many of the ~133 sites you found and migrated
(give the actual final count from your own grep, both before and after —
confirm the "after" count of remaining `..Default::default()` inside
`QueryResult { .. }` literals is at or near zero, explaining any deliberate
exceptions), which helper constructors you added and how many sites each
one covers, and any suspected latent-bug findings you deliberately did NOT
fix (per the "do not change behavior" instruction above). If anything fails,
fix it before reporting done.
