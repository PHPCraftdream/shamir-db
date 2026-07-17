# Security posture audit — 2026-07-17 release pass

Fresh security pass over the whole S.H.A.M.I.R. workspace, on top of the
prior audit rounds (2026-07-06, 2026-07-10, 2026-07-11). Prior reports read
first (`docs/dev-artifacts/audits/`, `docs/dev-artifacts/design/`); this
document focuses on **NEW** findings and on **re-confirming the current
status** of previously-flagged items against the actual code, not on
re-reporting already-closed issues.

Scope emphasis per the brief: this-session churn that has had no security
review yet — the `ForEach` DoS gates (#666), `ExecutionDeadline`
(`execution_deadline.rs`), the `MvccStore` `entry_sync`/`retain_sync`
deadlock fix, and the #661 nested-tx threading in `query_runner.rs`.

---

## Executive summary

The single most important result of this pass is a **NEW CRITICAL
broken-access-control finding introduced by this development cycle's OQL
`ForEach` feature (Epic04/B, #653)**: a top-level `BatchOp::ForEach` is not
classified as an admin op and its `required_access()` returns `None`, so
**no per-table authorization is ever performed on the ops inside a
`ForEach` body**. Any authenticated non-superuser can read, insert, update,
or delete rows in *any* table of a database they can open, simply by
wrapping the operation in a `ForEach` loop. `BatchOp::Batch` is protected
from this only because it is (bluntly) marked `is_admin() == true` and thus
superuser-only over the wire; `ForEach` was never given the equivalent
protection, and — worse — a `Batch` nested *inside* a `ForEach` body also
escapes the "Batch is superuser-only" coarse gate.

Everything else the brief asked me to re-check is in good shape. The
previously-flagged HIGH/CRITICAL items from the 2026-07-10/11 permission
audits have genuinely landed their fixes: `resource_meta` is now
fail-closed, `effective_fn_actor` no longer escalates a missing-owner
record to System, the HMAC "did-you-mean-it" gate covers the full
grant/chmod/chown/create/retention/group surface, per-function `net_grants`
egress scoping exists, the WASM structural sanitizer is implemented, and
the identity model binds to the directory-minted `user_id` rather than a
username hash. Bootstrap secrets are redacted and zeroized. The #666
DoS gates and the cooperative `ExecutionDeadline` are sound and close the
runaway-loop threat safely (no `commit_tx` cancellation hazard). The
`entry_sync` MVCC change is a genuine liveness fix with no new
security-adjacent DoS surface.

---

## Findings (severity-ranked)

| # | Severity | Finding | Status |
|---|----------|---------|--------|
| F1 | **CRITICAL** | `ForEach` body ops bypass all per-table authorization — any authenticated user can read/write/delete any table via a top-level `ForEach` | **NEW / OPEN** |
| F2 | Medium | Untrusted-Rust native `cargo build` (build-scripts/proc-macros run as native host processes, no seccomp/rlimit/container) reachable by **any authenticated user** under the default `WasmCompiler` mode `0o755` (Other-Execute set) | Known/accepted decision (#607); default mode permissiveness re-flagged |
| F3 | Low | Dynamic-`over` `ForEach` (iteration count from a `$query`) is not bounded by the plan-time `virtual_units`/`max_queries` check; the only backstops are `ABSOLUTE_MAX_FOR_EACH_ITERATIONS` (100k) + the wall-clock deadline | NEW / by-design, worth documenting |
| — | (clean) | `resource_meta` fail-closed (A1) | Re-confirmed CLOSED |
| — | (clean) | `effective_fn_actor` missing-owner → caller (A2) | Re-confirmed CLOSED |
| — | (clean) | HMAC coverage across admin/batch ops (A3) | Re-confirmed CLOSED |
| — | (clean) | Bootstrap secrets Debug/Clone/zeroize | Re-confirmed CLOSED |
| — | (clean) | Per-function `net_grants` egress scoping (A5) | Re-confirmed CLOSED (#609) |
| — | (clean) | WASM structural sanitizer + SSRF/DNS-rebind guard (A6) | Re-confirmed CLOSED |
| — | (clean) | Identity `Actor` bound to minted `user_id`, not `fxhash(username)` (B1) | Re-confirmed CLOSED |
| — | (clean) | #666 DoS gates + `ExecutionDeadline` cancel-safety | Re-confirmed SOUND |
| — | (clean) | `MvccStore` `entry_sync`/`retain_sync` deadlock fix | Re-confirmed SOUND |
| — | (clean) | #661 nested-tx threading in `query_runner.rs` | Re-confirmed SOUND (no isolation regression) |

---

## F1 — CRITICAL: `ForEach` body ops bypass per-table authorization

### Where

- `crates/shamir-query-types/src/batch/batch_op.rs:468-558` — `BatchOp::required_access()` returns `None` for both `BatchOp::Batch(_)` and `BatchOp::ForEach(_)` (they fall into the `... | BatchOp::Batch(_) | BatchOp::ForEach(_) | ... => return None` arm at line 534-535/547).
- `crates/shamir-query-types/src/batch/batch_op.rs:577-646` — `BatchOp::is_admin()` includes `BatchOp::Batch(_)` (line 634) but **does NOT include `BatchOp::ForEach(_)`**.
- `crates/shamir-db/src/shamir_db/execute/db_execute.rs:57-63` — the ONLY enforcing per-op authorization loop iterates `request.queries.values()` (top-level entries only) and calls `entry.op.required_access(db_name)`; it does **not** recurse into nested `Batch`/`ForEach` bodies.
- `crates/shamir-db/src/shamir_db/execute/db_tx.rs:144-162` — the interactive-tx path's per-op loop is identical (top-level only, `required_access`-driven).
- `crates/shamir-server/src/db_handler/handler.rs:416-425` — the coarse wire "admin ⇒ superuser" gate rejects only `entry.op.is_admin() && !is_coarse_admin_gate_exempt(&entry.op)`; top-level only.
- `crates/shamir-engine/src/query/batch/query_runner.rs:480-686` — the `ForEach` executor arm recurses into the body via `run_nested_body_in_outer_tx` (tx) / `execute_batch_impl` (non-tx). Each body data op reaches `QueryRunner::run`'s `Insert`/`Update`/`Delete`/`Read`/`Set` arms (lines 807-1267), which call only `trace_access(...)` — an R2 observability trace that **always returns `Ok`** (see `access.rs::trace_access` and the runner's own doc comment at lines 309-317). No `authorize_access` is called anywhere in the engine layer.

### Failure scenario

1. A regular (non-superuser) client authenticates. It holds `Read` on a
   database `app` (typical — the connect DB is readable) but has **no
   rights** on table `app/secret/creds` (owner=victim, mode `0o700`).
2. It submits a batch with a single top-level entry:
   `ForEach { over: [1], bind_row: "x", batch: { del: Delete(app/secret/creds) } }`
   (any literal or `$param` one-element `over` works; the body can be
   `Insert`/`Update`/`Delete`/`Read` against any table).
3. Wire gates: `ForEach.is_admin() == false` → the coarse admin/superuser
   gate at `handler.rs:418` passes. `check_destructive_hmacs`
   (`admin.rs:634`) has no `ForEach` arm and never walks bodies → passes.
   Read-only-replica gate only fires on a follower node.
4. `execute_as` (`db_execute.rs:33-41`) authorizes only
   `Database(app):Read` (the container), which the user holds. The per-op
   loop (line 57) calls `ForEach.required_access("app")` → `None` → **no
   target-table check**.
5. `execute_batch` → `QueryRunner::run`'s `ForEach` arm → recurses into the
   body → the `Delete` executes against `app/secret/creds` via
   `run_implicit_batch_tx(self.actor.clone(), ...)` with only a `trace_access`
   (always `Ok`). The row is deleted with **zero per-table authorization**.

The same shape works for `Insert`/`Update`/`Read` to exfiltrate or corrupt
any table. Because a `Batch` may be nested *inside* a `ForEach` body, even
the "`Batch` is superuser-only" coarse protection is bypassable:
`ForEach { over:[1], batch: { b: Batch { ...forbidden ops... } } }`.

### Why the ADR's "template authorization" claim doesn't hold

`docs/dev-artifacts/design/oql-04-loops-foreach-adr.md:278-324` (Decision 5)
asserts the `ForEach` body is "authorized as a template … in
`execute_batch_impl` BEFORE `execute_plan_impl`". That is incorrect:
`execute_batch_impl` is the **engine** layer and never calls
`ShamirDb::authorize_access`; the real gate is the top-level-only
`required_access` loop in the `shamir-db` layer. The ADR conflates the two
layers. `Batch`'s de-facto protection is not "template authorization" — it
is the blunt `is_admin()==true` wire block, which `ForEach` never
replicated.

### Suggested remediation direction (not implemented — investigation-first)

The cleanest fix mirrors the compiler-enforced discipline the code already
values: make `required_access` (and the top-level authz loops in
`db_execute.rs` / `db_tx.rs`) **recurse into `Batch`/`ForEach` bodies** so
every nested data op's `(Action, ResourcePath)` is authorized against the
real actor before execution (the "template authorization" the ADR
describes but never actually wired). Alternatively, as a stop-gap matching
the existing `Batch` treatment, add `BatchOp::ForEach(_)` to `is_admin()` —
but that only restricts to superusers wholesale (regressing the feature for
regular users) and does **not** fix the `Batch`-nested-in-`ForEach` escape,
so the recursive-authz approach is the correct one. This is a release
blocker.

---

## F2 — MEDIUM: native untrusted-Rust compile reachable by any authenticated user under default mode

### Where

- `crates/shamir-wasm-host/src/compile.rs:8-47` — documents that guest Rust
  source "runs arbitrary native processes (build scripts, proc-macros, …)
  with full filesystem and environment access" and that "Full seccomp/rlimit
  isolation is out of scope". Mitigations are: forbidden-macro scan
  (`include!`/`env!`/…), env allowlist (`env_clear` + a small allowlist),
  wall-clock timeout, and the post-compile WASM sanitizer.
- The gating layer is a POSIX `Action::Execute` check on the
  `ResourcePath::WasmCompiler` singleton, applied in `shamir-db`'s
  `create_function_with_opts_as` (`FunctionSource::Source` branch only).
- `crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:166-179` — the
  `WasmCompiler` `resource_meta` default (absent settings key) is
  `owner: System, mode: 0o755`.

### Assessment

`0o755` grants **Other = r-x**, i.e. the Execute bit is set for every
authenticated non-owner actor. So by default, *any authenticated user* can
trigger a host-side `cargo build` of Rust source they supply, which runs
build-scripts/proc-macros as native host processes outside any OS sandbox.
The forbidden-macro scan + host-generated `Cargo.toml` (no guest-declared
dependencies — `compile.rs:57-60`) meaningfully narrow the reachable
compile-time execution channels, and this is an explicitly **documented,
user-signed-off decision** (`#607`, and
`docs/dev-artifacts/design/wasm-untrusted-compile-sanitization-decision.md`)
where OS sandboxing was deliberately deferred in favor of a POSIX gate.

This is therefore *not a new hole* — but the permissiveness of the **default
mode** deserves an explicit operational note: shipping with
`WasmCompiler = 0o755` means the native-compile surface is open to every
authenticated principal unless the operator `chmod`s it down. If the intent
was "only privileged actors compile Rust source", the default should
arguably be `0o750` (drop Other-Execute), matching the deny-by-default
posture Root itself adopted (`0o751`, task #620). At minimum this should be
called out in the deployment/security docs. Re-flagged as Medium because
the gate is intended to be *the* mitigation yet its default admits everyone.

---

## F3 — LOW: dynamic-`over` `ForEach` iteration count not bounded at plan time

### Where

- `crates/shamir-query-types/src/batch/planner.rs:138-156` — the
  `virtual_units = iterations × body_len` check against `max_queries` only
  applies when `over` is a **literal array** whose length is known at plan
  time.
- `crates/shamir-engine/src/query/batch/query_runner.rs:34-44,565-575` —
  `ABSOLUTE_MAX_FOR_EACH_ITERATIONS = 100_000` and `effective_max_iterations`
  clamp the client-supplied `max_iterations` to ≤ 100k, checked at runtime
  before iteration 0.

### Assessment

For a `ForEach` whose `over` is a `$query`/`$param` (dynamic), the planner
cannot see the cardinality, so `virtual_units` never constrains it. The
runtime backstops are the 100k absolute iteration ceiling and the
cooperative wall-clock `ExecutionDeadline` (server-capped default 60s, per
`config.rs:302-304` and clamped in `handler.rs:402-405`). That means a
dynamic `ForEach` over a large query result can spin up to 100k iterations,
bounded ultimately by the 60s wall-clock budget. This is the intended
design (ADR Decision 5's pessimistic model + #666's deadline as the real
backstop), and the deadline check runs once per iteration
(`query_runner.rs:586`) so the loop stops promptly at budget. Noted as Low
because the plan-time budget is silently inapplicable to the dynamic case —
the wall-clock deadline is doing all the work, and an operator who disables
or greatly raises `max_execution_time_secs` loses the effective bound
(iteration ceiling 100k × body-op cost remains, but could still be large).
No action required beyond documenting that the deadline is the primary
backstop for dynamic loops.

---

## Re-confirmed CLEAN (verified against current code)

### resource_meta fail-closed (prior A1 / model-core F1) — CLOSED

`crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:41-239`: every
catalogue-backed arm now returns `DbResult<ResourceMeta>` and splits
`Ok(None)` (genuinely absent → documented `open()`/computed default) from
`Err(_)` (real read failure → propagated `Err`). `authorize_access`
(lines 848-908) maps a `resource_meta` `Err` on either an ancestor
(line 852-867) or the target (line 882-896) to an unconditional
`AccessError` deny — the fail-open collapse the prior audit flagged is gone.
The only intentional `Ok(None) → open()` fallbacks are `FunctionFolder`
(implicit never-created folders, #118, line 112) and nonexistent `Group`
(line 219/228), both documented.

### effective_fn_actor missing-owner (prior A2 / model-core F3) — CLOSED

`access_control.rs:990-1017`: a wholly-missing/erroring record returns the
caller (`let Ok(Some(rec)) = ... else return caller.clone()`), and the
escalation owner is read via `ResourceMeta::owner_field(&rec).unwrap_or_else(|| caller.clone())`
— an **absent** owner field resolves to the caller, never to
`from_record`'s `System` default. `Definer` and setuid-`Invoker` both route
through this `escalated_owner()` closure, so neither can escalate a
missing-owner record to System.

### HMAC coverage (prior A3 / admin-ddl §7) — CLOSED

`crates/shamir-server/src/db_handler/admin.rs:634-780`
(`check_destructive_hmacs`) now covers Drop{Db,Repo,Table,Index,User},
Start/Commit/RollbackMigration, **GrantRole/RevokeRole, Chmod/Chown/Chgrp,
CreateUser, SetRetention/PurgeHistory, CreateGroup/DropGroup/RenameGroup/
Add|RemoveGroupMember**, and a conditional CreateFunction (required iff
`security=="definer"` or non-empty `secret_grants`). The top-level
`CreateScramUser`/`SetSuperuser`/`SetReplicator` `DbRequest`s carry their own
inline HMAC gates (`admin.rs:98-404`). Caveat inherited from F1: this gate,
like the authz loop, walks only top-level `batch.queries` and does not
recurse into `Batch`/`ForEach` bodies — but since destructive DDL ops are
themselves `is_admin()` (superuser-only) or, for `Chmod`/`Chown`, would
still be unauthorized-by-F1 rather than HMAC-bypassed, the practical HMAC
exposure is subsumed by F1's fix.

### Bootstrap secrets — CLOSED

`crates/shamir-connect/src/server/bootstrap.rs`: `BootstrapState`'s custom
`Debug` (lines 40-59) redacts `bootstrap_token_hash`; `BootstrapRequest`'s
custom `Debug` (lines 232-243) redacts `token` and `server_key`. `token`
and `server_key` are `Zeroizing<[u8;32]>`; neither `BootstrapState` nor
`BootstrapRequest` derives `Clone`, so no accidental plaintext copies.
`parking_lot::Mutex` here is the sanctioned setup-only (bootstrap)
low-frequency fallback, not a hot path.

### Per-function net_grants (prior A5) — CLOSED

`crates/shamir-db/src/shamir_db/shamir_db/core.rs:698-745`
(`build_net_gateway`): per-function `net_grants` are intersected with the
DB-wide `net_allowlist` (a function can never exceed the DB ceiling), and
per #609 an **empty** `net_grants` now means **no egress** (deny) rather
than inheriting the full DB allowlist. Wire surface added at
`admin_function.rs:63` / `function_ops` `net_grants`.

### WASM sanitizer + SSRF/DNS-rebind (prior A6) — CLOSED / SOUND

`crates/shamir-wasm-host/src/wasm/wasm_sanitizer.rs`: `verify_wasm_module`
does a pre-instantiation `wasmparser` import-section scan against an
explicit `SANCTIONED_HOST_IMPORTS` allowlist (10 `shamir_host` funcs),
rejects the component encoding, and is kept in sync with the linker
registrations by a dedicated cross-check test. `crates/shamir-wasm-host/src/net_gateway.rs`:
the SSRF guard covers scheme allow (http/https only), default-deny
allowlist, private/loopback ranges, **non-canonical IP forms**
(`inet_aton` octal/hex/short, bare decimal/hex u32, IPv4-mapped IPv6), and
a **DNS-rebind TOCTOU fix** via `check_url_allowed_resolved` returning a
`ResolvedPin` so the actual curl connection is pinned to the validated IP.
Thorough; no gaps found.

### Identity model (prior B1) — CLOSED

`crates/shamir-server/src/db_handler/handler.rs:123-130` (`session_actor`):
the enforcement `Actor` is built from `principal64(session.user_id)` — the
directory-minted 16-byte id stamped at login — **not** from a
`fxhash(username)`. A dropped-and-recreated account gets a fresh id even
when reusing the same name, closing the inheritance-on-recreate bug.
Superuser sessions map to `Actor::Admin(id)` (ownership-attributed bypass),
regular sessions to `Actor::User(id)`; `authorize_access` treats
`System | Admin(_)` as bypass (`access_control.rs:839`). The `"superuser"`
role name is reserved and rejected at `create_scram_user`/`update_roles`
(`admin.rs:168-175`), so escalation-by-role-string is closed.

### #666 DoS gates + ExecutionDeadline — SOUND

`crates/shamir-engine/src/query/batch/execution_deadline.rs`: the redesign
replaced the original `tokio::time::timeout`-wrapping (which could drop a
non-cancel-safe `commit_tx` future between WAL-begin and completion, and
leak Level-3 pessimistic locks by skipping the `Err`-arm cleanup) with
**cooperative checkpoints**. `check()` returns an ordinary
`Err(BatchError::ExecutionTimedOut)` consulted only at safe boundaries
(per stage-alias, per `ForEach` iteration, nested-body entry, and — crucially —
immediately BEFORE `commit_tx`, `batch_execute.rs:580-590`), never racing
the commit. Overflow-safe (`checked_add` → `unbounded` fallback for
`u64::MAX` budgets), and `0 → 1s` minimum (a client cannot opt out). The
`ABSOLUTE_MAX_FOR_EACH_ITERATIONS = 100_000` ceiling caps runaway loops
independent of client `max_iterations`. The runaway-loop / accumulated-time
threat model is closed; the only accepted non-goal (a single op stalling
forever inside one `.await`) is explicitly documented and is an I/O-liveness
class, not a gate bypass.

### MvccStore entry_sync/retain_sync — SOUND (liveness fix, no new DoS)

`crates/shamir-tx/src/mvcc_store/mod.rs:476-521,270`: `publish_cell` /
`swap_vacuum_anchor` / `try_reserve` use `cells.entry_sync` (not
`entry_async`) so the exclusive scc bucket lock is only ever held by a
**running** thread for a few instructions, never across an `.await`
suspension — this fixes the runtime deadlock where every worker parked in
`read_sync` on a hot-key bucket while the lock-owner sat un-polled in the
run queue (#589). The critical sections are O(1) single-cell updates
(monotonic version bump / anchor swap / reservation claim) with no
attacker-controlled iteration inside the lock, so no new synchronous-hold
DoS is introduced. `retain_sync` on `pending_ts` is a bounded sweep over a
small map. Security-adjacent verdict: strictly an improvement (removes a
liveness/DoS bug).

### #661 nested-tx threading — SOUND (no isolation/privilege regression)

`crates/shamir-engine/src/query/batch/query_runner.rs:164-234,404-450,620-666`:
`run_nested_body_in_outer_tx` threads the *same* `&mut TxContext` into
nested `Batch`/`ForEach` bodies so their writes participate in the outer
transaction (correct abort-on-failure via the existing RAII rollback in
`execute_transactional_impl`, `batch_execute.rs:592-617`). From a security
standpoint: the nested body executes under the **same actor** as the outer
tx (`&self.actor` is passed through, `query_runner.rs:409/625`), the
`nested_tx_not_supported` guard (lines 331-337/484-490) still forbids a
transactional sub-body inside an open tx, and the shared `TxContext` carries
no additional privilege — it is the same isolation boundary, not a widened
one. The change is correctness-only; it does not create a
privilege-escalation or isolation-boundary concern. (Note: it does **not**
address F1 — the authz gap predates and is orthogonal to tx threading; both
the tx and non-tx nested paths execute body ops without per-table authz.)

---

## Not re-investigated this pass (out of time-box / lower priority)

- Full replication-path authorization (`apply_replicated` on followers) —
  prior audits ruled this by-design safe (authz on leader; follower
  trusted). Not re-verified here.
- The LOW hardening cluster A7 (unified authz registry / doc-guards /
  create-TOCTOU / coverage matrix) — the `required_access`/`is_write`
  exhaustive-match discipline is in place (`batch_op.rs`), which is the
  core of A7's intent; a full re-audit of the remaining sub-items was not
  done. Note F1 is precisely the failure mode A7's "coverage matrix" test
  (real `execute_as` under a no-rights `Actor::User` for every object×op,
  **including a `ForEach`-wrapped op**) would have caught — that test
  should be added as part of F1's fix.
