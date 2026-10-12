# Brief — VIS-4c visibility metrics, structured logs and changes_since visible_through

You are a sub-agent working inside an isolated git worktree (your cwd) of the S.H.A.M.I.R. Rust database repository. Read `CLAUDE.md` in the cwd first: code ideology (lock-free / async / scc maps, no guard across `.await`), TDD, test layout (`tests/` dir per module, `mod.rs` = manifest only), imports at the top of the file, one primary export per file, query builder only (never hand-assembled JSON).

## Hard rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- Do NOT run `cargo`, `rustc`, `nextest`, or any build or test command: the owner's machine must stay idle and ALL compilation and testing happens later in GitHub CI. Therefore be extra careful that the code compiles: read the real definitions (Read/Grep) of every type, trait and function you call instead of guessing signatures; keep the change small and local; prefer the narrowest layer that owns the bug.
- Stay strictly inside the FILE SCOPE below. Other agents work concurrently on other files of this repository; do not touch any file outside your scope even if it looks related (if you believe an out-of-scope change is required, say so in your final message instead of doing it).
- TDD: add the regression tests described in the task in the same change. They must FAIL on the current code for the stated reason, be deterministic (no wall clock, no sleeps; use barriers, notifies, hooks or paused tokio time), assert observable behaviour (not just `is_ok()`), and live in the module's `tests/` directory wired through its `mod.rs` manifest.
- Write rustfmt-clean code (4 spaces, trailing commas, short lines); the orchestrator formats touched files.
- Layout limits (CLAUDE.md, ratchet): at most 7 direct entries per folder and at most 1000 lines per source file. Existing violations are grandfathered but must not grow: put new logic into NEW files / subfolders instead of extending an over-limit file or folder, and never push a new file past 1000 lines or a folder past 7 entries.
- Do not add dependencies, do not bump versions, do not rename or reformat unrelated code, do not touch unrelated comments.
- Finish with a final message listing: files changed, what each test proves and why it fails today, any design deviation from the task text, and anything you could not verify.


## Scope discipline

If a file outside the FILE SCOPE is strictly required to make the change compile or to keep an existing test meaningful (a caller of a changed signature, a fixture, a manifest line), edit it minimally and LIST it in your final message; do not stop to ask. Ask (and stop) only for genuine design decisions or when the scope would grow into another task area.
## FILE SCOPE (only these paths may be edited)

- NEW subfolder `crates/shamir-tx/src/visibility_metrics/` (lock-free counters, a fixed-bucket histogram of atomics, a waiters gauge reusing the completion tracker's registration, a plain-data snapshot) + the smallest re-export in `lib.rs` and the smallest accessor on `RepoTxGate`/`TxMetrics` needed to reach it
- recording call sites with minimal edits: `crates/shamir-engine/src/tx/ack/**`, `table/write_receipt/**`, `crates/shamir-tx/src/mvcc_store/write_receipt/**`, `query/batch/receipts/commit.rs`, the AsOf read-wait path and the `WaitForVisibility` handler (find them; reuse what REPO-DIAGNOSTICS added)
- NEW file(s) under `crates/shamir-server/src/observability/` (submodule dir of `observability.rs`) exporting the snapshot through the existing poller/registry, with the smallest edit in `observability.rs`
- structured logs (`tracing`/`log` as the neighbouring code does) at the existing wait-expiry sites
- `changes_since`: `crates/shamir-db/src/shamir_db/execute/admin_retention.rs` (smallest edit) + a NEW file in a new subfolder for the visibility sampling helper; additive `visible_through` on the response type in `shamir-query-types` (serde default, omitted when absent) and the builder only if a result helper exists there; wire tests
- tests: NEW files in NEW `tests/` subfolders next to the code
- docs: extend the existing sections of `docs/guide-docs/guide/07-operations.md` (metric names, labels, alert hints) and `docs/guide-docs/client-server-protocol-spec/TRANSACTION_OUTCOMES.md` (`changes_since` semantics); nothing else
- NOT in scope: Rust/TS client adoption of `changes_since.visible_through` (VIS-5b), per-alias receipt types, commit pipeline internals beyond recording points, replication/seed code

## Task

VIS-4c (task #1337): visibility metrics, structured logs and `changes_since visible_through`, step 4c of 5 (VIS-5b adopts the new response field in the clients). VIS-1..VIS-4b and VIS-5a are MERGED on master, your base: contract types, `[transactions]` config, `RepoInstance::observe_visibility`, `WaitForVisibility`, transactional bounded acknowledgement (`crates/shamir-engine/src/tx/ack/`), direct-write receipts (`table/write_receipt/`), implicit-batch per-alias receipts (`query/batch/receipts/`), `visibility_ack_mode = "bounded"` accepted by the server, Rust/TS client adoption. REPO-DIAGNOSTICS (gate diagnostics + Prometheus metrics for stalls/orphans) is merged too: reuse its plumbing and conventions, do not duplicate. Read all of that first; verify every claim against the code (line numbers drift). Sibling work: SEED-5 runs concurrently and owns `crates/shamir-server/tests/seed_recovery_e2e/` and seed/replication docs: stay out. Do NOT delegate to child agents and do not wait on schedules: finish the work yourself in this worktree. No git mutation, NO builds or tests of any kind (not even `cargo check`: only CI compiles and tests; earlier agents burned their whole time budget running cargo locally), imports at the top, one primary export per file, `mod.rs` re-exports only, tests under `tests/` dirs, builder-only queries (wire-format docs excepted).

COMPILE-READINESS (mandatory; the main cost driver of this program is one compile error per CI cycle): before finishing re-read every new/changed file as a compiler would: every `use` resolves, visibility across CRATE boundaries (shamir-server cannot touch engine/tx-private items: add a documented public seam in a NEW file), every struct literal has all fields (adding a field to a public struct breaks every literal and every exhaustive match: grep them ALL including tests, benches and other crates), real signatures of every call (grep, do not recall), `Send` across `.await`. Repository lints that fail CI (`-D warnings`): `clippy::absolute_paths` (never write a path like `crate::a::b::c::f()` inline: import it with `use`), `clippy::type_complexity`, `dead_code` on Windows for `cfg(test)`/`cfg(unix)`-only helpers, `unused_must_use`, `clippy::disallowed_methods` (`scc::*::len()` is banned: keep an atomic mirror). `shamir-query-types`, `shamir-db`, `shamir-engine` and `shamir-server` unit tests have NO `serde_json` dependency: use `rmp_serde` and serde-derived mirror structs for wire fixtures (never add a dependency to fix a test; Cargo.lock is checked with `--locked`). Ordinary behaviour must stay exactly as before unless this task says otherwise: existing tests asserting current responses of `changes_since` or the metrics output must keep passing (additive fields only). Async futures in the recursive batch runner are large in debug builds and several tests run on a 2 MiB stack: do NOT inline new large futures into `QueryRunner::run_with_wait`, `receipts::execute_plan` or the commit path (box them or put the work in a small non-async helper); metrics code on those paths must be plain atomic increments. Hot-path rule: the Strict/Visible acknowledgement fast path may add at most ONE relaxed atomic increment and NO allocation, lock, `Instant::now()` pair or histogram observation (time and histogram only when the wait actually blocked or timed out).

LAYOUT RULE (CLAUDE.md, ratchet): at most 7 direct entries per folder, at most 1000 lines per source file; `crates/shamir-tx/src/`, `crates/shamir-server/src/`, `shamir_db/execute/` and `observability.rs`-adjacent code are over limit/grandfathered: put ALL new code into NEW files in NEW subfolders (for example `crates/shamir-tx/src/visibility_metrics/`, `crates/shamir-server/src/observability/` as a submodule directory of `observability.rs`) and keep edits to existing files to the smallest call-site/re-export changes.

GOAL: operators can see how long acknowledgements wait, how often commits answer Pending, how often reads time out, how many acknowledgements are currently waiting, with structured logs at expiry; and `changes_since` reports the sampled settled position so a caller can tell a pending write from an empty journal. Read the appended reviewer design (section 6 'Metrics/docs' and section 7 `changes_since`) as BINDING.

REQUIRED:
1. Metrics (lock-free atomics in `shamir-tx`, exported by the server poller like the existing `TxMetrics`/gate diagnostics; follow the existing naming such as `shamir_tx_*`):
   - acknowledgement duration histogram labelled by `mode` (strict|bounded) and `result` (visible|pending_timeout|pending_reopen|error); fixed bucket bounds chosen to cover sub-millisecond to the configured maximum wait; observed ONLY when the acknowledgement actually waited (fast path = counter only);
   - pending counters by `reason` (timeout|reopen_required) and `kind` (explicit_tx|implicit_batch|direct_write);
   - read-wait metrics: AsOf read-wait timeout counter and reconciliation (`WaitForVisibility`) wait counter + duration/result;
   - waiters gauge (current number of acknowledgement waiters), reusing the tracker's existing registration rather than a second counter updated on the hot path;
   - commit counters keep counting commits (do not count only visible acknowledgements).
   NEVER use version, transaction id, repository or table names as metric labels. Zero-touch registration so every series appears at the first scrape (follow `observability.rs`).
2. Structured logs at wait expiry (Pending timeout, reopen required, AsOf read-wait timeout, WaitForVisibility timeout): repository, target position, sampled W (`visible_through`), first unsettled position (`blocking_version`), elapsed wait; rate-limit like the existing REPO-DIAGNOSTICS warning so a stall cannot flood the log.
3. `changes_since`: add the sampled `visible_through` (settled prefix W at response time) to the response; events returned stay bounded by the settled prefix as today; document that an empty event list is NOT proof a pending write aborted or vanished, and that journal gaps/retention still require resynchronisation. Additive, serde default, omitted when unavailable; old payloads still decode.
4. Docs: metric names/labels/types and suggested alerts; `changes_since` semantics; remove any remaining claim that materialization or a visible acknowledgement implies push delivery.
5. Tests (deterministic, paused clock where time matters, no sleeps): histogram bucket edges and snapshot arithmetic (unit); a strict ack that does not block records only the counter (no observation); a bounded ack that times out records one Pending(timeout) + one histogram observation with the right labels; reopen-required records the reopen reason; direct-write and implicit-batch kinds are distinguished; waiters gauge rises while an acknowledgement is parked and returns to zero after settlement and after cancellation; AsOf read timeout and WaitForVisibility counters; the server exposition contains every new series name after a scrape (use the existing observability test helpers); `changes_since` returns `visible_through` equal to W, bounded events, and a wire round trip with and without the field.
Hot path: this edits the acknowledgement tail of commit/direct-write/implicit paths. State which owed A/B cell covers it (the owner dispatches benches; do not run them).

REVIEWER ANSWER (bounded visibility design, BINDING):

## Verdict

**Approve a separate visibility outcome, not a new materialization variant and not a generic error/success code. Keep strict W-based acknowledgement as the default; offer an explicit bounded mode that returns committed metadata with `visibility: pending`.**

Approval is conditional on **end-to-end propagation through implicit/non-transactional writes, protocol version gating, and a reconciliation operation**. Merely adding a timeout around the existing wait is insufficient: implicit commits currently discard successful outcomes, post-wait backpressure adds another delay, and async-index tails can retain writer guards.

**Evidence classification:** “OBSERVED” below means inspected in checkout source, not reproduced at runtime. Recommendations and consequences are explicitly design decisions or inferences. No files were changed; no builds or tests were run.

---

## 1. Outcome and wire contract

### OBSERVED

- The current enum is **`MaterializationState::Complete | Deferred`**, not `Materialized | Deferred`: `crates/shamir-engine/src/tx/tx_outcome.rs:35`.
- `TxOutcome` independently carries commit/snapshot versions, materialization, and an optional background-tail handle: `crates/shamir-engine/src/tx/tx_outcome.rs:54`.
- Reopen during acknowledgement currently changes materialization to `Deferred`, conflating this transaction’s projections with repository-prefix visibility: `crates/shamir-engine/src/tx/commit.rs:597`.
- Wire metadata uses a string status, optional published-commit error, and `materialized: bool`; `is_committed()` checks only status: `crates/shamir-query-types/src/batch/transaction_info.rs:7`, `crates/shamir-query-types/src/batch/transaction_info.rs:98`.

### Decision: separate orthogonal facts

Introduce these shared types in `shamir-query-types`, re-exporting them where appropriate:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum VisibilityWait {
    Strict,
    Bounded { timeout_ms: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisibilityPendingReason {
    Timeout,
    ReopenRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum VisibilityState {
    Visible,
    Pending {
        visible_through: u64,
        blocking_version: u64,
        reason: VisibilityPendingReason,
    },
}
```

Changes:

- Add `pub visibility: VisibilityState` to `TxOutcome`.
- Add `visibility: VisibilityState` to `TxError::WalDurability`.
- Add `pub visibility: Option<VisibilityState>` to `TransactionInfo`: required for committed outcomes in the new protocol, absent for aborted outcomes.
- Keep `status: "committed"` and `reason: None` for pending acknowledgements.
- Preserve `error: "wal_durability"` independently when applicable. Timeout must not overwrite a durability warning.
- **Do not change materialization when the acknowledgement wait expires or observes reopen.**
- Add `is_visible()` separately; retain `is_committed()` as a commit-decision predicate.

Wire representation, in named-field MessagePack, illustrated here as JSON:

```json
{
  "tx_id": 7,
  "status": "committed",
  "snapshot_version": 40,
  "commit_version": 43,
  "materialized": true,
  "visibility": {
    "state": "pending",
    "visible_through": 41,
    "blocking_version": 42,
    "reason": "timeout"
  }
}
```

`blocking_version` means **the first version outside the sampled settled prefix**, calculated as `visible_through + 1`. It is not a claim that the version is wedged, that its owner is known, or that it is the only obstacle. Diagnostics may explain it separately.

`Visible` establishes only the contiguous settled-prefix guarantee. It does **not** upgrade materialization, WAL durability, or async-index completion.

### Old-client behavior and mandatory gating

**OBSERVED:**

- Old Rust `Client::execute` returns `Ok(BatchResponse)` without interpreting transaction status or error: `crates/shamir-client/src/client.rs:1065`, `crates/shamir-client/src/client.rs:1081`.
- Old TS `db.tx()` rejects only aborted status or a populated `error`; otherwise it returns the callback result: `crates/shamir-client-ts/src/core/db.ts:165`.
- TS metadata currently has no visibility field: `crates/shamir-client-ts/src/core/types/batch.ts:286`.

**INFERRED:** An additive visibility field alone would silently become ordinary success for those clients. Adding `error: "visibility_pending"` would make the TS helper throw, encouraging failure/retry interpretation, while Rust still returns `Ok`. Neither is adequate.

**Decision:** introduce a new query/protocol version and reject old-version execution/interactive transaction requests **before writes**. Require explicit versioning on `TxCommit` as well; it currently has none: `crates/shamir-query-types/src/wire/db_message.rs:119`. Do not deserialize a missing committed visibility field as `Visible` in the new contract. Clients must reject malformed or unsupported outcome metadata.

No representation can prevent an arbitrary application from deliberately ignoring metadata; the supported SDK APIs must make pending a first-class result.

---

## 2. Configuration and handle lifecycle

### Decision

Use a new top-level server **`[transactions]`** section:

| Knob | Default | Meaning |
|---|---:|---|
| `visibility_ack_mode` | `"strict"` | Default acknowledgement policy |
| `visibility_ack_timeout_ms` | `5000` | Budget when bounded mode is selected |
| `max_visibility_wait_ms` | `30000` | Maximum requested bounded wait |
| `allow_strict_visibility_wait` | `true` | Whether network callers may request strict waits |
| `read_visibility_timeout_ms` | `5000` | Default allocated-version read wait |

Validate millisecond values; zero means an immediate predicate check, not infinity. Define strict mode explicitly rather than using a magic zero timeout.

**Why strict default:** preserve the requested default contract: normal successful acknowledgement still establishes W ≥ commit version. Bounded mode is an operator/caller choice, not a silent weakening. Operational documentation must recommend bounded mode for services prioritizing connection-resource liveness. Strict mode intentionally retains indefinite acknowledgement risk.

This is not `security.query_limits`: that section governs pre-commit execution resources and client-shrinkable caps, whereas visibility is a post-commit consistency contract. Existing placement and execution policy are visible at `crates/shamir-server/src/config.rs:233` and `crates/shamir-query-types/src/batch/exec_policy.rs:5`.

Embedded configuration should expose the same `VisibilityWait` through repository/DB construction policy and `commit_tx_with_options`. `RepoConfig` is a concrete existing configuration entry point: `crates/shamir-engine/src/repo/repo_config.rs:5`. Do not assume a `DbConfig` field exists merely from the question’s suggested name.

Per-request:

- `BatchRequest.visibility_wait: Option<VisibilityWait>`.
- `TxCommit.visibility_wait: Option<VisibilityWait>`.
- Effective policy is resolved before execution; an excessive or forbidden override is rejected, not silently clamped.
- `TxExecute` does not commit and must not produce commit visibility pending. Reject commit-only overrides there rather than pretending they apply.

### Deadline interaction

**OBSERVED:** the transactional execution deadline’s last check precedes commit; commit deliberately is not raced under the execution timeout: `crates/shamir-engine/src/query/batch/batch_execute.rs:814`. Interactive execution passes a remaining lifetime cap: `crates/shamir-server/src/db_handler/tx_handlers.rs:174`.

**Decision:**

- `ExecutionDeadline` remains a pre-commit execution/abort budget.
- Visibility timeout starts afresh after the inner commit returns a known outcome.
- The server’s execution-time cap does not become a post-commit abort deadline.
- Never wrap the whole inner commit in `timeout`.
- Client round-trip timeout remains a transport-level **unknown acknowledgement**, not committed/pending proof. Existing Rust request timeouts are explicitly round-trip deadlines: `crates/shamir-client/src/client.rs:86`.

`TxCommit` consumes the interactive handle regardless of visible/pending/error outcome. It already removes the handle before committing: `crates/shamir-server/src/db_handler/tx_handlers.rs:259`. Pending must not retain or re-register it. Release its snapshot ownership when the known commit has finished; do not pin an obsolete transaction snapshot merely to observe W.

---

## 3. Exact acknowledgement algorithm

### OBSERVED: correct seam, with two qualifications

`commit_tx_inner` returns before the wrapper waits: `crates/shamir-engine/src/tx/commit.rs:588`. The synchronous path releases cell reservations, Serializable serialization, and pessimistic locks before finalization/return: `crates/shamir-engine/src/tx/commit.rs:1265`, `crates/shamir-engine/src/tx/commit.rs:1282`.

However, **“all locks are released” is not universally true for repository work**:

- Async-index mode drops the inline commit guard but transfers `uwl_guards` and `drain_guards` into a spawned tail: `crates/shamir-engine/src/tx/commit.rs:1004`, `crates/shamir-engine/src/tx/commit.rs:1014`.
- Async-index durability-error handling awaits that tail **inside** the inner driver: `crates/shamir-engine/src/tx/commit.rs:1052`. The proposed bound cannot cover that await.
- Success and `WalDurability` currently run backpressure after visibility waiting: `crates/shamir-engine/src/tx/commit.rs:615`, `crates/shamir-engine/src/tx/commit.rs:631`. Backpressure has its own five-second ceiling: `crates/shamir-engine/src/tx/commit.rs:758`.

### Decision: implementation sequence

1. Resolve policy and retain the repository gate before entering the commit pipeline. Avoid a fallible gate lookup after a known committed result that could erase that result.
2. Await the inner commit without introducing a timeout.
3. For pre-commit abort or indeterminate WAL outcome, preserve existing outcome classification. Do not manufacture committed/pending metadata.
4. For `Ok(TxOutcome)` or known published `WalDurability`, start a monotonic `tokio::time::Instant` budget.
5. No-allocation/read-only outcomes already at their snapshot position return visible without waiting; preserve the existing distinction at `crates/shamir-engine/src/tx/commit.rs:592`.
6. Check W first:
   - W ≥ target → `Visible`.
   - Otherwise reopen → `Pending { reason: ReopenRequired, ... }`.
   - Otherwise wait according to strict/bounded policy.
7. On bounded expiry, re-sample W; if now visible, visibility wins. Otherwise return pending with a sampled floor and first unsettled version.
8. Store that visibility in the known outcome. Never abort, roll back, replay, or alter completion state.
9. Finish acknowledgement without another independent five-second wait.

For step 9, make existing soft backpressure deadline-aware: **pending returns skip the post-ack park; visible returns use only the remaining bounded budget**. Strict mode can retain the existing backpressure ceiling. This preserves the current soft-brake policy while avoiding a misleading “five-second acknowledgement” that takes ten seconds. Document that this remains soft backpressure, not a hard memory admission guarantee.

Implement bounded observation as an engine-side adapter around existing `await_visible`, with expiry and final predicate recheck. The existing primitive already has waiter RAII and notification registration/recheck discipline: `crates/shamir-tx/src/repo_tx_gate.rs:1078`. Do not rewrite its concurrency algorithm unnecessarily.

### Cancellation and resource release

**OBSERVED:** `await_visible` mutates no version state and uses an RAII waiter registration: `crates/shamir-tx/src/repo_tx_gate.rs:1061`.

Thus cancellation of **only the acknowledgement observer** drops its timer/registration. It must not abort async materialization, settle the blocking version, or remove recovery obligations. Keep the background handle unchanged; dropping it must not be repurposed as cancellation.

The pending response follows normal serialization/resource ownership:

- Upfront byte reservation spans execution and becomes the actual serialized response reservation: `crates/shamir-server/src/db_handler/handler.rs:562`, `crates/shamir-server/src/db_handler/handler.rs:613`.
- The reservation travels with the writer message until socket write completion: `crates/shamir-server/src/db_handler/handler.rs:621`.
- The request permit remains owned until dispatch and queue send finish: `crates/shamir-server/src/connection/request_loop.rs:323`, `crates/shamir-server/src/connection/request_loop.rs:399`.

**INFERRED:** bounded acknowledgement removes this particular indefinite wait, not socket/channel backpressure, queue admission, inner-commit stalls, or disconnect ambiguity. Do not promise a bounded end-to-end network response.

### Reconciliation operation

Add a read-only application operation and embedded equivalent:

```rust
WaitForVisibility {
    query_version: u32,
    db: String,
    repo: String,
    version: u64,
    visibility_wait: VisibilityWait,
}
```

Return `VisibilityObserved { version, visibility }`. A zero bounded timeout is the status-read form.

Requirements:

- Authenticate and authorize repository read access.
- Reject versions above the allocation high-water mark.
- W ≥ version takes precedence over reopen.
- Return pending on expiry/reopen without any transaction mutation.
- Apply the configured network wait cap and strict permission.
- No interactive handle, snapshot registration, or background polling task survives the response.
- State explicitly: this operation proves **settlement through a position**, not whether an unknown transaction committed. An aborted allocated version can also lie beneath W.
- Treat positions as repository-scoped. Do not reconcile against a deleted/recreated repository; bind receipts to repository identity or reject identity mismatches.

---

## 4. Non-transactional writes and batches

### OBSERVED

- Non-tx `await_non_tx_ack` logs reopen and returns unit: `crates/shamir-tx/src/repo_tx_gate.rs:1113`, `crates/shamir-tx/src/mvcc_store/mod.rs:788`.
- CRUD releases its writer guards before waiting, then returns the ordinary result: `crates/shamir-engine/src/table/table_manager_crud.rs:258`.
- Similar wait sites exist for insert-many, delete, and set: `crates/shamir-engine/src/table/table_manager_crud.rs:443`, `crates/shamir-engine/src/table/table_manager_crud.rs:534`, `crates/shamir-engine/src/table/table_manager_crud.rs:665`.
- Network “non-transactional” insert uses an implicit transaction: `crates/shamir-engine/src/query/batch/query_runner.rs:1369`. Its commit helper discards `Ok(_outcome)`: `crates/shamir-engine/src/repo/repo_instance.rs:1338`.

### Decision: typed receipts, not write errors

```rust
#[must_use]
pub struct WriteOutcome<T> {
    pub value: T,
    pub commit_version: Option<u64>,
    pub visibility: VisibilityState,
}
```

`None` represents no minted version, including non-MVCC/no-op cases; do not expose zero as a real reconciliation position.

Change CRUD receipt methods to return:

- insert: `DbResult<WriteOutcome<RecordId>>`
- insert-many: `DbResult<WriteOutcome<Vec<RecordId>>>`
- delete: `DbResult<WriteOutcome<bool>>`
- set: `DbResult<WriteOutcome<()>>`

`await_non_tx_ack` returns `VisibilityState` and accepts the resolved policy. Extend analogous direct MVCC write APIs so bounded results cannot be discarded.

To minimize embedded disruption, retain old value-only convenience methods **only as explicit strict adapters**. They must not silently inherit bounded mode. On reopen-before-visible, preserve the committed receipt in a typed exceptional outcome rather than returning plain success or an ordinary retryable `DbError`. Bounded APIs return receipts normally.

Direct CRUD is documented as best-effort/no-WAL and unsafe to cancel mid-sequence: `crates/shamir-engine/src/table/table_manager_crud.rs:155`. A visibility receipt must not imply transactional atomicity or stronger crash durability.

### Batch propagation

- Explicit transactional batches retain results and attach full committed/pending `TransactionInfo`, as committed durability errors already retain results: `crates/shamir-engine/src/query/batch/batch_execute.rs:880`.
- Change `commit_implicit_batch_tx` to return committed metadata rather than unit; preserve known published durability errors as metadata too.
- Attach implicit commit metadata to each write alias’s `QueryResult`, even when row data is suppressed.
- Nested and multi-repository non-tx batches need per-alias receipts; a single top-level transaction cannot describe them.
- Do not reclassify pending as a query error.
- Dependent reads must not infer read-your-write from DAG completion. Return an explicit visibility dependency failure before a dependent read requiring the pending position, or require the caller to reconcile before issuing that read. Independent operations may continue.

---

## 5. AsOf and read-side waits

**Recommend bounded read waits with a typed timeout error; allow explicit strict mode.**

Unlike writes, a failed read wait has no committed write outcome to preserve:

```rust
DbError::AsOfVisibilityTimeout {
    requested: u64,
    visible_through: u64,
    blocking_version: u64,
}
```

Map it to `as_of_visibility_timeout`, retaining structured fields through the wire error path. Preserve `AsOfFutureVersion` and `AsOfReopenRequired`.

**OBSERVED:** AsOf validates allocation before waiting and currently has no give-up timeout: `crates/shamir-engine/src/table/read_temporal.rs:85`.

Use the smaller of the selected read-wait budget and remaining execution/call-time budget. Never return data from a substituted older version on timeout. Ordinary Latest snapshots should remain W-based without waiting for the allocation high-water mark.

For interactive `TxExecute`, a visibility-specific read timeout should leave the handle open unless the enclosing execution deadline itself expires; preserve the existing deadline/limit cleanup distinction at `crates/shamir-server/src/db_handler/tx_handlers.rs:212`.

---

## 6. Clients, metrics, documentation, and required CI tests

### Client changes

- **Rust:** expose typed visibility, receipts, options, and `wait_for_visibility`; retain transport errors separately. High-level helpers return a discriminated committed result rather than mapping pending to `ClientError`.
- **TS:** change `db.tx()` from `Promise<T>` to a receipt-bearing discriminated result, including the callback value and transaction metadata. Pending resolves as committed/pending, never an abort exception. Add options and `waitForVisibility`.
- TS versions must use lossless `bigint` handling for u64 reconciliation positions. Existing transaction versions are typed as `number`: `crates/shamir-client-ts/src/core/types/batch.ts:292`.
- SDK convenience methods that currently unwrap row results must not discard implicit commit receipts.
- Neither SDK automatically resends writes on pending, durability warning, or transport timeout.

### Metrics/docs

Add:

- visibility acknowledgement duration histogram, by strict/bounded and result;
- pending counters by timeout/reopen and explicit/implicit/direct write;
- read-wait timeout and reconciliation-wait metrics;
- finite acknowledgement waiters gauge, reusing tracker registration;
- structured logs with repository, target, sampled W, first unsettled position, and elapsed wait.

Avoid version/transaction IDs as metric labels. Keep commit counters counting commits, not only visible acknowledgements.

Update `TRANSACTION_OUTCOMES.md`, configuration/operator guidance, embedded API documentation, and SDK examples. Existing protocol documentation explicitly distinguishes durability/materialization and warns against retries: `docs/guide-docs/client-server-protocol-spec/TRANSACTION_OUTCOMES.md:41`. Extend that distinction, and remove claims equating materialization with current visibility.

### Deterministic CI tests required

1. Hold lower version N at an explicit hook; **spawn** higher commits. Assert inner outcome completion separately, W remains below target, and bounded acknowledgements become pending after paused-time advancement.
2. Strict counterpart stays pending until N settles.
3. W advancement at expiry wins over timeout; W also wins over simultaneous reopen.
4. Reopen returns pending without changing materialization.
5. Published WAL durability warning and pending visibility coexist.
6. No-allocation commits and direct-write no-ops avoid unnecessary timers.
7. Fast/slow CRUD barrier paths release guards before observation; lower replay can proceed.
8. Dropping only the observer restores waiter counts and leaves commit/recovery/background-tail state intact.
9. Bounded post-commit backpressure cannot add an independent budget.
10. Explicit, implicit, nested, multi-repo, and suppressed-result batches retain receipts.
11. Interactive pending commit consumes the handle and releases snapshot ownership.
12. Pending response shrinks byte reservations and releases dispatch permits through normal writer ownership.
13. AsOf timeout returns no downgraded snapshot; strict/reopen/future-version cases remain distinct.
14. Reconciliation validates authorization, allocation, and repository identity.
15. Wire round trips reject missing committed visibility; old protocol requests are rejected before mutation.
16. Rust/TS helpers do not rollback or replay after pending and preserve lossless versions.

Run these **only in GitHub CI**, through the repository’s centralized test entry points.

---

## 7. Ordering, retries, subscriptions, and `changes_since`

**Pending means the commit decision is known, but repository-prefix visibility is not established.**

Consequences:

- Do not resend the write.
- A subsequent fresh W-based snapshot may precede the commit.
- Same-connection completion order does not establish a repository visibility fence.
- Reconcile until visible, then open a **new** snapshot; an already-open older snapshot does not advance.
- Visibility alone does not guarantee secondary-index readiness for async-index mode, or repair deferred projections. Those existing independent caveats remain.

**OBSERVED:** changefeed broadcasts can happen inside commit finalization, before the wrapper’s acknowledgement wait: `crates/shamir-engine/src/tx/finalize.rs:103`. Subscription draining treats broadcasts as hints and reads journal pages while tracking sampled W: `crates/shamir-server/src/subscriptions/journal_drain.rs:78`, `crates/shamir-server/src/subscriptions/journal_drain.rs:94`.

**Decision:** preserve subscription W-bounding; do not deliver a pending version merely because the client received its receipt. Settlement may unblock a batch of later events. Visibility acknowledgement does not imply push delivery has completed.

`changes_since` currently exposes events and `gap_at`, not a visibility receipt: `crates/shamir-db/src/shamir_db/execute/admin_retention.rs:191`. Add a sampled `visible_through`/processed-through position to its response and preserve settled-prefix bounds. Empty events are not proof that a pending write aborted or disappeared. Journal gaps/retention still require resynchronization; this operation is not the primary reconciliation API.

---

## PR-sized implementation sequence

| PR | Scope | Dependency |
|---|---|---|
| **1. Contract and protocol** | Shared visibility/options/receipt types; `transaction_info.rs`; wire messages/version gates; builders; protocol docs and serde tests | First |
| **2. Observation adapter and reconciliation** | Engine visibility-wait adapter; authenticated DB/server wait operation; read timeout plumbing; configuration parsing/validation | PR 1 |
| **3. Transaction acknowledgement** | `tx/commit.rs`, `tx_outcome.rs`, repo commit accessors, interactive facade/handlers, `batch_execute.rs`; deadline-aware post-commit backpressure | PRs 1–2 |
| **4. Direct and implicit write propagation** | `mvcc_store/mod.rs`, `table_manager_crud.rs`, `repo_instance.rs` implicit helper, `query_runner.rs`, per-alias receipts and dependency handling | PRs 1–3 |
| **5. SDK adoption and operational rollout** | Rust/TS typed APIs, lossless versions, helpers/examples, resource-lifecycle integration tests, observability exports | PRs 1–4 |

PRs 2 and SDK type/builder preparation can run in parallel after PR 1. **Do not enable bounded server mode until PR 4 closes all outcome-discarding paths and supported clients are updated.**

### Parallelism with REPO-DIAGNOSTICS

Keep this program’s wait adapter outside `repo_tx_gate.rs`, using existing `await_visible`, floor, reopen, and allocation accessors. That allows contract, configuration, engine wrapper, CRUD propagation, clients, and read-side work to proceed while diagnostics owns:

- `repo_tx_gate.rs`
- `completion_tracker.rs`
- `version_guard.rs`
- drainer observation counters.

Coordinate the small change making the gate’s non-tx acknowledgement helper return observation metadata after diagnostics lands, rather than concurrently rewriting it. Serialize final `observability.rs` integration or agree its metric-export interface first.

**Final recommendation:** proceed with this split. The feature bounds **post-known-outcome visibility acknowledgement**, not inner commit execution, WAL uncertainty, repository memory growth, or network delivery. Those exclusions must be contractual, not footnotes.

**Files changed:** none. **Tests/builds run:** none, as requested. **Notable source findings:** implicit outcomes are discarded today; async-index background tails retain writer guards; existing post-visibility backpressure must share the bounded acknowledgement budget.
