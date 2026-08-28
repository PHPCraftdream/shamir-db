# shamir-wasm-host -- Concurrency & lock-free invariants

## Summary

The crate is, with two exceptions, a model citizen of CLAUDE.md's five pillars: all shared state uses `scc::HashMap` with `THasher` (registry, batch context, globals), every `scc::*::len()` call carries a `// O(N) ack:` allow, there is zero `std::sync::Mutex`/`RwLock`/`parking_lot`, host imports follow a documented borrow-dance so no lock/guard is held across `.await`, and Argon2id correctly offloads via `spawn_blocking` (with a worker-starvation test). The two exceptions are serious: (1) the task-#612 aggregate cross-Store fuel budget is implemented with a load-before-grant / debit-at-exit ordering, so the documented aggregate bound does **not** hold for the nested `ctx.call` descent it was built for, and the regression test cannot detect this; (2) the pub `compile_rust_source` API blocks for up to 120 s and is invoked directly inside `async fn`s (in-repo call sites in `shamir-db`), pinning tokio workers in violation of pillar 2. Remaining findings are low-severity atomicity/robustness nits.

## Findings

### 1. Aggregate cross-Store fuel budget does not bound the nested-call descent (grant loaded before ancestors debit)

- **File:line:** `crates/shamir-wasm-host/src/wasm/wasm_function.rs:438-447` (grant computed from a pre-descent `load`), `:585-587` (debit only at exit); test `src/tests/wasm_tests.rs:271-317`
- **Severity:** high
- **Issue:** The module doc (task #612) claims "a guest that recurses via `ctx.call` can no longer execute N × `limits.fuel` instructions across N nested Stores — the aggregate instruction count across the whole fan-out is bounded by one shared budget." The implementation computes `grant = min(remaining, limits.fuel)` from `fuel_budget.load()` **at each level's entry**, but debits consumed fuel only **at each level's exit** (`fetch_sub` after the async block). Since a nested chain is a stack, every level's entry happens before any level's exit: during the descent the counter still reads the full `limits.fuel`, so **every level draws a full fresh grant** — exactly the per-Store-reset behavior the fix was meant to eliminate. The budget only binds sequential *sibling* calls that start after ancestors have fully unwound. Additionally, the load-then-grant is not an atomic reservation, so the invariant would also be overshootable N× under any future concurrent use of one budget (safe today only by the undocumented sequential-chain property).
- **Failure scenario:** A guest recurses via `ctx.call` to the depth limit (32 by default; 1000 in the test). Each level burns up to `limits.fuel` (default 1e9) instructions, for an aggregate of ~(depth+1) × budget before the counter ever goes ≤ 0; execution is stopped only by the depth limit, the per-Store epoch deadline (re-armed at every nesting), or the depth-0 wall-clock timeout. Any deployment relying on fuel as the metering/fairness bound (e.g. having raised `wall_clock_deadline` for long-running legit functions) silently loses the aggregate control. The regression test passes anyway because `result.expect_err(...)` cannot distinguish "stopped by the aggregate budget" from "stopped by the depth-limit guard" — both are `Err`.
- **Suggested fix:** Reserve the grant up front — atomically `fetch_sub(grant)` (via a `fetch_update` CAS loop clamped to the current remaining) before running, and refund `grant − consumed` with `fetch_add` at exit on every path. Strengthen the test to assert the *mechanism*: recursion must terminate at a depth ≪ `depth_limit` (or match the `"aggregate fuel budget exhausted"` error specifically) rather than any `Err`.

### 2. `compile_rust_source` blocks up to 120 s and is called directly on tokio workers (pillar 2)

- **File:line:** `crates/shamir-wasm-host/src/compile.rs:454-456` (pub sync `compile_rust_source`), `:462` (`compile_rust_source_with_timeout`), `:471` (`check_toolchain` spawns two subprocesses), `:592` (`wait_timeout`), `:648` (`maybe_wasm_opt`); live async call sites: `crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:172` (inside `async fn create_function_with_opts_as`), `crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:221` (inside `async fn create_validator_inner`); related: `WasmFunction::from_binary`'s Cranelift compilation (`wasm_function.rs:275-289`) is likewise called inline at `function_management.rs:183` / `validator_management.rs:232`
- **Severity:** high
- **Issue:** Pillar 2 mandates CPU-bound work cross to `tokio::task::spawn_blocking` (the crate itself honors this for Argon2id, `builtin.rs:63`). The compile pipeline is fully synchronous and long-running: a `cargo build` subprocess with a 120 s wall-clock cap, two toolchain probe subprocesses, and an optional `wasm-opt` pass. It is exposed as a plain `pub fn` with no async wrapper, and the workspace's only callers (both `async fn`s; `grep spawn_blocking` in `shamir-db` returns nothing) invoke it directly on the runtime.
- **Failure scenario:** Every `CREATE FUNCTION ... FROM SOURCE` / validator creation pins one tokio worker thread for tens of seconds (up to 120 s on timeout, plus Cranelift compilation of the artifact right after). Unrelated requests multiplexed on that worker stall for the duration; on a `worker_threads = 1` runtime (used by this crate's own tests and plausible in embedded setups) the entire runtime freezes for the compile.
- **Suggested fix:** Add async wrappers in this crate (`compile_rust_source_async` delegating to `tokio::task::spawn_blocking`, same for `WasmFunction::from_binary`/`from_wat` — wasmtime also recommends compiling `Module`s off the async context) and route the DDL call sites through them; at minimum, document the blocking contract on the pub fns and fix the two call sites in `shamir-db`.

### 3. Non-atomic remove-then-insert overwrite in `replace` / `put` / `set`

- **File:line:** `crates/shamir-wasm-host/src/registry.rs:50-54` (`FunctionRegistry::replace`), `crates/shamir-wasm-host/src/context.rs:50-54` (`BatchContext::put`), `crates/shamir-wasm-host/src/context.rs:164-168` (`GlobalVars::set`)
- **Severity:** low
- **Issue:** All three implement overwrite as `remove_sync` followed by `insert_sync`, which is not a per-key atomic swap. (a) A concurrent reader landing in the window sees the key absent — `registry.get` returns `None` (a new invocation fails `NotFound`) or `globals.get`/`batch.get` report absent. (b) Two racing writers have no ordering guarantee: interleaving `A.remove; B.remove; B.insert(v2); A.insert(v1)` leaves the **older** value `v1` as the final state, violating last-writer-wins. The crate's own `update`/`incr` methods already use scc's atomic entry API, so the fix pattern is in-file.
- **Failure scenario:** Two functions concurrently `global_set("shared", ...)` on the process-lifetime `GlobalVars`; the stale value survives. Or a `replace("f", new_artifact)` races a burst of `invoke("f")` calls, some of which fail with a spurious `NotFound`.
- **Suggested fix:** Use the scc entry API for overwrite (`entry_sync` → `Occupied(occ) => *occ.get_mut() = v` / `Vacant(vac) => vac.insert_entry(v)`), making the swap atomic per key.

### 4. `FunctionRegistry::rename` rollback can silently drop the function

- **File:line:** `crates/shamir-wasm-host/src/registry.rs:73-89` (rollback at `:83-88`)
- **Severity:** low
- **Issue:** `rename` is a check-then-act sequence (`contains(to)` → `remove_sync(from)` → `insert_sync(to)`), with a compensating re-insert of `from` if `to` was taken mid-race. The compensation itself is fallible (`insert_sync(from, f)` fails if a racing `register(from, …)` re-took the name after the rename's remove), and its failure is swallowed with `let _ = ...` — the moved-in `Arc<dyn ShamirFunction>` is then dropped, so the function silently vanishes from the registry with no error and no log.
- **Failure scenario:** `rename("a","b")` races `register("a", h)` in the window after rename's `remove_sync("a")` and before the rollback: `insert_sync("b")` fails (taken), the rollback `insert_sync("a")` also fails (re-taken), `f` is silently discarded. Contrived and admin-frequency, but it is a lost-object race, not just a lost update.
- **Suggested fix:** Handle the rollback failure explicitly (return a distinguishable error and/or `log::error!`), or restructure rename via the entry API on `to` plus a conditional remove so the compensation cannot lose the artifact.

### 5. Epoch-ticker spawn failure silently disables pure-CPU guest preemption

- **File:line:** `crates/shamir-wasm-host/src/wasm/wasm_engine.rs:154-163`
- **Severity:** low
- **Issue:** `spawn_epoch_ticker` ends in `.ok()`, discarding a `SpawnError`. Per the module's own analysis (`wasm_engine.rs:43-51`, `wasm_function.rs:449-455`), epoch interruption is the *only* mechanism that can preempt a pure-CPU guest that never hits a host `.await` — fuel can be set arbitrarily high and `tokio::time::timeout` cannot preempt a future that never yields. If the ticker thread fails to spawn, that safety net disappears silently while the engine otherwise reports healthy.
- **Failure scenario:** Thread exhaustion (or a hardened runtime thread cap) at `WasmEngine::new()` → ticker absent → a CPU-bound guest with a large fuel budget spins its worker with no 100 ms preemption; only the depth-0 wall-clock `timeout` (which itself cannot run the timeout future to completion on a saturated single-worker runtime) remains.
- **Suggested fix:** Replace `.ok()` with `log::error!` (naming the lost guarantee) and either retry or fail engine construction; a silent `.ok()` here contradicts the loud-failure posture used everywhere else in the crate.

### 6. Nit: cargo child not killed on the `wait_timeout` error path

- **File:line:** `crates/shamir-wasm-host/src/compile.rs:607-611`
- **Severity:** nit
- **Issue:** The `Err(e)` arm of `child.wait_timeout(timeout)` joins the pipe-reader threads and returns, but never `kill()`s/`wait()`s the child. The timed-out path (`Ok(None)`) handles this correctly; the error path can orphan a running `cargo`/`rustc` process that keeps writing into the `TempDir` (and blocks its deletion on Windows).
- **Suggested fix:** Mirror the timeout arm: `let _ = child.kill(); let _ = child.wait();` before returning the error.
