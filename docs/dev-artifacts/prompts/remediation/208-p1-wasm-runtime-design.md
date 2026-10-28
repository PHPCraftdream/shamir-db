# P1 stage G5b — p1-wasm-runtime-design

## Goal

Finish the four open cards of `docs/dev-artifacts/prompts/remediation/204-p1-wasm-runtime.md` (read it in full: cards, rulings, requirements, final-message rules) using the reviewed design below. Branch base is G5a (`fix/p1-wasm-runtime`): A07B9019, FA126F4A and C51052DF are already fixed there — do not redo them. Also extend 91385BE0 coverage where the new tests naturally cover host-import failure paths.

Owner decision (2026-10-27): adding `windows-sys = "0.60"` as a `[target.'cfg(windows)'.dependencies]` entry of `crates/shamir-wasm-host` is APPROVED (features Win32_Foundation, Win32_Security, Win32_System_JobObjects, Win32_System_Threading, Win32_System_Diagnostics_ToolHelp). It already resolves at 0.60.2 in Cargo.lock; do not bump any other dependency or version. No other new dependency is allowed.

Order: BD9BC412 first (live production breakage), then EAE6ABE8, 51B448DF, 58C052B0.

Every card gets deterministic regression tests named `rv_wasm_host_<card>_*` that fail when the production fix is reverted (no probabilistic stress tests). Layout limits: ≤ 7 entries per folder, ≤ 1000 lines per file; `mod.rs` re-exports only; tests under `tests/` dirs; imports at the top.

# G5b design (from @oh review, 2026-10-26) — crates/shamir-wasm-host

All four cards are real defects. Cargo.lock pins wasmtime 48.0.7.

## BD9BC412 — host imports call the guest allocator synchronously (PRODUCTION BREAKAGE)
- `globals.rs:108` and `batch.rs:67` call `alloc_typed.call(&mut caller, ..)` (blocking API). The imports are registered with sync `func_wrap` (`wasm_function.rs:206-214`), but the guest always runs through `call_async` on a fiber (`wasm_function.rs:617,643`).
- In wasmtime 48.0.7, `TypedFunc::call` runs `validate_sync_call`, which bails with "cannot do blocking `call` from an async context" whenever `can_block()` holds, i.e. while running on the fiber. It also bails when `async_required` is set (mixed async imports). As a result, every `global_get` / `batch_get` that finds a value traps.
- CI never caught this: the SDK e2e tests skip without the wasm32 toolchain.
- Fix:
  - Register `batch_get` and `global_get` with `func_wrap_async`.
  - Each body is `Box::new(async move { .. })`: look up the value synchronously, then reuse `write_value_to_guest` (`wasm_function.rs:379`, already `call_async`).
  - Drop the dead `let _ = (batch, globals)` clones.
  - The wasm signatures are unchanged.
- Tests go in a new `tests/wasm/bindings/globals_batch_tests.rs`:
  - `rv_wasm_host_bd9bc412_present_global_get_returns_value`: the WAT imports only `global_get`, data "k" sits at 1024, and `shamir_call` returns `(call $global_get (i32.const 1024) (i32.const 1))`. The context is `FnCtx::with_globals(g)` after `g.set("k", Int(7))`. Expect `Ok(Int(7))`; the old code returns `Err(Compute(".. cannot do blocking `call` .."))`.
  - The same test for `batch_get`.
  - A mixed variant that also imports `call`.

## 51B448DF — guest-retained globals are unbounded
- The quota is per `GlobalVars` instance (per runtime/db), not per function.
- In `context.rs`:
  - The map value becomes a private `Slot { value, cost }`.
  - Add `entries: AtomicUsize` and `bytes: AtomicUsize` as O(1) mirrors.
  - Add `GlobalsQuota { max_entries, max_bytes, max_value_bytes }` with a Default of 16384 / 64 MiB / 1 MiB, plus `GlobalVars::with_quota`.
  - Define `GlobalsQuotaError` (thiserror) in `context.rs`; `runtime/` already has 7 entries.
- `try_set(key, value, encoded_len) -> Result<(), GlobalsQuotaError>`:
  1. `cost = key.len() + encoded_len + SLOT_OVERHEAD`.
  2. Read the old cost lock-free with `read_sync`, then reserve the growth with `fetch_update` + `checked_add` filtered on `<= max`: entries first, then bytes, rolling back entries if the bytes reservation fails.
  3. Under `entry_sync(key)`, reconcile against the real old cost: CAS-reserve any extra (on failure, refund everything and return Err), or refund the surplus. Then publish; when the value shrinks, `fetch_sub` after publishing.
  - Never reserve while holding the scc bucket lock.
- Native `set` stays infallible (trusted path, `seed_env`). It accounts with saturating arithmetic and ignores the caps. Overwrites that shrink a value or keep its size are always admitted. `update`, `incr` and `remove` adjust the counters.
- Bytes are the exact encoded msgpack length: `val_len` on the guest path, `to_bytes().len()` on the native path. `host_global_set` checks `val_len > max_value_bytes` before reading memory or decoding.
- The guest sees a trap: `wasmtime::Error::msg(format!("global_set: {e}"))`. The import signature must not change.
- Tests:
  - `rv_wasm_host_51b448df_guest_global_set_quota_traps`: `max_entries=1`, the WAT sets "a" then "b". Expect a Compute error containing "quota", with "a" present and "b" absent.
  - `rv_wasm_host_51b448df_reservation_precedes_publish`: uses a `#[cfg(test)]` admit hook (`OnceLock<Box<dyn Fn(&str)+Send+Sync>>`) that fires after the reservation and before `entry_sync`. Thread A parks in the hook on a Barrier; main runs `try_set("b")` with `max_entries=1` and must get Err; after release, A succeeds and `len()==1`.

## EAE6ABE8 — `FunctionRegistry::rename` can drop the function
- `registry.rs:81-84` removes before it inserts, and `:90` ignores the result of putting the function back.
- Fix (copy, then retire with an identity check):
  1. `f = get(from)`, or `NotFound(from)`.
  2. `insert_sync(to, f.clone())`. If that fails, return `AlreadyExists(to)` (this also covers `from == to`).
  3. `remove_if_sync(from, same(f))`. If it removes, return Ok.
  4. Otherwise, run `remove_if_sync(to, same(f))` and return `NotFound(from)`.
  - `same` is `std::ptr::addr_eq(Arc::as_ptr(a), Arc::as_ptr(b))`, not `Arc::ptr_eq` on dyn.
  - Never hold two scc entries at once (deadlock).
  - A brief window where the function is visible under both names is acceptable; it is never under neither.
- Seam: `#[cfg(test)] interleave: OnceLock<Box<dyn Fn(&FunctionRegistry)+Send+Sync>>`, called between the first and second mutation with no guard held.
- Tests:
  - `rv_wasm_host_eae6abe8_rename_never_drops_function`: the hook registers Const(2) at "a" and Const(3) at "b"; "a" or "b" must still resolve to the original Arc.
  - `rv_wasm_host_eae6abe8_rename_rollback_preserves_concurrent_replace`: the hook calls `replace("a", g)`. Expect Err(NotFound), "a" holds g, and "b" is absent.

## 58C052B0 — compiler subprocess teardown is incomplete
- Defects in `compile.rs`:
  - `:756-759`: the timeout path ignores a `kill()` failure, then calls `wait()` and joins the readers with no bound.
  - `:765-767`: the wait-error path joins the readers without killing.
  - `:772-773`: the success path joins without a bound.
  - `:693-724`: no process group or Job Object. Descendants (RUSTC_WRAPPER / sccache) can hold the pipes open.
- Layout: convert `compile.rs` (886 lines) into a `compile/` folder (src/ entry count stays at 7) with `mod.rs` (re-exports), `pipeline.rs` (existing body, local `use std::io::Read` hoisted), `supervise.rs`, `process_tree_unix.rs`, `process_tree_windows.rs`.
- Unix (no new dependency):
  - Spawn with `CommandExt::process_group(0)`.
  - Terminate with `rustix::process::kill_process_group(Pid::from_child(&child), Signal::KILL)`; rustix is already a cfg(unix) dependency. Verify these names against rustix 1.1.4.
  - Kill the group before reaping the leader; `ESRCH` counts as success. `Drop` does a best-effort group kill.
- Windows: Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
  - Spawn `CREATE_SUSPENDED`, call `AssignProcessToJobObject`, then resume the primary thread (Toolhelp32 snapshot → OpenThread → ResumeThread).
  - If assignment fails, terminate and return Err.
  - `terminate()` is `TerminateJobObject`; `Drop` is `CloseHandle`.
  - This NEEDS a `cfg(windows)` dependency on `windows-sys = "0.60"` (already resolved at 0.60.2 through shamir-private-dir) — owner approval required. The fallback is `taskkill /T /F /PID` on the abnormal paths, which is weaker.
- Supervisor:
  - Readers send their buffers over `mpsc::sync_channel(1)`; collect with `recv_timeout` against a deadline, and detach on timeout.
  - Timeout and wait-error paths: `tree.terminate()` (on failure, log and fall back to `child.kill()`), then `child.wait_timeout(GRACE)`, then collect the readers until the deadline. Return Err with "; teardown incomplete: …" appended when any step failed.
  - `GRACE` is 5 s.
  - The success path also bounds the readers with `GRACE`; on timeout, terminate the tree and drain.
  - Test seam: `#[cfg(test)] TeardownFaults { fail_tree_kill, fail_wait }`.
- Tests go in a new `tests/runtime/compile_teardown_tests.rs`:
  - The child is a shell whose own child holds stdout (Unix `sh -c "sleep 1000; :"`, Windows `cmd /C "ping -n 1000 127.0.0.1"`).
  - Use `timeout = Duration::ZERO`. Run in a std::thread; main waits with `recv_timeout(60 s)`.
  - `rv_wasm_host_58c052b0_timeout_kills_descendants_holding_pipes`, `rv_wasm_host_58c052b0_wait_error_kills_tree`, `rv_wasm_host_58c052b0_kill_failure_is_bounded`.
