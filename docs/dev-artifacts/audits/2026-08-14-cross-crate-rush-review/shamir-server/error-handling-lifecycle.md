<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-server — error-handling-lifecycle revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

The clean-bill-of-health conclusion and several universal guarantees are unsupported or contradicted by current source. The timestamp-discard and best-effort-push observations are valid.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 8 | 0 | 0 | 0 | 5 | 0 | 3 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-summary"></a>

### Claim Summary — Every fallible production path uses thiserror Results and lifecycle is uniformly RAII-correct

Status: `refuted`. Current risk: —.

Supervisor production helpers return Result<_, String>. More importantly, the assigned bridge attachment race contradicts universal task-cleanup claims. This is not proof that every String error is a runtime defect.

Evidence: [crates/shamir-server/src/replication/supervisor.rs:256](../../../../../crates/shamir-server/src/replication/supervisor.rs#L256); [crates/shamir-server/src/replication/supervisor.rs:343](../../../../../crates/shamir-server/src/replication/supervisor.rs#L343); [crates/shamir-server/src/subscriptions/registry.rs:134](../../../../../crates/shamir-server/src/subscriptions/registry.rs#L134).

<a id="review-notes-expect"></a>

### Claim Notes/.expect() — All expect sites are structurally invariant-backed

Status: `refuted`. Current risk: —.

The retry-loop and cursor expects have visible structural guards, but installing a SIGTERM handler is fallible OS setup, not a structural invariant. The claim that failure necessarily means a doomed process was not proven.

Evidence: [crates/shamir-server/src/runtime.rs:81](../../../../../crates/shamir-server/src/runtime.rs#L81); [crates/shamir-server/src/doctor.rs:221](../../../../../crates/shamir-server/src/doctor.rs#L221); [crates/shamir-server/src/doctor.rs:238](../../../../../crates/shamir-server/src/doctor.rs#L238); [crates/shamir-server/src/access_tree.rs:159](../../../../../crates/shamir-server/src/access_tree.rs#L159); [crates/shamir-server/src/db_handler/cursor_handlers.rs:1323](../../../../../crates/shamir-server/src/db_handler/cursor_handlers.rs#L1323).

<a id="review-notes-boot-time-cleanup"></a>

### Claim Notes/boot-time cleanup — Every early launch return releases acquired resources correctly through Drop

Status: `refuted`. Current risk: —.

Local file ownership releases the instance lock, but spawned tasks retain cloned resources. Reapers start before later fallible metadata/TLS/bind steps; their plain token/JoinHandle drops do not cancel/join them. Successful ServerHandle shutdown does not run when launch returns Err.

Evidence: [crates/shamir-server/src/server/server_launcher.rs:102](../../../../../crates/shamir-server/src/server/server_launcher.rs#L102); [crates/shamir-server/src/server/server_launcher.rs:529](../../../../../crates/shamir-server/src/server/server_launcher.rs#L529); [crates/shamir-server/src/server/server_launcher.rs:539](../../../../../crates/shamir-server/src/server/server_launcher.rs#L539); [crates/shamir-server/src/server/server_launcher.rs:549](../../../../../crates/shamir-server/src/server/server_launcher.rs#L549); [crates/shamir-server/src/server/server_launcher.rs:601](../../../../../crates/shamir-server/src/server/server_launcher.rs#L601); [crates/shamir-server/src/server/server_handle.rs:107](../../../../../crates/shamir-server/src/server/server_handle.rs#L107); [crates/shamir-server/src/observability.rs:90](../../../../../crates/shamir-server/src/observability.rs#L90).

<a id="review-notes-finalize-change-password"></a>

### Claim Notes/finalize_change_password — Discarded password-finalization value is a timestamp, not an error

Status: `not-applicable`. Current risk: —.

The callee returns u64, so the let-discard does not swallow a Result.

Evidence: [crates/shamir-server/src/db_handler/admin.rs:572](../../../../../crates/shamir-server/src/db_handler/admin.rs#L572); [crates/shamir-connect/src/server/changepw.rs:105](../../../../../crates/shamir-connect/src/server/changepw.rs#L105).

<a id="review-notes-try-push"></a>

### Claim Notes/try_push — Discarded control-push failures follow best-effort subscription semantics

Status: `not-applicable`. Current risk: —.

Gap/Closed control frames are best-effort, and event delivery tracks consecutive push failures before closing slow consumers. A failed Gap push does not guarantee that every delivery loss is announced.

Evidence: [crates/shamir-server/src/subscriptions/bridge.rs:577](../../../../../crates/shamir-server/src/subscriptions/bridge.rs#L577); [crates/shamir-server/src/subscriptions/bridge.rs:597](../../../../../crates/shamir-server/src/subscriptions/bridge.rs#L597); [crates/shamir-server/src/subscriptions/push.rs:100](../../../../../crates/shamir-server/src/subscriptions/push.rs#L100); [docs/guide-docs/client-server-protocol-spec/SUBSCRIPTIONS.md:29](../../../../../docs/guide-docs/client-server-protocol-spec/SUBSCRIPTIONS.md#L29); [docs/guide-docs/client-server-protocol-spec/SUBSCRIPTIONS.md:48](../../../../../docs/guide-docs/client-server-protocol-spec/SUBSCRIPTIONS.md#L48).

<a id="review-notes-error-path-tests"></a>

### Claim Notes/error-path tests — Dedicated tests force both restore swap-failure subcases and other resource errors

Status: `refuted`. Current risk: —.

Registered tests cover successful rollback, first-rename failure, copy failure and manifest failures. The first-rename test positively asserts Io and explicitly excludes both second-rename variants; it is not a SwapPartialFailure test. The successful-rollback test uses a scheduling-sensitive watcher, not a deterministic injected hook.

Evidence: [crates/shamir-server/src/tests/mod.rs:2](../../../../../crates/shamir-server/src/tests/mod.rs#L2); [crates/shamir-server/src/tests/mod.rs:11](../../../../../crates/shamir-server/src/tests/mod.rs#L11); [crates/shamir-server/src/tests/restore_tests.rs:66](../../../../../crates/shamir-server/src/tests/restore_tests.rs#L66); [crates/shamir-server/src/tests/restore_tests.rs:93](../../../../../crates/shamir-server/src/tests/restore_tests.rs#L93); [crates/shamir-server/src/tests/restore_tests.rs:118](../../../../../crates/shamir-server/src/tests/restore_tests.rs#L118); [crates/shamir-server/src/tests/restore_tests.rs:203](../../../../../crates/shamir-server/src/tests/restore_tests.rs#L203); [crates/shamir-server/src/tests/restore_tests.rs:539](../../../../../crates/shamir-server/src/tests/restore_tests.rs#L539); [crates/shamir-server/src/tests/backup_tests.rs:236](../../../../../crates/shamir-server/src/tests/backup_tests.rs#L236).

<a id="review-notes-tx-cursor-reapers"></a>

### Claim Notes/tx-cursor-reapers — Registry reapers release expired transaction/cursor ownership

Status: `not-applicable`. Current risk: —.

The registered tests use real contexts/guards and check removal, ownership counts and cursor reapability. Those assertions are meaningful source-level coverage, not evidence of test execution or exhaustive release correctness.

Evidence: [crates/shamir-server/src/tests/mod.rs:9](../../../../../crates/shamir-server/src/tests/mod.rs#L9); [crates/shamir-server/src/tests/mod.rs:16](../../../../../crates/shamir-server/src/tests/mod.rs#L16); [crates/shamir-server/src/tests/tx_registry_tests.rs:138](../../../../../crates/shamir-server/src/tests/tx_registry_tests.rs#L138); [crates/shamir-server/src/tests/cursor_registry_tests.rs:237](../../../../../crates/shamir-server/src/tests/cursor_registry_tests.rs#L237); [crates/shamir-server/src/tests/cursor_registry_tests.rs:327](../../../../../crates/shamir-server/src/tests/cursor_registry_tests.rs#L327).

<a id="review-notes-request-loop-teardown"></a>

### Claim Notes/request-loop teardown — Request-loop teardown is a hard ordered-release barrier under every exit path

Status: `refuted`. Current risk: —.

The described call ordering exists, but close_all runs before dispatch cancellation/drain and can lose a concurrently attached bridge handle. Non-panic JoinErrors are ignored, and panic detection happens only when the reader next drains the JoinSet, not immediately on task completion.

Evidence: [crates/shamir-server/src/connection/request_loop.rs:240](../../../../../crates/shamir-server/src/connection/request_loop.rs#L240); [crates/shamir-server/src/connection/request_loop.rs:413](../../../../../crates/shamir-server/src/connection/request_loop.rs#L413); [crates/shamir-server/src/connection/request_loop.rs:415](../../../../../crates/shamir-server/src/connection/request_loop.rs#L415); [crates/shamir-server/src/connection/request_loop.rs:417](../../../../../crates/shamir-server/src/connection/request_loop.rs#L417); [crates/shamir-server/src/subscriptions/registry.rs:134](../../../../../crates/shamir-server/src/subscriptions/registry.rs#L134).

Grouping/duplicate: `correctness-tdd.md#3`. This row is not another independent defect.

## Corrections and qualified non-findings

- RAII releases local owners, not resources still held by detached tasks; separate pre-spawn failures from later launch failures.
- Do not label fallible signal installation an invariant proof.
- Remove the claim of dedicated coverage for SwapPartialFailure.
- Restore watcher-based fault forcing is platform-gated and scheduling-sensitive; do not characterize it as deterministic.
- The original JoinError observation is latent diagnostics debt, not by itself proof of currently reachable request loss.

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
