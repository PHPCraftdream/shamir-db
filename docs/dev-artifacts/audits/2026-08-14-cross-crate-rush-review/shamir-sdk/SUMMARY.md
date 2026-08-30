# shamir-sdk — Cross-Lens Review (all 7 lenses, synthesized)

Crate: `crates/shamir-sdk/` — the guest-authoring SDK for the WASM UDF ABI: the msgpack
`Value` mirror of the host `QueryValue`, `Params`/`Validation` DTOs, the `Ctx` capability
object (`db` / `call` / `http_fetch`), the `HttpRequest`/`HttpResponse` egress types, and
the `__rt` runtime helpers that all four `shamir-sdk-macros` attribute macros
(`#[scalar]` / `#[function]` / `#[procedure]` / `#[validator]`) generate calls into.

Review basis: synthesis of the seven 2026-08-14 lens reports under this directory —
`correctness-tdd.md`, `concurrency-lockfree.md`, `security-crypto.md`,
`performance-hotpath.md`, `api-wire-protocol.md`, `error-handling-lifecycle.md`,
`style-claude-md.md` — with the workspace
`docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/SUMMARY.md` (Per-crate
breakdown + Health Scorecard rows) used for counting context only. Structure/tone
calibrated on the two exemplar syntheses (`shamir-client-node/SUMMARY.md`,
`shamir-transport-ipc/SUMMARY.md`). This is a read-only consolidation, not a fresh
review — no build/test/lint commands were run and no source file was modified; cited
`file:line` references were spot-checked against the working tree and verified accurate
(e.g. `__rt.rs:36-61`, `host_imports.rs:97/106/131/146/162/183/207`, `http.rs:130-153`,
`db.rs:74-109`, `lib.rs:18`, test inventory). No new defects were found during
spot-checking.

**Counting convention:** findings are listed once under their *primary* lens with full
detail; where the same root-cause defect was flagged by multiple lens files, the other
lenses carry a one-line cross-reference stub ("*(dedup: primary N.M)*") and the defect is
counted once. The raw lens-tagged census (50: 0c / 6h / 15m / 19l / 10n) matches the
workspace SUMMARY.md row for this crate exactly.

## Executive summary

The pure-data core is in good shape — the msgpack `Value` mirror is pinned to byte-identity
with the host `QueryValue` by bidirectional conformance tests, and the concurrency pillar is
trivially clean (zero locks, atomics, or concurrent maps anywhere in the crate). The risk is
concentrated at the host-import boundary, which is the crate's entire reason to exist:
(1) **every decode of a host reply is fail-open** — seven sites silently convert protocol
violations into `Ok(vec![])` / `None` / `Value::Null`, so a corrupt reply is indistinguishable
from "no rows" / "absent"; (2) **`__rt::block_on` busy-spins forever on any genuinely
`Pending` future** behind a no-op waker, defended only by a stale "pure functions only"
comment that the crate's own non-pure API has invalidated; (3) **every host call leaks both
directions' msgpack buffers**, so loop-heavy procedures grow guest linear memory
O(cumulative traffic) until an opaque OOM trap. Behind those three: user errors are
flattened into `FunctionError::Compute` traps (the error taxonomy dies at the ABI boundary),
the HTTP egress boundary validates nothing (a CRLF/token-injection source), and
`http.rs`/`params.rs`/`db.rs`/`__rt` ship with zero tests. Fix the three boundary defects
before anything else ships from this crate.

---

## 1. correctness-tdd

### 1.1 — high — No tests for `http.rs` / `params.rs` / `db.rs` mapping / `__rt` ABI helpers (TDD protocol not followed for most of the crate)
- File:line: `crates/shamir-sdk/src/http.rs:24-44, 98-160`; `src/params.rs:26-93`;
  `src/db.rs:86-109`; `src/__rt.rs:11-30`; test inventory (verified): only
  `src/tests/value_tests.rs` + `src/tests/validation_tests.rs`, plus compile-only
  `tests/scalar_compile_pass.rs` / `tests/procedure_compile_pass.rs`.
- Issue: CLAUDE.md's development protocol is Red (failing test) → Green → Refactor. Every
  module listed contains non-trivial pure, host-testable logic with no test at all:
  `decode_fetch_envelope` (both `[true, map]` and `[false, msg]` branches plus the
  wrong-shape/wrong-flag/non-bool branches); `HttpRequest::to_value` (the exact wire map the
  host's `decode_http_request` depends on); `HttpResponse::from_value` (including its three
  silent-loss branches, finding 1.6); all `Params` typed getters and their error paths
  (`bytes`'s Str fallback untested; only happy paths covered at
  `value_tests.rs:400-411`); `__rt::decode_params`' non-map fallback; `__rt::leak_result`'s
  packed `(ptr<<32)|len` convention — the single most fragile cross-crate invariant (the host
  unpacks it at `shamir-wasm-host/src/wasm/wasm_function.rs:567-568`, but the packing side is
  never tested); and `encode_value`'s empty-vec fallback. `Table::insert`'s `Null → Err` and
  `Table::query`'s non-List → `Err` arms are fused to the panicking non-wasm stubs, so they
  are untestable by construction — there is no seam to substitute a fake import. The
  security-crypto lens adds: the egress response boundary — the code most exposed to remote
  data — is exactly the untested part. The error-handling lens adds: zero error-branch tests
  anywhere (all five `decode_fetch_envelope` error arms, the `from_value` leniency matrix,
  `unpack_ptr_len`'s 0-absent semantics, `decode_params`' malformed-input fallback).
- Also flagged by: security-crypto #10 (low — HTTP envelope coverage), error-handling-lifecycle #5 (medium — error-path coverage).
- Failure scenario: a getter regression (e.g. someone drops `Value::Str` acceptance from
  `bytes()`), a wire-shape drift in `to_value`, or a packing-convention change on either side
  of `leak_result` ships green; only host-crate or e2e tests could catch it. Envelope-shape
  or message drift in the exact paths findings 6.1/6.2 protect land silently.
- Suggested fix: add `src/tests/http_tests.rs`, `params_tests.rs`, `rt_tests.rs` (per the
  documented `tests/` layout) plus an error-path suite covering `decode_fetch_envelope`,
  `from_value`'s leniency matrix, and `Params` getter failures; introduce a minimal internal
  seam (fn-parameter or trait) for `db.rs`'s import calls so the Null/non-List mappings get
  red-test coverage.

### 1.2 — medium — *(dedup: primary 6.1)* — Fail-silent msgpack decode fallbacks in the wasm host imports
- Deduplicated into **6.1** (fail-open protocol handling), which carries the full write-up —
  all seven sites, the downstream `Ok(vec![])`/`None`/`Null` conflation, and the fix.
  Correctness-lens emphasis preserved there: the failure is on the crate's core read path and
  is today unreachable only because the shipped host always writes what it encoded — any
  host-side envelope evolution turns this into silent wrong answers.

### 1.3 — medium — `Ctx::call` accepts non-map `args`, but the host traps on them — contract enforced only on the far side, doc says "should"
- File:line: `crates/shamir-sdk/src/context.rs:78-88` (doc: "`args` **should** be a
  `Value::Map`" — verified); `src/host_imports.rs:121-132` (no validation); host side
  `shamir-wasm-host/src/wasm/host_call.rs:91-94` (`Params::from_value(... "call: params not
  a map")` → trap).
- Issue: the guest API accepts any `Value` and returns `Value` (not `Result`), doing no shape
  check. If a guest passes e.g. `Value::Int(5)` or a `Value::List`, the host import traps and
  the *entire calling function* dies with an uncatchable `FunctionError::Compute` at runtime.
  Nothing in the guest, and no test, pins this contract; "should" invites the bug.
- Failure scenario: `ctx.call("double", Value::Int(5))` compiles, passes every guest-side
  test (there are none), and only detonates in production inside the WASM runtime.
- Suggested fix: validate in `Ctx::call` (fail fast with a clear message at the call site)
  *and* strengthen the doc to "MUST be a `Value::Map`" with a `# Panics/Traps` section;
  add a doc example to `prelude.rs`.

### 1.4 — medium — *(dedup: primary 2.1)* — `__rt::block_on` is a guaranteed livelock for any future that yields `Pending`
- Deduplicated into **2.1** (spin-on-`Pending` executor), which carries the full write-up.
  Correctness-lens emphasis preserved there: the stale premise and the TDD angle — a test
  cannot pin a spin (it hangs), which is itself a gap to document next to the macro docs.

### 1.5 — medium — No compile-pass tests for `#[function]` and `#[validator]`; flagship doc examples never compiled anywhere
- File:line: `crates/shamir-sdk/tests/` (only `scalar_compile_pass.rs`,
  `procedure_compile_pass.rs` — verified); `src/lib.rs:5-13`, `src/prelude.rs:21-38` (all
  examples ` ```ignore `; `doctest = false` in `Cargo.toml:28`).
- Issue: half the exported macro surface (`function` — the crate's front-door example in the
  lib doc — and `validator`) has no expansion smoke test, unlike `procedure`/`scalar`.
  Combined with `doctest = false` + `ignore` fences, none of the documented usage in
  `lib.rs`/`prelude.rs`/`context.rs` is compile-checked by any target of this crate.
- Failure scenario: a signature change in the macro (e.g. arg-count check, return-type
  normalisation in `is_result_value_return`) breaks every real `#[function]` guest while
  this crate's suite stays green.
- Suggested fix: add `tests/function_compile_pass.rs` and `tests/validator_compile_pass.rs`
  mirroring the two existing files (separate integration-test crates so the `#[no_mangle]`
  symbols don't collide, per the existing files' own rationale). Optionally a `ui`-style
  compile-fail test for `#[scalar] fn x(ctx: Ctx, ...)`.

### 1.6 — low — `HttpResponse::from_value` silently drops data and truncates status
- File:line: `crates/shamir-sdk/src/http.rs:130-153` (verified: `*n as u16` at :134,
  header `filter_map` at :139-148, body fallback at :150-153).
- Issue: three fail-silent branches: (a) non-`Int` status → misleading "missing status field"
  error; (b) non-`Str` header values silently `filter_map`ped away; (c) non-`Bin` body (e.g.
  `Str`) silently becomes an empty body. Also `Value::Int(n) => Some(*n as u16)` truncates
  any out-of-range value (70_000 → 4_464; negatives wrap) instead of erroring. Two different
  strictness policies in one decoder: `status` is strict (missing → error), everything else
  is maximally lenient.
- Also flagged by: security-crypto #9 (nit), error-handling-lifecycle #10 (nit),
  api-wire-protocol #5 (within its envelope-codec finding).
- Failure scenario: currently unreachable from the shipped host
  (`encode_http_response`, `shamir-wasm-host/src/wasm/host_http.rs:86-97`, always writes
  `Int` status in range, `Str` headers, `Bin` body), but any envelope evolution turns into
  silent data loss in guest code at exactly the place guests consume untrusted remote data —
  and these are the untested branches (finding 1.1).
- Suggested fix: make (b)/(c) strict (`Err`) or explicitly documented as lossy; replace
  `as u16` with `u16::try_from(n)` mapping to `Err`.

### 1.7 — low — `Value` edge cases: `visit_u64` wrap-around and NaN/Infinity untestable under `PartialEq`
- File:line: `crates/shamir-sdk/src/value.rs:97-99` (`Ok(Value::Int(v as i64))` — verified),
  `:26` (`#[derive(PartialEq)]` over `F64(f64)`).
- Issue: (a) a msgpack `u64 > i64::MAX` silently wraps negative (unreachable from the host
  today — `QueryValue::Int` is `i64` — but reachable if a guest ever decodes third-party
  msgpack, e.g. an HTTP response body decoded as `Value`); (b) `Value::F64(NaN) !=
  Value::F64(NaN)`, so the otherwise-excellent `assert_bidirectional` conformance harness
  structurally cannot cover NaN/Inf F64 — the "every shared variant" claim in the `value.rs`
  doc is untested for exactly those inputs.
- Also flagged by: error-handling-lifecycle #11 (nit — the `visit_u64` half).
- Failure scenario: a guest that decodes an external payload stores a silently-wrapped id; a
  NaN round-trip regression would be invisible to the conformance suite.
- Suggested fix: for (a) document the wrap or use a checked conversion; for (b) add a NaN/Inf
  test that compares *bytes* (and decoded bit patterns) instead of relying on `PartialEq`.

### 1.8 — nit — Residual convention nits (the correctness lens's nit bundle, after dedup)
- File:line: `src/params.rs:96-109`.
- Issue: `impl Value { fn type_name }` is defined in `params.rs` rather than beside `Value` —
  bends the "one file = one primary export" rule (it is private, so cosmetic). The other
  three bullets of that bundle are deduplicated: the stale `tests/value_tests.rs` doc path →
  **7.3**; the missing builder-exception comment on `db.rs` → **5.1**; the hand-rolled
  `Error` struct → **6.2**.
- Suggested fix: one-line move/comment; no functional change.

---

## 2. concurrency-lockfree

Pillar verdict: **trivially compliant.** A full grep shows zero `std::sync::Mutex`/`RwLock`,
zero `parking_lot`, zero atomics, zero `scc`/`dashmap`/`ArcSwap`, and no hash-keyed
structures at all (`Params`/`Value::Map` are deliberately `Vec`-based, documented at
`src/value.rs:13-15`), so there are no locks across `.await`, no hot-path lock
justifications missing, and no `scc::*::len()` calls lacking an O(N) ack. The single genuine
concurrency-invariant concern:

### 2.1 — high — `__rt::block_on` busy-spins forever on `Poll::Pending` with a no-op waker; the "pure functions only" guard is a stale comment, not an enforced invariant
- File:line: `crates/shamir-sdk/src/__rt.rs:36-61` (no-op waker vtable :38-46; spin branch
  :53-58 — verified); consumed by all four macro expansions
  (`shamir-sdk-macros/src/lib.rs:144, 264, 391, 556`).
- Also flagged by: correctness-tdd #4 (medium), security-crypto #1 (medium — untrusted-guest
  DoS framing), performance-hotpath #3 (medium — infinite per-op CPU), error-handling-lifecycle #9 (low — "documented slice-3 limitation"), style-claude-md #2 (low — the stale-comment facet). This is one of the workspace SUMMARY's headline findings for this crate.
- Issue: `block_on` drives a future with a `Waker` whose `wake`/`wake_by_ref` are no-ops
  and, on `Poll::Pending`, enters `core::hint::spin_loop()` forever. Its doc comment claims
  correctness because "pure functions (the only kind this slice supports) are `Ready` on the
  first poll," while the `Pending` branch concedes "If a future genuinely needs async I/O
  (slice 4 host imports), this will spin. For now, a tight loop is correct." That deferral
  has aged out on two fronts. First, all four attribute macros route through `block_on`, and
  user code inside a `#[function]`/`#[procedure]` may `.await` anything (a channel receiver,
  a hand-rolled `Pending` future, a timer) — the SDK explicitly advertises "plain async
  Rust" authoring. Second, slice-4 host imports exist and are wired `func_wrap_async` on the
  host (`wasm_function.rs:195-213`), so they suspend *below* the poll and never surface as
  `Pending` — the load-bearing fact (every current import is a synchronous FFI) is true but
  the comment never says it. What *does* return `Pending` is any guest-local await — and
  then the guest spins at 100% CPU with **no progress mechanism whatsoever** (the waker can
  never schedule anything) until the host's epoch-interruption / top-level wall-clock
  deadline kills it (`wasm_function.rs:542-559`), having burned the entire fuel budget for a
  generic deadline error. On the host target ("also works on the host target for testing",
  `lib.rs:15-16`) there is no epoch/timeout: a single such future hangs the native test
  runner until nextest's 180 s kill. Per CLAUDE.md's "hangs are BUGS — hunt them, never
  tolerate" doctrine, an unbounded-spin wedge is exactly the banned bug class. The `Pending`
  branch has zero test coverage (`value_tests.rs:414-417` exercises only the Ready path), so
  CI would not catch the regression path either.
- Failure scenario: a guest author writes `#[procedure]` awaiting a
  `tokio::sync::oneshot`/timer (plausible — that is the advertised authoring model). Every
  invocation burns full fuel + hits the wall-clock deadline with an unhelpful error; a
  host-target unit test of that function deadlocks the suite (180 s `TIMEOUT`); under host
  concurrency every such guest pins a thread at 100% CPU and the host's executor starves.
- Suggested fix: enforce the invariant instead of commenting it — poll once, and on
  `Poll::Pending` call `__rt::trap("guest future yielded Pending — host imports suspend
  transparently; guest-local awaits are unsupported")` so the misuse fails fast as a
  catchable `Compute` error rather than hanging the host (a no-op waker can never make
  progress, so fail-fast is the honest WASM primitive; a real park only makes sense on the
  host-testing target, e.g. a `thread::park`-backed waker there). Delete/replace the stale
  "pure functions (the only kind this slice supports)" premise with the real invariant
  ("all four generated kinds run through `block_on`; every host import is a synchronous
  extern call, so no body yields `Pending` — a spin on `Pending` is a programming bug, not a
  wait") and surface it where authors read (`context.rs`/`prelude.rs`/macro docs). Add a test
  asserting the chosen `Pending` behavior. Revisit with a schedulable waker when async host
  imports land.

### 2.2 — nit — *(dedup: primary 4.4)* — O(N) linear scans for keyed lookups — consciously acked
- Deduplicated into **4.4** (`Params::get` / `from_value` scans), which carries the detail.
  Concurrency-lens contribution preserved there: the pattern was checked against pillar 3 and
  *consciously accepted* — no concurrent structures exist in this crate, the
  Vec-over-IndexMap choice is documented (`value.rs:13-15`), and hashing short keys would
  likely be slower at single-digit N. Listed in that lens so the audit trail shows
  acceptance, not a miss.

---

## 3. security-crypto

Scope note: this crate contains no auth, HMAC, SCRAM, or TLS code (those live in
`shamir-connect` / `shamir-server` / `shamir-db`); its security surface is the guest-host
WASM ABI (`__rt`, `host_imports`) and the HTTP-egress request builder (`http`). No-finding
notes from the lens: no timing side-channels (nothing compares secrets); `Value`'s recursive
`Deserialize` is depth-bounded by rmp-serde's default recursion limit (no guest-stack blowup
via deeply nested host payloads); the host side of the ABI is properly defensive in both
directions (`read_guest_mem` bounds-checks guest-provided `ptr`/`len`;
`wasm_function.rs:567-574` validates the guest result pointer before slicing) — the
guest-side gap in 3.2 is the mirror image, not a live hole.

### 3.1 — medium — `HttpRequest` performs zero validation of method / URL / header name / header value (CRLF & token-injection source at the egress boundary)
- File:line: `crates/shamir-sdk/src/http.rs:80-95` (`header`, `method`, `body` setters);
  consumed by `Ctx::http_fetch` at `context.rs:116-119` (verified).
- Issue: the SDK is the boundary through which fully guest-controlled strings enter egress,
  yet it imposes no constraints at all. Downstream today is the curl gateway
  (`shamir-db/src/shamir_db/curl_gateway.rs:71-89`), whose `escape_curl_value` (:210-220)
  escapes only the backslash and the double-quote characters — CR, LF, and other control
  characters pass verbatim into curl-config `header = "name: value"` / `request = "method"`
  quoted strings, and from there into curl's `-H`/`-X` arguments. Whether an embedded LF
  becomes extra proxied headers or a smuggled request line depends on the installed curl
  version's `-H`/`-X` parsing — a boundary must not depend on that. Any future host
  implementation (e.g. a raw-socket writer instead of curl) inherits the gap silently, and
  nothing in the SDK's docs states the host contract.
- Failure scenario: a guest sets `header("X-Trace", "a\r\nHost: internal-admin")` or
  `method("GET\r\nX-Priv: 1")`; on a curl build that tolerates embedded CRLF, the proxied
  request gains attacker-chosen headers/request line, reaching targets already passed by the
  allowlist/SSRF guard.
- Suggested fix: validate at construction in `HttpRequest` — reject `\r`, `\n`, `\0` in
  method, URL, header names and values (optionally restrict method to RFC token characters,
  URL to `http`/`https`) — and document the invariant. Keep the host-side guard too (defense
  in depth), and add a strip/escape-CRLF step in `escape_curl_value`.

### 3.2 — low — Guest ABI builds slices from host-returned `(ptr, len)` with no sanity check (undocumented unsafe trust assumption)
- File:line: `crates/shamir-sdk/src/host_imports.rs:96, 105, 130, 145, 161, 182, 206, 224`
  (all via `unpack_ptr_len` :70-77) — verified.
- Issue: every host-import result path executes `core::slice::from_raw_parts(ptr, len)`
  directly on the packed `i64`. A negative `len` (host bug / ABI drift) becomes a
  ~4-billion-byte slice — instant UB in the guest; the per-site "Safety:" comments assert
  the invariant but nothing checks it. The trusted-host model is legitimate (and the host's
  mirror function does validate: `read_guest_mem`, `wasm_function.rs:297-313`, rejects
  negative and out-of-bounds pairs), but the guest side is where the `unsafe` lives and it
  is entirely unguarded.
- Failure scenario: a future host change returns a malformed packed pair (e.g. an error code
  stuffed into the low bits with the high bits zeroed, or a negative length); the guest
  constructs a wildly out-of-bounds slice instead of cleanly reporting "absent"/`Null`.
- Suggested fix: centralize one `guest_slice(packed) -> Option<&[u8]>` helper that rejects
  `ptr <= 0 || len < 0` (and, ideally, `ptr + len` overflow via checked math) before
  `from_raw_parts`; all eight call sites shrink to one audited unsafe block.

### 3.3 — low — *(dedup: primary 6.1)* — Encode/decode failures silently swallowed, fail-open: a failed filter encode degrades to "query ALL rows"
- Deduplicated into **6.1** (fail-open protocol handling). Security-lens emphases preserved
  there: `encode_value` maps rmp-serde failure to `Vec::new()`, and in
  `Table::query(Some(f))` a zero-length buffer means "no filter" per the ABI — i.e. the host
  returns the **whole table** where the author asked for a filtered subset (over-exposure
  inside the function's own actor permissions), the fail-open *direction* that makes this
  family security-relevant and not merely a debuggability problem.

### 3.4 — low — Documented scalar "purity guarantee" is a token-match lint, trivially bypassed by a type alias
- File:line: enforcement in `crates/shamir-sdk-macros/src/lib.rs:425-432`
  (`type_contains_ctx`); the guarantee is asserted in this crate's docs at
  `src/context.rs:15-16`, `src/db.rs:17-18`, `src/prelude.rs:14`.
- Issue: `#[scalar]` rejects only argument types whose *token string* contains the exact
  segment `Ctx`. `type CtxAlias = shamir_sdk::Ctx;` followed by
  `fn f(p: Params, c: CtxAlias)` passes the check, and the generated scalar export happily
  wires a `Ctx::new()` — the alias path hands a "pure" scalar the full capability object
  (`db`, `call`, `http_fetch`). The real boundary is host-side per-invocation gating (db
  gateway only via `invoke_function_in_db`; missing net gateway traps — `context.rs:92-94`,
  `wasm_function.rs:138-142`), which stays intact, so this is a
  documented-guarantee-vs-mechanism gap, not an exploitable escalation — but the docs
  present it as a guarantee, which will mislead security reviewers and guest authors alike.
- Failure scenario: a marketplace "pure scalar" uses the alias to read globals/db when a
  misconfigured host wires the gateway for all kinds; reviewers who trusted the documented
  guarantee have no second line of defense in the SDK.
- Suggested fix: either enforce structurally (scalars receive a zero-capability token type;
  only `#[procedure]`/`#[function]` generation constructs the capability-carrying `Ctx`) or
  reword the docs in this crate and the macros to "compile-time lint; runtime isolation is
  enforced by host gateway wiring", and add a macros test covering the alias bypass.

### 3.5 — low — *(dedup: primary 6.2)* — User `Err` results surface as WASM panics mapped to `FunctionError::Compute`
- Deduplicated into **6.2** (error taxonomy + trap transport), which carries the full
  write-up. Security-lens emphasis preserved there: crash-vs-user-error blurring is bad for
  audit logs and retry policy, and `__rt::trap` (= `panic!`, deviating from CLAUDE.md's
  "avoid `panic!` outside invariant violations") is the mechanism — the fix belongs to the
  result-envelope transport.

### 3.6 — low — `leak_result` truncates pointers on 64-bit host targets (latent UB in the host-testing ABI path)
- File:line: `crates/shamir-sdk/src/__rt.rs:25-30` (verified: `as usize as u64 << 32` at
  :27, `len & 0xFFFF_FFFF` mask at :29).
- Issue: the packing assumes 32-bit pointers. It is only correct under `wasm32`; on the
  x86_64 host target — which this crate explicitly supports for testing (`lib.rs:15-16`) and
  on which the macro-generated `shamir_call` also compiles — the upper 32 bits of the real
  pointer are silently dropped, so any host-side consumer unpacking `(packed >> 32)` gets
  garbage and `from_raw_parts` on it is UB. Additionally the length is masked
  (`len & 0xFFFF_FFFF`), silently corrupting the packed result for a >4 GiB buffer (the
  error-handling lens flagged this half as a nit). Today the only unpacker is the wasm host
  (`wasm_function.rs:567`) where guest pointers are genuinely 32-bit, so this is latent —
  but nothing gates the function to `wasm32`.
- Also flagged by: error-handling-lifecycle #11 (nit — the len-mask half).
- Failure scenario: someone writes a host-target test harness that calls the generated
  `shamir_call` and unpacks the packed result; dereferencing the truncated pointer crashes
  or corrupts the test process.
- Suggested fix: `#[cfg(target_arch = "wasm32")]`-gate `leak_result` (mirroring
  `host_imports`' `host_only()` panic on other targets), or build the packed value from
  explicit `as u32` casts / `debug_assert!` preconditions (ptr fits 32 bits, len fits 32
  bits) so the truncation is declared rather than accidental and violations fail in
  host-target tests.

### 3.7 — nit — *(dedup: primary 4.1)* — Unbounded per-call buffer leaks undocumented as a resource budget
- Deduplicated into **4.1** (ABI buffer leaks), which carries the full write-up including
  the `shamir_alloc` leak-everything allocator and the documentation remedy.

### 3.8 — nit — *(dedup: primary 1.6)* — `HttpResponse::from_value` truncating status cast and lenient header/body parsing
- Deduplicated into **1.6**, which carries the detail (security-lens framing: host-controlled
  input, so not directly exploitable — the risk is hiding wire-format drift at exactly the
  place guests consume untrusted remote data).

### 3.9 — low — *(dedup: primary 1.1)* — Security-relevant HTTP envelope decoding has zero test coverage
- Deduplicated into **1.1** (the crate-wide TDD gap). Security-lens emphasis preserved
  there: the boundary parsing most exposed to remote data (`[ok, payload]` envelope,
  error-message extraction, status/headers/body coercion) should not be the untested part;
  the suggested `http_tests.rs` case list (ok/failure envelopes, non-List/wrong-shape,
  missing status, non-string headers, status out of `u16` range, empty body) is folded into
  1.1's fix.

---

## 4. performance-hotpath

The hot paths are the msgpack (de)serialization around every host-import call and the guest
linear-memory lifecycle that goes with them. The dominant theme is memory, not CPU: every
host-import call leaks its outbound msgpack buffer, and the host-returned buffer is never
reclaimed either (finding 4.1). CPU-level linear scans exist but are small-N (4.4). Tests
cover wire conformance and validation only; nothing exercises the host-import memory
lifecycle, and the crate has no benches.

### 4.1 — high — Host-import ABI leaks both directions' buffers on every call — unbounded guest linear-memory growth in loops
- File:line: `crates/shamir-sdk/src/host_imports.rs:60-66` (`encode_leak`, `mem::forget` at
  :64 — verified), leak call sites 82, 112, 124, 142, 155, 174, 200; host-returned buffers
  read via `from_raw_parts` at 96, 105, 130, 145, 161, 182, 206, 224 and never freed; the
  one-shot result leak is `__rt::leak_result` (`src/__rt.rs:25-30`); the guest's
  `shamir_alloc` is itself a leak-everything bump allocator
  (`shamir-sdk-macros/src/lib.rs:105-114`, allocator wiring :238-244); the extern block
  (:29-53) has no dealloc import anywhere in the ABI.
- Also flagged by: error-handling-lifecycle #3 (medium — no free path on the error path
  either), security-crypto #8 (nit — undocumented as a per-call resource budget). This is
  one of the workspace SUMMARY's headline findings for this crate.
- Issue: `encode_leak` `core::mem::forget`s a fresh `Vec<u8>` on every `batch_put` /
  `global_set` / `call` / `db_get` / `db_insert` / `db_query` / `http_fetch`, and every
  buffer the host returns via `shamir_alloc` is likewise abandoned after decoding. The
  inline justification ("the Store is dropped after `shamir_call` returns", :55-59) bounds
  the leak **per invocation**, not per call — within a single invocation growth is O(total
  bytes transferred), violating pillar 3 (per-op cost must trend toward constant). Guest
  linear memory is a hard-capped 32-bit space, so exhaustion is reachable with realistic
  data volumes; there is no `shamir_dealloc` import or export anywhere to reclaim anything,
  on success or error paths.
- Failure scenario: a `#[procedure]` bulk-load — `for doc in docs {
  ctx.db().table("users").insert(doc)?; }` with 100k × 1 KiB docs — leaks ~100 MB
  request-side plus the same again response-side, tripping wasm allocation failure / OOM
  trap mid-batch, far from the root cause. A per-row `batch_put` scratchpad loop in a
  `#[function]` behaves identically; a paginating `db_query` loop leaks both directions
  monotonically.
- Suggested fix: make per-call memory transient — (a) keep one reusable scratch buffer in a
  `static Cell<Vec<u8>>` (the wasm32 guest is single-threaded here), resize-and-overwrite
  per call, hand the host its ptr/len — bounded at max-seen message size; or (b) export a
  `shamir_free(ptr, len)` the host calls after its synchronous read (both directions); or
  (c) a bump arena reset between host calls. At minimum, document the per-call leak as an
  invocation-lifetime budget so authors know loops are the hazard (and see 4.2 for the
  query-specific amplifier).

### 4.2 — medium — `Table::query` has no limit/pagination — the whole result set is buffered twice and retained
- File:line: `crates/shamir-sdk/src/db.rs:98-109`; ABI side
  `crates/shamir-sdk/src/host_imports.rs:170-184`; advertised pattern in
  `crates/shamir-sdk/src/prelude.rs:34-37`.
- Issue: the ABI returns one packed `(ptr, len)` blob that the guest decodes into a full
  `Vec<Value>`, and the SDK exposes no `limit`/`offset`/cursor parameter — the only bound a
  guest author has is whatever their filter achieves. `query(None)` materializes the entire
  table twice (host-side contiguous msgpack buffer + guest-side `Vec<Value>` tree), and per
  finding 4.1 both copies are also leaked for the invocation's lifetime. Combined cost is
  ~2× result size, retained.
- Failure scenario: the prelude's own example (`let rows =
  ctx.db().table(params.str("table")?).query(None)?;`) against a million-row table: host
  builds a multi-hundred-MB blob, guest decodes an equal-size structure, then OOM-traps.
- Suggested fix: add `limit`/keyset-cursor parameters to the `db_query` ABI (and a
  `Table::query_paginated` / iterator-style API), or chunked/streaming returns. If the ABI
  can't change soon, document the unbounded-buffering contract loudly on `Table::query` and
  in the prelude example.

### 4.3 — medium — *(dedup: primary 2.1)* — `block_on` busy-spins forever on `Pending` — unbounded CPU burn
- Deduplicated into **2.1**. Perf-lens framing preserved there: this is the degenerate
  O(x→0) case — per-op CPU cost is infinite instead of constant — and the fix options
  (trap-on-Pending, or a real waker via a host import) are unchanged.

### 4.4 — low — `Params::get` linear-scans the parameter map on every typed access; `bytes()` clones the payload
- File:line: `crates/shamir-sdk/src/params.rs:26-32` (scan), `params.rs:68-77` (clone in
  `bytes`); also `http.rs:130-153` (three sequential `map.iter().find` passes per parsed
  response).
- Also flagged by: concurrency-lockfree #2 (nit — explicitly acked: no concurrent structures
  exist, the Vec-over-IndexMap choice is documented at `value.rs:13-15` for guest-binary
  dependency avoidance, and hashing short keys would likely be slower than the linear scan
  at these sizes; listed so the pillar-3 audit trail shows conscious acceptance).
- Issue: every `params.i64(..)` / `str(..)` / `bytes(..)` is an O(P) `iter().find()` over
  `Vec<(String, Value)>`; a function reading M params pays O(P×M) per invocation, and each
  miss additionally allocates an error `String`. `bytes()` also clones the whole
  `Vec<u8>`/str payload per call. It is exactly the "repeated lookups / full scans in
  helpers" pattern pillar 3 names, with no comment acknowledging the accepted cost.
- Failure scenario: none at documented sizes; visible only if P or per-invocation accessor
  counts grow large (e.g. row-mapping functions reading 10+ params per record).
- Suggested fix: keep the `Vec` but do one O(P) indexing pass in `decode_params` (sorted key
  index or a tiny Fx-hash map, per pillar 4) so lookups are O(1); or at least add a comment
  recording the accepted small-N cost. Optionally add a consuming `take_bytes` variant to
  avoid the clone for `Bin`.

### 4.5 — low — HTTP path double-copies payloads and triple-scans the response map
- File:line: `crates/shamir-sdk/src/http.rs:98-111` (`to_value` clones method, url,
  headers, and the **entire body** — verified), `http.rs:130-153` (`from_value` does three
  separate linear `find` passes and clones every header string plus the body),
  `context.rs:116-119` (`http_fetch` builds an intermediate `Value::Map` then msgpack-encodes it).
- Issue: each `http_fetch` copies the request body twice (once into the intermediate
  `Value::Bin`, once into msgpack bytes, which is then leaked per finding 4.1), and the
  response map is scanned three times (status / headers / body) with full clones instead of
  one ownership-moving pass. Per-call overhead is O(body bytes) extra allocations on top of
  the unavoidable encode.
- Failure scenario: a multi-MB POST (file upload) holds 3–4 simultaneous copies of the body
  in guest linear memory; irrelevant for small JSON calls but measurable for binary payloads.
- Suggested fix: make `HttpRequest::to_value` consuming (`into_value(self)`) and have
  `Ctx::http_fetch` use it — the borrowed path has no other callers; fold `from_value` into
  a single pass over the map, moving `body`/header strings out instead of cloning.

### 4.6 — nit — `Db::table` allocates a fresh `String` per handle
- File:line: `crates/shamir-sdk/src/db.rs:50-54`.
- Issue: `ctx.db().table("users")` clones the table name into a new `String` each call; a
  row-loop that re-opens the handle per iteration re-allocates it every time (and finding
  4.1 makes that pattern likely, since the handle itself is cheap to recreate).
- Failure scenario: none material — a few bytes per iteration.
- Suggested fix: none needed; if loops are the common shape, show hoisting
  `let users = ctx.db().table("users");` outside the loop in the docs (the `db.rs:22-38`
  example already implies it).

---

## 5. api-wire-protocol

The msgpack `Value` mirror and the `Validation` ABI are genuinely well done: byte-identity
with the host `QueryValue` is enforced by bidirectional conformance tests, and the validator
result shape is pinned against the engine's `decode_validation_result`. The weak spots are
the non-gated raw-`Value` filter surface (5.1/5.2/5.8), the unversioned wire path (5.4), and
the HTTP envelope's duplicate-header collapse (5.5).

### 5.1 — high — Raw-`Value` filter surface (`Table::get`/`Table::query`) bypasses the builder-only rule, with no required exception comment; unsupported filter values silently coerce to `FilterValue::Null`
- File:line: `crates/shamir-sdk/src/db.rs:5-9, 79-109`; host counterpart
  `crates/shamir-db/src/shamir_db/shamir_db/db_gateway.rs:41-85`.
- Also flagged by: correctness-tdd #8 (nit — the missing "why no builder" comment that
  CLAUDE.md requires wherever the builder does not apply).
- Issue: CLAUDE.md ("Query construction — builder only") mandates that all queries/filters go
  through the query builder, forbids hand-assembling a filter/wire op from raw `Value`, and
  requires a one-line comment stating *why* wherever the builder genuinely does not apply.
  `Table::get(key: Value)` and `Table::query(Option<Value>)` are a non-feature-gated query
  surface built from raw `Value`s whose semantics live in an ad-hoc host-side
  mini-interpreter (`FacadeDbGateway::key_to_filter`): map → conjunction of `Eq`, scalar →
  `Eq` on `"id"`, empty map → match-all. No comment in the SDK justifies the builder
  exemption, and the crate docs (db.rs module doc, `Ctx` examples) present this as a
  first-class query API. It also silently diverges from the builder path that exists in the
  same crate (`Db::execute`, feature `query-builder`): no comparison ops, no projection, no
  pagination (host hardcodes `Pagination::None` and `Temporal::Latest`), and unsupported
  filter values (Dec/Big/List/Set/nested Map) are silently coerced to `FilterValue::Null`
  (`db_gateway.rs:49-52`) rather than rejected.
- Failure scenario: a guest filters on a decimal or list-typed field; the filter silently
  becomes `Null` and the query returns wrong/empty results with no error. Or an author
  assumes builder semantics (limits, ordering) and gets an unbounded scan.
- Suggested fix: either deprecate the raw filter parameters in favor of the `query-builder`
  path (route `Table` through `Batch` + builder ops internally), or keep it but add the
  required justification comment, document the key convention (including empty-map =
  match-all and the `Null` coercion) in `db.rs`, and make the host error out on unsupported
  filter value types instead of coercing to `Null`.

### 5.2 — medium — `Table::get` with an empty `Value::Map` returns the table's first record
- File:line: `crates/shamir-sdk/src/db.rs:74-81`; host `db_gateway.rs:62-67`.
- Issue: the SDK never checks the key; the host maps an empty map to "no filter"
  (match-all), and `get` returns `records.first()`. This convention is documented only in
  host code, not in the SDK's public docs.
- Failure scenario: a guest builds its key from request params that turn out to be empty
  (missing fields, empty `Value::Map` payload); "get by primary key" silently returns an
  arbitrary (first) row and the function proceeds operating on the wrong record — e.g.
  re-validating or returning another tenant's/user's document.
- Suggested fix: reject an empty-map key in `Table::get` (SDK-side `Error::user` before
  crossing the ABI) or have the host return an error for empty-map keys on `get`; at minimum
  document the edge in `db.rs`.

### 5.3 — medium — *(dedup: primary 6.1)* — Decode failures silently coerced into success values
- Deduplicated into **6.1** (fail-open protocol handling), which carries the full write-up.
  API-lens emphasis preserved there: the coercion makes host/guest wire bugs *nearly
  undebuggable* — "only the `packed == 0` sentinel legitimately means absent" is the
  invariant the fix must restore.

### 5.4 — medium — No wire-format versioning or capability negotiation on `Db::execute` (or the guest ABI)
- File:line: `crates/shamir-sdk/src/db.rs:135-148`, `crates/shamir-sdk/Cargo.toml:14-18`;
  host `db_gateway.rs:285-294`.
- Issue: `BatchRequest`/`BatchResponse` cross the ABI as bare msgpack with no version tag,
  magic, or handshake; the `shamir_host` import set itself carries no version either. Serde
  ignores unknown fields and defaults missing `Option`s, so *additive* changes decode "fine"
  while renames/removals silently change semantics. Compiled guests are persisted and run
  against a host that upgrades independently — exactly the deployment model where this bites.
- Failure scenario: a function compiled against sdk 0.1.0-alpha.1 runs on a host upgraded a
  year later; a renamed `BatchRequest` field (e.g. `return_only`) decodes as `None` and the
  batch silently returns a different result set instead of failing loudly.
- Suggested fix: add an explicit `protocol_version` field (or a header byte/import) checked
  on both sides, failing closed with a clear error on mismatch; document the wire-compat
  policy for the alpha. The mitigating factor today is that both sides live in one repo,
  but that will not hold once guests are compiled and stored.

### 5.5 — medium — Duplicate headers collapse on the wire (last-wins in both directions); the envelope shape is hand-mirrored between two crates
- File:line: `crates/shamir-sdk/src/http.rs:24-44, 54, 98-111, 124-160`; host
  `crates/shamir-wasm-host/src/wasm/host_http.rs:20-97`.
- Issue: `HttpRequest::headers` is a `Vec<(String, String)>` that permits duplicate names,
  but the wire shape is a msgpack map: the host decodes into an `IndexMap` (last value wins,
  one header silently dropped), and response headers are deduped the same way. The envelope
  shape itself is duplicated by hand between the two crates' doc comments as the only
  contract. (The *zero-tests* half of this lens finding is deduplicated into **1.1**; the
  `from_value` strictness half into **1.6**. The workspace SUMMARY also catalogues the
  host-side half of this defect — `Set-Cookie` loss on both directions — under
  shamir-wasm-host.)
- Failure scenario: a guest adds `Authorization` twice (or a retry loop appends it); one
  wins silently and upstream auth fails mysteriously. A future refactor of either side's
  envelope drifts undetected because nothing pins the bytes.
- Suggested fix: reject duplicate header names in `HttpRequest::header` (or define last-wins
  explicitly in the docs), and pin the envelope with msgpack round-trip tests against the
  host's exact shape (mirroring `value_tests.rs`) — the test half closes with 1.1's suite.

### 5.6 — low — *(dedup: primary 6.2)* — `Error` is an unkind-ed message string; internal protocol failures constructed as "user" errors
- Deduplicated into **6.2** (error taxonomy), which carries the full write-up. API-lens
  emphasis preserved there: callers cannot branch (retry on transport vs. surface to user),
  and the misleading "returned null" message hides the real `packed == 0` case; the host only
  consumes the stringified message, so a `thiserror` enum changes no wire encoding.

### 5.7 — low — `pub mod __rt` contradicts its own "not part of the public SDK surface" doc
- File:line: `crates/shamir-sdk/src/lib.rs:18` (verified); `crates/shamir-sdk/src/__rt.rs:1-3`.
- Also flagged by: style-claude-md #4 (nit — the same contradiction, rustdoc/public-api framing).
- Issue: `__rt` is declared `pub` with `pub fn decode_params/encode_value/leak_result/trap`,
  which makes them semver-public and rustdoc-visible while the module doc claims the
  opposite. They do need to stay `pub` (the proc-macro-generated code lives in *consumer*
  crates), but as-is the crate accidentally commits them to its public API.
- Failure scenario: tooling (rustdoc, `cargo public-api`-style checks, or a reviewer triaging
  "is this a breaking change?") treats `__rt` as a supported public API; users
  `use shamir_sdk::__rt::leak_result` and get no signal it's off-limits. (The workspace
  SUMMARY also lists `pub mod __rt` under its dead/unwired-public-API systemic pattern.)
- Suggested fix: `#[doc(hidden)] pub mod __rt;` plus an explicit semver-exemption note in
  the module doc ("public only so macro-generated code can reach these paths; not for direct use").

### 5.8 — low — Dec/Big down-level to `Str` on the wire — guests get silent permanent no-matches when re-filtering on those fields
- File:line: `crates/shamir-sdk/src/value.rs:8-11`; interaction with `src/db.rs:74-79` and
  host `db_gateway.rs:47-52`.
- Issue: the lossy Dec/Big → `Str` mapping is documented for *reads* ("lossy but stable"),
  but its interaction with the key convention is not: a decimal field read from a record
  arrives as `Value::Str("123.456")`; passing it back inside a `get`/`query` key produces
  `FilterValue::String`, which never equals the decimal column value host-side — a silent,
  permanent no-match. Without the (feature-gated) builder the guest has no typed way to
  express a decimal predicate.
- Failure scenario: the natural "fetch record, then `get` by one of its own fields" pattern
  returns `None` forever for decimal/bigint keys; the author concludes the data is missing.
- Suggested fix: document the trap in `Table::get`/`query` docs; longer term, funnel
  filtering through the builder (finding 5.1) where decimal predicates are typed.

### 5.9 — nit — Guest ABI passes pointers as signed `i32`; host rejects addresses ≥ 2 GiB
- File:line: `crates/shamir-sdk/src/host_imports.rs:60-66, 79-86`; host
  `crates/shamir-wasm-host/src/wasm/wasm_function.rs:302-304`.
- Issue: `encode_leak` casts `bytes.as_ptr() as i32`, which is negative for guest addresses
  ≥ `0x8000_0000`, and the host's `read_guest_mem` rejects `ptr < 0`. Harmless for typical
  (≤ 2 GiB) linear memories, but every host import fails on a 2-4 GiB guest memory even
  though wasm32 addresses are unsigned by construction.
- Failure scenario: none at current memory limits; a large-memory guest hits opaque host
  rejections on every import.
- Suggested fix: on the host, reinterpret as `u32` (`(ptr as u32) as usize`) before the
  bounds check, or document a 2 GiB linear-memory limit as part of the ABI contract in
  `wasm_function.rs`.

---

## 6. error-handling-lifecycle

The pure-Rust surface (`Params`, `Validation`, the http envelope decoder) follows the
documented `Result`/`?` discipline well, but the host-import boundary does not. Resource
lifecycle on the error path has no reclaim story at all (6.1 → 4.1), and error paths are
essentially untested (→ 1.1).

### 6.1 — high — Fail-open protocol handling: decode/encode failures silently become plausible success values (7 decode sites + `decode_params` + `encode_value` + `Table::insert`'s message)
- File:line: `crates/shamir-sdk/src/host_imports.rs:97` (`batch_get`, `.ok()`), `:106`
  (`global_get`, `.ok()`), `:146` (`db_get`, `.ok()`), `:131` (`call`,
  `.unwrap_or(Value::Null)`), `:162` (`db_insert`, `.unwrap_or(Value::Null)`), `:183`
  (`db_query`, `.unwrap_or(Value::List(Vec::new()))`), `:207` (`http_fetch`,
  `.unwrap_or(Value::Null)`) — all verified; `src/__rt.rs:11-16` (`decode_params` → empty
  `Params`), `:19-21` (`encode_value` → empty vec); `src/db.rs:86-92` (`insert`'s
  "db_insert returned null"), `:103-104` (`query`'s `Ok(items)` on a decoded `Value::List`);
  `host_imports.rs:170-177` (`db_query` len-0 == no filter).
- Also flagged by: correctness-tdd #2 (medium), api-wire-protocol #3 (medium),
  security-crypto #4 (low — the over-exposure direction). This is one of the workspace
  SUMMARY's headline findings ("fail-open decode") for this crate.
- Issue: every host-import response goes through
  `rmp_serde::from_slice(bytes).ok()` / `.unwrap_or(...)`, so a decode failure of
  host-written bytes is silently swallowed instead of propagated, contrary to CLAUDE.md's
  error rules ("Return `Result<T, E>`", "Use `?` to propagate"). Downstream conflations:
  `Table::query` returns `Ok(vec![])` — a corrupt reply indistinguishable from "no matching
  rows"; `db_get`/`global_get`/`batch_get` return `None` — indistinguishable from "absent";
  `call`/`db_insert` return `Value::Null` — indistinguishable from a legitimate null result,
  and `Table::insert` then reports the misleading "db_insert returned null" (which itself
  conflates three situations: host `packed == 0` contract violation, decode-failure
  fallback `Null`, and a genuinely null stored record). `__rt::decode_params` maps malformed
  host bytes to an empty `Params`, so every getter then fails with "missing parameter: X"
  instead of a protocol error. `__rt::encode_value` maps any rmp-serde failure to
  `Vec::new()`: in `Table::query(Some(f))` that leaks a zero-length buffer, which the ABI
  documents as "zero-length filter means no filter" — i.e. the host returns the **whole
  table** where the author asked for a filtered subset (fail-open in the over-exposure
  direction, inside the function's own actor permissions). Only the `packed == 0` sentinel
  legitimately means "absent". `rmp_serde::to_vec` is practically infallible for this
  self-contained `Value` today, so the encode half is latent — which makes it precisely the
  kind of silent fallback that will never be noticed if it ever becomes reachable.
- Failure scenario: after any wire drift between `shamir_types::QueryValue` and the guest
  mirror `Value` (the exact drift `value.rs` exists to track), version skew, or a truncated
  host write, `Table::query` returns `Ok(vec![])` — every query "returns no rows", every key
  lookup is "absent", every insert "returned null", every param "missing". The guest author
  chases a phantom data bug that is actually a wire failure. And if a future `Value` variant
  (or an upstream rmp-serde change) ever makes filter serialization fail, guests silently
  receive full-table scans and act on data their filter was supposed to exclude, with no
  error anywhere.
- Suggested fix: decode to `Result` internally and propagate a decode error at the
  `Table`/`Ctx` API layer (these already return `Result`), e.g.
  `Error::decode("db_query", e)` with the underlying rmp-serde message; reserve
  `None`/`Value::Null` exclusively for the `packed == 0` "absent" path; reserve zero-length
  exclusively for the explicit `filter = None` case and trap (or thread a `Result`) on
  `encode_value` failure instead of substituting empty bytes; keep the params decode failure
  in-band but truthful — store the error in `Params` so the first accessor reports "params
  failed to decode: {e}" instead of "missing parameter"; distinguish `packed == 0` on insert
  as a contract error naming the host op.

### 6.2 — high — No error taxonomy: single-message `Error`, `Error::user` used for infra failures, and the trap transport flattens user errors into `FunctionError::Compute`
- File:line: `crates/shamir-sdk/src/error.rs:6-23` (verified: hand-rolled one-field struct,
  only constructor `Error::user`); `crates/shamir-sdk/src/__rt.rs:64-69` (`trap` =
  `panic!("shamir function error: {msg}")`); misuse sites `src/db.rs:141, 144, 147`,
  `src/http.rs:27, 30, 34, 41, 127, 137`; transport: macro-generated match at
  `shamir-sdk-macros/src/lib.rs:271-273, 398-400` (self-acknowledged `TODO(slice 4)` at
  :251); host mapping `shamir-wasm-host/src/wasm/wasm_function.rs:593-600`.
- Also flagged by: security-crypto #6 (low), api-wire-protocol #6 (low), correctness-tdd #8
  (nit — the hand-rolled-struct half; CLAUDE.md prefers `thiserror` for library errors;
  defensible only as guest-dependency minimisation, noted for the record).
- Issue: this crate hand-rolls a one-field struct whose only constructor is `Error::user`,
  then uses it for everything: http allowlist/curl/timeout failures, host contract
  violations ("host returned unexpected value"), and internal encode/decode failures in
  `Db::execute` ("execute: encode batch: {e}"). The doc positions `Error::user` as "a
  deliberate, user-surfaced error" (error.rs:5), which none of those are. Worse, the
  macro-generated entrypoint turns every user `Err(e)` into `__rt::trap(&e.to_string())` →
  WASM trap → host `FunctionError::Compute`, so a guest panic and a deliberate user error
  are indistinguishable on the host — the `Error` type's entire semantic content is
  destroyed at the ABI boundary (acknowledged as `TODO(slice 4)` in the macro crate).
  CLAUDE.md mandates "`thiserror` for library error enums (with `#[from]` where natural)".
- Failure scenario: a `#[validator]`-style "field `zip` is invalid" user error reaches the
  client as `FunctionError::Compute` ("compute/internal error"), while a genuine guest bug
  also reaches the client as `Compute`; the host cannot route, filter, or i18n user errors,
  guest authors cannot branch on callee failure kinds, and an operator debugging an
  authorization-style rejection ("missing parameter: token") chases a phantom engine bug —
  with audit logs and retry policy unable to distinguish user errors from real crashes.
- Suggested fix: convert `Error` to a `thiserror` enum (`User { message }`,
  `Host { op, message }`, `Decode { context, #[source] }`), keep `Error::user` as the
  constructor for the user variant, stop using it for infra errors; encode the `Err`
  through the normal result channel (a `[false, message]` envelope like `http_fetch` already
  uses) and have the host map it to `FunctionError::User`, keeping `trap` for actual
  invariant violations — completing the macros crate's planned `TODO(slice 4)`. The host
  only consumes the stringified message today, so the local enum refactor changes no wire
  encoding.

### 6.3 — medium — `Ctx::call` failures are uncatchable, and callee-result decode failure is conflated with `Value::Null`
- File:line: `crates/shamir-sdk/src/context.rs:86-88`; `crates/shamir-sdk/src/host_imports.rs:121-132` (verified).
- Issue: `call` returns `Value`, not `Result`. Per its own doc, a missing callee,
  depth-limit excess, or callee error traps the whole guest function — the caller cannot
  catch or branch on callee failure. Inconsistent with `Ctx::http_fetch`, which deliberately
  returns catchable `Err` via an `[ok, payload]` envelope (context.rs:103-119). Additionally
  `unpack_ptr_len(packed) == None → return Value::Null` and `.unwrap_or(Value::Null)`: if
  the host signals failure with 0 instead of trapping, or the success payload fails to
  decode, the caller silently receives `Value::Null`, indistinguishable from a callee that
  legitimately returned `Null` (the decode half is the 6.1 family applied to `call`).
- Failure scenario: a procedure composing N sub-functions via `ctx.call` loses the entire
  batch on the first sub-function error with no recovery path; or a decode hiccup silently
  turns a callee's Map result into `Null` and downstream `params.get("x")`-style reads start
  failing with "missing parameter" far from the real fault.
- Suggested fix: mirror the `http_fetch` envelope for `call` (`Result<Value>`), or at minimum
  document the `Null` ambiguity and make decode failure a distinct, loud outcome (finding 6.1).

### 6.4 — medium — *(dedup: primary 1.1)* — Missing error-path tests: all suites cover happy paths and wire conformance only
- Deduplicated into **1.1**, which carries the full inventory of untested error branches
  (all five `decode_fetch_envelope` arms, the `from_value` leniency matrix,
  `Table::insert`/`query` error arms, `Params` getter failures, `decode_params`' fallback,
  `unpack_ptr_len` semantics) and the suggested `tests/error_path_tests.rs` remedy — with
  the note that per the TDD protocol these branches have no failing-first test to protect
  them, which is exactly what findings 6.1/6.2's fixes need (Red first).

### 6.5 — low — *(dedup: primary 6.1)* — `Table::insert` error path conflates absent, decode-failure, and genuine null
- Deduplicated into **6.1** (its `db.rs:86-92` paragraph). Error-lens emphasis preserved
  there: during a wire-drift incident every insert surfaces as "db_insert returned null",
  pointing the author at the table/record rather than the decode layer.

### 6.6 — low — *(dedup: primary 6.1)* — `__rt::decode_params` masks a params decode failure as an empty `Params`
- Deduplicated into **6.1** (its `__rt.rs:11-16` paragraph and the in-band truthful-error fix).

### 6.7 — low — *(dedup: primary 6.1)* — `__rt::encode_value` maps encode failure to empty bytes
- Deduplicated into **6.1** (its `__rt.rs:19-21` paragraph). Error-lens emphasis preserved
  there: treat it as the programmer-bug invariant it is (`unreachable!`-style panic with the
  error message — sanctioned by CLAUDE.md for invariant violations — or thread a `Result` to
  the macro entrypoint).

### 6.8 — low — *(dedup: primary 2.1)* — `block_on` busy-spins forever on `Pending` with a no-op waker
- Deduplicated into **2.1**. Error-lens framing preserved there: the symptom (host SLOW /
  nextest `TIMEOUT`) points nowhere near the guest spin; the bounded-yields-then-trap variant
  of the fix converts the hang into a named, greppable failure.

### 6.9 — nit — *(dedup: primary 1.6)* — `HttpResponse::from_value`: truncating status cast plus silent leniency on malformed headers/body
- Deduplicated into **1.6**, which carries the detail.

### 6.10 — nit — *(dedup: split 1.7 / 3.6)* — Silent truncations in wire helpers: `visit_u64` wraparound and `leak_result` len mask
- Deduplicated: the `visit_u64` (`value.rs:98`, `v as i64` wrap) half into **1.7**; the
  `leak_result` (`__rt.rs:25-30`, 64-bit pointer truncation + `len & 0xFFFF_FFFF` mask)
  half into **3.6**. Both carry the shared remedy: checked/documented conversions and
  `debug_assert!` packing preconditions so violations fail in host-target tests.

---

## 7. style-claude-md

Largely exemplary (verified): exactly one `mod.rs` (`src/tests/mod.rs`) and it is a
manifest-only re-export; tests wired via `#[cfg(test)] mod tests;` in `lib.rs` (no inline
`#[cfg(test)] mod tests { ... }` blocks anywhere); every non-test source file keeps all
`use` statements in its header (the per-`cfg` `use crate::Value;` in `host_imports.rs` sits
at the enclosing `mod imp` header, which the rule allows); each file's multiple public types
(`Ctx`+`Batch`, `Db`+`Table`, `HttpRequest`+`HttpResponse`,
`Validation`+`ValidationError`+`IntoFieldPath`) qualify as closely-coupled groups. The
deviation is concentrated in `src/tests/`.

### 7.1 — low — Function-body `use` imports in `src/tests/` violate the "Imports at the top" rule
- File:line: `crates/shamir-sdk/src/tests/value_tests.rs:146, 177, 193, 225, 246, 279-280,
  298, 312-313, 331, 345, 366, 385, 401`; `crates/shamir-sdk/src/tests/validation_tests.rs:239, 305`
  (17 sites total).
- Issue: CLAUDE.md §"Imports at the top" is unconditional: "All `use` statements live in the
  file header ... never inside a function or block body," with exactly three documented
  exceptions. None applies here: these are not `use super::*;` in a test mod; the only trait
  import (`use std::str::FromStr;` at value_tests.rs:280, 313) is a single-method call with
  *no* top-level collision and *no* justifying comment (the exception requires both); and
  nothing is macro-generated or `cfg`-gated. Repeated offenders:
  `use shamir_types::types::common::new_map;` / `new_set;` (11 sites),
  `use shamir_types::types::value::QueryValue;` (validation_tests.rs:239, 305),
  `use rust_decimal::Decimal;`, `use num_bigint::BigInt;`, `use std::str::FromStr;`. None of
  these names collide at file scope, so hoisting is purely mechanical.
- Failure scenario: none at runtime — the cost is convention drift: the documented rule
  erodes case-by-case ("tests are different"), making the next mid-body import in non-test
  code harder to argue against, and `git blame`/greppability of imports degrades.
- Suggested fix: hoist all 17 imports to the file headers, merging with the existing header
  block (`use shamir_types::types::common::{new_map, new_set};`,
  `use shamir_types::types::value::QueryValue;`, `use rust_decimal::Decimal;`,
  `use num_bigint::BigInt;`, `use std::str::FromStr;`). Land as a separate `style:` commit
  per the CLAUDE.md sweep rule.

### 7.2 — low — *(dedup: primary 2.1)* — Stale slice-jargon comments in `__rt::block_on` misdescribe what the SDK supports
- Deduplicated into **2.1**, whose suggested fix rewrites the comment to the current reality.
  Style-lens framing preserved there: the doc rot causes two wrong maintenance reactions —
  (a) "fixing" `block_on` into a real executor unnecessarily, or (b) adding a
  genuinely-`.await`ing future believing the comment covers it, getting a silent WASM
  busy-spin.

### 7.3 — nit — `value.rs` module doc points at a test path that does not exist
- File:line: `crates/shamir-sdk/src/value.rs:9` (verified: "see conformance tests in
  `tests/value_tests.rs`"; the crate-root `tests/` contains only the compile-pass files).
- Also flagged by: correctness-tdd #8 (nit — same stale path).
- Failure scenario: a reader following the pointer greps/opens `crates/shamir-sdk/tests/`
  and concludes the claimed conformance coverage doesn't exist.
- Suggested fix: change the reference to `crate::tests::value_tests` (or
  `src/tests/value_tests.rs`).

### 7.4 — nit — *(dedup: primary 5.7)* — `__rt` doc claims "not part of the public SDK surface" while the module is fully `pub` with no `#[doc(hidden)]`
- Deduplicated into **5.7**, which carries the fix (`#[doc(hidden)]` + reworded module doc).

---

## Finding counts

| Severity | Lens-tagged findings (as filed) | Distinct defects (after dedup) | Dedup mapping (distinct ← raw) |
|---|---|---|---|
| critical | 0 | 0 | — |
| high | 6 | 6 | 1.1 (C#1) · 2.1 (CON#1; + C#4, S#1, P#3, E#9, ST#2) · 4.1 (P#1; + E#3, S#8) · 5.1 (A#1; + C#8c) · 6.1 (E#1; + C#2, A#3, S#4, E#6, E#7, E#8) · 6.2 (E#2; + S#6, A#6, C#8d) |
| medium | 15 | 8 | 1.3 (C#3) · 1.5 (C#5) · 3.1 (S#2) · 4.2 (P#2) · 5.2 (A#2) · 5.4 (A#4) · 5.5 (A#5, tests half → 1.1) · 6.3 (E#4) |
| low | 19 | 10 | 1.6 (C#6; + S#9, E#10) · 1.7 (C#7; + E#11a) · 3.2 (S#3) · 3.4 (S#5) · 3.6 (S#7; + E#11b) · 4.4 (P#4; + CON#2) · 4.5 (P#5) · 5.7 (A#7; + ST#4) · 5.8 (A#8) · 7.1 (ST#1) |
| nit | 10 | 4 | 1.8 (C#8b) · 4.6 (P#6) · 5.9 (A#9) · 7.3 (ST#3; + C#8a) |
| **total** | **50** | **28** | |

Lens keys in the mapping: C = correctness-tdd, CON = concurrency-lockfree, S = security-crypto,
P = performance-hotpath, A = api-wire-protocol, E = error-handling-lifecycle, ST = style-claude-md
(numbered as in each lens file).

Deduplicated defect census: **0 critical, 6 high, 8 medium, 10 low, 4 nit = 28 distinct
defects** (50 lens-tagged findings as filed; the raw census matches the workspace
SUMMARY.md Per-crate breakdown row for shamir-sdk exactly).

## Fix Plan

**P0 — before anything else ships from this crate**

1. **Fail-closed protocol handling on the host-import boundary.** Decode host replies to
   `Result` internally and propagate through the APIs that already return `Result`
   (`Table::query`/`insert`, `http_fetch`); reserve `None`/`Value::Null` exclusively for the
   `packed == 0` sentinel; reserve zero-length exclusively for explicit `filter = None` and
   make `encode_value` trap (or thread a `Result`) instead of substituting empty bytes;
   keep a params decode failure in-band so the first accessor reports "params failed to
   decode" rather than "missing parameter"; distinguish insert's `packed == 0` as a host
   contract error. Write the failing decode-branch tests first (Red per CLAUDE.md). Closes
   **6.1** with its facets **1.2, 3.3, 5.3, 6.5, 6.6, 6.7** — the workspace headline
   "fail-open decode".
2. **Make `block_on` fail fast on `Pending` instead of spinning.** Poll once; on
   `Poll::Pending` trap ("guest future yielded Pending — host imports suspend transparently;
   guest-local awaits are unsupported") so misuse is a catchable `Compute` error, not a
   fuel-burning wedge or a nextest `TIMEOUT`; delete the stale "pure functions only"
   comment; document the real invariant where authors read; add the Pending-behavior test.
   Closes **2.1** with its facets **1.4, 4.3, 6.8, 7.2** (and security-crypto #1) — the
   workspace headline "spin-on-Pending executor".
3. **Bound guest linear memory per host call.** Implement reclamation — reusable scratch
   buffer, a `shamir_free(ptr, len)` import/export consumed after the host's synchronous
   read, or a bump-arena reset between calls; until the ABI change lands, document the
   per-call leak as an invocation-lifetime budget and warn against per-row host-call loops.
   Closes **4.1** with its facets (**error-handling #3**, **3.7**) — the workspace headline
   "unbounded guest memory".

**P1 — soon**

4. **Restore an error taxonomy and stop flattening user errors into traps.** `thiserror`
   enum (`User`/`Host`/`Decode`), stop misusing `Error::user` for infra failures, and route
   user `Err` through a `[false, message]` result envelope (as `http_fetch` already does) so
   the host maps it to `FunctionError::User` — completing the macros crate's `TODO(slice 4)`.
   Closes **6.2** with facets **3.5, 5.6** (and correctness #8's Error-struct nit).
5. **Close the TDD gap.** `src/tests/http_tests.rs`, `params_tests.rs`, `rt_tests.rs` + an
   error-path suite; `tests/function_compile_pass.rs` + `tests/validator_compile_pass.rs`;
   a minimal internal seam for `db.rs`'s import calls so the Null/non-List arms become
   host-testable. Closes **1.1** (with facets **3.9**, **6.4**) and **1.5**.
6. **Harden `Ctx::call`.** Validate `args` is a `Value::Map` guest-side (fail fast), fix the
   "should" doc to MUST + `# Panics/Traps`, and move to a `Result`/envelope transport
   mirroring `http_fetch` so callee failures are catchable and `Null` stays unambiguous.
   Closes **1.3** and **6.3**.
7. **Validate the egress boundary.** Reject `\r`/`\n`/`\0` in method/URL/header names/values
   in `HttpRequest` (+ RFC-token/scheme checks), strip CRLF in the host's
   `escape_curl_value` as defense in depth; make `HttpResponse::from_value` strict
   (`u16::try_from`, `Err` on wrong-typed headers/body); define duplicate-header policy
   (reject or explicit last-wins) and pin the envelope bytes with round-trip tests. Closes
   **3.1**, **1.6**, **5.5**.
8. **Bound `Table::query`.** Add `limit`/keyset-cursor to the `db_query` ABI (+
   `query_paginated`/iterator API), or at minimum document the unbounded double-buffering
   loudly on `Table::query` and in the prelude example. Closes **4.2** (compounds P0 item 3).
9. **Version the guest ABI wire path.** Explicit `protocol_version` field/header checked on
   both sides, failing closed with a clear error; document the wire-compat policy before
   compiled guests are persisted. Closes **5.4**.
10. **Fix the raw-filter surface.** Add the required builder-exception justification comment,
    document the key convention (incl. empty-map = match-all and the `Null` coercion and the
    Dec/Big re-filter trap), make the host reject unsupported filter value types instead of
    coercing to `FilterValue::Null`, and reject empty-map keys in `Table::get`. Closes
    **5.1** (with correctness #8's comment nit), **5.2**, **5.8**.

**P2 — backlog**

11. **Memory-safety hygiene in the ABI helpers.** Centralize one bounds-checked
    `guest_slice(packed)` helper for all eight `from_raw_parts` sites; `#[cfg(target_arch =
    "wasm32")]`-gate (or `debug_assert!`) `leak_result`'s packing; reinterpret `i32`
    pointers as `u32` host-side or document the 2 GiB linear-memory limit. Closes **3.2**,
    **3.6** (with error #11's len-mask half), **5.9**.
12. **Small perf wins.** One O(P) index pass in `decode_params` (or an accepted-cost
    comment) + consuming `take_bytes`; consuming `into_value` + single-pass `from_value` for
    the HTTP path; hoist-the-handle doc note. Closes **4.4** (with concurrency #2's acked
    nit), **4.5**, **4.6**.
13. **Honesty fixes for docs, visibility, and the purity claim.** `#[doc(hidden)]` +
    semver-exemption note on `pub mod __rt`; reword the scalar "purity guarantee" to
    "compile-time lint; isolation enforced by host gateway wiring" (or enforce structurally
    with a zero-capability token type + an alias-bypass macros test); fix the `value.rs`
    test path; move `type_name` beside `Value`; add a NaN/Inf byte-comparison conformance
    test and document/check `visit_u64`'s wrap. Closes **5.7** (with style #4), **3.4**,
    **7.3**, **1.8**, **1.7** (with error #11's visit_u64 half).
14. **Style sweep.** Hoist the 17 function-body imports in `src/tests/` as a separate
    `style:` commit per CLAUDE.md. Closes **7.1**.
