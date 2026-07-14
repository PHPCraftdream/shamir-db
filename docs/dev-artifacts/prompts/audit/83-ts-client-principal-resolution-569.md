בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Brief: fix TS client chown/addGroupMember/removeGroupMember broken for
# username input (task #569)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Context

Found while investigating task #567 against a genuinely fresh
`shamir-server` binary (rebuilt 2026-07-13, confirmed via mtime — see
task #566). This is a REAL, currently-broken client bug, not a stale
test assertion.

`crates/shamir-client-ts/src/core/principal-id.ts`'s `principalId(username)`
is a client-side FxHash reimplementation of `fxhash(username)` — the OLD
(pre-#548) model where `Actor::User` was literally `fxhash(username)`,
reproducible offline from just the string. Task #548 ("unbind
security-Actor from fxhash(username), bind to stable user_id")
deliberately REPLACED this with a real, server-assigned, random 128-bit
`user_id` projected into `principal64` — architecturally NOT
reproducible client-side from a username alone anymore (that
unpredictability was the whole point of #548's security fix).
`principalId()` was never updated/removed after #548 landed, and
nothing caught it because the TS e2e suite was running against a
week-stale pre-#548 binary the entire time (#566).

Confirmed broken by a real fresh-binary run (`npx vitest run
src/__tests__`, 2026-07-13): 4 failures, all this same root cause —

- `e2e-permissions.test.ts` A11/G4d-group:
  `db error [invalid_owner]: add_group_member target user id
  <garbage> does not resolve to a known principal`
- `e2e-principal.test.ts` (3 failures): `chown target owner id
  <garbage> does not resolve to a known principal`,
  `add_group_member target user id <garbage> does not resolve to a
  known principal`, and `invalid input: username exists`.

The SERVER is behaving correctly (task #561 added the real
resolver-gated existence check on `add_group_member`/`chown` that
rejects a fabricated id). The CLIENT is broken.

### Wire-protocol constraint (checked — do not attempt to change this)

`ChownOp.owner`, `AddGroupMemberOp.user`, `RemoveGroupMemberOp.user` are
typed `u64` on the wire
(`crates/shamir-query-types/src/admin/access.rs:106,186,201`). The
server does NOT accept a username string for these ops and never
resolves one — a real principal64 must be supplied by the caller. This
is NOT changing; the fix is entirely client-side.

## Design decision (already made — implement this, do not re-litigate)

The `string` convenience overload on `chown`/`addGroupMember`/
`removeGroupMember` is being REMOVED, not patched. Reasoning: these are
synchronous builder functions (they must be, since HMAC signing needs
the full canonical bytes computed synchronously) — but resolving a
username to its real principal64 is inherently an async network
round-trip. There is no way to keep a synchronous `string` input path
that is actually correct. Silently keeping a "supported" string input
that produces WRONG results (as it does today) is worse than a
compile-time type error forcing the caller to resolve first.

### 1. Remove the `string` overload from three builders

In `crates/shamir-client-ts/src/core/builders/admin.ts`:

- `chown(signer, resource, owner: bigint | number)` (currently
  `owner: Actor | string`, ~line 129-138 — check the actual current
  parameter type name/shape, the exact union may differ slightly from
  this description; the point is: drop the `string` arm and the
  `principalId(owner)` call, keep only the pre-computed-id path).
- `addGroupMember(signer, ref, user: bigint | number)` (drop the
  `string` arm, ~line 186-194).
- `removeGroupMember(signer, ref, user: bigint | number)` (drop the
  `string` arm, ~line 205-213).

Update each function's doc comment to remove the "`string` — username,
hashed to `principalId(username)`" bullet and instead say: "callers
must resolve a username to its real principal64 first — see
`ShamirClient.resolvePrincipal(username)`."

### 2. Add `ShamirClient.resolvePrincipal(username): Promise<bigint>`

In `crates/shamir-client-ts/src/core/client.ts` (alongside the other
convenience methods like `setSuperuser`): a new async method that:

1. Calls `this.execute(<some db — check how other admin convenience
   methods on ShamirClient pick a db context; `listUsers` at the
   `ListOp::Users` level is NOT database-scoped in the Rust handler —
   confirm and use whatever the established pattern is for other
   Root-scoped admin calls on this client>, { queries: { u:
   ddl.listUsers() } })` — `ddl.listUsers()` already exists
   (`crates/shamir-client-ts/src/core/builders/ddl.ts:508-510`).
2. Reads `resp.results.u.records` (an array of `{ name, principal64,
   superuser, database }` — this shape comes from
   `crates/shamir-db/src/shamir_db/execute/admin_list.rs:94-108`'s
   `ListOp::Users` handler).
3. Finds the record whose `name === username`. If found, returns its
   `principal64` as a `bigint`. If not found, throws a clear error
   (`Error(\`no such user: ${username}\`)` or similar — check the
   codebase's error-throwing convention for "not found" cases on this
   client and match it).

Add the corresponding type declaration wherever `ShamirClient`'s other
methods are declared (check `types/admin.ts`/`types/index.ts` for the
established pattern — `setSuperuser`'s task #560 addition is a recent
precedent to mirror).

### 3. Update callers to use the new async resolve step

- `crates/shamir-client-ts/src/__tests__/e2e-permissions.test.ts`'s
  A11/G4d-group test (`admin.addGroupMember(adminClient!,
  admin.groupId(gid), USER_G)`) — resolve `USER_G`'s principal64 via
  `await adminClient!.resolvePrincipal(USER_G)` first, pass the
  resolved bigint into `addGroupMember` instead of the raw username
  string.
- `crates/shamir-client-ts/src/__tests__/e2e-principal.test.ts` — this
  whole file's premise (`expect(owner).toBe(principalId(TEST_USER))`,
  currently at lines ~94/127/172) assumed `principalId(username) ===
  the server's real principal64`. Rewrite it to assert instead that
  `await adminClient!.resolvePrincipal(TEST_USER)` matches the
  principal64 actually observed elsewhere (e.g. from `accessTree`'s
  principals section, or from the id returned by the resolve call
  itself used consistently as the oracle) — the test's *purpose*
  (principal-id consistency across surfaces) is still valid, only the
  offline-hash assumption was wrong. Also check this file for the
  `username exists` failure mentioned above — likely the SAME
  redundant-dual-creation pattern task #560's A8 test hit (creating the
  same username via both `createScramUser` and `admin.createUser`);
  find and remove the redundant creation, matching the A8 fix's shape
  (`crates/shamir-client-ts/src/__tests__/e2e-permissions.test.ts`'s
  A8 test, already fixed in task #560, is the precedent to follow).

### 4. Fix the two now-invalid unit tests

`crates/shamir-client-ts/src/core/builders/__tests__/admin.test.ts` has
two tests exercising the removed string-overload behavior (search for
`'accepts string username and hashes to principalId'` — two hits, one
for `chown`, one for `addGroupMember`). Since the feature is
deliberately removed, delete these two test cases (a TS compile error
is the correct replacement proof that the string path no longer type
checks — you cannot easily unit-test a compile error at runtime, and
inventing a `// @ts-expect-error` compile-fail test is a reasonable
alternative if the codebase has a precedent for that pattern; check
before introducing a new one).

### 5. Decide the fate of `principalId()` / `principal-id.ts`

Confirmed (grep) that `principalId()` has NO remaining legitimate
caller once the above changes land — its only real call sites were the
three builders being fixed here. Per this repo's discipline ("if you
are certain something is unused, delete it completely — no
re-exports, no backwards-compat shims"): DELETE
`crates/shamir-client-ts/src/core/principal-id.ts` and its own test
file (`crates/shamir-client-ts/src/core/__tests__/principal-id.test.ts`),
and remove its export from wherever `principal-id.ts` is re-exported
(check `core/index.ts` or equivalent barrel file). Before deleting,
re-grep the WHOLE `crates/shamir-client-ts/src` tree for
`principalId`/`principal-id` one more time yourself to confirm nothing
was missed by this brief's own grep (things may have shifted since this
brief was written).

## Out of scope

- Do NOT touch the wire protocol (`ChownOp`/`AddGroupMemberOp`/
  `RemoveGroupMemberOp` stay `u64`-typed exactly as they are).
- Do NOT touch the Rust `shamir-client` crate or any server-side code —
  this is purely a `shamir-client-ts` fix.
- Do NOT attempt to make `resolvePrincipal` synchronous or find some
  clever way to preserve the string overload — the design decision
  above (remove it, add an explicit async resolve step) is final for
  this task.

## Verification

```
cd crates/shamir-client-ts
npx tsc --noEmit
npx vitest run src/__tests__
```

The fresh `shamir-server` binary this brief's investigation used is at
`D:\dev\rust\.cargo-target\release\shamir-server.exe` (rebuilt
2026-07-13 12:40) — the e2e harness's new stale-binary guard (task
#566) will refuse a stale binary automatically now, so if verification
fails with a "stale shamir-server binary" error, rebuild
(`cargo build --release -p shamir-server` from the repo root) before
re-running, don't bypass the guard.

## Definition of done

- `chown`/`addGroupMember`/`removeGroupMember` only accept
  `bigint | number` for the owner/user parameter — no `string` overload.
- `ShamirClient.resolvePrincipal(username): Promise<bigint>` exists,
  backed by `ListOp::Users`, throws clearly on an unknown username.
- `e2e-permissions.test.ts`'s A11/G4d-group test passes against the
  fresh binary.
- `e2e-principal.test.ts` passes against the fresh binary, rewritten to
  not depend on the invalidated offline-hash assumption.
- The two now-invalid unit tests in `admin.test.ts` are removed or
  replaced with a compile-fail check, not left testing dead behavior.
- `principal-id.ts` and its test file are deleted if (as expected) no
  other caller survives; if you find a surprising remaining caller,
  STOP and report it instead of silently keeping the file around "just
  in case."
- `npx tsc --noEmit` clean.
- `npx vitest run src/__tests__` green (compare against the known
  271/275-passing baseline from this brief's own investigation — the 4
  known failures this brief targets should now pass, and no other test
  should newly fail).

## Report

When done, produce a final summary (not a bare tool call): every file
changed (including deletions), the full diff of `resolvePrincipal`'s
implementation, the gate command outputs, confirmation of what
`principalId()`'s removal grep found (nothing else, or something
unexpected), and any discrepancy between this brief's assumptions and
the actual code you found.
