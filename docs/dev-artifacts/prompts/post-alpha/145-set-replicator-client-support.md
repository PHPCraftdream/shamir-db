# Task #921 -- add SetReplicator client support (shamir-client + napi binding)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background

Task #920 (tests/e2e query-builder rewrite, just landed) fixed a
`hmac_required` bug in `tests/e2e/tests/16-replication.test.js` /
`17-replication-convergence.test.js`'s chmod calls. Fixing that exposed a
DEEPER, previously-masked bug (confirmed via a local test run 2026-08-02):

```
db error [query]: user 'repl' was created but role attachment failed:
invalid input: "replicator" is a reserved role name -- use SetReplicator
to grant/revoke replication access
```

Both test files call `client.createScramUser(replUser, replPw,
['replicator'])`, passing `'replicator'` as a generic role string. The
server explicitly rejects this -- `'replicator'` is a RESERVED pseudo-role
that can only be granted through the dedicated `SetReplicator` wire
operation, not the generic role-attachment path `createScramUser`'s
`roles` array goes through.

## What already exists (confirmed 2026-08-02) -- don't re-investigate

- `DbRequest::SetReplicator { user: String, on: bool, hmac: Option<String>
  }` and `DbResponse::ReplicatorSet { user: String, on: bool }` are BOTH
  real, working wire types
  (`crates/shamir-query-types/src/wire/db_message.rs`, task #621) --
  mirrors `SetSuperuser`/`SuperuserSet`'s shape and gate exactly
  (unconditional hmac required, no "last remaining" guard).
- The canonical HMAC input function already exists:
  `crates/shamir-query-types/src/hmac.rs::canonical_set_replicator(user:
  &str, on: bool) -> Vec<u8>` -- joins `b"set_replicator"`, `user`,
  `b"true"`/`b"false"` with null bytes (grep for it to see the exact
  signature/behavior before writing new code against it).
- The SERVER-SIDE handler for `DbRequest::SetReplicator` already works --
  confirmed by `crates/shamir-server/tests/set_replicator_wire.rs`
  (already exists, already passing) which sends the raw wire request
  directly against a `RequestHandler` (bypassing any client library,
  since none exists) and gets back `DbResponse::ReplicatorSet`. Read this
  test file FIRST -- it shows you the exact request/response shape and
  the HMAC derivation pattern (`session_key(&session)` +
  `canon::compute_tag_hex(&key, &canon::canonical_set_replicator(user,
  on))`) to replicate client-side.
- **No client library exposes `SetReplicator` at all**: not
  `crates/shamir-client/src/client.rs` (the shared Rust client core used
  by both the napi binding and any future native consumers), not
  `crates/shamir-client-node` (the napi binding `tests/e2e` uses), not
  `crates/shamir-client-ts` (the TS/WS SDK). This is a plumbing gap, not
  a design decision -- `SetSuperuser` has the exact same gap (no
  `set_superuser` method exists in `shamir-client` either), so this
  isn't an isolated oversight, just something nobody has needed until now.

## What to do

### 1. `crates/shamir-client/src/client.rs` -- add `set_replicator`

Read `create_scram_user` in this file first (around line 817) as the
closest existing pattern for "compute an hmac tag client-side, build a
top-level `DbRequest` variant, roundtrip, match the expected response
variant". Add a new method with the same shape:

```rust
pub async fn set_replicator(&self, user: &str, on: bool) -> Result<(), ClientError> {
    let tag = {
        let key = shamir_connect::common::crypto::derive_session_hmac_key(&self.session_id);
        let canonical = shamir_query_types::hmac::canonical_set_replicator(user, on);
        shamir_query_types::hmac::compute_tag_hex(&key, &canonical)
    };
    let req = DbRequest::SetReplicator {
        user: user.to_string(),
        on,
        hmac: Some(tag),
    };
    match self.roundtrip(&req).await? {
        DbResponse::ReplicatorSet { .. } => Ok(()),
        other => Err(ClientError::Protocol(format!(
            "expected ReplicatorSet, got {other:?}"
        ))),
    }
}
```
(Adjust to match this file's actual conventions -- error type names,
whether `roundtrip` takes `&DbRequest` or something else, whether other
methods return the echoed `user`/`on` instead of `()`, etc. Don't
copy-paste blindly; match the surrounding code's actual style.)

### 2. `crates/shamir-client-node/src/lib.rs` -- add the napi wrapper

Read `create_scram_user`'s napi wrapper in this file first (around line
254) as the pattern. Add:

```rust
/// Grant or revoke replication API access on an existing SCRAM user.
/// Requires the current session to belong to a superuser.
#[napi]
pub async fn set_replicator(&self, user: String, on: bool) -> Result<()> {
    let guard = self.inner.lock().await;
    let client = guard
        .as_ref()
        .ok_or_else(|| Error::from_reason("client closed"))?;
    match client.set_replicator(&user, on).await {
        Ok(()) => Ok(()),
        Err(core::ClientError::Db { code, message }) => {
            // mirror how create_scram_user/repl surface Db errors --
            // check what encode_db_error does for methods returning
            // Result<()> vs Result<Buffer> and match that pattern
            // (this method may need to return Result<Buffer> instead
            // of Result<()> if the wrapper.js typed-error convention
            // needs a decodable DbResponse::Error marker -- check
            // wrapper.js's decodeOrThrow/repl/createScramUser handling
            // before deciding the return type here).
            Err(infra_error(core::ClientError::Db { code, message }))
        }
        Err(e) => Err(infra_error(e)),
    }
}
```
**Important**: check `crates/shamir-client-node/wrapper.js`'s
`decodeOrThrow` pattern (used by `repl`/`createScramUser`) to see whether
`set_replicator` needs to return a msgpack-encoded `Buffer` (so the JS
wrapper can detect a `DbResponse::Error` marker and throw
`ShamirDbError`) rather than a plain `Result<()>` that can only signal
failure via a generic napi error with no `.code`. Match whatever the
existing convention is -- don't introduce a THIRD error-surfacing shape.
Update `wrapper.js` too if `set_replicator` needs the same typed-error
treatment as `repl`/`createScramUser`.

### 3. Update the two failing test files

In `tests/e2e/tests/16-replication.test.js` and
`17-replication-convergence.test.js`, change:
```js
await client.createScramUser(replUser, replPw, ['replicator']);
```
to:
```js
await client.createScramUser(replUser, replPw, []);
await client.setReplicator(replUser, true);
```
(or whatever the exact final napi method name/signature ends up being --
match it). Do NOT change any other test logic/assertions in these files.

## Verification

Rebuild is required this time (Rust source changes): `cd tests/e2e && npm
run build:server && npm run build:binding`. **Note on local target-dir**:
if `build:server` reports success but `tests/e2e`'s `npm test` can't find
`target/release/shamir-server.exe`, check whether `$CARGO_TARGET_DIR` is
set in your shell to something other than the repo-relative `target/` --
if so, copy/symlink the built binary to
`<repo-root>/target/release/shamir-server.exe` before running `npm test`
(this is a known LOCAL environment quirk on the orchestrator's machine,
unrelated to your changes -- CI has no such override and builds to the
default path).

Also run `cargo fmt -p shamir-client -p shamir-client-node -- --check` and
`cargo clippy -p shamir-client -p shamir-client-node --all-targets -- -D
warnings` (scope clippy to just these two crates -- don't run the full
workspace `--all-targets` sweep for this task) plus
`./scripts/test.sh -p shamir-client -p shamir-client-node` for any Rust
unit tests.

Then `cd tests/e2e && node e2e.test.js` -- expect:
```
files:  18
passed: 128
failed: 2
```
(the 2 remaining failures are `13-migration.test.js`'s intentionally-gated
`experimental_feature_disabled` cases, unrelated to this task -- do not
try to fix those).

## Definition of done

- `shamir-client::Client::set_replicator` exists and works.
- `shamir-client-node`'s napi binding exposes it with correct
  typed-error surfacing (matching the existing `repl`/`createScramUser`
  convention).
- Both replication test files updated to use it instead of passing
  `'replicator'` via `createScramUser`'s roles array.
- Local `node e2e.test.js` matches the 128/2 baseline above.
- `cargo fmt`/`clippy` clean for the two touched Rust crates;
  `./scripts/test.sh` green for them.
- Trigger `gh workflow run ts-e2e-nightly.yml --ref master` and confirm
  the `node napi e2e` job's replication scenarios pass on real CI (the
  orchestrator will do this triggering + confirmation step if you don't
  have `gh` CLI access -- report back clearly if so).

Commit with `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>` per
repo convention.
