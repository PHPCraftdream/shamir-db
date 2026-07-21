# FG-2: `with_version` full CAS contour — result-side version, expected_version, conflict error, both builders, TS SDK

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## DECIDED CONTRACT (user, 2026-07-21) — do not re-litigate

Complete the `with_version`/CAS feature fully — do not hide the flag or
scope it down. Review 2026-07-21 P0#2: `ReadQuery::with_version` is
accepted on the wire but has ZERO result-side effect — a client asking for
row versions silently gets nothing back. This task delivers the full
contour: read-side version reporting AND write-side optimistic
concurrency control (CAS).

## Context — already investigated, do not re-derive

### Wire contract — result side (DECIDED)

`crates/shamir-query-types/src/read/read_query.rs:37-41`'s `with_version`
flag already exists (request-only, no result wiring — confirmed, this is
the review's exact finding). `QueryResult`
(`crates/shamir-query-types/src/read/query_result.rs`) already has the
established convention of adding an optional, `records`-parallel field for
opt-in extra data (see `stats`/`pagination`/`explain` — all
`#[serde(default, skip_serializing_if = "Option::is_none")]`). Add:

```rust
/// Per-record version, index-aligned with `records` (i.e.
/// `versions[i]` is the version of `records[i]`). `Some` only when the
/// originating `ReadQuery::with_version == true`; `None`/omitted
/// otherwise (backward-compatible — existing peers that don't ask for
/// versions never see this field).
#[serde(default, skip_serializing_if = "Option::is_none")]
pub versions: Option<Vec<u64>>,
```

**Version source (DECIDED, already exists — reuse, do not invent):**
`crates/shamir-tx/src/mvcc_store/mod.rs`'s `MvccStore::version_of(key: &[u8]) -> u64`
is ALREADY the canonical per-key committed version accessor (its own doc
comment: "Used by SSI read-set validation... captures this value when
reading inside a tx, then commit re-queries it"). Call this for each
returned record's key when `with_version == true`.

**Mechanical scope (the hard part — 10+ call sites):**
`crates/shamir-engine/src/table/read_exec.rs` pushes
`QueryRecord::Direct(qv)` at many distinct call sites across many
optimized read paths (full scan, min-index shortcut, various sorted/keyset
paths, `read_temporal.rs`'s `read_history`/`read_as_of`, etc. — you must
find ALL of them, not just a sample). For every site, when
`query.with_version` is true, you additionally need the record's raw KEY
bytes (already available at each site — it's what was used to `get()` the
record or what the scan is iterating over) to call `version_of(key)` and
push the result onto a parallel `Vec<u64>` in lockstep with the
`records.push(...)`. **This is genuinely large mechanical surface — budget
real time for it, and verify you found every push site via
`grep -n "QueryRecord::Direct" crates/shamir-engine/src/table/read_exec.rs
crates/shamir-engine/src/table/read_temporal.rs` (and any other file with
a read path) before declaring done.** If a specific push site's key isn't
readily available without a structurally invasive change (e.g. an
aggregate-only result with no single backing record), it's acceptable for
that site to push `0` (or skip populating `versions` for that shape) — but
you MUST note every such exception explicitly in your summary, do not
silently under-populate the array without saying so.

### Write-side CAS — expected_version (DECIDED contract)

Add to both `UpdateOp` and `DeleteOp`
(`crates/shamir-query-types/src/write/types.rs`):
```rust
/// Optimistic concurrency control: when `Some(v)`, every row this
/// operation would match must currently be at version `v` — if ANY
/// matched row's committed version differs, the ENTIRE operation is
/// rejected with a `version_conflict` error and NO row is modified (no
/// partial application). `None` (default) disables the check — existing
/// callers are unaffected.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub expected_version: Option<u64>,
```

**Semantics (DECIDED — whole-operation abort, not per-row partial
success):** this matches how this engine's existing SSI conflicts already
abort a transaction wholesale rather than partially committing some rows
— treat a CAS mismatch the same way. A `WHERE`-matched multi-row
update/delete with `expected_version` set checks EVERY matched row before
staging ANY write for this op; the first mismatch aborts the whole op.

**Validation timing (DECIDED — reuse the existing SSI contour, do not
duplicate):** `crates/shamir-tx/src/tx_context.rs`'s
`TxContext::record_read(table_id: u64, key: Bytes, version: u64)` and
`validate_read_set<F>(...)` are the EXISTING SSI read-set mechanism (its
own doc comment on `version_of`: "captures this value when reading inside
a tx, then commit re-queries it to detect another tx wrote this key
since"). Implement the CAS check as a hybrid, exactly two steps:
1. **Immediate check at staging time**: for each matched row, call
   `MvccStore::version_of(key)` and compare against `expected_version`
   right now — a mismatch aborts the op immediately with the
   `version_conflict` error (the common case: the client's read is
   already stale by the time it writes).
2. **ALSO call `record_read(table_id, key, expected_version)`** for every
   matched row, exactly as an ordinary tx read would — this makes the
   EXISTING `validate_read_set` commit-time re-check close the race
   window between step 1's check and this tx's actual commit (another tx
   writing the same key in between). Do NOT hand-roll a second/duplicate
   revalidation mechanism — this IS the existing SSI contour, just fed a
   caller-supplied expected version instead of an internally-observed one.

**New typed error** — add a `version_conflict` code, following the exact
existing pattern (`code: Some("fk_actions".to_string())` etc. throughout
`crates/shamir-engine/src/query/batch/*.rs` — plain string codes on
`BatchError::QueryError`, no strict enum in this wire contract). Message
should name the table/key and both the expected and actual version where
feasible (do not leak more than that).

### Rust query-builder (`crates/shamir-query-builder`)

- `crates/shamir-query-builder/src/read/*.rs` (find the `ReadQuery`
  builder, e.g. wherever `.select`/`.where_`/`.order_by` are already
  fluent methods) — add `.with_version(bool)` (or a bare `.with_version()`
  setting `true`, matching this builder's existing boolean-flag naming
  convention — check `explain`/`count_total` for the established style
  and mirror it exactly).
- `crates/shamir-query-builder/src/write/update.rs`'s `Update` builder —
  add `.expected_version(v: u64)`.
- `crates/shamir-query-builder/src/write/delete.rs`'s delete builder —
  same.

### TS client builder + SDK

Find the TS equivalents of the Rust builders above (check
`crates/shamir-client-ts/src/core/builders/` for the read/update/delete
builder files — mirror whatever naming/fluent-API convention is already
established there) and add the same three additions:
`.withVersion()`/`withVersion: true` on the read builder,
`.expectedVersion(v)` on update/delete. Add a typed way for the TS SDK to
surface the new `version_conflict` error distinctly (check how the TS
client currently surfaces OTHER typed `BatchError` codes, e.g.
`fk_actions` or similar, to a caller — mirror that same mechanism, do not
invent a new one).

## Tests (MANDATORY — every level named in the task)

1. **Unit** — a read with `with_version: true` returns a version per
   record that INCREASES after an update to that record (read once,
   update, read again, confirm `versions[i]` strictly increased).
2. **Unit** — `expected_version` matching the current version succeeds
   and the write is applied; a stale `expected_version` is rejected with
   `version_conflict` and the row is UNCHANGED (verify by reading it back
   before and after the rejected attempt).
3. **Negative** — `expected_version` against a non-existent row (0 rows
   matched by `WHERE`) — decide and test the exact behavior (either a
   distinct "not found" outcome or a no-op with `affected: 0`, whichever
   is more consistent with how this op already behaves when `WHERE`
   matches nothing today — check and match that existing convention,
   don't invent new zero-match semantics here).
4. **CONCURRENT CAS test (MANDATORY, this is the core value of the
   feature)** — two concurrent tasks/threads/tx's: both read the same
   row's version, both attempt an update with that SAME `expected_version`
   concurrently; exactly ONE must succeed, the other must get
   `version_conflict` (not both succeeding, not both failing) — and
   confirm a RETRY (re-read the now-current version, retry the update
   with the fresh `expected_version`) succeeds. This is the textbook
   optimistic-concurrency-control proof; do not skip it or fake it with
   sequential (non-concurrent) calls — use real concurrent tokio tasks (or
   whatever concurrency primitive the existing SSI/MVCC test suite already
   uses — check `crates/shamir-tx/src/tests/` or
   `crates/shamir-engine/src/tx/tests/` for the established concurrent-tx
   test harness pattern and mirror it).
5. **Rust e2e** (through the real server) — mirror the shape of test 4
   but through `shamir_client::Client` against a real
   `crates/shamir-server/tests/` harness (check `tests/common/mod.rs` for
   the spawn helper convention established in this campaign's RI-9/RI-11
   work).
6. **TS e2e** — mirror test 5 through the TS SDK (follow the
   `describe.skipIf(!SERVER_AVAILABLE)` convention from `e2e-harness.ts`,
   established in RI-5).
7. Confirm the read-side `versions` array is genuinely `None`/absent on
   the wire when `with_version` is NOT requested (no accidental always-on
   cost/leak).

## Docs

- `docs/guide-docs/client-server-protocol-spec/` — document the new
  `versions` result field, the `expected_version` request field, and the
  `version_conflict` error code (find the existing protocol-spec doc
  covering `ReadQuery`/`UpdateOp`/`DeleteOp` wire shapes, or the file this
  campaign's earlier `NUMERIC_WIRE_SEMANTICS.md` addition sits alongside,
  and add a section there).
- `CHANGELOG.md`: one `[Unreleased]` bullet — this is a genuinely NEW
  capability (not a behavior change to existing callers, since
  `with_version`/`expected_version` are both opt-in and `None`/`false` by
  default), but still worth recording.

## Out of scope

- Do NOT build a distinct "not found vs version mismatch" error taxonomy
  beyond what's needed — reuse whatever this codebase's existing
  zero-match convention already is for update/delete (see test 3).
- Do NOT hand-roll a second commit-time revalidation mechanism — reuse
  `TxContext::record_read`/`validate_read_set` (the existing SSI contour)
  exactly as described above.
- Do NOT implement per-row partial-success semantics for a multi-row CAS
  mismatch — whole-operation abort only (see write-side semantics above).
- `AsOf`/point-in-time historical version queries are a SEPARATE, already
  distinct temporal-read feature (`Temporal::AsOf`) — this task is about
  the CURRENT row's version, not historical version browsing. Do not
  conflate the two or expand scope into `AsOf`.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh @oracle --full` green (this repo's scope alias for
  the tx+engine "Version Oracle" area — matches the task's own gate).
- `./scripts/test.sh -p shamir-query-types --full` green.
- `./scripts/test.sh @server --full` green.
- TS tests (`npm test` in `crates/shamir-client-ts`) pass (unit-level; TS
  e2e tests self-skip cleanly if no server binary is available — that's
  fine, note which mode you actually ran in).
- `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets
  -- -D warnings` clean.
- Report literal command output for all of the above, an exhaustive list
  of every `QueryRecord::Direct` push site you found and how each was
  handled (version threaded, or explicitly noted as an exception), and
  the concurrent-CAS test's actual pass/fail result (not just "it
  compiles").

If, after real investigation, some sub-part of this proves structurally
harder than described here (mirroring FG-1's honest "STOP and report"
outcome for its own mandatory verification) — do not force a broken or
silently-incomplete implementation. Report the exact structural blocker
precisely in your summary so it can be triaged, rather than papering over
a gap.
