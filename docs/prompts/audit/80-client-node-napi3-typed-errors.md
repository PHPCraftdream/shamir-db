בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Brief: napi-rs 2.x → 3.x upgrade + typed `.code`/`.retryable` errors (task #519)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Context

`crates/shamir-client-node`'s `to_napi` (in `src/lib.rs`) currently
collapses every server error into a plain string via
`Error::from_reason(e.to_string())` — the JS side gets only
`.message`, no structured `.code`/`.retryable`, unlike the TS SDK's
`ShamirDbError` (task #497, `crates/shamir-client-ts`). This was
explicitly deferred pending a napi-rs major-version bump, which is
normally out of scope without explicit user request — **the user has
now explicitly authorized this specific bump.** Confirmed baseline:
this crate builds cleanly today (`cargo build --release` in
`crates/shamir-client-node`, ~6 min, MSVC toolchain present and
working) — re-confirm this baseline builds before touching anything.

**Read `crates/shamir-client-node/src/lib.rs` in full first** to see
every `#[napi]`-annotated struct/method, the current `to_napi`
function, and every call site that currently returns `Result<T,
napi::Error>` (2.x's plain, `Status`-typed error).

## IMPORTANT — the exact typed-error mechanism is genuinely uncertain;
## verify via real compilation, do not trust documentation alone

Two independent research passes produced CONTRADICTORY claims about
whether napi-rs 3.x lets an `#[napi]`-annotated **async** function
return `Result<T, CustomErrorType>` where `CustomErrorType` is
something other than the default `napi::Error<Status>`:

- One pass claimed you can return `Result<T, MyErrorObject>` where
  `MyErrorObject` is a `#[napi(object)]` struct with `code`/`message`/
  `retryable` fields, and napi-rs serializes it as a structured JS
  object.
- A second, GitHub-issue-sourced pass found that `napi::Error<S>` IS
  generic over `S: AsRef<str>` (default `Status`), but the `#[napi]`
  macro's codegen for **async** functions specifically pins the
  return-position `Result`'s error type to the default
  `Status`-parameterized `napi::Error` — meaning a custom `S` (or an
  entirely different error type) may NOT be usable directly in an
  async fn's exported signature, only in lower-level contexts (manual
  `Task` impls, `ThreadsafeFunction` callbacks).

**Do not guess which is correct from documentation. Resolve it by
actually compiling a minimal test** — e.g. write one small
`#[napi]` async method returning `Result<Buffer, YourCandidateErrorType>`
(whatever shape you're trying) in an isolated scratch spot, run
`cargo check -p shamir-client-node`, and read the ACTUAL compiler
error/success. Let the compiler settle the disagreement, not further
web research. Budget real iteration time for this — it's the crux of
the task.

## Fallback design if the Rust-side custom-error-type approach doesn't
## work for async fns (have this ready, don't discover it's needed and
## then improvise from scratch)

If napi-rs 3.x genuinely cannot carry a structured `.code`/`.retryable`
through an async `#[napi]` fn's thrown `Error` (i.e., you're stuck with
`status`/`reason` on the standard `Error<Status>`), the fallback is to
move structured-error construction to the **JS side of the boundary**
instead of the Rust/napi boundary:

- Keep the async methods returning `Result<Buffer, napi::Error>` as
  today for genuine infrastructure failures (connection closed, invalid
  args, panics) — these stay plain JS `Error`s with just `.message`,
  matching current behavior (acceptable, since these aren't the
  `ShamirDbError`-parity case task #497/#519 care about).
- For **domain-level DB errors** (a `DbResponse::Error { code, message
  }` coming back over the wire — the actual case task #519 is about),
  do NOT throw a napi `Error` at all. Instead let the async method
  return `Ok(Buffer)` where the buffer is the msgpack-encoded
  `DbResponse::Error` payload itself (exactly like a success payload),
  and add a **thin JS-side wrapper** (in whatever `.js`/`.d.ts` file
  already sits between the raw native binding and the published
  package — check `crates/shamir-client-node/index.js`/`index.d.ts`
  or wherever the JS entry point lives) that decodes the msgpack
  buffer, checks whether it's a `DbResponse::Error` shape, and if so
  constructs and throws a real JS `Error` subclass (mirroring the TS
  SDK's `ShamirDbError` from `crates/shamir-client-ts` — check that
  file for the exact shape: `.code`, `.message`, `.retryable`) instead
  of returning it as a success value. This keeps the Rust/napi layer
  completely unchanged in its error-handling shape (avoids the
  uncertain napi-rs feature entirely) and does the actual "make
  structured error data typed on the JS side" work in JS, where it's
  trivial.

Pick WHICHEVER of these two actually compiles and works, based on your
real experimentation — report which one you landed on and why in your
final summary.

## Scope

### §1. Version bump

Bump `crates/shamir-client-node/Cargo.toml`:
- `napi = { version = "3", ... }` (pin to the latest stable 3.x you can
  confirm via `cargo search napi` or crates.io — verify the exact
  number yourself rather than trusting any pre-supplied figure, since
  new patch releases ship constantly).
- `napi-derive = "3"` (matching major version).
- `napi-build` build-dependency: verify whether it needs to move to a
  3.x line too, or whether it stays compatible at its current major
  version pinned against napi 3.x (check napi-build's own crates.io
  page / changelog for napi-3.x compatibility — do not assume).
- Keep the existing `features = ["napi6", "async"]` unless you find a
  documented reason napi-rs 3.x renamed/restructured these features
  (verify via the crate's own `Cargo.toml`/docs on docs.rs for the
  version you pin, not assumption).

### §2. Typed error mechanism

Implement whichever approach (Rust-side custom error type, or the
JS-side wrapper fallback) your compilation experiment (see above)
proves actually works, delivering: a domain-level DB error surfaces to
JS-side calling code with `.code: string` and `.retryable: boolean`
properties, matching the TS SDK's `ShamirDbError` shape as closely as
this binding's existing conventions allow. Update `to_napi` (or
whatever replaces/wraps it) accordingly.

### §3. Update every call site

Every `#[napi]` async method in `src/lib.rs` that currently uses
`to_napi`/`Error::from_reason` needs to route through the new
mechanism consistently — grep for every use, don't miss one.

### §4. Tests

This crate's existing test setup (check whether it has any — inspect
`crates/shamir-client-node/` for a test directory, `__tests__`, or
similar; if there's a JS-side test harness for the native binding,
extend it with a test proving a real DB error surfaces `.code`/
`.retryable` correctly). If no test harness exists for this crate today,
note that explicitly in your report rather than inventing an elaborate
new one — a minimal proof (even a small standalone Node script the
brief's DoD requires you to run and paste output from) is acceptable
given this crate's existing test maturity, but say so explicitly.

## Out of scope

- Do NOT touch `crates/shamir-client` (the plain Rust client, no napi
  dependency) or `crates/shamir-client-ts` (the TS SDK) — this task is
  ONLY the native Node.js binding crate.
- Do NOT bump any OTHER dependency's version in this crate beyond what
  napi-rs 3.x's own `Cargo.toml`/lockfile genuinely requires as a
  transitive consequence (e.g. if napi 3.x requires a newer `tokio`
  minimum — verify, don't guess, and only bump what's actually forced).
- `crates/shamir-client-node` is excluded from the default workspace
  (`Cargo.toml`'s `members = ["crates/*"]` excludes it — MSVC-only,
  built separately per this repo's own `CLAUDE.md`). Do NOT add it back
  into the default workspace members list — that's a separate,
  unrelated infrastructure decision this task doesn't need to make.

## Definition of done

- `cargo build --release -p shamir-client-node` (run from
  `crates/shamir-client-node/` or via `-p`, whichever this repo's
  tooling supports for an out-of-workspace crate — check first) builds
  clean on napi-rs 3.x.
- A domain-level DB error demonstrably surfaces `.code`/`.retryable` to
  JS calling code (via whichever mechanism you land on) — prove this
  with a real, runnable script/test, not just a code-reads-plausible
  claim.
- `cargo fmt` / `cargo clippy` for this crate specifically (it's
  outside the default workspace, so `-p shamir-client-node` explicitly)
  clean, or only pre-existing issues you didn't introduce.
- Every existing call site updated consistently — no leftover
  `Error::from_reason(e.to_string())` for domain-level DB errors
  (infrastructure-failure call sites may keep the plain form if that's
  genuinely still appropriate per your design — explain which is
  which).

## Report

When done, produce a final summary (not a bare tool call): which of
the two typed-error mechanisms you landed on and why (cite the actual
compiler behavior you observed, not documentation), the exact napi/
napi-derive/napi-build version numbers you pinned, every file changed,
the full text of your proof (test/script) that `.code`/`.retryable`
actually reaches JS, and every discrepancy between this brief's
assumptions and what you actually found.
