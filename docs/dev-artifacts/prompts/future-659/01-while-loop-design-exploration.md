# #659 — FUTURE: while-style loops in OQL (design exploration only, NO CODE)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## What this task is (and is not)

This is a DESIGN EXPLORATION, not an implementation task. Produce a design
document; write NO engine/builder/test code. The user proposed this idea
mid-session during Epic04 (Loops/`for_each`) and explicitly deferred it
("не сейчас — доделать простой for-each") to keep that epic scoped. This
task is the promised follow-up: think it through properly now that
`for_each` (resolve-`over`-once-then-iterate-K-times) has shipped in full
(ADR: `docs/dev-artifacts/design/oql-04-loops-foreach-adr.md`; commits
`e267406b`, `6ff521d5`, `79510a13`, `7ed75075`, `f0ccf786`, `a565d436`).

## The user's original proposal (read exactly as stated, do not embellish)

"возможно стоит сделать два типа циклов - у которых условия пересчитываются
на каждом шаге, и у которых не пересчитываются. Или просто в самих условиях
это задавать — давай в самих условиях это задавать" — i.e., rather than a
wholly separate loop primitive, express "recheck this condition every step"
as a FLAG living inside the condition/loop-spec itself, distinguishing it
from `for_each`'s existing "resolve `over` once" semantics.

## Required reading before writing anything

1. `docs/dev-artifacts/design/oql-04-loops-foreach-adr.md` in full — pay
   close attention to Decision 1 (why `for_each`'s body is planned ONCE,
   statically, as a black box — this is the crux of the tension a
   while-loop introduces) and any place it mentions static DAG planning
   constraints.
2. `crates/shamir-query-types/src/batch/planner.rs`'s `BatchPlanner::plan`
   — understand that batch execution today is planned as a STATIC DAG
   (stages, dependencies) computed ONCE before any op runs. A while-loop
   whose iteration count is genuinely unknown until runtime (unlike
   `for_each`'s `over`, which resolves to a concrete list length before the
   loop starts) does not fit this static-planning model without a real
   design change — that tension is the central question this task must
   resolve.
3. The `#651` fix (commit `3315f414`) — `Filter::ValueCompare` now supports
   genuine value-vs-value comparisons (e.g. "balance >= amount") using real
   `$query`/`$fn`/`$param` data, with NO field/record dependency. This
   directly changes the feasibility landscape versus what was true when the
   user's proposal was first raised (at that point, #651 was still an open
   CRITICAL bug blocking any real data-driven condition) — a while-loop's
   condition would naturally use `Filter::ValueCompare` today.
4. `docs/dev-artifacts/roadmap/oql/FINAL-SUMMARY.md` — the honest
   cross-epic summary, including its explicit note that #659 "may be
   achievable via repeated `for_each` + `when` composition instead of a
   wholly new primitive" now that `when`'s field-based-comparison bug is
   fixed — evaluate this claim directly as part of your exploration; don't
   just assert it or dismiss it.

## What the design document must cover

Write to `docs/dev-artifacts/design/oql-05-while-loop-exploration.md`
(this is exploratory, NOT a numbered "OQL Epic 05" commitment — name it
accordingly, e.g. title it "Exploration: while-style loops — feasibility
and recommendation", not an ADR-with-decisions like the other 4).

1. **Restate the problem precisely.** What would a while-loop actually let
   a caller express that `for_each` cannot? Give a concrete canonical
   scenario (e.g. "repeatedly deduct from an account until balance < some
   threshold, unknown number of iterations up front" or similar) — a real
   motivating use case, not an abstract one.
2. **The static-planning tension, spelled out.** Explain concretely why
   `for_each`'s design (plan-body-once, black-box per ADR Decision 1) works
   with today's static DAG planner, and why a while-loop's "recheck
   condition every step, unbounded iteration count" breaks that assumption
   — what specifically would planner.rs / query_runner.rs need to do
   differently, and is that a small extension or a structural rewrite?
3. **Evaluate the "flag in the condition" idea directly.** Is embedding a
   recheck-flag inside a condition object (as the user proposed) actually
   the cleanest wire shape, or does the investigation suggest a different
   shape is cleaner (e.g. a distinct `BatchOp::While` variant, analogous to
   how `ForEach` is distinct from `Batch`)? Give a reasoned recommendation,
   not just a restatement of the user's phrasing.
4. **Evaluate the "compose for_each + when" alternative** the
   FINAL-SUMMARY.md speculates about — can a caller already get
   while-loop-like behavior today by combining `for_each` (over some
   generously-sized upper-bound array) with a `when` guard (using the now-
   fixed `Filter::ValueCompare`) on each iteration's body to skip once a
   condition is no longer met? What are the real limitations of this
   workaround (e.g. must guess an upper bound, wastes iterations,
   `max_iterations` limit interacts awkwardly) versus a real while-loop?
5. **Authorization/limits implications.** `for_each`'s pessimistic
   authorization model (classify by static body, ignore runtime iteration
   count) and `max_iterations` DoS gate both rely on being able to bound
   the work at plan time. A genuinely unbounded while-loop has no such
   bound. What would a safe DoS/authorization story look like for a
   while-loop (e.g. a hard `max_iterations` ceiling is now MANDATORY rather
   than optional, since there's no static list length to fall back on)?
6. **Recommendation.** End with an explicit, honest recommendation: (a)
   build it as a new primitive (sketch the shape, if so), (b) don't build
   it — the `for_each`+`when` composition is good enough and a new
   primitive isn't worth the static-planning rework, or (c) build a
   narrower middle-ground (e.g. a bounded "repeat with early-exit when
   condition becomes false" that's really `for_each` over a fixed max
   count with an early-stop check — closer to `for_each` than a true
   while-loop). Whichever you recommend, justify it against the effort/
   value tradeoff the way this session's other design documents have
   (e.g. the Epic01 ADR's "stage parallelism: closed, not implemented"
   precedent for a reasoned "no" being just as valid an outcome as a "yes").

## Verification

- This task produces ONE new markdown file, no code changes. Read it back
  yourself before finishing and confirm it directly answers all 6 points
  above with genuine engineering reasoning (citing real file:line
  references from your reading), not generic filler.
- No `cargo`/`npm`/test commands are relevant here — this is docs-only.

## Out of scope

- Do NOT implement any code for this task, no matter how tempted — if your
  recommendation is "build it", describe the shape and defer actual
  implementation to a future task with its own ADR + phased rollout
  (mirroring how Epic03/04 each required their own ADR-first phase before
  any code).
- Do NOT touch any other task's files.
