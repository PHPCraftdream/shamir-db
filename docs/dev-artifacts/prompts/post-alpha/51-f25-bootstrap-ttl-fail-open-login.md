# Brief for F-25 (#818, P0) — bootstrap TTL fail-open in the login path

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context: a confirmed P0, found by the deepest of three post-wave reviews

F-3 (#793) fixed bootstrap-credential rotation's ordering (rotate → consume
→ delete, all best-effort/non-fatal, retryable) and the boot-time TTL sweep
in `server_launcher.rs` correctly expires/rotates outstanding bootstrap
tokens on startup. But a third independent review of Wave F
(`docs/dev-artifacts/research/2026-07-26-new-wave-release-review.md`, R2)
found — and the orchestrator has personally confirmed by reading the code —
that **the login path itself never checks token expiry at all.**

`crates/shamir-server/src/connection/handshake.rs`'s `run_handshake` has
**zero** calls to `ctx.meta.bootstrap_token_expired(...)` (confirmed via
`grep -n bootstrap_token_expired`). The only place that check exists is
`server_meta.rs:490`'s accessor itself and its boot-time-sweep caller. The
login path (~line 417 onward) only checks:

```rust
if ctx.meta.bootstrap_token_active()
    && ctx.meta.bootstrap_username().as_deref() == Some(username.as_str())
{
    match crate::bootstrap::rotate_bootstrap_credential_to_random(...).await {
        Ok(()) => { /* consume + delete file */ }
        Err(e) => {
            tracing::warn!(?e, "... token left active for retry");
            // execution CONTINUES — login still succeeds
        }
    }
}
```

— i.e. it tries to rotate on every successful login for the outstanding
bootstrap username, but **never asks whether the token's TTL has already
passed**, and a rotation failure is explicitly non-fatal (by design, so a
storage hiccup doesn't lock out a legitimate first-login). The consequence:
if the bootstrap token's TTL expires between issuance and the login
attempt, AND the boot-time sweep hasn't run yet (server hasn't restarted)
OR the sweep already tried and failed to rotate (storage error), **the
still-valid SCRAM credential authenticates successfully** — the TTL
security guarantee (bootstrap credentials should stop working after a
bounded window) does not actually hold at the login boundary, only at boot.

This is a real authentication-boundary security gap, not a hypothetical:
the SCRAM proof itself is genuinely cryptographically valid (the underlying
credential hasn't been rotated yet), so `hs.verify_proof(...)` legitimately
returns `Accepted` — nothing before this point has any reason to reject the
login.

## Design

**Fail-closed on expiry, indistinguishably from a wrong password.**

Add the expiry check **after** `hs.verify_proof(...)` returns
`ProofOutcome::Accepted` (so we only act once we know the credential is
genuinely correct — checking expiry any earlier would need duplicate logic
for both proof outcomes) but **before** the session is built/returned. If
`ctx.meta.bootstrap_username().as_deref() == Some(username.as_str())` AND
`ctx.meta.bootstrap_token_expired(now_ns)` (reuse the same `now_ns` already
computed at the top of `run_handshake` for the lockout pre-check — same
pattern F-3's own comment already establishes), the login must be
**rejected**, not accepted.

**Critical invariant: this rejection must be externally indistinguishable
from a wrong-password rejection** — same response wire shape, same
backoff/lockout accounting, same audit-log shape — so an attacker cannot
use response differences to learn "this username has an outstanding
bootstrap token past its TTL" (a targeting signal). The cleanest way to
guarantee this: do NOT write a parallel/separate rejection branch. Instead,
right where `outcome = hs.verify_proof(...)` is matched
(`ProofOutcome::Accepted(ok) => *ok` vs `ProofOutcome::Rejected => { ...
register_failure... return Err(HandshakeError::BadProof { backoff_ms })
}`), add the expiry check as an ADDITIONAL condition that, when true, makes
`Accepted` fall through into the EXACT SAME code the `Rejected` arm already
runs (register_failure, backoff computation, the `tracing::info!`/
`audit_emit` calls, the `HandshakeError::BadProof` return) — i.e. treat
"proof correct but bootstrap TTL expired" as if it were
`ProofOutcome::Rejected` for every purpose from that point forward. Read
the existing `match outcome { ... }` block fully before editing (~lines
343-390 based on the orchestrator's earlier read) and find the least
invasive way to route both cases through one shared rejection path (e.g. a
small local closure/helper, or restructuring the match to compute a
`let treat_as_rejected = matches!(outcome, ProofOutcome::Rejected) ||
(is_accepted_but_expired_bootstrap)` up front) — whichever keeps the diff
smallest and doesn't duplicate the backoff/audit logic.

**Rotation-on-login stays exactly as it is today** for the NON-expired
case — F-3's best-effort/non-fatal rotate → consume → delete flow is
correct and must not change. This task only adds a NEW, EARLIER gate for
the specific case where the token already passed its TTL; it does not
touch what happens after a successful (non-expired) login.

**Do not add a check anywhere before proof verification** — the review's
"before proof acceptance" framing describes the SECURITY OUTCOME (an
expired credential must never yield a session), not a literal requirement
to check before the crypto verification runs. Checking after
`ProofOutcome::Accepted` and folding into the SAME rejection path as
`Rejected` achieves the identical security property (no session issued)
with much less risk of introducing a NEW timing/response-shape side
channel by writing a second, subtly-different rejection branch.

## Tests

Add to whatever handshake/bootstrap integration test file already covers
F-3's rotate-on-login behavior (find it — likely
`crates/shamir-server/tests/` or `crates/shamir-server/src/tests/`, search
for existing tests referencing `rotate_bootstrap_credential_to_random` or
`bootstrap_token_expired`):

1. **Expired token + real login attempt = rejected.** Set up a server with
   an outstanding bootstrap token whose `bootstrap_token_expires_at_ns` is
   already in the past (construct this directly via the same
   `server_meta`/`PersistedBootstrap` plumbing the existing bootstrap tests
   use — do not sleep-wait for real time to pass). Attempt a real login
   with the still-valid-but-expired credential. Assert the login is
   rejected with the same error shape as a wrong-password attempt (not a
   distinct error code/message).
2. **Expired token + forced rotation failure + login = still rejected.**
   Combine with a forced storage failure during
   `rotate_bootstrap_credential_to_random` (if the existing test
   infrastructure has a fault-injection hook for this from F-3's own tests
   — reuse it) to confirm the login is STILL rejected even though rotation
   never got a chance to run — this is the exact gap being closed (today
   this scenario logs a warning and lets the login through).
3. **Non-expired token + login still rotates and succeeds** (regression
   guard — confirm this task didn't accidentally break F-3's happy path).
   After a successful rotation, a SECOND login attempt with the OLD
   (now-rotated-away) credential must also fail (this may already be
   covered by an existing F-3 test — confirm rather than duplicate).
4. Confirm lockout/backoff state after an expired-token rejection matches
   exactly what a wrong-password rejection would produce (same
   `register_failure` call, same backoff arithmetic) — a concrete assertion
   that the indistinguishability invariant holds, not just an eyeballed
   code read.

## Constraints

- Do NOT change F-3's rotate → consume → delete ordering or its
  best-effort/non-fatal semantics for the NON-expired case.
- Do NOT introduce a new, distinguishable error code/response shape for
  "expired bootstrap credential" — it MUST look identical to a bad-proof
  rejection from outside the process.
- Do NOT check expiry before proof verification completes — see Design
  section for why.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-server -- --check` and
  `cargo clippy -p shamir-server --all-targets -- -D warnings` must be
  clean.
- Surgical diff — no incidental refactors of `handshake.rs` beyond what
  this task needs.

## Verification the orchestrator will run

```
cargo fmt -p shamir-server -- --check
cargo clippy -p shamir-server --all-targets -- -D warnings
./scripts/test.sh -p shamir-server -- bootstrap
./scripts/test.sh -p shamir-server --full
```
