בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Brief: retire shamir-db's Store B — PrincipalResolver / UserAdminPort seam (task #559)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Context — read this whole section before touching any code

`docs/dev-artifacts/design/identity-privilege-unification-548-549-decision.md` §3
(already signed off) is the source of truth for this task's design —
read it in full first, especially §3.1 (exact trait shapes, quoted
below) and §6 item 3 (why NO auto-import from Store B happens). This
brief is step #74 of that design's phased plan (§7), renumbered to 77
since briefs 70-76 were claimed by #552-558.

**This is explicitly the largest and riskiest step in the whole
#548/#549 chain** (the design doc's own words: "the largest, should
land alone"). Take your time. If you find yourself needing more than
one turn to finish, that's expected — do NOT rush a shortcut through
any of the sections below to finish faster.

**The problem being solved:** shamir-db has its OWN, completely
separate "users"/"roles" tables (`crates/shamir-db/src/shamir_db/execute/admin_users_roles.rs`
— read this file in full, it's short, ~770 lines, every handler is
already fully understood and quoted below for reference) that are
**historically ineffective** — confirmed by every prior task in this
campaign: no login, resume, or authorization gate has EVER read this
store. It exists only to *look like* user administration. Meanwhile
`crates/shamir-server/src/user_directory.rs`'s `FjallUserDirectory` IS
the real, durable, actually-consulted SCRAM directory (already
extended by tasks #556/#557/#558 with a `principal64` keyspace,
`remove()`, `set_superuser()`, `state_by_user_id()`).

This task builds the seam between them: shamir-db becomes fully
identity-agnostic (consumes opaque `principal64` ids only), and gains
two narrow injected trait objects — a read-only `PrincipalResolver`
and a write-side `UserAdminPort` — implemented by shamir-server over
the real directory. shamir-db's own users/roles tables are RETIRED
from the write path entirely (no code deletes the tables/records
themselves — this is not a data migration — but nothing writes to them
via these ops anymore; see the boot-audit requirement in §7 below for
the read-only visibility given to operators).

## 1. Define the two seam traits

Per design doc §3.1 (quoted verbatim — follow this exactly):

```rust
// shamir-db (or shamir-types) — implemented by the embedding layer.
pub trait PrincipalResolver: Send + Sync {
    fn resolve(&self, principal64: u64) -> Option<PrincipalInfo>; // name, user_id, db scope, superuser
    fn list(&self) -> Vec<PrincipalInfo>;                          // access_tree / List introspection
}
```
```rust
pub trait UserAdminPort: Send + Sync {
    async fn create_user(&self, name, password, roles, database) -> Result<[u8; 16]>;
    async fn drop_user(&self, name) -> Result<bool>;
    async fn grant_role(&self, user, role) -> Result<()>;
    async fn revoke_role(&self, user, role) -> Result<()>;
    async fn set_superuser(&self, user, on: bool) -> Result<()>;
}
```

**Where to put them:** define both in `shamir-db` (a new module,
e.g. `crates/shamir-db/src/shamir_db/ports.rs` — pick a location
consistent with this crate's existing module layout; check first).
`shamir-db` already depends on `shamir-types` (for `Actor` etc.) but
NOT on `shamir-server` — and must never depend on it (`shamir-server`
depends on `shamir-db`, not the reverse; this is the whole point of
the seam). `shamir-server` will `impl UserAdminPort for ...` /
`impl PrincipalResolver for ...` on a new type wrapping
`Arc<FjallUserDirectory>`.

**`PrincipalInfo` shape** (not fully spelled out in the design doc
snippet — derive it from context: "name, user_id, db scope,
superuser" plus the projection key itself, since `list()` returns a
`Vec` with no external key to match entries against):
```rust
#[derive(Debug, Clone)]
pub struct PrincipalInfo {
    pub principal64: u64,
    pub name: String,
    pub user_id: [u8; 16],
    pub database: Option<String>,
    pub superuser: bool,
}
```

**`UserAdminPort::create_user`'s exact parameter types** — match the
existing wire op's shape where sensible: `name: &str` (or `String`),
`password: &str`/`Zeroizing<Vec<u8>>` (plaintext — Argon2id derivation
happens INSIDE the port impl, see §3 below), `roles: Vec<String>`,
`database: Option<String>`. Return `Result<[u8; 16], E>` for some
appropriate error type each method already uses in this crate (check
`shamir_connect::common::error::Error` / this crate's own error
conventions — do not invent a new error type if an existing one fits).
`drop_user`/`grant_role`/`revoke_role`/`set_superuser` return
`Result<bool>` / `Result<()>` per the design doc snippet — match it.

Both traits are `#[async_trait::async_trait]` (this workspace already
uses `async_trait` elsewhere, e.g. `AdminExecutor` in
`admin_dispatch.rs` — mirror that exact pattern).

## 2. `ShamirDb` gains two optional injected ports

`ShamirDb` (`crates/shamir-db/src/shamir_db/shamir_db/core.rs`) is a
cheap-clone, `Arc`-backed struct (read it — every field is already
`Arc<...>`-wrapped, with `pub fn accessor(&self) -> &Arc<...>` methods,
e.g. `admin_user_locks()`). Add two new fields following this EXACT
existing pattern:
```rust
pub(super) user_admin_port: Option<Arc<dyn UserAdminPort>>,
pub(super) principal_resolver: Option<Arc<dyn PrincipalResolver>>,
```
plus accessor methods (`pub fn user_admin_port(&self) -> Option<&Arc<dyn UserAdminPort>>`,
similarly for the resolver). Both default to `None` in every existing
constructor (`ShamirDb::init_memory()`, `init(...)`, whatever this
crate's actual constructors are called — find them, there should only
be a small number). Add builder methods to set them post-construction
(mirroring whatever builder-style method this crate already uses for
optional cross-cutting config, if one exists — e.g. something like
`.with_user_admin_port(port: Arc<dyn UserAdminPort>) -> Self` /
`.with_principal_resolver(...)`; check the actual constructor style
in `core.rs` before inventing a shape that doesn't fit).

`ShamirAdminExecutor` (`admin_dispatch.rs`) already holds
`pub(super) shamir: ShamirDb` — so `self.shamir.user_admin_port()`/
`self.shamir.principal_resolver()` are reachable from every handler in
`admin_users_roles.rs`/`access_control.rs` without further plumbing.

## 3. Re-target `handle_create_user`/`handle_drop_user`/`handle_grant_role`/`handle_revoke_role`

`crates/shamir-db/src/shamir_db/execute/admin_users_roles.rs` — **the
authorization gates stay EXACTLY where they are** (`authorize_user_lifecycle`
for create/drop, `Manage(Root)` for grant/revoke) — only the
STORAGE/EXECUTION half of each handler changes, from reading/writing
shamir-db's own `users_table()`/`roles_table()` to calling the port.

**`handle_create_user`** — after the existing
`authorize_user_lifecycle` call succeeds, instead of hashing the
password and writing to `users_table()`, call:
```rust
match self.shamir.user_admin_port() {
    Some(port) => {
        let user_id = port
            .create_user(&op.create_user, op.password.reveal(), op.roles.clone(), op.database.clone())
            .await
            .map_err(|e| err(e.to_string()))?;
        Ok(admin_result(mpack!({
            "created_user": @(QueryValue::Str(op.create_user.clone())),
        })))
    }
    None => Err(err_code("not_supported", "user administration is not configured on this server".to_string())),
}
```
(Argon2id hashing moves OUT of this handler entirely — shamir-db never
touches SCRAM crypto per the design doc. The port impl, in
shamir-server, does the derivation, reusing the existing
`create_scram_user` derivation code path in
`crates/shamir-server/src/db_handler/admin.rs:69-104` — factor that
Argon2id-derivation-then-`FjallUserDirectory::insert` logic into a
method the port impl calls, don't duplicate the spawn_blocking Argon2id
code a third time.)

**`CreateUserOp`'s `profile` field** (currently read at
`admin_users_roles.rs:53`, `user.profile = op.profile.clone()`) has NO
consumer in the real directory (`PersistedUser` has no `profile`
field, and the design doc's port signature doesn't carry one). Confirm
via grep that `profile` has no OTHER consumer anywhere in the
workspace, then remove the field from `CreateUserOp`'s wire type
entirely (clean-cutover posture, consistent with this campaign's
established practice) — if you find ANY other consumer, stop and
report it in your final summary rather than silently keeping or
dropping it.

**`handle_drop_user`** — after resolving the target's scope (this
scope lookup ITSELF needs to change — see below) and the
`authorize_user_lifecycle` call, call `port.drop_user(&op.drop_user)`.
The scope resolution currently reads shamir-db's OWN `users_table()`
to find the target's `database` field (lines 152-174) — this must
instead come from the PrincipalResolver (`resolver.resolve(principal64_of(name))`
— but resolving requires a principal64 key, and this call site only
has a USERNAME; you need a name→principal64 path. Check whether
`PrincipalResolver` needs a `resolve_by_name` variant, OR whether it's
simpler to have `resolve_by_name` be a DEFAULT-implemented convenience
on the trait that internally calls `list()` and linear-scans for a
name match (acceptable given `list()` is already O(N) and admin ops are
low-frequency) — pick whichever is cleaner, document the choice in
your final summary. **If no resolver is installed**: scope resolves to
`None` (per design doc §3.1's explicit fallback — "Absent resolver...
scope-delegation unavailable" — meaning only a global admin, never a
database-owner, can drop a user when no resolver is wired; this is the
documented degraded-but-safe behavior, not a bug to work around).

**`handle_grant_role`/`handle_revoke_role`** — after the existing
`Manage(Root)` check, call `port.grant_role(&op.user, &op.grant_role)` /
`port.revoke_role(&op.user, &op.revoke_role)` instead of the
read-mutate-write against `users_table()`. **Preserve the existing
per-user `admin_user_locks()` mutex acquisition** around the port call
if the port itself doesn't already serialize concurrent grants for the
same user internally — check `FjallUserDirectory::update_roles`'s own
`write_lock` (it already serializes at the directory level, per
task #556/#557), so the shamir-db-level `admin_user_locks()` acquisition
may now be REDUNDANT (harmless double-locking, not a correctness bug)
— your call whether to keep it for defense-in-depth or remove it as
dead weight; note your choice.

**No `not_found`-style special-casing needed for grant/revoke beyond
what the port itself returns** — `FjallUserDirectory::update_roles`
already returns `Err("user not found")` for the read-modify-write path
(it errors, per its own existing behavior at `crates/shamir-server/src/user_directory.rs`'s
`read_modify_write` — confirm this by reading the actual current
code) — surface that through the port's `Result`.

**Where's `set_superuser`'s wire op?** It already exists as
`DbRequest::SetSuperuser` (task #557) with its OWN handler in
`crates/shamir-server/src/db_handler/admin.rs` that ALREADY calls
`admin.user_dir.set_superuser(...)` directly (not through a BatchOp,
not through shamir-db at all — it's a top-level `DbRequest`, per
#557's design). **Do NOT change that handler in this task** — the
`UserAdminPort::set_superuser` method exists on the trait for
COMPLETENESS/symmetry and for any FUTURE shamir-db-internal caller
that might need it (there is none today), but the live wire path for
`SetSuperuser` stays exactly as #557 built it. If you find yourself
tempted to route the existing `SetSuperuser` handler through the new
port "for consistency," don't — that would be scope creep past what
this task's own brief asks for, and #557's handler already works and
is already tested.

## 4. Delete `CreateRole`/`DropRole`/`RenameRole` from the wire surface

Per the design: "a role becomes a plain string label on users in the
directory — no role object exists anymore, dissolving the
RenameRole-can-rename-superuser question rather than needing a guard."

This is a HARD compile-dependency chain — deleting the `BatchOp`
variants breaks anything that constructs them. Delete, in this exact
order (or verify the compiler catches every site if you delete
top-down):
1. `crates/shamir-query-types/src/batch/batch_op.rs` — remove the
   `CreateRole`/`DropRole`/`RenameRole` variants from the `BatchOp`
   enum, their serialize/deserialize match arms, and their entries in
   `is_admin()`/`is_write()`/`required_access()`.
2. `crates/shamir-query-types/src/admin/types/*.rs` — delete
   `CreateRoleOp`/`DropRoleOp`/`RenameRoleOp` struct definitions.
3. **`crates/shamir-query-builder`'s Rust builder functions for these
   three ops** — this is IN SCOPE for #559, not deferred to #560 (a
   hard compile dependency: the query-builder crate references the
   now-deleted `BatchOp` variants and cannot compile otherwise). Find
   and delete the `ddl::create_role`/`drop_role`/`rename_role`-style
   builder functions (check the exact names — mirror how `create_user`/
   `grant_role` are named there for the naming convention). TS client
   builder cleanup (a SEPARATE, non-compile-coupled concern) stays
   deferred to task #560 as originally scoped — do not touch
   `crates/shamir-client-ts` in this task.
4. `crates/shamir-db/src/shamir_db/execute/admin_users_roles.rs` —
   delete `handle_create_role`/`handle_drop_role`/`handle_rename_role`
   entirely (their bodies read/write `roles_table()`, which is being
   retired from the write path along with them).
5. `crates/shamir-db/src/shamir_db/execute/admin_dispatch.rs` — remove
   the `BatchOp::CreateRole(op) => ...` / `DropRole` / `RenameRole`
   match arms from `execute_admin`.
6. `crates/shamir-query-types/src/hmac.rs` — remove
   `canonical_create_role`/`canonical_drop_role` (grep for a
   `canonical_rename_role` too — if one exists, remove it; if
   `RenameRole` never had an HMAC canonical form, note that in your
   summary rather than assuming one should exist).
7. `crates/shamir-server/src/db_handler/admin.rs`'s
   `check_destructive_hmacs` — remove the match arms for
   `CreateRole`/`DropRole`/`RenameRole` (if present — the earlier
   research pass found the function's DOC COMMENT lists `DropRole`
   among covered ops but did not confirm the exact match arm text; read
   the actual current function body and remove whatever arms
   reference the now-deleted variants).
8. **Also delete `ListOp::Roles`** (`crates/shamir-query-types/src/admin/types/list_ops.rs`)
   and its handler in `crates/shamir-db/src/shamir_db/execute/admin_list.rs`
   (`ListOp::Roles`'s current handler reads shamir-db's `roles_table()`
   — since role-objects no longer exist, there is nothing coherent left
   to list under this name; this is a clean, consistent deletion
   alongside the BatchOp removal, not a separate design decision).
9. Repo-wide grep for `CreateRole`/`DropRole`/`RenameRole`/`roles_table`/
   `ListOp::Roles` afterward to confirm nothing references the deleted
   surface (test files that exercised these ops will need their tests
   REMOVED, not adapted — there is no replacement behavior to test
   against; if a test exercises a genuinely different, still-relevant
   property alongside the deleted op, extract just that property into a
   new/adjusted test rather than deleting indiscriminately).

**`GrantRole`/`RevokeRole` are RETAINED** (they already only reference
a bare role STRING, `op.grant_role`/`op.revoke_role`, never a role
object — no shape change needed, just re-targeted per §3 above).

## 5. `PersistedUser` gains a `database: Option<String>` field

Per design doc §6 item 3: "The `database` scope field (the only
Store-B datum with live enforcement meaning, via
`authorize_user_lifecycle`) moves *schema-wise* to the directory
record; existing scoped users, if any deployment has them, appear in
the audit diff for manual re-creation" (i.e. NOT auto-imported — see
§7 below).

`crates/shamir-server/src/user_directory.rs`'s `PersistedUser` gains:
```rust
/// Database-scope for owner-delegation (authorize_user_lifecycle).
/// `#[serde(default)]` — no pre-#559 persisted blob has this field.
/// Set only via `UserAdminPort::create_user`'s `database` parameter;
/// NOT auto-imported from shamir-db's retired Store B (design doc §6.3
/// — importing risks silently RE-GRANTING a stale scoped-admin
/// privilege that was never actually enforceable before this task).
#[serde(default)]
database: Option<String>,
```
Thread it through `PersistedUser::from_record` (gains a `database`
parameter, mirroring how `superuser: bool` was added in #556), and
expose it via `state_by_user_id`'s `UserDirectoryState` (gains a
`database: Option<String>` field) so the `PrincipalResolver` impl (in
shamir-server, wrapping `FjallUserDirectory`) can read it.

## 6. `PrincipalResolver`/`UserAdminPort` implementations in shamir-server

New file (or extend `user_directory.rs` if that fits better — your
call, note which you chose): a thin adapter type, e.g.
```rust
pub struct DirectoryPorts(pub Arc<FjallUserDirectory>);
```
implementing both traits by delegating to `FjallUserDirectory`'s
existing (or newly-added, see below) methods:

- **`PrincipalResolver::resolve(principal64)`** — needs a NEW
  `FjallUserDirectory` method (since no existing method takes a
  `principal64` key directly): look up username via the
  `principal64_to_name_v1` keyspace (built in #556 specifically for
  this consumer — read `user_directory.rs`'s existing
  `read_username_by_user_id`-style helper for the exact pattern to
  mirror against the NEW keyspace instead of `user_id_to_name_v1`),
  then load the full record the same way `state_by_user_id` does.
  Name this new method something like `resolve_by_principal64` — add
  it to `FjallUserDirectory` itself (not just the adapter), following
  the exact same shape/error-handling conventions as `state_by_user_id`.
- **`PrincipalResolver::list()`** — needs a NEW `FjallUserDirectory`
  method that iterates the `users_v1` keyspace once, decoding every
  `PersistedUser` into a `PrincipalInfo` (projecting via
  `shamir_types::access::principal64(user_id)` for the `principal64`
  field). This is an O(N) full-directory scan — acceptable, this
  mirrors the existing `access_tree`/`List` introspection cost model
  exactly (both are already O(N) over all principals today).
- **`UserAdminPort::create_user`** — derives Argon2id (reusing/factoring
  the existing derivation from `create_scram_user`, see §3 above), then
  calls `FjallUserDirectory::insert` + `update_roles` (if `roles` is
  non-empty) — mirror `bootstrap.rs`'s existing `insert_superuser`
  two-step pattern, generalized to accept the caller's `roles` and
  thread `database` into the initial record (may need
  `FjallUserDirectory::insert`'s signature to gain a `database`
  parameter, or a follow-up call — your call on the cleanest shape,
  note your choice). Reservation of `"superuser"` in `roles` is ALREADY
  enforced inside `update_roles` (task #557) — nothing extra needed
  here.
- **`UserAdminPort::drop_user`** → `FjallUserDirectory::remove` (task
  #556 — already has the last-superuser guard built in).
- **`grant_role`/`revoke_role`** → read current roles via
  `state_by_user_id`, compute the new list (push/retain), call
  `update_roles`. (Simpler than `FjallUserDirectory` needing dedicated
  grant/revoke methods — but if you find `update_roles`'s
  read-modify-write shape doesn't compose cleanly for an
  add-one/remove-one operation from OUTSIDE the directory struct
  (i.e., no atomic "add-if-absent" primitive), consider whether
  `FjallUserDirectory` should gain small
  `grant_role`/`revoke_role` methods of its OWN — mirroring
  `set_superuser`'s bespoke-method precedent — rather than doing a
  read-then-write with a TOCTOU gap in the adapter. **Prefer the
  directory-side atomic method** if there's any real concurrent-write
  risk; don't introduce a new race just to keep the adapter thin.
- **`set_superuser`** → `FjallUserDirectory::set_superuser` directly
  (trivial passthrough, for trait completeness per §3's note above).

## 7. Re-point `access_tree` and `List` through the resolver

`crates/shamir-db/src/shamir_db/shamir_db/access_control.rs` — the TWO
interim `principal64_from_username(name)` call sites (task #555's
explicitly-documented bridge, meant to be replaced exactly by THIS
task — read both call sites' existing comments, which literally say
"replaced by PrincipalResolver in task #559"):
1. The `ResourcePath::User { name }` meta-resolution arm.
2. The `access_tree` principals-listing loop.

For BOTH: if `self.principal_resolver()` is `Some(resolver)`, use it
(resolve-by-name for #1 — see the `resolve_by_name` discussion in §3
above; `list()` for #2, building the `"principals": {"users": [...]}`
display structure from real `PrincipalInfo` entries instead of the
Store-B users list). **If no resolver is installed**, fall back to
the CURRENT `principal64_from_username` behavior for #1 (keeps
embedded/no-directory deployments working, per design doc's own
"absent resolver → names resolve to null" framing — actually re-read
that framing: it says names resolve to `null`, not to the OLD
hash-based bridge — so the correct absent-resolver behavior is
`owner_name: None`/no synthetic owner at all, NOT continuing to use
`principal64_from_username`. Get this right: absent resolver means
DEGRADE to no-name-resolution, not keep the interim hack alive
forever).

`crates/shamir-db/src/shamir_db/execute/admin_list.rs`'s
`ListOp::Users` handler — re-point from reading shamir-db's
`users_table()` to `principal_resolver().list()` (if installed), typed
`not_supported` (or an empty list — pick whichever matches this
handler's existing style for a similarly-degraded case; check how
other List handlers behave when their backing data is absent) if no
resolver is installed.

## 8. One-time boot audit (read-only, WARN-log only, no auto-apply)

Per design doc §6 item 3: a one-time boot-time audit that diffs
shamir-db's Store B against the directory and logs WARN lines for:
usernames present only in one store, role sets that diverge, phantom
`superuser` grants in Store B that were never live — each with an
explicit "these had no effect and were not applied" trailer.
Log-only, no auto-apply, removable after one release (a comment
marking it as such is sufficient — no actual removal mechanism needed
in this task).

This runs ONCE at server boot, in `shamir-server`'s server-launcher
(`crates/shamir-server/src/server/server_launcher.rs` — same file that
already wires `FjallUserDirectory::open`/`AdminGlue`/`ShamirDbHandler`,
read the existing wiring sequence there first): after both are
constructed, before (or after — pick whichever is more natural given
the existing boot sequence) the handler starts serving, read shamir-db's
Store B users/roles tables directly (via whatever the simplest
read path is — a raw `ShamirDb::execute_as(Actor::System, ...)`
`ListOp::Users` call against a bootstrap `ShamirDb` instance, OR direct
system-store table reads if that's simpler given what's already
in scope at that point in `server_launcher.rs`) and the directory's
`PrincipalResolver::list()`, diff them, and `tracing::warn!`/`log::warn!`
(match whatever this file already uses) one line per divergence found.
Keep this genuinely minimal — a straightforward set-difference/field-compare,
not a general reconciliation engine.

## Out of scope — do not touch

- TS client builder changes, and the Rust query-builder's `setSuperuser`
  convenience method (if one doesn't already exist) — task #560's
  explicit scope.
- `chown`/`chgrp`/`addGroupMember` target-resolves-to-a-real-principal
  validation (the #543 follow-up the design doc names as a
  `PrincipalResolver` consumer) — that's task #561's job, which
  consumes the SAME resolver this task builds but is a separate,
  narrower change to `admin_access.rs`'s validation logic. Do not
  pre-emptively wire that validation in this task.
- `SetSuperuser`'s existing wire handler (`crates/shamir-server/src/db_handler/admin.rs`) — already correct from #557, do not touch (see §3's explicit note above).
- Ticket/resume logic — already correct from #558.

## Red tests required first (TDD)

Given the size of this task, aim for thorough coverage across each
numbered section above rather than a fixed small list — at minimum:

1. **`create_user`/`drop_user`/`grant_role`/`revoke_role` route through
   the port and actually persist to the REAL directory** — a wire-level
   test (mirror the pattern in `crates/shamir-server/tests/set_superuser_wire.rs`
   from #557) driving `ShamirDbHandler`'s real batch dispatch, confirming
   the directory (`FjallUserDirectory::state_by_user_id`) reflects the
   change — NOT that shamir-db's own Store B table changed (the OLD,
   now-wrong assertion).
2. **Without an installed port, these four ops return `not_supported`**
   — and do NOT silently fall back to writing Store B (the retirement
   must be a hard behavioral cutover, not a soft one where the old path
   quietly still works if a caller "forgets" to install the port).
3. **`CreateRole`/`DropRole`/`RenameRole`/`ListOp::Roles` no longer
   parse/dispatch** — a wire request naming one of these ops fails at
   deserialization (unknown variant) or dispatch, not silently
   succeeds against dead code.
4. **`access_tree`/`ListOp::Users` reflect the resolver's data** when a
   resolver is installed, and degrade correctly (no synthetic owner /
   empty-or-`not_supported`, per §7's exact framing above) when absent.
5. **The boot audit logs but never mutates** — seed BOTH stores with
   deliberately-divergent data (a user in Store B not in the directory,
   a role-string mismatch), boot, assert a WARN was logged (capture via
   whatever test-logging-capture mechanism this workspace already uses
   elsewhere, if any) AND assert neither store's data changed as a
   result.
6. **Database-scope owner-delegation still works end-to-end** —
   `authorize_user_lifecycle`'s database-owner path, now resolving scope
   via the resolver instead of Store-B's `users_table()`, still lets a
   database owner manage users scoped to their own database (this is a
   MEANING-PRESERVING refactor of the scope lookup, not a behavior
   change — write a test proving the delegation still works end-to-end
   through the new path).

## Definition of done

- `cargo check --workspace --all-targets` clean (this changes a
  cross-crate seam AND deletes wire-surface variants — confirm no
  stale reference anywhere, including benches/examples if any exist).
- `cargo fmt -p shamir-db -p shamir-server -p shamir-query-types -p shamir-query-builder -- --check` clean on touched files.
- `cargo clippy -p shamir-db -p shamir-server -p shamir-query-types -p shamir-query-builder --all-targets -- -D warnings` clean, modulo the already-tracked pre-existing issues (task #562's `read_planner.rs`, the `hmac_tests.rs` octal_escapes) — confirm via `git diff --stat` neither touched file is yours if you hit either.
- `./scripts/test.sh -p shamir-db -p shamir-server -p shamir-query-types -p shamir-query-builder --full` green.
- Repo-wide grep confirms zero remaining references to `CreateRoleOp`/
  `DropRoleOp`/`RenameRoleOp`/`ListOp::Roles`/`roles_table` in
  non-comment code.
- `CreateUser`/`DropUser`/`GrantRole`/`RevokeRole` demonstrably persist
  to the REAL `FjallUserDirectory`, not shamir-db's retired tables, in
  at least one wire-level test each.

When done, produce a final summary (not a bare tool call) listing:
every file changed with a one-line description, the full text of every
new test, the gate command outputs, and EVERY discrepancy between this
brief's assumptions and the actual code (there will likely be several,
given this task's size — call each one out explicitly rather than
silently resolving it, so the orchestrator's review can weigh in on
anything non-obvious). Specifically confirm: the exact `ShamirDb`
builder-method names you added, whether you kept or removed the
redundant `admin_user_locks()` acquisition around grant/revoke, whether
`grant_role`/`revoke_role` got dedicated atomic `FjallUserDirectory`
methods or stayed as adapter-side read-modify-write, and the exact
mechanism you used for the boot-audit's WARN-log capture in tests.
