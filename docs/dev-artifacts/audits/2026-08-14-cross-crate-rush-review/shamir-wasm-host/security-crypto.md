# shamir-wasm-host -- Security & crypto boundary

## Summary

This crate carries no `unsafe`, no `std::sync`/`parking_lot` locks, and no HMAC/SCRAM/TLS or password-comparison code (auth lives in shamir-connect; the compile permission gate lives in shamir-db). The boundary work is largely strong: the WASM import sanitizer is allowlist-based with a linker-sync regression test, the SSRF guard handles non-canonical IP encodings and DNS-rebind pinning with dedicated tests, secrets fail closed ("denied looks absent"), and the RI-7 confused-deputy fix threads the actor through nested calls. The main gaps are all in untrusted-input handling around the host compiler and egress guard: the forbidden-macro scanner is trivially bypassable with grammar-legal whitespace between macro name and `!`, the SSRF private-IP set misses `0.0.0.0` (loopback in practice) and several non-routable ranges, and `meta.rs` documents contradictory fail-open/fail-closed defaults for `net_grants`.

## Findings

### 1. Forbidden-macro scan bypassed by whitespace/comment between macro name and `!`
- **File:line:** `crates/shamir-wasm-host/src/compile.rs:361-371` (also `FORBIDDEN_MACROS` at 112-118, scan entry at 138-141)
- **Severity:** high
- **Issue:** `find_forbidden_macro_in_clean` requires `!` to appear *immediately* after the macro name (`bytes[after] == b'!'`). In Rust, macro invocation grammar (`SimplePath ! DelimTokenTree`) is token-based, so whitespace or a comment between the name and the bang is legal: `env !"HOME"`, `env /*x*/!("HOME")`, `include_str !"C:/Users/me/secret.txt"` all compile as real `env!`/`include_str!` invocations, but the scanner (which blanks comments to spaces and then demands `!` at `name.len()` offset) sees `env    !(` and passes the source. The scan is documented (compile.rs module docs, CRIT-6 / audit #440 part A) as the control that "closes the cheapest exfiltration paths", and the test suite (`tests/compile_tests.rs`) exercises only the tight `env!("X")` form.
- **Failure scenario:** An authorized-but-malicious tenant submits a function body containing `include_str !"C:/Users/<operator>/.aws/credentials"` (or `env !"PATH"`). The scan passes, the child `cargo build` reads the host file at compile time and embeds it as a `const` in the `.wasm` artifact (or the scrubbed-but-identifying env values: `USERPROFILE`, `APPDATA`, `HOME` leak usernames/paths), which the tenant then retrieves/exfiltrates. `include_str!` has no path restriction — the env scrub does not contain it.
- **Suggested fix:** In the cleaned text, when a forbidden name is found, skip spaces/comments between the name and the next non-blank character before testing for `!` (i.e. treat name + optional-blanks + `!` as the invocation shape). Add adversarial regression tests: `env !"X"`, `env /*c*/!("X")`, `include_str \n !"f"`.

### 2. SSRF guard misses `0.0.0.0` (and other non-routable IPv4 ranges) — wildcard allowlist reaches loopback services
- **File:line:** `crates/shamir-wasm-host/src/net_gateway.rs:455-481` (`is_private_or_loopback_ip`), canonicalization at 305-334
- **Severity:** medium
- **Issue:** The IPv4 arm checks only 127/8, 10/8, 172.16/12, 192.168/16, 169.254/16. It misses `0.0.0.0/8` ("this network" — on both Windows and Linux an outbound connect to `0.0.0.0` lands on the local host), plus `100.64.0.0/10` (CGNAT / some cloud-internal), `198.18.0.0/15`, `192.0.0.0/24`, and `224.0.0.0/4`/`240.0.0.0/4`. Both the string-level check (`check_host_allowed`) and the resolved check (`check_url_allowed_resolved`) key off the same predicate, so the miss survives both layers.
- **Failure scenario:** Operator configures a broad allowlist entry (e.g. `*` or `*.guest-egress.example`). A guest function calls `http_fetch({url: "http://0.0.0.0:5984/..."})`. `canonicalize_ip("0.0.0.0")` → `0.0.0.0` → not "private" by this predicate → string check passes; `lookup_host("0.0.0.0")` returns `0.0.0.0` → not "private" → pin returned; the gateway connects to `0.0.0.0` and reaches a service bound on localhost (metadata-style or admin endpoints) that the exact-entry-only policy was designed to protect.
- **Suggested fix:** Add `octets[0] == 0` to the IPv4 arm (and consider `100.64..191.255` CGNAT and `198.18/15`), mirroring the existing `private_ip_wildcard_denied` test with `0.0.0.0` / `00.00.00.00`-style encodings.

### 3. Contradictory fail-open vs fail-closed documentation of the `net_grants` default
- **File:line:** `crates/shamir-wasm-host/src/meta.rs:83-91` vs `meta.rs:188-190`
- **Severity:** medium
- **Issue:** `FunctionMeta::net_grants`'s doc (task #609) states "an EMPTY/absent `net_grants` means NO egress for this function" (fail-closed, restrictive-by-default, matching `secret_grants`). Twenty lines below, `CreateFunctionOptions`'s doc states "empty `net_grants` = full DB-wide `net_allowlist`" (fail-open), and even cross-references `[FunctionMeta::net_grants]` as its authority. Both descriptions live in the same file and cannot both be true; the enforcing code (`build_net_gateway`) is in shamir-db, so this crate cannot reconcile them itself.
- **Failure scenario:** A future change to egress wiring is implemented from the wrong doc. If the fail-open reading is (or becomes) the actual behavior, creating a user function without an explicit `net_grants` silently grants it the DB-wide egress ceiling — the exact default task #609 was landed to prevent. This is the same doc-drift class CLAUDE.md documents for the F-9 Mutex-exception list.
- **Suggested fix:** Verify `build_net_gateway`'s actual behavior in shamir-db, then rewrite both doc comments to one truth (and state it in `CreateFunctionOptions::default`), ideally with a cross-crate test asserting empty-grants ⇒ `fetch` traps.

### 4. Compile timeout kills `cargo` but not its `rustc`/build-script grandchildren
- **File:line:** `crates/shamir-wasm-host/src/compile.rs:592-606` (timeout kill path)
- **Severity:** low
- **Issue:** On timeout the code calls `child.kill()`, which terminates only the direct `cargo` process. Cargo's `rustc` children (which evaluate guest `const fn`s — cf. the `sum_to(200_000)` heavy-const source in `tests/compile_tests.rs:235-250` — and run proc-macro expansion) are not in a Job Object / process group and survive as orphans, continuing to burn CPU/RAM to completion. This weakens the module doc's claim that "a malicious or pathological guest cannot wedge the host indefinitely"; the kill is best-effort on Windows (no `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`), despite the comment "kill and reap to avoid orphaned cargo/rustc".
- **Failure scenario:** A tenant submits a guest source whose const-eval takes 30 minutes; after 120 s the wrapper reports "compilation timed out" but an orphaned `rustc` keeps a core pegged (repeated submissions multiply this), degrading the host beyond the intended budget.
- **Suggested fix:** Spawn cargo inside a Windows Job Object with kill-on-close (or a process group on POSIX and kill the group), so the timeout tears down the whole compiler tree.

### 5. No bound on concurrent Argon2id invocations; cost parameters fully caller-supplied at this layer
- **File:line:** `crates/shamir-wasm-host/src/builtin.rs:42-83`
- **Severity:** low
- **Issue:** `argon2id` accepts caller-supplied `memory_kb`/`time`/`parallelism` and runs each call on `spawn_blocking` with no semaphore/permit limiting concurrent KDFs. Per-call ranges are delegated to funclib (the `"out_of_range"`/`"bad_params"` error mapping in `builtin.rs:66-74` shows bounds exist), but nothing here caps how many KDFs run simultaneously, and guest fuel/epoch/wall-clock budgets do not apply inside `spawn_blocking` work.
- **Failure scenario:** A hostile tenant fans out many concurrent function invocations hitting `argon2id`; with the tokio blocking pool defaulting to ~512 threads at 19.45 MiB default cost each, that is ~9.5 GiB of hashing memory plus CPU saturation, degrading other tenants on a shared node.
- **Suggested fix:** Wrap the `spawn_blocking` call in a process-wide `tokio::sync::Semaphore` sized to a small concurrency (e.g. 2–8), and/or clamp the maximum accepted `memory_kb`/`time` at this layer regardless of funclib's looser bounds.

### 6. Epoch-ticker thread spawn failure silently disables the CPU-guest kill switch
- **File:line:** `crates/shamir-wasm-host/src/wasm_engine.rs:154-163`
- **Severity:** low
- **Issue:** `spawn_epoch_ticker` ends with `.ok()`, discarding a failed `thread::Builder::spawn`. The module's own analysis (wasm_function.rs:52-56, wasm_engine.rs:43-51) states that a pure-CPU guest that never awaits cannot be pre-empted by fuel or the top-level tokio timeout — the epoch deadline is the *only* control for that case. If the ticker never runs, `set_epoch_deadline` never fires and such a guest pins its worker for its whole (fuel-sized, but potentially minutes-long) run, unbounded in wall-clock terms.
- **Failure scenario:** Under thread-exhaustion (e.g. concurrent `spawn_blocking` pressure from finding 5) the ticker spawn fails; every subsequently created engine silently lacks wall-clock pre-emption for CPU-bound guests.
- **Suggested fix:** Log the failure at `error` (or fail `WasmEngine::new`), since the epoch guarantee is a load-bearing security control, not an optimization.

### 7. `wasm-opt`/toolchain probes run with the full inherited host environment
- **File:line:** `crates/shamir-wasm-host/src/compile.rs:648-669` (`maybe_wasm_opt`), 699-710 (`check_toolchain`)
- **Severity:** nit
- **Issue:** The guest `cargo build` gets the CRIT-6 env scrub (`env_clear` + allowlist), but the `wasm-opt` and `cargo --version`/`rustup` probe invocations inherit the host's full environment. There is no realistic exfiltration channel today (they process only host-generated artifacts and produce diagnostics to the log), but the inconsistency means the "no host secret reaches a child spawned from untrusted input" story is only true for one of the child processes.
- **Suggested fix:** Apply the same `scrubbed_env()` allowlist to these invocations for uniformity.

### 8. `GlobalVars::seed_env` panics on non-UTF-8 environment variables
- **File:line:** `crates/shamir-wasm-host/src/context.rs:210-216`
- **Severity:** nit
- **Issue:** `std::env::vars()` panics if any environment variable (name or value) is not valid Unicode — possible on Unix. Since seeding happens in-process from the host environment, a single malformed variable crashes seeding (and thus any startup path that calls it).
- **Failure scenario:** Host starts with a legacy non-UTF-8 var set (e.g. by a service wrapper); `seed_env` panics on first call instead of degrading.
- **Suggested fix:** Iterate `std::env::vars_os()` and skip (or lossy-convert) non-UTF-8 entries.

## Not findings (checked, clean for this theme)

- **No `unsafe`** anywhere under `src/`; no `std::sync::Mutex`/`RwLock`/`parking_lot` (grep-verified; only doc mentions).
- **`verify_wasm_module`** (`wasm/wasm_sanitizer.rs`): allowlist posture, component-encoding explicitly rejected, `TypeRef`-agnostic name check, and a two-direction sync test against the real `Linker` registrations (`tests/wasm_sanitizer_tests.rs:248`). Guest-memory accessors (`read_guest_mem`, `write_*_to_guest`) bounds-check negative ptr/len and end-vs-`data_size` on every path.
- **Secret reads fail closed and without an oracle**: denied `env.*` reads return the same `Ok(0)` as absent keys (`host_globals.rs:79-83`); the `env.*` write path is unconditionally blocked for guests (`host_globals.rs:43-50`); grants are exact-string set lookups, no manual secret comparison exists in this crate.
- **Actor propagation (RI-7)**: nested `ctx.call` inherits the caller's actor/db/net/grants (`host_call.rs:76-126`), closing the confused-deputy path, with tests (`nested_actor_tests.rs`).
- **SSRF non-canonical IP handling** (decimal/hex/inet_aton short forms, IPv4-mapped IPv6) is unusually thorough and well-tested; the `ResolvedPin` DNS-rebind TOCTOU contract is sound. `0.0.0.0` is the one literal-IP gap (finding 2).
