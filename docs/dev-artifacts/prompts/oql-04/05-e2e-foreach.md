# Epic04/Phase E — e2e Rust+TS tests for loops (#656)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

`BatchOp::ForEach` (Epic04) is fully implemented and unit-tested:
- Engine: Phase B, commit `6ff521d5`.
- Rust+TS builders: Phase C, commit `79510a13` (`Batch::for_each` /
  `Batch.forEach`).
- Unit-test gap-closure: Phase D, commit `7ed75075`.

This phase proves the primitive over a REAL wire round-trip — a real
`ServerLauncher` (TCP), a real `shamir_client::Client` / TS `ShamirClient`,
real SCRAM handshake — not an in-process planner call. Mirror the
established e2e pattern from Epic03/E exactly:

- Rust twin: `crates/shamir-client/tests/batch_when_e2e.rs` — read this file
  FIRST, in full. Copy its `fast_kdf()`/`make_config()`/`boot()` helpers
  verbatim (same server boot ceremony every e2e file in this repo uses).
  `shamir_client::builder` is a direct re-export of `shamir_query_builder`
  (see `crates/shamir-client/src/lib.rs:59`), so `Batch::for_each` is already
  available there with no new wiring.
- TS twin: `crates/shamir-client-ts/src/__tests__/e2e-when.test.ts` — read
  this file too, and its `SERVER_AVAILABLE`/`startServer`/`HOST` harness
  imports (find the harness file itself, likely
  `crates/shamir-client-ts/src/__tests__/e2e-harness.ts`).

New files to create:
- `crates/shamir-client/tests/batch_for_each_e2e.rs`
- `crates/shamir-client-ts/src/__tests__/e2e-for-each.test.ts`

## Scenario (canonical, and per-language identical in spirit)

Unlike Epic03's `when` (which hit a real blocking bug — field-based
comparisons always fold to a fixed result, #651, still open), `ForEach`'s
`over` has NO such limitation: `over` can be a genuine `$query` column
reference, resolved once against real data, with no scratch-interner
involved (see `docs/dev-artifacts/design/oql-04-loops-foreach-adr.md`'s
"Bug #651 — independence of `bind_row`" section — confirmed independent).
So this phase's e2e tests should exercise the REAL, INTENDED, canonical
scenario, not a workaround.

**Canonical scenario**: "read all order ids for a customer, then insert one
audit-log row per order, each row referencing that order's id" — in ONE
transactional batch:

1. Create db + repo + tables: `orders` (seed with a few rows for a given
   `customer_id`) and `audit_log` (empty).
2. Transactional batch:
   - `orders_q`: `Query::from("orders").where_eq("customer_id", ...)` — a
     real read.
   - `loop`: `Batch::for_each("loop", orders_q.column("id"), "order_id",
     inner_batch)` where `inner_batch` inserts one row into `audit_log` with
     `{"order_id": {"$param": "order_id"}, "note": "audited"}`.
3. Assert: the batch response's `loop` result is a `QueryValue::List` with
   exactly as many elements as seeded orders; a follow-up read of
   `audit_log` shows exactly one row per order, each row's `order_id`
   matching a real seeded order's id (not a placeholder/synthetic value —
   this proves `over` resolved real cross-query data end-to-end over the
   wire, unlike `when`'s workaround).

Cover at least these scenarios (both languages):

1. **Basic query-driven loop** — the canonical scenario above. Prove
   real data flows from `over` into each iteration's `bind_row` parameter,
   and that the loop body's writes land correctly, once per element.
2. **Zero iterations** — seed zero matching orders; assert the loop result
   is an empty list and NO audit rows are inserted, over the real wire.
3. **Literal-array `over`** — a `for_each` whose `over` is a small literal
   array of ids (not a `$query` ref), proving that source works over the
   wire too (not just in-process).
4. **Error mid-loop in a transactional batch** — force one iteration to fail
   (e.g. a unique-index violation, same technique as
   `for_each_iteration_error_stops_at_first_in_non_tx_batch` /
   `for_each_iteration_error_aborts_whole_tx_batch` in
   `crates/shamir-engine/src/query/batch/tests/for_each_tests.rs` — read
   those for the exact construction) — assert the WHOLE transactional batch
   is rolled back (no partial audit rows survive), over the real wire.

## Verification (MANDATORY before you report done)

- Rust: `./scripts/test.sh -p shamir-client -- for_each` (or `@e2e` scope
  if that's more appropriate — check `scripts/test.sh`'s scope_args) must be
  green. This is a real e2e test (spins up a real server), so it WILL be
  slower than a unit test — that's expected, not a hang.
- TS: run the TS e2e test suite for this one new file specifically (check
  how `e2e-when.test.ts` is invoked — likely a vitest command with a longer
  timeout / `SHAMIR_SKIP_STALE_BINARY_CHECK` consideration; read
  `crates/shamir-client-ts/src/__tests__/e2e-harness.ts`'s
  `assertServerBinaryFresh` — if it flags staleness due to only test-file
  changes (not real server-code changes), you may need to rebuild the
  release server binary once:
  `CARGO_TARGET_DIR=D:/dev/rust/.cargo-target cargo build --release -p shamir-server`
  (use forward slashes — a known Git-Bash backslash-escaping bug creates a
  stray `devrust.cargo-target` directory in the repo root otherwise; if it
  appears anyway, delete it before finishing, never commit it).
- `cargo fmt -p shamir-client -- --check` clean.
- `cargo clippy --workspace --all-targets -- -D warnings` clean (full
  workspace — every prior phase this session broke some OTHER crate by
  growing a struct/enum without updating all call sites elsewhere).
- Report the literal command output for each of the above, not just a
  summary claim.

## Out of scope (do not touch)

- Any production/engine code. If something doesn't work as the ADR
  describes, STOP and report the discrepancy precisely (file:line, expected
  vs actual, minimal repro) rather than patching around it — unlike
  Epic03/E's `when` bug, this phase's canonical scenario is NOT expected to
  hit any known blocker, so a real failure here is noteworthy and should be
  reported clearly, not silently worked around with a synthetic-guard
  substitute like `when`'s e2e file had to use.
- Benchmarks (Phase F, #657), docs (Phase G, #658), the deferred while-loop
  design (#659).
