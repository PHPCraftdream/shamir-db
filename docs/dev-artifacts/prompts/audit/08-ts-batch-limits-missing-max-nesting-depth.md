Task: CRIT-8 — TS client's `Batch.limits()` always breaks the request
because `BatchLimits` is missing a required field the server expects.

## Where

- Rust wire type (source of truth): `crates/shamir-query-types/src/batch/batch_limits.rs:29-59`
  — `BatchLimits` has **5** required fields, `#[derive(Deserialize)]` with
  **no** `#[serde(default)]` on any field:
  ```rust
  pub struct BatchLimits {
      pub max_queries: usize,
      pub max_dependency_depth: usize,
      pub max_execution_time_secs: u64,
      pub max_result_size: usize,
      pub max_nesting_depth: usize,   // <-- missing on the TS side
  }
  ```
- TS type: `crates/shamir-client-ts/src/core/types/batch.ts:89-94` —
  `BatchLimits` interface has only **4** fields (no `max_nesting_depth`).
- TS builder: `crates/shamir-client-ts/src/core/builders/batch.ts:34-39`
  (`DEFAULT_LIMITS`) and `:193-203` (`.limits()` method) — same 4 fields,
  missing `max_nesting_depth` and its default (4, per the Rust
  `Default` impl at `batch_limits.rs:48-56`).
- Test that BAKES IN the bug as the expected shape:
  `crates/shamir-client-ts/src/core/builders/__tests__/batch.test.ts:143-153`
  (`'limits fills missing fields with defaults'`) — asserts the built
  request's `limits` object has exactly 4 fields, which is the WRONG
  (incomplete) shape.

## Why this is CRITICAL

Any call to `Batch.create()....limits({max_queries: 20})...build()`
followed by `.execute()` sends a `limits` object missing
`max_nesting_depth` over the wire. The server
(`crates/shamir-server/src/db_handler/handler.rs:251` area — confirm
exact line when reading) deserializes `BatchLimits` with serde's
default (all-fields-required) behavior, so it rejects the ENTIRE batch
with `invalid_request: missing field 'max_nesting_depth'` — an opaque
protocol-level failure with no indication to the caller that `.limits()`
is the culprit. This makes `.limits()` unusable from the TS client
today; the existing test locks in the broken 4-field shape as if it
were correct.

## Fix (required — CRITICAL, TS-side parity)

1. Add `max_nesting_depth: number;` to the `BatchLimits` interface in
   `crates/shamir-client-ts/src/core/types/batch.ts`.
2. Add `max_nesting_depth: 4` to `DEFAULT_LIMITS` in
   `crates/shamir-client-ts/src/core/builders/batch.ts` (4 is the Rust
   default — `BatchLimits::default().max_nesting_depth`, per
   `batch_limits.rs:56`, and matches the doc-comment
   `/// Maximum sub-batch nesting depth. 0 = no nesting allowed.`).
3. Wire it through the `.limits(partial: Partial<BatchLimits>)` method
   body (`batch.ts:193-203`) the same way every other field is handled:
   `max_nesting_depth: partial.max_nesting_depth ?? DEFAULT_LIMITS.max_nesting_depth,`
4. Fix the test at `batch.test.ts:143-153` — it currently asserts the
   WRONG (4-field) shape as correct. Update the expected object to
   include `max_nesting_depth: 4`, so the test now asserts the CORRECT
   full 5-field parity shape, not the bug. Do not just add a new test
   alongside the broken one — the existing test's assertion is itself
   the bug-as-spec and must be corrected.
5. Check for any other parity fixture/snapshot referencing the 4-field
   `BatchLimits` shape (grep the crate for `max_result_size` /
   `DEFAULT_LIMITS` / `max_dependency_depth` to find any other
   hardcoded 4-field literal) and fix those too — the audit specifically
   calls out "добавить поле в тип+дефолты (4) и в parity-фикстуру".

## Optional secondary hardening (mentioned in the audit as a longer-term
fix, NOT required to close CRIT-8 — do only if the primary fix above is
done and gate is clean, and keep it a SEPARATE, clearly-labeled part of
your diff so it can be reviewed/reverted independently)

The audit note (`docs/dev-artifacts/audits/2026-07-06-client-surface-parity.md:63`)
suggests adding `#[serde(default)]` to each field of the Rust
`BatchLimits` struct (`batch_limits.rs`), so the server tolerates a
client that sends only a SUBSET of limit fields (each defaulting to the
Rust-side default when absent), rather than requiring the client to
always send the full struct. This is defense-in-depth against the same
class of drift recurring for a 6th field in the future. If you attempt
this: add `#[serde(default = "...")]` per-field (each field needs its
own default function or a shared `Default` derive pattern — check how
other structs in this codebase already do partial-defaults via serde,
if any, for a consistent idiom), and confirm no other caller/test
relies on `BatchLimits` deserialization REJECTING a partial object.

## TDD requirement

1. **Red**: before your fix, `batch.test.ts:143` passes (wrongly) with
   the incomplete 4-field object — that's the bug locked in as a test.
   Confirm you understand why: the test's `toEqual` assertion literally
   omits `max_nesting_depth` from the expected value, so it can never
   catch its own absence.
2. **Fix + correct the test** as described above so the test's expected
   object matches the true 5-field Rust `BatchLimits::default()` shape.
3. Add (or extend) a test asserting the FULL request built by `.limits()`
   round-trips through JSON encoding with all 5 fields present — this is
   what would have caught the bug pre-fix (a test asserting only 4 keys
   passes regardless of whether the 5th is missing or present-but-wrong,
   so make sure the new/fixed assertion is precise, e.g. `Object.keys(req.limits).sort()`
   check or an exact `toEqual` with all 5 keys).

## Test scope command

This is a TS package — check `crates/shamir-client-ts/package.json` for
the test runner (likely `vitest` or `jest`) and run its scoped test
command, e.g.:

```
cd crates/shamir-client-ts && npm test -- batch.test.ts
```

(Confirm the actual invocation from `package.json` scripts — do not
guess a runner that isn't configured.)

## Gate (must be clean before finishing)

Run whatever this package's lint/typecheck step is (check
`package.json` scripts — likely `tsc --noEmit` and/or `eslint`). Rust
side is untouched unless you also did the optional secondary hardening,
in which case additionally run:

```
cargo fmt -p shamir-query-types -- --check
cargo clippy -p shamir-query-types --all-targets -- -D warnings
./scripts/test.sh -p shamir-query-types
```

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

When done, report exactly:
- Which files changed and the exact fields/defaults added.
- Confirmation the pre-existing test at `batch.test.ts:143` was
  CORRECTED (not left as-is, not just supplemented) to assert the true
  5-field shape.
- Whether you found and fixed any other 4-field `BatchLimits` literal
  elsewhere in the TS package.
- Whether you attempted the optional Rust-side `#[serde(default)]`
  hardening — if yes, keep that as a clearly separate, describable part
  of the diff.
- Test/lint/typecheck results (exact commands run + pass/fail).
