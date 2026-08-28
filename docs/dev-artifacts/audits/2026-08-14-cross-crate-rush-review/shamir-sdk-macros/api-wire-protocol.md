# shamir-sdk-macros -- API & wire-protocol design

## Summary

The crate exposes four attribute macros (`validator`, `function`, `procedure`, `scalar`) that emit the guest WASM ABI (`shamir_alloc` + `shamir_call`) against `shamir_sdk::__rt`. The public-interface weak spot is signature validation: four macros use three different, string-matching return-type checks with inconsistent qualification acceptance, so identical idiomatic spellings compile under one macro and panic under another; diagnostics are `assert!`/`panic!` rather than spanned compile errors. On the wire side, the protocol has no error channel at all -- malformed payloads silently degrade to empty `Params`/`Null` `record`, and deliberate user errors surface as bare trap strings (an acknowledged slice-4 TODO). Builder-only query-construction rule: compliant -- the crate constructs no queries, batches, or filters and contains no `serde_json` usage; its only query-shaped text is the `#[procedure]` doc example using the SDK's typed `Table` API.

## Findings

### 1. Return-type validation is string-based, per-macro inconsistent, and rejects valid spellings
File: `crates/shamir-sdk-macros/src/lib.rs:63-72` (`validator`), `:193-202` (`function`), `:411-420` (`is_result_value_return`)
Severity: high
Issue: Each macro validates the return type by `quote!(#ty).to_string().replace(' ', "")` against different literal sets: `#[validator]` accepts only the bare token `Validation`; `#[function]` accepts only `Result<Value>` or `core::result::Result<Value,Error>`; `#[procedure]`/`#[scalar]` use `is_result_value_return`, which strips `shamir_sdk::`, `crate::`, and `core::result::` prefixes but not `std::result::`, despite its doc claiming "any qualification form". Meanwhile the sibling compile-pass tests (`crates/shamir-sdk/tests/procedure_compile_pass.rs:11`, `scalar_compile_pass.rs:8`) use exactly `shamir_sdk::Result<shamir_sdk::Value>` -- the spelling `#[function]` rejects. Validation is also asymmetric in the other direction: argument *types* are never checked at all (only arity, `lib.rs:56-60`, `:187-190`), and generics are half-supported (`split_for_impl` forwards impl generics at `lib.rs:89`/`:219`/`:351`/`:519` but the concrete `shamir_call` body can never satisfy them).
Failure scenario: A user writes `#[shamir_sdk::function] pub async fn f(_ctx: Ctx, _b: Batch, p: Params) -> shamir_sdk::Result<Value> { ... }` (consistent with how they wrote their `#[procedure]`) and gets a proc-macro panic "#[function] must return Result<Value>, got: shamir_sdk::Result<Value>" for a perfectly valid signature. Conversely `#[validator]` with `-> shamir_sdk::Validation` is rejected; a same-named user-local alias `Validation` would be wrongly accepted and fail later with a confusing error inside generated code.
Suggested fix: One shared, token-based checker for all four macros: match on `syn::Type::Path` (final segment `Validation`; or final segments `Result` + `Value`), ignoring qualification entirely -- or drop the check and let the wrapper's hardcoded return type (`lib.rs:98`, `:228`, `:359`, `:526`) produce a natural type error, keeping the nice message as a spanned pre-check. Reject generic signatures with an explicit error instead of half-forwarding `impl_generics`.

### 2. Wire protocol has no decode-failure channel: malformed input silently becomes empty Params / Null record
File: `crates/shamir-sdk-macros/src/lib.rs:128-132` (`record` -> `Null` fallback); compounding `crates/shamir-sdk/src/__rt.rs:11-16` (`decode_params` -> `Params::new()` on any failure)
Severity: medium
Issue: The ABI's only input channel is msgpack bytes, but every failure mode collapses to silence: `decode_params` returns an empty `Params` when the bytes fail to decode or are not a map, and the emitted `#[validator]` entrypoint maps a missing/failed `record` lookup to `Value::Null`. There is no version field, no handshake, and no way for the guest to say "the payload itself was invalid".
Failure scenario: A host/guest version mismatch sends a non-map envelope (or a future format change alters the wire shape); the guest decodes nothing, the validator sees `record = Null`, and returns `Validation::record_error("empty_record")`-style results. The host records a *data validation failure* when the actual cause is a protocol incompatibility -- exactly the class of silent degradation the project's "checksums everywhere / reliability" goal targets.
Suggested fix: Make `decode_params` trap (or return a distinguished error envelope) on undecodable/non-map input instead of yielding empty `Params`; in the emitted `shamir_call`, only treat a genuinely-absent `record` key as `Null` if that is the intended semantic -- and document that semantic in the macro doc (currently the null-fallback behavior at `lib.rs:39`/`:128-132` is described but its rationale is not).

### 3. User errors travel as bare trap strings; no structured error envelope
File: `crates/shamir-sdk-macros/src/lib.rs:266-274` (`function`), `:393-401` (`procedure`), `:558-566` (`scalar`); TODO at `:251`
Severity: medium
Issue: `Err(e)` from the user's function is converted to `shamir_sdk::__rt::trap(&e.to_string())` -- a guest panic. Per `__rt.rs:63-64`, the host maps *any* trap to `FunctionError::Compute`, so a deliberate `Error::user("insufficient funds")` is wire-indistinguishable from a genuine crash, and the error taxonomy built into `shamir_sdk::Error` is discarded at the ABI boundary. The in-code `TODO(slice 4)` acknowledges this, but it is a live wire-protocol property today.
Failure scenario: A procedure legitimately returns `Error::user("duplicate key")`; the host surfaces `FunctionError::Compute` to the client, which retries or reports an internal error instead of a user error. Debuggability relies entirely on parsing a `panic!`-formatted string ("shamir function error: {msg}") across the guest boundary.
Suggested fix: Land the planned envelope: encode `Result` as a tagged msgpack value (e.g. `{"ok": value}` / `{"err": {"kind": "user"|"compute", "message": ...}}`) in `leak_result`'s place, or reserve a leading marker byte, and have the host map the tagged branch to `FunctionError::User`. Keep trap only for genuine guest crashes.

### 4. Diagnostics via `assert!`/`panic!` instead of spanned `compile_error!`
File: `crates/shamir-sdk-macros/src/lib.rs:51-54, 57-60, 63-72, 81, 183-202, 211, 314-334, 344, 471-481, 486-492, 513`
Severity: medium
Issue: Every rejection path uses `assert!`/`panic!`, which aborts macro expansion with "proc-macro panicked" and no underline on the offending item. CLAUDE.md's error-handling rules ("Avoid `panic!` outside `unreachable!()` / invariant violations that mean a programmer bug") treat user mistakes as errors to report, not macro-programmer bugs to panic on; for proc-macros the sanctioned mechanism is `syn::Error::new_spanned(..).into_compile_error()`.
Failure scenario: A user writes `#[scalar] fn f(a: Params, b: Params) -> Result<Value>`; the panic message names the rule but the compiler shows no span on the actual argument list, and on the return-type paths the offending type is only visible inside the panic text (see finding 1). Multi-error signatures report only the first problem.
Suggested fix: Replace each `assert!` with `return syn::Error::new_spanned(fn_item.sig.ident / ty, "...").to_compile_error().into()`; accumulate errors where cheap. This also composes with the fix for finding 1.

### 5. No compile coverage for `#[function]` / `#[validator]`; no compile-fail tests anywhere
File: `crates/shamir-sdk-macros/` (no `tests/` directory at all); `crates/shamir-sdk/tests/` contains only `procedure_compile_pass.rs`, `scalar_compile_pass.rs`
Severity: medium
Issue: Two of the four public macros have zero compile coverage in the entire workspace (the only references to `#[validator]`/`#[function]` outside the macros crate are doc comments). All rejection paths (non-async, wrong arity, wrong return type, `Ctx` in `#[scalar]`) are untested, and the string-matching checks of finding 1 are exactly the behavior that compile-pass/compile-fail tests pin down. CLAUDE.md's TDD protocol and per-module `tests/` organization are not honored in this crate.
Failure scenario: A refactor of the return-type check (e.g. fixing the `std::result::` gap) silently breaks `#[function]` acceptance, and no test fails.
Suggested fix: Add `crates/shamir-sdk-macros/src/tests/` or extend `crates/shamir-sdk/tests/` with compile-pass files for `#[validator]` and `#[function]` (one file per macro -- the existing files' header comments correctly note `shamir_alloc` symbol collisions across shared test binaries), covering each accepted spelling from finding 1, plus a `trybuild` suite for the rejection paths. Also add a round-trip ABI test (encode params -> `shamir_call` -> decode result) so the wire shape of findings 2/3 is regression-guarded.

### 6. `#[procedure]` doc example does not compile
File: `crates/shamir-sdk-macros/src/lib.rs:290-294`
Severity: low
Issue: The example body is `let rows = ctx.db().table("users").query(None); Ok(rows)`, but `Table::query` returns `Result<Vec<Value>>` (`crates/shamir-sdk/src/db.rs:98`): the `?` is missing and `Vec<Value>` is not `Value`. The prelude's counterpart example (`crates/shamir-sdk/src/prelude.rs:35-36`) is correct.
Failure scenario: A guest author copies the macro's own doc example and hits two type errors that look like SDK bugs.
Suggested fix: Change the body to `let rows = ctx.db().table("users").query(None)?; Ok(Value::List(rows))`.

### 7. Attribute payload silently ignored; "one macro per crate" constraint unenforced
File: `crates/shamir-sdk-macros/src/lib.rs:44, 176, 307, 464` (`_attr` discarded); `:15, :157, :283, :437` (one-per-crate docs)
Severity: low
Issue: `#[function(anything)]`, `#[scalar(...)]` etc. are accepted without comment, so typo'd options silently do nothing. The documented "only one per crate" rule is not enforced by the macro: two expansions emit duplicate `#[no_mangle]` `shamir_alloc`/`shamir_call`, producing an opaque duplicate-symbol linker error far from the macro call sites.
Failure scenario: A user writes `#[procedure(auto_reload)]` expecting a behavior change and gets none; a user applies two macros and debugs a linker error instead of a compile error at the second attribute.
Suggested fix: Error on a non-empty attribute payload; enforce single-entrancy the standard way (each expansion emits a `const _: () = ...`/`static` collision sentinel in a fixed link section or references a per-crate `#[no_mangle]`-adjacent symbol so the second use fails at compile time with a pointing message).

### 8. `shamir_alloc` performs no length validation; negative `len` allocates ~2^63 bytes
File: `crates/shamir-sdk-macros/src/lib.rs:109-114` (and verbatim copies at `:239-244`, `:370-375`, `:537-542`)
Severity: low
Issue: `len as usize` on a negative `i32` wraps to a huge value; `vec![0u8; huge]` aborts the guest. The host is trusted and an abort is still a trap, but the ABI contract ("host wrote `len` bytes") is silently undefined for `len < 0`, and the check costs one branch.
Failure scenario: A buggy or fuzzed host passes `-1`; the guest dies with an uninstrumented allocation abort rather than a diagnosable trap.
Suggested fix: `if len < 0 { /* trap or return -1 */ }` first; also `shrink_to_fit` is unnecessary but consider documenting that `len = 0` returns a valid dangling-ish pointer (currently `Vec::new()`-equivalent alignment is fine, but the contract is implicit).

### 9. Four macros in one `lib.rs` with the ABI emitter duplicated four times
File: `crates/shamir-sdk-macros/src/lib.rs` (whole file, 572 lines)
Severity: low
Issue: CLAUDE.md's "One file = one primary export" rule suggests one file per macro with `lib.rs` re-exporting. More materially, `shamir_alloc` and `shamir_call` are emitted by four near-identical hand-written `quote!` blocks (~150 duplicated lines); the packed `(ptr << 32) | len` return convention lives only in `__rt::leak_result` plus four doc/comment copies. Any calling-convention change (e.g. the error envelope of finding 3) must be replicated in four places -- the drift that finding 1 already exhibits across the validators can recur in the ABI itself.
Failure scenario: A future slice changes the result encoding in `#[function]`'s emitter but not `#[scalar]`'s; two guest kinds speak different dialects of the same wire protocol.
Suggested fix: Extract a shared `fn emit_guest_abi(kind: Kind, ...) -> TokenStream` used by all four macros; split each macro into its own file per the repo convention.

### 10. `type_contains_ctx` purity check is lexical, not semantic
File: `crates/shamir-sdk-macros/src/lib.rs:425-432`
Severity: nit
Issue: The `#[scalar]` `Ctx` rejection matches the literal identifier `Ctx` in any type path: a user's coincidental `my_app::Ctx` type is falsely rejected, while `use shamir_sdk::Ctx as C; x: C` is falsely accepted. Actual purity is enforced structurally (the wrapper hardcodes the parameter to `shamir_sdk::Params` at `lib.rs:525`, and no `Ctx` is ever constructed), so this check is only a lint -- the doc's "**No argument type may contain `Ctx`**" phrasing implies more than it delivers.
Failure scenario: Minor: confusing rejection of an unrelated user type named `Ctx`; no real purity escape (the structural guarantee holds).
Suggested fix: Match `Ctx` only when the path's last segment is `Ctx` (like the fix in finding 1), and soften the doc wording to "declared as `Ctx`".
