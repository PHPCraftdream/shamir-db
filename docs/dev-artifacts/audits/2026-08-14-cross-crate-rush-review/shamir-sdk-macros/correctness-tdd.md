<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-sdk-macros — correctness-tdd revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Most implementation defects remain open. Crate-local coverage is absent, but workspace-wide zero-coverage claims are false. Generic diagnostic and purity explanations require correction.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 10 | 9 | 0 | 0 | 0 | 1 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — Zero test coverage -- TDD protocol not honored; even pure helpers are untested

Status: `confirmed-open`. Current risk: `medium`.

No crate-local tests, helper tests, or signature UI tests exist. However, the registered host compile_and_invoke_double test compiles #[function] and invokes its generated ABI; historical TDD ordering cannot be inferred from absent local tests.

Evidence: [crates/shamir-sdk-macros/Cargo.toml:13](../../../../../crates/shamir-sdk-macros/Cargo.toml#L13); [crates/shamir-sdk-macros/src/lib.rs:411](../../../../../crates/shamir-sdk-macros/src/lib.rs#L411); [crates/shamir-wasm-host/src/tests/compile_tests.rs:19](../../../../../crates/shamir-wasm-host/src/tests/compile_tests.rs#L19); [crates/shamir-wasm-host/src/tests/mod.rs:2](../../../../../crates/shamir-wasm-host/src/tests/mod.rs#L2); [crates/shamir-wasm-host/src/lib.rs:57](../../../../../crates/shamir-wasm-host/src/lib.rs#L57).

<a id="review-2"></a>

### Claim 2 — User parameter patterns are re-used as call-position expressions (`mut x`, `_`, `ref x` break the expansion)

Status: `confirmed-open`. Current risk: `medium`.

Each emitter still clones the complete parameter pattern and interpolates it into both a wrapper parameter and a call argument. There is no normalization or rejection of non-expression patterns.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:80](../../../../../crates/shamir-sdk-macros/src/lib.rs#L80); [crates/shamir-sdk-macros/src/lib.rs:102](../../../../../crates/shamir-sdk-macros/src/lib.rs#L102); [crates/shamir-sdk-macros/src/lib.rs:232](../../../../../crates/shamir-sdk-macros/src/lib.rs#L232); [crates/shamir-sdk-macros/src/lib.rs:363](../../../../../crates/shamir-sdk-macros/src/lib.rs#L363); [crates/shamir-sdk-macros/src/lib.rs:530](../../../../../crates/shamir-sdk-macros/src/lib.rs#L530).

<a id="review-3"></a>

### Claim 3 — String-based return-type validation: inconsistent across sibling macros, false rejects and false accepts

Status: `confirmed-open`. Current risk: `medium`.

Validator/function retain exact string checks; procedure/scalar use a different normalizer that still omits std::result::. All four retain concrete SDK wrapper returns, so foreign-type false accepts remain compile-time issues.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:66](../../../../../crates/shamir-sdk-macros/src/lib.rs#L66); [crates/shamir-sdk-macros/src/lib.rs:197](../../../../../crates/shamir-sdk-macros/src/lib.rs#L197); [crates/shamir-sdk-macros/src/lib.rs:411](../../../../../crates/shamir-sdk-macros/src/lib.rs#L411).

Grouping/duplicate: `api-wire-protocol.md#1`. This row is not another independent defect.

<a id="review-4"></a>

### Claim 4 — "Only one `#[...]` per crate" contract is documented but not enforced

Status: `confirmed-open`. Current risk: `low`.

No targeted macro diagnostic exists for multiple entrypoints. Fixed no_mangle exports already enforce uniqueness through compilation/linking failure; this is diagnostic debt, not an unenforced runtime safety invariant.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:15](../../../../../crates/shamir-sdk-macros/src/lib.rs#L15); [crates/shamir-sdk-macros/src/lib.rs:108](../../../../../crates/shamir-sdk-macros/src/lib.rs#L108); [crates/shamir-sdk-macros/src/lib.rs:119](../../../../../crates/shamir-sdk-macros/src/lib.rs#L119).

<a id="review-5"></a>

### Claim 5 — `assert!`/`panic!` diagnostics instead of `syn::Error` compile errors

Status: `confirmed-open`. Current risk: `low`.

Semantic signature checks still panic; only initial parse_macro_input parsing follows the compile-error path.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:45](../../../../../crates/shamir-sdk-macros/src/lib.rs#L45); [crates/shamir-sdk-macros/src/lib.rs:51](../../../../../crates/shamir-sdk-macros/src/lib.rs#L51); [crates/shamir-sdk-macros/src/lib.rs:71](../../../../../crates/shamir-sdk-macros/src/lib.rs#L71); [crates/shamir-sdk-macros/src/lib.rs:486](../../../../../crates/shamir-sdk-macros/src/lib.rs#L486).

Grouping/duplicate: `error-handling-lifecycle.md#1`. This row is not another independent defect.

<a id="review-6"></a>

### Claim 6 — Generic functions hit E0207 inside the expansion instead of a clear rejection

Status: `unverified`. Current risk: `low` (provisional; not a confirmed defect).

No generic rejection exists, and generic wrappers are called without explicit type arguments, creating inference problems for the proposed type-generic example. The asserted E0207 diagnosis is unsupported: emitted items are functions, unused function generics are not themselves E0207 violations, and not every generic signature necessarily fails.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:89](../../../../../crates/shamir-sdk-macros/src/lib.rs#L89); [crates/shamir-sdk-macros/src/lib.rs:219](../../../../../crates/shamir-sdk-macros/src/lib.rs#L219); [crates/shamir-sdk-macros/src/lib.rs:232](../../../../../crates/shamir-sdk-macros/src/lib.rs#L232); [crates/shamir-sdk-macros/src/lib.rs:264](../../../../../crates/shamir-sdk-macros/src/lib.rs#L264).

<a id="review-7"></a>

### Claim 7 — `shamir_alloc` does not guard negative or zero `len`

Status: `confirmed-open`. Current risk: `low`.

Unchecked signed-to-usize allocation remains. Normal host calls use checked nonnegative lengths. Zero-length allocation is not inherently defective; its returned non-null dangling pointer is suitable only for zero-length access.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:109](../../../../../crates/shamir-sdk-macros/src/lib.rs#L109); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:518](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L518).

Grouping/duplicate: `security-crypto.md#2`. This row is not another independent defect.

<a id="review-8"></a>

### Claim 8 — Macros accept any `async` body, but the emitted `__rt::block_on` busy-spins forever on `Pending`

Status: `confirmed-open`. Current risk: `medium`.

The no-op-waker polling loop remains unbounded for indefinitely-Pending guest futures. A single Pending followed by Ready does complete. Production execution has fuel/epoch limits, and async host imports suspend Wasmtime's fiber rather than necessarily yielding the guest Rust future.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:144](../../../../../crates/shamir-sdk-macros/src/lib.rs#L144); [crates/shamir-sdk/src/__rt.rs:50](../../../../../crates/shamir-sdk/src/__rt.rs#L50); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:195](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L195); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:477](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L477).

Grouping/duplicate: `concurrency-lockfree.md#1`. This row is not another independent defect.

<a id="review-9"></a>

### Claim 9 — `type_contains_ctx` is a name heuristic and the `#[scalar]` purity claim overreaches

Status: `confirmed-open`. Current risk: `low`.

The lexical check remains advisory. No Ctx parameter is passed, but a scalar body can construct public Ctx::new() and invoke its host-backed methods. Ctx construction is not an inert capability boundary; actual access depends on host gateways and policy.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:425](../../../../../crates/shamir-sdk-macros/src/lib.rs#L425); [crates/shamir-sdk-macros/src/lib.rs:454](../../../../../crates/shamir-sdk-macros/src/lib.rs#L454); [crates/shamir-sdk/src/context.rs:64](../../../../../crates/shamir-sdk/src/context.rs#L64); [crates/shamir-sdk/src/context.rs:86](../../../../../crates/shamir-sdk/src/context.rs#L86); [crates/shamir-sdk/src/context.rs:99](../../../../../crates/shamir-sdk/src/context.rs#L99); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:178](../../../../../crates/shamir-wasm-host/src/wasm/wasm_function.rs#L178).

<a id="review-10"></a>

### Claim 10 — Generated code hardcodes `shamir_sdk::` paths; `_attr` tokens silently ignored

Status: `confirmed-open`. Current risk: `nit`.

All four entrypoints still discard attributes and emit literal SDK paths. Cargo dependency renaming without a compatible alias remains unsupported; the built-in compiler supplies the canonical dependency name.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:44](../../../../../crates/shamir-sdk-macros/src/lib.rs#L44); [crates/shamir-sdk-macros/src/lib.rs:176](../../../../../crates/shamir-sdk-macros/src/lib.rs#L176); [crates/shamir-sdk-macros/src/lib.rs:307](../../../../../crates/shamir-sdk-macros/src/lib.rs#L307); [crates/shamir-sdk-macros/src/lib.rs:464](../../../../../crates/shamir-sdk-macros/src/lib.rs#L464); [crates/shamir-wasm-host/src/compile.rs:511](../../../../../crates/shamir-wasm-host/src/compile.rs#L511).

## Corrections and qualified non-findings

- Replace workspace-wide zero-coverage assertions with absent crate-local/helper/UI/validator coverage. The wired function runtime test already existed before the review: crates/shamir-wasm-host/src/tests/compile_tests.rs:19; historical 07b511b2 contains it.
- Examples contain real validator/function invocations, not only documentation: examples/fn-validator/src/lib.rs:15 and examples/wasm-baseline/src/lib.rs:3. They are not proof of registered workspace tests.
- Do not prescribe returning zero from shamir_alloc(0) followed by constructing a slice from that pointer: from_raw_parts requires non-null even for zero length. Preserve a valid zero-length representation.
- A same-named module-local const cannot detect entrypoints in different modules.
- The generic inference defect needs a corrected diagnostic example; no reproduction was run.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-sdk-macros -- Correctness & TDD-coverage

## Summary
`shamir-sdk-macros` is a 572-line proc-macro crate (four attribute macros + two pure helpers) with **zero tests of any kind** -- no `tests/` directory, no `#[cfg(test)]`, no trybuild UI tests -- so CLAUDE.md's normative Red/Green/Refactor protocol was skipped entirely for this crate. Even the host-testable pure helpers (`is_result_value_return`, `type_contains_ctx`, all arity/return validation) are untested, and the pre-commit gate (`./scripts/test.sh`) therefore runs zero tests for this crate. Static review found several latent expansion bugs (parameter patterns re-used as expressions, string-based return-type checks inconsistent across sibling macros) that a single expansion-level test would have caught.

## Findings

### 1. Zero test coverage -- TDD protocol not honored; even pure helpers are untested
- **File:** crate-wide (`src/lib.rs`, `Cargo.toml`; no `src/**/tests/` exists)
- **Severity:** high
- **Issue:** CLAUDE.md "🛡️ Protocol of development (TDD)" (lines 487-493) mandates Red (failing test first) / Green / Refactor for every feature, and "📁 Test organisation" (lines 571-611) mandates a `tests/` directory per module. This crate has neither -- no `#[cfg(test)]` mod, no integration tests, no trybuild/compile-fail UI tests for the macro's rich error paths (asyncness, arity, return type, `Ctx` purity). The two helpers are trivially unit-testable today with no refactor: they take `&syn::Type`, constructible via `syn::parse_str::<syn::Type>("Result<Value>")`. Likewise the validation logic could be hosted behind a `fn core(input: TokenStream2) -> syn::Result<TokenStream2>` seam. The generated-code semantics (missing `record` → `Value::Null`, `old_record` null/absent → `None`, `(ptr << 32) | len` packing contract with the host) are behavioral invariants of the ABI that live in this crate's expansion and are pinned by no test anywhere here.
- **Failure scenario:** any refactor of the macro (e.g. fixing findings 2/3 below) can silently change generated code or error messages; nothing fails. The doc examples are ` ```ignore ` blocks and `doctest = false`, so even the documented usage compiles nowhere.
- **Suggested fix:** add `src/tests/` per the CLAUDE.md layout: unit tests for `is_result_value_return` / `type_contains_ctx` over the qualification matrix (`Result<Value>`, `core::result::Result<Value,Error>`, `std::result::Result<...>`, `shamir_sdk::Result<shamir_sdk::Value>`, aliases), refactor each macro's body into a `TokenStream2 -> syn::Result<TokenStream2>` core so expansion shape is snapshot-testable, and add trybuild UI tests for each `assert!` diagnostic.

### 2. User parameter patterns are re-used as call-position expressions (`mut x`, `_`, `ref x` break the expansion)
- **File:** `src/lib.rs:74-87` + `102` (validator), `204-217` + `232` (function), `337-349` + `363` (procedure), `506-517` + `530` (scalar)
- **Severity:** medium
- **Issue:** the macros extract `PatType.pat` and splice the same pattern into two positions: (a) the re-typed inner signature `#arg0: shamir_sdk::Value` and (b) the forwarded call `#fn_name(#arg0, #arg1, #arg2).await`. Position (a) tolerates any pattern; position (b) requires a plain ident. syn's `ToTokens for PatIdent` emits the `mut`/`ref` tokens, and `Pat::Wild` renders as `_`, which is not an expression.
- **Failure scenario:** a perfectly legal, idiomatic signature `#[validator] pub async fn check(mut record: Value, old: Option<Value>, ctx: Ctx) -> Validation` expands to `__shamir_impl_check(mut record, ...)` → `error: expected expression, found keyword `mut`` deep inside the expansion. Same for `fn f(_: Params)` (wildcard) and `ref record`. The user gets no hint their signature is "fine but unlucky".
- **Suggested fix:** when extracting args, accept only `Pat::Ident` without `subpat`; map `_` (and non-ident patterns) to fresh generated idents (`format_ident!("__shamir_arg{i}")`) used in both positions, or emit a clear `syn::Error::new_spanned(pat, "...")`.

### 3. String-based return-type validation: inconsistent across sibling macros, false rejects and false accepts
- **File:** `src/lib.rs:63-72` (validator), `193-202` (function), `408-420` (`is_result_value_return`)
- **Severity:** medium
- **Issue:** `#[validator]` requires the token string to equal exactly `"Validation"` -- it rejects legal spellings `shamir_sdk::Validation` / `crate::Validation`, and false-accepts any local type coincidentally named `Validation` (which then fails later inside the expansion at `into_value()`). `#[function]` uses its own ad-hoc list `"Result<Value>" || "core::result::Result<Value,Error>"`, rejecting `shamir_sdk::Result<Value>` and `std::result::Result<Value, Error>` -- while `#[procedure]` and `#[scalar]` use the normalizing `is_result_value_return`, which accepts both. So the *same* return spelling is valid on two macros and rejected on the third. `is_result_value_return` itself strips `core::result::` but not `std::result::`. All of this is inherent to matching stringified tokens instead of resolved types (aliases like `use other::Result` can false-accept).
- **Failure scenario:** user writes `-> shamir_sdk::Validation` or `-> std::result::Result<Value, Error>` (both semantically correct) and gets "must return Validation/Result<Value>" despite the docs' examples compiling only in the bare-prelude spelling; confusion is maximal because the acceptance matrix differs per macro for no documented reason.
- **Suggested fix:** route validator and function through a single normalized checker (extend `is_result_value_return` to also strip `std::result::`, add a `is_validation_return` sibling that strips `shamir_sdk::`/`crate::` prefixes); document the alias limitation.

### 4. "Only one `#[...]` per crate" contract is documented but not enforced
- **File:** `src/lib.rs:15, 157, 283, 437` (doc claims); no enforcement anywhere
- **Severity:** low
- **Issue:** all four macro docs state "**Only one per crate is supported** (single entrypoint)" because each application emits `#[no_mangle] shamir_alloc`/`shamir_call`. Nothing in the macro detects a second application. Two macros in the same module give E0428 (tolerably clear); two in *different* modules compile and fail only at link time with an opaque duplicate-symbol error naming `shamir_call`, with no pointer to the entrypoint contract.
- **Failure scenario:** user adds `#[function]` beside an existing `#[procedure]` in another module; wasm link fails with `multiple definition of 'shamir_call'` and no diagnosis.
- **Suggested fix:** emit a fixed-name sentinel item (e.g. `const SHAMIR_SDK_ENTRYPOINT_TAKEN: () = ();`) alongside the exports so a second application across any module produces a duplicate-definition error naming the sentinel, or document how to resolve the link error.

### 5. `assert!`/`panic!` diagnostics instead of `syn::Error` compile errors
- **File:** `src/lib.rs:51-60, 66-72, 183-190, 196-202, 314-323, 329-335, 471-481, 486-492, 498-504`
- **Severity:** low
- **Issue:** every validation failure panics. CLAUDE.md's error-handling rules say avoid `panic!` outside genuine programmer-bug invariants; for proc-macros the idiomatic form is `syn::Error::new_spanned(...).to_compile_error()`, which points the rustc error at the *offending* item (the bad return type, the surplus argument) instead of rendering the whole invocation as "proc macro panicked". The bare `panic!("...must return Validation")` arms (`71, 201, 334, 503`) also drop the actual type from the message.
- **Failure scenario:** a user with a 30-line validator gets `error: proc macro panicked` + "help: message: #[validator] must return Validation, got: ..." with span = whole function, rather than a squiggle on `-> Validation`.
- **Suggested fix:** refactor each macro body to return `syn::Result<TokenStream2>` and convert with `into_compile_error()` (this is also the test seam from finding 1).

### 6. Generic functions hit E0207 inside the expansion instead of a clear rejection
- **File:** `src/lib.rs:89, 219, 351, 519` (`split_for_impl` usage)
- **Severity:** low
- **Issue:** the macros copy the user's generics onto the inner fn, but the inner signature is re-typed to concrete SDK types. If a generic parameter appeared only in the user's own parameter types, it becomes unused on the inner fn → `error[E0207]: the type parameter `T` is not defined... parameter `T` is never used`, deep in generated code. Generics are meaningless for a fixed WASM ABI and should be rejected outright.
- **Failure scenario:** `#[function] async fn f<T: Display>(ctx: Ctx, b: Batch, p: Params) -> Result<Value>` compiles the check pass (arity 3, return matches) then fails with E0207 pointing into macro output.
- **Suggested fix:** early `assert!`/`syn::Error` on `!fn_item.sig.generics.params.is_empty()` with "generic entrypoints are not supported".

### 7. `shamir_alloc` does not guard negative or zero `len`
- **File:** `src/lib.rs:109-114, 239-244, 370-375, 537-542`
- **Severity:** low
- **Issue:** `len: i32` is cast with `len as usize` unchecked: a negative `len` becomes a huge allocation → guest OOM abort instead of a clean trap; `len == 0` returns the dangling align-1 pointer of an empty `Vec` (mostly harmless but a footgun for host code that does not special-case `len == 0`). The host is trusted, so this is defense-in-depth, not an exploit path.
- **Failure scenario:** host bug passing `-1` → `vec![0u8; usize::MAX]` → allocation failure abort in the guest, indistinguishable from a guest OOM.
- **Suggested fix:** `if len <= 0 { return len; }` (or return `0`/trap) before allocating.

### 8. Macros accept any `async` body, but the emitted `__rt::block_on` busy-spins forever on `Pending`
- **File:** `src/lib.rs:144, 264, 391, 556` (emitted `block_on` calls); root cause shared with `crates/shamir-sdk/src/__rt.rs:36-61`
- **Severity:** low
- **Issue:** the generated entry drives the user's future with a no-op-waker loop that `spin_loop()`s on `Poll::Pending`. The macros impose no requirement (nor doc note on `#[validator]`/`#[procedure]`/`#[scalar]`) that the future be `Ready` on first poll. DB/host-import access happens to be synchronous (`pub fn query`, sync `extern "C"` imports) so the advertised flows work, but any user who awaits something that pends (e.g. `tokio::time::sleep`, an `AsyncRead`) gets a compiled-clean guest that hangs in a 100% CPU spin until the host times it out as a trap.
- **Failure scenario:** validator does `.await` on a real async I/O future → guest never traps, burns the CPU budget, surfaces as a host-side timeout with no guest-side diagnosis.
- **Suggested fix:** document the "first-poll-Ready" constraint on all four macros; optionally have `shamir-sdk` cap the spin iterations and trap with "futures that yield Pending are not supported in this slice".

### 9. `type_contains_ctx` is a name heuristic and the `#[scalar]` purity claim overreaches
- **File:** `src/lib.rs:422-432` (helper), `434-462` (docs)
- **Severity:** nit
- **Issue:** purity is enforced only by token-string matching on the single parameter: an alias (`type C = shamir_sdk::Ctx`) bypasses the check (failing later with an unrelated expansion error), while the docs promise the scalar "cannot access the database ... or perform HTTP requests" -- which the macro cannot enforce at all, since the body could call `Ctx::new()` directly (it is `pub`). The actual guarantee is "no `Ctx` parameter + `Ctx::new()` is inert", an SDK property, not a macro property.
- **Suggested fix:** soften the doc wording to what is enforced ("no `Ctx`-typed parameter"), note the alias limitation.

### 10. Generated code hardcodes `shamir_sdk::` paths; `_attr` tokens silently ignored
- **File:** `src/lib.rs:44, 95-98, 126, 176, 307, 464` (and every `quote!` block); `_attr` unused at `44, 176, 307, 464`
- **Severity:** nit
- **Issue:** all emitted items reference `shamir_sdk::*` by literal path, so a dependency rename (`package = "shamir-sdk"` under another key) breaks every expansion. `_attr` is discarded without checking it is empty, so `#[validator(anything)]` is silently accepted. Both are standard proc-macro trade-offs in a single-consumer workspace.
- **Suggested fix:** at minimum, assert `_attr` is empty with a clear message; consider `proc-macro-crate` only if renamed consumption becomes real.

</details>
