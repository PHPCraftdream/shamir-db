<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-sdk-macros — SUMMARY independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

All 43 summary claims and 14 plans were independently checked. Concrete API, decode, error-classification, polling and cloning mechanisms remain. Some current confirmed-open labels convert already-enforced contracts or optional maintenance preferences into defects; those are corrected below. No source-fixed closure was established.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 43 | 34 | 0 | 0 | 3 | 2 | 4 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1-1"></a>

### Claim 1.1 — Zero test coverage: TDD protocol not honored; even pure helpers are untested

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Local/helper/UI/validator coverage is missing, but registered generated-function success coverage predates the audit. Historical TDD ordering is not inferable.

Evidence: [crates/shamir-sdk-macros/Cargo.toml:13](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/Cargo.toml#L13); [crates/shamir-wasm-host/src/tests/compile_tests.rs:19](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/compile_tests.rs#L19).

Grouping/duplicate: [correctness-tdd.md#1](correctness-tdd.md#review-1). This is not an additional independent defect.

<a id="review-1-2"></a>

### Claim 1.2 — User parameter patterns re-used as call-position expressions (`mut x`, `_`, `ref x` break the expansion)

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Complete patterns are interpolated into call arguments; mut/ref/wildcard are not valid forwarding expressions.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:80](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L80); [crates/shamir-sdk-macros/src/lib.rs:102](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L102).

Grouping/duplicate: [correctness-tdd.md#2](correctness-tdd.md#review-2). This is not an additional independent defect.

<a id="review-1-3"></a>

### Claim 1.3 — String-based return-type validation is inconsistent across sibling macros

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Qualified SDK and std::result:: acceptance remains inconsistent; concrete returns prevent runtime type confusion.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:197](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L197); [crates/shamir-sdk-macros/src/lib.rs:411](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L411).

Grouping/duplicate: [api-wire-protocol.md#1](api-wire-protocol.md#review-1). This is not an additional independent defect.

<a id="review-1-4"></a>

### Claim 1.4 — "Only one `#[...]` per crate" contract documented but not enforced

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

Fixed unmangled symbols enforce uniqueness. A special diagnostic is not promised and is optional.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:108](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L108); [crates/shamir-sdk-macros/src/lib.rs:238](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L238).

Grouping/duplicate: [correctness-tdd.md#4](correctness-tdd.md#review-4). This is not an additional independent defect.

<a id="review-1-5"></a>

### Claim 1.5 — `assert!`/`panic!` diagnostics instead of `syn::Error` compile errors

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Semantic validation still panics rather than constructing spanned errors.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:51](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L51); [crates/shamir-sdk-macros/src/lib.rs:71](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L71).

Grouping/duplicate: [error-handling-lifecycle.md#1](error-handling-lifecycle.md#review-1). This is not an additional independent defect.

<a id="review-1-6"></a>

### Claim 1.6 — Generic functions hit E0207 inside the expansion instead of a clear rejection

Status: `unverified`. Current risk: `low`.

Prior-cycle decision: `unverified`.

E0207 does not describe these generic functions. Unselected type arguments create inference obligations; support policy and exact diagnostics remain unverified.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:219](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L219); [crates/shamir-sdk-macros/src/lib.rs:264](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L264).

Grouping/duplicate: [correctness-tdd.md#6](correctness-tdd.md#review-6). This is not an additional independent defect.

<a id="review-1-7"></a>

### Claim 1.7 — `shamir_alloc` does not guard negative or zero `len`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Negative-length rejection is absent; valid zero-length allocation is not a defect.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:109](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L109).

Grouping/duplicate: [security-crypto.md#2](security-crypto.md#review-2). This is not an additional independent defect.

<a id="review-1-8"></a>

### Claim 1.8 — Emitted `__rt::block_on` busy-spins forever on `Pending`

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Persistent guest-local Pending repolls indefinitely; finite Pending and production metering qualify the scenario.

Evidence: [crates/shamir-sdk/src/__rt.rs:50](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/__rt.rs#L50).

Grouping/duplicate: [concurrency-lockfree.md#1](concurrency-lockfree.md#review-1). This is not an additional independent defect.

<a id="review-1-9"></a>

### Claim 1.9 — `type_contains_ctx` is a name heuristic and the `#[scalar]` purity claim overreaches

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

A valid scalar body can construct Ctx; lack of a parameter is not a capability restriction.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:453](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L453); [crates/shamir-sdk/src/context.rs:64](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/context.rs#L64).

Grouping/duplicate: [correctness-tdd.md#9](correctness-tdd.md#review-9). This is not an additional independent defect.

<a id="review-1-10"></a>

### Claim 1.10 — Generated code hardcodes `shamir_sdk::` paths; `_attr` tokens silently ignored

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Literal SDK paths and discarded attribute tokens remain. Built-in compilation supplies the canonical dependency name.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:44](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L44); [crates/shamir-wasm-host/src/compile.rs:511](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L511).

Grouping/duplicate: [correctness-tdd.md#10](correctness-tdd.md#review-10). This is not an additional independent defect.

<a id="review-2-1"></a>

### Claim 2.1 — Generated guest ABI drives the author's `async fn` on a spin-on-`Pending` executor — latent busy-wait/livelock, undocumented and unguarded

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Unbounded guest-local polling remains; async host callbacks already suspend fibers rather than necessarily pending the guest future.

Evidence: [crates/shamir-sdk/src/__rt.rs:50](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/__rt.rs#L50); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:195](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L195).

Grouping/duplicate: [concurrency-lockfree.md#1](concurrency-lockfree.md#review-1). This is not an additional independent defect.

<a id="review-2-2"></a>

### Claim 2.2 — The "Ready on first poll" contract of the emitted ABI has zero test coverage

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Immediate-Ready tests exist; no controlled polling/rejection oracle distinguishes guest rejection from old host fuel exhaustion.

Evidence: [crates/shamir-sdk/src/tests/value_tests.rs:414](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/tests/value_tests.rs#L414); [crates/shamir-sdk/src/__rt.rs:50](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/__rt.rs#L50).

Grouping/duplicate: [correctness-tdd.md#1](correctness-tdd.md#review-1). This is not an additional independent defect.

<a id="review-3-1"></a>

### Claim 3.1 — `#[validator]` silently coerces undecodable params into `record = Null`, `old_record = None` — UPDATE presents as INSERT

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Fail-quiet decoding remains. UPDATE-as-INSERT requires abnormal wire/caller behavior; the normal adapter supplies both keys.

Evidence: [crates/shamir-sdk/src/__rt.rs:11](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/__rt.rs#L11); [crates/shamir-engine/src/validator/wasm_record_validator.rs:65](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/validator/wasm_record_validator.rs#L65).

Grouping/duplicate: [security-crypto.md#1](security-crypto.md#review-1). This is not an additional independent defect.

<a id="review-3-2"></a>

### Claim 3.2 — Generated ABI constructs `unsafe` slices from unverified `(ptr, len)` and allocates from unvalidated `i32` — negative `len` is immediate UB

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Raw integer inputs reach slice construction unchecked, including null-zero calls through a safe public export. Ordinary host ranges are checked.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:120](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L120); [crates/shamir-sdk-macros/src/lib.rs:123](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L123); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:533](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L533).

Grouping/duplicate: [security-crypto.md#2](security-crypto.md#review-2). This is not an additional independent defect.

<a id="review-3-3"></a>

### Claim 3.3 — No tests cover the two boundary-bearing macros or any runtime behavior of the generated ABI

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

The registered function test invokes generated WASM and asserts 42; it existed before this review.

Evidence: [crates/shamir-wasm-host/src/tests/compile_tests.rs:35](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/compile_tests.rs#L35); [crates/shamir-wasm-host/src/tests/compile_tests.rs:37](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/compile_tests.rs#L37).

Grouping/duplicate: [security-crypto.md#3](security-crypto.md#review-3). This is not an additional independent defect.

<a id="review-3-4"></a>

### Claim 3.4 — User error strings passed verbatim into the trap channel

Status: `unverified`. Current risk: `low`.

Prior-cycle decision: `unverified`.

Guest panic formatting is proven, but transport of that message into returned Wasmtime error text is not.

Evidence: [crates/shamir-sdk/src/__rt.rs:68](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/__rt.rs#L68); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:593](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L593).

Grouping/duplicate: [security-crypto.md#4](security-crypto.md#review-4). This is not an additional independent defect.

<a id="review-3-5"></a>

### Claim 3.5 — String-comparison return-type checks accept same-named foreign types

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Bare foreign names can pass spelling checks but incompatible results fail concrete SDK typing; qualified foreign Validation is not accepted.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:66](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L66); [crates/shamir-sdk-macros/src/lib.rs:98](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L98).

Grouping/duplicate: [api-wire-protocol.md#1](api-wire-protocol.md#review-1). This is not an additional independent defect.

<a id="review-3-6"></a>

### Claim 3.6 — "Only one macro per crate" constraint unenforced

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

Duplicate exported symbols already enforce the supported artifact shape; friendly diagnostics are optional.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:108](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L108); [crates/shamir-sdk-macros/src/lib.rs:238](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L238).

Grouping/duplicate: [correctness-tdd.md#4](correctness-tdd.md#review-4). This is not an additional independent defect.

<a id="review-4-1"></a>

### Claim 4.1 — `#[validator]` `shamir_call`: full decoded payload retained across the entire await + redundant deep copies of `record`/`old_record`

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Borrowed getters deep-clone recursive owned data while Params remains live. Numerical peak-memory multipliers are not proved.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:130](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L130); [crates/shamir-sdk-macros/src/lib.rs:137](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L137); [crates/shamir-sdk/src/value.rs:26](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/value.rs#L26).

Grouping/duplicate: [performance-hotpath.md#1](performance-hotpath.md#review-1). This is not an additional independent defect.

<a id="review-4-2"></a>

### Claim 4.2 — `shamir_alloc` leaks every allocation; the "module is short-lived" contract is documented only here and enforced nowhere in this crate

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Fresh Store destruction reclaims guest memory; the lifetime is documented outside the macro too. Future live-instance reuse is a prospective risk.

Evidence: [crates/shamir-sdk/src/host_imports.rs:55](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/host_imports.rs#L55); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:474](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L474).

Grouping/duplicate: [performance-hotpath.md#2](performance-hotpath.md#review-2). This is not an additional independent defect.

<a id="review-4-3"></a>

### Claim 4.3 — Generated `shamir_call` busy-spins at 100% CPU on `Pending`

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Persistent guest-local Pending busy-polls until interruption; an exact CPU percentage and every-Pending exhaustion claim are unsupported.

Evidence: [crates/shamir-sdk/src/__rt.rs:50](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/__rt.rs#L50); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:487](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L487).

Grouping/duplicate: [concurrency-lockfree.md#1](concurrency-lockfree.md#review-1). This is not an additional independent defect.

<a id="review-4-4"></a>

### Claim 4.4 — `shamir_alloc` zero-fills O(len) bytes the host immediately overwrites; negative `len` not rejected

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Negative-length checks remain absent. Initialized allocation followed by overwrite is source-proven, but separate memset cost is not.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:110](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L110); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:540](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L540).

Grouping/duplicate: [security-crypto.md#2](security-crypto.md#review-2). This is not an additional independent defect.

<a id="review-5-1"></a>

### Claim 5.1 — Return-type validation is string-based, per-macro inconsistent, and rejects valid spellings

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Qualified SDK false rejections and the std::result:: gap remain.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:197](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L197); [crates/shamir-sdk-macros/src/lib.rs:411](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L411).

Grouping/duplicate: [api-wire-protocol.md#1](api-wire-protocol.md#review-1). This is not an additional independent defect.

<a id="review-5-2"></a>

### Claim 5.2 — Wire protocol has no decode-failure channel: malformed input silently becomes empty Params / Null record

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Malformed/non-map input and valid empty parameters are conflated.

Evidence: [crates/shamir-sdk/src/__rt.rs:11](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/__rt.rs#L11).

Grouping/duplicate: [security-crypto.md#1](security-crypto.md#review-1). This is not an additional independent defect.

<a id="review-5-3"></a>

### Claim 5.3 — User errors travel as bare trap strings; no structured error envelope

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Deliberate guest Err becomes Compute through the trap path. Verbatim message transport is a separate unverified claim.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:272](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L272); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:593](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L593).

Grouping/duplicate: [error-handling-lifecycle.md#2](error-handling-lifecycle.md#review-2). This is not an additional independent defect.

<a id="review-5-4"></a>

### Claim 5.4 — Diagnostics via `assert!`/`panic!` instead of spanned `compile_error!`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Signature errors remain assert/panic-based.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:51](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L51); [crates/shamir-sdk-macros/src/lib.rs:486](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L486).

Grouping/duplicate: [error-handling-lifecycle.md#1](error-handling-lifecycle.md#review-1). This is not an additional independent defect.

<a id="review-5-5"></a>

### Claim 5.5 — No compile coverage for `#[function]`/`#[validator]`; no compile-fail tests anywhere

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Validator and signature-failure coverage remain absent; function success compilation/runtime coverage is pre-existing counter-evidence.

Evidence: [crates/shamir-sdk-macros/Cargo.toml:13](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/Cargo.toml#L13); [crates/shamir-wasm-host/src/tests/compile_tests.rs:19](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/compile_tests.rs#L19).

Grouping/duplicate: [correctness-tdd.md#1](correctness-tdd.md#review-1). This is not an additional independent defect.

<a id="review-5-6"></a>

### Claim 5.6 — `#[procedure]` doc example does not compile

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The example directly wraps Result&lt;Vec&lt;Value&gt;&gt; where Value is required; ? and Value::List are needed.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:292](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L292); [crates/shamir-sdk/src/db.rs:98](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/db.rs#L98).

Grouping/duplicate: [api-wire-protocol.md#6](api-wire-protocol.md#review-6). This is not an additional independent defect.

<a id="review-5-7"></a>

### Claim 5.7 — "One macro per crate" unenforced; attribute payload silently ignored

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Ignored attribute tokens remain actionable. Export uniqueness is already enforced, so only that bundled facet remains open.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:44](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L44); [crates/shamir-sdk-macros/src/lib.rs:108](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L108).

Grouping/duplicate: [correctness-tdd.md#10](correctness-tdd.md#review-10). This is not an additional independent defect.

<a id="review-5-8"></a>

### Claim 5.8 — `shamir_alloc` performs no length validation; negative `len` allocates ~2^63 bytes

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Unchecked signed conversion remains; wasm32 -1 maps to approximately 4 GiB, not 2^63, and allocation may never occur.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:110](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L110); [crates/shamir-wasm-host/src/compile.rs:539](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L539).

Grouping/duplicate: [security-crypto.md#2](security-crypto.md#review-2). This is not an additional independent defect.

<a id="review-5-9"></a>

### Claim 5.9 — Four macros in one `lib.rs` with the ABI emitter duplicated four times

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

Duplication alone proves maintenance opportunity, not violated wire semantics or mandatory file organization.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:91](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L91); [crates/shamir-sdk-macros/src/lib.rs:521](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L521); [CLAUDE.md:505](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L505).

Grouping/duplicate: [style-claude-md.md#1](style-claude-md.md#review-1). This is not an additional independent defect.

<a id="review-5-10"></a>

### Claim 5.10 — `type_contains_ctx` purity check is lexical, not semantic

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The signature restriction does not prevent Ctx::new() in the body; host policy controls actual effects.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:425](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L425); [crates/shamir-sdk/src/context.rs:64](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/context.rs#L64).

Grouping/duplicate: [correctness-tdd.md#9](correctness-tdd.md#review-9). This is not an additional independent defect.

<a id="review-6-1"></a>

### Claim 6.1 — All signature validation panics via `assert!`/`panic!` instead of `syn::Error::to_compile_error()`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Semantic failures still bypass the spanned syn error path.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:51](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L51); [crates/shamir-sdk-macros/src/lib.rs:513](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L513).

Grouping/duplicate: [error-handling-lifecycle.md#1](error-handling-lifecycle.md#review-1). This is not an additional independent defect.

<a id="review-6-2"></a>

### Claim 6.2 — Generated `Err` path flattens typed user errors into a panic-trap the host misclassifies as `FunctionError::Compute`

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

The message-only Error::user is trapped and mapped to Compute; deliberate failure classification is lost.

Evidence: [crates/shamir-sdk/src/error.rs:13](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/error.rs#L13); [crates/shamir-sdk-macros/src/lib.rs:272](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L272); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:593](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L593).

Grouping/duplicate: [error-handling-lifecycle.md#2](error-handling-lifecycle.md#review-2). This is not an additional independent defect.

<a id="review-6-3"></a>

### Claim 6.3 — Zero tests in-crate; no error-path coverage for any validation branch anywhere in the workspace

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Existing function success coverage cannot discriminate erroneous decode, deliberate Err or signature rejection behavior.

Evidence: [crates/shamir-wasm-host/src/tests/compile_tests.rs:19](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/compile_tests.rs#L19); [crates/shamir-sdk-macros/Cargo.toml:13](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/Cargo.toml#L13).

Grouping/duplicate: [correctness-tdd.md#1](correctness-tdd.md#review-1). This is not an additional independent defect.

<a id="review-6-4"></a>

### Claim 6.4 — `#[validator]` generated code silently swallows param-extraction errors — missing vs malformed payload indistinguishable

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Decode failure becomes empty Params before missing-key Null/None fallback.

Evidence: [crates/shamir-sdk/src/__rt.rs:11](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/__rt.rs#L11); [crates/shamir-sdk-macros/src/lib.rs:129](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L129).

Grouping/duplicate: [security-crypto.md#1](security-crypto.md#review-1). This is not an additional independent defect.

<a id="review-6-5"></a>

### Claim 6.5 — Return-type validation inconsistent across the four macros; equivalent spellings spuriously rejected

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

The exact checks and shared normalizer still disagree.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:66](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L66); [crates/shamir-sdk-macros/src/lib.rs:411](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L411).

Grouping/duplicate: [api-wire-protocol.md#1](api-wire-protocol.md#review-1). This is not an additional independent defect.

<a id="review-6-6"></a>

### Claim 6.6 — Generated ABI functions trust `i32` inputs unvalidated: negative `len` yields alloc-abort or UB

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Guest checks are absent; standard host checks do not justify arbitrary safe export calls or all pointer preconditions.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:120](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L120); [crates/shamir-sdk-macros/src/lib.rs:123](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L123).

Grouping/duplicate: [security-crypto.md#2](security-crypto.md#review-2). This is not an additional independent defect.

<a id="review-6-7"></a>

### Claim 6.7 — Generated `block_on` has no deadline/fuel guard: a genuinely-Pending future spins hot forever on the failure path

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

The helper has no local bound, but fuel/epoch limits and async host fiber suspension invalidate the unconditional production-hang scenario.

Evidence: [crates/shamir-sdk/src/__rt.rs:50](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/__rt.rs#L50); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:487](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L487).

Grouping/duplicate: [concurrency-lockfree.md#1](concurrency-lockfree.md#review-1). This is not an additional independent defect.

<a id="review-7-1"></a>

### Claim 7.1 — Single `lib.rs` holds four public exports + two helpers; the ABI emitter is duplicated four times

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

The closely coupled group exception applies; no current behavioral defect follows merely from organization.

Evidence: [CLAUDE.md:505](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L505); [crates/shamir-sdk-macros/src/lib.rs:464](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L464).

Grouping/duplicate: [style-claude-md.md#1](style-claude-md.md#review-1). This is not an additional independent defect.

<a id="review-7-2"></a>

### Claim 7.2 — Zero tests in the crate; TDD protocol and `tests/` layout unfulfilled

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Local/helper gaps persist; downstream generated-function coverage exists and TDD history is not established.

Evidence: [crates/shamir-sdk-macros/Cargo.toml:13](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/Cargo.toml#L13); [crates/shamir-wasm-host/src/tests/compile_tests.rs:19](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/compile_tests.rs#L19).

Grouping/duplicate: [correctness-tdd.md#1](correctness-tdd.md#review-1). This is not an additional independent defect.

<a id="review-7-3"></a>

### Claim 7.3 — Divergent duplicated return-type validation: `#[function]` bypasses the shared helper

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Function bypasses the helper and exhibits a real qualified-type acceptance difference.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:197](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L197); [crates/shamir-sdk-macros/src/lib.rs:330](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L330).

Grouping/duplicate: [api-wire-protocol.md#1](api-wire-protocol.md#review-1). This is not an additional independent defect.

<a id="review-7-4"></a>

### Claim 7.4 — Inline comment under-documents the normalisation chain

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

The comment does not claim an exhaustive list and adjacent documentation names core::result::. Expanding it is optional style.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:408](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L408); [crates/shamir-sdk-macros/src/lib.rs:413](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L413).

Grouping/duplicate: [style-claude-md.md#4](style-claude-md.md#review-4). This is not an additional independent defect.

## Revalidated plan decisions

| Plan decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 14 | 12 | 0 | 0 | 0 | 0 | 2 |

Historical P0/P1/P2 numbering is an identifier, not a current release mandate. The reasons below include completion status, safety qualifications and discriminating acceptance requirements.

<a id="plan-p0-1"></a>

### Plan P0.1 — P0.1

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Local/helper/UI and validator/boundary tests remain missing. Reuse registered function coverage; qualify target/tooling skips and unlocked guest resolution. Do not execute unchecked raw-slice UB as a pre-fix test.

Evidence: [crates/shamir-sdk-macros/Cargo.toml:13](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/Cargo.toml#L13); [crates/shamir-wasm-host/src/tests/compile_tests.rs:19](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/compile_tests.rs#L19); [crates/shamir-wasm-host/src/compile.rs:535](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L535).

<a id="plan-p0-2"></a>

### Plan P0.2 — P0.2

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Shared consistent return acceptance is not implemented. Syntax matching cannot resolve aliases; retain concrete SDK typing. Define generic support explicitly instead of claiming E0207 or automatically rejecting workable lifetime-only signatures.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:197](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L197); [crates/shamir-sdk-macros/src/lib.rs:219](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L219); [crates/shamir-sdk-macros/src/lib.rs:411](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L411).

<a id="plan-p0-3"></a>

### Plan P0.3 — P0.3

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Decode failure is still suppressed. Reject malformed/non-map payloads explicitly while preserving valid empty function parameters and legitimate null/deletion record semantics. Testing emptiness alone is an incompatible recipe.

Evidence: [crates/shamir-sdk/src/__rt.rs:11](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/__rt.rs#L11); [crates/shamir-engine/src/validator/wasm_record_validator.rs:60](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/validator/wasm_record_validator.rs#L60).

<a id="plan-p1-4"></a>

### Plan P1.4 — P1.4

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Spanned semantic errors and a testable expansion seam remain absent. A syn::Result helper with compile-error conversion is compatible if successful expansion semantics are preserved.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:51](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L51); [crates/shamir-sdk-macros/src/lib.rs:503](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L503).

<a id="plan-p1-5"></a>

### Plan P1.5 — P1.5

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

User classification remains lost. Coordinate an explicitly identified/versioned guest-host result protocol and legacy success decoding. Untagged envelope maps can collide with arbitrary Value maps; retrofitted leading bytes can collide with legacy MessagePack. Panic prefixes are not a proven repair.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:251](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L251); [crates/shamir-sdk/src/__rt.rs:19](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/__rt.rs#L19); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:578](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L578); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:593](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L593).

<a id="plan-p1-6"></a>

### Plan P1.6 — P1.6

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Negative-length defense is absent. Preserve non-null zero-length representation and address arbitrary safe export callers through an explicit unsafe contract or validated ownership protocol. Sign checks alone do not close provenance/extent/aliasing obligations.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:109](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L109); [crates/shamir-sdk-macros/src/lib.rs:120](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L120); [crates/shamir-sdk-macros/src/lib.rs:123](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L123).

<a id="plan-p1-7"></a>

### Plan P1.7 — P1.7

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Guest polling policy/bound remains undocumented at macro entrypoints and unimplemented locally. Choose finite-Pending support deliberately; distinguish local rejection from host fuel exhaustion. Existing async host imports do not require an executor replacement.

Evidence: [crates/shamir-sdk/src/__rt.rs:50](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/__rt.rs#L50); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:195](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L195).

<a id="plan-p1-8"></a>

### Plan P1.8 — P1.8

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Pattern forwarding is unchanged. Fresh wrapper bindings can preserve arbitrary original body patterns and owned forwarding. Merely forwarding an ident extracted from a ref-bound wrapper is insufficient.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:80](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L80); [crates/shamir-sdk-macros/src/lib.rs:102](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L102); [crates/shamir-sdk-macros/src/lib.rs:530](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L530).

<a id="plan-p1-9"></a>

### Plan P1.9 — P1.9

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Clones and retention remain. Move extraction should preserve first-match duplicate-key behavior and null/absence semantics. A general swap_remove API can change raw() ordering; scope extraction or preserve that observable order. Dropping after cloning does not eliminate transient peak copies.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:130](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L130); [crates/shamir-sdk-macros/src/lib.rs:137](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L137); [crates/shamir-sdk/src/params.rs:26](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/params.rs#L26); [crates/shamir-sdk/src/params.rs:91](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/params.rs#L91).

<a id="plan-p2-10"></a>

### Plan P2.10 — P2.10

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Ignored attributes and canonical-name dependence remain. Export uniqueness already enforces single entry. Module-local/anonymous const sentinels cannot provide the promised cross-module diagnostic; do not treat them as a safety repair.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:44](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L44); [crates/shamir-sdk-macros/src/lib.rs:108](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L108); [crates/shamir-sdk-macros/src/lib.rs:525](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L525).

<a id="plan-p2-11"></a>

### Plan P2.11 — P2.11

Status: `not-applicable`.

Prior-cycle decision: `confirmed-open`.

Emitter extraction is optional maintenance, not required defect closure under the closely coupled exception. If chosen, retain annotated root entrypoints and delegate privately; a conventional re-export-only proc-macro facade is not a safe assumed recipe.

Evidence: [CLAUDE.md:505](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L505); [crates/shamir-sdk-macros/src/lib.rs:43](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L43); [crates/shamir-sdk-macros/Cargo.toml:9](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/Cargo.toml#L9).

<a id="plan-p2-12"></a>

### Plan P2.12 — P2.12

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

The incorrect procedure body remains. query(None)? followed by Value::List preserves the declared SDK return contract.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:292](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L292); [crates/shamir-sdk/src/prelude.rs:35](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/prelude.rs#L35).

<a id="plan-p2-13"></a>

### Plan P2.13 — P2.13

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Scalar purity wording still overpromises; document the actual signature restriction and host-policy boundary. The normalization-comment expansion is optional. Lifetime documentation and fresh Store teardown already exist, so adding free/high-water machinery is not current leak remediation.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:453](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L453); [crates/shamir-sdk/src/context.rs:64](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/context.rs#L64); [crates/shamir-sdk/src/host_imports.rs:55](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/host_imports.rs#L55); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:474](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L474).

<a id="plan-p2-14"></a>

### Plan P2.14 — P2.14

Status: `not-applicable`.

Prior-cycle decision: `unverified`.

The plan explicitly permits retaining initialized allocation, which remains valid and avoids disclosure hazards. No measured benefit establishes a remediation obligation. Any optional optimization requires sound uninitialized storage and initialization ownership, not a premature u8 slice.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:110](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L110); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:540](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L540).

## Additional observations

| Observation decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 2 | 2 | 0 | 0 | 0 | 0 | 0 |

Existing observation IDs remain stable. New cycle-2 rows follow them; cross-module duplicates and extra triggers are grouped explicitly. None is an implemented fix.

<a id="observation-new-1"></a>

### Observation NEW.1 — Raw function identifiers panic during helper-name construction

Status: `confirmed-open`. Current risk: `low`.

Additional observation in this independent cycle; it may overlap an existing root.

A valid annotated async function named r#type reaches format("__shamir_impl_{}", fn_name), producing __shamir_impl_r#type, which is not an identifier. proc-macro2 1.0.106 preserves the raw prefix in Display and rejects invalid characters in Ident::new; published src/fallback.rs:805/850/910 and compiler delegation in src/wrapper.rs:674 confirm the mechanism: https://docs.rs/crate/proc-macro2/1.0.106/source/src/fallback.rs. The compiler-facing constructor contract also requires a valid identifier: https://doc.rust-lang.org/proc_macro/struct.Ident.html. All four macros use this construction. A compile-pass raw-name fixture would discriminate a repair; no reproduction was executed.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:48](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L48); [crates/shamir-sdk-macros/src/lib.rs:180](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L180); [crates/shamir-sdk-macros/src/lib.rs:311](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L311); [crates/shamir-sdk-macros/src/lib.rs:468](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L468); [Cargo.lock:2537](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2537).

<a id="observation-new-2"></a>

### Observation NEW.2 — Generated standard-library names remain caller-shadowable

Status: `confirmed-open`. Current risk: `low`.

Additional observation in this independent cycle; it may overlap an existing root.

All allocators emit bare Vec&lt;u8&gt;; validator also emits bare Option/Some/None, and core paths lack a leading root qualifier. A consumer with an otherwise valid qualified procedure signature and a module-local nongeneric Vec changes resolution of the generated allocator annotation and prevents compilation. This is independent of Cargo dependency renaming. quote 1.0.45 generates call-site spans: https://docs.rs/crate/quote/1.0.45/source/src/lib.rs; the Rust Reference confirms unhygienic inline resolution: https://doc.rust-lang.org/reference/procedural-macros.html. A consumer compile-pass fixture shadowing these names would catch the defect.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:96](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L96); [crates/shamir-sdk-macros/src/lib.rs:110](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L110); [crates/shamir-sdk-macros/src/lib.rs:112](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L112); [crates/shamir-sdk-macros/src/lib.rs:135](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L135); [crates/shamir-sdk-macros/src/lib.rs:371](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L371); [Cargo.lock:2660](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2660).

## Evidence and recipe corrections

- Current claim totals are ledger decisions, not unique defects or current severity counts; repeated rows are explicitly linked here.
- Treat uniqueness diagnostics, emitter/file organization, exhaustive inline comments and unmeasured zero-fill removal as optional work, not confirmed safety defects or release mandates.
- The registered function runtime test is real but skippable; its scaffold builds a separate guest graph without --locked. Do not certify frozen dependency execution from registration alone.
- A structured result change requires an identified compatibility boundary. Neither arbitrary envelope maps nor an unversioned leading marker safely distinguish all legacy successful Values.
- The generated-path and parsed-identifier assurances miss bare-name hygiene and raw-identifier helper panics, documented as new mechanisms below.
- Malformed-input decoding, missing required record and legitimate deletion Null must remain separate. The ordinary adapter makes both keys explicit.
- All inspected macro/runtime history is unchanged since the earlier implementation commits; positive counter-evidence is refutation, not source-fixed closure.
- Trap-text disclosure must not be deduplicated into classification loss as though an envelope automatically proves both.

## Module scope and limitations

Coverage: 8 assigned documents, 86 current claim rows, 14 plan rows, 0 pre-existing observation rows; 2 added observation rows in this cycle. Counts are calculated from the accepted rows.

Assigned documents: [correctness-tdd.md](correctness-tdd.md); [concurrency-lockfree.md](concurrency-lockfree.md); [security-crypto.md](security-crypto.md); [performance-hotpath.md](performance-hotpath.md); [api-wire-protocol.md](api-wire-protocol.md); [error-handling-lifecycle.md](error-handling-lifecycle.md); [style-claude-md.md](style-claude-md.md); [SUMMARY.md](SUMMARY.md).

- Read-only inspection only; no files written and no builds, tests, reproductions or benchmarks executed.
- HEAD matched the required base. Relevant source was unchanged; modified dependency, manifest, toolchain and CI metadata was inspected through git show at the frozen base.
- Exact compiler diagnostics, guest panic-text transport, optimized allocation behavior and quantitative CPU/memory/latency effects were not experimentally established.
- The registered generated-function runtime test can skip for unavailable tooling. Its temporary guest build does not enforce the workspace Cargo.lock, so registration does not certify execution against the frozen dependency graph.
- Exact cached source and published archive contents were inspected for syn 2.0.114, quote 1.0.45, proc-macro2 1.0.106, rmp-serde 1.3.1 and wasmtime 46.0.2; archive SHA-256 values matched the frozen lockfile.
- Bounded claim revalidation, not an exhaustive new audit. No assigned TASK_GROUPS.md or numbered Observation rows exist.

## Guarantee checks

- **Ordinary async SDK signatures are transformed into the documented fixed guest ABI.** — `diverges`. Complete parameter patterns are reused as expressions, rejecting legitimate mut/ref/wildcard arguments. Qualified SDK return types are also inconsistently rejected. Reference: crates/shamir-sdk-macros/src/lib.rs:74.
- **Only one annotated entrypoint per guest crate is supported.** — `supported`. Every expansion defines the same unmangled allocator and call symbols. Duplicate expansions cannot produce a valid multiple-entrypoint artifact; a specialized diagnostic is optional. Reference: crates/shamir-sdk-macros/src/lib.rs:108.
- **A scalar cannot access the database, other functions or HTTP.** — `diverges`. A valid Params-only scalar body can construct public Ctx::new(). Host import registration is independent of macro kind; actual effects require the corresponding gateway and authorization. Reference: crates/shamir-sdk-macros/src/lib.rs:453.
- **Malformed ABI input remains distinguishable from valid parameters and validator records.** — `diverges`. Decode failures and non-map values become empty Params, then validator Null/None fallbacks. Ordinary engine invocation supplies both keys; a remote UPDATE bypass was not demonstrated. Reference: crates/shamir-sdk/src/__rt.rs:11.
- **Guest Error::user denotes a deliberate user-surfaced failure.** — `diverges`. Generated Err branches panic and the host maps the resulting runtime error to Compute. The guest Error is message-only, not a variant taxonomy. Reference: crates/shamir-sdk/src/error.rs:13.
- **Guest buffers are valid until invocation teardown and do not accumulate across ordinary calls.** — `supported`. Each invocation owns a fresh Store and instance. wasmtime 46.0.2 published src/runtime/store.rs:2565 destroys Store state and deallocates instances: https://docs.rs/crate/wasmtime/46.0.2/source/src/runtime/store.rs. Pin: Cargo.lock:4740. Reference: crates/shamir-sdk/src/host_imports.rs:55.
- **Async host imports require the guest Rust future to return Pending.** — `diverges`. The synchronous guest extern call bridges into Wasmtime fiber suspension. wasmtime 46.0.2 src/runtime/fiber.rs:291 suspends on host-future Pending: https://docs.rs/crate/wasmtime/46.0.2/source/src/runtime/fiber.rs. Pin: Cargo.lock:4740. Reference: crates/shamir-wasm-host/src/wasm/wasm_function.rs:195.
- **Guest-local Pending execution has an intrinsic polling bound.** — `diverges`. The helper repolls indefinitely with a no-op waker. Production fuel and epoch interruption bound guest execution externally; finite Pending sequences can complete. Reference: crates/shamir-sdk/src/__rt.rs:50.
- **No test exercises generated function ABI behavior.** — `diverges`. Registered compile_and_invoke_double compiles the actual macro, invokes WASM and asserts 42. It catches changed forwarding/encoding/results when the compile path executes, but permits tooling-based skips. Reference: crates/shamir-wasm-host/src/tests/compile_tests.rs:19.
- **Generated infrastructure uses hygienic library paths and safely derives helper identifiers.** — `diverges`. Bare Vec/Option/Some/None and relative core paths remain caller-sensitive. Formatting a raw function identifier into Ident::new also creates an invalid helper name; these are additional mechanisms. Reference: crates/shamir-sdk-macros/src/lib.rs:48.
- **The macro crate conforms to builder-only query construction and owns no runtime locking.** — `supported`. It constructs no raw JSON query and introduces no lock or guard. The procedure example uses the typed Table API, although its result handling is incorrect. Arbitrary author bodies are excluded from this assurance. Reference: crates/shamir-sdk-macros/src/lib.rs:292.
- **Unused function generics cause E0207.** — `diverges`. The expansion creates generic functions, not an unconstrained impl. The documented E0207 rule concerns impl parameters: https://doc.rust-lang.org/error_codes/E0207.html. Unselected type parameters can instead create inference problems. Reference: crates/shamir-sdk-macros/src/lib.rs:219.

## Reviewer's prior-cycle comparison

These are the independent reviewer's comparisons before parent refinements; the accepted ledgers above govern final decisions and counts.

- correctness-tdd.md#4, security-crypto.md#6 and SUMMARY.md#1.4/#3.6: confirmed-open -&gt; refuted. Fixed ABI symbol collisions already enforce the only documented single-entrypoint guarantee; specialized diagnostics are optional.
- style-claude-md.md#1, api-wire-protocol.md#9 and SUMMARY.md#5.9/#7.1: confirmed-open -&gt; not-applicable. The closely coupled exception prevents an unconditional file-layout violation; duplication alone is maintenance preference, not current wire failure.
- style-claude-md.md#4 and SUMMARY.md#7.4: confirmed-open -&gt; not-applicable. The inline comment does not claim exhaustiveness, and adjacent documentation includes core::result::.
- SUMMARY.md Plan P2.11: confirmed-open -&gt; not-applicable; Plan P2.14: unverified -&gt; not-applicable. Neither optional extraction nor optional uninitialized allocation is a required current defect repair.
- The first-cycle corrections concerning existing function ABI coverage, fresh Store reclamation, fiber-backed host async imports, conditional malformed-validator reachability and unproven panic-text disclosure are independently supported.
- Generic E0207 remains an incorrect mechanism, not an observed diagnostic. A lack of generic support policy remains unverified; rejecting all generic signatures is not automatically compatible.
- Additional source-derived corrections concern raw-identifier totality, bare-name hygiene, safe public raw-slice exports, ordering effects of a take/swap_remove API and legacy result-envelope collisions.
- No implementation fix, test success or measured performance outcome is claimed.

## Current follow-up order

1. Establish discriminating helper/UI and guest-target boundary tests, retaining existing function coverage and making tooling skips/dependency resolution explicit.
2. Repair pattern forwarding and return acceptance with spanned diagnostics; include raw identifiers and caller-shadowed library names.
3. Make decode failure explicit while preserving valid empty parameters, explicit record keys and deletion Null semantics.
4. Coordinate an identified, backward-compatible guest/host user-error protocol; assert User versus actual Compute failure.
5. Define guest-local Pending support and bounded rejection with an oracle distinct from host fuel/epoch exhaustion.
6. Strengthen the raw ABI safety contract and negative-length defense without invoking UB or breaking zero-length handling; then transfer validator ownership with ordering/duplicate-key semantics preserved.
7. Correct purity guarantees and the procedure example. Treat emitter organization, custom uniqueness diagnostics, comment expansion and allocation optimization as optional follow-up.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-sdk-macros — Consolidated 7-Lens Review (synthesized)

Crate: `crates/shamir-sdk-macros/` — a 572-line proc-macro crate (single `src/lib.rs`, no
other source files) exporting four `#[proc_macro_attribute]`s (`validator`, `function`,
`procedure`, `scalar`) that lower an author's `async fn` into a WASM guest module emitting
the `shamir_alloc`/`shamir_call` guest ABI against `shamir_sdk::__rt`.

Review basis: synthesis of the seven 2026-08-14 cross-crate lens reports for this crate —
`correctness-tdd.md`, `concurrency-lockfree.md`, `security-crypto.md`,
`performance-hotpath.md`, `api-wire-protocol.md`, `error-handling-lifecycle.md`,
`style-claude-md.md` (all in this directory) — deduplicated per the workspace SUMMARY.md
convention and formatted for calibration after the exemplar syntheses
`shamir-client-node/SUMMARY.md` and `shamir-transport-ipc/SUMMARY.md`.
During synthesis the cited `file:line` references were spot-checked against
`src/lib.rs`, `Cargo.toml`, and the sibling-crate evidence sites
(`shamir-sdk/src/__rt.rs`, `params.rs`, `db.rs`, `prelude.rs`,
`shamir-sdk/tests/{scalar,procedure}_compile_pass.rs`); no source file was modified and
no build/test/lint command was run. **No new defects were found during spot-checking.**

## Executive summary

The generated ABI is behaviorally correct for the flows the SDK actually advertises (all
current host imports are synchronous and Ready-on-first-poll), and the bump-allocator
leak design is deliberate and currently honored by the host — but the crate is **not
shippable as maintained**: it has **zero test coverage anywhere in the workspace** (two
of four macros are compiled by no test at all), so every fix below is unguarded against
regression, and its **string-based return-type validation is inconsistent across the four
macros**, spuriously rejecting `shamir_sdk::Result<Value>`/`shamir_sdk::Validation` —
spellings this repo's own tests use. Fix first: (1) build the test seam and coverage,
(2) unify return-type validation into one shared structural checker, (3) make param-decode
failures loud — today `#[validator]` silently coerces undecodable params into
`record = Null`/`old_record = None`, so a corrupted UPDATE presents to the validator as an
INSERT. This matches the workspace scorecard verdict: *#19 — "lean but untested — zero
coverage anywhere; spurious rejections of valid signatures"* (0c / 2h).

---

## 1. correctness-tdd

### 1.1 — high — Zero test coverage: TDD protocol not honored; even pure helpers are untested *(dedup primary — also flagged by 2.2, 3.3, 5.5, 6.3, 7.2)*
- File:line: crate-wide (`src/lib.rs`, `Cargo.toml`; no `src/**/tests/` or `tests/`
  directory exists; no dev-dependencies in `Cargo.toml`; `doctest = false` with the
  doc examples ` ```ignore `-fenced).
- Issue: CLAUDE.md's mandatory Red/Green/Refactor protocol and the per-module `tests/`
  layout are both skipped; the pre-commit gate runs zero tests for this crate.
  Concretely untested: (a) the two pure helpers `is_result_value_return`
  (`lib.rs:411-420`) and `type_contains_ctx` (`lib.rs:425-432`) — trivially unit-testable
  today via `syn::parse_str::<syn::Type>`; (b) `#[validator]` and `#[function]` — compiled
  by no test in the workspace (sibling compile-pass tests cover only `scalar`/`procedure`);
  (c) all ~15 `assert!`/`panic!` validation branches (messages and existence); (d) the
  generated `shamir_call` runtime paths — malformed msgpack, negative `len`, user `Err`,
  and the `(ptr << 32) | len` packing contract with the host; (e) the concurrency-critical
  "Ready on first poll" contract of the emitted ABI (the only coverage is indirect and
  Ready-only: `shamir-sdk/src/tests/value_tests.rs::block_on_resolves_immediately`); (f) no
  trybuild/compile-fail UI tests anywhere in the workspace.
- Failure scenario: any refactor of the macro (fixing findings 1.2/5.1 below, or the
  `TODO(slice 4)` envelope rework at `lib.rs:251`) can silently change generated code,
  acceptance spellings, or error messages — nothing fails, and the first signal is a
  guest fuel-exhaustion trap or spurious rejection far from the change. Finding 3.1 has
  survived precisely because nothing observes it.
- Suggested fix: add `src/tests/` per the CLAUDE.md layout (`tests/mod.rs` manifest +
  `return_type_tests.rs`/`ctx_detection_tests.rs` unit-testing the helpers through
  `syn::parse_str`); add `validator_compile_pass.rs`/`function_compile_pass.rs` beside the
  existing sibling tests (the separate-crate pattern avoids `#[no_mangle]` symbol
  collisions); add a trybuild UI test per `assert!` diagnostic; refactor each macro body
  through a `fn core(input: TokenStream2) -> syn::Result<TokenStream2>` seam (see 6.1) so
  expansion shape is snapshot-testable; add one round-trip ABI test (encode params →
  `shamir_call` → decode result) driving both the `Ok` and `Err` arms — written Red first
  for 3.1.

### 1.2 — medium — User parameter patterns re-used as call-position expressions (`mut x`, `_`, `ref x` break the expansion)
- File:line: `src/lib.rs:74-87` + `:102` (validator), `:204-217` + `:232` (function),
  `:337-349` + `:363` (procedure), `:506-517` + `:530` (scalar).
- Issue: the macros extract `PatType.pat` and splice the same pattern into two positions:
  (a) the re-typed inner signature `#arg0: shamir_sdk::Value` — tolerates any pattern —
  and (b) the forwarded call `#fn_name(#arg0, #arg1, #arg2).await` — requires a plain
  ident. syn's `ToTokens for PatIdent` emits the `mut`/`ref` tokens, and `Pat::Wild`
  renders as `_`, which is not an expression.
- Failure scenario: a perfectly legal, idiomatic
  `#[validator] pub async fn check(mut record: Value, old: Option<Value>, ctx: Ctx) -> Validation`
  expands to `__shamir_impl_check(mut record, ...)` → `error: expected expression, found
  keyword `mut`` deep inside the expansion. Same for `fn f(_: Params)` and `ref record`.
  The user gets no hint their signature is "fine but unlucky".
- Suggested fix: when extracting args, accept only `Pat::Ident` without `subpat`; map `_`
  (and non-ident patterns) to fresh generated idents (`format_ident!("__shamir_arg{i}")`)
  used in both positions, or emit a clear `syn::Error::new_spanned(pat, "...")`.

### 1.3 — medium — *(dedup: same defect as 5.1)* — String-based return-type validation is inconsistent across sibling macros
- (Full write-up at 5.1, whose lens tagged it high.) This lens's angle: the *same* return
  spelling is valid on two macros and rejected on the third; `is_result_value_return`
  strips `core::result::` but not `std::result::`; `#[validator]` false-accepts any local
  type coincidentally named `Validation`, which then fails later inside the expansion at
  `into_value()`.

### 1.4 — low — "Only one `#[...]` per crate" contract documented but not enforced *(dedup primary — also flagged by 3.6, 5.7)*
- File:line: doc claims at `src/lib.rs:15, 157, 283, 437`; no enforcement anywhere.
- Issue: all four macro docs state "**Only one per crate is supported** (single
  entrypoint)" because each application emits `#[no_mangle] shamir_alloc`/`shamir_call`,
  but nothing detects a second application. Two macros in the same module give E0428
  (tolerably clear); two in *different* modules compile and fail only at link time with an
  opaque duplicate-symbol error naming `shamir_call`, with no pointer to the entrypoint
  contract. The ABI safety story (exactly one export pair per module) rests on prose.
- Failure scenario: user adds `#[function]` beside an existing `#[procedure]` in another
  module; wasm link fails with `multiple definition of 'shamir_call'` and no diagnosis.
- Suggested fix: emit a fixed-name sentinel item (e.g.
  `const SHAMIR_SDK_ENTRYPOINT_TAKEN: () = ();`) alongside the exports so a second
  application across any module produces a duplicate-definition error naming the sentinel,
  or at minimum document how to resolve the link error.

### 1.5 — low — *(dedup: same defect as 6.1)* — `assert!`/`panic!` diagnostics instead of `syn::Error` compile errors
- (Full write-up at 6.1.) This lens's angle: the bare `panic!("...must return
  Validation")` arms (`lib.rs:71, 201, 334, 503`) also drop the actual offending type from
  the message.

### 1.6 — low — Generic functions hit E0207 inside the expansion instead of a clear rejection
- File:line: `src/lib.rs:89, 219, 351, 519` (`split_for_impl` usage).
- Issue: the macros copy the user's generics onto the inner fn, but the inner signature is
  re-typed to concrete SDK types. A generic parameter used only in the user's own
  parameter types becomes unused on the inner fn → `error[E0207]: the type parameter `T`
  is not defined... parameter `T` is never used`, deep in generated code. Generics are
  meaningless for a fixed WASM ABI and should be rejected outright. (Also noted as a
  sub-clause of 5.1: `split_for_impl` half-forwards `impl_generics` that the concrete
  `shamir_call` body can never satisfy.)
- Failure scenario: `#[function] async fn f<T: Display>(ctx: Ctx, b: Batch, p: Params) ->
  Result<Value>` passes the check phase (arity 3, return matches) then fails with E0207
  pointing into macro output.
- Suggested fix: early rejection (`syn::Error` per 6.1) on
  `!fn_item.sig.generics.params.is_empty()` with "generic entrypoints are not supported".

### 1.7 — low — *(dedup: same defect as 3.2)* — `shamir_alloc` does not guard negative or zero `len`
- (Full write-up at 3.2, which also covers the `from_raw_parts` UB half.)
  This lens's angle: `len == 0` returns the dangling align-1 pointer of an empty `Vec`
  (mostly harmless, but a footgun for host code that does not special-case `len == 0`).

### 1.8 — low — *(dedup: same defect as 2.1)* — Emitted `__rt::block_on` busy-spins forever on `Pending`
- (Full write-up at 2.1, the concurrency lens's primary.)

### 1.9 — nit — `type_contains_ctx` is a name heuristic and the `#[scalar]` purity claim overreaches *(dedup primary — also flagged by 5.10)*
- File:line: `src/lib.rs:425-432` (helper), `:434-462` (`#[scalar]` docs).
- Issue: purity is enforced only by token-string matching on the single parameter. An
  alias (`type C = shamir_sdk::Ctx` → `x: C`) bypasses the check (failing later with an
  unrelated expansion error); conversely a user's coincidental `my_app::Ctx` type is
  falsely rejected. The docs promise the scalar "cannot access the database ... or perform
  HTTP requests" — which the macro cannot enforce at all, since the body could call
  `Ctx::new()` directly (it is `pub`). The actual guarantee is "no `Ctx`-typed parameter +
  `Ctx::new()` is inert" — an SDK property, not a macro property.
- Suggested fix: match `Ctx` only when the path's last segment is `Ctx`; soften the doc
  wording to what is enforced ("declared as `Ctx`"), noting the alias limitation.

### 1.10 — nit — Generated code hardcodes `shamir_sdk::` paths; `_attr` tokens silently ignored
- File:line: `src/lib.rs:44, 176, 307, 464` (`_attr` unused); literal `shamir_sdk::` paths
  in every `quote!` block (e.g. `:95-98, :126, :464`).
- Issue: all emitted items reference `shamir_sdk::*` by literal path, so a dependency
  rename (`package = "shamir-sdk"` under another key) breaks every expansion. `_attr` is
  discarded without checking it is empty, so `#[validator(anything)]` is silently accepted
  (typo'd options do nothing — the same half is bundled into 5.7). Both are standard
  proc-macro trade-offs in a single-consumer workspace.
- Suggested fix: at minimum assert `_attr` is empty with a clear message; consider
  `proc-macro-crate` only if renamed consumption becomes real.

---

## 2. concurrency-lockfree

Lens verdict: the macro crate itself is trivially clean — no `Mutex`/`RwLock`/`parking_lot`,
no atomics, no `scc`/`dashmap`/`ArcSwap`; only `syn`/`quote`/`proc-macro2` (compile-time,
single-threaded). Nothing it emits holds a guard across an `.await`. The one real
theme-relevant surface is inherited by construction:

### 2.1 — medium — Generated guest ABI drives the author's `async fn` on a spin-on-`Pending` executor — latent busy-wait/livelock, undocumented and unguarded *(dedup primary — also flagged by 1.8, 4.3, 6.7)*
- File:line: `crates/shamir-sdk-macros/src/lib.rs:144` (`#[validator]`), `:264`
  (`#[function]`), `:391` (`#[procedure]`), `:556` (`#[scalar]`); mechanism in sibling
  crate `crates/shamir-sdk/src/__rt.rs:36-61` (spin at `:50-59`, its own comment concedes
  "If a future genuinely needs async I/O (slice 4 host imports), this will spin").
- Issue: every expansion emits `shamir_sdk::__rt::block_on(#inner_name(...))` as the sole
  bridge from the sync `extern "C"` entrypoint to the author's `async fn`. `block_on`
  polls with a no-op waker and, on `Poll::Pending`, enters `core::hint::spin_loop()`
  forever — with a no-op waker, wake notifications are impossible, so the spin is not even
  a correct wait. The macro crate originates this pattern yet never discloses the "must
  resolve on first poll" contract in any of the four macro docs, and applies no static or
  runtime guard (no deadline, no poll budget, no named trap). This is in tension with
  CLAUDE.md pillar 2 and the workspace's zero-tolerance stance on hangs; the `#[procedure]`
  doc example itself (`lib.rs:291-294`) advertises I/O-shaped work
  (`ctx.db().table("users").query(None)`) under an executor that cannot tolerate a single
  `Pending` poll.
- Failure scenario: latent today — all host imports (`shamir-sdk/src/host_imports.rs`)
  are synchronous, so SDK-provided awaits resolve on first poll. It becomes live the moment
  (a) a guest author awaits anything that pends (the guest burns WASM fuel at spin-loop
  rate until the host's fuel/epoch meter traps it — verified bounds in
  `shamir-wasm-host/src/wasm/wasm_function.rs:480-487, 549-559` — or hangs forever without
  metering), or (b) the planned slice-4 async host imports land, at which point the first
  real I/O-bound `#[procedure]` traps or hangs deterministically, starving every other task
  on that tokio worker until the deadline.
- Suggested fix: in this crate — state the Ready-first contract explicitly in each macro's
  doc ("a `Pending` poll spins the guest; use only immediately-ready awaits until slice-4
  host imports"); when slice 4 lands, replace the emitted lowering with a host-suspend
  waker or gate it behind the then-current sanctioned primitive. In the sibling crate (out
  of this review's scope, but the actual guard): cap consecutive `Pending` polls and
  trap/panic with a named message ("future never resolved") instead of spinning —
  coordinate with the `shamir-sdk` owner since the primitive is shared.

### 2.2 — low — *(dedup: same defect as 1.1)* — The "Ready on first poll" contract of the emitted ABI has zero test coverage
- (Full write-up at 1.1.) This lens's angle: nothing asserts the macro-emitted ABI contains
  exactly one `__rt::block_on` and no lock primitives, and no fixture exercises a
  once-pending-then-resolving future or trips the (post-fix) bounded trap — so a regression
  that swaps the executor or makes even the pure path pend is invisible to CI.

---

## 3. security-crypto

Lens verdict: no cryptographic primitives, auth, HMAC/SCRAM/TLS, or timing-sensitive code
exists in this crate; the entire security surface is the generated WASM guest ABI and the
crossing of host-supplied msgpack params into author code. Verified clean: no `unsafe`
outside the four generated `from_raw_parts` blocks and none in the macro code itself;
`shamir_alloc` zero-initializes before `mem::forget` (no uninit-memory disclosure); no
injection surface (generated identifiers come only from the author's own `Ident`).

### 3.1 — medium — `#[validator]` silently coerces undecodable params into `record = Null`, `old_record = None` — UPDATE presents as INSERT *(dedup primary — also flagged by 5.2, 6.4)*
- File:line: `src/lib.rs:129-139` (generated inside `validator`'s `shamir_call`: `:129-132`
  `record` → `Null`, `:135-139` `old_record` → `None`); enabling factor in
  `crates/shamir-sdk/src/__rt.rs:11-16` (`decode_params` returns an *empty* `Params` for
  any decode failure or non-map payload).
- Issue: two silent swallows in series. The `Err` arm conflates three distinct situations:
  key genuinely absent, key absent because decoding failed, and key absent because of
  host↔guest contract drift (schema change, key rename) — and `Err(_)` discards the cause
  entirely. For `record`, the documented authoring pattern (`record == Value::Null →
  record_error`, `lib.rs:23-28`) happens to be fail-closed; but for `old_record`, `None`
  is conventionally "insert", so an undecodable UPDATE payload presents to the validator
  as an INSERT and every old-record comparison is silently skipped. There is no version
  field, no handshake, and no way for the guest to say "the payload itself was invalid";
  the conflation is not documented as a design decision.
- Failure scenario: host-side parameter-schema drift or a corrupted parameter buffer → the
  module does not trap or log anything; the validator receives `(Null, None)` instead of a
  failure. Either all writes fail validation with "empty_record" (fail-closed DoS with a
  misleading domain error sending debuggers down the wrong path — the host records a *data
  validation failure* when the real cause is protocol incompatibility), or — for
  validators treating `old_record: None` as an insert — all old-record checks are bypassed.
  No signal reaches the host or the module author that validation ran on the wrong data.
- Suggested fix: make the decode failure loud — have `__rt::decode_params` return `Result`
  and `trap!` on `Err` (mirroring how the other three macros already trap on user `Err`),
  or trap when `len > 0` but the decoded `Params` is empty; keep absent-key → `Null`/`None`
  only for genuinely absent keys; at minimum `old_record`'s `Err(_) => None` must
  distinguish "absent" from "undecodable", and the intentional Null-on-error semantic
  should be documented in the `#[validator]` doc.

### 3.2 — low — Generated ABI constructs `unsafe` slices from unverified `(ptr, len)` and allocates from unvalidated `i32` — negative `len` is immediate UB *(dedup primary — also flagged by 1.7, 4.4, 5.8, 6.6)*
- File:line: `src/lib.rs:120-124` (`validator`), `:253-257` (`function`), `:381-385`
  (`procedure`), `:547-551` (`scalar`) — `core::slice::from_raw_parts(ptr as *const u8,
  len as usize)`; sibling sites `shamir_alloc` at `:109-114, 239-244, 370-375, 537-542`
  (`vec![0u8; len as usize]`).
- Issue: all four generated `shamir_call`s take `ptr` entirely on faith, and `len: i32`
  negative wraps to a ~4 GiB `usize` on wasm32, violating `from_raw_parts`'s safety
  contract the moment the slice is formed — undefined behavior on the first read, before
  msgpack decoding could bound anything. The `// Safety:` comments lean entirely on the
  host contract ("the host wrote `len` msgpack bytes at `ptr` via shamir_alloc") — a
  precondition the guest cannot verify, which a buggy host (sign/truncation slip when
  unpacking the `(ptr, len)` i64, or a `-1` sentinel) violates silently. On the alloc
  side, `shamir_alloc(-1)` requests `vec![0u8; ~2^32/2^64]` → allocation-failure abort →
  trap: fail-closed and sandbox-contained, but an opaque, uninstrumented death rather than
  a diagnosable error. WebAssembly's bounds-checks contain the worst case, so this is
  defense-in-depth consistent with the project's "checksums everywhere" reliability
  posture, not an exploit path. (Sub-point from the performance lens, 4.4: the
  `vec![0u8; len]` zero-fill is redundant work — the host overwrites the full buffer
  immediately via `copy_from_slice` at `wasm_function.rs:540` — only mattering for large
  payloads.)
- Failure scenario: a host-side ABI bug or fuzzed path passes `len = -1`; the guest dies
  with a wild read (UB) or an allocator abort, indistinguishable from a guest OOM, instead
  of trapping with "bad ABI length".
- Suggested fix: one-line guards at each boundary — reject `len < 0` before the `unsafe`
  (e.g. `trap("shamir: bad ABI length")` in `shamir_call`; return `-1`/`0` in
  `shamir_alloc`), or route through `usize::try_from` with an empty-slice fallback; also
  define the `len == 0` dangling-pointer case in the doc. Optionally allocate
  uninitialized via `alloc` + `from_raw_parts_mut` with the "host overwrites the full
  buffer" contract documented at the ABI boundary.

### 3.3 — low — *(dedup: same defect as 1.1)* — No tests cover the two boundary-bearing macros or any runtime behavior of the generated ABI
- (Full write-up at 1.1.) This lens's angle: `#[validator]` and `#[function]` — precisely
  the two macros carrying the extra boundary logic — have neither compile-pass nor
  behavioral tests; write the malformed-msgpack test Red first (it should fail until 3.1
  is fixed).

### 3.4 — low — *(dedup: same defect as 6.2)* — User error strings passed verbatim into the trap channel
- (Full write-up at 6.2.) This lens's angle: the trap text may interpolate untrusted
  parameter content (cf. `Params::i64` formatting the offending value's type,
  `crates/shamir-sdk/src/params.rs:35-43`); if the host ever surfaces trap text to the
  untrusted DB caller, guest-internal details and echoed param data cross the module
  boundary unfiltered — conditional on host policy.

### 3.5 — nit — *(dedup: same defect as 5.1)* — String-comparison return-type checks accept same-named foreign types
- (Full write-up at 5.1.) This lens's angle: every false-accept (e.g. `my_crate::Validation`)
  is compile-caught downstream in the generated coercion — never a runtime hole — but
  boundary validation that both over- and under-matches undermines the "compile-time
  checks" the docs advertise (`lib.rs:302-305, 457-462`).

### 3.6 — nit — *(dedup: same defect as 1.4)* — "Only one macro per crate" constraint unenforced
- (Full write-up at 1.4.) This lens's angle: the failure mode is loud (duplicate
  `#[no_mangle]` symbols fail compilation), so this is not a security hole — but the
  constraint the guest ABI depends on has no enforcement or error message pointing at the
  cause.

---

## 4. performance-hotpath

Lens verdict: the macro-implementation code runs once per annotated function at compile
time (no hot path, no findings there) — this lens judges the *generated* guest ABI, which
executes once per record validation / function call. No test executes the
validator/function-generated `shamir_call` at all, so none of the below is pinned.

### 4.1 — medium — `#[validator]` `shamir_call`: full decoded payload retained across the entire await + redundant deep copies of `record`/`old_record`
- File:line: `src/lib.rs:126-144` (generated code; `:129-139` for the two extractions —
  only `validator` does the extraction).
- Issue: the generated code does `params.get("record")` → `v.clone()` and
  `params.get("old_record")` → `Some(v.clone())`. `Params::get` returns `&Value`
  (`crates/shamir-sdk/src/params.rs:26`), so each `.clone()` is a full deep copy while the
  original stays inside `params`. Because `params` is an owned binding, the entire decoded
  payload (`Params { map: Vec<(String, Value)> }`) stays live across
  `__rt::block_on(#inner_name(record, old_record, ctx))` — the whole author validation.
  Peak live memory per invocation: raw input bytes (leaked in guest linear memory) + full
  decoded `params` map + the two deep clones — and `#[validator]` never passes `params` to
  the author, so the retained map is pure dead weight. Validators run once per record
  insert/update: this is exactly pillar-3's "hidden allocation/retention in the hot path".
- Failure scenario: large user records (payload size attacker/user-controlled) ×
  concurrent validations multiply guest linear memory ~3x per call; with high validation
  fan-out this wastes `memory.grow` bandwidth and pool capacity and can trip the host's
  `StoreLimits` memory cap earlier than necessary.
- Suggested fix: `drop(params)` immediately after the `old_record` extraction (safe:
  `validator` doesn't forward `params`), and avoid the deep copies by moving out of the
  map — add `Params::take(&mut self, key) -> Option<Value>` in `shamir-sdk`
  (`Vec::swap_remove` by position) and emit `params.take("record")` /
  `params.take("old_record")`. Peak footprint drops to ~1x payload.

### 4.2 — low — `shamir_alloc` leaks every allocation; the "module is short-lived" contract is documented only here and enforced nowhere in this crate
- File:line: `src/lib.rs:105-114` (and duplicated at `:235-244, 366-375, 533-542`);
  output-buffer counterpart `crates/shamir-sdk/src/__rt.rs:25-30` (`leak_result`).
- Issue: every `shamir_alloc(len)` call `mem::forget`s its `Vec<u8>`; there is no
  `shamir_free` in the ABI. The doc comment justifies it with "the WASM module is
  short-lived" — an assumption about the *host*, which this crate cannot enforce. It
  currently holds: fresh `instantiate_async` per call under wasmtime pooling (verified at
  `shamir-wasm-host/src/wasm/wasm_function.rs:495-500`), so leaked bytes die with the
  instance. But the contract is one host-side refactor away from breaking. Also within a
  single call: the host's nested-call path (`shamir-wasm-host/src/wasm/host_call.rs:142`)
  allocates a result buffer via `shamir_alloc` mid-guest-call, so one guest call fanning
  out N nested host calls leaks N buffers even under fresh-instance-per-call (bounded only
  by the store's memory limit).
- Failure scenario: host adopts instance reuse/pooling-at-the-Rust-level for throughput;
  validators called once per record grow linear memory without bound until `memory.grow`
  fails and every call traps.
- Suggested fix: either emit a `shamir_free(ptr, len)` export documented as required for
  long-lived guests, or add a module doc / debug-build high-water assertion naming the
  host contract explicitly; at minimum carry the "fresh instance per call is load-bearing"
  note into `shamir-wasm-host`'s call-path docs (cross-crate invariant).

### 4.3 — low — *(dedup: same defect as 2.1)* — Generated `shamir_call` busy-spins at 100% CPU on `Pending`
- (Full write-up at 2.1.) This lens's angle: the host's fuel + epoch deadline + wall-clock
  timeout bound the burn to a trap, so this degrades to worst-case-latency-plus-trap rather
  than an infinite hang — but it burns the entire budget every time and starves unrelated
  tasks on the same worker in the interim.

### 4.4 — nit — *(dedup: same defect as 3.2)* — `shamir_alloc` zero-fills O(len) bytes the host immediately overwrites; negative `len` not rejected
- (Full write-up at 3.2, which carries both the negative-`len` guard and the redundant
  memset observation; the perf-lens-only detail: if payload sizes justify it, allocate
  uninitialized via `alloc` + `from_raw_parts_mut`, otherwise accept the memset.)

---

## 5. api-wire-protocol

Lens verdict: four attribute macros emit the guest ABI against `shamir_sdk::__rt`. The
public-interface weak spot is signature validation — three different string-matching
return-type checks with inconsistent qualification acceptance, and `assert!`/`panic!`
diagnostics. On the wire: no error channel at all — malformed payloads silently degrade to
empty `Params`/`Null` `record`, and user errors surface as bare trap strings (acknowledged
slice-4 TODO). Builder-only query-construction rule: compliant — the crate constructs no
queries/batches/filters and contains no `serde_json` usage; its only query-shaped text is
the `#[procedure]` doc example using the SDK's typed `Table` API.

### 5.1 — high — Return-type validation is string-based, per-macro inconsistent, and rejects valid spellings *(dedup primary — also flagged by 1.3, 3.5, 6.5, 7.3)*
- File:line: `src/lib.rs:63-72` (`validator`: exact `"Validation"` only), `:193-202`
  (`function`: exact `"Result<Value>"` or `"core::result::Result<Value,Error>"` only) vs
  `:326-335`/`:495-504` which use `is_result_value_return` (`:411-420`); the normalizer
  strips `shamir_sdk::`, `crate::`, and `core::result::` prefixes but **not**
  `std::result::`, despite its doc claiming "any qualification form".
- Issue: each macro validates by `quote!(#ty).to_string().replace(' ', "")` against
  different literal sets. Consequences: `-> shamir_sdk::Validation` and
  `-> shamir_sdk::Result<Value>` — the spelling used in this repo's own tests
  (`shamir-sdk/tests/procedure_compile_pass.rs:11`, `scalar_compile_pass.rs:8`) — are
  spuriously rejected by the first two macros while passing under
  `#[procedure]`/`#[scalar]`; `std::result::Result<...>` is rejected by the helper;
  conversely all the checks accept a same-named foreign type (`my_crate::Validation`),
  which the generated coercion then rejects at type-check (a downstream compile error,
  never a runtime hole). All of this is inherent to matching stringified tokens instead of
  resolved types (aliases like `use other::Result` can false-accept). Validation is also
  asymmetric in the other direction: argument *types* are never checked at all (only
  arity, `lib.rs:56-60, 187-190`), and generics are half-supported (see 1.6).
- Failure scenario: a user writes `#[shamir_sdk::function] pub async fn f(_ctx: Ctx,
  _b: Batch, p: Params) -> shamir_sdk::Result<Value>` (consistent with how they wrote
  their `#[procedure]`) and gets a proc-macro panic "#[function] must return
  Result<Value>, got: shamir_sdk::Result<Value>" for a perfectly valid signature. The
  acceptance matrix differs per macro for no documented reason.
- Suggested fix: one shared, token-based checker for all four macros — match on
  `syn::Type::Path` final segments (`Validation`; or `Result` + `Value`), ignoring
  qualification entirely; extend the strip list to `std::result::` if `std` spellings
  should be accepted; route `validator` through a `is_validation_return` sibling; or drop
  the check and let the wrapper's hardcoded return type (`lib.rs:98, 228, 359, 526`)
  produce a natural type error, keeping the nice message as a spanned pre-check. Document
  the alias limitation. Reject generic signatures with an explicit error (closes 1.6).

### 5.2 — medium — *(dedup: same defect as 3.1)* — Wire protocol has no decode-failure channel: malformed input silently becomes empty Params / Null record
- (Full write-up at 3.1.) This lens's angle: the ABI's only input channel is msgpack bytes
  and every failure mode collapses to silence — a host/guest version mismatch sending a
  non-map envelope yields `Validation::record_error("empty_record")`-style *data
  validation failures* for what is really protocol incompatibility; also document the
  null-fallback rationale (behavior at `lib.rs:39, 128-132` is described but not
  justified).

### 5.3 — medium — *(dedup: same defect as 6.2)* — User errors travel as bare trap strings; no structured error envelope
- (Full write-up at 6.2.) This lens's angle: `Error::user("insufficient funds")` is
  wire-indistinguishable from a genuine crash; debuggability relies on parsing a
  `panic!`-formatted string ("shamir function error: {msg}") across the guest boundary;
  envelope design suggestion: tagged msgpack (`{"ok": ...}` / `{"err": {"kind": ...}}`) or
  a leading marker byte, with the host mapping the tagged branch to `FunctionError::User`.

### 5.4 — medium — *(dedup: same defect as 6.1)* — Diagnostics via `assert!`/`panic!` instead of spanned `compile_error!`
- (Full write-up at 6.1.) This lens's angle: multi-error signatures report only the first
  problem; accumulate errors where cheap.

### 5.5 — medium — *(dedup: same defect as 1.1)* — No compile coverage for `#[function]`/`#[validator]`; no compile-fail tests anywhere
- (Full write-up at 1.1.) This lens's angle: two of the four public macros have zero
  compile coverage in the entire workspace (the only references to `#[validator]`/
  `#[function]` outside the macros crate are doc comments); a refactor of the
  return-type check (e.g. fixing the `std::result::` gap) would silently break
  `#[function]` acceptance with a green suite.

### 5.6 — low — `#[procedure]` doc example does not compile
- File:line: `src/lib.rs:290-294`; correct counterpart at
  `crates/shamir-sdk/src/prelude.rs:35-36`.
- Issue: the example body is `let rows = ctx.db().table("users").query(None); Ok(rows)`,
  but `Table::query` returns `Result<Vec<Value>>` (`crates/shamir-sdk/src/db.rs:98` —
  verified): the `?` is missing and `Vec<Value>` is not `Value`.
- Failure scenario: a guest author copies the macro's own doc example and hits two type
  errors that look like SDK bugs.
- Suggested fix: change the body to
  `let rows = ctx.db().table("users").query(None)?; Ok(Value::List(rows));`.

### 5.7 — low — *(dedup: same defect as 1.4)* — "One macro per crate" unenforced; attribute payload silently ignored
- (Full write-up at 1.4.) This lens's angle: `#[function(anything)]`/`#[scalar(...)]` are
  accepted without comment, so typo'd options silently do nothing (the `_attr` half is
  carried in 1.10); a user applying two macros debugs a linker error instead of getting a
  compile error at the second attribute. Enforcement suggestion: error on non-empty
  payload; single-entrancy via a `const _: () = ...` collision sentinel.

### 5.8 — low — *(dedup: same defect as 3.2)* — `shamir_alloc` performs no length validation; negative `len` allocates ~2^63 bytes
- (Full write-up at 3.2.) This lens's angle: the ABI contract ("host wrote `len` bytes")
  is silently undefined for `len < 0` and the check costs one branch; `len = 0` returns a
  valid dangling-ish pointer whose contract is implicit.

### 5.9 — low — *(dedup: same defect as 7.1)* — Four macros in one `lib.rs` with the ABI emitter duplicated four times
- (Full write-up at 7.1.) This lens's angle: a calling-convention change (e.g. 6.2's
  error envelope) must be replicated in four places — the drift that 5.1 already exhibits
  across the validators can recur in the ABI itself; two guest kinds could end up speaking
  different dialects of the same wire protocol.

### 5.10 — nit — *(dedup: same defect as 1.9)* — `type_contains_ctx` purity check is lexical, not semantic
- (Full write-up at 1.9.) This lens's angle: actual purity is enforced structurally (the
  wrapper hardcodes the parameter to `shamir_sdk::Params` at `lib.rs:525` and no `Ctx` is
  ever constructed), so the check is only a lint — the doc's "**No argument type may
  contain `Ctx`**" phrasing implies more than it delivers.

---

## 6. error-handling-lifecycle

Lens verdict: the crate's entire error surface is compile-time validation of consumer
signatures, and every one of its ~15 validation branches reports failure with
`assert!`/`panic!` (only `parse_macro_input!` follows the graceful path), contradicting
CLAUDE.md's "avoid `panic!` outside programmer-bug invariants". In the generated guest
code, every user `Err` is flattened via `e.to_string()` into a panic-trap the host
classifies as `FunctionError::Compute`, collapsing the SDK's typed `thiserror` taxonomy at
the ABI boundary. Reviewed and sound: `parse_macro_input!` used correctly at all four
entry points; the intentional `shamir_alloc`/`leak_result` leaks are documented
bump-allocator design; wrapper bodies propagate `Result` faithfully with no
`unwrap()`/`expect()` in macro or generated code.

### 6.1 — medium — All signature validation panics via `assert!`/`panic!` instead of `syn::Error::to_compile_error()` *(dedup primary — also flagged by 5.4, 1.5)*
- File:line: `src/lib.rs:51-54, 57-60, 63-72, 81` (`validator`); `:183-190, 193-202, 211`
  (`function`); `:314-323, 326-335, 344` (`procedure`); `:471-481, 486-490, 495-504, 513`
  (`scalar`).
- Issue: consumer signature mistakes (non-async fn, wrong arity, wrong return type, `Ctx`
  in a scalar, receiver-style args) are macro-consumer *input* errors, not
  macro-programmer invariants, yet each is reported with `assert!`/`panic!` — CLAUDE.md's
  error-handling section is normative ("Avoid `panic!` outside `unreachable!()` /
  invariant violations that mean a programmer bug"). A rustc-caught proc-macro panic points
  at the macro invocation with a backtrace note and no span on the offending element, and
  it is inconsistent with the crate's own first line of defense (`parse_macro_input!` at
  `lib.rs:45/177/308/465`), which already emits proper spanned compile errors for parse
  failures. The bare `panic!("...must return Validation")` arms drop the actual type from
  the message.
- Failure scenario: an author writes `pub fn check(record: Value, old: Option<Value>,
  ctx: Ctx) -> Validation` (missing `async`) on a 30-line validator: rustc emits "proc
  macro panicked" + message + backtrace noise pointing at `#[shamir_sdk::validator]`,
  rather than a crisp error spanned on the fn signature (or a squiggle on the offending
  return type).
- Suggested fix: in each validation branch, `return syn::Error::new_spanned(&fn_item.sig.
  asyncness/ident/output/ty, "...").to_compile_error().into();` (short-circuiting
  expansion, as `parse_macro_input!` already does); accumulate errors where cheap; reserve
  `panic!`/`unreachable!()` for genuinely impossible arms. This refactor is also the test
  seam from 1.1 and composes with the 5.1 fix.

### 6.2 — medium — Generated `Err` path flattens typed user errors into a panic-trap the host misclassifies as `FunctionError::Compute` *(dedup primary — also flagged by 5.3, 3.4)*
- File:line: `src/lib.rs:271-273` (`function`), `:398-400` (`procedure`), `:563-565`
  (`scalar`) — `shamir_sdk::__rt::trap(&e.to_string())`; acknowledged TODO at `lib.rs:251`
  (`TODO(slice 4)`); root mechanism `crates/shamir-sdk/src/__rt.rs:64-69` (`trap` =
  `panic!("shamir function error: {msg}")`).
- Issue: a typed `thiserror` error is stringified and delivered through a panic. The host
  maps *any* trap to `FunctionError::Compute` (`__rt.rs:63`), so a user-level failure
  (e.g. `Error::MissingParam("n")`) is indistinguishable from a genuine guest compute
  crash. Error taxonomy is destroyed at exactly the boundary where the host needs it, the
  error path runs through `panic!` machinery (formatting + unwinding/abort) rather than a
  structured result channel, and the embedded author text may interpolate untrusted
  parameter content (see 3.4).
- Failure scenario: a `#[function]` returns `Err(...)` on bad params; the DB host records
  `FunctionError::Compute` for a routine user mistake — polluting compute-error
  metrics/alerts, triggering wrong retry behavior (a client sees "internal error" and
  retries/reports instead of surfacing a user error), and stripping the error kind from
  callers.
- Suggested fix: land the slice-4 TODO: emit a machine-readable envelope over the existing
  `(ptr, len)` ABI (tag byte or reserved export distinguishing `Ok`/`UserErr`/`ComputeErr`,
  message as msgpack/UTF-8) and have the host map the tagged branch to
  `FunctionError::User`; keep trap only for genuine guest crashes. Interim minimal step:
  prefix the trap message (`user-error: {e}`) so hosts can classify on string without
  breaking the ABI.

### 6.3 — medium — *(dedup: same defect as 1.1)* — Zero tests in-crate; no error-path coverage for any validation branch anywhere in the workspace
- (Full write-up at 1.1.) This lens's angle: the generated `shamir_call` `Ok`/`Err`
  runtime branches (the `Err => trap` path) have zero coverage; a refactor silently
  dropping the Ctx-purity assert in `#[scalar]` or changing a validation message ships
  with CI green.

### 6.4 — low — *(dedup: same defect as 3.1)* — `#[validator]` generated code silently swallows param-extraction errors — missing vs malformed payload indistinguishable
- (Full write-up at 3.1.) This lens's angle: a host-side msgpack encoding bug corrupting
  the payload makes *every* validator reject with per-record domain errors instead of one
  loud "malformed params" signal, sending debuggers down the wrong path; at minimum
  document the intentional `Null`-on-error conflation in the `#[validator]` doc comment.

### 6.5 — low — *(dedup: same defect as 5.1)* — Return-type validation inconsistent across the four macros; equivalent spellings spuriously rejected
- (Full write-up at 5.1.) This lens's angle: the author who writes the same signature
  style that works under `#[scalar]` under `#[function]` gets a spurious panic — styled as
  a proc-macro panic per 6.1.

### 6.6 — low — *(dedup: same defect as 3.2)* — Generated ABI functions trust `i32` inputs unvalidated: negative `len` yields alloc-abort or UB
- (Full write-up at 3.2.) This lens's angle: neither generated export validates its `i32`s
  before use; the `// Safety:` comments lean entirely on the host contract with no cheap
  runtime guard backing them.

### 6.7 — low — *(dedup: same defect as 2.1)* — Generated `block_on` has no deadline/fuel guard: a genuinely-Pending future spins hot forever on the failure path
- (Full write-up at 2.1.) This lens's angle: post-slice-4 the failure mode is a 100%-CPU
  infinite spin with no deadline, no trap message, and no resource cleanup, dying only via
  host fuel exhaustion with a generic trap; suggestion of a bounded poll/step budget
  ("timed out after N polls") or park-once-real-wakers-land; coordinate the fix with the
  `shamir-sdk` owner since the primitive is shared.

---

## 7. style-claude-md

Lens verdict: a single 572-line `src/lib.rs` carrying four public proc-macro exports plus
two private helpers conflicts with the "one file = one primary export" pillar; the shared
guest-ABI `quote!` blocks are duplicated verbatim four times and validation logic has
already drifted between macros. No `mod.rs` exists anywhere (re-export-only rule vacuously
met) and imports are fully compliant (all `use` in the file header, `lib.rs:8-10`;
generated code uses fully-qualified paths). Zero test code in the crate. Comment
discipline otherwise good: every `unsafe` in generated code carries a `// Safety:`
justification (`lib.rs:121, 254, 382, 548`); the one TODO carries a slice tag
(`lib.rs:251`); doc examples are ` ```ignore `-fenced, consistent with `doctest = false`.

### 7.1 — medium — Single `lib.rs` holds four public exports + two helpers; the ABI emitter is duplicated four times *(dedup primary — also flagged by 5.9)*
- File:line: `crates/shamir-sdk-macros/src/lib.rs:43-571` — `validator` (:43-44),
  `function` (:176), `procedure` (:307), `scalar` (:464), plus private helpers
  `is_result_value_return` (:411) and `type_contains_ctx` (:425); the guest-ABI
  `shamir_alloc`/`shamir_call` `quote!` blocks are copy-pasted verbatim at
  `:105-149, 235-275, 366-402, 533-567` (~150 duplicated lines).
- Issue: CLAUDE.md's "one file = one primary export" rule (`CLAUDE.md:505-509`) is
  strained past the "closely-coupled group" exemption — the file's size and the
  already-realized divergence (7.3) show it has outgrown it. The packed
  `(ptr << 32) | len` return convention lives only in `__rt::leak_result` plus four
  doc/comment copies, so any calling-convention change must be replicated in four places
  (5.9's dialect-drift scenario). Runtime cost: none — the price is maintainability
  (lockstep edits, coarse `git blame`, re-copied ABI per new macro family).
- Failure scenario: a future slice changes the result encoding in one macro's emitter but
  not another's; and every ABI-affecting fix in this document (3.2's guards, 6.2's
  envelope, 2.1's lowering) pays a 4x edit tax until the duplication is factored out.
- Suggested fix: split per the documented layout in a dedicated `refactor:` commit
  (CLAUDE.md bans riding-along refactors): `lib.rs` as a re-export-only manifest over
  siblings `validator.rs`/`function.rs`/`procedure.rs`/`scalar.rs`, plus one shared
  `abi.rs` with `fn emit_guest_abi(kind: Kind, ...) -> TokenStream` used by all four.

### 7.2 — medium — *(dedup: same defect as 1.1)* — Zero tests in the crate; TDD protocol and `tests/` layout unfulfilled
- (Full write-up at 1.1.) This lens's angle: the proc-macro entry fns themselves are not
  unit-testable in-process, but the two helpers are plain `fn(&Type) -> bool` testable via
  `syn::parse_str` (syn's default `parsing` feature is enabled) — a tweak to
  `type_contains_ctx`'s segment splitting or `is_result_value_return`'s normalisation
  chain silently breaks the `#[scalar]` purity guarantee or return-type validation with
  CI green.

### 7.3 — low — *(dedup: same defect as 5.1)* — Divergent duplicated return-type validation: `#[function]` bypasses the shared helper
- (Full write-up at 5.1.) This lens's angle: same concept, two implementations in one
  file — precisely the duplication the one-file-one-export rule exists to prevent; future
  edits to one copy (e.g. widening normalisation) leave the other stale, as already
  happened.

### 7.4 — nit — Inline comment under-documents the normalisation chain
- File:line: `src/lib.rs:413-418`.
- Issue: the comment says "Strip any `shamir_sdk::` or `crate::` prefixes", but the code
  also strips `core::result::` (`:418`). The doc comment above (`:408-410`) does mention
  `core::result::...`, so the inline comment and code disagree on scope.
- Failure scenario: none — a reader of the inline comment alone may misjudge which
  qualifications are normalised.
- Suggested fix: extend the inline comment to list all three stripped prefixes (and the
  missing `std::result::` gap is 5.1's fix, not this comment's).

---

## Finding counts

| Severity | Lens-tagged findings | Distinct defects after dedup | Distinct defect IDs |
|---|---|---|---|
| critical | 0 | 0 | — |
| high | 2 | 2 | 1.1 (zero test coverage — one defect, six lenses: +2.2, 3.3, 5.5, 6.3, 7.2) · 5.1 (return-type string checks — one defect, five lenses: +1.3, 3.5, 6.5, 7.3) |
| medium | 14 | 7 | 1.2 · 2.1 (spin-on-`Pending` — one defect, four lenses: +1.8, 4.3, 6.7) · 3.1 (silent param coercion — one defect, three lenses: +5.2, 6.4) · 4.1 · 6.1 (panic diagnostics — one defect, three lenses: +5.4, 1.5) · 6.2 (trap-string user errors — one defect, three lenses: +5.3, 3.4) · 7.1 (one-file/duplicated ABI — one defect, two lenses: +5.9) |
| low | 20 | 5 | 1.4 (one-per-crate unenforced — one defect, three lenses: +3.6, 5.7) · 1.6 · 3.2 (unvalidated `i32` ABI lengths — one defect, five lenses: +1.7, 4.4, 5.8, 6.6) · 4.2 · 5.6 |
| nit | 7 | 3 | 1.9 (lexical `Ctx` check — one defect, two lenses: +5.10) · 1.10 · 7.4 |
| **total** | **43** | **17** | |

Deduplicated defect census: **0 critical, 2 high, 7 medium, 5 low, 3 nit = 17 distinct
defects** (43 lens-tagged findings).

Counting notes (workspace-SUMMARY convention): every severity-tagged finding in the seven
source reports is counted once in the "lens-tagged" column, *as filed* — 0/2/14/20/7 = 43,
matching this crate's row in the workspace Per-crate breakdown. Where several lenses filed
the same root-cause defect, the group is listed once under its primary lens (the fullest
write-up, at that lens's severity) in the "Distinct defects" column, with the other
members noted; those members' own severities are folded into the group's primary severity
for the deduped census (e.g. the zero-coverage group: 1 high + 3 medium + 2 low as filed →
one high).

## Fix Plan

**P0 — before anything else ships from this crate**
1. **Build the test foundation first (Red-first per CLAUDE.md TDD).** Add `src/tests/`
   unit tests for `is_result_value_return`/`type_contains_ctx` (via `syn::parse_str`);
   `validator_compile_pass.rs`/`function_compile_pass.rs` beside the existing sibling
   compile-pass tests; a trybuild UI test per `assert!` diagnostic; one round-trip ABI
   test driving `shamir_call` through `Ok`, `Err`, malformed msgpack, and negative `len`
   (the malformed-msgpack case written Red — it fails until item 3 lands). Closes
   **1.1** (with 2.2, 3.3, 5.5, 6.3, 7.2) and makes every other fix verifiable.
2. **Unify return-type validation into one shared structural checker.** Match `syn::Type`
   final path segments (ignoring qualification), covering `Validation` and
   `Result<Value>` for all four macros; strip/accept `std::result::`; reject generics
   with an explicit error. Closes **5.1** (with 1.3, 3.5, 6.5, 7.3) and **1.6**.
3. **Make param-decode failures loud.** `__rt::decode_params` returns `Result` and the
   generated `shamir_call` traps on malformed/non-map payloads (or traps when `len > 0`
   decodes to an empty map); `old_record`'s `Err(_) => None` must distinguish absent from
   undecodable; document the genuine-absent → `Null`/`None` semantic in the macro doc.
   Closes **3.1** (with 5.2, 6.4) — coordinate with the `shamir-sdk` owner (root fix is
   the shared `decode_params`).

**P1 — soon**
4. **Refactor all validation to `syn::Result` + `to_compile_error()`.** Spanned errors on
   the offending element; also the snapshot-test seam from item 1. Closes **6.1**
   (with 5.4, 1.5).
5. **Land the slice-4 error envelope (`lib.rs:251` TODO).** Tagged `Ok`/`UserErr`/
   `ComputeErr` over the `(ptr, len)` ABI (or interim `user-error:` trap prefix); host
   maps the user branch to `FunctionError::User`. Closes **6.2** (with 5.3, 3.4) —
   coordinate cross-crate.
6. **Guard the `i32` ABI boundary.** One-line `len < 0` rejection in `shamir_alloc` and
   `shamir_call` before `from_raw_parts`; define the `len == 0` case. Closes **3.2**
   (with 1.7, 4.4, 5.8, 6.6).
7. **Disclose and bound the spin-on-`Pending` lowering.** State the Ready-first contract
   in all four macro docs now; add a bounded-poll trap ("future never resolved after N
   polls") coordinated with `shamir-sdk::__rt`; replace the spin when slice-4 async host
   imports land. Closes **2.1** (with 1.8, 4.3, 6.7).
8. **Fix pattern re-use in the expansion.** Accept only `Pat::Ident`; map `_`/`mut x`/
   `ref x` to fresh generated idents (`format_ident!("__shamir_arg{i}")`) or emit a
   spanned error. Closes **1.2**.
9. **Cut the validator's 3x payload retention.** `drop(params)` after extraction and a
   `Params::take` move-out API in `shamir-sdk` instead of deep clones. Closes **4.1**.

**P2 — backlog**
10. **Enforce single-entrancy + assert empty `_attr`.** Fixed-name sentinel
    (`const SHAMIR_SDK_ENTRYPOINT_TAKEN: () = ();`) per expansion so a second macro in any
    module fails at compile time with a pointing message; error on non-empty attribute
    payload; consider `proc-macro-crate` for path independence. Closes **1.4**
    (with 3.6, 5.7) and **1.10**.
11. **Split `lib.rs` per the CLAUDE.md layout in a dedicated `refactor:` commit.**
    Per-macro files + one shared `abi.rs` emitter, eliminating the 4x duplicated
    `quote!` blocks. Closes **7.1** (with 5.9).
12. **Fix the `#[procedure]` doc example** (`query(None)?; Ok(Value::List(rows))`).
    Closes **5.6**.
13. **Doc hygiene:** soften the `#[scalar]` purity wording to "no `Ctx`-typed parameter"
    and note the alias limitation (**1.9**, with 5.10); extend the normalisation comment
    to list all three stripped prefixes (**7.4**); carry the "fresh instance per call is
    load-bearing" contract into `shamir-wasm-host`'s call-path docs, and either emit a
    documented `shamir_free` or a debug high-water assertion (**4.2**).
14. **Optional perf polish:** if large payloads justify it, allocate `shamir_alloc`
    buffers uninitialized (host overwrites the full buffer) — the remaining sub-point of
    **4.4**; otherwise accept the memset.

</details>
