# Epic04/Phase G — docs for loops + epic closure + FINAL 4-epic summary (#658)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

This is the LAST phase of the LAST of 4 OQL roadmap epics
(`docs/dev-artifacts/roadmap/oql/01-04-*.md`). `BatchOp::ForEach` (Epic04)
is fully implemented, builder-wrapped, unit-tested, e2e-tested, and
benchmarked (Phases A-F, commits `e267406b`, `6ff521d5`, `79510a13`,
`7ed75075`, `f0ccf786`, `a565d436`). This phase writes user-facing docs for
`for_each`, following the exact pattern Epic03/G established (commit
`6ab3a573` — read `git show 6ab3a573` in full first), then produces a
final, honest summary across ALL 4 epics.

## Task 1 — user-facing docs (`docs/guide-docs/guide/01-queries.md`)

Add a new subsection inside section "## 2. Мульти-запросные батчи",
immediately after the existing "### `skipped`-статус в ответе" subsection
(around line 419-436) and before "## 3. Вторичные индексы". Title it
something like "### Циклы: `for_each`" (match the doc's existing
heading-level and Russian-language style exactly — read the surrounding
subsections first for tone/format).

Cover, with real working code examples (lift the canonical scenario
straight from `crates/shamir-client/tests/batch_for_each_e2e.rs`'s
`for_each_over_query_column_ref_inserts_one_audit_row_per_order_over_real_wire`
test — it is a REAL, WORKING example, unlike `when`'s doc which had to use
a synthetic workaround):

- What `for_each` is: a data-dependent loop, `over` resolves to a list
  EXACTLY ONCE before the loop starts, the body runs once per element with
  the element bound to a named parameter (`bind_row`).
- `over` can source from a `$query` column reference, an `$fn` call, or a
  literal array — show at least the `$query`-ref form (the canonical,
  fully-working scenario) and mention the other two exist.
- The "resolved exactly once, not per-iteration" guarantee — this matters
  for e.g. `now()`/random-ish functions used as `over`.
- `BatchLimits.max_iterations` (default 1000) and what happens when
  exceeded (errors before iteration 0, for the dynamic-`over` case).
- Error semantics: transactional batch aborts the whole batch on any
  iteration's failure; non-transactional (autocommit) batch stops at the
  first failing iteration without rolling back already-applied prior
  iterations.
- Pessimistic authorization: a `for_each` with a write body requires the
  write grant regardless of the runtime iteration count (even zero).
- Result shape: the loop's own result is a list of per-iteration result
  maps (note: per-element indexed addressing like `@loop[i].alias[j].field`
  is explicitly DEFERRED per the ADR's Decision 2 — mention this briefly if
  relevant, don't over-explain an unimplemented feature).
- UNLIKE Epic03's `when` doc, `for_each` has NO known blocking bug — do NOT
  add a warning block here; this feature works as designed for its
  canonical use case. (Contrast: #651 blocks `when`'s canonical use case,
  documented prominently in that section — `for_each` has no equivalent.)
- Do briefly mention the known perf characteristic from Phase F's bench
  (ForEach carries real per-iteration overhead vs. a hand-written flat
  batch — prefer emitting ops directly when N is small and fixed; use
  `for_each` when N is genuinely data-dependent/unknown at write time).
- Mention task #660 (a real gap: a transactional batch whose ONLY
  top-level entry is a bare `for_each`/sub-batch currently fails to
  determine its repo — workaround: include at least one top-level
  `Read`/`Insert`/etc. alongside it) as a known limitation, briefly, so
  users aren't surprised by it.

## Task 2 — Epic 04 roadmap closure

Update `docs/dev-artifacts/roadmap/oql/04-loops-foreach.md` (or whatever
the exact filename is — check `docs/dev-artifacts/roadmap/oql/`) to mark
the epic's phases as done, mirroring however Epic03's roadmap doc records
its own closure (check `docs/dev-artifacts/roadmap/oql/03-conditional-execution.md`
for the closure-note convention, if it has one — if not, add a short
"## Status: CLOSED" section at the top or bottom noting all phases done,
linking the ADR and the key commits).

## Task 3 — FINAL summary across all 4 OQL epics

This is the deliverable that matters most for this phase. Write a new file
`docs/dev-artifacts/roadmap/oql/FINAL-SUMMARY.md` — a single, honest,
comprehensive summary covering ALL 4 epics (01 Sequencing, 02 $cond/$expr,
03 Conditional execution, 04 Loops). This is NOT a victory-lap document —
it must give the same unflinching honesty this session applied throughout
(e.g. Epic03/G's docs prominently warning about #651 rather than hiding
it). Structure:

1. **What was built, epic by epic** — one paragraph each for Epic01-04:
   the primitive, its Rust+TS builder ergonomics, test/e2e/bench coverage,
   with commit references (check `git log --oneline` for the actual
   `feat`/`test`/`bench`/`docs` commits per epic — Epic01: `#628-633`;
   Epic02: `#635-640`; Epic03: `#644-650`; Epic04: `#652-658`).
2. **What works fully, with no known limitations**: Epic01 (sequencing —
   `after`/`$query` DAG edges, `EdgeKind` provenance), Epic04's `for_each`
   canonical scenario (real `$query`-driven iteration, confirmed by e2e).
3. **What works with known limitations** (list each with its task number):
   - #641 — `$cond`/`FilterValue` doesn't compose into write SET-values
     (`QueryValue` vs `FilterValue` type-split) — a real GAP, not a bug per
     se, needing its own design/ADR.
   - #643 — `$cond`/`$expr` evaluation re-compiles the filter per-row
     (~29-190x overhead vs a flat literal) — a PERF issue, correctness is
     fine.
   - #651 — CRITICAL: field-based `when` comparisons in `resolve_skip`
     always evaluate to a fixed result (scratch interner can never resolve
     real field paths) — this undermines Epic03's core promised use case
     (the ADR's own canonical "debit if balance >= amount" scenario is
     silently broken, not merely limited). State this one plainly as the
     single most impactful open issue from the whole 4-epic campaign.
   - #660 — `distinct_repos()`/`table_ref()` doesn't walk into
     `Batch`/`ForEach` bodies, so a transactional batch whose only
     top-level entry is a bare `Batch`/`ForEach` fails to determine its
     repo (workaround: include another top-level data op). Affects both
     Epic01's `SubBatchOp` and Epic04's `ForEachOp`.
   - Benchmark-derived characteristic (not a bug): `ForEach` carries real
     ~1.5-1.6x per-iteration overhead vs. a hand-written flat batch (Phase
     F finding) — a design trade-off (ergonomics/data-dependence over raw
     throughput), not a defect, but worth stating plainly here too.
4. **Explicitly deferred / not started**: #659 (while-style loops with
   per-step condition re-evaluation) — a genuinely new primitive shape, out
   of scope for this campaign, proposed by the user mid-session and
   deliberately deferred to keep Epic04 scoped to the simpler for-each
   shape.
5. **Process learnings worth preserving** (2-4 bullets, honest, not
   self-congratulatory): e.g. the concurrent-sub-agent-corruption incident
   during Epic04/Phase B (two agent continuations ran simultaneously,
   editing the same files — caught by the user, salvaged via read-only
   diagnosis rather than discarded) as a cautionary note about
   agent-continuation hygiene; the recurring "growing a struct/enum breaks
   some OTHER crate's construction site" pattern that made workspace-wide
   `cargo clippy --all-targets` mandatory after every phase, not just the
   touched crates.
6. **Recommended next steps** (a short punch list, not a plan): fix #651
   first (blocks Epic03's actual value), then #660 (small, mechanical),
   then #641/#643 as their own scoped efforts, then consider #659 as a
   fresh design exercise if while-loops are still wanted.

Be precise with task numbers and commit hashes — verify them against
`git log` and the actual TaskList state rather than reconstructing from
memory/pattern-matching. If you can't verify a specific number/hash with
certainty, say "see git log for exact commit" rather than inventing one.

## Verification (MANDATORY before you report done)

- The guide-doc addition must be internally consistent with the ADR and
  the actual passing test suite — spot-check at least the code example
  against the real e2e test it's lifted from (does it actually compile as
  a query-builder call today? Read the real builder method signatures in
  `crates/shamir-query-builder/src/batch/batch.rs` and TS
  `crates/shamir-client-ts/src/core/builders/batch.ts` to confirm).
- No production code changes in this phase.
- `cargo fmt --check` / clippy do not apply to markdown-only changes, but
  if you touch anything under `crates/`, run the appropriate check anyway.
- Report which files you created/modified and a brief self-check that the
  final summary's task-number/commit-hash claims are accurate (cite what
  you verified them against).

## Out of scope

- No engine/builder code changes.
- Do not attempt to fix #651, #641, #643, or #660 here — document them.
- Do not start #659's design work.
