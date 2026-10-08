<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-server — error-handling-lifecycle independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

The current refutations are supported. Local RAII is not a universal task-lifetime barrier; response guards and accepted connections supply additional concrete counter-evidence.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 8 | 0 | 0 | 0 | 5 | 0 | 3 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-summary"></a>

### Claim Summary — Every fallible production path uses thiserror Results and lifecycle is uniformly RAII-correct

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

Production helpers return Result&lt;_, String&gt;, and RAII does not prevent detached bridge/connection tasks or premature response-guard release. String error boundaries are not automatically runtime defects, but they refute the stated universal type inventory.

Evidence: [crates/shamir-server/src/replication/supervisor.rs:256](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/supervisor.rs#L256); [crates/shamir-server/src/db_handler/handler.rs:465](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/handler.rs#L465); [crates/shamir-server/src/subscriptions/registry.rs:134](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/subscriptions/registry.rs#L134); [crates/shamir-server/src/connection/request_loop.rs:348](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L348).

<a id="review-notes-expect"></a>

### Claim Notes/.expect() — All expect sites are structurally invariant-backed

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

Cursor mode and retry-loop expects have visible guards, but SIGTERM handler installation is fallible OS setup. Its failure is not established as structurally unreachable or proof that the process is otherwise doomed.

Evidence: [crates/shamir-server/src/runtime.rs:81](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/runtime.rs#L81); [crates/shamir-server/src/db_handler/cursor_handlers.rs:1323](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/cursor_handlers.rs#L1323); [crates/shamir-server/src/doctor.rs:238](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/doctor.rs#L238).

<a id="review-notes-boot-time-cleanup"></a>

### Claim Notes/boot-time cleanup — Every early launch return releases acquired resources correctly through Drop

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

A later TLS or listener error occurs after reapers, and potentially earlier listeners, were spawned. Dropping their plain handles detaches tasks; dropping the root token does not cancel remaining clones. Exact Tokio 1.49.0 JoinHandle and tokio-util 0.7.18 CancellationToken sources support this mechanism. The local instance-lock File does drop, creating different lifetimes for the guard and surviving resources.

Evidence: [crates/shamir-server/src/server/server_launcher.rs:102](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L102); [crates/shamir-server/src/server/server_launcher.rs:529](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L529); [crates/shamir-server/src/server/server_launcher.rs:601](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L601); [crates/shamir-server/src/server/server_launcher.rs:728](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L728); [crates/shamir-server/src/tx_registry.rs:298](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/tx_registry.rs#L298); [Cargo.lock:4195](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4195); [Cargo.lock:4261](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4261).

<a id="review-notes-finalize-change-password"></a>

### Claim Notes/finalize_change_password — Discarded password-finalization value is a timestamp, not an error

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

The callee returns u64; discarding it does not swallow a Result. This says nothing about separate persistence operations around the password-change flow.

Evidence: [crates/shamir-server/src/db_handler/admin.rs:572](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/admin.rs#L572); [crates/shamir-connect/src/server/changepw.rs:105](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/changepw.rs#L105).

<a id="review-notes-try-push"></a>

### Claim Notes/try_push — Discarded control-push failures follow best-effort subscription semantics

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Gap/Closed sends are intentionally best-effort, while event failures increment the slow-consumer counter. The normative subscription contract does not promise durable delivery or announcement of every individual loss.

Evidence: [crates/shamir-server/src/subscriptions/bridge.rs:577](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/subscriptions/bridge.rs#L577); [crates/shamir-server/src/subscriptions/bridge.rs:597](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/subscriptions/bridge.rs#L597); [crates/shamir-server/src/subscriptions/push.rs:99](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/subscriptions/push.rs#L99); [docs/guide-docs/client-server-protocol-spec/SUBSCRIPTIONS.md:29](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/guide-docs/client-server-protocol-spec/SUBSCRIPTIONS.md#L29); [docs/guide-docs/client-server-protocol-spec/SUBSCRIPTIONS.md:48](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/guide-docs/client-server-protocol-spec/SUBSCRIPTIONS.md#L48).

<a id="review-notes-error-path-tests"></a>

### Claim Notes/error-path tests — Dedicated tests force both restore swap-failure subcases and other resource errors

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

Registered Windows tests assert successful rollback or first-rename Io, not both second-rename failure variants. The successful-rollback fixture uses a polling watcher whose interception is scheduling-dependent. SwapPartialFailure lacks a dedicated discriminating assertion in the inspected tests.

Evidence: [crates/shamir-server/src/tests/mod.rs:11](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/tests/mod.rs#L11); [crates/shamir-server/src/tests/restore_tests.rs:64](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/tests/restore_tests.rs#L64); [crates/shamir-server/src/tests/restore_tests.rs:93](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/tests/restore_tests.rs#L93); [crates/shamir-server/src/tests/restore_tests.rs:118](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/tests/restore_tests.rs#L118); [crates/shamir-server/src/tests/restore_tests.rs:203](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/tests/restore_tests.rs#L203); [crates/shamir-server/src/restore.rs:265](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/restore.rs#L265).

<a id="review-notes-tx-cursor-reapers"></a>

### Claim Notes/tx-cursor-reapers — Registry reapers release expired transaction/cursor ownership

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Real registry fixtures exercise expired removal, background tx reaping, session-count cleanup and fetch-lease exclusion. Removing registry ownership permits final Arc/guard drops; outstanding owners can extend the lifetime. The assertions do not independently prove every MVCC GC-floor release.

Evidence: [crates/shamir-server/src/tests/tx_registry_tests.rs:138](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/tests/tx_registry_tests.rs#L138); [crates/shamir-server/src/tests/tx_registry_tests.rs:164](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/tests/tx_registry_tests.rs#L164); [crates/shamir-server/src/tests/cursor_registry_tests.rs:237](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/tests/cursor_registry_tests.rs#L237); [crates/shamir-server/src/tests/cursor_registry_tests.rs:327](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/tests/cursor_registry_tests.rs#L327); [crates/shamir-server/src/cursor_registry.rs:648](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/cursor_registry.rs#L648).

<a id="review-notes-request-loop-teardown"></a>

### Claim Notes/request-loop teardown — Request-loop teardown is a hard ordered-release barrier under every exit path

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

The sweep precedes dispatch cancellation/drain, allowing the bridge attachment/admission race. Aborting a bridge handle also requests cancellation rather than synchronously joining its destruction. Writer writes/shutdown are unbounded awaits. Panic detection only occurs when the reader next drains completed dispatch tasks.

Evidence: [crates/shamir-server/src/connection/request_loop.rs:240](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L240); [crates/shamir-server/src/connection/request_loop.rs:413](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L413); [crates/shamir-server/src/connection/request_loop.rs:417](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L417); [crates/shamir-server/src/subscriptions/registry.rs:23](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/subscriptions/registry.rs#L23).

Grouping/duplicate: [correctness-tdd.md#3](correctness-tdd.md#review-3). This is not an additional independent defect.

## Evidence and recipe corrections

- Exact published Tokio/tokio-util sources confirm that bare JoinHandle/token drops do not provide the claimed cancellation barrier.
- ServerHandle explicitly does not await connections; consequently its later 'no new writes can arrive' flush justification is false for already accepted connections.
- Scheduler drop does not terminate the runtime. Dropping its sender closes broadcast receivers, and run_periodic treats that closure as shutdown.
- Restore's two renames have distinct failure/recovery states; do not describe first-rename Io coverage as SwapPartialFailure coverage.
- Fjall 3.1.6 published source maps PersistMode::SyncAll to journal file sync_all, but that does not turn all surrounding file operations or error-swallowing APIs into durability guarantees.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-server -- Error handling & resource lifecycle

## Summary

`shamir-server` follows CLAUDE.md's error-handling rules closely: every fallible
production path returns `Result<T, thiserror::Error-derived E>`, `anyhow`/`Box<dyn
Error>` stay confined to `main.rs`/CLI boundary code, and the handful of `.expect()`
calls in non-test code are all invariant-proven (guarded by a prior branch that makes
the `None`/`Err` case structurally unreachable) rather than "shouldn't happen" hopes.
Resource lifecycle (file locks, redb/fjall handles, background tasks, MVCC snapshot
guards, TLS acceptors) is handled with unusually thorough RAII discipline and doc
comments that name the exact race each cleanup step closes — `backup.rs`/`restore.rs`'s
staged-temp-dir cleanup and atomic-swap rollback, `tx_registry.rs`/`cursor_registry.rs`'s
reaper-driven RAII abort, and `server_handle.rs::shutdown`'s ordered drain are all
model implementations of the crate's own conventions. No findings rise above "nit" —
this is a clean bill of health for the theme.

## Findings

No findings for this theme.

### Notes from the review (not findings — recorded for completeness)

- **`.expect()` sites are all invariant-backed, not defensive panics avoided by
  accident.** `runtime.rs:81` (`signal(SignalKind::terminate())` — a `#[cfg(unix)]`
  syscall that only fails on a kernel resource exhaustion so severe the process is
  already doomed), `doctor.rs:238` / `access_tree.rs:176` (`last_err.expect(...)`
  inside a `for _ in 0..20 { ... }` retry loop where the `None` branch already
  returned before this line is reached — the loop body only ever sets `last_err` on
  `Err`), `db_handler/cursor_handlers.rs:1331` (`order_by.expect(...)` gated by a
  `mode == PaginationMode::Keyset` check that `pagination_mode_for_query` only
  returns when `order_by` is already `Some`), `server_launcher.rs:404`
  (`get_db("default").expect(...)` immediately after `create_db("default")` a few
  lines above), `tests/`-only sites (`user_directory.rs`'s test corruption helper).
  None of these are reachable from untrusted input.

- **Boot-time resource acquisition (`server_launcher.rs::launch`) relies on Rust drop
  semantics for cleanup on early return, and this is correct here.** The single-
  instance file lock (`data_dir_lock`, line 114-133), `ServerMetaStore`, `FjallUserDirectory`,
  `FjallConsumedCounters`, and `FjallAuditAppender` are all opened before several
  later fallible steps (bootstrap, `ShamirDb::init`, TLS load, listener binds) that can
  return `Err` and unwind out of `launch()`. There is no explicit rollback/cleanup
  block for this — but there does not need to be: every one of these types releases
  its OS resource (file lock, fjall keyspace handle) in its own `Drop` impl, and a
  local `let` binding that goes out of scope on an early `?`/`return Err` runs that
  drop deterministically. This is the correct pattern for RAII-first Rust, not a gap.

- **`db_handler/admin.rs:572`'s `let _ = finalize_change_password(...)`** looks at
  first glance like a swallowed error, but `finalize_change_password` (defined in
  `shamir-connect::server::changepw`) returns `u64` (a timestamp), not a `Result` —
  there is no error being discarded. False lead, confirmed by reading the callee.

- **`subscriptions/bridge.rs`'s repeated `let _ = push.try_push(frame)`** (lines 242,
  387, 577, 597) discard a push-delivery failure, but this is the documented,
  intentional at-most-once semantics of the subscription push path — the same module
  already carries a `PushKind::Gap` mechanism specifically to tell a subscriber "you
  missed some events here," which only makes sense if individual pushes are allowed
  to drop. Not a resource-lifecycle or error-handling gap.

- **Error-path test coverage is a strength, not a gap.** `tests/restore_tests.rs`
  has dedicated fault-injection tests for both swap-failure sub-cases
  (`SwapFailedRollbackSucceeded` vs `SwapPartialFailure`) and the copy-step failure
  path, each asserting the exact on-disk state left behind. `tx_registry.rs` /
  `cursor_registry.rs` have paired reaper tests confirming RAII abort/release on
  expiry. `backup.rs`'s `verify_manifest` has tests for checksum mismatch, path
  traversal, and duplicate entries (security-relevant error paths). This is above
  the workspace norm, not below it.

- **`request_loop.rs`'s teardown sequence (lines 410-431)** is a good reference
  example of ordered resource release under every exit path (client EOF, writer
  death, dispatch panic, idle timeout): `registry.close_all()` before dropping
  `conn` (which holds the push-sink `Arc`), then `join_set.abort_all()` +
  drain before dropping `tx`, then conditionally awaiting `writer_handle`. A dispatch
  task panic is explicitly caught via `JoinSet::try_join_next()`'s `Err::is_panic()`
  and converted into a connection teardown (`break 'conn`) rather than propagating
  the panic or silently ignoring it.

</details>
