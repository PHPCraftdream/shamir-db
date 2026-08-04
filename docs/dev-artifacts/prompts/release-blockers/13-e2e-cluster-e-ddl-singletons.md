# Brief — e2e gap cluster E: `rename_db`, `describe_table`, `change_password`

Task: #978 in the session TaskList. Source: `docs/dev-artifacts/research/2026-08-03-e2e-oql-ddl-coverage-matrix.md`, "Cluster E — DDL singletons".

## Scope correction — interactive tx is ALREADY covered, do not touch it

The matrix's cluster E description lists interactive `TxBegin/TxExecute/
TxCommit/TxRollback` as a 4th gap. **This is stale** — verified: it is
already thoroughly covered live in
`crates/shamir-client-ts/src/__tests__/e2e.test.ts` (~line 700-763, tests
`'itx: begin → execute(write) → commit, row visible after commit'` and
`'itx: rollback discards the writes'`). Do NOT add more interactive-tx
tests — this task is `rename_db` + `describe_table` + `change_password`
only.

## Gap 1 — `rename_db`

`RenameDbOp` (`crates/shamir-query-types/src/admin/types/repo_ops.rs`
~line 87-91): `{ "rename_db": "old_name", "to": "new_name" }`. Guards:
`SYSTEM_DB` cannot be renamed; source must exist; destination must NOT
exist. Check `crates/shamir-client-ts/src/core/builders/ddl.ts` (or
`admin.ts`) for the exact `renameDb(...)` builder name/signature — do not
guess. Write a test: create a db with a table + some rows, `rename_db` it,
assert the OLD name is gone (404/error on use) and the NEW name serves the
same data (query the rows back). Also assert the two guard errors
(renaming a nonexistent db; renaming onto an existing name) via
`assertThrows`.

## Gap 2 — `describe_table`

`DescribeTableOp` (`crates/shamir-query-types/src/admin/types/table_ops.rs`
~line 64-68): `{ "describe_table": "name", "repo": "main" }`. The full
response shape (`crates/shamir-db/src/shamir_db/execute/admin_describe.rs`
~line 197-210): `describe_table`, `repo`, `schema`, `schema_version`,
`indexes` (list), `validators` (list), `retention`, `buffer`, `owner`,
`group`, `mode`. Write a test: create a table with a schema validator, at
least one index (any type), and a buffer config set — then `describe_table`
and assert EVERY one of those 10 response fields is present and reflects
what was actually configured (not just non-null — actually match the
values you set). Check the TS builder for `describeTable(...)`'s exact
name/signature first.

## Gap 3 — `change_password`

**No client-side wrapper exists at all today** — `setSuperuser`/
`createScramUser` have public `ShamirClient` methods over the same
internal `sendDbRequest`, but `change_password` has neither a builder nor
a client method. Decision (confirmed with the operator): add a small
public `changePassword(oldPassword, newPassword)` method to `ShamirClient`
mirroring the existing `setSuperuser`/`createScramUser` pattern exactly —
this is a real, legitimate SDK gap being closed, not scope creep.

### Wire flow (spec §12.5, 2 steps)

1. **`ChangePasswordChallenge`** request: `{ op: "change_password_challenge",
   client_nonce_cp: <32 random bytes> }`. Response (`kind:
   "change_password_challenge"`, per `DbResponse`'s `#[serde(tag = "kind",
   rename_all = "snake_case")]` in `crates/shamir-query-types/src/wire/db_message.rs`
   ~line 265, variant at ~line 361-376): `server_nonce_cp` (32 bytes),
   `salt` (16 bytes, current), `kdf_memory_kb`, `kdf_time`, `kdf_parallelism`,
   `kdf_argon2_version`.
2. **`ChangePasswordVerify`** request: `{ op: "change_password_verify",
   client_proof_old: <32 bytes>, new_salt: <16 random bytes>,
   new_stored_key: <32 bytes>, new_server_key: <32 bytes> }`. Response on
   success: `kind: "change_password_ok"` (`DbResponse::ChangePasswordOk`,
   no payload).

### Byte-exact crypto — read the Rust reference before writing ANY code

The auth-message construction for this flow is **DIFFERENT from login's**
`buildAuthMessage` in `crates/shamir-client-ts/src/core/scram.ts` — it has
a different domain tag and different inputs (notably includes the live
session id). Rust reference:
`crates/shamir-connect/src/common/changepw.rs::build_auth_message_cp`
(~line 54-84) + its `ChangePwAuthMessageInputs` struct (~line 32-51) +
domain tag `CHGPW_V1 = b"SHAMIR-CHGPW-v1"`
(`crates/shamir-connect/src/common/domain_tags.rs:20`). Exact byte layout:

```
"SHAMIR-CHGPW-v1"                         (15 bytes)
u16_be(byte_len(username_nfc)) || username_nfc
session_id(32) || client_nonce_cp(32) || server_nonce_cp(32) || salt(16)
u32_be(memory_kb) || u32_be(time) || u32_be(parallelism)
u8(argon2_version) || u8(transport_kind) || u8(binding_mode)
channel_binding_at_auth(32)
```

You must add a new `buildAuthMessageCp(...)` function to `scram.ts`
mirroring `buildAuthMessage`'s existing style/validation (length asserts on
every fixed-size field), NOT modify `buildAuthMessage` itself (login must
stay untouched).

**Where each input comes from on the client side:**
- `username`: same normalized value the client already used at login —
  check how `ShamirClient`/`protocol.ts` stores it post-login (search for
  where the login flow keeps `normalizedUser` after `connect()` resolves).
- `session_id`: `ShamirClient.sessionId()` — already a public method
  (`client.ts` ~line 337-338), 32 bytes.
- `client_nonce_cp`: fresh 32 random bytes per changePassword call (use
  `this.platform`'s CSPRNG the same way login's `clientNonce` is generated
  — find that call site and mirror it, do not hand-roll `Math.random`).
- `server_nonce_cp`, `salt`, `kdf_*`: from the `ChangePasswordChallenge`
  response (step 1).
- `transport_kind`, `binding_mode`, `channel_binding_at_auth`: this client
  only ever uses the WS/no-TLS-exporter path — confirmed by reading
  `protocol.ts`'s actual login-time `buildAuthMessage(...)` call (~line
  153-157): it passes NO `tlsExporterOrZeros`/`transportKind`/`bindingMode`,
  so all three default (`TRANSPORT_KIND_WS`, `BINDING_MODE_TLS_NO_EXPORT`,
  32 zero bytes respectively — already exported constants in `scram.ts`).
  Reuse those SAME exported constants for `buildAuthMessageCp` — the
  server snapshotted the identical values at session creation, so they
  MUST match bit-for-bit or the server's independent recomputation fails
  with `AuthFailed` (this is a self-checking property: if your byte layout
  is wrong anywhere, the live e2e test will fail with an auth error, not
  silently pass — treat any auth failure as a signal to re-check the byte
  layout against the Rust reference, not as a reason to loosen the test).

**Proof + new-credential derivation** — reuse the EXISTING
`computeClientProof(platform, password, salt, kdfParams, authMessage)`
from `scram.ts` twice:
1. Old-password proof: `computeClientProof(platform, oldPassword, <salt
   from challenge>, <kdf from challenge>, authMessageCp)` →
   `.clientProof` is `client_proof_old`.
2. New credential material: generate a fresh 16-byte `new_salt` (same
   CSPRNG as above), then `computeClientProof(platform, newPassword,
   new_salt, <SAME kdf params from the challenge — the server ignores the
   client's KDF choice for the new material anyway per
   `ChangePwApply.kdf_params` in `changepw.rs` ~line 55-56, but you still
   need a concrete `kdfParams` value to run `argon2id` locally>,
   <any dummy 1-byte non-empty message, since `authMessage` here is only
   used inside `computeClientProof` to derive `clientSignature`/
   `clientProof`, which you DON'T need for step 2 — you only need
   `.storedKey` and `.serverKey` from the returned `ClientProofResult`>)
   → `new_stored_key = .storedKey`, `new_server_key = .serverKey`.

Add the new `changePassword` method to `ShamirClient` (`client.ts`), right
after `setSuperuser`/`setReplicator` for consistency, following their exact
doc-comment + `sendDbRequest` + response-`kind`-check pattern. Signature:
`async changePassword(oldPassword: string, newPassword: string):
Promise<void>`.

### Test

In `crates/shamir-client-ts/src/__tests__/` (new file
`e2e-change-password.test.ts`, following the suite's `describe.skipIf(!SERVER_AVAILABLE)`
convention): create a SCRAM user with a known password, log in as that
user, call `client.changePassword(oldPw, newPw)`, assert it resolves
without error, then: (a) a fresh connect with the OLD password fails, (b) a
fresh connect with the NEW password succeeds. Also assert the
`ChangePasswordVerify` old-proof-mismatch case errors (call
`changePassword` with a wrong "old" password and expect rejection) —
exercises the `AuthFailed` path deliberately, proving the server-side
verification is real, not a no-op.

## Required work — file organization

- `rename_db` and `describe_table`: pick a home in `tests/e2e/tests/*.js`
  (JS suite) matching where similar singleton-DDL ops already live (check
  `08-admin-ddl.test.js`), OR a new small JS file if that's cleaner — your
  call.
- `change_password`: new TS file
  `crates/shamir-client-ts/src/__tests__/e2e-change-password.test.ts` (per
  above) since it requires the TS SCRAM primitives — the JS e2e suite has
  no equivalent crypto helpers.

Use ONLY query builders/client methods for `rename_db`/`describe_table`
(no hand-assembled wire objects, repo-wide CLAUDE.md rule). The new
`changePassword` client method and `buildAuthMessageCp` scram helper are
the one sanctioned production-code addition for this task — everything
else stays test-only.

## Verification

- `cd tests/e2e && node e2e.test.js` — baseline after #977 is 18 files /
  143 passed / 0 failed. Report exact counts before and after.
- `crates/shamir-client-ts`: run the FULL vitest suite (not just the new
  file) and report pass/fail counts — baseline after #976 was 55 files /
  1028 tests (before #977's JS-only changes, which don't affect vitest
  count). Also run `npx tsc --noEmit` in that package and confirm it's
  clean (new production code in `client.ts`/`scram.ts` must typecheck).

## Scope discipline

- Do NOT add more interactive-tx tests (already covered, see correction
  above).
- Do NOT touch keyset pagination (cluster F, #979) or the low-priority
  clusters G/H (#980/#981).
- Do NOT modify `buildAuthMessage`/`computeClientProof`/`verifyServerSignature`
  (existing login primitives) — add `buildAuthMessageCp` as a NEW function
  alongside them, reusing what already exists, not rewriting it.
- If the live round-trip fails with an auth error, that means the byte
  layout is wrong somewhere — debug against the Rust reference cited above,
  do not weaken the test to make it pass.

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Do NOT run
`git commit` or `git add` — the orchestrator verifies your diff and the
test run, then commits. Only edit/create files and run read-only/test
commands.

## What to report back

List every test added and what it proves. For `change_password`, walk
through exactly how you derived each of the 6 wire fields
(`client_nonce_cp`, `client_proof_old`, `new_salt`, `new_stored_key`,
`new_server_key`, and the `buildAuthMessageCp` byte layout) and confirm the
live round-trip actually succeeded against the real server (not mocked).
Give exact test-run output (JS suite + full vitest suite + tsc) with real
pass/fail counts.
