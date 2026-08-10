# Brief 69 — #1065: DDL status crash-safe write order, correlation id, no swallowed errors

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## Context — this is fixing gaps in an ALREADY-DESIGNED contract, not a redesign

The DDL op-status contract (`op_id` + poll endpoint) was already RFC'd and
partially implemented (#1015). Read
`docs/dev-artifacts/research/2026-08-05-ddl-result-contract-rfc.md` §2.2
(the design: mint `op_id` at dispatch, write `InProgress` BEFORE the first
mutation, tombstone carries `op_id`, terminal write on success/recovery) and
§2.4 (the status vocabulary table) before touching any code — this brief
fixes the IMPLEMENTATION's divergence from that design, found during a
2026-08-09 code review (P0-2) and independently re-verified by the
orchestrator. The operator's scope decision (recorded, GATE task #1084,
completed): implement the FULL crash-safe contract now, not an
"experimental"/best-effort descope.

**Scope boundary — do not expand beyond this.** The RFC's own §4
"Recommended first-implementation slice" already limited scope to hash
DROP/RENAME INDEX (regular+unique) + index2 DROP — the ONLY DDL ops that
currently mint an `op_id` and write status at all
(`admin_table_index.rs:664` DROP, `:824` RENAME). Sorted-family coverage
and DDL-op-log retention/GC are separately tracked as `#1067`/`#1068`
(blocked by this task) — do NOT touch `sorted_index_manager.rs` or
`ddl_op_log::maybe_evict_terminal_records`/`DDL_OP_LOG_CAP` in this task.

## The 4 defects (independently re-verified against the actual code)

### Defect 1 — `InProgress` is never written in production

`DdlOpState::InProgress` is fully documented
(`crates/shamir-query-types/src/read/ddl.rs:84-88`: "Written by the
dispatch handler before the first mutating step") but a grep of
non-test production code finds ZERO writes of it. `op_id = RecordId::new()`
is minted at `admin_table_index.rs:664` (DROP) and `:824` (RENAME) —
BEFORE the mutating `table.drop_index(...)`/`table.rename_index(...)`
call a few lines below — but nothing writes an `InProgress` status record
at that point. The op-status log only ever gets `Succeeded`/
`SucceededViaCrashRecovery` written, all AFTER the mutation already
happened.

**Fix:** immediately after minting `op_id` (right where it's already
minted, `:664` and `:824`), write `DdlOpStatus { op_id, kind, state:
InProgress }` via `ddl_op_log::write_op_status` BEFORE calling the
mutating method. `kind` needs to be resolved at this point using the same
family-detection logic already present a few lines above (`is_regular`/
`is_unique`/`is_index2` for DROP; the RENAME site currently determines
`is_unique` AFTER the mutation via `table.unique_index_exists(&op.to)` at
`:834` — for the InProgress write you need a PRE-mutation family
determination for RENAME too; check whether the existing pre-mutation
guards already resolve which family the source index belongs to, and
reuse that instead of guessing).

### Defect 2 — terminal status is written AFTER the tombstone is cleared, and the write error is swallowed

`admin_table_index.rs:731-736` (DROP) and `:856-860` (RENAME): both call
`ddl_op_log::write_op_status(...)` for the terminal `Succeeded` state, but
by this point the mutation (and its tombstone clear) has ALREADY
completed inline (the mutating call a few lines above already runs the
full drop/rename INCLUDING clearing its own tombstone on success — verify
this by reading `TableManager::drop_index`/`drop_unique_index`/
`drop_index2`/`rename_index`'s bodies). If the process crashes between
the mutation completing and this status write succeeding, the op is done
but durably invisible — no tombstone (already cleared) AND no `Succeeded`
record. Compounding this: if `write_op_status` itself returns `Err`, the
code does `eprintln!(...)` and CONTINUES as if nothing happened — the
client gets back a normal success response with an `op_id` that will
poll as `Unknown` forever.

The SAME ordering defect exists at the RECOVERY sites (verify exact
current line numbers before editing, code shifts):
`crates/shamir-engine/src/table/table_manager.rs`'s hash-DROP recovery
(~397-441) and index2-DROP recovery (~638-666: `recover_index2_drops`
clears the tombstone, THEN a status write happens after), and
`crates/shamir-engine/src/table/table_manager_index_mgmt.rs`'s hash-RENAME
recovery (~1483-1526, ~1577-1615: `clear_all_renaming` runs before the
`SucceededViaCrashRecovery` write).

**Fix — apply at EVERY one of these sites (inline success path AND all 3
recovery paths):**
1. Write the terminal status FIRST (durably), THEN clear the tombstone.
   The task's own required-contract text allows this as the acceptable
   fallback to a single atomic transaction ("допустимо — сначала durable
   terminal status, потом идемпотентная очистка tombstone" — sequential,
   status-first, is fine; a single atomic `Store::transact` combining both
   into one write is BETTER if the tombstone-clear at that site is a
   simple `info_store` key removal reachable at the same call site — check
   case by case, don't force a bigger refactor than needed to reorder).
   Tombstone-clear must already be idempotent (removing an already-absent
   key is a no-op) — confirm this holds at each site before relying on it;
   if a site's clear function is NOT idempotent, flag it and do not touch
   that site's ordering without discussing it in your report.
2. **Do not swallow the status-write error.** If the terminal status write
   fails, the caller must know the op's completeness is now AMBIGUOUS
   from the client's perspective (the mutation succeeded, but polling
   might not find it). Do not silently return success. At minimum:
   `log::error!` instead of `eprintln!` (see Defect "additionally found"
   below), AND enrich the returned success response (or, if this proves
   too invasive for the inline path, at minimum the RECOVERY paths' log
   output) so an operator can find these cases — check whether there's an
   existing enriched-error convention in this codebase for "mutation
   succeeded but a downstream durable step failed" (grep for the #967
   pattern already used elsewhere in `admin_table_index.rs`/
   `table_manager_index_mgmt.rs` — e.g. search for "Call
   TableManager::verify()" in DDL error messages) and reuse that shape
   rather than inventing a new one.

### Defect 3 — `eprintln!` in library code (found independently, not in the original review)

`admin_table_index.rs:734` and `:859` are 2 of only 2 non-test
`eprintln!` call sites in the whole `shamir-db`+`shamir-engine` codebase
— everywhere else uses `log::error!`. Fix both to `log::error!` while
you're already touching these exact lines for Defect 2.

### Defect 4 — the client cannot learn `op_id` if the server crashes/disconnects before the response arrives

`op_id` is server-minted and only ever communicated to the client via the
successful synchronous response's `QueryResult::op_id` field. If the
server crashes (or the connection drops) before that response is sent,
the client has NO way to learn the `op_id` it needs to poll by later —
the crash-safety contract this whole task exists to build is unusable in
exactly the scenario it's meant to cover.

**Fix — client-supplied correlation id (lower blast radius than a
separate `BeginDdl` round-trip endpoint, and avoids doubling DDL
latency):** add an optional correlation field to `DropIndexOp`/
`RenameIndexOp` (check the actual struct names/location —
`shamir-query-types`'s batch-op types, find them via the same module
`admin_table_index.rs` already imports `BatchOp::DropIndex`/
`BatchOp::RenameIndex` from). Something like `request_id:
Option<RecordId>` (or a client-chosen `String`/UUID — pick whichever
integrates more cleanly with `RecordId`'s existing `FromStr`/`Display`,
since the op-status log is already keyed by `RecordId`'s raw 16 bytes,
`ddl_op_log.rs:34-39`). If the client supplies one, the server uses it
AS the `op_id` (instead of minting a fresh `RecordId::new()`) — so even
if the response never arrives, the client already knows the exact id it
sent and can poll by it. If absent (old client, or a caller that doesn't
care), fall back to `RecordId::new()` exactly as today — fully backward
compatible, additive field only (matches this repo's established
`#[serde(default, skip_serializing_if = ...)]` convention for new wire
fields — check `query_result.rs`/`batch_response.rs` for the exact
pattern already in use and mirror it).

**Idempotent retry.** When a client-supplied `request_id` is present and
an op-status record ALREADY exists for it (from a previous send of the
same request — e.g. the client retried after a timeout, not knowing if
the first attempt landed), the dispatch handler must NOT re-run the DDL
mutation a second time — it should short-circuit and return the EXISTING
status instead. Add this check right after resolving the correlation id,
before any mutation: `ddl_op_log::read_op_status(table.info_store(),
&op_id).await` — if `Some(existing)`, return based on `existing.state`
(e.g. `InProgress`/`Succeeded`/`Failed` all mean "don't re-run, report
what's known"; decide the exact response shape for each case and
document your reasoning in the diff).

**Versioned envelope.** `ddl_op_log.rs:51`
(`bincode::serialize(status)`) has no format-version tag — a future
schema change to `DdlOpStatus` would silently fail to decode old records
(or worse, misdecode). Wrap the serialized bytes in a small versioned
envelope, e.g. a single leading version byte (`0x01` for the current
shape) before the bincode payload, checked on read
(`read_op_status` rejects/reports an unrecognized version rather than
attempting to decode it as the current shape). Keep this minimal — a
one-byte tag, not a generic versioning framework.

## Do not touch

- `docs/dev-artifacts/research/2026-08-05-ddl-result-contract-rfc.md` (read
  only — reference document, not part of this change).
- `sorted_index_manager.rs` and any sorted-family DDL path (out of scope,
  tracked separately as `#1067`).
- `ddl_op_log::maybe_evict_terminal_records`/`DDL_OP_LOG_CAP` (out of
  scope, tracked separately as `#1068`).
- `crates/shamir-server`'s `GetDdlOpStatus` authorization (already fixed,
  `#1064`, committed `4a993a39`) — do not touch `handler.rs`'s
  `get_ddl_op_status` method's authorization logic.

## Tests (TDD)

1. **InProgress written before mutation.** Install a way to observe the
   op-status log's state mid-operation (a pause hook, or simply check
   there's a natural seam — e.g. if the mutating call can be raced via
   `tokio::select!` against a hook the SAME way `#1060`'s crash tests did,
   reuse that exact pattern: `tokio::select!` racing the DDL call against
   a pause hook, NEVER `tokio::spawn`+`drop`). Assert the op-status log
   shows `InProgress` for the `op_id` while the operation is paused
   mid-flight.
2. **Terminal status durable before tombstone clear (or atomic).** For at
   least the DROP INDEX inline path: after a successful DROP, assert the
   status log shows `Succeeded` — this should already pass; the NEW test
   value is proving the ORDER via a crash-simulation at the exact
   boundary between status-write and tombstone-clear (mirror `#1060`'s
   `select!`-based crash pattern) — after the simulated crash, reopen and
   assert EITHER the tombstone still exists (status already durably
   written, recovery can still run) OR (if you implemented one atomic
   transact) both are already consistent — never "status missing but
   tombstone also gone."
3. **Status-write failure is not silently swallowed.** Inject a write
   failure (check if `InMemoryStore` or a test double supports failure
   injection elsewhere in this codebase — grep for existing failure-
   injection patterns in DDL-adjacent tests before inventing a new one)
   and assert the caller gets a clear signal (not a bare success as if
   nothing happened).
4. **Client-supplied correlation id round-trips.** DROP INDEX with a
   supplied `request_id` → the returned `op_id` equals it → polling by
   that same id finds the status.
5. **Idempotent retry.** Send the SAME DROP INDEX request (same
   `request_id`) twice in a row. Assert the second call does NOT
   re-execute the drop (e.g. assert no error/double-drop artifact) and
   returns the SAME status as the first.
6. **Versioned envelope round-trips.** Write then read a status record,
   assert it decodes correctly; if practical, assert an unrecognized
   version byte is rejected cleanly (not panicking, not silently
   misdecoding).

**Every test must FAIL on code lacking the mechanism it proves** — this
codebase's own convention (already stated in `#1061`'s brief this
session, reused here): a test that passes with or without the fix has no
regression value.

## Gate before you report done

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-index
./scripts/test.sh -p shamir-engine
./scripts/test.sh -p shamir-db
./scripts/test.sh -p shamir-server
```

This is a large, multi-file change touching wire types, two DDL handlers,
and (at least) three recovery functions. If you run out of time/budget
mid-way, STOP and report EXACTLY what's done vs. not — per-defect, not
"mostly done" — do not claim completion for a defect you didn't actually
verify end-to-end. Report the exact diff, which tests you wrote, their
individual pass/fail status, and the full gate's final summary lines for
every crate above.
