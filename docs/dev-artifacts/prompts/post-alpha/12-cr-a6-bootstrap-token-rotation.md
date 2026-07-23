# Brief: CR-A6 — bootstrap token truly one-time (#765)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run ALL test/build commands in the FOREGROUND. Do
not background `cargo`/`./scripts/test.sh` invocations.

## Problem — SECURITY, verified against the current tree 2026-07-23

`crates/shamir-server/src/bootstrap.rs`'s random-token mode generates a
32-byte token and stores it as a NORMAL SCRAM password for the bootstrap
account (via `derive_scram_record`, same helper `CreateScramUser` uses).
On first successful login (`crates/shamir-server/src/connection/handshake.rs`,
~lines 399-415) — and on the 24h TTL boot-time sweep for an unused token
(`crates/shamir-server/src/server/server_launcher.rs`, ~lines 160-183) —
only the PLAINTEXT TOKEN FILE (`std::fs::remove_file`) and the server-meta
bookkeeping row (`ctx.meta.consume_bootstrap_token()`) are cleared. **The
account's SCRAM `stored_key`/`server_key` are never rotated** — the same
token keeps working as the account's password indefinitely, contradicting
`bootstrap.rs`'s own doc comment ("the token now auto-deletes itself...
consumes the token record on the FIRST successful login") and the RI-9
CHANGELOG entry, both of which overclaim.

## Fix — atomically rotate the SCRAM credential to something nobody knows

### The primitive already exists — reuse it

`crates/shamir-connect/src/server/admin.rs`'s `UserDirectory` trait already
has `update_credentials(&self, username, new_salt, new_stored_key,
new_server_key, new_kdf_params, now_ns) -> Result<bool>` — this is the
EXACT mechanism the `changePassword` ceremony (spec §12.5) already uses to
rotate a user's SCRAM record. `crates/shamir-server/src/db_handler/admin.rs`'s
`derive_scram_record(password: String, kdf: KdfParams) -> Result<UserRecord, String>`
is the shared Argon2id-derivation helper (already used by `CreateScramUser`
and `UserAdminPort::create_user`) — reuse it here too, rather than
hand-rolling SCRAM key material.

**Rotation recipe** (same at both call sites — factor into one shared
helper function, e.g. `rotate_bootstrap_credential_to_random`, placed in
`bootstrap.rs` or a new small module, taking `&dyn UserDirectory` (or the
concrete `FjallUserDirectory` type, check what the two call sites actually
have in scope) + `username` + `kdf` + `now_ns`):

1. Generate a random "password" — e.g. `random_array::<32>()` (already
   imported in `bootstrap.rs` from `shamir_connect::common::crypto`)
   base64/hex-encoded into a `String` — **never log it, never persist it
   anywhere, never return it to any caller.** It exists only transiently to
   feed `derive_scram_record`.
2. Call `derive_scram_record(random_password, kdf).await` to get a fresh
   `UserRecord { salt, stored_key, server_key, .. }`.
3. Call `user_dir.update_credentials(username, record.salt, record.stored_key, record.server_key, kdf, now_ns)`.
4. Drop the random password immediately after step 2 (it's a local
   `String`/`Zeroizing` — the SAME "wipe ASAP" pattern this codebase
   already applies elsewhere, e.g. `derive_scram_record`'s own `pw_buf:
   Zeroizing<Vec<u8>>`).
5. Best-effort, non-fatal, exactly like the existing token-file-delete and
   `consume_bootstrap_token()` calls right next to it — a rotation failure
   must NOT abort an otherwise-successful login (log a `tracing::warn!`
   and move on; the TTL sweep or a manual operator rotation remains the
   backstop for a rotation that failed here).

### Wire it into BOTH call sites

- **`handshake.rs`'s first-successful-login path** (~lines 399-415):
  right where the token FILE is already deleted and
  `ctx.meta.consume_bootstrap_token()` is already called — add the
  rotation call there too, using the SAME `username`/`kdf`/`now_ns` already
  in scope (check what KDF params are available at this point — the
  original bootstrap KDF, or does this handler have access to a per-user
  KDF record already on file? Use whatever's already correct/available,
  don't invent a new KDF source).
- **`server_launcher.rs`'s TTL sweep** (~lines 160-183): this block
  currently does NOT extract the bootstrap username before consuming the
  token record — add a `meta.bootstrap_username()` call BEFORE
  `meta.consume_bootstrap_token()` (order matters: the username must still
  be readable) and rotate the same way, using `kdf_for_bootstrap` (already
  constructed a few lines below at ~line 187 for the bootstrap step itself
  — you may need to hoist that construction earlier, or build an
  equivalent `KdfParams` inline for the sweep).

### Race consideration (note, don't over-engineer)

Two near-simultaneous logins with the same token could both pass SCRAM
verification before either rotation completes (SCRAM verification reads
the CURRENT stored_key at proof-check time, which is still valid until
rotation lands). This is an acceptable, documented residual — the
important invariant is "the token stops working AFTER this point," not
"exactly one concurrent use ever succeeds." Do not add distributed locking
for this; a one-line doc comment noting the residual is sufficient.

## Docs — fix the overclaim

- `crates/shamir-server/src/bootstrap.rs`'s module doc comment (the
  "Random-token mode" paragraph, ~lines 16-26): currently says "the server
  ... removes the file and consumes the token record on the FIRST
  successful login... via a 24h TTL boot-time sweep" — extend this to
  accurately describe that the SCRAM credential itself is now also rotated
  to a random, permanently-unknown value at both of those points (not just
  file/metadata cleanup).
- `CHANGELOG.md`'s `[Unreleased]` RI-9 bullet (search for "auto-deletes
  itself" / "bootstrap-token lifecycle") — reword to describe the ACTUAL
  new behavior accurately: the token stops authenticating (credential
  rotated), not merely "the file disappears."

## Tests (TDD — write failing tests first)

Mirror whatever existing bootstrap/handshake integration test harness this
codebase already has (check `crates/shamir-server/tests/` for an existing
bootstrap-token e2e test — RI-9 landed one; extend it rather than
inventing a new fixture):

- **First login with the token succeeds** (already covered, keep green).
- **A SECOND login attempt with the SAME token, after the first login
  already completed, FAILS** (`BadProof` / equivalent auth failure) — this
  is the core new guarantee CR-A6 adds; the CURRENT test suite almost
  certainly does NOT have this test (it would have caught the bug) — write
  it as a genuinely failing test first, confirm it fails against the
  UN-fixed code, then make it pass.
- **`changePassword` still works** from the session opened by the first
  (successful) token login — the rotation must not somehow interfere with
  the normal changePassword ceremony that follows.
- **TTL sweep rotates an unused token's credential too** — boot with a
  token whose TTL has already elapsed (mirror however the existing TTL
  sweep test simulates an expired token, e.g. via a backdated `now_ns` or
  a test-only TTL override), then attempt a login with that (now-expired
  AND rotated) token and confirm it fails.

## Gate

```
cargo fmt -p shamir-server -- --check
cargo clippy -p shamir-server --all-targets -- -D warnings
./scripts/test.sh -p shamir-server --full
```

All must pass before returning. Stay inside `shamir-server`
(`bootstrap.rs`, `connection/handshake.rs`, `server/server_launcher.rs`,
`CHANGELOG.md`, tests).
