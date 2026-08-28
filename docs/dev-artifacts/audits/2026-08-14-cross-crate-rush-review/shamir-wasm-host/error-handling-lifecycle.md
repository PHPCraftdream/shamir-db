# shamir-wasm-host -- Error handling & resource lifecycle

## Summary

Overall this crate holds the line well against the CLAUDE.md error-handling rules: a single `thiserror` `FunctionError` enum, `?` propagation throughout, no unguarded `unwrap`/`expect` in production paths, graceful degradation with `log::warn!` for cache/pooling setup, and a fail-closed sanitizer. The weak spots are concentrated in error-path cleanup and observability: the aggregate fuel-budget debit runs as straight-line code after `.await` (so task cancellation leaks budget permanently), the epoch-ticker spawn failure is swallowed with `.ok()` and no log (silently disabling wall-clock pre-emption of CPU-bound guests), and a panic inside the Argon2id blocking task is misreported as "cancelled". Test coverage of error paths is strong for the sanitizer/net-guard/fuel-budget successes but absent for the entire host-import layer's trap paths and the cancellation/panic paths named above.

## Findings

### 1. Aggregate fuel-budget debit is skipped when the call future is cancelled (cancelled task permanently leaks budget)

- **File:line:** `crates/shamir-wasm-host/src/wasm/wasm_function.rs:583-589` (debit), `:495-581` (async block with the await points)
- **Severity:** medium
- **Issue:** The debit of consumed fuel back into the shared `Arc<AtomicI64>` budget (`fuel_budget.fetch_sub(consumed, ...)`) is straight-line code executed *after* the async block's `.await`. The task-#612 doc comment promises the debit happens "exactly once per `call`, on every exit path", but a dropped future is an exit path it does not cover: if the enclosing task is aborted at any of the `.await` points (`instantiate_async`, `alloc_fn.call_async`, `call_fn.call_async` inside the depth-0 `timeout`), lines 583-589 never run and the fuel already consumed by the guest is never returned to the shared counter. There is no `Drop` guard.
- **Failure scenario:** A batch executor or connection task with its own upstream timeout/disconnect aborts the `WasmFunction::call` future mid-guest-execution. The consumed fuel stays debited from the aggregate budget forever; subsequent (perfectly legitimate) calls on the same `FnCtx` chain then fail with `FunctionError::Compute("aggregate fuel budget exhausted across nested calls")` even though no guest work is running -- a permanent, invisible capacity regression until the ctx chain is discarded.
- **Suggested fix:** Move the debit into a `Drop` guard (struct holding `Arc<AtomicI64>` + `grant` + a handle to the `Store`'s remaining fuel) created before the async block; on drop, read `store.get_fuel()` and debit the difference. This makes cancellation, panics, and early returns all debit exactly once. (While there: `store.get_fuel().unwrap_or(0)` at line 585 fails toward over-debiting the full `grant`; with a guard, prefer failing toward *not* debiting, or handle the infallible case explicitly.)

### 2. Epoch-ticker thread spawn failure is swallowed with `.ok()` -- wall-clock pre-emption silently disabled, nothing logged

- **File:line:** `crates/shamir-wasm-host/src/wasm/wasm_engine.rs:154-163` (`.ok()` at line 162)
- **Severity:** medium
- **Issue:** `spawn_epoch_ticker` discards the `io::Result<JoinHandle>` with `.ok()`. If the OS thread cannot be spawned (thread-limit/resource exhaustion), construction succeeds silently but the engine epoch never advances, so every Store's `set_epoch_deadline` (set in `WasmFunction::call`, `wasm_function.rs:485-487`) never fires. The two sibling graceful degradations in the same constructor (disk cache, pooling allocator) both `log::warn!` on failure; this one -- the *safety* mechanism -- logs nothing.
- **Failure scenario:** Under thread pressure, an engine is built without a ticker. A pure-CPU guest (`fuel` configured generously, e.g. `u64::MAX` as `wasm_tests.rs:246` itself demonstrates is a supported configuration) then runs unimpeded: the `tokio::time::timeout` backstop cannot fire either, because a non-yielding guest never lets the timer poll. The only remaining bound is fuel exhaustion -- the exact "pins a worker indefinitely" hazard epoch interruption was added to close.
- **Suggested fix:** At minimum `log::error!` (matching the file's own degradation-logging pattern) naming the lost guarantee; better, treat it as fatal and return `Err(FunctionError::Compute(...))` from `WasmEngine::new` -- a half-functioning engine whose wall-clock bound silently doesn't exist is worse than a failed startup for a reliability-first database.

### 3. No test coverage for any host-import trap/error path (db/http/batch/global imports, depth limit, OOB result pointers, missing exports)

- **File:line:** `crates/shamir-wasm-host/src/tests/` (whole directory); cf. `wasm_sanitizer_tests.rs:68-106` (imports declared, never called) and `src/wasm/host_call.rs:97-101` (depth-limit trap)
- **Severity:** low
- **Issue:** No test in this crate ever *invokes* a `shamir_host` import through guest code. All eight host imports' error paths are unexercised: `db_get/db_insert/db_query/db_execute` "no db gateway" traps (`host_db.rs:45-47, 92-94, 145-147, 181-183`), `http_fetch` "no net gateway" trap (`host_http.rs:138-142`), the `env.*` write-protection trap (`host_globals.rs:46-50`), secret-grant-denied-looks-absent (`host_globals.rs:79-83`), msgpack/UTF-8 decode failures, and `call`'s depth-limit trap (`host_call.rs:97-101`) -- the recursive-fuel test always exhausts fuel before reaching the depth limit, so that branch never runs. Also untested: a module exporting no `memory` (hits `wasm_function.rs:502-504`), and a guest returning an out-of-bounds result pointer (`wasm_function.rs:570-574`).
- **Failure scenario:** A refactor of `HostState` threading or the borrow-dance in any host import can silently break a trap path (e.g. turning the fail-closed `env.*` write-protection into a silent no-op) with a green suite -- precisely the drift the sanitizer's cross-check test was built to prevent for the import *names*, but no equivalent exists for the imports' *behaviour*.
- **Suggested fix:** Add a `tests/host_import_tests.rs` in the established `tests/` layout: small WAT modules that actually call `db_get` (with and without a gateway), `http_fetch` (without a gateway), `global_set("env.X", ...)` (must trap), `global_get("env.X")` ungranted (must return 0), and a self-recursive caller with a small `depth_limit` (must surface the depth-limit error). Some of these may also be covered by `shamir-db` integration tests, but the per-crate suite should own its ABI contract.

### 4. Public gateway traits and net-guard functions return `Result<_, String>` instead of a typed thiserror error

- **File:line:** `crates/shamir-wasm-host/src/db_gateway.rs:60-86`; `crates/shamir-wasm-host/src/net_gateway.rs:60, 69, 110, 157-160`
- **Severity:** low
- **Issue:** `DbGateway::{get,insert,query,execute}`, `NetGateway::fetch`, and the exported guards `check_host_allowed` / `check_url_allowed` / `check_url_allowed_resolved` are public library APIs returning `Result<_, String>`. CLAUDE.md prescribes `thiserror` for library error enums and `Box<dyn Error>` only as a boundary last resort. Every hop flattens to string interpolation (e.g. `host_db.rs:53`: `format!("db_get: {e}")`), so error kinds can't be matched, causes can't be chained, and the allowlist-denied vs DNS-failure vs curl-failed distinction is only recoverable by parsing prose.
- **Failure scenario:** A caller of `check_url_allowed_resolved` that wants to distinguish "operator forgot to allowlist" (config error, surface to admin) from "transient DNS failure" (retry) cannot do so without substring matching; a future `NetGateway` implementation can't attach a source chain (e.g. curl exit code + stderr) without embedding it in the string.
- **Suggested fix:** Introduce a `#[derive(Debug, Error)] pub enum GatewayError` / `NetGuardError` with thiserror variants (`NoGateway`, `Decode(String)`, `EgressDenied { host, reason }`, `Dns(String)`, `Transport(String)`), and keep `to_string()` flattening only at the guest-visible trap/envelope boundary (which is already the stated design for `http_fetch`'s catchable Err envelope).

### 5. Panic inside the Argon2id blocking task is collapsed into `FunctionError::Cancelled`, discarding the panic payload

- **File:line:** `crates/shamir-wasm-host/src/builtin.rs:63-83` (`.map_err(|_| FunctionError::Cancelled)` at line 83)
- **Severity:** low
- **Issue:** The `spawn_blocking(...).await` result is mapped with `|_|`, discarding the `JoinError`. A panic in the closure (or an aborted blocking task) both become `Cancelled`, whose message ("function task cancelled") describes only one of the two cases and names neither the panic message nor location.
- **Failure scenario:** A future funclib change panics on some input (e.g. an assert in the KDF); production logs show a stream of "function task cancelled" for user-visible failures, sending operators chasing cancellation/backpressure instead of the actual defect. The panic info is unrecoverable.
- **Suggested fix:** Branch on the `JoinError`: `if e.is_panic()` -> `FunctionError::Compute(format!("argon2id task panicked: {payload}"))` (extract via `e.into_panic()` + `downcast_ref::<&str>/<String>`), else `FunctionError::Cancelled`. Optionally add a dedicated `Panicked(String)` variant so callers can tell them apart.

### 6. `map_wasm_error` classifies traps by substring-matching the error message

- **File:line:** `crates/shamir-wasm-host/src/wasm/wasm_function.rs:593-602`
- **Severity:** low
- **Issue:** Fuel vs. epoch-deadline vs. generic traps are discriminated with `msg.contains("fuel")` / `msg.contains("epoch")` on wasmtime's Display text. This couples error *semantics* to wasmtime's prose: the strings are not a stable API across wasmtime major versions (the crate tracks wasmtime `46`, so this will be exercised), and any trap whose message merely contains those substrings is misclassified.
- **Failure scenario:** After a wasmtime bump rewords the fuel message, every exhausted-guest call starts surfacing as the generic "shamir_call trap: ..." -- the fuel-specific operator message silently degrades, and no test fails because `wasm_fuel_exhaustion_traps` only asserts `FunctionError::Compute(_)`. Conversely, a genuine guest trap whose message happens to embed "fuel" is misreported as budget exhaustion.
- **Suggested fix:** Prefer typed discrimination (`wasmtime::Error::downcast_ref::<wasmtime::Trap>()` / `TrapCode::OutOfFuel` / epoch-interrupt codes where available in wasmtime 46), keeping the substring check only as a last-resort fallback, and assert the *specific* mapped message (not just the variant) in `wasm_fuel_exhaustion_traps`.

### 7. `compile.rs` degrades invalid-UTF-8 temp paths into `unwrap_or("")` arguments for cargo instead of a typed error

- **File:line:** `crates/shamir-wasm-host/src/compile.rs:541-544`
- **Severity:** low
- **Issue:** `tmpdir.path().join("Cargo.toml").to_str().unwrap_or("")` (and the same for `--target-dir`) feed an empty string to cargo if the temp path is not representable as UTF-8 (possible on Windows with unusual temp-dir names). Everywhere else this function maps I/O failures to `FunctionError::Compute(...)` with context; this one site manufactures a guaranteed-broken cargo invocation instead.
- **Failure scenario:** On such a host, every `compile_rust_source` call fails with a confusing cargo diagnostic about an empty `--manifest-path` (or writes artifacts to an empty target-dir path), and the operator-debuggable "temp path" root cause never appears in the error.
- **Suggested fix:** Convert once: `let manifest = ...to_str().map_err(|_| FunctionError::Compute("temp path is not valid UTF-8".into()))?;` and reuse for both arguments.

### 8. Timeout kill path: ignored `kill()` result followed by an unbounded `child.wait()` can hang past the very timeout being enforced

- **File:line:** `crates/shamir-wasm-host/src/compile.rs:592-606`
- **Severity:** low
- **Issue:** On the `Ok(None)` (timed-out) branch, `let _ = child.kill();` discards failure (TerminateProcess/OpenProcess can fail on Windows, e.g. access-denied on a stuck child), and the subsequent `child.wait()` has no deadline. If the kill fails, the caller blocks indefinitely -- recreating the "pathological guest build wedges the host" problem the wall-clock timeout (CRIT-6 part A) exists to prevent, now on the host's own thread.
- **Failure scenario:** A guest build whose rustc child becomes unkillable (AV interference, job-object weirdness) turns a bounded 120 s compile into a permanently stuck `create_function` call with no diagnostic.
- **Suggested fix:** Log if `kill()` errors, and reap with a bounded secondary `wait_timeout` loop (e.g. a few seconds), returning `Compute("compilation timed out; kill failed: ...")` if the child still hasn't exited -- the reader threads are already joined on every path, so the structure exists.

### 9. `FunctionRegistry::rename`'s error-path rollback insert can itself fail silently, dropping the function

- **File:line:** `crates/shamir-wasm-host/src/registry.rs:73-89` (restore at line 86)
- **Severity:** low
- **Issue:** On a racing collision at `to`, the closure restores the entry with `let _ = self.functions.insert_sync(from.to_string(), f);`. If a concurrent `register(from, ...)` landed between `remove_sync(from)` and the restore, the restore fails and `f` is dropped on the floor -- the function exists under neither name -- while the caller receives `AlreadyExists(to)` implying a clean rollback (the comment says "put `from` back").
- **Failure scenario:** Concurrent `rename("a","b")` + `register("a", other)` (both plausible from parallel DDL/wire requests on a shared registry): the renamed artifact vanishes; `invoke("a")` reaches the *other* function and `invoke("b")` returns `NotFound`, with no log of the lost artifact.
- **Suggested fix:** Handle the restore's `Err` -- at minimum `log::error!` with the lost name; better, make the re-key atomic from the reader's perspective (retry loop that re-inserts the captured `f` under a fresh name, never dropping it).

### 10. Nit: one leaked OS thread per `WasmEngine`, and tests instantiate engines freely

- **File:line:** `crates/shamir-wasm-host/src/wasm/wasm_engine.rs:150-162`; e.g. `src/tests/wasm_tests.rs` (each test builds its own `WasmEngine::new()`)
- **Severity:** nit
- **Issue:** The epoch ticker is a never-terminating OS thread captured per engine; the doc argues engines are "long-lived singletons", but nothing enforces that, and the crate's own tests create an engine per test (each leaking a ticker thread for the process lifetime). Not an error path per se, but an unchecked lifecycle assumption.
- **Suggested fix:** Document/enforce the singleton (e.g. construct via a `OnceLock` in the facade) or share one engine across tests via a `OnceLock<WasmEngine>` test helper.

### 11. Nit: forbidden-macro scanner fails open (`unwrap_or_default`) on an invariant break

- **File:line:** `crates/shamir-wasm-host/src/compile.rs:250-252`
- **Severity:** nit
- **Issue:** `String::from_utf8(out).unwrap_or_default()` would silently return an *empty* cleaned source on UTF-8 corruption, which `find_forbidden_macro_in_clean` then scans as clean -- i.e. the one failure mode of this function makes the security scan pass. Today it is genuinely infallible (input is `&str`; only ASCII bytes are replaced), which is exactly why it should be an `expect` with the invariant named, not a fail-open default.
- **Suggested fix:** Replace with `.expect("strip only writes ASCII spaces over non-newline bytes; input was &str")` so a future edit that breaks the invariant fails loudly, not permissively.

### 12. Nit: `FunctionMeta::from_record` silently coerces malformed persisted catalogue fields to defaults

- **File:line:** `crates/shamir-wasm-host/src/meta.rs:110-148`
- **Severity:** nit
- **Issue:** Unknown `visibility`/`security` strings parse-fail into `Private`/`Invoker` and non-string grant entries are dropped by `filter_map`, all silently. The direction is fail-closed (most restrictive), which is the right bias, but a present-but-unparseable value almost certainly signals catalogue corruption or a version-skew from a newer writer, and that signal is invisible -- as are silently truncated grant lists.
- **Suggested fix:** Keep the fail-closed defaults but `log::warn!` once per discarded/unparsed field (name + raw value) so operators can detect a damaged or forward-compat catalogue row.
