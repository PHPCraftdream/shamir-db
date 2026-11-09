# Brief: design addendum for PROC-2c (`call`, result references, update/delete)

## Why

The `shamir` language procedures (milestones 7a/7b) can read and insert through the
direct intrinsics and the typed batch builder, but not: call another stored function,
use the result of one batch query in another (`$query` references, `after`
dependencies), or build update/delete operations. Today these are explicitly out of scope
in `docs/dev-artifacts/design/shamir-lang-design.md` (sections 4 and "Non-goals", the host
import table row `call`). Before any implementation, the design must be written and
reviewed: this brief produces ONLY the design addendum.

## What

Write one new document, `docs/dev-artifacts/design/shamir-lang-call-refs-addendum.md`.
Do NOT edit any other file (a parallel task is editing `shamir-lang-design.md`; the
orchestrator will link the addendum afterwards). Read first, in this order: the design
document `shamir-lang-design.md` (sections 3, 4 and 6 to 8), the implementation of the
typed builder in `crates/wasm/shamir-lang/src/back/batch` and
`back/emit/support/db.rs`, the host imports in
`crates/wasm/shamir-wasm-host/src/wasm/bindings/` (`call.rs`, `db.rs`), the gateway in
`crates/wasm/shamir-wasm-host/src/gateway/db_gateway.rs`, the native builder
`crates/query/shamir-query-builder` (batch aliases, references, `after`, update and delete
builders) and the engine's reference/dependency handling in `shamir-query-types`.

The addendum decides, with the real constraints of the code and the same level of rigor
as the main design (exact signatures, typing rules, lowering, error cases, bounds):

1. **`call`**: language surface and typing for calling a stored function from a
   procedure (named/positional arguments, result type, nullability); which entries may
   call which (capability ceiling, existing recursion/call-graph rules); effect
   inference; identity semantics (`Security::Definer`/`Invoker` of the callee and the
   chain), permissions checked at which point, recursion and depth limits across
   functions, fuel/work accounting across the nested call, cancellation, and what the
   existing host import `call` already does and does not guarantee. State clearly what
   needs a host change and what does not.
2. **Result references**: how a later query/insert of one batch refers to a value of an
   earlier one (`$query` references, `after`): typed surface in the language, how it
   lowers to the existing request DTO (check the native builder and the wire format for
   the exact representation), static checks (alias existence, order, cycles, type of the
   referenced value), and how the keep-order flag and the DTO oracle tests extend.
3. **update/delete in the builder**: the typed surface and lowering for update and
   delete operations, filters, return shapes, and the interaction with permissions
   (which action the gateway checks for each) and with the `Definer` identity.
4. A **phased implementation plan** (small stages, each with crates/files, tests that
   prove it, risks), an explicit **non-goals** list, and **open questions** that need
   the product owner (as a short list; do not invent answers).
5. A "Verified against the code" list: each factual claim about existing behaviour
   with the file it comes from; anything not verified goes to a "Not verified" list.

Write for a reviewer who will challenge the design: prefer exact rules and examples
(small source snippets of the proposed surface and the request they lower to) over prose.
Examples that construct queries use the project's builder style, never hand-written JSON
(the wire-format snippets are the documented exception when they show the wire shape).

## Observable acceptance

1. Only the one new document is added; no code, no other doc changes.
2. Every claim about existing behaviour names its source file; open design points are
   listed, not hidden.
3. No private machine paths, no mention of how or where CI runs, no version changes.
4. `python -I scripts/ci/check_layout.py` passes.

## Constraints

Documentation task: no builds or tests. Only read-only git, reading files and
`python -I scripts/ci/check_layout.py`. No cargo, no nextest, no npm.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.
