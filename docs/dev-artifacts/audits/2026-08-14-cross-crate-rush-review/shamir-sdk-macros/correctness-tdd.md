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
