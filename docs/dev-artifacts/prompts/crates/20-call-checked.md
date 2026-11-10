# Brief: checked stored calls in shamir-lang (PROC-2c stage 2b)

## Why

Stage 2a (the previous brief, `crates/18-call-hardening.md`, landed) gave the legacy guest `call`
import one hardened resolution path in the host: a public `StoredCallResolver` seam carried by `FnCtx`
and implemented in `shamir-db` (`crates/engine/shamir-db/src/shamir_db/shamir_db/db/function/invocation.rs`),
captured bindings (`RegistryBinding`), per-hop Execute authorization, effective actor per
Definer/Invoker/setuid, the owner guard, rebuilt callee capabilities, and the active-call identity path
(`crates/wasm/shamir-wasm-host/src/runtime/registry/`). Read those files first.

The `shamir` language still cannot call another stored procedure. This stage adds the checked
call: `call(<literal target>, { ...args })` in the language, compiled to a new host import
`call_checked`, with the callee's signature compared byte for byte at runtime, using the stage 2a
resolver. The reviewed design is `docs/dev-artifacts/design/shamir-lang-call-refs-addendum.md`
(read sections 1, 2.1, 2.2, 2.3, 2.4, the stage 2 row of section 6 and the stage 2 bullets of section 7,
and F1/F9/F12 of section 10), then `docs/dev-artifacts/design/shamir-lang-design.md` where it speaks
of calls and imports. Decisions there are made; do not re-open them.

## What

1. **Signature metadata (compiler-owned).** `shamir-lang::compile` additionally returns the canonical
   signature descriptor bytes of the entry procedure (`procedure(p1:T1,p2:T2)->R`, exact grammar of
   addendum 2.1: no whitespace, parameters in declaration order, record fields sorted by UTF-8 bytes,
   `[T]` lists, `?` optionals, scalar tokens `i32 i64 f64 bool string bytes null`; the function name is not
   part of it; opaque types are not callable signature types). Extend the compile result or accompany
   it with compiler-owned metadata; say which. Scalar and validator entries have no callable signature.
2. **`call` intrinsic.** Procedure-only; the first argument is a string literal (compile-time) naming a
   stored `shamir` procedure, the second a record naming exactly the target's declared parameters
   (no positional arguments, no implicit conversions; nullable parameters must be present and may
   be nil; an empty record encodes as an empty MessagePack map). The expected result type is the target's
   exact declared result including nullability. The compiler checks the call against a catalog of
   previously emitted signatures that the caller supplies as an input to `compile` (a plain
   name -> signature-bytes map; no async, no I/O in `shamir-lang`). Diagnostics: `T004` target or
   signature mismatch, `T007` capability (scalar and validator entries reject the effect; helpers inherit
   it transitively), `T008` known local cycle/depth, `T009` unsupported boundaries; reuse existing codes,
   add none unless unavoidable (then say so).
3. **Emission and ABI.** Import `(shamir_host, call_checked)`:
   `(i32 name_ptr, i32 name_len, i32 signature_ptr, i32 signature_len, i32 args_ptr, i32 args_len) -> i64 packed_result`.
   The guest embeds the target name and its expected signature bytes, encodes the argument map with the
   shared encoder, decodes the returned bytes with the shared codec and projects to the declared result
   (malformed or incompatible shape is codec/projection kind 3; a missing/zero host result is a
   host-contract failure, not a nullable result; a nil result is a non-zero packed result carrying
   MessagePack nil). Keep the import-count ceiling of the main design in mind (addendum 2.1, last
   paragraph); say what the compiler's emitted ceiling is.
4. **Admission and linker together.** Add `call_checked` to the admission import allowlist
   (`crates/wasm/shamir-wasm-admission/src/sanitizer.rs`, with its exact 6-parameter signature check)
   and to the host linker (`crates/wasm/shamir-wasm-host/src/wasm/wasm_function.rs` import
   registration plus a new binding next to `bindings/call.rs`). Never one without the other.
5. **Host runtime.** The `call_checked` binding reuses the stage 2a path (`dispatch_stored_call` /
   resolver / active path / depth / fuel lease handling, exactly as `host_call`), but the resolver also
   receives the expected signature bytes. In `shamir-db` the checked resolver captures binding + record
   once (same capture-read-recapture rule), requires the stored signature to exist and to equal the
   guest's bytes exactly, and fails closed on a missing signature (legacy artifact, native function,
   raw wasm upload): such targets are not callable through `call_checked`. Everything else
   (Execute as the current hop's actor, effective actor, owner guard, rebuilt capabilities, repeat and
   32-entry rejection) is the shared stage 2a code; do not fork it. Extend the shared seam rather than
   duplicating (e.g. an optional expected signature in the resolve request) and list the changes.
6. **Persistence.** The signature bytes are persisted atomically with the artifact on every
   create/replace path of `shamir` source functions (`management.rs` create/alter/rebuild paths and the
   `lang = "shamir"` wiring from milestone 6); replacing an artifact replaces the signature in the same
   catalogue write; rename keeps it; functions created any other way carry none. The record shape change
   must stay readable by existing rows (missing field = no signature). The compile service in
   `shamir-wasm-host` (`create_shamir_source` / `check_shamir_source`) and `shamir-db` supply the
   catalog snapshot (all stored signatures) to the compiler at create/check time.
7. **No other surface.** No reference (`$query`, `after`), no gateway-originated `Call`, no new
   fuel/lease behaviour, no change to the legacy `call` import, no version changes.

## Tests (they run in CI, not here)

* `shamir-lang`: compile units for signature emission (grammar vectors incl. records, lists, optionals,
  nullable results, empty parameter list), each diagnostic, effect rejection in scalar/validator
  entries, helper transitivity, the emitted import and its type, mutation-fuzz and purity guards still pass
  (the production dependencies must stay exactly as pinned by the existing guard).
* Admission: the import accepted with the exact signature, rejected with a wrong one.
* Host + `shamir-db` end to end with real compiled procedures (use the existing helpers in
  `crates/engine/shamir-db/tests/functions/gateway/shamir_lang_*_e2e.rs` and
  `crates/wasm/shamir-wasm-host/src/tests/runtime/lang/`): A calls B with arguments and decodes the
  result; nullable and record results; wrong argument name/type is a compile error; B replaced with a
  different signature -> the call fails at runtime with the mismatch, not silently; B renamed -> the call
  fails (target not found), the signature stays with the artifact; legacy/native/raw-wasm target
  -> refused; Execute denied for the current actor; Definer target runs as owner and its tables are
  reachable only by it; Definer parent -> Invoker target of another owner refused; repeat (A -> B -> A)
  refused at the repeat; 32-entry boundary; cancelled call leaves the path clean.
* Existing tests: never delete or weaken; list every changed existing test with the reason.

## Observable acceptance

1. All of the above implemented and tested; expectations derived from the rules above, not from the
   implementation.
2. `cargo fmt -p <crate> -- --check`, `cargo check -p <crate> --all-targets` and
   `cargo clippy -p <crate> --all-targets -- -D warnings` clean for every touched crate
   (`shamir-lang`, `shamir-wasm-admission`, `shamir-wasm-host`, `shamir-engine`, `shamir-db` as touched).
3. `python -I scripts/ci/check_layout.py` passes (at most 7 entries per folder, 1000 lines per file; split
   rather than grow; the compiler's `check`/`back` folders are close to their limits).
4. rust-guard rules: no logic in `mod.rs`, no inline `#[cfg(test)]` modules, no local `use`, no new
   `#[allow]`, no visibility widening beyond what the seam strictly needs (list every one).
5. Docs in the same change: `docs/dev-artifacts/design/shamir-lang-design.md` (intrinsic table, imports
   and ceiling, call chapter), `docs/dev-artifacts/design/shamir-lang-call-refs-addendum.md` status line
   (stage 2 implemented; references still not), and one sentence in the `shamir-lang` paragraph of
   `CLAUDE.md` and `AGENTS.md`.

## Non-goals

References and `batch_after`, gateway `Call`, changes to the legacy `call` import, fuel/lease changes,
signature versions/hashes, migration of legacy artifacts, version changes.

## Constraints

Compilation, builds and tests run in CI, not here. The only allowed commands are
`cargo fmt -p <crate>`, `cargo check -p <crate> --all-targets`,
`cargo clippy -p <crate> --all-targets -- -D warnings` and
`python -I scripts/ci/check_layout.py`, plus read-only git. Run cargo only under
`wnice -p idle -c 50 -t 8 -m 50 --` and with `-j 2`. No `cargo build/test/run/bench/doc`, no nextest,
no npm. Run `cargo fmt -p <crate>` for every touched crate before the final report.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

Your last message must be a final report: what is implemented, where the signature lives and in which
record field, the compile-result shape chosen, the import ceiling, every widened item, every existing
test changed and why, anything unverified. Do not end on a tool call.
