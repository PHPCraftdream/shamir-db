# ADR PROC-0: WASM functions: module contract, upload/download, in-process compiler, DDL/OQL API

Status: **proposed, awaiting owner approval. No code starts before approval.** Covers three
works that share one contract: PROC-1 (upload/download plumbing), PROC-2 (in-process
compiler), PROC-3b (DDL/OQL API and builders). The two-tier compilation core (Winch then
Cranelift) is already implemented, see `wasm-two-tier-compilation.md`; this ADR only adds
the surface around it.

## Facts from the code (baseline)

* A guest module exports `memory`, `shamir_alloc(len: i32) -> i32` and
  `shamir_call(ptr: i32, len: i32) -> i64` (packed `(ptr, len)` result) and may import only
  `shamir_host.*` from `SANCTIONED_HOST_IMPORTS` (`batch_put`, `batch_get`, `global_set`,
  `global_get`, `call`, `db_get`, `db_insert`, `db_query`, `db_execute`, `raise_error`,
  `http_fetch`). `wasm_sanitizer.rs` enforces the import allowlist and rejects the component
  encoding before any compile; `wasm_function.rs` looks exports up by fixed literal names.
* `CreateFunctionOp` (`shamir-query-types`, `deny_unknown_fields`) carries `source` (Rust
  source, compiled by `cargo` on the host, gated by `Execute` on `WasmCompiler`) or `wasm`
  (base64). The catalogue row stores `wasm_b64`, `wasm_hash` (64-bit `FxHasher`, hex),
  `lang` (`"wasm"` or `"rust"`), `source`, `version` (constant 1), `kind`.
* There is no way to read a function back: no export, no describe, no alter. Validators
  persist the same row shape (`validator_management.rs`).
* Limits at call time: `WasmLimits` (fuel 1e9, memory 64 MiB, wall clock 30 s). There is no
  limit on module size, function count or section sizes at load time.

## Decisions to approve

| # | Decision | Recommendation |
|---|---|---|
| D1 | Module contract is versioned by an optional custom section `shamir.abi` | accept |
| D2 | Load-time limits and a feature allowlist before any compile | accept |
| D3 | Download returns the exact stored bytes; identity is a composite fast hash | accept |
| D4 | The new language compiles in the server process, no `cargo`, no subprocess | accept |
| D5 | Tier mode is a per-function property `optimize: auto \| now \| never` | accept |
| D6 | New ops `alter_function`, `describe_function`, `export_function`, `check_function`; `create_function` gains `lang`, `optimize` | accept |

## D1. Module contract

The contract above is ABI **v1** and is frozen as is. Additions:

* Custom section `shamir.abi`, payload: `u32 LE` ABI version. Absent section means v1.
  A version the host does not know is rejected at load with a message naming both versions.
  Future incompatible changes bump the number; compatible ones do not.
* Custom section `shamir.origin` (informational only): a short UTF-8 tag naming the producer
  (`rust-cargo`, `shamir-lang`, `external`). It never changes validation, limits or
  permissions: **every origin passes the same checks**. It is shown by `describe_function`.
* Both sections are optional, ignored by wasmtime, stripped by no step, and counted in the
  size limit. Unknown custom sections are allowed and ignored (toolchains add `name`,
  `producers`, DWARF).
* Entry point naming stays fixed: a function module has exactly one callable, `shamir_call`.
  Several logical procedures per module are not introduced here.

## D2. Load

Validation order (cheapest first, all before compilation, once per module, shared by both
tiers; nothing here runs at call time):

1. Raw size cap (`max_module_bytes`, default 8 MiB; a server option).
2. `wasmparser` pass collecting counts without compiling: functions, types, imports,
   exports, globals, tables, data segments, memories, locals per function, and the size of
   the largest function body. Each has a cap with a conservative default; exceeding any
   rejects the module naming the limit.
3. Feature allowlist: MVP plus sign-extension, non-trapping float-to-int, bulk memory,
   multi-value, mutable-global import/export; rejected: threads/atomics, SIMD (until the
   tier-parity tests cover it), reference types beyond the funcref table, GC, exceptions,
   tail calls, memory64, multi-memory, component model. The allowlist is a table in the
   code, mirrored by the `wasmtime` `Config` flags, with a test that the two agree.
4. Existing import allowlist and encoding check (unchanged).
5. Contract check: exports `memory`, `shamir_alloc`, `shamir_call` exist with the exact
   types; `shamir.abi` version known.
6. Compile budget: the fast-tier compile runs with a wall-clock cap
   (`max_compile_time`, default 10 s) on the blocking pool; on expiry the module is
   rejected. The optimized tier already runs in the background.

`check_function` (dry run) performs steps 1 to 5 and the fast compile, persists nothing, and
returns the structured report (limits hit, counts, ABI version, origin, composite hash).

## D3. Download and identity

* `export_function` returns the **exact bytes stored**, byte for byte. No recompilation, no
  re-encoding, no normalization. The row keeps `wasm_b64` as the source of truth; a source
  language (`lang`) additionally keeps `source` for `describe`/`export` of the text.
* **Round-trip invariant:** for any accepted module `m`, `export(create(m)) == m`, and a
  create from the exported bytes yields the same hash. Tested for each origin.
* **Identity hash:** a fixed array `[u64; N]` (N = 2 or 3) of fast, independent
  non-cryptographic hashes over the full bytes, replacing the single 64-bit `FxHasher`
  value. It is an identity and corruption check, not a security measure: no secret seed, and
  no mandatory byte-by-byte comparison on collision (owner decision). The wire/describe form
  is lowercase hex of the concatenated words; the legacy 16-hex `wasm_hash` of existing rows
  stays readable and is recomputed lazily on the next write (a row without the composite
  field is treated as "unknown", never as a mismatch).
* **When the hash is checked** (never on the call path): on create (computed), on first
  load of a row into memory, on export, and on replication/restore of a row. A mismatch on
  first load or export marks the function `corrupt`: calls fail with a clear error and
  `describe_function` reports it. The compile cache is keyed by hash plus tier strategy.

## D4. In-process language and compiler

* **Language:** a strictly typed, TypeScript-like subset: scalar types (`i32`, `i64`,
  `f64`, `bool`), `string`/`bytes` through the existing msgpack call convention, records,
  lists, `if`/`while`/`for`, calls, no closures at first, no dynamic typing. Details of
  grammar are a separate document at PROC-2 start; this ADR fixes only the constraints.
* **Compiler:** a Rust module in the server process: lex, parse, type-check, then emit
  wasm through `wasm-encoder`. The output goes through exactly the same load pipeline as an
  uploaded module (D2), with `shamir.origin = shamir-lang`. No subprocess, no filesystem, no
  `cargo`, no toolchain dependency: this closes the residual risk recorded in
  `wasm-untrusted-compile-sanitization-decision.md` for the new language. The Rust-source
  path (`lang = rust`) stays as is, behind the `WasmCompiler` `Execute` permission.
* **Compiler resource bounds:** source size cap, AST depth cap, a compile fuel counter
  inside the compiler, and the same wall-clock cap as D2 step 6; the compile runs on the
  blocking pool.
* **Async host calls** (`db_*`, `call`, `http_fetch`) need no code transformation: the host
  imports are already async wasmtime imports and suspend the guest fiber, so source code
  calls them as ordinary functions.
* **Staging:** MVP is scalars and validators (pure functions over parameters); then
  procedures with database access through the existing `shamir_host.db_*` imports.
  Optimization passes are out of scope; the Winch/Cranelift tiers do that.

## D5. Compilation mode

Per function: `optimize: auto | now | never`, default `auto`, mapped one to one to
`TierMode` (`Auto`, `Now`, `Never`). Stored in the catalogue row (`optimize` field, absent
means `auto`), changeable by `alter_function` without re-uploading the module. Engine-wide
`TierOptions` (`hot_calls`, `background_jobs`) stay server settings. `describe_function`
reports the current tier, upgrade counters and the configured mode.

## D6. Wire, DDL/OQL and builders

* `create_function`: new optional `lang` (`"wasm" | "rust" | "shamir"`, default inferred
  from which of `wasm`/`source` is present: `source` keeps meaning Rust for compatibility)
  and `optimize`. `deny_unknown_fields` stays, so old servers reject new fields loudly.
* New ops, each with a builder method in the Rust and TypeScript builders (never raw JSON):
  * `alter_function { name, optimize?, visibility?, ... }`: metadata only, no new module;
    permission `Write` on the function.
  * `describe_function { name }`: lang, origin, ABI version, composite hash, size, tier,
    optimize mode, counters, corrupt flag; no bytes. Permission follows the existing
    function read rules.
  * `export_function { name }`: bytes (base64) and, for source languages, the source text.
    Needs `Write` or an explicit new `Read` right on the function: **open point**, the
    default is the stricter `Write` because the bytes reveal logic of `Private` functions.
  * `check_function { wasm | source, lang }`: D2 dry run, needs only `Create` on
    `FunctionNamespace`.
* Existing hmac rules for `definer`/`secret_grants` apply unchanged to every language.
* Docs: a guide page in `docs/guide-docs/` and the protocol spec entries.

## Non-goals

No component model, no WASI, no multi-module linking, no hot reload of a running call, no
signing of modules, no migration of existing rows (they stay valid: `lang` and `optimize`
default), no change to fuel/epoch/memory limits.

## Risks

* Feature allowlist too strict for modules built by common toolchains: mitigated by the
  `check_function` report listing the exact rejected feature, and by a corpus test of real
  Rust-built modules.
* Composite hash is not collision-resistant against an adversary (accepted by the owner):
  nothing security-relevant may rely on it; permissions and `definer` checks never do.
* Compiler bugs produce invalid wasm: the output is validated by D2 like any upload, so a
  bug becomes a rejected create, not a runtime fault.

## Acceptance criteria

* PROC-1: limits and allowlist tests (one per limit and per rejected feature), ABI section
  tests, round-trip for each origin, composite hash tests, `check_function` writes nothing,
  corrupt-row behaviour, legacy rows readable.
* PROC-2: golden compile tests, differential tests against a reference interpreter on the
  scalar subset, rejected-source tests (caps, depth), no process spawn (asserted by a test
  that runs with an empty `PATH`).
* PROC-3b: wire round-trip tests for all new ops, builder tests in Rust and TypeScript,
  permission matrix tests, `describe_function` reflects tier changes after `alter_function`
  and after the background upgrade.
* Bench (second stage, PROC-BENCH-2): compile time at 1 KB to 1 MB, tier-up break-even, p99
  during a background compile, memory per tier, hash cost per MB.
