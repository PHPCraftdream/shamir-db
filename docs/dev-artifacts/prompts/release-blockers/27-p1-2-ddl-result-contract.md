# Brief — P1-2 (#1015): DDL result contract (operation id + typed status)

## Context

S.H.A.M.I.R. Database. A full RFC for this already exists and is
**approved as the design of record** — read it in full before writing any
code: `docs/dev-artifacts/research/2026-08-05-ddl-result-contract-rfc.md`.
This brief only restates the RFC's own §4 "Recommended first-implementation
slice" as the scope for this pass — do not re-derive the design, the RFC
already did that analysis (file+line grounded). Follow it precisely; if you
find the RFC's file/line anchors have drifted (code moved since 2026-08-05),
adapt mechanically to the equivalent current location and note the drift in
your final report, don't redesign around it.

## Problem being solved (one paragraph, see RFC §0 for full reasoning)

DDL operations (CREATE/DROP/RENAME INDEX today) have no per-operation
identity and no per-operation status on the wire. A DDL op either succeeds
inline or aborts the whole batch — and worse, if the server crashes
mid-operation, the existing crash-recovery tombstones (#997/#988/#972 work)
can silently finish the operation on the *next* restart with no way for
the original client (or a new client instance) to ever learn it completed.
This RFC adds: an `op_id` on every DDL `QueryResult`, a durable
`DdlOpState` status log keyed by that `op_id`, a poll endpoint to query it,
and wiring so crash-recovery paths write `SucceededViaCrashRecovery` to
that log using the SAME `op_id` that was in the original tombstone.

## Scope for THIS pass — exactly the RFC's §4 "First PR scope (in)"

1. **New types in `shamir-query-types`** (RFC §3.1): `DdlOpStatus` struct,
   `DdlOpState` enum (`InProgress` / `Succeeded` / `SucceededViaCrashRecovery
   { completed_at_restart, .. }` / `Failed { detail }` / `Unknown` — see RFC
   §2.4's table for exact semantics and who writes each state).
2. **`QueryResult::op_id: Option<RecordId>`** and
   **`QueryResult::ddl_status: Option<DdlOpState>`** — both additive,
   `#[serde(default, skip_serializing_if = ...)]`, mirroring the existing
   `interner_delta` field precedent the RFC points at
   (`crates/shamir-query-types/src/batch/batch_response.rs:57-64`) and
   `query_result.rs:151-177`. **Backward compatibility is load-bearing here**
   — verify old-shape msgpack still decodes (there may already be a
   round-trip test pattern for additive fields in this crate; find and
   extend it, don't invent a new backcompat-test style).
3. **New durable op-status log** (RFC §3.4): keyed `system:ddl_op:<op_id>`,
   living in the same `info_store` the tombstones already use — no new
   storage substrate. Ship with a generous fixed-cap + FIFO eviction of
   terminal (`Succeeded`/`Failed`/`SucceededViaCrashRecovery`) records (RFC
   explicitly defers tuning this — don't over-engineer retention policy).
4. **`op_id` stamped into `QueryResult`** via the single most central place
   the RFC identifies: `helpers::admin_result(...)` in
   `crates/shamir-db/src/shamir_db/execute/helpers.rs` (RFC §3.2) — minimize
   per-handler churn, this is deliberately the one function to extend.
5. **`DbRequest::GetDdlOpStatus { op_id }`** / **`DbResponse::DdlOpStatus {
   status: Option<DdlOpStatus> }`** new wire enum arms
   (`crates/shamir-query-types/src/wire/db_message.rs`, RFC §3.1). RFC §6 Q2
   flags an open question (new enum arm vs. version-gate) — **resolve it as:
   accept the new arm, rely on the existing `not_supported` error code
   fallback for old servers** (RFC §3.6's option (i)) — do NOT attempt a
   `CURRENT_QUERY_LANG_VERSION` bump in this pass, that's a bigger,
   separately-reviewable change or matter for the operator (task #1021 is
   already tracking a pending version bump decision — don't couple this to
   it).
6. **Tombstone `op_id` carry + recovery status-write for exactly the three
   families the RFC scopes in** (RFC §3.3, §4 point 1):
   - hash `DROP INDEX` (regular + unique) — tombstone sites in
     `crates/shamir-index/src/base_index/index_manager.rs` (`idx_drop`/
     `uidx_drop` set tombstones, `recover_in_progress_drops`).
   - hash `RENAME INDEX` (regular + unique, including the SEVERE case RFC
     §2.3 traces in detail — read that worked example, it's the reference
     implementation for how recovery must write the status record) —
     `crates/shamir-engine/src/table/table_manager_index_mgmt.rs`
     (`recover_hash_renames`, the `rename_index` regular/unique branches).
   - index2 `DROP INDEX` — `drop_index2` / `recover_index2_drops` in
     `table_manager_index_mgmt.rs`.
   Each tombstone payload gains an `op_id` field; each of the three
   recovery functions writes `SucceededViaCrashRecovery` to the op-status
   log using that carried `op_id` at the exact point it clears the
   tombstone (RFC §2.2 numbered steps 1-4 are the precise sequencing).
   **Sorted-family DROP/RENAME is explicitly OUT of scope for this pass**
   (RFC §4 "defer to follow-ups") — do not touch
   `sorted_index_manager.rs`'s tombstones.
   **`CREATE INDEX` status is explicitly OUT of scope** (RFC §4 defers it —
   ownership split with #966 self-heal, ROC issue).
   **All non-index DDL (db/repo/table/schema/function/validator/user/
   access) is explicitly OUT of scope** — they get `op_id` stamped (via
   `admin_result`, item 4 above, which is central so this may fall out for
   free) but no `SucceededViaCrashRecovery` wiring, since they have no
   tombstone recovery today.
7. **Retire the #967 enriched-error-TEXT sites for the three in-scope
   families** — RFC §5 and §3.3's line list (as of RFC authorship;
   re-verify current line numbers): the free-text `DbError::Internal(...)`
   sites in `table_manager_index_mgmt.rs` for hash DROP/RENAME and index2
   DROP become structured `DdlOpState::Failed { detail }` records under the
   op_id instead. (Leave `table_manager_sorted_index.rs`'s enriched-error
   sites untouched — sorted family is out of scope.)
8. **Both SDKs, minimal surface** (RFC §3.5):
   - Rust: `Client::get_ddl_op_status(op_id) -> Result<Option<DdlOpStatus>,
     ClientError>` in `crates/shamir-client/src/client.rs`. No `Batch`
     builder change needed (op_id is server-minted).
   - TypeScript: `client.getDdlOpStatus(op_id): Promise<DdlOpStatus | null>`
     in `crates/shamir-client-ts/src/core/client.ts`, plus `QueryResult`
     interface gains `op_id?: string` / `ddl_status?: DdlOpState` in
     `crates/shamir-client-ts/src/core/types/batch.ts`. No DDL builder
     change needed.

## Explicit non-goals (do not implement, do not "helpfully" extend)

- No `CURRENT_QUERY_LANG_VERSION` bump.
- No sorted-index-family wiring.
- No `CREATE INDEX` Building-state wiring.
- No non-index DDL (db/repo/table/schema/function/validator/user/access)
  recovery wiring.
- No op-status-log retention/GC tuning beyond a simple fixed-cap FIFO.
- No client-supplied op_id (server-minted only, per RFC §6 Q1's
  recommendation).

If mid-implementation you find the in-scope slice is still too large to
land as one coherent, well-tested change, **stop and report the natural
split point** (e.g. "wire types + op-status log + admin_result stamping"
as sub-slice A, "the three tombstone-family wirings" as sub-slice B) rather
than shipping something half-wired across all three families. This is a
legitimate outcome for a task this size — say so explicitly in your final
report if it happens, don't silently under-deliver one of the three
families while claiming full completion.

## Constraints

- Follow `CLAUDE.md`: no inline `#[cfg(test)] mod tests {}` (tests live in
  `tests/` sibling directories, `tests/mod.rs` is a manifest only); one
  file = one primary export; imports at top of file; `Result<T, E>` +
  `thiserror` for library error enums, no `panic!` outside invariant
  violations.
- This changes wire-protocol types read/written by BOTH Rust and
  TypeScript clients — after the Rust side lands, verify TS-side
  compilation/build too if the repo's TS package has its own build/test
  step reachable from your sandbox (check `crates/shamir-client-ts/
  package.json` scripts); if TS tooling isn't reachable from your
  environment, say so explicitly rather than silently skipping it.
- Add tests proving: (a) the wire round-trip for `op_id`/`ddl_status`
  decodes correctly including backward-compat with an old-shape payload
  missing these fields; (b) a synchronous DROP/RENAME INDEX (all 3
  in-scope families) returns a `QueryResult` with `op_id` set and
  `ddl_status: Succeeded`; (c) `GetDdlOpStatus` poll returns the right
  state for a completed op and `Unknown` for a nonexistent one; (d) the
  crash-recovery path — reuse/extend whatever existing test harness
  already simulates the tombstone-crash-then-restart scenario for these
  three families (the RFC's §2.3 worked example names `maybe_pause_rename_mid()`
  as an existing test seam — search for and reuse it, do not build a new
  crash-simulation mechanism from scratch) — proves recovery writes
  `SucceededViaCrashRecovery` under the SAME op_id the tombstone carried.
- Gate: `cargo fmt --all -- --check` (scope to touched crates if a
  workspace-wide check is too slow, but report which you ran),
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `./scripts/test.sh @oracle @types --full` (the "Version Oracle" +
  types scopes cover shamir-tx/shamir-engine/shamir-types/
  shamir-collections — the areas most affected) plus
  `./scripts/test.sh -p shamir-query-types -p shamir-db -p shamir-client
  --full`. **Run the real wrapper, never raw `cargo test`/`cargo nextest
  run` directly** (this repo blocks raw `cargo test` outright and mandates
  `./scripts/test.sh`) — a prior delegation this session shipped a
  self-report of "tests pass" that turned out to have run `--lib` against
  an integration-test file and matched zero tests; do not repeat that
  mistake. Show the actual pass/fail summary line in your final report.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.
⛔ Do not create scratch files at the repo root (a prior delegation this
session left `test_output.txt` / `full_test_output.txt` /
`test_doctor_manual.sh` there — clean up after yourself, or better, don't
create them in the first place).

## Definition of done

- [ ] `DdlOpStatus`/`DdlOpState` types added to `shamir-query-types`.
- [ ] `QueryResult::op_id` / `QueryResult::ddl_status` additive fields,
      backward-compat verified by a test.
- [ ] Durable op-status log (fixed-cap FIFO) landed in the shared
      `info_store`.
- [ ] `admin_result` (or the RFC's identified central point) stamps
      `op_id` into DDL `QueryResult`s.
- [ ] `DbRequest::GetDdlOpStatus` / `DbResponse::DdlOpStatus` wired
      end-to-end (server handler + both SDKs).
- [ ] Tombstone `op_id` carry + recovery status-write for hash DROP INDEX
      (regular+unique), hash RENAME INDEX (regular+unique, incl. SEVERE
      case), and index2 DROP INDEX — exactly these three, nothing more.
- [ ] The #967 enriched-error-TEXT sites for these three families replaced
      with structured `Failed { detail }`.
- [ ] `Client::get_ddl_op_status` (Rust) + `client.getDdlOpStatus` (TS)
      added.
- [ ] Tests per the Constraints section (a)-(d), including a real
      crash-recovery-writes-SucceededViaCrashRecovery proof.
- [ ] fmt/clippy/test gates green, real command output reported (not
      paraphrased, not "all green" without the actual summary line).
- [ ] Any scope-split or escalation clearly called out in the final
      report, not silently absorbed.
