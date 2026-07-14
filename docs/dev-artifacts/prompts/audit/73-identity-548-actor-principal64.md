בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Brief: shamir-types identity primitive — `Actor::Admin` + `principal64`, delete `principal_id` (task #555)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Context

`docs/dev-artifacts/design/identity-privilege-unification-548-549-decision.md` §2.2,
§2.3 (already signed off by the project owner) is the source of truth
for this task's design — read it first. This brief is step #70 of that
design's phased plan (§7), renumbered to 73 since briefs 70-72 were
already claimed by tasks #552-554.

**The bug this closes:** `principal_id(username) = fxhash::hash64(username)
& i64::MAX` (`crates/shamir-types/src/access.rs:33-35`) derives identity
from a *name*, not an *account*. Drop user "alice", recreate a new
account also named "alice" — it gets the **same** `Actor::User(id)` as
the old one, silently inheriting every grant/ownership the old account
had. This is failure mode 3 recorded in the design doc §1.1.

**The fix, in two parts:**
1. `Session` already carries a real, per-account, directory-minted
   `user_id: [u8; 16]` (`crates/shamir-connect/src/server/session.rs:111`,
   populated at login — see `DbResponse::UserCreated`,
   `crates/shamir-server/src/db_handler/admin.rs:127-130`, which already
   returns the minted bytes). Today this field exists but nothing
   authorization-relevant reads it — `Session::principal_id()`
   (`session.rs:246-252`) computes a *username* hash instead, `session_actor`
   (`crates/shamir-server/src/db_handler/handler.rs:119-125`) calls that
   method. Wiring `session_actor` to read `session.user_id` instead closes
   the bug for every real wire session, because a fresh account gets fresh
   random/monotonic bytes from the directory regardless of what username
   it reuses — no #556 (directory v2) work is required for this to be true
   today.
2. The 16-byte `user_id` doesn't fit the catalogue's `i64` owner/group-member
   encoding. `principal64(user_id: [u8;16]) -> u64` is a **fixed 63-bit
   projection** (`u64::from_be_bytes(user_id[0..8]) & i64::MAX`), not a new
   hash — it's a pure truncation of already-unique bytes. See design doc
   §2.2 for why this (not widening the catalogue to 128 bits) was chosen.

## Exact changes

### 1. `crates/shamir-types/src/access.rs`

Add the projection function, right where `principal_id` currently lives:

```rust
/// Fixed 63-bit projection of a directory-minted 16-byte user id into the
/// catalogue's `i64`-safe integer space (owner / group-member encoding).
///
/// Pure truncation — NOT a hash of anything attacker-chosen. Uniqueness and
/// non-zero-ness are enforced once, at mint time, by the directory that
/// produces the 16 bytes (`FjallUserDirectory`, task #556) — this function
/// only projects. See `docs/dev-artifacts/design/identity-privilege-unification-548-549-decision.md`
/// §2.2 for the full rationale (why 63-bit projection over widening the
/// catalogue to 128 bits).
pub fn principal64(user_id: [u8; 16]) -> u64 {
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&user_id[0..8]);
    u64::from_be_bytes(buf) & (i64::MAX as u64)
}
```

Add a second, clearly-scoped-down helper — this is the answer to "what do
the two production call sites in `access_control.rs` that only have a
*username string* (no live `Session`, no directory lookup available yet —
that's task #559's `PrincipalResolver`) do once `principal_id` is gone?":

```rust
/// Interim, username-keyed bridge into the `principal64` id space.
///
/// **NOT the real identity primitive** — it is a deterministic hash of a
/// *name*, exactly what `principal64`'s real 16-byte projection is designed
/// to replace (see `principal_id`'s deletion, this same commit). It exists
/// ONLY for two classes of caller that cannot reach a real 16-byte
/// directory-minted id today:
///
/// 1. Two production call sites in `shamir-db`'s `access_control.rs` that
///    resolve a synthetic `/users/<name>` resource path (or list existing
///    usernames for `access_tree`) with no live `Session`/directory lookup
///    available — both are replaced by `PrincipalResolver` (task #559),
///    which resolves a REAL principal64 id via the directory. Do not add
///    new production call sites of this function; if you need a principal
///    id in new production code, you have a `Session` (use
///    `principal64(session.user_id)`) or you need `PrincipalResolver`.
/// 2. Test/bench fixtures that need a stable, per-name, non-colliding id
///    and do not care about mint-time randomness (they are not testing the
///    recreate-inherits-identity bug — that property is tested against
///    real `Session`/`user_id` bytes instead, see this task's red tests).
pub fn principal64_from_username(username: &str) -> u64 {
    let hash = fxhash::hash64(username);
    let mut user_id = [0u8; 16];
    user_id[0..8].copy_from_slice(&hash.to_be_bytes());
    principal64(user_id)
}
```

**Delete** `principal_id` (`access.rs:33-35`) entirely — not deprecated,
not `#[allow(dead_code)]`, gone.

Change the `Actor` enum (`access.rs:301-305`):

```rust
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Actor {
    #[default]
    System,
    /// A superuser session. Bypasses `permits()` exactly like `System`, but
    /// (unlike `System`) carries the real principal64 id, so
    /// `ResourceMeta::owned_enforced`/`to_owner_id()` attributes
    /// admin-created resources to their creator instead of collapsing them
    /// to `owner = 0 = System`. NEVER produced by `from_owner_id` — admin-ness
    /// is a live session property, never a persisted owner property; a
    /// persisted owner id round-trips to `Actor::User`, never `Actor::Admin`,
    /// even if that id happens to belong to an admin account.
    Admin(u64),
    User(u64),
}
```

Update `impl Actor`:
- `to_owner_id()`: add `Actor::Admin(id) => *id` (the REAL id, unlike
  `System`'s `OWNER_SYSTEM`).
- `from_owner_id()`: **unchanged** — still only ever produces `System` or
  `User`, never `Admin` (see the doc comment above; this is deliberate per
  design doc §2.3, "`from_owner_id` is unchanged").
- `Display`: add `Actor::Admin(id) => write!(f, "Admin({id})")`.

Update `permits()` (`access.rs:652-665`): the bypass check
`matches!(actor, Actor::System)` becomes
`matches!(actor, Actor::System | Actor::Admin(_))`. Everything after that
(the `Action::Manage` owner check, `class_of`/`Mode::is_set`) is unchanged —
`class_of` already works correctly for `Admin` because it only ever calls
`actor.to_owner_id()`, which now returns the real id for `Admin` too.

No other function in this file needs to change — `class_of`,
`ResourceMeta::owned_enforced`, `inject_into`, `to_query_value`,
`from_record` all go through `to_owner_id`/`from_owner_id`, which already
do the right thing once those two methods are updated above.

### 2. `crates/shamir-connect/src/server/session.rs`

**Delete** `Session::principal_id()` (lines 246-252) entirely.

### 3. `crates/shamir-server/src/db_handler/handler.rs`

Rewrite `session_actor` (lines 113-125) and its doc comment:

```rust
/// Resolve the [`Actor`] for the current session.
///
/// Superuser sessions get `Actor::Admin(principal64(session.user_id))` —
/// bypasses the Shomer gate exactly like `Actor::System`, but attributes
/// ownership of admin-created resources to the real account instead of
/// collapsing to `owner = 0`. Regular sessions get
/// `Actor::User(principal64(session.user_id))`. `user_id` is the directory-
/// minted 16-byte id stamped on the session at login — NOT derived from the
/// username — so a dropped-and-recreated account gets a fresh id even if it
/// reuses the same name (closes the identity-inheritance-on-recreate bug,
/// design doc §1.1 finding 3).
pub(super) fn session_actor(session: &Session) -> Actor {
    let id = shamir_types::access::principal64(session.user_id);
    if session.permissions.is_superuser {
        Actor::Admin(id)
    } else {
        Actor::User(id)
    }
}
```

(Adjust the `use`/path to however `principal64` is already imported in
this file's other `shamir_types::access::*` references — match the
existing import style, don't introduce a new one gratuitously.)

### 4. `crates/shamir-db/src/shamir_db/shamir_db/access_control.rs` — the two production call sites

**Call site A** (`ResourcePath::User { name }` meta arm, around line
159-166): replace

```rust
ResourcePath::User { name } => Ok(ResourceMeta {
    // principal_id, NOT principal64 — task #555 (Actor::Admin/
    // principal64) hasn't landed yet; this call site gets a
    // one-line swap when it does.
    owner: Actor::User(principal_id(name)),
    group: None,
    mode: 0o750,
}),
```

with

```rust
ResourcePath::User { name } => Ok(ResourceMeta {
    // Interim: principal64_from_username, NOT a real directory lookup —
    // there is no live Session/PrincipalResolver at this call site to
    // resolve `name` to its real minted id. Replaced by a real
    // PrincipalResolver-backed lookup in task #559; until then this
    // preserves today's exact behavior (a name-keyed synthetic owner)
    // just routed through the new principal64 projection instead of the
    // now-deleted principal_id.
    owner: Actor::User(principal64_from_username(name)),
    group: None,
    mode: 0o750,
}),
```

**Call site B** (the `access_tree` principals loop, around line 887-895 —
grep for `principal_id(uname)`): same mechanical swap,
`principal_id(uname)` → `principal64_from_username(uname)`. Same
"interim, replaced by `PrincipalResolver` in #559" rationale — add a
one-line comment at that call site too if there isn't already a nearby
one (there isn't; add one).

Update the `use` import at the top of this file
(`use shamir_types::access::{..., principal_id, ...}` around line 11) to
import `principal64_from_username` instead of `principal_id`.

### 5. Compile-driven sweep (~25 call sites)

After steps 1-4, `cargo check --workspace --all-targets` will name every
remaining broken call site. They fall into three categories — **use the
right one per site, don't default to one pattern everywhere**:

**(a) Direct `Actor::User(principal_id(name))` fixture construction** (no
`Session` involved) — the majority of the sweep: benches
(`shamir-server/benches/subscription_delivery.rs`,
`subscription_fanout.rs`, `subscription_throughput.rs`,
`wire_latencies.rs`, `wire_pipelining.rs`), and test files
(`shamir-db/src/shamir_db/tests/access_tree_tests.rs`,
`admin_access_validation_tests.rs`, `root_user_group_meta_tests.rs`,
`shamir-db/tests/create_function_gating.rs`,
`shamir-server/src/db_handler/tests/node_mode_tests.rs`,
`repl_handler_tests.rs`, `shamir-server/src/replication/tests/
follower_loop_tests.rs`, `supervisor_tests.rs`,
`shamir-server/tests/repl_convergence_e2e.rs`). Mechanical swap:
`principal_id(name)` → `principal64_from_username(name)`, import
`shamir_types::access::principal64_from_username` (or
`shamir_db::access::principal64_from_username`, whichever re-export path
the file already uses for `Actor`) in place of `principal_id`.

**(b) `Session::new(fixed_bytes, name, ...)` fixtures whose test then calls
the now-deleted `.principal_id()`** — e.g.
`shamir-server/tests/db_handler.rs`'s `named_user_session` /
`shomer_dac_denies_non_owner_through_handler_wire` (search for
`.principal_id()` call sites after step 2 deletes the method — compile
errors will point at all of them). **Do NOT touch the `Session::new(...)`
constructor call's first argument** (the fixed 16-byte array, e.g.
`[0xCC; 16]`) — it is already a legitimate directory-style id, just
unused by the old code. Only replace the `.principal_id()` method call
with the free function `shamir_types::access::principal64(session.user_id)`
(or the crate's existing re-export path for it).

**(c) `crates/shamir-server/tests/permission_e2e.rs` — special-case, read
carefully:**
- The local mirror function (lines 720-723,
  `fn principal_id(username: &str) -> u64 { fxhash::hash64(username) &
  (i64::MAX as u64) }`) must be **deleted**, not swapped to
  `principal64_from_username` — unlike every other sweep site, this one
  computes the id used as the wire-level target for a REAL
  `add_group_member` call against a REAL account ("bob") created earlier
  in the same test via `DbRequest::CreateScramUser` (line ~970). Under the
  new model "bob"'s real principal id is `principal64(bob's minted
  user_id)`, which has nothing to do with hashing the string `"bob"`.
- Fix: `DbResponse::UserCreated { name, user_id }` already returns the
  minted bytes (`crates/shamir-server/src/db_handler/admin.rs:127-130`,
  `user_id: Vec<u8>`). In the `for (name, pw) in [("bob", ...), ("carol",
  ...)]` creation loop (~line 968), capture the `UserCreated` response's
  `user_id` for `"bob"` specifically (e.g. into a
  `let mut bob_user_id: Option<Vec<u8>> = None;` set inside the loop when
  `name == "bob"`), convert the `Vec<u8>` to `[u8; 16]`
  (`.try_into().expect("user_id is 16 bytes")`), and replace
  `let bob_pid = principal_id("bob");` (line 1016) with
  `let bob_pid = shamir_types::access::principal64(bob_user_id_bytes);`.
- Grep this file for any OTHER call site of the local `principal_id(...)`
  beyond the two already named (720-723 definition, 1016 call) — fix each
  the same way (resolve via the real `UserCreated` response for that
  username, never via a name hash) if any exist.

**(d) `crates/shamir-types/src/tests/access_tests.rs`** — three existing
tests exercise `principal_id`'s own properties
(`principal_id_is_deterministic_and_distinct`,
`principal_id_always_fits_i64`,
`principal_id_round_trips_through_actor_owner_id`, lines 121-157).
Replace them with equivalent tests of `principal64` (same three
properties — deterministic for the same 16-byte input, fits i64, round-trips
through `Actor::from_owner_id` — use fixed byte arrays like `[0xAB; 16]`
instead of names as input, since `principal64` takes bytes not strings).
Also extend `actor_owner_round_trip` (line 113-119) with an `Actor::Admin`
case: `Actor::Admin(42).to_owner_id() == 42`, and confirm
`Actor::from_owner_id(42)` is `Actor::User(42)` — **not** `Actor::Admin` (the
non-round-trip property called out in this brief's `Actor::Admin` doc
comment above — this is a real invariant worth a real test, not just a
comment).

## Red tests required first (TDD)

Write these to FAIL against the current code, then make them pass:

1. **`recreate_same_username_gets_different_actor`** (put it in
   `crates/shamir-server/src/db_handler/tests/` — wherever `session_actor`'s
   existing tests live, or create that location if none exists yet;
   check first) — build two `Session`s with the SAME `username` but
   DIFFERENT `user_id` byte arrays (simulating: drop the account, recreate
   one with the same name — the directory mints fresh bytes either way),
   assert `session_actor(&session_a) != session_actor(&session_b)`. This
   is the direct regression test for the bug this task closes — under the
   OLD code (`session_actor` reading `session.principal_id()`, i.e. a
   username hash) this would have produced the SAME actor for both
   sessions; after the fix it must not.
2. **`admin_created_resource_owner_is_admin_id_not_zero`** (put it in
   `crates/shamir-types/src/tests/access_tests.rs` or a `shamir-db`
   integration test, whichever this brief's own scope naturally reaches —
   prefer the `shamir-types` unit level since it's really testing
   `ResourceMeta::owned_enforced`/`to_owner_id`, not any wiring) — build
   `ResourceMeta::owned_enforced(Actor::Admin(777))`, assert
   `.owner.to_owner_id() == 777` (not `OWNER_SYSTEM`/`0`). Contrast with
   `ResourceMeta::owned_enforced(Actor::System)` still giving
   `to_owner_id() == 0` — the whole point is `Admin` is real ownership,
   `System` stays anonymous.

## Out of scope — do not touch

- `crates/shamir-server/src/server/admin.rs`'s `InMemoryUserDirectory`
  (monotonic-counter id minting) and `FjallUserDirectory` — task #556.
  This task does not change how `user_id` bytes are minted, only how
  they're *projected* into the POSIX model once minted.
- `PersistedUser`, the `principal64 → username` reverse keyspace, boot
  normalization — task #556.
- `SessionPermissions`/superuser-as-flag, `SetSuperuser` — task #557.
- Ticket v2 / resume — task #558.
- `PrincipalResolver`/`UserAdminPort`, retiring shamir-db's own
  users/roles tables, replacing the two `principal64_from_username`
  interim call sites with a real resolver lookup — task #559 (this is
  explicitly where `principal64_from_username`'s two production call
  sites get removed; do not attempt that removal here).
- Anything in `crates/shamir-client-ts` or `crates/shamir-query-builder` —
  this task is server/engine-internal identity plumbing, no wire format
  changes.

## Definition of done

- `cargo fmt -p shamir-types -p shamir-connect -p shamir-server -p shamir-db -- --check` clean on touched files (pre-existing drift elsewhere is not your concern — note it if you hit it, per the standard convention this campaign already follows).
- `cargo clippy -p shamir-types -p shamir-connect -p shamir-server -p shamir-db --all-targets -- -D warnings` clean, modulo the two already-tracked pre-existing issues (task #562's `read_planner.rs` `type_complexity`, and the `hmac_tests.rs` `octal_escapes` lints) — confirm via `git diff --stat` neither touched file is yours, then re-run without `-D warnings` to show zero new warnings if you hit either.
- `cargo check --workspace --all-targets` clean (this task's blast radius is wide — benches and every crate downstream of `shamir-types` must still compile).
- `./scripts/test.sh -p shamir-types -p shamir-connect -p shamir-server -p shamir-db --full` green, including the 2 new red-tests-now-green and the 3 replaced `access_tests.rs` tests.
- `principal_id` (both the `shamir-types` free function and
  `Session::principal_id()`) no longer exist anywhere in the workspace —
  confirm with a repo-wide grep before finishing.
- Every one of the ~25 swept call sites uses the category-appropriate
  replacement from §5 above, not a one-size-fits-all substitution.

When done, produce a final summary (not a bare tool call) listing: every
file changed with a one-line description, the full text of the 2 new red
tests plus the 3 rewritten `access_tests.rs` tests, the gate command
outputs, and any place this brief's assumptions didn't match the actual
code (with how you resolved it).
