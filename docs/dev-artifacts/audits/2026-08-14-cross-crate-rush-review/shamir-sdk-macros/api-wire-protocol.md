<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-sdk-macros — api-wire-protocol independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Public signature acceptance and boundary/error semantics have concrete gaps. Export uniqueness is already enforced. The bad procedure example is source-proven; duplication is optional maintenance work.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 10 | 9 | 0 | 0 | 0 | 0 | 1 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — Return-type validation is string-based, per-macro inconsistent, and rejects valid spellings

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

The same qualified SDK Result spelling is accepted by procedure/scalar and rejected by function; qualified Validation is rejected. std::result:: is omitted despite the helper's qualification assurance. Compiler-enforced concrete SDK returns prevent runtime foreign-type confusion. Use a cross-macro compile matrix, not snapshots alone.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:66](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L66); [crates/shamir-sdk-macros/src/lib.rs:197](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L197); [crates/shamir-sdk-macros/src/lib.rs:408](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L408); [crates/shamir-sdk/tests/procedure_compile_pass.rs:11](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/tests/procedure_compile_pass.rs#L11).

<a id="review-2"></a>

### Claim 2 — Wire protocol has no decode-failure channel: malformed input silently becomes empty Params / Null record

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Failed/non-map decoding loses the error channel before validator fallbacks. Ordinary host construction emits a map; abnormal wire input is required for the reported malformed-input scenario.

Evidence: [crates/shamir-sdk/src/__rt.rs:11](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/__rt.rs#L11); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:407](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L407).

Grouping/duplicate: [security-crypto.md#1](security-crypto.md#review-1). This is not an additional independent defect.

<a id="review-3"></a>

### Claim 3 — User errors travel as bare trap strings; no structured error envelope

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Err takes the panic path and becomes Compute, not User. This is a classification defect; actual panic-message delivery and rich guest error variants are not established.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:272](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L272); [crates/shamir-wasm-host/src/wasm/wasm_function.rs:593](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/wasm/wasm_function.rs#L593).

Grouping/duplicate: [error-handling-lifecycle.md#2](error-handling-lifecycle.md#review-2). This is not an additional independent defect.

<a id="review-4"></a>

### Claim 4 — Diagnostics via `assert!`/`panic!` instead of spanned `compile_error!`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Semantic validation remains assert/panic-based; parse_macro_input already handles parsing through compile errors.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:45](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L45); [crates/shamir-sdk-macros/src/lib.rs:486](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L486).

Grouping/duplicate: [error-handling-lifecycle.md#1](error-handling-lifecycle.md#review-1). This is not an additional independent defect.

<a id="review-5"></a>

### Claim 5 — No compile coverage for `#[function]` / `#[validator]`; no compile-fail tests anywhere

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

The function-zero-coverage facet is refuted by the registered WASM test. Validator and signature UI/error-path coverage remain absent; existing successful bare-prelude compilation cannot catch rejected qualified spellings.

Evidence: [crates/shamir-wasm-host/src/tests/compile_tests.rs:11](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/compile_tests.rs#L11); [crates/shamir-wasm-host/src/tests/compile_tests.rs:19](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/tests/compile_tests.rs#L19); [crates/shamir-sdk-macros/Cargo.toml:13](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/Cargo.toml#L13).

Grouping/duplicate: [correctness-tdd.md#1](correctness-tdd.md#review-1). This is not an additional independent defect.

<a id="review-6"></a>

### Claim 6 — `#[procedure]` doc example does not compile

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

query(None) returns Result&lt;Vec&lt;Value&gt;&gt;, but the example directly wraps it in Ok for a Result&lt;Value&gt; function. The independent prelude example correctly uses ? followed by Value::List.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:292](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L292); [crates/shamir-sdk/src/db.rs:98](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/db.rs#L98); [crates/shamir-sdk/src/prelude.rs:35](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/prelude.rs#L35).

<a id="review-7"></a>

### Claim 7 — Attribute payload silently ignored; "one macro per crate" constraint unenforced

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Nonempty attribute arguments are silently discarded. The separate unenforced-uniqueness facet is refuted by fixed ABI symbol collisions; specialized diagnostics are optional.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:44](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L44); [crates/shamir-sdk-macros/src/lib.rs:108](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L108).

Grouping/duplicate: [correctness-tdd.md#10](correctness-tdd.md#review-10). This is not an additional independent defect.

<a id="review-8"></a>

### Claim 8 — `shamir_alloc` performs no length validation; negative `len` allocates ~2^63 bytes

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The signed cast remains unchecked. On the declared wasm32 guest target, -1 becomes usize::MAX, approximately 4 GiB, not 2^63; capacity rejection can precede actual allocation.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:110](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L110); [crates/shamir-wasm-host/src/compile.rs:539](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-wasm-host/src/compile.rs#L539).

Grouping/duplicate: [security-crypto.md#2](security-crypto.md#review-2). This is not an additional independent defect.

<a id="review-9"></a>

### Claim 9 — Four macros in one `lib.rs` with the ABI emitter duplicated four times

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

Duplication is present, but closely coupled macros are permitted and no current wire divergence follows merely from file organization. Extraction is optional maintenance; actual return-check divergence is tracked separately.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:91](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L91); [crates/shamir-sdk-macros/src/lib.rs:521](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L521); [CLAUDE.md:505](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L505).

Grouping/duplicate: [style-claude-md.md#1](style-claude-md.md#review-1). This is not an additional independent defect.

<a id="review-10"></a>

### Claim 10 — `type_contains_ctx` purity check is lexical, not semantic

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The argument check does not resolve aliases, but passing an actual Ctx still conflicts with concrete Params forwarding. The real purity escape is Ctx construction inside a valid body, subject to host policy.

Evidence: [crates/shamir-sdk-macros/src/lib.rs:425](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L425); [crates/shamir-sdk-macros/src/lib.rs:525](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk-macros/src/lib.rs#L525); [crates/shamir-sdk/src/context.rs:64](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-sdk/src/context.rs#L64).

Grouping/duplicate: [correctness-tdd.md#9](correctness-tdd.md#review-9). This is not an additional independent defect.

## Evidence and recipe corrections

- Changing all successful results to an unversioned {ok:...}/{err:...} map is incompatible with the current raw QueryValue ABI and arbitrary user maps. Coordinate explicit ABI identification and legacy decode behavior; a small retrofitted leading tag can collide with existing MessagePack.
- Final-segment syntax checks cannot establish semantic type identity or resolve aliases. Preserve concrete SDK compatibility checks.
- Guest ABI runtime tests must use the guest target; native 64-bit pointers cannot be represented by these i32 exports.
- Published rmp-serde 1.3.1 src/decode.rs routes this Value through deserialize_any, whose array/map branches decrement depth. This does not justify a universal depth assurance for every decoder dispatch: https://docs.rs/crate/rmp-serde/1.3.1/source/src/decode.rs; pin Cargo.lock:2949.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

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

</details>
