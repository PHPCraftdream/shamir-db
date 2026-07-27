# Brief for F-38 (#846, P0) — bootstrap TTL check must fail CLOSED on a metadata read error

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

A readonly review (`docs/dev-artifacts/research/2026-07-27-new-wave-readonly-review.md`,
finding P0-4) found a real fail-open gap in the bootstrap-TTL security
check F-25 (#818) added. **Read
`crates/shamir-server/src/server_meta.rs`'s `bootstrap_token_active`/
`bootstrap_username`/`bootstrap_token_expired` (~line 453-499) and
`crates/shamir-server/src/connection/handshake.rs`'s password-proof
handling (~line 355-435) in full first.**

`bootstrap_token_active`, `bootstrap_username`, and `bootstrap_token_expired`
each independently call `self.read_blob::<PersistedBootstrap>(KEY_BOOTSTRAP)`
and swallow any error via `.ok()`, collapsing a genuine storage error into
the exact same shape as "genuinely absent" (`false`/`None`/`false`). In
`handshake.rs`, AFTER a client's password proof has already been verified
correct, the fail-closed TTL check does:

```rust
if ctx.meta.bootstrap_token_active()
    && ctx.meta.bootstrap_username().as_deref() == Some(username.as_str())
    && ctx.meta.bootstrap_token_expired(now_ns)
{
    // ... reject with HandshakeError::BadProof ...
}
```

**Confirmed by reading the code**: if the underlying metadata read errors
(I/O failure, or `rmp_serde` decode failure on a corrupted blob) at this
exact moment, `bootstrap_token_active()` silently returns `false`, so the
`if` is skipped entirely — a session is granted for what may be an
EXPIRED, READABLE-UNDER-NORMAL-CONDITIONS bootstrap credential, purely
because the read happened to fail at that instant. The existing code
comment (line ~409-416) explicitly acknowledges this is "pre-existing
behavior this check does not change or worsen" — that framing is no
longer acceptable for a P0: a transient storage error must never silently
grant a bootstrap session.

## Scope: ONLY the security-gating check, not the housekeeping ones

There are THREE call sites reading these bootstrap getters — **only ONE
of them is a security gate; the other two are intentionally best-effort
housekeeping and must stay lenient**:

1. **`handshake.rs`'s fail-closed TTL check** (~line 398-434, the block
   quoted above) — **THIS is the one that must become fail-CLOSED on a
   read error.** It runs after a valid password proof and decides whether
   to grant a session; an error here must reject the login, not grant it.
2. **`handshake.rs`'s rotate/consume-on-login block** (~line 461-490,
   comment: *"Best-effort and non-fatal throughout — a failure here must
   NEVER abort an otherwise-successful login"*) — leave this lenient. A
   read/rotate failure here should log and skip (exactly as today), not
   block the login — the boot-time sweep is its backstop.
3. **`server_launcher.rs`'s boot-time TTL sweep** (~line 170-210, comment:
   *"Best-effort — an I/O failure ... is logged and does not fail
   boot"*) — leave this lenient too; it's not an auth gate.

Do not make (2) or (3) fail-closed — that would change intentional,
already-correct leniency into something that could fail server boot or
abort successful logins for unrelated storage hiccups. Only (1) is the
security-critical gate this task fixes.

## What to build

### 1. One fallible read, not three independent fail-open getters

Add a new method on the type that owns `read_blob` (the struct containing
`bootstrap_token_active`/etc — read the surrounding `impl` block to get
the exact type name), e.g.:

```rust
pub struct BootstrapTokenSnapshot {
    pub active: bool,
    pub username: Option<String>,
    pub expired: bool,
}

pub fn read_bootstrap_token_state(&self, now_ns: u64) -> Result<BootstrapTokenSnapshot, MetaError> {
    // read PersistedBootstrap ONCE via read_blob(KEY_BOOTSTRAP)?  (propagate
    // the error, do NOT .ok() it away), then derive all three fields from
    // that single read — same derivation logic the three existing getters
    // each currently duplicate.
}
```

Do NOT reuse or entangle this with `shamir_connect::server::bootstrap::BootstrapState`
(imported at the top of `server_meta.rs` for the UNRELATED, explicitly
"not wired into any live dispatch path" wire-bootstrap-flow feature — a
different, heavier, mutex-based type for a dead code path; check its
module doc comment yourself to confirm this before deciding, don't just
take this brief's word for it). This new type/method is local to
`server_meta.rs`'s actual live-server bootstrap bookkeeping.

Keep the existing three getters (`bootstrap_token_active`/
`bootstrap_username`/`bootstrap_token_expired`) as-is — do not remove
them, other call sites (the two lenient ones above, plus existing tests)
still use them and should keep their current fail-open behavior
unchanged.

### 2. Wire the fail-closed read into handshake.rs's ONE security gate

Replace the 3-getter `if` condition (site 1 above) with a single call to
`read_bootstrap_token_state(now_ns)`:

```rust
let bootstrap_state = ctx.meta.read_bootstrap_token_state(now_ns);
match bootstrap_state {
    Ok(state) if state.active
        && state.username.as_deref() == Some(username.as_str())
        && state.expired =>
    {
        // ... existing reject logic (same HandshakeError::BadProof, same
        // register_failure/audit_emit pattern) ...
    }
    Ok(_) => { /* not an outstanding-expired-bootstrap-for-this-user case — proceed */ }
    Err(e) => {
        // FAIL CLOSED: a metadata read error after a valid password proof
        // must reject, using the SAME failure shape as a bad password
        // (HandshakeError::BadProof, same register_failure/audit_emit
        // call) so this is indistinguishable from a wrong password to an
        // outside observer -- no oracle telling an attacker "the password
        // was right, only a storage error happened."
        //
        // (log the error with tracing::warn! or similar for operator
        // visibility, then reject)
    }
}
```

Preserve EVERY existing property of this block exactly: same
`register_failure`/backoff computation, same `audit_emit` call shape
(check what `reason` string the existing reject path passes and either
reuse it or add a clearly-distinct one for the new metadata-error case —
your call, but state which you picked and why), same "runs BEFORE
`reset_on_success`" ordering, same constant-time/no-user-enumeration
behavior (do not let a metadata-error case return distinguishably faster/
slower or with a different error code than the existing expired-token
rejection).

## Tests — MANDATORY, in the same commit

**Fault-injection test, in `crates/shamir-server`'s existing test module for
this area** (check `tests/bootstrap_tests.rs`/`tests/connection_tests.rs`/
`tests/server_meta_tests.rs` for the right home — this is exercising the
handshake path, so `connection_tests.rs` is likely correct, but confirm):

- Simulate a metadata read failure at the exact moment `read_bootstrap_token_state`
  is called during a login attempt with a VALID password proof. The
  simplest realistic injection: after normally provisioning a bootstrap
  token (so `KEY_BOOTSTRAP` has a real entry), directly corrupt the raw
  bytes stored under `KEY_BOOTSTRAP` in the underlying store (bypassing
  `ServerMetaStore::put`, writing garbage bytes directly via whatever raw
  handle the underlying store exposes in tests) so the next `read_blob`
  call hits `rmp_serde::from_slice`'s decode-error branch — a real,
  reproducible read failure, not a mock. Check how existing tests in this
  crate construct/access the raw store to do this (or find another
  injection seam this codebase already uses for storage-error testing) —
  state which mechanism you used.
- Assert: a login with a VALID password proof against a
  valid-bootstrap-shaped-but-now-corrupted metadata state is REJECTED
  (never grants a session) — this is the core proof.
- Assert the rejection has the SAME shape/error code as the existing
  expired-token rejection (no distinguishable oracle).
- A regression test: the EXISTING non-error case (readable, genuinely
  expired token) still rejects exactly as F-25 already proved (find and
  re-confirm the existing F-25 test still passes — don't skip re-running
  it).
- A regression test: the EXISTING non-error, non-expired case (readable,
  active, NOT expired) still grants a session normally.

## Constraints

- Do NOT touch the two lenient (non-security-gate) call sites — leave
  their existing fail-open behavior exactly as-is.
- Do NOT remove the three existing granular getters — other callers/tests
  still need them.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-server -- --check` and
  `cargo clippy -p shamir-server --all-targets -- -D warnings` must be
  clean.

## Verification the orchestrator will run

```
cargo fmt -p shamir-server -- --check
cargo clippy -p shamir-server --all-targets -- -D warnings
./scripts/test.sh -p shamir-server -- bootstrap
./scripts/test.sh -p shamir-server --full
```
