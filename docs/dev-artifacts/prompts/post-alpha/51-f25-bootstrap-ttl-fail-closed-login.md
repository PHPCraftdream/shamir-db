# Brief for F-25 (#818, P0) — bootstrap TTL must be fail-closed in the login path

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

F-3 (#793, earlier "Wave F" hardening) reordered the boot-time bootstrap-
token TTL sweep (`crates/shamir-server/src/server/server_launcher.rs`) to
rotate → consume → delete, and made the login-path rotation
(`crates/shamir-server/src/connection/handshake.rs:401-456`) follow the same
ordering, explicitly designed to be retryable: "Best-effort and non-fatal
throughout — a failure here must NEVER abort an otherwise-successful login;
the boot-time TTL sweep is the backstop for anything missed here."

A deeper independent static-audit review (found this P0 — NOT caught by two
earlier agent reviews; see
`docs/dev-artifacts/research/2026-07-26-new-wave-release-review.md`, finding
R2, and the cross-reference in
`docs/dev-artifacts/research/2026-07-26-wave-f-consolidated-synthesis/SYNTHESIS.md`,
item C3) found — and the orchestrator personally verified by reading the
code — that **the login path never checks whether the outstanding bootstrap
token has actually expired**. It only checks:

```rust
if ctx.meta.bootstrap_token_active()
    && ctx.meta.bootstrap_username().as_deref() == Some(username.as_str())
{
    match crate::bootstrap::rotate_bootstrap_credential_to_random(...).await {
        Ok(()) => { /* consume + delete */ }
        Err(e) => {
            tracing::warn!(?e, "... token left active for retry");
        }
    }
}
```

`ServerMeta::bootstrap_token_expired(now_ns)`
(`crates/shamir-server/src/server_meta.rs:490-499`) already exists (used by
the boot-time sweep) but is **never called anywhere in
`handshake.rs`**. Consequence: if the bootstrap token's TTL has expired but
the boot-time sweep hasn't run yet (server started recently, or the sweep's
own rotation attempt failed on a transient storage error and left the row
active for retry — exactly the retry design F-3 intended), the SCRAM proof
still verifies successfully (the account's credential hasn't changed yet),
the login path attempts rotation AGAIN, and if THAT also fails (same
storage outage, a race, etc.) — **the login still succeeds and a session is
granted**, because the rotation failure is explicitly non-fatal to the
login. The TTL security guarantee (a bootstrap credential must stop working
after its expiry) does not hold at that moment.

## What to fix

Before a session is granted for a login against the outstanding bootstrap
username, the code must check whether that bootstrap token has expired, and
if so, **reject the login with the SAME failure shape as a wrong-password
rejection** — not a distinguishable error, to avoid creating an oracle that
tells an attacker "the password was right, only the timing was wrong."

### Exact insertion point and mechanism

In `run_handshake` (`handshake.rs`), between where `ProofOutcome::Accepted`
is matched (the `let auth_ok: AuthOkView = match outcome { ... }` block,
~line 349) and the bootstrap-rotation block (~line 401), insert a check:

```rust
if ctx.meta.bootstrap_token_active()
    && ctx.meta.bootstrap_username().as_deref() == Some(username.as_str())
    && ctx.meta.bootstrap_token_expired(now_ns)
{
    // fail-closed: same failure shape as a wrong-proof rejection, so an
    // attacker cannot distinguish "correct password, expired token" from
    // "wrong password" -- see the file's own oracle-avoidance discipline
    // around ProofOutcome::Rejected just above.
    let backoff_ms = match ctx.lockout.register_failure(pair, now_ns) {
        FailureOutcome::Backoff { delay_ms } => delay_ms,
        FailureOutcome::LockedOut => BACKOFF_CAP_MS,
    };
    tracing::info!(user_hash = %hex::encode(uhash), "auth_failed: bootstrap token expired");
    audit_emit(ctx, "auth_failed", username.as_str(), subnet, None, "bootstrap_token_expired");
    return Err(HandshakeError::BadProof { backoff_ms });
}
```

Read the surrounding code carefully before inserting — confirm the exact
variable names available at that point (`pair`, `now_ns`, `uhash`, `subnet`
are all already in scope earlier in the function for the
`ProofOutcome::Rejected` arm; reuse them, don't recompute). Do **NOT** call
`ctx.lockout.reset_on_success(pair)` before this new check — that call must
stay AFTER this check (only reached on a truly successful, non-expired
login), otherwise a lockout counter would be reset for a login that's about
to be rejected anyway.

This check must run BEFORE the existing rotation attempt block, and BEFORE
building/sending `AuthOkView`/session creation (step 8). It should NOT touch
or depend on the rotation attempt at all — the rotation block can stay
exactly as-is afterward for the non-expired case (still best-effort,
non-fatal, retryable, per F-3's existing design).

Note the asymmetry this creates on purpose: an ACTIVE, non-expired
bootstrap token still logs in fine even if rotation fails afterward
(F-3's existing retryable design, unchanged) — only an EXPIRED token is
now rejected outright, regardless of whether rotation would succeed or
fail.

### Metadata unavailability

If `ctx.meta` itself is somehow unavailable/erroring when checking
`bootstrap_token_active()`/`bootstrap_token_expired()` — check how these
methods currently handle a read failure (they appear to return `false`/
`None` via `.ok().flatten()` chains rather than propagating errors, per
`server_meta.rs`). Confirm this fail-open-on-storage-error behavior for the
EXPIRY check specifically doesn't reopen the same class of gap (e.g. if
metadata is unreadable, `bootstrap_token_expired` currently returns `false`
via `.is_some_and(...)` on `None`, meaning "not expired" — this is
consistent with the EXISTING `bootstrap_token_active()` semantics used by
the surrounding `if`, so no NEW fail-open is introduced beyond what already
exists for detecting an outstanding token in the first place; document this
in a code comment rather than trying to redesign `ServerMeta`'s read
semantics, which is out of scope here).

## Tests

Add to `crates/shamir-server/tests/` (find the existing bootstrap-lifecycle
integration test file — likely near where F-3's own two-boot test lives, or
alongside `migration_api_gating.rs`/similar boot-driven integration tests;
search for existing bootstrap e2e tests before creating a new file) a test
that:

1. Boots a server with an outstanding bootstrap token.
2. Forces the token's `bootstrap_token_expires_at_ns` into the past (either
   via a test-only `ServerMeta` setter if one exists, or by waiting past a
   very short configured TTL if the test harness supports that — check
   existing test patterns in `server_meta_tests.rs`'s
   `bootstrap_token_expired_false_before_true_at_and_after_expiry` for how
   expiry is exercised in unit tests, and reuse the same approach at the
   integration level if feasible).
3. Attempts a real login with the correct (still-valid, not-yet-rotated)
   bootstrap password.
4. Asserts the login is REJECTED (same shape as a bad-proof rejection, not
   a distinguishable error).
5. A second test: after a NORMAL (non-expired) successful rotation, assert
   the OLD bootstrap token/password no longer works (regression guard that
   this fix didn't break the existing F-3 rotation-on-success behavior).

If forcing real wall-clock expiry in an integration test is impractical,
a focused unit test at the `ServerMeta`/`handshake.rs` boundary (mocking or
directly constructing the expired-token state) is an acceptable substitute
— use your judgment on the cheapest test that genuinely exercises the new
check, and explain the choice in your final report.

## Constraints

- Do NOT change `bootstrap_token_expired`, `bootstrap_token_active`, or any
  other existing `ServerMeta` method's signature or semantics.
- Do NOT change the boot-time TTL sweep in `server_launcher.rs` — it's
  already correct (F-3) and out of scope here.
- Do NOT change the rotation-on-success mechanics
  (`rotate_bootstrap_credential_to_random`) — only gate WHETHER the login
  is allowed to proceed at all when the token is expired.
- Preserve every existing oracle-avoidance property already documented
  in this file (constant-time padding, no user-existence leak via timing
  or response shape) — the new rejection path must be indistinguishable
  from a normal bad-proof rejection in shape and (as much as reasonably
  achievable) timing.
- `cargo fmt -p shamir-server` and
  `cargo clippy -p shamir-server --all-targets -- -D warnings` must be
  clean.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.

## Verification the orchestrator will run

```
cargo fmt -p shamir-server -- --check
cargo clippy -p shamir-server --all-targets -- -D warnings
./scripts/test.sh -p shamir-server -- bootstrap
./scripts/test.sh -p shamir-server --full
```
