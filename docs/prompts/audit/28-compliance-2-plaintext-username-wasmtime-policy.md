Task: MEDIUM compliance — stop logging plaintext usernames on the
general `tracing::info!` channel during authentication failures (PII +
user-enumeration signal), and document a wasmtime security-advisory
tracking policy (audit findings C3 + C5,
`docs/audits/2026-07-06-security-compliance-supplychain.md`).

## Where — C3 (plaintext username in auth logs)

- `crates/shamir-server/src/connection/handshake.rs`:
  - Line ~216: `tracing::info!(user = %username.as_str(), "locked_out
    at auth_init");` — logs the RAW username on the general `info`
    channel when a lockout pre-check rejects the attempt.
  - Line ~377: `tracing::info!(user = %username.as_str(), "auth_failed:
    bad proof");` — same pattern for a bad-proof rejection.
  - BOTH sites immediately follow with an `audit_emit(ctx, "auth_failed"
    /"locked_out", username.as_str(), subnet, None, ...)` call — this
    is the PROTECTED, HMAC-backed audit chain
    (`crates/shamir-connect/src/server/audit_chain.rs`) where the full
    plaintext identity is APPROPRIATE (it's the record of record for
    security investigation, not a general-purpose log stream).
  - A `uhash` value (`username_hash(&ctx.secrets.lockout_secret,
    username.as_bytes())`, ~line 213) already exists in scope at BOTH
    call sites — this is the exact "hash/truncated identifier" the
    audit's fix sketch asks for, already computed for lockout-keying
    purposes. No new hashing logic needs to be introduced.

## Why this is MEDIUM

Two problems with logging the raw username on the general `info`
channel:
1. **PII exposure**: for a database product, `tracing::info!` output
   typically flows to general-purpose log aggregation (not the
   security-hardened audit chain) — usernames (often email addresses
   or real identifiers) sitting in plaintext in general logs is a
   GDPR Art.32/SOC2-relevant PII-handling gap.
2. **User-enumeration / credential-stuffing signal**: an attacker (or
   anyone with read access to general logs — a broader population than
   those with audit-chain access) can aggregate `auth_failed`/
   `locked_out` events BY USERNAME to determine which usernames exist
   in the system (even without ever succeeding at auth), aiding
   credential-stuffing target selection.

## Fix — C3

In BOTH `tracing::info!` call sites (line ~216 and ~377), replace
`user = %username.as_str()` with `user_hash = %uhash` (or an
equivalently-named field — match whatever naming convention makes it
clear in the log output that this is a hash, not the plaintext
identity; check if `PairKey`/`uhash`'s type has a natural
`Display`/`Debug` impl suitable for a tracing field, or format it as
hex/base64 if needed). The subsequent `audit_emit(...)` calls on both
sites are UNCHANGED — they correctly continue to carry the full
plaintext username into the protected audit chain, which is the
intended "full identity only in the protected audit log" split the
audit's fix asks for.

Do NOT touch `audit_emit`'s signature or the audit chain itself — this
fix is scoped to the two `tracing::info!` call sites only. Do NOT
change the log level (`info` stays `info`) or the log message text
itself beyond the field substitution.

Grep the REST of the codebase for other `tracing::{info,warn,error}!`
call sites that log a raw `username`/`user` field in an
auth-failure-adjacent context (search for `username.as_str()` /
`user = %` patterns near auth/login/session code) to confirm these are
the ONLY two sites, or find others sharing the same issue — fix any
genuinely-analogous site you find (same class: plaintext username on
a general log channel during/around an auth failure), but do NOT
touch success-path logging or non-auth-related username logging (that
would be scope creep beyond this specific audit finding).

## Where — C5 (wasmtime advisory tracking policy)

This is a POLICY/PROCESS fix, not a code fix. Per the audit: `wasmtime
45.0.0` (the entire `wasmtime-internal-*` cluster) is the trusted
execution boundary for untrusted guest WASM code. No CRITICAL open
RUSTSEC advisories were found against 45.0.0 at audit time, but there
is currently NO PROCESS for tracking wasmtime-specific security
advisories SEPARATELY from the general dependency-advisory flow (which
task #483 just added via `cargo deny`/`cargo audit` — those cover
RUSTSEC broadly, but wasmtime advisories are also announced through the
Bytecode Alliance's own channel, which may surface issues before/
outside the RUSTSEC pipeline).

## Fix — C5

1. Add a short, explicit note to `SECURITY.md` (created by task #483,
   already committed — this is an ADDITIVE edit, read the current file
   first) under an appropriate section (likely the existing "Supply-
   chain posture" section, or a new small subsection) stating: (a)
   `wasmtime` is treated as a priority-upgrade dependency given its
   role as the untrusted-code execution boundary, (b) maintainers
   should watch the Bytecode Alliance's security-announce channel
   (link: `https://github.com/bytecodealliance/wasmtime/security` or
   whatever the actual current official channel is — check wasmtime's
   own repo/docs for the correct current link rather than guessing;
   if genuinely unable to verify the current canonical announcement
   channel URL, state that as a placeholder needing confirmation
   rather than inventing one), and (c) `wasmtime` bumps should not be
   deferred through the standard dependency-cooldown period the same
   way other, lower-trust-boundary dependencies are (cross-reference
   the existing cooldown policy mentioned in SECURITY.md's "Supply-
   chain posture" section from task #483).
2. Optionally (use judgment, keep this proportionate — a single doc
   note may be sufficient for a MEDIUM finding): check if there's an
   existing "audit checklist" doc anywhere in the repo (search
   `docs/` for anything resembling a periodic-audit or dependency-
   review checklist) where a "wasmtime advisories checked" line item
   could be added; if no such checklist exists, do NOT invent a new
   separate checklist document from scratch — the `SECURITY.md`
   addition above is sufficient to close this MEDIUM finding without
   over-engineering process documentation nobody asked for.

## Verification requirement (no TDD needed for either half)

**C3 verification:**
1. Show the before/after diff for both `tracing::info!` call sites.
2. Write (or confirm existing coverage in) a test that exercises the
   `locked_out`/`bad_proof` auth-failure paths and asserts the log
   output does NOT contain the plaintext username — check
   `crates/shamir-server/src/connection/` test modules (e.g.
   `handshake_tests.rs` or similar) for existing test infrastructure
   that captures `tracing` output (a test subscriber/layer), and add
   an assertion there if such infrastructure exists; if capturing
   tracing output in tests is not already set up in this codebase and
   would require significant new test-harness work, a SIMPLER
   acceptable proof is: read the code change and confirm by inspection
   that `username.as_str()` no longer appears in either `info!` macro
   call — state clearly in your report which verification approach you
   used and why.
3. Confirm the `audit_emit` calls (which SHOULD still carry the full
   plaintext username) are unchanged.

**C5 verification:** show the `SECURITY.md` diff; no test needed (docs
only).

## Test scope command

```
./scripts/test.sh -p shamir-server
```

## Gate (must be clean before finishing)

```
cargo fmt -p shamir-server -- --check
cargo clippy -p shamir-server --all-targets -- -D warnings
```

If clippy flags PRE-EXISTING lints in code you did not touch, do not
fix them here — note them in your final report instead.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

When done, report exactly:
- The exact before/after for both C3 call sites (and any additional
  analogous sites found and fixed, with justification for why they
  qualify).
- Confirmation `audit_emit` calls are unchanged and still carry the
  full plaintext username to the protected audit chain.
- The verification approach used for C3 (log-capture test, or
  by-inspection confirmation) and its result.
- The exact `SECURITY.md` diff for C5.
- Full test/gate results (exact commands + pass/fail).
