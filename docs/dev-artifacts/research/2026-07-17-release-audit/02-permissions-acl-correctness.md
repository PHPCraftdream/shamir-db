# Permission / ACL Enforcement Correctness — Release Audit

_Deep-dive audit, 2026-07-17. Scope: authorization-gate consistency across
every query/batch entry point, the `BatchOp → Action` mapping, `resource_meta`
fail-open/closed posture, embedded-vs-server guarantees, `Actor::System`
construction surfaces, and this session's OQL nesting changes (#653/#661).
Builds on — and does not re-derive — the 2026-07-06 / 2026-07-10 / 2026-07-11
permission audit rounds. Each claim is grounded in code read at the current
`HEAD` (`7a4abf62`), which is well past the `2026-07-17-0815` checkpoint._

---

## Executive summary

The POSIX core and the previously-flagged HIGH fail-open defects are now
genuinely closed at HEAD: **A1** (`resource_meta` `Err` vs `Ok(None)` split)
is fixed for every arm — Database/Store/Table/Function/FunctionFolder/
FunctionNamespace/Root/WasmCompiler all propagate `Err(e)` and only fall
open on genuine `Ok(None)` (`access_control.rs:54-179`); the **#602 Group-path
regression** the 2026-07-14 review found is fixed exactly as its brief
prescribed — `Err(DbError::NotFound(_)) => open()`, `Err(e) => Err(e)`
(`access_control.rs:215-234`). **A2** (`effective_fn_actor` missing-owner →
System escalation) is fixed via `owner_field(...).unwrap_or_else(|| caller)`
(`access_control.rs:990-1017`). Root listing is now default-closed at `0o751`
(#615/#620), and the System-wrapper backdoor surface (A7 G-perm1e / #606) is
`#[doc(hidden)]` with explicit SAFETY comments. The subscriptions bridge still
gates each source with `authorize_access(Table:Read)` (`bridge.rs:148`). The
`BatchOp → (Action, ResourcePath)` mapping was unified into a single
exhaustive `BatchOp::required_access` (A7 G-perm1a), eliminating the duplicated
inline classifiers.

**However, the OQL nesting work landed this session (#653 ForEach, #661
outer-tx threading) opened a NEW, wire-reachable ACL bypass that none of the
prior rounds covered — because `ForEach` did not exist when they ran.** The
authorization architecture is: (1) the server coarse gate rejects top-level
`is_admin()` ops from non-superusers; (2) `execute_as`/`tx_execute_as` run a
per-op `authorize_access` loop driven by `required_access`; (3) the engine's
per-op `trace_access` calls are always-`Ok` observability, NOT enforcement (so
documented, `query_runner.rs:309-317`). Critically, **`required_access` returns
`None` for both `BatchOp::Batch` and `BatchOp::ForEach`** (they carry no
`table_ref()`), so the per-op loop never authorizes the ops _inside_ a nested
body. `Batch` is deliberately kept in `is_admin()` so the coarse gate forces it
to superuser (the mitigation is spelled out at `admin.rs:590-591`). **`ForEach`
is absent from `is_admin()`** — so a non-superuser can submit a top-level
`ForEach`, the coarse gate lets it through, the per-op loop skips it
(`required_access` → `None`), and the engine executes the loop body's
Insert/Update/Delete/Read against tables the caller has NO rights on, with only
the always-`Ok` `trace_access`. This is a full per-table ACL bypass reachable
over the wire (both `execute_as` and `tx_execute_as`) and through the WASM
`db_execute` gateway (which routes to the same `execute_as`). There is no
e2e/coverage test for the `ForEach` case (the analogous `Batch` case IS tested:
`permission_e2e.rs:539 nested_batch_read_req`).

This is a release blocker: HIGH severity, wire-reachable, no privileged
precondition (mere `Database:Read`), and silent (attacker sees data / mutates
tables, victim sees nothing).

---

## Severity-ranked findings

| # | Severity | Status | Area | Where | One-line |
|---|----------|--------|------|-------|----------|
| P1 | **HIGH** | **NEW / OPEN** | Gate coverage | `batch_op.rs:577-645` (is_admin) + `:468-558` (required_access) + `query_runner.rs:476-490` | Top-level `ForEach` bypasses ALL per-table ACL: not in `is_admin` (coarse gate misses it), `required_access`→`None` (per-op loop skips it), engine only `trace_access` (no enforcement). Wire- and WASM-reachable. |
| P2 | **MEDIUM** | **OPEN (latent)** | Gate coverage | `batch_op.rs:468-558`, `query_runner.rs:326-474` | `Batch` is protected ONLY by its `is_admin()==true` coarse-gate membership, NOT by real per-op authz. Any future exemption, or any embedded/`Actor::System`-internal caller that reaches nested bodies without the coarse gate, re-opens the same nested-body hole `ForEach` already has. Fragile by construction. |
| P3 | **LOW** | **OPEN (test gap)** | Coverage | `permission_e2e.rs` (no ForEach case), `coverage_matrix_tests.rs:20-23` | The coverage-matrix + e2e suites test nested `Batch` read-ACL but not nested `ForEach`; the matrix comment asserts WASM `db_execute` "inherits execute_as coverage" — true, but that coverage has the P1 hole, so the assertion is misleading. |
| — | HIGH | **FIXED** | Core model | `access_control.rs:54-179` | A1 `resource_meta` fail-open → fail-closed: every arm now splits `Err(e)` (propagate) from `Ok(None)` (open by design). |
| — | HIGH | **FIXED** | Core / Group | `access_control.rs:215-234` | #602 Group-path regression fixed: `Err(NotFound)→open`, `Err(e)→Err`. |
| — | HIGH | **FIXED** | Core / WASM | `access_control.rs:990-1017` | A2 `effective_fn_actor` missing-owner no longer escalates to System (resolves to caller). |
| — | LOW-MED | **FIXED** | Core / Root | `access_control.rs:149-160` | Root default `0o751` — DB-name enumeration closed to non-owner (#615/#620). |
| — | LOW | **FIXED** | Admin backdoor | `db_management.rs:30`, etc. | System-wrapper convenience fns `#[doc(hidden)]` + SAFETY comment (A7 G-perm1e / #606). |
| — | LOW-MED | **FIXED** | Gate coverage | `batch_op.rs:468-558` | A7 G-perm1a: single exhaustive `required_access`, no wildcard; the two inline classifiers now call it. |
| — | — | CONFIRMED CLOSED | Subscriptions | `bridge.rs:141-159` | Per-source `authorize_access(Table:Read)`; denied sources excluded. |

The B-bucket DECISION items (B1 identity/`principal_id`, B2 superuser axis /
store desync, A3 HMAC breadth, A6 WASM sanitizer) are architectural and remain
tracked separately; this audit does not re-litigate them. Note that
`principal64(session.user_id)` is now derived from the directory-minted 16-byte
`user_id`, not `fxhash(username)` (`handler.rs:123-129`) — the
inheritance-on-recreate half of B1 is closed; the parallel-superuser-axis half
(B2) is a separate decision.

---

## Detailed findings

### P1 — HIGH (NEW, OPEN): top-level `ForEach` bypasses all per-table ACL

**Root cause — three independent gates all miss the loop body:**

1. **Server coarse gate** (`handler.rs:416-425`, mirrored `tx_handlers.rs:107-116`):
   ```rust
   if !session.permissions.is_superuser {
       for (alias, entry) in &batch.queries {
           if entry.op.is_admin() && !is_coarse_admin_gate_exempt(&entry.op) { /* reject */ }
       }
   }
   ```
   This iterates **top-level** ops only. `BatchOp::is_admin()`
   (`batch_op.rs:577-645`) lists `BatchOp::Batch(_)` (line 634) but **does NOT
   list `BatchOp::ForEach(_)`** (verified: 0 occurrences of `ForEach` in the
   `is_admin` match body). So a top-level `ForEach` has `is_admin()==false` and
   sails past the coarse gate for any authenticated non-superuser.

2. **Per-op authorization loop** in `execute_as` (`db_execute.rs:57-63`) and
   `tx_execute_as` (`db_tx.rs:144-162`):
   ```rust
   for entry in request.queries.values() {
       if let Some((action, path)) = entry.op.required_access(db_name) {
           self.authorize_access(&actor, &path, action).await?;
       }
   }
   ```
   `BatchOp::required_access` (`batch_op.rs:468-558`) returns `None` for
   `ForEach` (and `Batch`) — both are in the explicit `... => return None` arm
   (lines 534-535, 547), because `table_ref()` (`:561-574`) returns `None` for
   them. So the loop authorizes NOTHING inside the loop body.

3. **Engine execution** (`query_runner.rs:480-490` for `ForEach`,
   `:326` for `Batch`): the loop body's entries dispatch through
   `QueryRunner::run`, whose data-op arms call `trace_access` — explicitly
   documented as an **always-`Ok` R2 observability trace, NOT the enforcement
   gate** (`query_runner.rs:309-317`, and `trace_access` returns `Ok(())`
   unconditionally, `access.rs:657-659`). The nested recursion
   (`run_nested_body_in_outer_tx` → `execute_plan_tx_impl`, or
   `execute_batch_impl` for the non-tx case) performs no `authorize_access`.

**Concrete failure scenario (wire):** Alice authenticates as a normal
(non-superuser) user. She holds `Database:Read` on `appdb` (enough to pass the
`execute_as` database-level gate at `db_execute.rs:33-41`) but has NO rights on
table `appdb/secrets/salaries`. She submits a batch with a single top-level op:
`for_each` over a one-element literal array whose body contains
`Read(appdb/secrets/salaries)` (or `Delete`/`Update`). The coarse gate: `ForEach`
`is_admin()==false` → pass. The per-op loop: `required_access(ForEach)==None` →
skip. The engine runs the body once and returns every row of `salaries` (or
deletes them). Alice never had read/write on that table. Same story for
`tx_execute_as` (interactive tx path — same `required_access` loop, same hole).

**Concrete failure scenario (WASM):** `FacadeDbGateway::execute`
(`db_gateway.rs:285-290`) routes a guest's msgpack `BatchRequest` through
`self.db.execute_as(self.actor.clone(), ...)`. A `Security::Invoker` function
with a low-privilege effective actor can emit a `ForEach`-wrapped write to a
table its effective actor lacks rights on and bypass the per-table gate — the
same hole, reached through the gateway.

**Why prior rounds missed it:** `ForEach` is a #653 op added this session;
the 2026-07-10/11 rounds predate it. The `Batch` case they DID analyze is
protected by a deliberate, documented mitigation (see P2) that was never
extended to `ForEach`.

**Fix direction (not implemented — audit only):** the correct fix is to make
`required_access` (or a sibling walk) recurse into `Batch`/`ForEach` bodies so
each nested op is authorized in the outer per-op loop, OR add `ForEach` to
`is_admin()` to match `Batch`'s coarse-gate mitigation (weaker: over-restricts
to superuser, and leaves the embedded/System-internal path of P2 open).
Recursion is the principled fix because it also closes P2. A discriminating
e2e test (P3) must accompany it: a non-superuser `ForEach{ Read(secret) }` and
`ForEach{ Delete(secret) }` must both be denied.

---

### P2 — MEDIUM (OPEN, latent): `Batch` protected only by coarse-gate membership

`Batch`'s nested body has the identical no-per-op-authz property as `ForEach`
(`required_access(Batch)==None`, engine does only `trace_access`). It is safe
**today over the wire** solely because `BatchOp::Batch(_) ∈ is_admin()`
(`batch_op.rs:634`) forces the coarse gate to demand superuser, and superuser ==
`Actor::Admin`/`Actor::System` == full bypass. The rationale is documented at
`admin.rs:590-591`:

> "Exempting `Batch` would let `Batch{ Read(forbidden_table) }` execute with
> zero per-table authorization — reopening the bug class task #510 closed for
> `Subscribe`."

This is a **fragile, non-defense-in-depth** arrangement: the real per-table
authz for a sub-batch body does not exist; a single coarse-gate string
(`is_admin` membership) is the only thing standing between a nested body and an
unauthorized table. Two ways it breaks:

- Anyone who later adds `Batch` (or `ForEach`) to `is_coarse_admin_gate_exempt`
  (`admin.rs:598-606`, currently `List`/`AccessTree`/`DescribeTable`/
  `GetTableSchema`) to allow non-superuser sub-batches instantly re-opens the
  P1 hole for `Batch` too. The comment warns against it, but it is a comment,
  not a compiler-enforced invariant.
- Any non-wire caller that reaches a nested body WITHOUT passing through the
  server coarse gate — e.g. a future embedded API, or an internal
  `Actor::System` path that constructs a `Batch`/`ForEach` from
  partially-trusted input — has no per-op ACL on the body at all.

**Recommendation:** the P1 recursion fix subsumes this — once `required_access`
authorizes nested bodies, `Batch` no longer depends on its `is_admin`
membership for safety, and the coarse gate becomes a redundant (defense-in-
depth) layer rather than the sole barrier.

---

### P3 — LOW (OPEN): coverage gap for nested `ForEach`; misleading matrix comment

`permission_e2e.rs` builds and asserts a denied nested-`Batch` read
(`nested_batch_read_req`, `:539`, used at `:1770`) precisely to pin the
`admin.rs:590` mitigation. There is **no equivalent `ForEach` case** (grep for
`for_each` in `permission_e2e.rs` returns nothing), so the P1 regression was
never caught by the suite.

`coverage_matrix_tests.rs:20-23` states the WASM `db_execute` gateway "has no
independent gate ... [it] delegates to `execute_as(self.actor.clone(), ...)`,
so it inherits this same coverage." That is factually correct about the code
path — and precisely why the P1 hole propagates into WASM. The comment reads as
reassurance but actually describes the vector by which the `ForEach` bypass
reaches guest functions. Once P1 is fixed, add a matrix row that drives a
no-rights `Actor::User` through a `ForEach{ write(secret) }` via
`execute_as`/`tx_execute_as` and the WASM gateway, asserting `access_denied`.

---

## Items verified as already-fixed since the last round (with the code that proves it)

- **A1 (`resource_meta` fail-open, HIGH):** `access_control.rs:54-179`. Every
  arm is now `Ok(Some) => from_record`, `Ok(None) => default/open (by design)`,
  `Err(e) => { log::warn!; Err(e) }`. The `.ok().flatten()...unwrap_or_default()`
  collapse the round-3 audit flagged is gone.
- **#602 Group-path regression (HIGH):** `access_control.rs:215-234`.
  `Err(DbError::NotFound(_)) => return Ok(ResourceMeta::open())`,
  `Err(e) => return Err(e)`. Matches the `01-group-metadata-fail-closed.md`
  brief exactly. The sibling `load_group` `Err(e) => Err(e)` arm is also correct.
- **A2 (`effective_fn_actor` → System escalation, HIGH):**
  `access_control.rs:990-1017`. `escalated_owner()` uses
  `owner_field(&rec).unwrap_or_else(|| caller.clone())`; a record with an absent
  `owner` field resolves to the caller, not `Actor::System`. `Definer` and
  setuid-`Invoker` both route through `escalated_owner()`.
- **Root default-closed listing (LOW-MED):** `access_control.rs:149-160`.
  Absent `root_meta` → `owner: System, mode: 0o751` (Other loses Read, keeps
  Execute for ancestor traversal). DB-name enumeration by any authenticated
  user is closed (#615/#620).
- **System-wrapper backdoor (LOW, A7 G-perm1e / #606):** `db_management.rs:30`,
  `function_management.rs`, `table_management.rs`, `validator_management.rs` —
  the `_as`-less convenience wrappers are `#[doc(hidden)]` with a SAFETY
  comment forbidding wire-reachable callers.
- **A7 G-perm1a (duplicated authz mapping):** `batch_op.rs:468-558` —
  `required_access` is a single exhaustive `match` with no wildcard; both
  `execute_as` (`db_execute.rs:58`) and `tx_execute_as` (`db_tx.rs:145`) call
  it. Adding a new table-bearing `BatchOp` now fails to compile until
  classified.
- **#670 (interactive-tx `validate_filter_depth`):** `interactive_tx.rs:95` —
  present. This closed a DoS-guard inconsistency, NOT an ACL one; the ACL
  enforcement for the interactive-tx path lives in `tx_execute_as`'s per-op
  loop (which shares the P1 `ForEach`/`Batch` hole). No OTHER guard is
  inconsistently applied between `execute_in_open_tx` and `execute_batch_impl`
  beyond what #670 already addressed — both now call `BatchPlanner::plan`,
  `validate_tables`, `validate_filter_depth`.
- **Subscriptions bridge (CRIT-5 / #439):** `bridge.rs:141-159` — per-source
  `authorize_access(Table:Read)`, snapshot reads via `execute_as`
  (`bridge.rs:336`, `reactive.rs:83/163`). Confirmed still closed.
- **`Actor::System` construction surfaces:** all `pub` System-constructing
  entry points are either the documented offline/CLI `#[doc(hidden)]` wrappers
  (above) or `_as`-methods that take an explicit actor. `session_actor`
  (`handler.rs:123-130`) is the only wire→actor mapping and correctly yields
  `Actor::Admin(principal64(user_id))` for superusers / `Actor::User(...)`
  otherwise — no wire path constructs a bare `Actor::System`. No new
  privilege-confusion surface found here.

---

## Bottom line

One release-blocking regression (P1) introduced by this session's `ForEach`
work, with a latent-fragility twin (P2) and a test gap (P3) that together
should be fixed as a unit — the principled fix (recurse `required_access` into
`Batch`/`ForEach` bodies) closes all three. Everything the prior rounds flagged
as HIGH is genuinely fixed at HEAD, verified against the current code.
