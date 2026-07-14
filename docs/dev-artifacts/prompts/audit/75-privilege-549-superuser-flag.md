בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Brief: superuser as a first-class flag — reservation, SetSuperuser op, handshake wiring (task #557)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Context

`docs/dev-artifacts/design/identity-privilege-unification-548-549-decision.md` §4
(already signed off) is the source of truth for this task's design —
read it first. This brief is step #72 of that design's phased plan
(§7), renumbered to 75 since briefs 70-74 were claimed by #552-556.

Builds on task #556 (already landed): `PersistedUser.superuser: bool`
exists, boot-time normalization already migrates the legacy
`"superuser"` role STRING into this flag, and `superuser_count:
AtomicU64` is already warmed/maintained by `remove()`. This task wires
the ENFORCEMENT side: reserving the string so it can never be written
again, a wire op to flip the flag, and switching `SessionPermissions`
to read the flag instead of scanning role strings.

## 1. Reserve the `"superuser"` string — single choke point

Add the reservation check inside `FjallUserDirectory::update_roles`
itself (`crates/shamir-server/src/user_directory.rs`), NOT duplicated
at every caller. This is the directory's own write boundary, and every
current + future role-writing caller goes through it:

```rust
fn update_roles(&self, username: &str, roles: Vec<String>, now_ns: u64) -> Result<bool> {
    if roles.iter().any(|r| r == "superuser") {
        return Err(Error::InvalidInput(
            "\"superuser\" is a reserved role name — use SetSuperuser to grant/revoke superuser status",
        ));
    }
    self.read_modify_write(username, |user| { /* unchanged body */ })
}
```

This closes the reservation for BOTH of the task's named call sites in
one place:
- `crates/shamir-server/src/db_handler/admin.rs`'s `create_scram_user`
  (line ~121-125: `admin.user_dir.update_roles(&name, roles, 0)` with
  the raw wire-supplied `roles` — now rejected if it contains the
  string, surfacing as a `query`-class error from the existing
  `Err(e) => ...` handling around that call; give it a distinguishable
  message but do not invent a new response variant for this — the
  existing generic error path is sufficient).
- `crates/shamir-connect/src/server/admin.rs`'s `update_user` (line
  207-264, `directory.update_roles(target_username, new_roles,
  now_ns)`), even though this generic function currently has NO live
  wire call site in shamir-server (confirmed by grep — dead code
  today, likely activated by a future admin surface). Reserving at the
  directory boundary means it's already safe whenever that surface
  lands, with zero extra work.
- The future `GrantRole` port (task #559) — same reasoning, free.

**Important — this breaks an existing #556 test.** The already-landed
test `normalization_migrates_superuser_role_string_and_is_idempotent`
(`crates/shamir-server/tests/user_directory.rs`) seeds the legacy
pre-migration shape via
`store.update_roles("alice", vec!["superuser".to_string()], 1_000)` —
that call will now return `Err` instead of silently succeeding, since
this task reserves the string. You must fix this without weakening the
reservation:
- Move (or duplicate, if moving is awkward) this test's seeding step to
  bypass the now-reserved `update_roles` and construct the legacy
  on-disk shape directly. `PersistedUser` and its `roles`/`superuser`
  fields are already `pub(crate)` (from #556) specifically so in-crate
  tests can do this — but the EXTERNAL integration test file
  (`tests/user_directory.rs`, a separate compilation unit) cannot see
  `pub(crate)` items. Relocate this specific test (or write an
  equivalent replacement covering the same normalization-idempotence
  property) into the in-crate unit-test module
  (`crates/shamir-server/src/tests/user_directory_tests.rs`, which
  #556 already established for exactly this kind of internals-testing
  need), where you can construct a `PersistedUser { roles: vec!["superuser".into()], superuser: false, .. }`
  directly, serialize it, and write it into the `users` keyspace via
  whatever low-level access the existing tests in that file already use
  (check `projection_fails_closed_on_collision_naming_both_usernames`
  etc. for the established pattern — do not invent a new bypass
  mechanism if one already exists there).
- Do NOT leave the old external test broken/deleted without a
  replacement — the property it verifies (boot-time migration of
  legacy on-disk data is idempotent) is still real and still needs
  coverage; it just needs to seed its fixture through a different path
  now that the public API it used to seed with is reserved.

## 2. New `FjallUserDirectory::set_superuser` method

```rust
/// Grant or revoke superuser status. Idempotent (no-op if the flag is
/// already at the requested value). Bumps `tickets_invalid_before_ns`
/// on an actual change (spec §12.6 — a privilege change must invalidate
/// existing sessions, same as `update_roles`). Refuses to revoke the
/// LAST remaining superuser (uses the O(1) `superuser_count` from
/// task #556) — mirrors `remove()`'s last-superuser guard.
pub fn set_superuser(&self, username: &str, on: bool, now_ns: u64) -> Result<bool> {
    let _guard = self.write_lock.lock();

    let blob = match self.read_blob(username)? {
        Some(b) => b,
        None => return Err(Error::InvalidInput("user not found")),
    };
    let mut user: PersistedUser = rmp_serde::from_slice(&blob)
        .map_err(|e| Error::Encoding(format!("rmp decode: {e}")))?;

    if user.superuser == on {
        return Ok(false); // already at the requested state
    }
    if user.superuser && !on && self.superuser_count.load(Ordering::Relaxed) <= 1 {
        return Err(Error::InvalidInput(
            "cannot revoke superuser status from the last remaining superuser account",
        ));
    }

    user.superuser = on;
    if now_ns > user.tickets_invalid_before_ns {
        user.tickets_invalid_before_ns = now_ns;
    }
    let new_bytes = rmp_serde::to_vec_named(&user)
        .map_err(|e| Error::Encoding(format!("rmp encode: {e}")))?;
    self.users
        .insert(username.as_bytes(), new_bytes.as_slice())
        .map_err(|e| Error::Encoding(format!("fjall: insert: {e}")))?;
    self.db
        .persist(PersistMode::SyncAll)
        .map_err(|e| Error::Encoding(format!("fjall: persist: {e}")))?;

    if let Some(id) = user.user_id_array() {
        self.update_cache(&id, user.tickets_invalid_before_ns);
    }
    if on {
        self.superuser_count.fetch_add(1, Ordering::Relaxed);
    } else {
        self.superuser_count.fetch_sub(1, Ordering::Relaxed);
    }
    Ok(true)
}
```

This is deliberately a bespoke method (not routed through the existing
`read_modify_write` helper) because it needs to adjust
`superuser_count` in the SAME critical section as the blob mutation —
`read_modify_write`'s closure has no way to touch the counter. Match
this repo's actual field/method names in `user_directory.rs` (read the
file — do not assume the pseudocode above is 100% exact if the real
code shape differs slightly, e.g. error variant names).

## 3. Bootstrap sets the flag, not the role string

`crates/shamir-server/src/bootstrap.rs`'s `insert_with_role` (line
160-174) currently does:
```rust
dir.insert(name.to_string(), record)?;
dir.update_roles(name, vec![role.to_string()], 0)?;
```
called from `ensure_superuser` as `insert_with_role(dir, name, record, "superuser")`.
Since `update_roles` now reserves the string, this call would fail.
Replace it: after `dir.insert(...)`, call `dir.set_superuser(name, true, 0)?`
instead of `dir.update_roles(name, vec!["superuser".to_string()], 0)`.
If `insert_with_role`'s `role: &str` parameter becomes dead (only ever
called with `"superuser"` today — check before removing it; if there's
truly only one call site, simplify the helper's signature rather than
leaving an unused parameter).

## 4. `SessionPermissions` reads the flag, not role strings

`crates/shamir-connect/src/server/session.rs:32-41`:
```rust
impl SessionPermissions {
    pub fn from_roles(roles: Vec<String>) -> Self {
        let is_superuser = roles.iter().any(|r| r == "superuser");
        Self { is_superuser, roles }
    }
```
Add a new constructor that takes the flag directly instead of deriving
it from the role list:
```rust
    /// Construct from the directory's authoritative `superuser` flag
    /// plus the (now-reserve-string-free) role list. Use this instead of
    /// `from_roles` wherever the caller has a real flag value (i.e. every
    /// production call site after task #557) — `from_roles`'s string-scan
    /// is kept only for callers that never had the flag (in-memory/test
    /// directories, fixtures) and is now effectively unreachable for the
    /// literal string `"superuser"` in any roles list produced by the real
    /// directory, since that string is reserved at the write boundary.
    pub fn new(is_superuser: bool, roles: Vec<String>) -> Self {
        Self { is_superuser, roles }
    }
```
Do NOT delete `from_roles` — other callers (`InMemoryUserDirectory`-backed
tests/fixtures elsewhere in the workspace, `bootstrap_tests.rs`, etc.)
may still legitimately build a `SessionPermissions` from a plain role
list without ever touching `FjallUserDirectory`. Check actual call
sites via grep before deciding whether any of them need updating too —
if a caller's roles list could realistically still contain `"superuser"`
(e.g. `InMemoryUserDirectory`, which has no reservation and isn't
touched by this task), `from_roles` staying correct for it is
important; don't break that path.

## 5. Wire `SessionPermissions::new` into handshake

`crates/shamir-server/src/connection/handshake.rs:404-459` currently
calls `ctx.user_dir.lookup_roles(username.as_str())` TWICE — once to
build the session's `SessionPermissions::from_roles(roles)` (line
409-417), once more for `roles_for_ticket` (line 451-459). `user_id`
is already resolved locally just above (line 405-408). Replace BOTH
`lookup_roles` calls with a SINGLE `ctx.user_dir.state_by_user_id(&user_id)`
call (the one-lookup snapshot task #556 built), and feed:
- `SessionPermissions::new(state.superuser, state.roles.clone())` for
  the session.
- `state.roles` for `roles_for_ticket` (task #558 removes roles from
  the ticket entirely — until then, keep threading the SAME value, this
  is a pure refactor of where it comes from, not a behavior change to
  the ticket's payload).

Handle the `None` case (the user existing at `user_id()` lookup but
`state_by_user_id` returning `None` should not happen in practice since
both read the same directory state — but be defensive: treat it the
same as the existing `Err`/`UnknownUser` handling already in this
function rather than panicking or unwrapping).

**`session_actor` needs NO change** — it already reads
`session.permissions.is_superuser` (a plain bool) to decide
`Actor::Admin` vs `Actor::User` (wired in task #555). This task only
changes HOW that bool gets computed at handshake time (flag-derived vs
role-string-scan); `session_actor` is agnostic to that and already
correct.

## 6. New wire op: `SetSuperuser`

This is a `DbRequest`/`DbResponse` pair (like `CreateScramUser`), NOT a
`BatchOp` — `BatchOp`s dispatch through `shamir-db`'s engine, which has
no handle to `shamir-server`'s real `FjallUserDirectory` (that bridge
is task #559's `UserAdminPort`, not yet built). Mirror
`CreateScramUser`'s exact shape in
`crates/shamir-query-types/src/wire/db_message.rs`:

```rust
/// Grant or revoke superuser status on an existing SCRAM-directory
/// account. Requires an already-superuser session AND an HMAC
/// confirmation tag (same "did-you-mean-it" mechanism as
/// destructive BatchOps, tasks #542/#551/#554 — see
/// `check_destructive_hmacs`'s doc comment for the pattern this
/// mirrors; this op is gated inline in its own handler rather than
/// through that BatchOp-shaped function, since `SetSuperuser` is a
/// top-level `DbRequest`, not a `BatchOp` inside a batch).
SetSuperuser {
    /// Target username.
    user: String,
    /// `true` to grant, `false` to revoke.
    on: bool,
    /// Hex-encoded HMAC-SHA256 tag over the canonical form — always
    /// required (unconditional, unlike `CreateFunctionOp`'s
    /// `security`/`secret_grants` fields).
    hmac: Option<String>,
},
```
Paired response (mirror `UserCreated`'s shape):
```rust
/// Successful [`DbRequest::SetSuperuser`].
SuperuserSet {
    user: String,
    on: bool,
},
```

### HMAC canonical form

`crates/shamir-query-types/src/hmac.rs` — add, following this file's
exact existing conventions (see `canonical_create_function` for the
most recent example, or the doc-table pattern used by every other
`canonical_*` function):

```rust
/// `b"set_superuser\0<user>\0<on>"`. `<on>` is the literal `"true"` or
/// `"false"` string (not `"1"`/`"0"`), matching how the field
/// round-trips through the wire's `bool` — the server and any client
/// signer must agree on this exact literal.
pub fn canonical_set_superuser(user: &str, on: bool) -> Vec<u8> {
    join_null(&[
        b"set_superuser",
        user.as_bytes(),
        if on { b"true" } else { b"false" },
    ])
}
```
Add the doc-table row alongside the other canonical forms in this
file's module doc comment.

### Handler + dispatch

`crates/shamir-server/src/db_handler/handler.rs`'s top-level `match
request` (around line 293-358, alongside `CreateScramUser`) gains:
```rust
DbRequest::SetSuperuser { user, on, hmac } => {
    set_superuser(self.admin.as_ref(), session, user, on, hmac).await
}
```
New handler in `crates/shamir-server/src/db_handler/admin.rs`,
positioned near `create_scram_user` (mirror its shape: superuser check
first, `AdminGlue` unwrap, then the op itself):
```rust
pub(super) async fn set_superuser(
    admin: Option<&AdminGlue>,
    session: &Session,
    user: String,
    on: bool,
    hmac: Option<String>,
) -> DbResponse {
    if !session.permissions.is_superuser {
        return DbResponse::Error {
            code: "permission_denied".into(),
            message: "set_superuser requires superuser".into(),
        };
    }
    let admin = match admin {
        Some(a) => a,
        None => {
            return DbResponse::Error {
                code: "not_supported".into(),
                message: "handler built without AdminGlue (no user_dir)".into(),
            }
        }
    };

    use shamir_query_types::hmac as canon;
    let canonical = canon::canonical_set_superuser(&user, on);
    let Some(tag) = hmac.as_ref() else {
        return DbResponse::Error {
            code: "hmac_required".into(),
            message: "set_superuser missing `hmac` field".into(),
        };
    };
    if !canon::verify_tag_hex(&session.hmac_key(), &canonical, tag) {
        return DbResponse::Error {
            code: "hmac_mismatch".into(),
            message: "set_superuser `hmac` does not match canonical input".into(),
        };
    }

    match admin.user_dir.set_superuser(&user, on, /* now_ns */ shamir_connect::common::time::UnixNanos::now().as_u64()) {
        Ok(_) => DbResponse::SuperuserSet { user, on },
        Err(e) => {
            let msg = e.to_string();
            let code = if msg.contains("not found") {
                "user_exists" // match this repo's existing convention for "target doesn't exist" if a more specific code already exists elsewhere — check before reusing "user_exists" verbatim, it may be semantically wrong here (that code means "already exists", not "doesn't exist"); pick whatever this codebase's actual convention is for "unknown user" errors (grep existing admin handlers)
            } else if msg.contains("last remaining superuser") {
                "invalid_owner" // or whatever this codebase's convention is for a refused-privileged-mutation; check ERR_INVALID_OWNER / similar constants already used by admin_access.rs's chown-lockout guard for a precedent to reuse rather than inventing a new string
            } else {
                "query"
            };
            DbResponse::Error { code: code.into(), message: msg }
        }
    }
}
```
The pseudocode above is illustrative, not exact — read the actual
existing error-code conventions in `db_handler/admin.rs`/`admin_access.rs`
before finalizing the code strings; consistency with the rest of the
codebase's vocabulary matters more than matching this brief literally.

Import the new function in `handler.rs`'s existing `use
super::admin::{...}` (or wherever `create_scram_user` is currently
imported from) alongside it.

## 7. Out of scope — do not touch

- `PrincipalResolver`/`UserAdminPort`, the `BatchOp::CreateUser`/
  `GrantRole`/etc. that route through shamir-db's own (currently
  ineffective) Store B tables, retiring those tables — task #559.
- Ticket v2 dropping `roles` entirely / resume re-verifying against the
  directory — task #558. This task keeps threading `roles` into the
  ticket exactly as today, just sourced from `state_by_user_id` instead
  of `lookup_roles`.
- Rust query-builder / TS client builder support for `SetSuperuser`
  (a `.setSuperuser()`-style convenience, HMAC helper on the TS side) —
  task #560's own scope explicitly includes "add setSuperuser". Tests
  for THIS task construct `DbRequest::SetSuperuser` directly (no
  builder exists yet) and compute the HMAC tag via
  `canonical_set_superuser` + `compute_tag_hex` directly, the same way
  wire-level tests already do for ops that predate their builder
  support.
- `crates/shamir-connect/src/server/bootstrap.rs`'s `BootstrapState`
  (`superuser_ever_existed`, `consume_bootstrap_token`) and
  `crates/shamir-server/src/server_meta.rs`'s mirror of it — investigated
  and confirmed these belong to a SEPARATE, currently-unwired
  "wire-triggered random-token bootstrap" mechanism
  (`consume_bootstrap_token` has zero production call sites today) that
  is entirely distinct from `ensure_superuser`'s live server-startup
  bootstrap path this task actually touches. Re-keying that dead
  mechanism to the new flag/count is not this task's job — do not touch
  `bootstrap.rs` (shamir-connect) or `server_meta.rs`.
- `repl_handler.rs`'s `has_role(REPLICATOR_ROLE)` check — confirmed
  unrelated (checks for `"replicator"`, not `"superuser"`), no change.

## Red tests required first (TDD)

1. **Reservation rejected** — `FjallUserDirectory::update_roles(username,
   vec!["superuser".to_string()], now_ns)` on an existing user returns
   `Err`, and the user's persisted `roles`/`superuser` are UNCHANGED
   afterward (confirm via `state_by_user_id`).
2. **`set_superuser` grants and revokes, bumps `tickets_invalid_before_ns`,
   maintains `superuser_count`** — grant on a non-superuser: `Ok(true)`,
   flag becomes `true`, tib bumps, `remove()`'s last-superuser guard now
   sees one more superuser (verify indirectly: create a second
   superuser, revoke the first, confirm `remove()`-style last-superuser
   semantics — or directly assert on `state_by_user_id` before/after).
3. **`set_superuser` refuses to revoke the last superuser** — mirrors
   #556's `remove_refuses_last_superuser_then_succeeds_with_two`
   pattern: with exactly one superuser, `set_superuser(name, false, ..)`
   returns `Err` and the flag stays `true`; with two superusers, revoking
   one succeeds.
4. **`set_superuser` is idempotent** — granting an already-superuser
   account (or revoking an already-non-superuser account) returns
   `Ok(false)`, no tib bump, no count change.
5. **`SetSuperuser` wire op end-to-end** (in `crates/shamir-server/tests/`,
   driving the real `ShamirDbHandler`/`RequestHandler` dispatch, not the
   directory directly): superuser session + correct hmac → succeeds,
   response is `SuperuserSet { user, on }`, and `state_by_user_id`
   reflects the change; missing hmac → `hmac_required`; wrong hmac →
   `hmac_mismatch`; non-superuser session → `permission_denied` (checked
   BEFORE the hmac check, matching `create_scram_user`'s ordering);
   revoking the last superuser (correct hmac, superuser session) still
   returns the typed refusal from `set_superuser`, not a silent success.
6. **Handshake wiring** — a real handshake for an account with the
   `superuser` flag set produces a `Session` whose
   `permissions.is_superuser == true` WITHOUT the role list containing
   the literal string `"superuser"` (confirms the switch away from
   role-string scanning actually took effect, not just that the old
   behavior still coincidentally works).
7. The relocated/replacement version of the #556 normalization test
   (see §1) — confirm it still passes and still proves the same
   idempotence property, now seeding through the in-crate bypass instead
   of the now-reserved `update_roles`.

## Definition of done

- `cargo check --workspace --all-targets` clean.
- `cargo fmt -p shamir-server -p shamir-connect -p shamir-query-types -- --check` clean on touched files.
- `cargo clippy -p shamir-server -p shamir-connect -p shamir-query-types --all-targets -- -D warnings` clean, modulo the already-tracked pre-existing issues (task #562's `read_planner.rs` type_complexity; the `hmac_tests.rs` octal_escapes lints) — confirm via `git diff --stat` neither touched file is yours if you hit either.
- `./scripts/test.sh -p shamir-server -p shamir-connect -p shamir-query-types --full` green, including all new/relocated tests.
- Repo-wide grep confirms no remaining PRODUCTION call site can write the literal string `"superuser"` into a directory-backed role list via `update_roles` (test fixtures using the new in-crate bypass are fine and expected).
- `bootstrap.rs`'s `ensure_superuser` still produces a working superuser
  account end-to-end (existing bootstrap tests, if any exercise login,
  must still pass) — just via the flag now, not the role string.

When done, produce a final summary (not a bare tool call) listing: every
file changed with a one-line description, the full text of every
new/relocated test, the gate command outputs, and any place this
brief's assumptions didn't match the actual code (with how you resolved
it) — in particular the exact error-code strings you chose for
`set_superuser`'s "user not found" / "last remaining superuser" cases
and why, and confirmation of whether `InMemoryUserDirectory`-backed
callers of `SessionPermissions::from_roles` needed any change.
