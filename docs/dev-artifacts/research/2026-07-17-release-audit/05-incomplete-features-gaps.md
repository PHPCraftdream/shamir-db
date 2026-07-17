# Release audit 05 — Incomplete / half-finished features and capability gaps

Date: 2026-07-17. Method: repo-wide marker sweep (`TODO`/`FIXME`/`unimplemented!`/`todo!`/
`not (yet) supported`/`MVP`/`deferred`/`stub`/`placeholder`/`not wired`) over `crates/*/src`
(tests excluded), followed by context reads of every non-trivial hit, spot-checks of
design/roadmap docs against current code, and a client/server capability comparison with
`shamir-client-ts`. Line numbers are as of working tree at commit `d8888158`.

Notable negative results (good news): **zero** `unimplemented!()` / `todo!()` in any
shipped crate; AsOf temporal reads are implemented (`read_temporal.rs`), not stubbed;
per-request durability (`buffered`/`synced`/`async_index`) is wired end-to-end including
the TS builder; interactive transactions, keyset pagination, FTS, vector search, FK
on-delete/on-update, replication follower loops, and DDL rename ops all have real
implementations with e2e TS coverage.

## Executive summary

The codebase is largely honest about its limits — most narrow-scope implementations fail
with a **clear coded error** rather than silently misbehaving. However, five gaps produce
**silent wrong behavior** on paths an ordinary user will plausibly hit, and these should be
triaged before release:

1. **UPSERT MERGE silently overwrites `created_at`** — a documented, known-wrong outcome
   on the normal upsert path (not an edge case).
2. **Schema defaults/transforms on nested (multi-segment) field paths are silently
   ignored** — accepted at DDL time, dropped at write time, no error anywhere.
3. **Function `Call` ops inside transactional batches / interactive txs autocommit
   outside the transaction** — the batch looks atomic but the function's DB writes are
   not covered by it.
4. **Computed defaults are fail-open** — an evaluation error silently skips the stamp.
5. **`$ref`/`$fn`/`$expr`/`$cond` in `Call` params silently collapse to `Null`.**

A second cluster is security/audit attribution: WASM nested calls do not thread the parent
actor, validators run as `Actor::System`, and function **Visibility** metadata is stored
but apparently never enforced (Security DEFINER/INVOKER *is* enforced via
`effective_fn_actor`).

Replication (386-c) is a functional MVP with several deferred pieces that limit real
multi-node deployment: one shared replicator credential for all subscriptions, TOFU with
no persisted leader pin, no DNS resolution for upstream endpoints, and tick-polling
reconcile instead of an event-driven watch.

Finally, several "not wired / stub" comments are **stale** — the code *is* wired
(`WalGroupCommit`, `RecordView`, funclib categories) — pure doc rot, listed at the end so
nobody re-audits them.

## Gap table (ranked: silent-wrong-behavior first, then clear-error capability holes)

| # | Feature area | What's missing / wrong | User-facing impact | Failure mode | Location |
|---|---|---|---|---|---|
| 1 | Upsert + schema transforms | UPSERT MERGE branch applies `AutoNowAdd` with `is_insert=true`; when the caller omits `created_at` on an upsert that MERGES an existing record, the transform stamps a fresh `created_at` into the set-map and **overwrites the original value** | Silently corrupts `created_at` audit data on every upsert-merge of a table with an auto-created-at rule | **Silent wrong data** (self-documented as accepted debt: "Track and fix in a follow-up if needed") | `crates/shamir-engine/src/table/write_exec.rs:863-870` |
| 2 | Schema defaults / transforms | MVP scope: **single-segment field paths only**. Multi-segment paths (`["address","zip"]`) are *silently skipped* in both `apply_defaults` and `apply_transforms` | A user declaring `default` or `auto_now` on a nested field gets nothing — no DDL-time rejection, no write-time error | **Silent no-op** | `crates/shamir-engine/src/table/write_helpers.rs:146-149, 166-172, 200-201, 222-227` |
| 3 | Functions inside transactions | `Call` ops in a batch delegate to `FunctionInvoker` with "(autocommit, no tx)" — even when the batch is `transactional: true` or running inside an interactive tx (the invoker is passed into the interactive path too). The gateway design doc explicitly defers RYOW/SSI integration | A transactional batch containing a `Call` is **not atomic**: the function's DB writes commit independently and survive an outer abort; the function also cannot read the batch's own uncommitted writes | **Silent isolation violation** (no error, no warning) | `crates/shamir-engine/src/query/batch/query_runner.rs:700-710`; `crates/shamir-wasm-host/src/db_gateway.rs:7-32`; `crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:685-687`; `crates/shamir-engine/src/query/batch/interactive_tx.rs:73,102` |
| 4 | Computed defaults | `TransformSpec::ComputedDefault` evaluation errors are swallowed: "on evaluation error the stamp is skipped silently (fail-open)". Also user-registered scalars are NOT available in computed defaults (builtins only) | A typo'd or failing default expression yields records missing the field with no diagnostic; a default referencing a user scalar never works | **Silent no-op** | `crates/shamir-engine/src/table/write_helpers.rs:202-206`; `crates/shamir-engine/src/validator/schema/schema_validator.rs:90-92` |
| 5 | `Call` op params | `$ref`/`$fn`/`$expr`/`$cond` in `Call` positional params **collapse to `Null`** (no record/interner in scope for the record-free resolver); `$query`/literals work | A function invoked from a batch with a dynamic param silently receives `Null` instead of the computed value | **Silent Null substitution** (documented in code, invisible to the user) | `crates/shamir-db/src/shamir_db/execute/helpers.rs:206-220` |
| 6 | Actor attribution (security/audit) | (a) WASM nested `call()` does not thread the parent actor into the child `FnCtx` — `TODO(Shomer R2)`; (b) tx-path validators run with hardcoded `Actor::System` — `TODO actor threading` (two sites) | Nested function calls lose caller identity (privilege attribution + audit trail); actor-sensitive validators see `System` instead of the real principal | **Silent mis-attribution** | `crates/shamir-wasm-host/src/wasm/host_call.rs:93`; `crates/shamir-engine/src/table/write_exec.rs:207, 759` |
| 7 | Function Visibility | `Visibility` (Public/Private) is stored in the catalogue but no enforcement site was found (module doc: "enforcement is deferred to slice 10"). `Security` (Definer/Invoker) IS enforced via `effective_fn_actor`; visibility gating on list/exists appears absent | A user marking a function `private` may believe it is hidden; it is not (only the generic Execute ACL applies) | **Silent non-enforcement** of declared metadata | `crates/shamir-wasm-host/src/meta.rs:1-18, 43`; enforcement present only for Security: `crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:632, 671, 720` |
| 8 | Server boot / listeners | Unsupported (kind, profile) listener combinations are **skipped with a warn** and boot continues (`bound_addrs` gets `None`); plain-TCP (binding 0x00) accept loop is a follow-up — plain profile routes through the TLS path | A misconfigured listener silently doesn't listen; operator discovers at connect time. Loopback plain-TCP debugging documented in config surface but not actually available | **Semi-silent** (warn log only, boot succeeds) | `crates/shamir-server/src/server/server_launcher.rs:496-528`; `crates/shamir-server/src/server/server_handle.rs:31-34` |
| 9 | Engine migration | `Migrate` admin op supports only `dst_engine: "in_memory"` — migration **to fjall (the persistent engine) is not implemented**, which inverts the most useful direction (mem → durable) | Cannot promote an in-memory repo to persistent storage via the migration feature; clear error | Clear error ("not yet supported. Supported: in_memory") | `crates/shamir-db/src/shamir_db/execute/admin_migration.rs:90-98` |
| 10 | DDL rename | Rename of schema-bearing tables is rejected (validator name embeds table path; would orphan it) | Any table with a declarative schema — the recommended setup — cannot be renamed | Clear error | `crates/shamir-db/src/shamir_db/shamir_db/table_management.rs:235-245` |
| 11 | Live subscriptions | (a) Multi-repo subscriptions rejected; (b) subscription filters support only Eq/Ne/Gt/Gte/Lt/Lte/In/NotIn/IsNull/IsNotNull/Exists/NotExists/And/Or/Not — **Like, ILike, Regex, Contains*, Between, FieldEq, Fts, VectorSimilarity, Computed are rejected** | Filters that work in a normal query fail when reused in `subscribe` — an asymmetry users will hit when converting a query into a live view | Clear coded errors (`multi_repo_subscriptions_not_supported`, `subscription_filter_unsupported_operator`) | `crates/shamir-engine/src/query/batch/query_runner.rs:714-746, 1373-1404` |
| 12 | Transactions | (a) Cross-repo transactional batches rejected (2PC intentionally out of scope); (b) transactional sub-batch inside an open tx rejected (`nested_tx_not_supported`); (c) DDL inside an interactive tx out of scope (TS `Tx` handle exposes no DDL wrappers) | Multi-repo atomicity must be composed client-side; documented design decisions | Clear coded errors | `crates/shamir-engine/src/query/batch/interactive_tx.rs:86`; `batch_execute.rs:131`; `query_runner.rs:326-337`; `crates/shamir-client-ts/src/core/db.ts:218-224` |
| 13 | Schema constraints | `unique` accepts only single-segment field paths (multi-segment rejected at DDL time — properly, unlike defaults) | No unique constraints on nested fields | Clear error | `crates/shamir-db/src/shamir_db/execute/admin_schema.rs:130-134` |
| 14 | Replication (386-c) | (a) one shared `replicator` credential for ALL subscriptions (per-subscription creds TODO); (b) TOFU — `accept_new_host: true`, no persisted leader pin (#388); (c) upstream must be a literal `SocketAddr` — **no DNS/hostname resolution**; (d) reconcile is a fixed-interval tick, not an event-driven changefeed watch; (e) no creds configured → active subscriptions are skipped (logged) | Multi-leader topologies, DNS-named upstreams, and pinned-identity leaders are all unusable; a MITM on first connect is accepted by design (TOFU) | Mixed: (c) is a clear transport error; (b) is a **silent security posture**; (e) is warn-log-only | `crates/shamir-server/src/replication/prod_factory.rs:12-15, 113-116, 188-201`; `crates/shamir-server/src/replication/supervisor.rs:28-32, 91-94`; `crates/shamir-server/src/server/server_launcher.rs:690, 1116-1117`; `crates/shamir-server/src/config.rs:117` |
| 15 | Browser WSS channel binding | Browser JS cannot access the TLS exporter, so binding_mode 0x02 uses an all-zero 32-byte placeholder — strictly weaker channel binding accepted by protocol design | Browser connections lack the MITM-binding guarantee native clients get; documented per spec §6.4, decision recorded in `resumption-ticket-channel-binding-512-decision.md` | Documented protocol limitation (not silent — mode byte differs) | `crates/shamir-transport-ws/src/tls_exporter.rs:9-11, 25`; `crates/shamir-transport-ws/src/lib.rs:11` |
| 16 | WASM SDK error envelope | User `Err(...)` from a guest function traps and is reported as `FunctionError::Compute`, not a clean `FunctionError::User` — `TODO(slice 4)` | Function authors cannot distinguish their own domain errors from host/compute failures at the client | Wrong error *category* (message preserved) | `crates/shamir-sdk-macros/src/lib.rs:249-251` |
| 17 | Aggregate fast paths | `MIN(field)` uses the O(log n) sorted-index walk; `MAX` needs reverse iteration ("Opt R — not wired yet, falls through to full scan") | `SELECT max(field)` on an indexed field is O(N) while `min` is O(log n) — perf surprise only, results correct | Silent perf cliff | `crates/shamir-engine/src/table/read_exec.rs:241-246` |
| 18 | Audit details helper | `encode_details_canonical()` ignores its input and returns `Vec::new()` — "placeholder until callers supply real msgpack values". Zero callers found | Dead stub; anyone who adopts it would silently persist empty audit details | Silent (currently unreachable) — recommend deleting or implementing | `crates/shamir-connect/src/server/audit_chain.rs:355-361` |
| 19 | WAL corruption recovery | Single-frame-skip resync on a corrupt mid-segment frame is a deferred follow-up — recovery truncates the tail at the first bad frame | A single corrupted frame discards all later (already-acked-buffered) entries in that segment; conservative but lossier than necessary | Documented recovery-granularity limitation | `crates/shamir-wal/src/wal_segment.rs:554` |
| 20 | Runtime tunables | `RuntimeTunables` is initialized and settable, but "consumer wiring to accept-loop sleep sites is deferred to a follow-up slice" | Some tunables can be set at runtime yet have no effect on the loops they describe | Silent no-op for the unwired knobs | `crates/shamir-server/src/server/server_handle.rs:84-88` |
| 21 | Interner client cache | Server-side `InternerDump`/`InternerTouch` exist; the client-side auto-cache is "deferred to Stage 5" (TS has manual `interner-ops` + `field-map`, no automatic warm/refresh) | Perf-only: clients pay name-keyed writes or manage the id cache by hand | Documented, no correctness impact | `crates/shamir-db/src/shamir_db/execute/admin_interner.rs:1-5` |
| 22 | Read-path row locking | Streaming reads: "locking over a stream is a known harder problem and out of scope here" | Long streams observe MVCC snapshots without lock coordination — by design (MVCC), noted for completeness | Documented design boundary | `crates/shamir-engine/src/table/table_manager_streaming.rs:162` |

## Detail on the top-5 silent gaps

### 1. UPSERT MERGE `created_at` overwrite (`write_exec.rs:863`)
The code comment is explicit that this ships wrong: `apply_transforms` runs with
`is_insert=true` before the key lookup decides INSERT vs MERGE, so `AutoNowAdd`'s
absence-guard checks the *incoming* record, not the *stored* one. Correct fix requires
moving the transform (or re-checking) after the existing-record lookup. Every user with a
`created_at` auto-stamp and an upsert workload will corrupt creation timestamps. This is
the single highest-value pre-release fix in this report: small blast radius, known
solution shape, silent data corruption otherwise.

### 2. Nested-path defaults/transforms silently skipped (`write_helpers.rs`)
The asymmetry with `unique` is the problem: `admin_schema.rs:132` **rejects** nested paths
for unique constraints at DDL time, but nested `default`/`auto_now`/computed-default
rules are accepted at DDL time and dropped at write time. Minimum viable fix without
implementing the recursive walker: reject nested paths in these rules at schema-creation
time with the same clear error `unique` gives.

### 3. `Call` escapes the transaction (`query_runner.rs:700`)
`db_gateway.rs` documents the full plan (thread `TxContext` through `FnCtx`/`HostState`,
re-entrancy guard, RYOW). Until that lands, a cheap honesty fix is to **reject** `Call`
ops inside `transactional: true` batches and interactive txs with a coded error
(`call_in_tx_not_supported`), the same way cross-repo and nested-tx are rejected —
converting a silent isolation violation into a clear error. Note the engine already has
precedent for this exact pattern at `query_runner.rs:331` (`nested_tx_not_supported`).

### 4/5. Fail-open computed defaults and Null-collapsing Call params
Both are deliberate fail-open choices documented in code but invisible at runtime. At
minimum they deserve a `warn!` log with the field/param name; ideally computed-default
evaluation errors on INSERT should fail closed (consistent with the fail-closed validator
precedent cited in `run_validators_tests.rs`).

## Client/server capability asymmetry (shamir-client-ts)

Checked the TS SDK surface against server capabilities. Coverage is strong — batches
(incl. `transactional`, `isolation`, `durability`, `name`, `returnOnly`, limits),
interactive txs (`txBegin`/`txExecute`/`txCommit`/`txRollback` + auto-managed `Db.tx()`),
DDL, admin, replication admin builders, subscriptions, FTS, vector, keyset pagination,
principal/permissions, interner ops — each with e2e tests. Gaps found:

- **DDL inside `Tx`** — intentionally absent (`db.ts:218-224`), matching the server
  boundary; fine.
- **No client-side interner auto-cache** (gap 21) — manual `field-map`/`interner-ops`
  only.
- No TS surface for **engine migration** ops was located in the builders sweep — matches
  the half-finished server state (gap 9); low priority until fjall-target migration
  exists.

The napi `shamir-client-node` binding was not audited (excluded from workspace; separate
MSVC build), but roadmap note `#519` records its typed-errors work as deferred pending a
napi-rs 3.x bump decision.

## Stale "not wired / stub" comments (doc rot — NOT gaps, do not re-fix the code)

| Comment claims | Reality | Location of stale comment |
|---|---|---|
| `WalGroupCommit` "PURELY ADDITIVE: not wired into RepoWalManager or the commit path" | It IS the funnel: `RepoWalManager` wraps `Arc<WalGroupCommit>` and all writes go through it | `crates/shamir-wal/src/wal_group_commit.rs:65-66` (cf. `crates/shamir-tx/src/repo_wal_manager.rs:12-30`) |
| `record_view` "ADDITIVE — not wired into the engine" | `RecordView`/`RecordRef` are used across read_exec, filters, projections, crud, streaming (15+ engine files) | `crates/shamir-types/src/record_view/mod.rs:5-7` |
| funclib "math is the fully-implemented reference; the remaining categories are stubs" | All 13 categories are registered and populated | `crates/shamir-funclib/src/lib.rs:12-13` |
| `VersionedOverlay` "Scaffold (P1a) … not wired into any" | Wired into `mvcc_store` and `storage_membuffer` | `crates/shamir-tx/src/versioned_overlay.rs:14` |
| wasm-host lib.rs "FnCtx/FnBatch … both are intentional placeholders in this slice" | Real bodies exist (DB gateway, net gateway, globals, batch context) | `crates/shamir-wasm-host/src/lib.rs:14-18` |
| access_control "DDL wiring (chmod/chown) is deferred to a later slice" | chmod/chown admin ops exist in `execute/admin_access.rs` + dispatch | `crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:244-245` |

A one-pass `chore(docs)` commit deleting these six stale claims would prevent future
audits (and new contributors) from mis-modeling the system.

## Suggested pre-release triage order

1. Fix gap 1 (upsert `created_at`) — silent data corruption, known fix shape.
2. Convert gaps 2, 3 to clear errors (DDL-time rejection of nested default/transform
   paths; `call_in_tx_not_supported`) — cheap honesty fixes.
3. Add warn logs for gaps 4, 5 (fail-open stamps, Null Call params).
4. Decide on gap 7 (Visibility): either enforce or document as advisory metadata.
5. Actor-threading TODOs (gap 6) — schedule as the already-named "Shomer R2" slice.
6. Replication hardening (gap 14 b/c: leader pin persistence #388, DNS resolution) before
   advertising multi-node deployment.
7. Land the stale-comment cleanup commit.
