# Brief: scalar read references in typed batches (PROC-2c stage 3)

## Why

Stages 1 and 2 of PROC-2c landed (`crates/17-proc2c-update-delete.md`, `crates/20-call-checked.md`). The `shamir`
language builds typed batches (`batch`, `batch_read`, `batch_insert`, `batch_update`, `batch_delete`, `db_execute`,
`response_*`) but an operation cannot use a value read by an earlier operation of the same plan; a procedure must
execute twice and splice the value by hand. The reviewed design is
`docs/dev-artifacts/design/shamir-lang-call-refs-addendum.md`: read section 3 (Deferred scalar read references),
the stage 3 row of section 6, the stage 3 bullet of section 7, decision 6 of section 9 and findings F4, F6, F8, F10,
F11 of section 10. Decisions there are made; do not re-open them. Read the compiler files first:
`crates/wasm/shamir-lang/src/sem/check/builtins/batch/*`, `back/batch/{lower,template}.rs`,
`back/emit/support/db.rs`, and the native side that gives the marker its meaning:
`crates/engine/shamir-engine-query/src/query/filter/evaluation/semantics/resolve.rs` (~755-807),
`crates/engine/shamir-engine/src/query/batch/runner/param_subst.rs` (~239-271),
`crates/query/shamir-query-builder/src/read/val/filter_value.rs` (`qref`).

## What

1. **Surface (compiler notation, not a public type).** One intrinsic `query_ref_field(<earlier read op>, "<alias>", <index
   literal>, "<field>")` (name may follow the existing intrinsic naming; say what you chose) usable ONLY as (a) a
   value in a `batch_read` equality record and (b) a field value of `batch_insert`. Accepted path is exactly
   `[n].field` (non-negative literal index, one literal field name). Rejected statically with the existing
   diagnostics (`T010` for provenance/sink facts that cannot be proved, `T004` for type mismatch, `T009` for
   unsupported boundaries): whole-row, `[].field`, columns/lists, nested paths, producers that are not reads
   (insert/update/delete), sinks other than read equality and insert values (update `set`, update/delete predicates,
   `batch_after`), forward or self references, unknown alias, an alias from another plan value, a reference whose
   expected scalar type cannot be derived from the sink.
2. **Wire form and lowering.** The reference lowers to the exact marker the native builder produces
   (`{"$query":"@alias","path":"[n].field"}`, alias normalised to `@alias`, F10); the dependent operation is placed
   after its producer by the existing planner (waves, F6), no `after` edges are added. Byte oracle against the native
   `Batch::query` / `Batch::insert` + `qref` builder output (the existing oracle tests in `tests/` show how).
3. **Soundness gate (the explicit stage 3 gate; decide with evidence, do not assume).** Static checking proves alias,
   order, producer/sink eligibility; it does NOT prove that the runtime row has the field or that the value has
   the type the sink needs. The runtime gate must be exactly this, with no engine change unless you can show it is
   impossible without one:
   * every operation that carries a reference gets a `when` guard that makes the engine skip it unless the
     referenced value exists and has the sink's scalar kind (use the existing value navigation / `type_of` style
     filter functions, compare with `value_nav/type_of` in `crates/query/shamir-funclib`; read how `when` and
     reference resolution interact in `param_subst.rs` and `batch_execute.rs` first: an unresolved reference must
     fail or skip BEFORE dependent dispatch and must never be sent as a wrong-typed literal);
   * after `db_execute` the guest checks, for each operation that carries a reference, that the response entry was
     not `skipped`; a skipped guarded operation maps to runtime kind 2 (plan/reference failure) with a stable
     message; malformed value shapes remain kind 3.
   If you find that this cannot be made sound with the existing mechanism (for example the guard cannot express
   "present and of kind X" for the needed kinds, or an unresolved reference can still reach dispatch), STOP the
   implementation of the unsound part, keep the compiler rejecting it, and report precisely what is missing; a
   partial, sound subset (for example only string/i64 sinks) is acceptable and preferred over an unsound whole.
4. **No other surface.** No `batch_after`, no whole-row/list/column refs, no refs from writes, no engine/DTO/ABI/
   import changes (the import ceiling stays six), no transactional reference mode, no version changes.

## Tests (run in CI, not here)

* Compiler: positive read->read equality and read->insert scalar; each rejection above with its diagnostic; byte oracle
  vs the native builder; guard presence on every referencing operation.
* Host + `shamir-db` e2e with real compiled procedures (helpers in `crates/engine/shamir-db/tests/functions/gateway/`):
  value flows from the read into the dependent insert and read; missing row / missing field / wrong runtime type ->
  the dependent operation does not dispatch and the procedure fails with kind 2; independent operations keep the
  planner's wave order with insertion-order tie break (assert the observable order); fresh independent invocations
  after a failure behave normally (keep-order reset caveat, F8: assert terminal behaviour only, no claim about trap
  cleanup); Definer/ACL cases: the reference does not read anything the actor could not read (the read op is
  authorized as before).
* Never delete or weaken existing tests; list every changed one with the reason (read the real behaviour first).

## Observable acceptance

fmt/check/clippy clean for every touched crate; layout limits (<=7 entries per folder, <=1000 lines per file; the
compiler's `check`/`back` folders are close to the limits, split rather than grow; rerun the layout check on the final
tree); rust-guard rules (no logic in `mod.rs`, no inline test modules, no local `use`, no new `#[allow]`, no
visibility widening beyond what is strictly needed, list each). Docs in the same change: design doc intrinsic table
and call chapter, addendum status line (stage 3 implemented or the precise remaining gate), one sentence in the
`shamir-lang` paragraph of `CLAUDE.md` and `AGENTS.md`.

## Constraints

Compilation, builds and tests run in CI, not here. Allowed locally only: `cargo fmt -p <crate>`,
`cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`,
`python -I scripts/ci/check_layout.py`, read-only git. Run cargo only under `wnice -p idle -c 50 -t 8 -m 50 --` with
`-j 2`. No `cargo build/test/run/bench/doc`, no nextest, no npm.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the
working tree or index. Only edit files; the orchestrator commits. (`git add -A` only for the layout check, if your
tool layer refuses it verify the layout over the working tree yourself.)

Final report: what is implemented (and what the soundness gate decided, with the evidence), tests added/changed and
why, widened items, anything unverified. Do not end on a tool call.
