# Transaction outcomes and execution error codes

Reference for the current implementation; additive fields must be tolerated by
clients. The payload is named-field MessagePack; JSON below illustrates only the
wire format, not query construction.

## TransactionInfo

Transactional `Execute` returns `BatchResponse.transaction`; `TxExecute` leaves
it absent because the transaction remains open. `TxCommit` returns
`TxCommitted.transaction`, including when the commit outcome is aborted.
The response variant alone is not proof of success: inspect `status`, then
`error` (sources: `crates/shamir-db/src/shamir_db/execute/db_tx.rs`,
`crates/shamir-engine/src/query/batch/batch_execute.rs`).

| Field | Contract |
|---|---|
| `tx_id` | Repository transaction identifier. |
| `status` | `"committed"` or `"aborted"`. |
| `reason` | Abort diagnostic; omitted when absent, not necessarily a stable code. |
| `error` | **Additive, optional** published-commit error; omitted when absent. Does not change `status` or set `reason`. |
| `snapshot_version` | Read snapshot version; present for committed outcomes. |
| `commit_version` | Published version; present for committed outcomes, absent for aborted outcomes. |
| `materialized` | Inline projection outcome, independent of visibility and WAL durability. A timeout acknowledgement does not change it; a reopen-required acknowledgement reports `false` (`Deferred`), as before. Only meaningful for committed outcomes. Deserialization defaults to `true` for older payloads. |

(Source: `crates/shamir-query-types/src/batch/transaction_info.rs`.)

### Published commit with a durability error

```json
{
  "tx_id": 7,
  "status": "committed",
  "error": "wal_durability",
  "snapshot_version": 40,
  "commit_version": 41,
  "materialized": true,
  "visibility": { "state": "visible" }
}
```

`wal_durability` means the commit was published despite failed WAL durability.
It is **not an abort** and does not guarantee crash durability. **Never retry
these writes** as if they had failed: reconcile using the returned transaction
and version. `wal_completion` distinguishes written-but-not-durable from
not-written and indeterminate WAL outcomes; the commit pipeline reports
`WalDurability` after publication/materialization, and `into_transaction_info`
retains committed status (source: `crates/shamir-engine/src/tx/commit.rs`).

The TS `db.tx()` helper resolves this committed outcome as a receipt containing
`value`, `transaction` (the entire metadata object), and discriminated
`state: visible | pending` / `visibility`. It does not issue rollback after a
commit attempt, even when acknowledgement is unknown. Inspect the retained
`transaction.error`; a published durability warning never becomes an abort.

`wal_requires_reopen` is different: WAL outcome was indeterminate, the repository
is marked as requiring reopen, and new commits are refused. Stop issuing commits
and reopen/reconcile the repository rather than blindly replaying writes. The
current metadata mapping reports `status: "aborted"`,
`reason: "wal_requires_reopen"`; that is not evidence that an indeterminate WAL
write is absent on disk (source: `crates/shamir-engine/src/tx/commit.rs`,
`wal_completion` and the commit-entry reopen gates).

### Query v3: transactional visibility acknowledgement

Query version **3** requires explicit opt-in on `Execute`, `TxBegin`,
`TxExecute`, and `TxCommit`. Older, future, or omitted versions return
`DbResponse.Error { code: "unsupported_query_version", ... }` before database
execution or interactive commit consumes a handle. Omission is legacy version
0, not implicit support for new outcome semantics. The auth handshake version
is unchanged.

`BatchRequest.visibility_wait` and `TxCommit.visibility_wait` are optional:

- `{ "mode": "strict" }` means strict settled-prefix acknowledgement.
- `{ "mode": "bounded", "timeout_ms": 0 }` describes an immediate predicate
  check, never infinity. All millisecond values are unsigned u64s. The policy
  validation helper rejects forbidden strict requests and above-cap bounded
  requests without clamping; the server applies `[transactions]` policy before
  admission or writes and reports `visibility_wait_invalid` on rejection.
- Absence selects the configured Strict default even when explicit Strict
  overrides are forbidden. `TxExecute` rejects this commit-only override
  (including nested bodies) with `validation`, rather than silently ignoring it.

Per-request Bounded is supported on `TxCommit` and top-level transactional
`Execute`. Non-transactional Execute (including implicit writes) and nested
Batch/ForEach bodies cannot return the required commit receipt and reject
Bounded before writes. The server checks caps and Strict permission recursively
for every explicit override, including bodies that might not execute. Server
startup still rejects `visibility_ack_mode = "bounded"` pending VIS-4/VIS-5;
per-request support does not enable a bounded server default.

Every committed `TransactionInfo` in v3 includes `visibility`; aborted metadata
omits it. Missing/null committed visibility, visibility on an abort, invalid
state/reason tags, and invalid sampled-prefix arithmetic are malformed, never
inferred to be visible. `is_committed()` remains a commit-decision predicate;
`is_visible()` independently checks committed status and `state: "visible"`.

The named-field MessagePack shapes are illustrated as JSON:

```json
{
  "tx_id": 7,
  "status": "committed",
  "snapshot_version": 40,
  "commit_version": 43,
  "materialized": true,
  "error": "wal_durability",
  "visibility": {
    "state": "pending",
    "visible_through": 41,
    "blocking_version": 42,
    "reason": "timeout"
  }
}
```

The other observation is `{ "state": "visible" }`. Pending reasons are
`"timeout"` and `"reopen_required"`. Pending remains committed, with no abort
`reason`; a WAL durability warning is independent and retained. Do not resend
writes on pending. `blocking_version` is exactly `visible_through + 1`, the
first position outside the sampled contiguous settled prefix; it is **not** a
claim that this position is wedged, its owner known, or it is the only obstacle.
Overflow cannot describe a valid pending observation. Visible establishes only
prefix settlement, not stronger WAL durability, materialization or async-index
completion. Pending is a known committed decision, **not a retryable failure**.

#### Acknowledgement budget and reconciliation

The Bounded budget starts only after the inner commit returns a known outcome
(success or a published `wal_durability` warning). Strict waits for W to reach
the commit version; Bounded checks W immediately, waits at most its budget, and
re-samples W at expiry so visibility wins. Reopen before visibility yields
Pending with `reason: "reopen_required"` and `materialized: false` (`Deferred`).
Read-only/no-allocation commits already at their snapshot position are Visible.
Pre-commit aborts and indeterminate WAL outcomes keep their existing classification;
a timeout never manufactures committed metadata for them.

**Contractual exclusions:** this is not a bound on inner commit execution,
execution/admission time, WAL uncertainty, response encoding, channel/socket
backpressure, or end-to-end network response time. In async-index mode the
inner driver awaits its spawned materialization tail on a durability error;
that await is inside the inner commit and is excluded from the visibility budget.
A disconnect or client round-trip timeout is unknown acknowledgement, not proof
of Pending, abort, or absence of writes.

Pending skips post-ack soft backpressure. A bounded Visible acknowledgement
uses only the remaining budget for that soft brake; Strict retains its existing
five-second soft-pressure ceiling. This is soft pressure, **not a hard memory
admission guarantee**. Dropping only the visibility observer does not cancel
materialization, settle the blocking version, or remove recovery obligations.

Do not retry or roll back Pending writes. Reconcile with `WaitForVisibility`
against the same repository and returned `commit_version`; Bounded zero is an
immediate status check. Visible proves settlement through the position, not
whether an unknown transaction committed (aborted versions also settle). Do not
use a receipt against a deleted/recreated repository. After reconciliation, open
a **new snapshot/transaction** for reads: an old snapshot is not advanced by
waiting. On `reopen_required`, reopen/reconcile first; visibility observation
never repairs projections or upgrades durability. Rust/TS transactional SDK
receipts are adopted in VIS-5a; implicit per-alias SDK receipts remain VIS-5b.

### Embedded direct-write receipts (VIS-4a)

`TableManager::{insert, insert_many, delete, set}_with_receipt(..., wait)`
returns a must-use `WriteOutcome<T>` with the ordinary value, an optional
`commit_version`, and `VisibilityState`. Values are respectively `RecordId`,
`Vec<RecordId>`, `bool` (removed), and `bool` (created). `None` means no version
was minted: non-MVCC writes, missing-record delete and empty insert-many do
not expose zero as a reconciliation position or create a visibility timer.

Each caller chooses explicit Strict or Bounded; Bounded zero is an immediate
status sample. W is checked before reopen, and re-sampled on expiry so visibility
wins. Pending preserves the committed value/position and is **not retryable**.
Writer drain and unique-write guards are released before either observation
path, allowing lower posting replay to proceed. Dropping only the observer
restores waiter registration without undoing the write or settling its blocker.

The existing value-only methods (including internal version-returning adapters)
are explicitly Strict, never inheriting a bounded repository default. If reopen
prevents visibility, they return `DbError::WriteCommittedVisibilityPending`
(`write_committed_visibility_pending`) carrying `commit_version`,
`visible_through`, and `reason: "reopen_required"`. This exceptional outcome
means **committed**, not failed/aborted; do not resend it. Reconcile against the
same repository incarnation and then open a new snapshot.

Direct `MvccStore` writes expose analogous receipt forms for set/delete and both
many variants, including immediate-observation no-wait receipt forms. Their
low-level delete still writes a tombstone even for an absent key; missing-record
no-op semantics belong to TableManager. The lower storage/tx layers use local
visibility types; engine receipts use the shared query DTO types.

**No durability upgrade:** direct CRUD remains best-effort/no-WAL and unsafe to
cancel mid-sequence (data, counters and indexes are not a transaction). Bound
only post-known-outcome observation, never the entire write. A receipt promises
neither transactional atomicity, fsync/crash durability, async-index completion,
nor bounded end-to-end latency. Direct writes add no independent post-ack brake;
existing transactional soft backpressure and execution/admission limits are
unchanged. Implicit/network batch receipts remain VIS-4b; server bounded-default
rollout and SDK adoption remain gated.

### Materialization is not durability

`materialized` describes the projection outcome, **not a visibility field**.
`false` (`Deferred`) can mean a projection was deferred to idempotent online
orphan replay or cold recovery on repository open. A visibility acknowledgement
that expires leaves materialization untouched; one that observes
`ReopenRequired` reports `Deferred`, even if this commit's own projections
succeeded. It is **not an abort**, nor a
next-open-only or bounded visibility guarantee. Do not replay the transaction
to repair it. A
`wal_durability` error remains a durability warning regardless of this flag;
do not infer guaranteed crash recovery from `materialized` alone. Sources:
`crates/shamir-engine/src/tx/materialize.rs:255-273`,
`crates/shamir-engine/src/tx/drainer.rs:1190-1211`,
`crates/shamir-engine/src/tx/recovery.rs:413-430`,
`crates/shamir-engine/src/tx/commit.rs`.

For commits that allocate a version, Strict acknowledgement waits for the
contiguous completion watermark W to reach it (all versions through it must
materialize or abort). Read-only/empty commits skip this wait. A stuck lower
version can delay Strict acknowledgements indefinitely. Bounded expiry or
reopen returns Pending, not Deferred or proof of visibility. Embedded direct
writes now expose receipts or explicit Strict exceptional outcomes as described
above. Sources: `crates/shamir-engine/src/tx/ack/`,
`crates/shamir-engine/src/table/write_receipt/`,
`crates/shamir-tx/src/mvcc_store/write_receipt/`.

`requires_reopen` is repository state, **not a generic wire error code or a
TransactionInfo field**. The commit-entry metadata reason is
`wal_requires_reopen` (documented above); a pending AsOf read instead maps to
`as_of_reopen_required` (below). Sources:
`crates/shamir-engine/src/tx/commit.rs:389,814-829`,
`crates/shamir-query-types/src/batch/transaction_info.rs:7-48`,
`crates/shamir-storage/src/error.rs:117-118`.

## BatchError → DbResponse.Error.code

This table exhausts the fixed mapping in
`crates/shamir-server/src/db_handler/handler.rs::error_code`. Structured
`QueryError.code` values are forwarded verbatim, so this is not a closed
allowlist of every operation-specific DDL/query code.

| Code | Source variant / meaning |
|---|---|
| `pagination_too_large` | `LimitExceeded(PageSize / Offset)` |
| `work_budget_exceeded` | `LimitExceeded(Work / TotalQueries)` |
| `result_too_large` | `LimitExceeded(ResultBytes)`; policy-enabled execution materialization budget |
| `nesting_too_deep` | `LimitExceeded(Depth)` or legacy `NestingTooDeep` |
| `too_many_iterations` | `LimitExceeded(Iterations)` or legacy `TooManyIterations` |
| `limits` | `TooManyQueries`, `TooDeep` (dependency depth), or `ExecutionTimedOut` |
| `validation` | `CircularDependency`, `UnknownAlias`, `AfterPathIgnored`, `InvalidWhenFilter`, `InvalidCondCondition` |
| `timeout` | `Timeout` (distinct from the cooperative execution deadline) |
| `lock_timeout` | `LockTimeout` |
| `unknown_db` | Untagged `QueryError` with empty alias and message containing `not found` (legacy heuristic, can include repository lookup failures) |
| `access_denied` | Untagged `QueryError` beginning `access denied:`; also forwarded when explicitly coded |
| `query` | Other untagged `QueryError` |
| `tx_cross_repo_not_supported` | `CrossRepoNotSupported` |
| `cursor_not_found` | `CursorNotFound` |
| `cursor_expired` | `CursorExpired` |
| `cursor_limit_exceeded` | `CursorLimitExceeded` |
| `cursor_temporal_not_supported` | `CursorTemporalNotSupported` |
| `cursor_with_version_not_supported` | `CursorWithVersionNotSupported` |
| `invalid_page_size` | `InvalidPageSize` |
| `cursor_page_too_large` | `CursorPageTooLarge` (serialized cursor page cap) |

### Post-execution response rejection

`result_too_large` also comes directly from encoded Execute response enforcement,
after execution and subscription activation. It can therefore follow committed
writes and discard the transaction metadata: **it is not proof of rollback and
not a blind-retry guarantee**. Reconcile the outcome before issuing writes
again. Serialized cursor page rejection is `cursor_page_too_large`, not
`result_too_large`. Sources:
`crates/shamir-server/src/db_handler/handler.rs:595-605,631-642,747`,
`crates/shamir-server/src/db_handler/cursor_handlers.rs:315-322`.

### AsOf read errors (forwarded QueryError.code)

| Code | Meaning / action |
|---|---|
| `as_of_future_version` | Requested version exceeds the repository allocation high-water mark; use an existing version or a consistent snapshot position. |
| `as_of_reopen_required` | Requested allocated version cannot settle because the repository requires reopen; reopen/reconcile before retrying. |

Allocated AsOf positions above the contiguous completion watermark wait until
all versions through the requested position materialize or abort. This wait has
no give-up timeout; callers cancel the read to bound it, without settling a
version or changing the requested position. Positions at or below the watermark
do not wait, even when reopening is required. Sources:
`crates/shamir-engine/src/table/read_temporal.rs:85-101`,
`crates/shamir-tx/src/repo_tx_gate.rs:1055-1107`,
`crates/shamir-storage/src/error.rs:117-118`,
`crates/shamir-server/src/db_handler/handler.rs:711-719`.

### Interactive handle lifecycle

`TxExecute` removes the handle on **`ExecutionTimedOut`** (`limits`) and
**every `LimitExceeded` kind** listed above. It also removes it on `tx_expired`
(absolute lifetime exhausted) and `tx_too_large` (staged bytes above the server
transaction cap). Removal initiates rollback/RAII cleanup, not a commit.
Do not continue using the handle; start a new transaction after correcting the
cause (source: `crates/shamir-server/src/db_handler/tx_handlers.rs`).

Classification uses the **variant**, not merely the wire string. Legacy
`TooManyQueries`/`TooDeep` also map to `limits`, but are not in the handle-removal
match; legacy `TooManyIterations`/`NestingTooDeep` similarly share codes with
`LimitExceeded` without its cleanup rule. `Timeout` (`timeout`), `LockTimeout`,
and ordinary `QueryError` do not trigger that match. In particular a
`string_agg` accumulator-cap error is a query error, not `LimitExceeded`.

Other direct interactive codes: `tx_already_open` (one open tx per session),
`tx_error` (registration failed), `tx_not_found`, `tx_forbidden` (wrong session),
`tx_wrong_db`, and `tx_wrong_repo`. Version/admin gates can return
`unsupported_query_version` or `permission_denied`; destructive-op HMAC gate
codes are forwarded. These gates do not apply the budget-error removal rule.
`TxCommit` removes a valid, correctly addressed handle **before** attempting
commit, regardless of Visible, Pending, aborted or error outcome, including an
invalid commit policy. Pending releases snapshot ownership normally and never
re-registers the handle. Version/ownership/address gates still precede removal.
Rollback also removes it (source: `crates/shamir-server/src/db_handler/tx_handlers.rs`).

## Commit outcome reasons and retry decisions

Commit errors are mapped to transaction metadata, not necessarily to a top-level
`DbResponse.Error`. Single-batch execution and interactive commit use the same
`TxError::into_transaction_info` mapping
(`crates/shamir-engine/src/tx/commit.rs`,
`crates/shamir-db/src/shamir_db/execute/db_tx.rs`).

| Metadata value | Meaning / action |
|---|---|
| `reason: "tx_conflict"` | SSI, phantom, or wound-wait conflict. Retryable as a **new transaction**, re-reading state. |
| `reason: "version_conflict"` | Commit-time CAS mismatch; also used by staging-time CAS errors. Re-read and refresh the expected version before retrying; do not blindly resend the stale expectation. |
| `reason: "unique_violation"` | Unique constraint failure. Not retryable without changing input/state. |
| `reason: "table_migrating"` | Source table is frozen for migration. Wait for migration completion/cancellation and resolve the active table before submitting a new write. |
| `reason: "wal_requires_reopen"` | Indeterminate WAL outcome; reopen/reconcile first, no blind retry. |
| `reason` beginning `storage: ` | Storage diagnostic, not a stable retry code. Investigate the underlying failure. |
| `reason` beginning `tx expired: elapsed ` | Engine commit lifetime check failed; diagnostic includes elapsed/max durations, **not** the literal code `tx_expired`. Begin a new transaction. |
| `error: "wal_durability"` with committed status | Published writes; **not retryable**, no rollback. |

Migration freeze can also surface `table_migrating` as a structured query error
on a direct write. Preparation raises the freeze bit, drains writers, then
fences the write lock; transactional commit checks the bit too
(`crates/shamir-engine/src/table/table_manager.rs::begin_migration_freeze`,
`crates/shamir-engine/src/tx/pre_commit.rs`).

## Read-only visibility reconciliation (protocol v3)

`WaitForVisibility { query_version: 3, db, repo, version, visibility_wait }`
returns `VisibilityObserved { version, visibility }`. Repository READ access is
required. `visibility_wait` is `{ mode: "strict" }` or
`{ mode: "bounded", timeout_ms: N }`. Zero is an immediate status check,
not infinity. Excessive bounded waits and forbidden strict waits are rejected
with `visibility_wait_invalid`, never clamped. Unallocated positions are rejected
with `visibility_future_version`. Legacy query versions are rejected before lookup.

`Visible` proves settlement through a **position**, not whether an unknown
transaction committed: aborted allocated positions also lie beneath the settled
prefix. It does not upgrade WAL durability, materialization, index readiness, or
push delivery. Open a new snapshot after reconciliation; old snapshots do not advance.
Pending carries the sampled `visible_through`, `blocking_version = visible_through + 1`,
and `reason: timeout | reopen_required`. Settlement wins over timeout and reopen.
The operation mutates no transaction, creates no snapshot or background polling task,
and cancellation drops only its temporary observer.

Positions are repository/incarnation-scoped. **Current limitation:** this request
has no receipt identity field, although repository identity exists internally.
The server cannot detect a stale receipt for a deleted/recreated repository at the
same name. Do not reconcile such receipts against a replacement repository.

Allocated-but-unsettled AsOf reads use `transactions.read_visibility_timeout_ms`,
limited by the remaining execution/call budget (including interactive execution).
Expiry returns the structured `as_of_visibility_timeout` response with `requested`,
`visible_through`, and `blocking_version`; no older snapshot/data is substituted.
Future-version and reopen errors remain distinct. A read-specific timeout leaves
an interactive handle open; an enclosing execution deadline still removes it.
This bounds the visibility wait, not inner execution, storage I/O, or network delivery.
Embedded table reads can select explicit strict waits with
`FilterContext::with_read_visibility_wait(VisibilityWait::Strict)` or bounded
budgets; strict is not represented by zero. Execution/call budgets are sampled
at the observation seam after asynchronous prerequisites.
Commit acknowledgements support explicit Strict or Bounded waits as above.
