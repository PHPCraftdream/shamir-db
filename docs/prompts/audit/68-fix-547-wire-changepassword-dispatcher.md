Task #547 — wire the already-implemented `changePassword` flow
(`shamir-connect`'s `changepw.rs`) into the live server's request
dispatcher (`shamir-server`), so a user can self-service revoke a
stolen/portable resumption ticket without operator involvement.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## What already exists — do NOT re-implement

`crates/shamir-connect/src/server/changepw.rs` (confirmed at the time of
this brief — re-verify, code may have shifted):

- `start_change_password_challenge(session, user_salt, user_kdf_params,
  client_nonce_cp, now_ns) -> ChangePwChallengeView` — issues a fresh
  `server_nonce_cp`, stores `PendingChangePwChallenge` on the session
  (single-in-flight, a second call overwrites the first).
- `verify_change_password_request_with_sid(session, session_id,
  user_salt, user_stored_key, user_kdf_params, request: &ChangePwRequest,
  current_kdf_params, now_ns) -> Result<ChangePwApply>` — pops the pending
  challenge atomically (single-use), SCRAM-verifies `client_proof_old`
  against the OLD credentials in constant time, and on success returns
  `ChangePwApply { salt, stored_key, server_key, kdf_params }` — the NEW
  material the caller must persist. Does NOT persist anything itself.
- `finalize_change_password(store: &SessionStore, user_id: &[u8; 16],
  now_ns: u64) -> u64` — kills every session belonging to `user_id`
  (`snapshot_by_user` + `remove` per sid) and returns `now_ns` for the
  caller to persist as `tickets_invalid_before_ns`.

None of this is reachable today: `grep -r "ChangePw\|changepw\|change_password"
crates/shamir-server/src` returns nothing (re-verify).

## Two structural gaps this task must close (found by the orchestrator's
## own investigation — confirm both still hold before implementing)

**Gap 1 — no wire request type.** `crates/shamir-query-types/src/wire/
db_message.rs`'s `DbRequest`/`DbResponse` enums (the post-handshake
application-layer payload, matched in
`crates/shamir-server/src/db_handler/handler.rs::ShamirDbHandler::handle`)
have no `ChangePassword*` variant. `CreateScramUser` is the closest
existing precedent for an auth-management op living directly in
`DbRequest` (not inside a `BatchOp`) — follow its shape.

**Gap 2 — `ShamirDbHandler` has no `SessionStore` access.**
`finalize_change_password` needs `&SessionStore` to kill the user's other
sessions, but `SessionStore` is owned by the connection/server layer
(`Arc<SessionStore>` lives in `connection_context.rs`/`scheduler.rs`,
created once in `server_launcher.rs`) and is passed to
`dispatch_request_view` as ITS OWN parameter — NOT threaded into the
`RequestHandler` trait's `handle(&self, session, req, conn)` signature
that `ShamirDbHandler` implements. `ConnectionServices` (the `conn`
parameter) also carries no `SessionStore` reference (it only has
`conn_id`/`push`/an `Any`-typed per-CONNECTION extension slot — the
wrong lifetime scope for a server-wide `SessionStore`).

**Investigate before implementing**: confirm this gap is real (re-trace
the actual call chain from `server_launcher.rs`'s `session_store`
creation through to where `ShamirDbHandler` is constructed and where
`dispatch_request_view` is invoked). The straightforward fix — add an
`Arc<SessionStore>` field to `ShamirDbHandler` (builder-style, matching
the existing `.with_node_mode(...)` chaining pattern seen on
`ShamirDbHandler::new(...)`), threaded in at construction time from
`server_launcher.rs` where `session_store` is already created — is the
recommended approach unless you find a cleaner seam. Do NOT try to move
`SessionStore` ownership or restructure `dispatch_request_view`'s
signature; this is additive.

## The fix

**1. Extend `UserDirectory` trait** (`crates/shamir-connect/src/server/
admin.rs` — confirm exact location) with a new method to persist new
credentials, since none of `lookup_by_name`/`insert`/`update_roles`/
`bump_tickets_invalid`/`user_id` currently can:

```rust
/// Persist new SCRAM credentials (salt/stored_key/server_key/kdf_params)
/// for an existing user. Returns true if the user was found and updated.
fn update_credentials(
    &self,
    username: &str,
    new_salt: [u8; limits::SALT_BYTES],
    new_stored_key: StoredKey,
    new_server_key: [u8; 32],
    new_kdf_params: KdfParams,
) -> Result<bool>;
```

(Adjust the exact signature to whatever's cleanest given `ChangePwApply`'s
actual field types — investigate first.) Implement it in
`crates/shamir-server/src/user_directory.rs`'s `FjallUserDirectory`,
reusing the EXISTING private `read_modify_write` helper that
`update_roles`/`bump_tickets_invalid` already use (same
read-modify-write-and-persist shape — `PersistedUser` already has
mutable `salt`/`stored_key`/`server_key`/`kdf_params` fields, confirmed
at the time of this brief). Do not invent a new persistence mechanism.

**2. Add two `DbRequest` variants** (and matching `DbResponse` variants)
in `db_message.rs`, mirroring `CreateScramUser`'s doc-comment style:

- `ChangePasswordChallenge { client_nonce_cp: [u8; 32] }` → response
  carrying `ChangePwChallengeView`'s fields (`server_nonce_cp`, `salt`,
  `kdf_params`).
- `ChangePasswordVerify { client_proof_old: [u8; 32], new_salt: [u8; ...],
  new_stored_key: [u8; 32], new_server_key: [u8; 32] }` → a success/error
  response (no payload needed beyond ok/err — the client already knows
  its own new credentials).

Investigate how `[u8; N]` arrays already (de)serialize elsewhere in this
enum (e.g. how session ids / nonces are carried in other `DbRequest`
variants) so the wire encoding is consistent — do not invent a different
byte-array wire convention.

**3. Wire both into `ShamirDbHandler::handle`'s match** (`handler.rs`):

- `ChangePasswordChallenge`: look up the calling session's user's CURRENT
  salt/kdf_params (via the `UserDirectory`/`admin.user_dir` the handler
  already holds — check how `CreateScramUser`'s handler resolves the
  admin/user-dir reference, same access path applies here), call
  `start_change_password_challenge`, return the challenge view.
- `ChangePasswordVerify`: look up current stored_key/salt/kdf_params, call
  `verify_change_password_request_with_sid` with the session's own id
  (the handler already has `session_id` available via whatever seam
  `dispatch_request_view` uses — check `Session`'s own fields; the
  function's doc comment notes `session_id` is passed explicitly because
  `Session` doesn't carry its own id, so confirm where the CALLER already
  has it and thread it through). On success: persist via the new
  `update_credentials`, then call `finalize_change_password(session_store,
  &user_id, now_ns)` (this is where Gap 2's `SessionStore` access is
  needed) and `bump_tickets_invalid` (or fold that into
  `update_credentials` directly — investigate which is cleaner; the spec
  note in `changepw.rs`'s doc comment says the caller persists
  `tickets_invalid_before_ns` "for atomicity reasons", so doing it in the
  SAME `read_modify_write` transaction as the credential update, rather
  than a second separate write, is likely the correct choice — verify and
  document your reasoning).

**Authorization**: changePassword only requires the caller to be
authenticated as the user whose password they're changing (proven by the
SCRAM proof-of-old-password itself) — no additional session/role
permission gate is needed beyond "you have a valid session and you know
the old password." Do not add an extra authorization check that would
be redundant with (or could conflict with) the SCRAM verification itself.

## Test requirement

In `shamir-server`/`shamir-connect`: a full live-dispatch-path test
proving `ChangePasswordChallenge` → `ChangePasswordVerify` (with a
correct old-password proof) over the real `ShamirDbHandler`/
`dispatch_request_view` path:
- Succeeds and persists the new credentials (a subsequent login with the
  OLD password fails; login with the NEW password succeeds).
- Bumps `tickets_invalid_before_ns` (a previously-issued resumption
  ticket for this user is now rejected).
- Kills all of the user's existing sessions (a second, concurrently-open
  session for the same user is invalidated).
- A `ChangePasswordVerify` with a WRONG old-password proof is rejected
  (`AuthFailed`) and changes nothing (old credentials still work,
  sessions still alive).

## Test scope

```
./scripts/test.sh -p shamir-server -p shamir-connect
```

## Verification (lighter per-task gate, agreed for this campaign)

```
cargo check --workspace --all-targets
./scripts/test.sh -p shamir-server -p shamir-connect
```
Full fmt/clippy/test --full is FINAL-GATE's (#529's) job. This task does
NOT block FINAL-GATE (MEDIUM — admin-path revocation already exists as a
fallback) — do not add it to #529's blockedBy.

## Explicit permission to scope down

If threading `SessionStore` into `ShamirDbHandler` (Gap 2) proves more
invasive than expected (e.g. it requires touching many call sites beyond
`server_launcher.rs`), it is fine to land the credential-update half
(`ChangePasswordVerify` persists new credentials + bumps
`tickets_invalid_before_ns`) WITHOUT the session-killing half, as long as
this is honestly documented as a partial fix (the ticket-revocation
half of the audit's stated goal would remain open) with a properly
scoped follow-up task description in your report. Do not silently skip
the session-killing call while claiming full closure.

## Report format

```
[Investigation]
  > Confirmed Gap 1 (no wire request type) and Gap 2 (no SessionStore
    access in ShamirDbHandler) still hold, or found they'd already
    partially changed
  > Chosen mechanism for threading SessionStore into ShamirDbHandler
[Implementation] Status: fixed / scoped-down-with-followup
  > update_credentials: exact signature, FjallUserDirectory impl via
    read_modify_write
  > New DbRequest/DbResponse variants: exact shape
  > ShamirDbHandler::handle wiring: challenge + verify flow, session-kill
    timing relative to credential persistence
  > New tests: confirmed RED before / GREEN after for each behavior in
    the Test requirement section
[Verification]
  cargo check --workspace --all-targets: pass/fail
  ./scripts/test.sh -p shamir-server -p shamir-connect: pass/fail
```

Given this touches session lifecycle and credential persistence, this
MUST go through an adversarial review pass before committing — same
discipline as the rest of this campaign. If that review finds a genuine
bug, the orchestrator fixes it directly (never re-delegates),
re-verifies, and sends the fix through a second review pass before
committing.
