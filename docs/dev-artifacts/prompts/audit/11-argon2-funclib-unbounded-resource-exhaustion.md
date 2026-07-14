Task: HIGH-security — the Argon2id concurrency semaphore protects the
WRONG path; the actual guest/query-reachable Argon2id call has no
concurrency cap and can OOM the server (audit §2b,
`docs/dev-artifacts/audits/2026-07-06-security-network-surface.md` line 78).

## Background (read first — investigate before fixing)

`crates/shamir-connect/src/server/argon2_semaphore.rs` defines
`Argon2Semaphore` (64-permit counting semaphore, spec §8/§8.1), wired
into `ConnectionContext` (`crates/shamir-server/src/connection/connection_context.rs:40`)
and constructed in `server_launcher.rs:213`. Its doc comment says it
protects `auth_init` against parallel-Argon2 OOM. **But per the audit,
the server's `auth_init`/SCRAM verify path does NOT run Argon2 at
all** (server-side SCRAM is HMAC/HKDF only; the only client-side
`argon2id()` reference is in `crates/shamir-connect/src/common/scram.rs:52`,
which runs on the CLIENT, not the server). So this semaphore currently
gates a path that never calls Argon2 — it's a no-op protection.

The REAL, guest/query-reachable Argon2 exposure is
`crates/shamir-funclib/src/crypto.rs:54` (`argon2id_fn`) — a scalar
function registered under the name `"argon2id"` (`crypto.rs:158`),
callable from any query/function expression a user or a WASM guest
function can construct. Its resource bounds today are PER-CALL only:

```rust
const A2_MAX_MEMORY_KB: u32 = 1_048_576;   // 1 GiB per call
const A2_MAX_TIME: u32 = 16;
const A2_MAX_PARALLELISM: u32 = 16;
const A2_MAX_LENGTH: u32 = 256;
```

There is NO cap on the NUMBER of concurrent or total `argon2id()`
invocations across all connections/requests. A low-privileged user (or
a WASM guest function acting as one) can issue queries that call
`argon2id(pw, salt, [1048576, 16, 16, 256])` repeatedly and in
parallel — each call can allocate up to 1 GiB and run for a
non-trivial time, with no aggregate limit. Per the audit: this can
exhaust the `spawn_blocking` thread pool (default 512 threads) AND
server memory, and the existing 64-permit `Argon2Semaphore` does
nothing to stop it (wrong path).

## REQUIRED FIRST STEP: investigate the actual call path (do not guess)

The crate doc comment at `crypto.rs:17` claims: "A caller dispatching
it on an async runtime MUST offload to `spawn_blocking`; do not invoke
it inline on a runtime worker." **Verify whether this is actually
true today.** Trace how `argon2id_fn` (registered as a plain
`FnEntry::pure` scalar in `ScalarRegistry`) gets invoked during real
query/validator/filter execution:

- `crates/shamir-engine/src/validator/record_validator.rs`,
  `validator/schema/field_rule.rs`, `validator/schema/format.rs`
  (validators can call scalars),
- `crates/shamir-engine/src/query/filter/eval_context.rs` (filter
  expression evaluation),
- `crates/shamir-engine/src/query/read/aggregate.rs`,
- `crates/shamir-engine/src/table/table_manager.rs` /
  `table_manager_validators.rs` / `write_helpers.rs`,
- `crates/shamir-engine/src/db_instance/db_instance.rs`,
- `crates/shamir-engine/src/query/batch/executor_traits.rs`.

Determine: is `ScalarRegistry::call`/whatever dispatches a registered
`FnEntry::pure` function ALWAYS invoked synchronously inline (i.e. on
whatever async task/thread is currently running the query), with NO
`spawn_blocking` wrapper anywhere in the call chain? If so, that is a
SEPARATE, arguably worse problem than the audit's stated concern (an
Argon2 call with `memory_kb=1_048_576, time=16` can block a tokio
worker thread for hundreds of milliseconds to seconds, stalling every
other task scheduled on that worker — not just consuming a
`spawn_blocking` pool slot). Report what you find precisely: quote the
actual call chain from wherever a query enters the engine down to
`argon2id_fn`, and state definitively whether `spawn_blocking` is
present anywhere in it.

## Fix — shape depends on what you find in the investigation

**If `argon2id_fn` is invoked inline (no `spawn_blocking`) anywhere in
the reachable call chain:** the minimal safe fix, given the scope of
this task, is to NOT attempt a deep architectural change (moving
scalar-function dispatch to `spawn_blocking` project-wide is a much
larger refactor, likely out of scope here — flag it as a follow-up
finding in your report if you conclude this, do not attempt it inline
with the concurrency-cap fix below). Focus this task on adding an
**aggregate concurrency gate specifically around `argon2id_fn`'s
actual expensive work** (the `argon.hash_password_into(...)` call),
using a process-wide semaphore analogous to the existing
`Argon2Semaphore` but reachable from `shamir-funclib` (which currently
has no dependency on `shamir-connect`, so either move/duplicate a
minimal semaphore primitive into a crate both can depend on — check
the workspace dependency graph before deciding: does `shamir-funclib`
already depend on anything `shamir-connect` also depends on where a
shared primitive could live, e.g. `shamir-collections` or
`shamir-tunables`? Prefer NOT introducing a new inter-crate dependency
edge if avoidable — a small `std::sync::atomic`-based counting
semaphore, mirroring `Argon2Semaphore`'s existing design exactly
(spec-consistent capacity, blocking `acquire`), can live directly in
`shamir-funclib` itself with no new dependency, since it needs no
crate-external types).

Concretely:
1. Add a process-wide (`static` or passed-through, whichever fits this
   crate's existing conventions — check whether `ScalarRegistry`/
   `FnEntry` already thread any shared state through registered
   functions, or whether they are pure `fn(&[QueryValue]) ->
   Result<...>` with no side-channel; if the latter, a `once_cell`/
   `std::sync::LazyLock`-style process-global semaphore instance is the
   pragmatic fit — check what synchronization primitives this crate
   already uses elsewhere for a similar "shared limiter, no explicit
   threading" need before inventing a new idiom) concurrency cap on
   `argon2id_fn` specifically. A reasonable default: mirror the
   existing spec's `MAX_CONCURRENT_ARGON2 = 64`, or choose a lower
   number given this path additionally allows a MUCH more expensive
   per-call profile (up to 1 GiB, vs the auth path's fixed ~128 MB) —
   use your judgment and state the reasoning in your report.
2. `argon2id_fn` blocks (does NOT return an error) waiting for a
   permit if the semaphore is exhausted — mirroring
   `Argon2Semaphore::acquire`'s existing blocking-wait design (this
   crate's functions are synchronous `fn`, so a blocking wait, not an
   async one, is what fits here) — UNLESS you determine from the
   investigation above that this function IS in fact reached only via
   `spawn_blocking` in every real call path (in which case a blocking
   wait inside the semaphore is safe and does not stall the async
   runtime; if it's reached INLINE on an async task somewhere, a
   blocking wait would ALSO stall the runtime the same way the
   uncapped Argon2 call already does today — note this tension
   explicitly in your report either way, since it constrains what
   "safe" means here).
3. Also reduce `A2_MAX_MEMORY_KB` from the current 1 GiB per-call cap
   to something more conservative if you judge the audit's characterization
   ("1 GiB без cap на число вызовов") warrants it — this is a secondary,
   optional hardening; the PRIMARY required fix is the concurrency cap,
   which bounds the AGGREGATE memory regardless of the per-call cap.

## TDD requirement

1. **Red**: write a test in `crates/shamir-funclib/src/tests/` (check
   existing test module structure/convention for `crypto.rs` first —
   likely `crypto_tests.rs` or similar under a `tests/` submodule per
   this project's CLAUDE.md test-organization rules) that:
   - Spawns N (e.g. 3-5) concurrent `argon2id_fn` calls with a small
     but non-trivial `memory_kb`/`time` (small enough to keep the test
     fast — do NOT use 1 GiB in a test) via `std::thread::spawn` (this
     is a sync fn, no tokio needed unless you're testing through an
     actual async call path).
   - Asserts that at most the configured cap number of them run their
     `hash_password_into` concurrently — e.g. by having each call
     record a timestamp/counter before and after the expensive call
     under a shared atomic "currently running" counter, and asserting
     the observed peak never exceeds the cap. This should FAIL before
     your fix (unbounded concurrency) and PASS after.
2. **Green**: implement the fix.
3. Confirm existing `shamir-funclib` tests (especially any existing
   `argon2id` correctness tests, which must still produce bit-identical
   output — the semaphore must not change the KDF's determinism/output,
   only its scheduling) still pass.

## Test scope command

```
./scripts/test.sh -p shamir-funclib
```

## Gate (must be clean before finishing)

```
cargo fmt -p shamir-funclib -- --check
cargo clippy -p shamir-funclib --all-targets -- -D warnings
```

If your investigation concludes a fix belongs partly in
`shamir-engine` (e.g. you find and fix an inline-dispatch issue there
too), also run:
```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine
```

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

When done, report exactly:
- **Investigation findings**: the actual call chain from query/engine
  entry to `argon2id_fn`, and whether `spawn_blocking` is present
  anywhere in it (quote the relevant code). This is the most important
  part of your report — do not skip it even if the answer complicates
  the fix.
- The concurrency-cap mechanism you added: where the semaphore lives
  (which crate/file), its capacity and reasoning, and whether it's a
  new primitive or reuses/mirrors an existing one.
- Whether you additionally reduced `A2_MAX_MEMORY_KB` (and why/why not).
- The failing-test-then-passing evidence.
- Gate results (exact commands + pass/fail).
- Any residual risk or follow-up you'd flag but did NOT fix in this
  pass (e.g. if you found a genuine inline-on-async-runtime dispatch
  problem that's out of scope for a minimal concurrency-cap fix).
