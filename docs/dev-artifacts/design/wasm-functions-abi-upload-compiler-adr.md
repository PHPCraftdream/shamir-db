# ADR PROC-0: WASM functions: module contract, upload/download, in-process compiler, DDL/OQL API

Status: **proposed (revision 2, after an independent review), awaiting owner approval. No code
starts before approval.** Covers three works that share one contract:

* PROC-1: load pipeline, limits, identity hash, facade API on `ShamirDb` (check, export,
  describe, alter), roundtrip tests. No wire ops.
* PROC-2: in-process compiler.
* PROC-3b: wire ops, DDL/OQL, Rust and TypeScript builders, guide docs, HMAC encoding.

Ordering: PROC-1, then PROC-2 and PROC-3b (the new language is reachable end to end only
through PROC-3b). The two-tier compilation core (Winch then Cranelift) is implemented, see
`wasm-two-tier-compilation.md`; this ADR adds the surface around it.

## Facts from the code (baseline)

* A guest module exports `memory`, `shamir_alloc(len: i32) -> i32` and
  `shamir_call(ptr: i32, len: i32) -> i64` (packed `(ptr, len)`), and may import only
  `shamir_host.*` from `SANCTIONED_HOST_IMPORTS` (11 names: `batch_put`, `batch_get`,
  `global_set`, `global_get`, `call`, `db_get`, `db_insert`, `db_query`, `db_execute`,
  `raise_error`, `http_fetch`). `wasm_sanitizer.rs` enforces the import allowlist and rejects
  the component encoding before any compile; exports are looked up by fixed literal names.
  The `call`, `db_*`, `http_fetch`, `batch_get`, `global_get` imports are async wasmtime
  imports (`func_wrap_async`).
* `CreateFunctionOp` (`deny_unknown_fields`) carries `source` (Rust source, compiled by
  `cargo` on the host, gated by `Execute` on `WasmCompiler`, default mode 0o755) or `wasm`
  (base64), plus `replace`, `visibility`, `security`, `secret_grants`, `net_grants`, `hmac`.
* The function row stores `name`, `wasm_b64` (the uploaded bytes verbatim), `wasm_hash`
  (64-bit `FxHasher`, 16 hex, **written and never read**), `lang` (`"wasm"` or `"rust"`),
  `source`, `version` (constant 1), `kind`, plus `FunctionMeta` fields (`metadata_version`,
  `visibility`, `security`, `secret_grants`, `net_grants`). Validator rows are a different
  shape (`_id`, `bound_in`, no `FunctionMeta`), written by `validator_management.rs`.
* At boot each stored row is recompiled through `from_binary_blocking` (mode `Auto`); a
  failure logs a warning and skips the function.
* Function introspection today is `List` on `FunctionNamespace`; there is no per-function
  `Read` check, no export, no describe, no alter. `Action::Read` exists. Legacy rows without
  a mode load as open (0o777). `Visibility` is not enforced by the host today.
* Call-time limits: `WasmLimits` (fuel 1e9, memory 64 MiB, wall clock 30 s). Load-time:
  only the pooling-allocator shape (one memory and one table per module) and the 16 MiB
  wire frame cap.
* The HMAC canonical form of `CreateFunction` (`security/hmac.rs`) covers name, replace,
  security, visibility, grants and the digests of `source`/`wasm`. It does not cover `lang`.

## Decisions to approve

| # | Decision | Recommendation |
|---|---|---|
| D1 | Optional custom sections `shamir.abi` (version) and `shamir.origin` (claimed producer) | accept |
| D2 | Two gate sets: admission (create/check/alter) and load (boot), structural caps as the real DoS bound, `wasmparser` Validator with an exact feature set | accept |
| D3 | Download returns exact stored bytes; identity is a fixed composite fast hash; the hash is never used for lookup, dedup or caching | accept |
| D4 | The new language compiles in the server process, no `cargo`, no subprocess | accept |
| D5 | Tier mode is a per-function property `optimize: auto \| now \| never` | accept |
| D6 | Ops `describe_function`, `export_function`, `check_function`, `alter_function` (only `optimize`); `create_function` gains `lang`, `optimize`; HMAC binds `lang` and `optimize` | accept |

Open points for the owner (defaults given): export permission (default `Write`, see D6);
whether `alter_function` may later widen to `visibility` with an HMAC (default no).

## D1. Module contract

The contract above is ABI **v1** and is frozen. Additions:

* Custom section `shamir.abi`, payload exactly 4 bytes, `u32 LE` ABI version. Absent means
  v1. A duplicate section, a payload of another length or an unknown version is rejected with
  a message naming both versions. Future incompatible changes bump the number.
* Custom section `shamir.origin`, informational: a short UTF-8 tag (`rust-cargo`,
  `shamir-lang`, `external`). It is a **claim by the uploader**: `describe_function` labels it
  as claimed and reports the server-derived `lang` as authoritative. It never changes
  validation, limits or permissions: every origin passes the same checks.
* Other custom sections (`name`, `producers`, DWARF) are allowed, ignored, and counted in
  the size cap. One callable per module: `shamir_call`.

## D2. Admission and load

Two gate sets, because a stored row must keep loading identically on every node:

* **Admission** (create, replace, `check_function`, `alter_function` when it rebuilds): all
  gates below, including operator-controlled caps and the compile time cap.
* **Load** (boot, replica apply): only deterministic gates that do not depend on server
  options or machine speed: parse, import allowlist, contract check, feature set.
  Operator caps and time caps do **not** apply. A row that fails load is not silently
  skipped: it stays in the catalogue, its function is marked `unloadable` with the reason,
  calls fail with that reason, `describe_function` reports it. Existing rows therefore stay
  valid or become visibly flagged, never vanish.

Order (cheapest first, before the compile except step 6):

1. Raw size cap `max_module_bytes` (default 8 MiB). The cap is validated at config load
   against the frame cap: `4/3 * max_module_bytes + max_source_bytes + overhead < frame cap`
   (16 MiB), so create and export always fit in one frame. The base64 length is checked
   before decoding.
2. One `wasmparser` pass with a `Validator` configured with an exact `WasmFeatures` set (the
   only authority). Counters and caps: functions, types, imports, exports, globals, locals
   per function, largest function body, operators per body, block nesting depth,
   `br_table` target count, parameters and results per type, **table min and max elements**,
   element segments and total size, data segments and total data bytes, memory minimum
   pages. Defaults are conservative and listed in one table in the code.
3. Feature set: MVP, sign-extension, non-trapping float-to-int, bulk memory, multi-value,
   mutable globals, **reference types** (Rust 1.82+ enables them by default for
   `wasm32-unknown-unknown` and encodes `call_indirect` with them; a gate without them would
   reject modules from our own `lang = rust` path). Partial use is limited by an operator
   scan: one funcref table, no `externref`, no table ops except `call_indirect`. Rejected:
   threads, SIMD (until tier-parity tests cover it), GC, exceptions, tail calls, memory64,
   multi-memory, component model. The `wasmtime` `Config` flags must be a **superset** of this
   set (tested); the Validator, not `Config`, decides what is admitted.
4. Existing import allowlist and encoding check.
5. Contract check: exports exist with the exact types; `shamir.abi` valid.
6. Compile at admission, any tier (fast, the `Auto` fallback to Cranelift, `now`):
   a shared compile semaphore for create, check, alter and boot bounds concurrency; a
   wall-clock cap (`max_compile_time`, default 10 s) **rejects the request only**: the
   blocking thread cannot be cancelled, so the structural caps of step 2 are the actual bound
   on compile work. The background upgrade stays bounded by `background_jobs`.

`check_function` runs steps 1 to 6 with the requested `optimize` mode (a `never` verdict can
differ from `auto` when Winch rejects the module), persists no catalogue row, and compiles
without the incremental disk cache, so it leaves no entry anywhere.

## D3. Download and identity

* `export_function` returns the **exact bytes stored**, no recompilation or normalization.
  A source language additionally returns its `source` text.
* **Round-trip invariant:** for any accepted module `m`, `export(create(m)) == m`, and
  creating from the exported bytes yields the same hash. Tested for each origin.
* **Identity hash:** fixed by this ADR as `[u64; 2]` = XXH3-128 of the full bytes (seed 0,
  hi then lo, platform independent), wire/describe form 32 lowercase hex characters. It is
  an identity and corruption check only: no secret seed and no mandatory byte comparison
  (owner decision). **Rule: nothing may use it for lookup, dedup, caching or any permission
  or `definer` decision.** A second-preimage against it is therefore harmless by
  construction. `FxHasher` is not a component (its output depends on `usize` width).
* Legacy rows carry the old 16-hex `wasm_hash`. On first load the legacy value is verified
  first (Fx over the bytes as `Vec<u8>`, including its length prefix); a match gets the
  composite hash written on the next write; a mismatch marks the row `corrupt`. A row
  without the composite field is "unknown", never a mismatch.
* **Checked:** on create (computed), on first load of a row into memory, on export. Never on
  the call path. Replication and restore of function rows has no code path today; the check
  is added when such a path exists (not testable now, so not an acceptance criterion).
  A mismatch marks the function `corrupt`: calls fail with a clear error and
  `describe_function` reports it. Storage already checksums records; this adds an end-to-end
  check of the module bytes.
* The wasmtime incremental compile cache keys itself; no cache of ours is keyed by the
  composite hash.

## D4. In-process language and compiler

**Implementation status (D4 only):** M1–M8 implementation and supporting tests are present in the source. This status note records the implementation; it does not claim that tests were run.

* **Language:** a strictly typed, TypeScript-like subset: `i32`, `i64`, `f64`, `bool`,
  `string`/`bytes` through the existing msgpack call convention, records, lists, `if`,
  `while`, `for`, calls; no closures at first; no dynamic typing. The grammar is documented
  in [`shamir-lang-design.md`](shamir-lang-design.md).
* **Compiler:** the server calls the separate, runtime-free Rust crate `shamir-lang`; its
  compile path scans, parses, type-checks, lowers, verifies, emits wasm through
  `wasm-encoder`, then self-admits the emitted bytes. Production dependencies are exactly
  `shamir-wasm-admission`, pinned `wasm-encoder = "=0.254.2"`, and `thiserror`. This path
  compiles in process and does not invoke `cargo`.
* **Purity evidence:** `crates/wasm/shamir-lang/src/api/tests/purity_tests.rs` contains a
  source-text test. It removes `//` comments and whitespace, then checks direct
  `std::process`, `std::fs`, `std::net`, `std::env`, and `std::thread` spellings, grouped
  `std::{...}` imports for those modules, and `Command::new`; a separate check compares the
  exact production dependency names with the three above. This is not a token or AST
  scanner: it does not establish coverage for aliases or `core`/`alloc`, and it does not
  parse or skip strings, raw strings, or macros. No clippy `disallowed-methods` entry or
  rust-guard purity rule enforces this constraint.
* **Crash safety:** a stack overflow aborts the server and `spawn_blocking` does not catch
  it. Depth caps are enforced inside the recursive-descent parser and the type checker, and
  the compiler runs on a dedicated thread with a fixed stack size. Criteria: deep-nesting
  and fuzz tests. Other bounds: source size cap, a compile fuel counter, the shared compile
  semaphore. The host uses an 8 MiB worker stack and the existing shared compile gate.
* **Guest runtime:** compiled modules already embed the allocator and MessagePack codec;
  raw-byte guest codec conformance is covered in the host tests. This is implemented, not a
  future first deliverable. With multi-module linking a non-goal, the runtime remains inlined
  into each module and counts against the size caps.
* **Async host calls** need no source transformation: the imports are already async
  wasmtime imports that suspend the guest fiber.
* **Staging:** `Stage::Mvp` (the default) supports scalar and validator entries. The
  implemented `Stage::Procedures` supports procedure entries with direct `db_get`,
  `db_insert` and `db_query`, and typed `batch_read`/`batch_insert` requests through
  `db_execute` and typed response projections. Direct database results and response
  projections require contextual result types (`T010`); the guest emitter preserves query
  insertion order. Source-level `call`, `$query`/`after` result references, and update/delete
  builder operations remain design-only (PROC-2c). Optimization passes are out of scope
  (the tiers do that).
* The Rust-source path stays behind `Execute` on `WasmCompiler`, whose default mode is
  0o755, i.e. open to everyone: the residual risk in
  `wasm-untrusted-compile-sanitization-decision.md` **stays fully open by default for
  `lang = rust`**; this ADR closes it only for the new language. Tightening that default is
  a separate decision.

## D5. Compilation mode

Per function: `optimize: auto | now | never`, default `auto`, mapped to `TierMode`. Stored in
the row (`optimize`, absent means `auto`). The mode is fixed at construction, so
`alter_function` rebuilds and re-registers the function (a synchronous Cranelift compile for
`now`) and per-function counters reset; boot must pass the stored mode instead of the default
(acceptance criterion). `never` with a module Winch rejects fails admission while `auto`
accepts it. Engine-wide `TierOptions` stay server settings. `describe_function` reports the
configured mode, the current tier and per-function upgrade state; that needs new
per-function accessors (today only per-engine `TierStats` and call counters that stop after
the upgrade exist).

## D6. Wire, DDL/OQL and builders (PROC-3b; PROC-1 provides the facade methods)

* `create_function`: optional `lang` (`"wasm" | "rust" | "shamir"`; default inferred as
  today, `source` means Rust) and `optimize`. `deny_unknown_fields` stays, so an old server
  rejects new fields loudly; **builders omit unset fields** so new clients still work against
  old servers.
* New ops, op key carries the name as for the existing ops:
  * `{ "describe_function": "name" }`: lang (server-derived), claimed origin, ABI version,
    composite hash, size, tier, optimize mode, counters, `corrupt`/`unloadable` state; no
    bytes. Permission: `Read` on the function (a new check; legacy rows are open, as every
    other right on them).
  * `{ "export_function": "name" }`: bytes (base64) and source text. Permission `Write` on
    the function (the bytes reveal the logic). Legacy rows without a mode are open, so for
    them this is open too; stated, not hidden.
  * `{ "check_function": { "lang", "wasm" | "source", "optimize" } }`: D2 dry run. Permission
    `Create` on `FunctionNamespace`; for `lang = rust` source it additionally needs `Execute`
    on `WasmCompiler`, exactly as `create_function` (the dry run runs `cargo`).
  * `{ "alter_function": "name", "optimize": ... }`: changes **only** `optimize`; grants,
    `security`, `visibility` change through `create_function` with `replace` (and its HMAC) or
    the existing `chmod`. Permission `Write` on the function. No HMAC (performance mode only).
* **HMAC:** the canonical form of `CreateFunction` gains the resolved `lang` and `optimize`
  (absent values encoded as their defaults, explicitly), with updated test vectors, so a
  signed `definer` or `replace` request cannot be replayed with another language. The
  `CreateFunction` destructuring is exhaustive, so a missed field fails to compile.
* Validators: D2 and D3 apply to validator rows through the same pipeline; `lang` on
  `CreateValidatorOp` is added in PROC-3b.
* Docs: a guide page in `docs/guide-docs/` and the protocol spec entries.

## Non-goals

No component model, no WASI, no multi-module linking, no hot reload of a running call, no
module signing, no migration of existing rows (they stay valid; `lang` and `optimize`
default), no change to fuel, epoch or memory limits, no tightening of the `WasmCompiler`
default.

## Risks

* Feature set too strict for real toolchains: the `check_function` report names the exact
  rejected feature; a corpus test of Rust-built modules (including trait objects and
  function pointers) is part of PROC-1 acceptance.
* Composite hash is not collision-resistant against an adversary (owner decision): contained
  by the D3 rule that no lookup, cache or permission uses it.
* Compiler bugs: output is validated like any upload, so a bug becomes a rejected create; a
  crash is contained by depth caps and a dedicated thread.
* A hostile module that Winch rejects triggers a Cranelift compile at admission: bounded by
  the structural caps and the shared semaphore, not by the timeout.

## Acceptance criteria

* PROC-1: a test per cap and per rejected feature; ABI section tests (duplicate, wrong
  length, unknown version); Rust-built corpus passes; round-trip per origin; composite hash
  vectors; legacy-row verification (match, mismatch, unknown); `check_function` leaves no
  catalogue row and no cache entry; `unloadable` and `corrupt` behaviour; boot honours the
  stored `optimize`; an export of a maximum-size module fits one frame; load gates ignore
  operator caps.
* PROC-2: golden compile tests; differential tests against a reference interpreter on the
  scalar subset; rejected-source tests (size, depth); deep-nesting and fuzz tests that must
  not abort; the source-text purity test and its dependency-name check described in D4 are
  present (they are not a structural no-process rule).
* PROC-3b: wire round-trip tests of all ops; builder tests in Rust and TypeScript; permission
  matrix including `check_function` with Rust source without `Execute`, legacy rows, and
  `alter_function` limits; HMAC vectors for `lang` and `optimize`; `describe_function`
  reflects tier changes after `alter_function` and after the background upgrade.
* Bench (second stage, PROC-BENCH-2): compile time at 1 KB to 1 MB, tier-up break-even, p99
  during a background compile, memory per tier, hash cost per MB.

## Implementation notes (PROC-1)

Deviations of this PROC-1 from the text above:

* (a) The contract check runs at admission only; the `Load` policy skips it.
* (b) Wire ops, builders, client and server changes are deferred to PROC-3b.
* (c) The server option for `AdmissionLimits` and the frame-cap check
  (`AdmissionLimits::validate_against_frame`) are deferred to PROC-3b: `shamir-db` cannot see
  the frame constant.
* (d) `check_function` returns `ModuleReport`; there is no separate verdict type.
* (e) Rows now carry `module_hash`, `abi`, `origin` and `optimize`. Legacy `wasm_hash` rows are
  verified at boot and are never rewritten by boot.
* (f) An unavailable function (unloadable or corrupt) is registered as a placeholder that fails
  closed, so `replace: true` repairs it and drop removes it.
* (g) "Composite hash written on the next write" is satisfied by create/replace; alter changes
  only `optimize`.
* (h) Admission now lives in its own runtime-free crate (`shamir-wasm-admission`, no
  wasmtime/tokio); the host re-exports the same public surface.
