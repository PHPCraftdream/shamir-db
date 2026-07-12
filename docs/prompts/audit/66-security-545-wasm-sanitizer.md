Task #545 (CRITICAL residual) — build a static structural sanitizer/
verifier over a compiled `.wasm` artifact, run before `wasmtime`
instantiation, that enforces a sanctioned host-import ABI allowlist. This
is defense-in-depth on TOP of existing runtime sandboxing (fuel/memory
caps, epoch interruption — already landed, do not re-implement) and the
existing compile-time lexical forbid-scan (`compile.rs:57-63,399-414` —
already landed, do not re-implement).

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## ARCHITECTURAL CONSTRAINT (explicit user directive — do not violate)

WASM modules are compiled by us (the operator), never accepted as
arbitrary untrusted bytecode from an anonymous party with zero
compilation step. **Do NOT** propose or implement "refuse to load
user-supplied WASM" or "remove WASM execution" as any part of this fix.
The deliverable is a sanitizer/verifier that runs as part of OUR OWN
compile/load pipeline and rejects unsafe artifacts before instantiation —
not a ban on WASM.

## MANDATORY first step: verify the actual marginal value before designing

The orchestrator's own investigation (re-verify — code may have shifted)
found that `wasmtime`'s `Linker::instantiate_pre` (called in
`crates/shamir-wasm-host/src/wasm/wasm_function.rs`'s
`build_instance_pre`, ~lines 124-164) ALREADY fails to resolve a module
whose imports aren't satisfied by the linker's registered functions —
i.e. a module importing anything other than the exact
`("shamir_host", <name>)` pairs the linker registers already fails to
instantiate today, with zero sanitizer needed. The sanctioned set,
enumerated directly from `build_instance_pre`'s registrations at the
time of this brief:

```
("shamir_host", "batch_put")
("shamir_host", "batch_get")
("shamir_host", "global_set")
("shamir_host", "global_get")
("shamir_host", "call")
("shamir_host", "db_get")
("shamir_host", "db_insert")
("shamir_host", "db_query")
("shamir_host", "db_execute")
("shamir_host", "http_fetch")
```

No WASI is registered anywhere in `wasm_engine.rs`/`wasm_function.rs` (no
`wasmtime_wasi` import found) — confirm this is still true.

**Before writing any sanitizer code**: write a small throwaway test (or
just verify by reading `Linker`/`Module` documentation + existing
`from_wat`/`from_binary` test coverage) confirming a WAT/wasm module with
a bogus import (e.g. `(import "evil" "syscall" (func))`) genuinely fails
at `instantiate_pre` today, BEFORE any host-import call ever happens.
This determines the sanitizer's REAL value-add, which is NOT "block an
attack the linker doesn't already block" (it likely already does) but
one or more of:

1. **Fail-fast, cheaper than full wasmtime compilation.** A
   `wasmparser`-based structural pass over the raw bytes (import/export
   section headers only) is far cheaper than constructing a full
   `wasmtime::Engine`/`Module` (which does full validation + Cranelift
   compilation). Rejecting an obviously-malicious artifact before paying
   that cost is a real, if modest, defense-in-depth/DoS-cost-reduction
   value.
2. **An explicit, auditable ABI allowlist independent of linker wiring.**
   Today, "what a WASM module may import" is IMPLICITLY defined by
   whatever happens to be registered in `build_instance_pre` — there is
   no single, explicit, human-and-tool-auditable list anyone can point at
   and say "this is the sanctioned ABI". A sanitizer with its own
   allowlist is a structurally independent control: if a future change
   accidentally broadens the linker's registered surface (e.g. someone
   adds a new host import for a feature and forgets to think about its
   security surface, or a dependency upgrade silently enables WASI), the
   sanitizer's OWN allowlist — reviewed/updated deliberately, not
   incidentally — still catches modules trying to use anything beyond
   what was explicitly sanctioned, closing the gap between "what the
   linker happens to accept" and "what we intended to allow."
3. **Structural checks the linker does NOT perform.** Investigate
   (`wasmparser`'s crate docs / `wasmparser::Parser`) whether it can
   cheaply assert things `instantiate_pre` doesn't care about at all:
   unexpected EXPORTS (does `WasmFunction` call by a fixed, known
   entrypoint name, or could a module export something unexpected that
   later code mistakenly invokes? — check `wasm_function.rs`'s
   invocation call site for how it looks up the entrypoint), memory/table
   MINIMUM size declarations (separate from wasmtime's own `StoreLimits`
   enforcement — confirm whether an oversized minimum could cause
   resource pressure during Module compilation itself, before
   `StoreLimits` would ever kick in), start-function abuse, or
   suspiciously-shaped data/element segments.

Report your findings on all of the above honestly in your final report
BEFORE describing what you built — if wasmtime's own linker resolution
already makes items 1-3 moot in practice, say so and scope the sanitizer
to whichever value-adds are real, rather than building a sanitizer that
merely re-implements what `instantiate_pre` already guarantees for free.

## The fix (once scope is grounded by the investigation above)

Add `wasmparser` as a direct dependency of `crates/shamir-wasm-host`
(already present transitively via wasmtime — check
`Cargo.lock`/`cargo tree -p shamir-wasm-host -i wasmparser` for the
version wasmtime itself pins, and match it to avoid a duplicate/
conflicting version in the dependency graph).

Implement a structural verification pass — a function like
`verify_wasm_module(bytes: &[u8]) -> Result<(), SanitizeError>` — that
parses the module's import/export sections (via `wasmparser::Parser`)
and:
- **Allowlist posture** (not denylist — enumerate exactly what's
  permitted, reject everything else by default): every import must be
  `("shamir_host", <name>)` where `<name>` is one of the 10 names listed
  above (re-verify the exact list against the current
  `build_instance_pre` before hardcoding it — keep the two lists in sync,
  ideally by having one be the single source of truth the other is
  derived from or tested against, so this sanitizer can't silently drift
  out of sync with the linker's actual registrations — investigate the
  cleanest way to keep them from diverging, e.g. a shared const array
  both `build_instance_pre` and the sanitizer consume).
- Reject any import from any OTHER module name (catches a smuggled `env.*`
  WASI-style import, or any host-function name not in the sanctioned
  set).
- Whatever additional structural checks your investigation above found to
  be genuinely load-bearing (exports/memory/etc.) — implement only what
  you've concretely justified, not a kitchen-sink of every possible
  wasmparser assertion.

Wire this verification to run BEFORE `WasmFunction::from_binary` (or
wherever `Module::new`/`instantiate_pre` is first reached) in the actual
compile/load pipeline — find every call site that turns compiled bytes
into a live `WasmFunction` (`compile.rs`'s output feeding into
`from_binary`, and any direct `FunctionSource::Wasm` load path bypassing
the Rust-source compile step) and ensure the sanitizer runs on ALL of
them, not just the `cargo build` output path.

## Explicitly OUT of scope — flag as a separate decision, do not implement

Whether native `cargo build` itself (inside
`compile_rust_source_with_timeout`) should be additionally isolated
(container/seccomp/rlimit/user-namespace) is a SEPARATE, larger decision
this task does not resolve. Do not attempt to sandbox the build process
itself. If your investigation surfaces a concrete, cheap mitigation here
(e.g. an existing crate-level feature), mention it in your report as a
recommendation, but do not implement it as part of this task — the
orchestrator will file (or decline) a dedicated follow-up.

## Test requirement

- A `.wasm` module (WAT is fine — check `WasmFunction::from_wat`'s
  existing test-construction pattern) with an import from an
  unsanctioned module/name is rejected by the sanitizer BEFORE
  `wasmtime::Module`/`instantiate_pre` is ever reached (assert this by
  showing the rejection happens even for well-formed WASM that wasmtime
  itself would otherwise happily attempt to instantiate — construct a
  case that's valid WAT/wasm syntax with a bogus import, not malformed
  bytes that would fail wasmtime parsing anyway, so the test genuinely
  isolates the sanitizer's own check).
- A legitimate module using only the sanctioned imports still loads and
  invokes successfully (no regression on the happy path — reuse/extend
  an existing `from_wat`/`from_binary` test).
- If you implement any additional structural check (exports/memory/etc.),
  add a corresponding positive+negative test pair for it too.

## Test scope

```
./scripts/test.sh -p shamir-wasm-host
```

## Verification (lighter per-task gate, agreed for this campaign)

```
cargo check --workspace --all-targets
./scripts/test.sh -p shamir-wasm-host
```
Full fmt/clippy/test --full is FINAL-GATE's (#529's) job. **This IS a
FINAL-GATE blocker (CRITICAL residual)** — already reflected in #529's
`blockedBy`, no action needed there.

## Report format

```
[Investigation] (MANDATORY, report BEFORE implementation details)
  > Confirmed (or refuted): wasmtime's Linker::instantiate_pre already
    rejects an unsanctioned import today, with evidence
  > Real marginal value of a pre-instantiation sanitizer, honestly
    assessed against items 1-3 above
  > Sanctioned import list re-verified against current
    build_instance_pre (list any drift from this brief's enumeration)
  > Any additional structural check found genuinely load-bearing
    (exports/memory/etc.), with reasoning — or confirmation that none
    were found beyond the import allowlist
[Implementation] Status: fixed / scoped-down-with-followup
  > verify_wasm_module (or equivalent) added, wired before EVERY path
    that turns compiled bytes into a live WasmFunction — list each call
    site
  > Sync mechanism between the sanitizer's allowlist and
    build_instance_pre's actual registrations (shared const / test that
    cross-checks them / etc.) so they cannot silently drift apart
  > New tests: confirmed RED before the fix, GREEN after
[Verification]
  cargo check --workspace --all-targets: pass/fail
  ./scripts/test.sh -p shamir-wasm-host: pass/fail
```

Given this is a CRITICAL security-boundary change gating FINAL-GATE,
this MUST go through an adversarial review pass before committing — same
discipline as #537/#540/#541/#542/#543/#544 this campaign, but with
extra scrutiny given the severity. If that review finds a genuine bug,
the orchestrator fixes it directly (never re-delegates), re-verifies,
and sends the fix through a second review pass before committing.
