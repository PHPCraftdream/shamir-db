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
| `materialized` | Whether all commit projections completed inline; only meaningful for committed outcomes. Deserialization defaults to `true` for older payloads. |

(Source: `crates/shamir-query-types/src/batch/transaction_info.rs`.)

### Published commit with a durability error

```json
{
  "tx_id": 7,
  "status": "committed",
  "error": "wal_durability",
  "snapshot_version": 40,
  "commit_version": 41,
  "materialized": true
}
```

`wal_durability` means the commit was published despite failed WAL durability.
It is **not an abort** and does not guarantee crash durability. **Never retry
these writes** as if they had failed: reconcile using the returned transaction
and version. `wal_completion` distinguishes written-but-not-durable from
not-written and indeterminate WAL outcomes; the commit pipeline reports
`WalDurability` after publication/materialization, and `into_transaction_info`
retains committed status (source: `crates/shamir-engine/src/tx/commit.rs`).

The TS `db.tx()` helper throws an `Error` for this committed outcome, carrying
`transaction` (the entire metadata object) and `commit_version`. It does **not**
issue rollback after that finalizing response. An exception from this helper
therefore does not by itself mean the writes were aborted
(source: `crates/shamir-client-ts/src/core/db.ts`).

`wal_requires_reopen` is different: WAL outcome was indeterminate, the repository
is marked as requiring reopen, and new commits are refused. Stop issuing commits
and reopen/reconcile the repository rather than blindly replaying writes. The
current metadata mapping reports `status: "aborted"`,
`reason: "wal_requires_reopen"`; that is not evidence that an indeterminate WAL
write is absent on disk (source: `crates/shamir-engine/src/tx/commit.rs`,
`wal_completion` and the commit-entry reopen gates).

### VIS-1: query v3 visibility contract

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
  requests without clamping; operator policy/configuration is a later stage.
- Absence selects the existing strict default. `TxExecute` rejects this
  commit-only override with `validation`, rather than silently ignoring it.

**This stage defines and carries the option only. It does not implement a
bounded wait or enable bounded mode. Existing commits keep their existing
strict wait/materialization behaviour and populate `visible`.**

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
completion. Pending production, reconciliation and SDK receipt APIs belong to
later stages; this subsection does not promise a bounded end-to-end response.

### Materialization is not durability

`materialized: true` means projections (main store, counters, secondary indexes,
vector graph) landed inline. `false` means at least one projection was deferred
to idempotent recovery on the next repository open: restart-bounded eventual
visibility, **not an abort**. Do not replay the transaction to repair a deferred
projection. A `wal_durability` error remains a durability warning regardless of
this flag; do not infer guaranteed crash recovery from `materialized` alone
(source: `crates/shamir-query-types/src/batch/transaction_info.rs`,
`crates/shamir-engine/src/tx/commit.rs`).

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

### AsOf read errors (forwarded QueryError.code)

| Code | Meaning / action |
|---|---|
| `as_of_future_version` | Requested version exceeds the repository allocation high-water mark; use an existing version or a consistent snapshot position. |
| `as_of_reopen_required` | Requested allocated version cannot settle because the repository requires reopen; reopen/reconcile before retrying. |

Allocated AsOf positions above the contiguous completion watermark wait until
all versions through the requested position materialize or abort. Positions at
or below the watermark do not wait, even when reopening is required.

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
commit, regardless of its eventual committed/aborted outcome. Rollback also
removes it (source: `crates/shamir-server/src/db_handler/tx_handlers.rs`).

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
