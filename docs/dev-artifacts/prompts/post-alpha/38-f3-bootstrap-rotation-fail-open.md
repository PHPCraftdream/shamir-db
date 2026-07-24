# Brief for #793 (F-3) — bootstrap credential rotation: fix fail-open ordering

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## The bug

Two call sites perform the SAME three-step "consume an outstanding
bootstrap token" sequence, and both order it unsafely:

1. `crates/shamir-server/src/connection/handshake.rs` (around lines
   410-436) — on the first successful SCRAM login using the bootstrap
   token's derived credential.
2. `crates/shamir-server/src/server/server_launcher.rs` (around lines
   180-213) — the boot-time TTL sweep for an expired-and-never-used token.

Current order at BOTH sites:

1. Delete the on-disk token file (`std::fs::remove_file`).
2. `ctx.meta.consume_bootstrap_token()` (or `meta.consume_bootstrap_token()`
   in the sweep) — clears the persisted bootstrap-token metadata row
   (`bootstrap_token_hash`/`bootstrap_username`/`bootstrap_token_path`), so
   `ServerMetaStore::bootstrap_token_active()` becomes `false` from this
   point on.
3. `crate::bootstrap::rotate_bootstrap_credential_to_random(...)` — writes
   a NEW random SCRAM credential to the user directory
   (`FjallUserDirectory::update_credentials`), replacing whatever
   credential the bootstrap token was derived from.

Step 3's error is only logged (`tracing::warn!`), never propagated — by
design, per `rotate_bootstrap_credential_to_random`'s own doc comment
("Best-effort and non-fatal... a rotation failure here must NOT abort an
otherwise-successful login or fail boot"). That non-fatal contract is
correct and must be PRESERVED. The bug is the ORDER: if step 3 fails
AFTER step 2 already ran, `bootstrap_token_active()` is now `false` (the
token is marked "consumed"), so:

- The boot-time TTL sweep will never look at this token/username again
  (it only acts on an `active` row).
- The handshake path's own "is there still an active bootstrap token for
  this username" check (`ctx.meta.bootstrap_token_active() &&
  ctx.meta.bootstrap_username().as_deref() == Some(username)`) is now
  false too, so a SUBSEQUENT login attempt won't even try to rotate again.
- Meanwhile the user directory's SCRAM credential was NEVER updated — it's
  still the one derived from the original bootstrap token/password. That
  password keeps authenticating the account indefinitely, silently
  defeating the entire "one-time bootstrap token" guarantee CR-A6
  originally fixed (see `rotate_bootstrap_credential_to_random`'s own doc
  comment referencing that bug).

## The fix

Reorder to: **(1) successfully rotate the credential FIRST (durable write
to the user directory) → only then (2) consume the metadata row → then (3)
delete the token file.** If step 1 fails, do NOT run steps 2/3 — leave the
token metadata row `active` (and the file in place) so a later event (the
NEXT login attempt with the same still-valid credential, or the next
server restart's TTL sweep) gets another chance to rotate. This makes the
whole operation naturally retryable without any new retry/flag machinery:
as long as `bootstrap_token_active()` stays `true` for this username, the
SAME code path runs again on the next opportunity.

Apply this reordering at BOTH call sites (`handshake.rs` and
`server_launcher.rs`) — they currently mirror each other's ordering
(handshake.rs's comment literally says "mirroring the sweep's ordering"),
so keep them mirrored, just in the NEW correct order.

### Exact shape at each site

`handshake.rs` (~line 410-436), inside the
`if ctx.meta.bootstrap_token_active() && ctx.meta.bootstrap_username().as_deref() == Some(username.as_str())`
block:

```rust
// NEW order: rotate first (durable credential write), consume metadata
// only on rotate success, delete the token file last (pure cleanup, not
// security-relevant to the consumed/active state).
match crate::bootstrap::rotate_bootstrap_credential_to_random(
    &ctx.user_dir,
    username.as_str(),
    kdf,
    now_ns,
)
.await
{
    Ok(()) => {
        if let Err(e) = ctx.meta.consume_bootstrap_token() {
            tracing::warn!(?e, "bootstrap: failed to consume token record on login");
        }
        if let Some(path) = ctx.meta.bootstrap_token_path() {
            if let Err(e) = std::fs::remove_file(&path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(?path, ?e, "bootstrap: failed to delete token file on login");
                }
            }
        }
    }
    Err(e) => {
        tracing::warn!(
            ?e,
            "bootstrap: failed to rotate SCRAM credential on login -- token left active for retry"
        );
    }
}
```

Note `bootstrap_token_path()` must still be read from `ctx.meta` — do this
AFTER a successful rotate but BEFORE `consume_bootstrap_token()` clears the
row (same "read username/path before consuming" constraint the ORIGINAL
code already respected — don't lose that ordering constraint while fixing
the other one). Adjust the sketch above if `bootstrap_token_path()`
actually needs to be read earlier for that reason — verify against the
current code rather than assuming the sketch's exact statement order is
final.

`server_launcher.rs` (~line 180-213), inside the
`if meta.bootstrap_token_expired(now_ns)` block: same reordering. Note this
site reads `expired_username` via `meta.bootstrap_username()` BEFORE doing
anything else (needed for the rotate call regardless of order), so that
read is unaffected — only the consume/rotate/delete-file ordering among
steps 2/3 (using this brief's step numbering above) changes. If rotation
fails here (or if there's no `expired_username` to rotate — the existing
`if let Some(name) = expired_username` guard), do NOT consume the token
metadata row — leave it for a future boot's sweep to retry.

## Constraints

- Preserve the "best-effort, non-fatal" contract exactly: neither call
  site may abort an otherwise-successful login, nor fail server boot, due
  to a rotation failure. Only the ORDER changes, not the fatality.
- Do not change `rotate_bootstrap_credential_to_random`'s signature,
  its documented "residual race" behavior (two near-simultaneous logins
  racing past SCRAM verification before either rotation completes — that
  residual is explicitly accepted, do not attempt to fix it here), or
  `consume_bootstrap_token`'s own semantics.
- Do not touch the SCRAM proof-verification logic above this block, the
  lockout/backoff machinery, or anything unrelated to this three-step
  sequence.

## Tests

Add regression tests (find the existing test file(s) covering
`handshake.rs`'s bootstrap-login path and `server_launcher.rs`'s TTL
sweep first — check `crates/shamir-server/src/tests/` and any
`connection`/`bootstrap`-scoped test modules; follow this repo's existing
test-organization convention, one `tests/` dir per module, no inline
`#[cfg(test)] mod tests`):

1. **Rotation failure leaves the token active and retryable (handshake
   path).** Simulate `rotate_bootstrap_credential_to_random` failing (may
   need a way to force `update_credentials` to fail — check
   `FjallUserDirectory`'s test doubles/fixtures for an existing failure-
   injection mechanism before inventing a new one) during a bootstrap
   login. Assert: `ctx.meta.bootstrap_token_active()` is STILL `true`
   afterward (not consumed), and the token file is NOT deleted. A
   subsequent retry (rotation now succeeding) then correctly
   consumes+deletes.
2. **Same scenario for the boot-time TTL sweep** in `server_launcher.rs`
   — rotation failure during the sweep must leave the expired token's
   metadata row active for the next boot's sweep to retry.
3. **Happy path unchanged**: a successful rotation still consumes the
   token and deletes the file, exactly as before (don't just add new
   tests — check whether existing happy-path tests for this flow already
   exist and still pass unmodified; if any assert on the OLD ordering
   specifically in a way that's now stale, update them, but don't weaken
   the assertion).

## Verification the orchestrator will run

```
cargo fmt -p shamir-server -- --check
cargo clippy -p shamir-server --all-targets -- -D warnings
./scripts/test.sh -p shamir-server
```
