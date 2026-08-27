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
