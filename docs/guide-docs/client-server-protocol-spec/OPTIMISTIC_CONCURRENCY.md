# Optimistic Concurrency Control (CAS) — `with_version` / `expected_version`

This document specifies the wire fields and error codes for optimistic-
concurrency control (compare-and-set) in SHAMIR DB. It is the authoritative
reference for the FG-2 feature.

## Overview

SHAMIR DB provides a two-part optimistic-concurrency (CAS) contour:

1. **Read side** — `ReadQuery::with_version` requests per-record versions in
   `QueryResult::versions`.
2. **Write side** — `UpdateOp::expected_version` / `DeleteOp::expected_version`
   guards the write: a mismatch on any matched row aborts the whole operation
   with a `version_conflict` error.

Both are opt-in and backward-compatible (`false`/`None` by default). Existing
peers that don't use them see no wire changes.

## Read side: `with_version` + `versions`

### Request field

`ReadQuery.with_version: bool` (default `false`, `skip_serializing_if = false`):

```json
{
  "from": "accounts",
  "with_version": true
}
```

### Result field

`QueryResult.versions: Option<Vec<u64>>` (default `None`,
`skip_serializing_if = "Option::is_none"`):

```json
{
  "records": [{ "id": "abc", "balance": 100 }],
  "versions": [3]
}
```

- `versions[i]` is the canonical committed version of `records[i]` (sourced
  from `MvccStore::version_of` — the same accessor SSI read-set validation
  uses).
- The array is `Some` only when the originating `ReadQuery::with_version ==
  true` AND the table has an MVCC backing store.
- Paths that cannot structurally attribute a single record version to a result
  row (aggregates, `ORDER BY` / `DISTINCT` which reorder rows, non-MVCC tables)
  leave `versions = None` even when `with_version == true`. This is opt-in
  assistance, never a correctness contract — callers must handle `None`.

## Write side: `expected_version`

### Request fields

`UpdateOp.expected_version: Option<u64>` and `DeleteOp.expected_version:
Option<u64>` (default `None`, `skip_serializing_if = "Option::is_none"`):

```json
{
  "update": "accounts",
  "where": { "Eq": { "field": ["id"], "value": "abc" } },
  "set": { "balance": 200 },
  "expected_version": 3
}
```

### Semantics

When `expected_version` is `Some(v)`:

- **Every matched row must currently be at version `v`.** If ANY matched row's
  committed version differs, the ENTIRE operation is rejected with
  `version_conflict` and NO row is modified (no partial application).
- **Zero matched rows is a no-op** (`affected: 0`) — matching the existing
  convention for a WHERE that matches nothing.
- The check is a hybrid (two steps):
  1. **Immediate check at staging time**: for each matched row, call
     `MvccStore::version_of(key)` and compare against `v`. A mismatch aborts
     immediately.
  2. **SSI registration**: each matched key is registered as a tx read at
     version `v` via the EXISTING `TxContext::record_read` /
     `validate_read_set` SSI contour. This closes the race window between step
     1 and this tx's actual commit — a concurrent committer writing the same
     key after step 1 will cause commit-time `validate_read_set` to abort.

### ⚠️ Isolation caveat — step 2 requires `Serializable`

`TxContext::record_read`/`record_read_shared` is a documented no-op under
`IsolationLevel::Snapshot` (SSI read-set tracking is gated on
`Serializable`). A **plain, non-transactional** `UpdateOp`/`DeleteOp` (an
ordinary single-op `client.execute(...)` call with no explicit transaction)
runs through `RepoInstance::run_implicit_batch_tx`, which is **hardcoded to
`IsolationLevel::Snapshot`** — deliberately, so ordinary non-tx writes never
abort on an SSI conflict (last-writer-wins semantics).

Consequence: on that plain non-transactional path, `expected_version` gets
ONLY step 1 (the immediate stale-read check) — step 2 (the commit-time race-
window backstop) never fires. Two concurrent non-transactional writers that
both read the same pre-write version and both pass step 1 before either
commits will **both succeed**, silently violating the "exactly one wins"
guarantee this document describes above.

**To get the full two-step CAS guarantee (including the race-window
backstop), wrap the `expected_version` write in an explicit transactional
batch with `Serializable` isolation:**

```rust
let mut b = Batch::new();
b.update("u", update("table").where_(filter::eq("id", key)).set(value).expected_version(v));
b.transactional().isolation(Isolation::Serializable);
client.execute(db, b.build()).await
```

Under a transactional batch, a step-2 abort surfaces differently than a
step-1 abort: the request still returns `Ok(BatchResponse)`, but with
`transaction.status == "aborted"` and `transaction.reason == Some("tx_conflict")`
(the existing generic SSI-conflict convention) rather than a top-level
`version_conflict`-coded error. Callers that want a uniform "did my CAS
write lose the race" check must handle BOTH shapes — see
`crates/shamir-server/tests/version_cas_e2e.rs`'s
`concurrent_cas_via_real_server_exactly_one_wins` for a worked example.

Proven correct under `Serializable` (both the concurrent-writer race closing
correctly, and the wire-level abort shapes above):
`crates/shamir-engine/src/table/tests/version_cas_tests.rs`'s
`concurrent_cas_exactly_one_wins` (direct engine `begin_tx(Serializable)`)
and `crates/shamir-server/tests/version_cas_e2e.rs`'s
`concurrent_cas_via_real_server_exactly_one_wins` (real server + real
`shamir_client::Client`, transactional + `Serializable`).

### Builder methods

| Language | Read builder | Update builder | Delete builder |
|----------|-------------|----------------|----------------|
| Rust | `Query::with_version()` | `Update::expected_version(v)` | `Delete::expected_version(v)` |
| TypeScript | `Query.withVersion()` | `UpdateBuilder.expectedVersion(v)` | `del(table, where, { expectedVersion: v })` |

## Error code: `version_conflict`

When the immediate check fails, the error surfaces as:

- `BatchError::QueryError { code: Some("version_conflict".to_string()), ... }`
  in batch responses.
- `DbResponse::Error { code: "version_conflict", message: "table '...': key ...
  expected version N but found M" }` on the wire.

### TS SDK surfacing

The TS SDK surfaces this as a `ShamirDbError` with `code === "version_conflict"`.
Use the `isVersionConflict(err)` helper to branch:

```typescript
import { isVersionConflict } from 'shamir-client-ts';

try {
  await db.run(update(...).expectedVersion(v));
} catch (err) {
  if (isVersionConflict(err)) {
    // Re-read the current version, then retry with the fresh value.
    const fresh = await db.run(query(...).withVersion());
    await db.run(update(...).expectedVersion(fresh.versions[0]));
  } else {
    throw err;
  }
}
```

`version_conflict` is NOT in `RETRYABLE_ERROR_CODES` because a blind retry
without re-reading would fail identically — the caller MUST re-read the
current version first.
