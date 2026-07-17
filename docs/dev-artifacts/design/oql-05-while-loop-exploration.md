# Exploration: while-style loops — feasibility and recommendation

Status: exploratory only — no decisions committed, no code written.
Task: #659 ("FUTURE"). Not a numbered "OQL Epic 05"; this deliberately
does not follow the ADR format of `oql-01`..`oql-04` because no
implementation is being authorized by this document. If a future task
picks up the recommendation below, THAT task should produce its own
ADR-first phase (mirroring Epic03/Epic04's own ADR-before-code
discipline) before any code lands.

Context this builds on:
- `docs/dev-artifacts/design/oql-04-loops-foreach-adr.md` (Epic04's ADR
  for `ForEachOp`, hereafter "the Epic04 ADR").
- `crates/shamir-query-types/src/batch/planner.rs` (`BatchPlanner::plan`).
- Commit `3315f414` (`#651` fix — `Filter::ValueCompare`).
- `docs/dev-artifacts/roadmap/oql/FINAL-SUMMARY.md` §4/§6, which
  speculates that #659 "may be achievable via repeated `for_each` +
  `when` composition ... now that `when`'s field-based-comparison bug is
  fixed."

---

## 1. Restate the problem precisely

`for_each` (`BatchOp::ForEach`, shipped in Epic04) resolves `over` to a
concrete `Vec<QueryValue>` **once**, then runs its body exactly
`elements.len()` times — one iteration per already-known element
(`crates/shamir-engine/src/query/batch/query_runner.rs:365-415` resolves
`over`, then `:429-458` iterates the resolved `elements` vector). The
iteration count is a function of *data that already exists* before the
loop starts: "how many rows does `@orders` have", "how many elements are
in this literal array". Nothing about the loop body's own effects can
change how many times it runs.

A while-loop is a fundamentally different shape: **the exit condition is
a function of a value that changes as a result of the loop body's own
writes**, and must be re-evaluated after every iteration, not resolved
once up front. `for_each` cannot express this at all — not "expresses it
awkwardly", literally cannot, because its iteration source is fixed
before iteration 0.

**Concrete canonical scenario:** an escrow/settlement sweep — "repeatedly
transfer a fixed installment amount from `hold_balance` to
`available_balance` on account `acct_id`, once per scheduled release
cycle, until `hold_balance < installment` (fewer funds remain on hold than
one more installment), stopping automatically rather than at a
caller-guessed cycle count." E.g. an escrow of 1,340 with a 250/cycle
release schedule needs exactly 5 iterations (250×5=1250, remainder 90 <
250, stop) — a number that is **not knowable from any query result before
the loop starts**, because it depends on the very field (`hold_balance`)
the loop body itself decrements on each pass. A `for_each` over
`@escrow_accounts[].hold_balance` divided by installment size is not
computable as a `FilterValue` expression today (no division/modulo in
`$expr`, and even if there were, that's a plan-time approximation of a
runtime process, not the process itself) — and even if it were, that
number changes with every write the loop performs, which `for_each`'s
"resolve `over` once" model structurally cannot follow.

This is the concrete gap: **a genuinely data-dependent, self-referential
stopping condition where the datum being tested is mutated by the very
operation being repeated.** `for_each` can iterate over pre-existing data;
it cannot iterate "until my own writes make this false."

---

## 2. The static-planning tension, spelled out

`for_each` fits today's static DAG planner precisely because its body is
a **black box planned once** — the Epic04 ADR's Decision 1 states this
directly: "The body is planned **once** (as a static `BatchRequest`,
exactly like a sub-batch today) and executed **K times**... the identical
recursive seam `QueryRunner::run`'s `BatchOp::Batch` arm already uses
(`crates/shamir-engine/src/query/batch/query_runner.rs:209-308`), just
invoked in a loop instead of once" (ADR lines 46-52). Concretely, in
`planner.rs`:

- `BatchPlanner::extract_dependencies`'s `BatchOp::ForEach(fe)` arm
  (`planner.rs:302-308`) extracts dependencies **only from `fe.over`**,
  explicitly refusing to descend into `fe.batch.queries`: "Do NOT descend
  into the loop body's queries — those are planned recursively at
  execution time (K times)". The loop's *body* never enters the outer
  DAG at all; only its *iteration source* does.
- `max_nesting_depth_of_ops` (`planner.rs:645-671`) walks into
  `fe.batch` purely for a depth-bound safety check, not to schedule
  anything inside it relative to sibling ops.
- The static DoS gate (`planner.rs:126-151`) computes
  `iterations × body.queries.len()` **only for the literal-array case**
  where `items.len()` is knowable from the `FilterValue::Array` itself at
  plan time — a compile-time constant, not a runtime observation.
- The actual per-iteration execution loop
  (`query_runner.rs:429-458`) is a plain Rust `for element in elements`
  over an already-fully-materialized `Vec<QueryValue>` — there is no
  per-iteration re-entry into the planner at all. The plan for the body
  is computed once (inside each recursive `execute_batch_impl` call, which
  re-plans the *same static* `fe.batch` K times — see the Epic04 ADR
  Decision 1's phrasing "executed K times" — but re-planning an unchanged
  static structure K times is not the same as needing K *different*
  plans).

A while-loop breaks every one of these load-bearing assumptions
simultaneously:

1. **No fixed K exists even conceptually before iteration 0.** `for_each`
   always has *some* K, discoverable either at plan time (literal array)
   or at one runtime checkpoint immediately before iteration 0 (dynamic
   `@alias[].field` — see ADR Decision 3, `query_runner.rs:417-427`'s
   `elements.len() > max_iterations` gate, which is checked exactly once,
   before the loop body runs at all). A while-loop's exit condition
   depends on state that only exists *after* each iteration completes —
   there is no single point before iteration 0 where "the total number of
   iterations" is knowable, not even approximately, because the condition
   references a field the loop body itself is actively mutating.
2. **The static DoS fold-in (`planner.rs:126-151`) cannot run at all.**
   That check multiplies a known `iterations` by `body.queries.len()`.
   For a while-loop there is no `iterations` operand to multiply by —
   not "hard to compute", structurally absent. The only remaining lever
   is a hard `max_iterations` ceiling enforced *during* execution (see
   §5), which is a different kind of check (a runtime abort partway
   through, not a plan-time reject before any work starts).
3. **`extract_dependencies`'s "don't descend into the body" invariant
   gets harder to justify.** For `ForEach`, not descending into the body
   is safe because the body's *inputs* (its own internal `$query` refs)
   are self-contained relative to the outer DAG — only `over` crosses the
   boundary. A while-loop's condition, by definition, must reference a
   value the *body itself produced in the previous iteration* — e.g. "did
   my last UPDATE bring `hold_balance` below `installment`". That is a
   dependency edge that does not exist in the outer static DAG (there is
   no separate alias for "the body's own last-iteration output" the way
   `over` is a separate, resolvable-once alias reference for `for_each`).
   The outer planner would need to either (a) treat the whole while-node
   as an opaque unit whose internal loop-carried dependency never
   participates in inter-alias ordering (workable, but then nothing
   *outside* the loop can reference intermediate per-iteration state,
   same limitation `ForEach`'s deferred `@loop[i]` addressing already
   has — see Epic04 ADR Decision 2), or (b) re-plan the condition against
   fresh `resolved_refs` after every iteration, which is not "extending"
   `extract_dependencies`, it is moving condition-resolution from a
   plan-time, once-per-batch operation to a runtime, once-per-iteration
   operation — a different phase of execution entirely.

**Is this a small extension or a structural rewrite?** It is closer to a
structural rewrite of the *execution* side (not the planner-DAG side).
Concretely:
- `planner.rs` itself barely changes: a `BatchOp::While` node would be
  treated exactly like `ForEach` is today for dependency extraction (only
  the condition's `$query` refs cross the outer boundary, the body stays
  opaque) and for nesting-depth walking. This part **is** a small,
  mechanical extension — a few more match arms, following the `ForEach`
  precedent almost verbatim.
- The DoS/limits story (§5) and the execution loop
  (`query_runner.rs`'s `for element in elements` block) are where the
  real work is: today's loop is "materialize a `Vec`, iterate it" — a
  while-loop needs "re-evaluate a `Filter` against **freshly produced**
  `resolved_refs` after every iteration, before deciding whether to run
  iteration N+1 at all." That means the condition-evaluation machinery
  (`resolve_skip`, `query_runner.rs:102-141`, today called exactly once
  per `QueryEntry` per stage) would need to be called in a *new* position
  — inside the loop body's own execution, after each iteration commits,
  something no existing code path does. This is a new control-flow
  primitive in the executor, not a parameter tweak to an existing one.
- Error/transaction semantics (Epic04 ADR Decision 4) assumed a
  *bounded, known-length* sequence of iterations whose total elapsed work
  is front-loaded-checked once (`elements.len() > max_iterations`). A
  while-loop's mandatory-ceiling check (§5) has to run *inside* the loop,
  every iteration, turning a one-time guard into a per-iteration runtime
  cost and a new failure mode ("hit the ceiling mid-loop" — something
  Decision 3 explicitly designed `for_each` to avoid: "never a partial
  run followed by a mid-loop abort", `query_runner.rs:417-419`'s comment).
  A while-loop cannot avoid exactly the mid-loop-abort shape `for_each`
  was designed to sidestep, because the whole point of a while-loop is
  that the total length is unknowable until you're already partway
  through.

Net: the DAG/dependency-extraction layer extends cheaply (following
`ForEach`'s existing pattern); the execution/limits layer requires a
genuinely new control-flow shape with a different (weaker) safety
guarantee than `for_each` currently provides. Call this "medium-large":
not a rewrite of the planner, but a real new primitive in the executor
with its own failure-mode class, not a config flag on `ForEach`.

---

## 3. Evaluate the "flag in the condition" idea directly

The user's proposal, verbatim: "возможно стоит сделать два типа циклов -
у которых условия пересчитываются на каждом шаге, и у которых не
пересчитываются. Или просто в самих условиях это задавать — давай в самих
условиях это задавать" — i.e., put a `recheck: true/false` (or similarly
named) flag *inside* the condition/loop-spec object itself, rather than
introducing a wholly separate loop primitive.

**Where would this flag actually live, mechanically?** `ForEachOp` has no
condition field at all today (`crates/shamir-query-types/src/batch/for_each_op.rs:20`
— it is `{ over: FilterValue, bind_row: String, batch: BatchRequest }`,
no `when`/`Filter` anywhere). So "a flag in the condition" cannot mean "a
flag on `ForEachOp.over`'s existing shape" — `over` is a `FilterValue`
(value-producing), not a `Filter` (predicate), per Epic04 ADR Decision 1's
explicit distinction ("`over` is deliberately `FilterValue`, not
`Filter`... `Filter` (used for WHERE/`when`) answers 'does this row match
a predicate'; `over` answers 'what is the list of elements to iterate'").
A recheck flag is meaningless attached to a value-producing expression —
there's no boolean there to recheck. So the flag has to live somewhere
new: either (a) a NEW `condition: Filter` field added onto `ForEachOp`
alongside `over`, with a `recheck_each_step: bool` flag governing whether
that `condition` is evaluated once (Epic03's existing `when` semantics —
evaluate before the whole op, once) or evaluated fresh after every single
iteration (the new while behavior) — or (b) the flag lives on the
existing `QueryEntry.when` field itself (already `Option<Filter>`,
`crates/shamir-query-types/src/batch/query_entry.rs` per Epic03), turning
`when` from "gate the whole op once" into "gate + optionally recheck this
op N times if the op happens to be a loop."

**Evaluating (b) first, since it's closest to the user's literal words**
("в самих условиях это задавать" — put it in the condition itself, i.e.
reuse `when`): this conflates two orthogonal concerns that today are
cleanly separated by design. `when` (Epic03) answers "should this **whole
op** run at all, evaluated once, before it starts" — a op-level admission
gate. A while-loop's condition answers "should **iteration N+1** of an
**already-running, already-iterating** op happen" — a per-iteration
continuation gate. These are different questions asked at different
times against different data (the first against pre-loop state, the
second against post-iteration-N state). Overloading `when` with a
`recheck` flag means: for a `ForEach`, `when` still means "run the whole
loop or not" (evaluated once, before element 0, exactly as today); for a
hypothetical while-primitive, the *same field name* would mean "run
iteration N+1 or not, re-evaluated after every iteration" — same field,
same type (`Filter`), semantically different operation depending on a
sibling boolean. That is a worse API than two distinct fields, because a
reader of `when: Some(f)` now has to also check a separate
`recheck_each_step` bool to know *which* of two very different semantics
applies — the flag doesn't just toggle a boolean output, it toggles
*which point in the state machine* the filter is evaluated at and *how
many times*.

**Evaluating (a):** a `condition: Filter` field added directly onto
`ForEachOp` (not reusing `when`), with the recheck-vs-once distinction
expressed as a flag, is closer to viable syntactically, but still fights
the grain of the type: `ForEachOp.over` is a `FilterValue` because
"how many times" is meant to be resolved from an existing list — adding a
`condition: Filter` field to the SAME struct that also carries `over:
FilterValue` produces an op that is simultaneously "iterate over this
known list" AND "keep going while this predicate holds", which is two
independent stopping mechanisms layered onto one struct. What does it
mean if `over` is exhausted (K elements consumed) but `condition` is
still true? What does it mean if `condition` goes false at iteration 3
but `over` has 10 elements? The combination is a bigger surface to
specify and test than either mechanism alone, for a use case (the
canonical scenario in §1) that doesn't actually have a natural `over`
value at all — there is no pre-existing list to iterate over in the
escrow-sweep example; the flag would be bolted onto a field
(`over`/`bind_row`) that the while use case has no use for.

**Recommendation on this point:** a flag-in-the-condition is not the
cleanest shape. The investigation supports a **distinct `BatchOp::While`
variant**, analogous to how `ForEach` is already distinct from `Batch`
rather than being "`Batch` with an `is_loop: bool` flag." Precedent
already set by this exact codebase: Epic04 explicitly chose NOT to add a
`repeat: N` field onto `Batch`/`ForEach` and instead degrade "repeat N" to
a literal-array `over` (Epic04 ADR Decision 1, "No dedicated `repeat: N`
field is introduced — keeping the wire surface to one new op variant, one
new struct"). The same reasoning applies here in the opposite direction:
a while-loop's shape (a `condition: Filter` re-evaluated per iteration,
no `over`, no `bind_row` — there is no "current element" to bind, only
loop-carried state already visible via `$query` refs to prior
iterations' own writes) is different enough from `ForEach`'s shape
(a pre-resolved list + per-element binding) that cramming both into one
struct via a flag creates more special-casing than a second variant
would. A sketch, not a commitment:

```rust
pub struct WhileOp {
    /// Re-evaluated after EVERY iteration (including a check before
    /// iteration 0) against the current resolved_refs — unlike
    /// QueryEntry.when's single pre-op evaluation.
    pub condition: Filter,
    pub batch: BatchRequest,
    /// Mandatory (see §5) — no static list length exists to fall back on.
    pub max_iterations: usize,
}
```

No `over`/`bind_row` — there is no "current element" in a while-loop; the
body reads whatever state it needs via ordinary `$query` refs onto its
own prior writes (the same alias re-resolves fresh at the top of each
iteration, since `resolved_refs` reflects the just-committed state).

---

## 4. Evaluate the "compose for_each + when" alternative

FINAL-SUMMARY.md §6 speculates: "with `when` actually working correctly,
some of the motivating use cases for `while` may be achievable via
repeated `for_each` + `when` composition instead of a wholly new
primitive." Evaluating this directly, concretely, against the escrow
scenario from §1.

**The workaround, spelled out:** wrap the loop body in `for_each` over a
generously-sized literal array whose only purpose is to bound the
iteration count (e.g. `over: [0, 1, 2, ..., 19]`, a fixed 20-element
placeholder array with `bind_row` never referenced by the body), and add
a `when` guard on the body's actual mutating op (the debit/transfer),
using `Filter::ValueCompare` (now that #651 is fixed, per commit
`3315f414`) to test `hold_balance >= installment` fresh against the
latest `@escrow_account` read inside each iteration. Once
`hold_balance < installment`, that iteration's `when` evaluates false and
the mutating op is skipped (`resolve_skip`, `query_runner.rs:102-141`)
— but the *loop itself* keeps running through all 20 placeholder
iterations regardless, each one now a no-op skip.

**Does this actually work, mechanically, today?** Partially, and with
real caveats:

1. **It requires a fresh read of the mutated field inside every
   iteration's body**, so that `when`'s `Filter::ValueCompare` sees the
   post-previous-iteration value, not a stale one resolved before the
   loop started. This is achievable: each iteration's body can `Read`
   `@escrow_account` again (each iteration is its own recursive call with
   its own fresh `resolved_refs`, `query_runner.rs:429-458`), so the read
   genuinely re-executes and sees the latest state from the prior
   iteration — **as of #661's fix**, a transactional outer batch threads
   the SAME `TxContext` through every iteration
   (`run_nested_body_in_outer_tx`), so prior iterations' writes really are
   visible to (and, on any later failure, really do roll back alongside)
   subsequent iterations; before that fix, each iteration's writes
   committed independently the moment the iteration finished, regardless
   of the outer batch's `transactional` flag. So the DATA side of the
   workaround is real, not fake.
2. **The upper bound must be guessed, and guessing wrong is a real
   failure mode in both directions.** Too small (fewer placeholder
   elements than iterations actually needed): the loop exhausts its
   `over` array before the real condition would have stopped it, silently
   leaving the escrow only partially swept — no error, just an
   incomplete result, because `for_each` has no concept of "ran out of
   `over` before the job was done"; it just stops, successfully, having
   done less work than intended. Too large: wasted iterations, each one
   still costing a real recursive `execute_batch_impl` call (its own
   planning pass, `Read` for the guard check, `when`-evaluation
   machinery) even though the guard causes the mutating op inside to
   skip — not free, per Epic03's own benchmark finding (Epic03/F,
   referenced in FINAL-SUMMARY.md §3: skip-vs-full-execution has a real,
   measured, non-zero cost even for a skip) and Epic04/F's finding that
   `ForEach` overhead grows with N rather than amortizing
   (FINAL-SUMMARY.md §3, "roughly 1.5-1.6x slower... growing slightly
   with N"). A caller who wants correctness (never silently under-run)
   must therefore *deliberately over-provision* the bound, directly
   trading correctness-safety for wasted-iteration cost — there is no way
   to have both without knowing the true count in advance, which is
   exactly the information a while-loop's condition is supposed to make
   unnecessary.
3. **`max_iterations` interacts awkwardly, as the brief anticipated.**
   `BatchLimits::max_iterations` (default 1000,
   `crates/shamir-query-types/src/batch/batch_limits.rs:60,71`) is a
   ceiling on `for_each`'s **resolved element count**, checked once before
   iteration 0 (`query_runner.rs:417-427`). Using a placeholder array to
   emulate "run until condition fails" pushes the caller into choosing
   between two bad options: pick a placeholder length close to
   `max_iterations` "just in case" (near-worst-case cost on every
   invocation, even the common case that only needs 5 real iterations),
   or pick something small and risk silent under-completion per point 2.
   Neither choice reflects what `max_iterations` was designed to gate —
   it was designed as a DoS backstop against a genuinely-large *known or
   discoverable* count (ADR Decision 3: "the real DoS backstop is the
   product `iterations × body.queries.len()`"), not as the caller's
   actual iteration-count planning mechanism. The workaround repurposes a
   safety ceiling as a business-logic parameter, which is a sign the
   abstraction is being bent past its intended use.
4. **The static DoS fold-in (`planner.rs:126-151`) still fires on the
   full placeholder length**, even though most of those iterations will
   be no-op skips at runtime — the planner has no way to know some
   fraction of a literal-array `for_each`'s iterations will be
   `when`-skipped, since `when` evaluation is a runtime-only decision
   (Epic03 ADR Decision 2/3). So the plan-time budget check charges the
   FULL placeholder-array cost against `max_queries`, not the
   (potentially much smaller) real number of non-skipped iterations —
   another instance of the abstraction being bent: the caller pays
   planning-budget cost for a guessed worst case, exactly the "waste" a
   real while-loop with a runtime-only ceiling (§5) would not force.
5. **Ergonomics: the guard has to be re-stated per-iteration, correctly,
   by the caller**, with no engine-level guarantee that the `when` filter
   actually corresponds to the loop's true continuation condition (a
   typo'd or drifted `when` clause silently produces a different
   iteration count than intended, with no error — this is inherent to
   composing two independent primitives rather than having one primitive
   whose condition IS the loop's contract).

**Verdict on the composition alternative:** it *functions* for the
common case (a small, boundable number of iterations, one transaction,
willingness to over-provision `over`'s length) — it is not "broken" the
way it would have been before #651 was fixed (before the fix, the `when`
guard's field-based comparison would have silently folded to a constant,
making this workaround non-functional regardless of loop-count
reasoning). But it is a real workaround with real, stated costs: an
upper-bound guess that trades correctness against waste, a `max_iterations`
ceiling repurposed away from its DoS-backstop design intent, and a
planner budget charged against the guessed worst case rather than the
real work performed. It is "good enough" for bounded, low-stakes loops
where over-provisioning cost is acceptable; it is not a substitute for a
primitive whose iteration count is allowed to be genuinely open-ended.

---

## 5. Authorization/limits implications

`for_each`'s two safety mechanisms both lean on the same fact: **the
iteration count is boundable before or at the very start of execution.**

- **Authorization (Epic04 ADR Decision 5):** `ForEachOp.batch`'s body is
  authorized as a template, exactly once, at the parent batch's planning
  time, via `is_write = fe.batch.queries.values().any(|qe|
  qe.op.is_write())` — pessimistic, iteration-count-independent
  classification, identical to `SubBatchOp`'s (`batch_op.rs:752` per the
  ADR). Crucially this works regardless of whether the loop runs 0, 1, or
  K times, because "does the body contain a write" is a structural
  question about the body's *shape*, answerable without ever running it.
- **DoS limit (Epic04 ADR Decision 3):** the static fold-in
  (`planner.rs:126-151`) handles the literal-array case at plan time; the
  dynamic `@alias[].field` case falls back to a **runtime check
  immediately before iteration 0** (`query_runner.rs:417-427`) — but that
  runtime check still runs entirely BEFORE any of the K iterations begin,
  because the resolved list's length is known in full the instant `over`
  resolves, before element 0 is touched.

Both mechanisms exploit the same property: **a while-loop's total work is
never knowable at any single checkpoint, including immediately before
iteration 0** — that's the entire premise of "while", as opposed to
"for [a count discoverable before start]". This has two concrete
consequences:

1. **The authorization story is actually fine, unmodified.** A
   `WhileOp.batch`'s `is_write` classification would be exactly as
   pessimistic and exactly as static as `ForEach`'s — "does the loop
   body's static shape contain a write", independent of how many times
   the loop runs (including the edge case "the condition is false at
   iteration 0, zero iterations run" — the loop is still classified
   `is_write() == true` if its body would write, mirroring `ForEach`'s own
   "zero iterations still counts as a write op" precedent, ADR Decision 5:
   "a `ForEach` whose `over` happens to resolve to an EMPTY list at
   runtime... is still classified `is_write() == true` at plan time"). No
   new authorization mechanism is needed — this part of the design
   transfers cleanly.
2. **The DoS story cannot transfer cleanly, and this is the crux.**
   There is no "static fold-in" option at all for a while-loop (there is
   no `iterations` operand — see §2 point 2) and there is no single
   "runtime check immediately before iteration 0" that bounds total work,
   because the whole reason to reach for a while-loop is that the total
   count is unknown even at that checkpoint. The ONLY remaining lever is
   a hard `max_iterations` ceiling **checked every iteration, during the
   loop**, aborting once the count is exceeded — turning what is an
   optional-but-generous safety margin for `for_each` (1000, "two orders
   of magnitude above `max_queries`'s default... generous enough for
   realistic fan-out sizes", ADR Decision 3) into something categorically
   different for a while-loop: **the only thing standing between a
   caller's buggy or malicious condition and genuine infinite work.**
   For `for_each`, if `max_iterations` were somehow removed entirely, the
   loop would still terminate on its own (bounded by the real length of
   `over`) — the limit is a backstop against an unexpectedly large but
   still-finite count. For a while-loop, if the ceiling were removed (or
   set too high, or misconfigured), a condition that never goes false
   (a bug in the caller's `Filter::ValueCompare`, or a body that doesn't
   actually make progress toward the condition — e.g. an off-by-one that
   decrements the wrong field) genuinely never stops. This makes
   `max_iterations` for a while-loop **mandatory in a load-bearing sense
   `for_each` never required it to be** — not a generous DoS backstop but
   the sole termination guarantee against a category of caller mistake
   that is otherwise unbounded. Concretely: `WhileOp.max_iterations`
   should not default from `BatchLimits` the way `ForEach` inherits the
   ambient 1000 default implicitly — it should be a required field on
   `WhileOp` itself (no default, or a much smaller default than 1000,
   since 1000 iterations each re-planning + re-executing a full recursive
   `execute_batch_impl` call, per iteration cost characteristics already
   measured as non-trivial in Epic04/F, is a much larger unit of "possibly
   wasted work" than 1000 elements of a `for_each` over pre-existing,
   already-fetched data).
3. **The mid-loop-abort shape `for_each`'s design explicitly avoided
   (ADR Decision 3: "never a partial run followed by a mid-loop abort")
   is unavoidable for a while-loop.** By definition, hitting the ceiling
   can only be detected AFTER some number of iterations have already run
   (there is no equivalent "check the length once before iteration 0",
   because there is no length to check). So a while-loop's
   `TooManyIterations`-equivalent error necessarily happens mid-loop,
   after real work (and, inside a transaction, real uncommitted writes)
   has already accumulated — reproducing exactly the "wasted work +
   contended lock time" cost the Epic04 ADR called out as the reason to
   front-load `for_each`'s check (Decision 3: "running 1000 iterations of
   real work and then aborting on iteration 1001 wastes the work AND
   still produces the same... outcome, but slower"). A while-loop cannot
   avoid this; it can only bound how bad it gets, via a conservative
   ceiling.

---

## 6. Recommendation

**(c) — build a narrower middle-ground, not a true unbounded while-loop,
and only if a concrete use case justifies it; otherwise, don't build
anything yet.**

Reasoning against the effort/value tradeoff, in the same spirit as the
Epic01 ADR's "stage parallelism: closed, not implemented" precedent (a
reasoned "no" is as valid an outcome as a "yes" — see
`docs/dev-artifacts/design/oql-01-stage-parallelism-adr.md`'s "No evidence
of benefit yet" / "Phase A is scoped to correctness, not performance"
framing):

1. **A true, unbounded while-loop is the highest-effort, highest-risk
   option, with a smaller-than-it-looks value delta over the
   composition workaround.** §4 shows the `for_each` + `when` composition
   is not broken (post-#651) and does cover the common case — the
   *marginal* value of a real while-loop is specifically "the caller no
   longer has to guess an upper bound and doesn't pay for wasted
   placeholder iterations." That's real, but §2 and §5 show the
   corresponding engineering cost: a new `BatchOp` variant whose DoS story
   cannot reuse ANY of `ForEach`'s static-or-front-loaded checks (§5 point
   2), whose error semantics necessarily reintroduce the exact
   mid-loop-abort shape Epic04 was designed to avoid (§5 point 3), and
   whose planner integration — while mechanically similar to `ForEach`'s
   dependency-extraction pattern (§2) — still requires new executor-level
   control flow (re-evaluating a condition after every iteration, a
   position in the code that does not exist today) that is a genuinely
   new capability, not a parameter on an existing one.
2. **No concrete, current product requirement demands unbounded
   iteration specifically.** The motivating scenario in §1 (escrow sweep)
   is naturally bounded in practice — the number of installments is
   itself bounded by real-world business constants (a hold balance
   divided by a minimum installment size is always some finite, generally
   small number in any real deployment). The theoretical case for
   "truly unbounded" (a condition that could in principle iterate
   arbitrarily many times) is a generality without an attached use case
   in this codebase today, whereas the "bounded, need-a-real-ceiling-
   anyway" case is what every realistic scenario actually looks like once
   examined concretely (as also confirmed by the mandatory-ceiling
   conclusion in §5).
3. **A narrower middle ground captures nearly all the value at a
   fraction of the design cost.** Sketch: a `BatchOp::Repeat` (name TBD)
   that is structurally `for_each` over an internally-synthesized
   `0..max_iterations` counter (no caller-visible placeholder array to
   guess — the ceiling itself, which §5 already established is mandatory
   for a while-shape anyway, doubles as the iteration source), combined
   with a `condition: Filter` checked at the TOP of each iteration
   (before running the body), re-evaluated fresh every time against the
   current `resolved_refs` — stop as soon as it's false, without running
   the remaining placeholder iterations. This is much closer to
   `for_each` than to a true while-loop:
   - **Planner integration is nearly free** — it reuses `ForEach`'s exact
     dependency-extraction and nesting-depth-walk pattern (§2's "small,
     mechanical extension" half), since the iteration source
     (`0..max_iterations`) is always a plan-time-known literal count, so
     the EXISTING static DoS fold-in (`planner.rs:126-151`) applies
     completely unmodified — no new planner-side check is needed at all,
     unlike a true while-loop (§5 point 2).
   - **The ceiling is load-bearing by construction, not bolted on** — it
     IS the iteration source, so there's no separate "mandatory
     max_iterations" design question to solve; it's already there because
     `Repeat` needs a count to be a `for_each`-shaped node in the first
     place.
   - **Error semantics stay front-loaded relative to a true while-loop**
     — the *maximum* possible work is exactly as bounded and
     plan-time-visible as `for_each`'s literal-array case (Decision 3's
     "never a partial run followed by a mid-loop abort" property is
     preserved for the OUTER bound — the ceiling itself is never a
     surprise; only "did we stop early via the condition" is a runtime
     fact, which is a normal, expected early-exit, not an error).
   - **It still doesn't solve genuinely-unbounded cases** — if a real
     product need for open-ended iteration ever materializes (not yet
     observed in this codebase), `Repeat`'s fixed ceiling means the caller
     must still pick a large-enough bound, same tradeoff as §4's
     workaround, just without the wasted-placeholder-array ergonomics
     complaint and with a cheaper, existing-mechanism planner check.

**Concrete recommendation:** do not build a true `BatchOp::While` now.
The `for_each` + `when` (post-#651) composition already covers the
realistic cases (§4), with clearly-documented costs (guess an upper
bound, pay for skipped iterations, `max_iterations` repurposed as a
budget). If those costs become a real, felt problem in practice (not
merely theoretical), the next step is the `Repeat`-with-early-exit
middle-ground sketched above — NOT a full while-loop — because it
captures the ergonomic win (no placeholder array to guess, condition
re-checked per iteration, early exit) while inheriting `for_each`'s
already-solved planner/DoS/authorization story almost unchanged, rather
than reinventing all three for a fully general primitive whose marginal
expressiveness (truly unbounded iteration) has no concrete use case
backing it in this codebase today. If a future task pursues `Repeat`, it
must follow the same ADR-first discipline as Epic03/Epic04 (a dedicated
ADR settling the wire shape, condition-evaluation-position semantics, and
limits story) before any code is written — this document is the
feasibility groundwork for that ADR, not a substitute for it.
