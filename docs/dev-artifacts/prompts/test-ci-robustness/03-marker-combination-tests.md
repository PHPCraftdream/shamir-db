# Test/CI Robustness 7d — write-value marker combination test file

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

Fourth item of "Этап 7 — Test / CI robustness"
(`docs/dev-artifacts/research/2026-07-17-release-audit/00-WORK-PLAN.md`),
sourced from report 08
(`docs/dev-artifacts/research/2026-07-17-release-audit/
08-test-coverage-ci-robustness.md`, §2 "Write-value marker combinations").
**This is a REAL TEST-ADDITION task, not docs-only.**

Report 08's §2 is unusually thorough — read it in full before starting
(it's short). It maps exactly what's covered today in
`crates/shamir-engine/src/query/batch/tests/executor_tests/
write_value_resolution_tests.rs` (resolver:
`crates/shamir-engine/src/query/batch/param_subst.rs`) against the four
#641 write-value markers (`$param`/`$query`/`$fn`/`$cond`/`$expr` — plus
`$query` used for cross-op references). The framing: this is "the
structural gap class that produced the shipped `$fn`+`$ref` bug" — i.e.
these are exactly the KINDS of untested corner that let a real bug ship
before, so this isn't speculative hardening.

**Verified independently before writing this brief** (re-verify yourself
too): `crates/shamir-engine/src/query/batch/param_subst.rs` —
`RESERVED_MARKER_KEYS` (line ~88) confirms `$param`/`$query`/`$fn`/`$cond`/
`$expr` are the five recognized marker keys; the `MalformedMarker` error
variant (line ~153) and the `FnCall`-only pass-through-to-table-layer
check (line ~244: `if let FilterValue::FnCall { call } = &fv`) are exactly
as report 08 describes.

## The five gaps (report 08's own recommendation — implement all five)

1. **Top-level `$expr` in a write value: ZERO tests exist.** Add a test
   exercising `{"field": {"$expr": {...}}}` as a write value (e.g. an
   `InsertOp.values` field) with a LITERAL (non-`$ref`) `Expr` body,
   confirming it resolves to the computed value correctly. This is "the
   most direct mirror of the escaped bug" per report 08 — prioritize it.
2. **`$expr`+`$ref` (or `$cond` whose condition contains a field-ref) is
   asymmetric and unpinned.** Confirm/pin the CURRENT actual behavior
   (don't change it unless you find a compelling reason to — this is a
   test-coverage task, not a behavior-change task):
   - `{"$expr": {"op":"add","args":[{"$ref":["a"]},{"$ref":["b"]}]}}` as a
     write value should hard-error with `WriteValueError::MalformedMarker`
     (per report 08's tracing: the pass-through check only fires for
     `FnCall`, so an `Expr` containing a `$ref` resolves against a dummy
     `Null` record and the `FieldRef` lookup misses). Write a test pinning
     this exact error, so a future change to this behavior is a deliberate,
     visible diff instead of a silent behavior change.
   - A `$cond` whose CONDITION references a record field (not a
     `ValueCompare`) currently evaluates against the dummy `Null` record
     and silently picks a branch — report 08 flags this as a
     "silent-wrong-value risk, untested". Write a test that demonstrates
     the CURRENT behavior explicitly (even if the behavior itself looks
     wrong) so it's visible and trackable rather than an undocumented trap.
     If, while writing this test, you conclude the current behavior is a
     genuine bug worth fixing (not just documenting), STOP and report this
     rather than silently fixing it — this brief is scoped to adding test
     coverage for existing behavior, a behavior CHANGE here needs its own
     separate task/brief given the "no test asserts this" framing suggests
     nobody has decided what the desired behavior should be yet.
3. **`SetOp.key` markers untested.** The only existing upsert test uses a
   literal `key(mpack!({...}))` (`write_value_resolution_tests.rs:170`).
   Add a test putting a marker (start with `$param` or `$query`, whichever
   is simpler given the existing test helpers) inside an upsert's `key`
   field (the row-identity path, not just `value`) and confirm it resolves
   correctly.
4. **Nesting combinations untested.** Add at least two tests: (a) a marker
   nested two levels deep inside a Map (e.g.
   `{"outer": {"inner": {"$param": "x"}}}`), and (b) a marker inside a List
   element (e.g. `{"items": [{"$fn": {...}}, "literal"]}`). Also add
   coverage for dependency extraction of a `$query` ref NESTED inside a
   `$fn`'s args or a `$cond`'s branch (report 08 notes only the top-level
   unknown-alias case is tested today, at `write_value_resolution_tests.
   rs:447`).

## The task

1. Add all five test cases above to
   `crates/shamir-engine/src/query/batch/tests/executor_tests/
   write_value_resolution_tests.rs` (extend the existing file — it already
   has the right test harness/helpers per the "What IS covered" table's
   citations; don't create a new file unless you find a structural reason
   the existing file can't accommodate these, per CLAUDE.md's "no new
   files unless the task genuinely needs them" rule).
2. Each test must be genuinely non-vacuous — confirm it would fail if the
   corresponding resolver code were reverted/broken (mentally trace this,
   or actually comment out the relevant resolver branch locally and
   confirm the new test fails, then restore it — the second approach is
   stronger evidence, use it if time permits).
3. For item 2's two pinning tests specifically: your test assertions
   should describe the CURRENT behavior with a comment explaining WHY
   (referencing report 08's own reasoning: the pass-through check is
   `FnCall`-only) so a future reader understands this is a known,
   deliberately-scoped asymmetry being tracked, not an oversight.

## Out of scope

- Do NOT change `param_subst.rs`'s actual resolution logic — this is a
  test-coverage task. If your investigation surfaces a genuine bug (not
  just an asymmetry worth pinning), STOP and report it rather than fixing
  it inline — flag it as a new standalone finding for the orchestrator to
  triage (mirroring how this campaign's task #695 was created as a
  standalone follow-up for a similar mid-task discovery).
- Do NOT touch report 08's other sections (§1, §3, etc.) or any other
  Этап 7 item — this brief is scoped to §2's five gaps only.
- Do NOT touch anything from the already-completed Этапы 1-6 or tasks
  7a/7b/7c — this brief is scoped to this one test file.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-engine --full` green, including all five
  new tests passing.
- `cargo fmt -p shamir-engine -- --check` clean.
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Report literal command output for all of the above.
- For each of the five new tests, state explicitly what code path it
  exercises and why you're confident it's non-vacuous (per point 2 in
  "The task" above — ideally with the "temporarily break it, confirm
  failure, restore it" evidence for at least the highest-priority one,
  item 1's top-level `$expr` test).
- Explicitly confirm whether your investigation surfaced any genuine bug
  (not just an asymmetry) — if so, describe it precisely (file:line,
  repro) without fixing it, so the orchestrator can triage it as a new
  standalone task.
