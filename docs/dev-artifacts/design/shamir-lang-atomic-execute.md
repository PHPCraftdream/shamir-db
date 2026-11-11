# shamir-lang: atomic `db_execute` (PROC-ATOMIC)

**Status:** design (decisions made); implementation in two phases, see "Implementation plan".
Supersedes the "no transactional reference mode / no engine DTO change" statements in
`shamir-lang-call-refs-addendum.md` (section 1 decision 6, sections 8 and 9) for the single case below.

## Problem

Before this change a `shamir` procedure has no atomicity at all:

* `db_get` / `db_insert` / `db_query` each autocommit (`FacadeDbGateway`, `invoke_function_in_db` doc).
* The typed batch bakes `transactional = false` into the static `BatchRequest` template
  (`back/batch/template.rs`), so `db_execute` is a non-transactional batch.
* A failed scalar-reference guard becomes a `skipped` result; the guest raises kind 2 only after the engine
  already committed the other operations of the batch (partial writes).

The engine already supports `BatchRequest.transactional = true` (single repository, MVCC, `Call` refused).
Setting that bit alone is not enough, for two reasons found in review:

1. In a transactional batch a false `when` still yields a skip, not an error, so the transaction would commit
   and the guest would raise afterwards.
2. When a transactional batch fails during execution the engine returns `Ok` with empty `results` and
   `transaction.status = "aborted"` (`batch_execute.rs`, error arm). Today's guest helpers read an absent alias
   as `null`, so an aborted transaction would silently look like "no rows". The guest must therefore inspect
   `transaction.status` after every transactional `db_execute`.

## Scope

One `db_execute` of a plan rooted in `batch_tx()` runs as one MVCC transaction on one repository. If anything
fails, nothing is written. Atomicity never extends across `db_execute` calls, `db_get`/`db_insert`/`db_query`
(they still autocommit) or `call_checked`. A procedure-wide transaction is deferred
(`release-1/06-deferred-and-non-goals.md`).

## 1. Language surface: `batch_tx() -> Batch`

* A new root constructor next to `batch()`. It lowers to a second static `BatchRequest` record cell that differs
  only in `transactional = true` (field 1, `BOOL`, payload 1).
* Every builder intrinsic copies the plan cells, so the transactional bit travels with the plan value
  (`batch_id`, `batch_read`, `batch_insert`, `batch_update`, `batch_delete`, `request`). There is no `TxBatch` type
  and no doubled signatures: the bit is a property of the value, resolved statically at the root.
* References keep their "known plan root/history" rule, so a reference sink knows statically whether its root is
  `batch_tx`.
* `Batch`, `Request`, `Response` are not callable signature types, so a transaction cannot span `call_checked`;
  the compiler never emits a batch `Call` and the gateway refuses one.
* The language has no repository qualifier (every operation uses `main`), so a cross-repository transaction cannot
  be written in it.
* Lazy emission: the transactional cell, descriptors, helper and strings are baked only when `batch_tx` is
  reachable. A module that does not use it is byte-identical to today's output.
* Rejected: `batch_transactional(b)` / `request_tx(b)` / `db_execute_tx` (the switch happens after reference ops were
  already lowered without the abort flag); `batch(true)` (overloads an existing builtin's arity, unreadable);
  emitting the flag on every reference op (changes existing module bytes, refused outside a transaction).

## 2. Atomic failure of reference guards: an engine opt-in per entry

No existing engine mechanism can express "this skip is an error" (no assert op, `when` always evaluates to a
boolean, an unresolved read-filter marker just matches nothing).

* **Wire.** `QueryEntry.abort_on_skip: bool`, `#[serde(default, skip_serializing_if = "std::ops::Not::not")]`,
  declared after `when`. Absent when false, so all existing bytes are unchanged; query version stays v4. An older
  server rejects the unknown key (the flattened ops use `deny_unknown_fields`), i.e. it fails closed.
* **Engine semantics.** When an entry with the flag would be skipped (its own `when` is false, or a data-flow
  dependency was skipped) the engine returns `BatchError::query_coded(alias, "skip_aborted",
  "entry skipped under abort_on_skip")` instead of a skipped result. In a single-call transactional batch that
  goes through the existing error path (locks released, transaction rolled back, response `Ok` with empty results
  and `transaction.status = "aborted"`). The abort `reason` for this code is exactly the token `"skip_aborted"`;
  every other reason keeps today's `format!("{:?}")` text.
* **Scope.** The flag is accepted only when the root has `transactional: true`, at any nesting depth (nested
  `Batch`/`ForEach` bodies run in the outer transaction). A non-transactional root anywhere in the tree is refused
  before any operation runs: code `validation`, message `abort_on_skip requires a single-call transactional
  batch`. Interactive `TxExecute` is refused with the same message (an error there would leave earlier staged
  writes inside the still-open transaction, so "abort" would be untrue). Refusing now allows relaxing later;
  tightening later would be incompatible.
* **Native builder.** `Batch::abort_on_skip(&mut self, handle: &Handle) -> &mut Self`; an unknown handle records a
  deferred `BuildError` like `when`. The builder does no transactional validation; the server is authoritative.
* **TS builder.** `add(alias, op, { when, abortOnSkip: true })` emits `abort_on_skip: true` only when true.
* **Guest emission.** In a plan rooted in `batch_tx`, every guarded read or insert uses the descriptor
  `[..., "when", "abort_on_skip"]` with a static `true` cell; unguarded operations are unchanged. The bytes must
  equal `Batch::new().transactional()` + `.when(h, g)` + `.abort_on_skip(&h)` + `to_msgpack()`.
* Rejected: a request-level flag (needs inheritance rules for nested bodies and would turn intentional conditional
  `when` operations of the same transaction into aborts); a new `BatchOp::Assert` (touches planner, access checks,
  op kinds and the TS client, plus ordering edges); "every skip in a transaction aborts" (breaks OQL-03 for
  existing transactional clients); a top-level `Err` (the gateway turns it into a host trap and the guest loses
  its stable kind-2 error); guest-side compensating deletes (not atomic, racy, need Delete rights); host imports for
  interactive transactions (import ceiling, a transaction living across guest code, traps and cancellation).
* **Security.** The abort reveals exactly the one bit `skipped: true` already reveals. That bit is computed only
  over results of operations of the same batch, and the whole tree is authorized before `begin_tx` regardless of
  `when` (OQL-03 decision 3).

## 3. What the guest does after a transactional `db_execute`

Only when the request cell's `transactional` field is true (read at runtime), a new helper runs **before** the
existing skip check:

| Response | Guest result |
|---|---|
| `transaction.status == "committed"` (including `error: "wal_durability"`) | continue |
| `aborted`, `reason == "skip_aborted"` | kind 2, `batch reference did not resolve to the expected scalar` |
| `aborted`, `reason == "tx_conflict"` | kind 2, `transactional batch aborted by a conflict` |
| `aborted`, any other reason | kind 2, `transactional batch aborted` |
| `status == "indeterminate"` | kind 2, `transactional batch outcome is unknown` |
| `transaction` missing or not a map; `status` or `reason` not a string; unknown status | kind 3, existing decode message |

The guest never copies `reason` into its message (under Definer, reasons are `Debug` strings that can contain
values the invoker may not see). Errors raised by the engine before any operation runs (cross-repository refusal,
"transactional batch has no data ops", `ExecutionTimedOut` and other limits) surface as unchanged
`db_execute: ...` host traps and never enter `commit_tx`. Retry is the caller's job: the language has no `catch`
and automatic retry would re-run non-idempotent guest logic; the distinct conflict message lets callers decide.

## 4. Isolation

Snapshot only; the `isolation` field is left out so the bytes match the native default. Reads of the batch,
references included, see one snapshot. Concurrent writers are last-writer-wins, so read-then-write is not
serializable; this is documented, not prevented. Rejected: exposing an isolation argument (more surface, SSI
aborts the guest cannot handle) and defaulting to Serializable (diverges from native, causes aborts that cannot be
retried inside the guest). A possible later form is `batch_tx_serializable()`.

## 5. Threat model

Attacker-controlled inputs: (1) procedure source from a possibly low-privilege author; (2) invoker-chosen
parameters; (3) arbitrary `BatchRequest` bytes from non-language guests (Rust SDK or imported Wasm) through
`FacadeDbGateway::execute`; (4) network clients (TS, Rust, napi); (5) database contents written by other
principals; (6) timing, concurrency and cancellation (wall-clock timeout, conflicting writers).

Invariants that must hold:

* **Compiler output.** `abort_on_skip` appears only on guarded reference operations of plans rooted in `batch_tx`.
  Request bytes equal the native-builder oracle. Parameters never change request structure. The `transactional`
  bit is static per root.
* **Engine.** The flag works only in a single-call transactional root and is refused everywhere else before any
  operation runs. The whole tree, including entries that will be skipped, is authorized before `begin_tx` under
  the effective actor (Definer: owner; Invoker: caller). `Call` stays refused inside transactions and through the
  gateway. Decode caps are unchanged.
* **Atomicity.** Every failure after `begin_tx` and before `commit_tx` rolls back and releases pessimistic locks
  (existing error path). Any outcome other than `committed` makes the guest raise before a projection can see
  empty results.
* **Confidentiality.** No new oracle (the same bit as `skipped: true`). Guest error messages are constants.
* **Resources.** An abort costs no more than commit work. `BatchLimits`, the 30 s budget and fuel/pool limits are
  unchanged; no transaction outlives one host call.
* **Wire skew.** An older server fails closed on the unknown key.

## Decisions on the open points of the review

* A wall-clock timeout of the guest call can drop the in-flight `gateway.execute` future during `commit_tx`, so a
  procedure may report a timeout although the transaction committed. This already holds for every guest write
  and is not worsened in kind by this change. It is documented in the functions guide (a timeout is not proof of
  rollback) and recorded in `release-1/06-deferred-and-non-goals.md`; running the transactional engine call as a
  joined spawned task is deferred because it conflicts with the stage-2 "no detached work" rule.
* The transactional reference failure keeps the same stable message as the non-transactional one.
* An empty `batch_tx()` request stays an engine error / host trap; no compile-time rejection in this change.
* "Committed with `wal_durability` error" counts as success for the guest (status first), as in the protocol spec.
* Only `skip_aborted` and `tx_conflict` are relied on as exact reason tokens; tests pin both.

## Implementation plan

Two bounded phases in one change. Briefs: `prompts/crates/25-proc-atomic-engine.md` (phase 1: wire, engine,
builders, spec; the language is untouched) and `prompts/crates/25b-proc-atomic-lang.md` (phase 2: language, guest
checks, host oracle, end-to-end). Evidence and file list are in the briefs.
