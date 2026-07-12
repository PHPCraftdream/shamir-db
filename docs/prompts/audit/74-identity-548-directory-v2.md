בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Brief: FjallUserDirectory v2 — principal64 keyspace, remove(), boot normalization (task #556)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Context

`docs/design/identity-privilege-unification-548-549-decision.md` §2.2,
§6 item 2 (already signed off by the project owner) is the source of
truth for this task's design — read it first. This brief is step #71
of that design's phased plan (§7), renumbered to 74 since briefs
70-73 were claimed by tasks #552-555.

This task builds ONLY on task #555 (already landed —
`shamir_types::access::principal64`/`Actor::Admin` exist). It does
**not** wait for or duplicate task #557's scope: #557 adds the
`superuser` field's *enforcement* (reservation, `SetSuperuser` op,
`SessionPermissions` wiring). This task adds the `superuser` field's
*schema and boot-time re-encoding* only — #557's own task description
explicitly says "`PersistedUser` gains `superuser: bool`... already
added in directory-v2's schema bump — this task wires the
enforcement/gating side." Do not build the `SetSuperuser` op, the
role-reservation checks, or the `SessionPermissions` wiring here — all
three are #557's job.

The file in scope is `crates/shamir-server/src/user_directory.rs` —
read it in full first. It already implements `FjallUserDirectory`
correctly for two keyspaces (`users_v1`: username→blob, and
`user_id_to_name_v1`: 16-byte user_id→username), a warmed
`tickets_cache`, and a `write_lock`-serialised `read_modify_write`
helper. This task extends that same file; nothing here is a rewrite.

## Exact changes

### 1. `PersistedUser` schema bump

Add two fields:

```rust
#[derive(Serialize, Deserialize)]
struct PersistedUser {
    #[serde(with = "serde_bytes")]
    user_id: Vec<u8>,
    #[serde(with = "serde_bytes")]
    salt: Vec<u8>,
    #[serde(with = "serde_bytes")]
    stored_key: Vec<u8>,
    #[serde(with = "serde_bytes")]
    server_key: Vec<u8>,
    kdf_params: PersistedKdfParams,
    roles: Vec<String>,
    tickets_invalid_before_ns: u64,
    /// Re-encoded from the legacy `"superuser"` role string by boot-time
    /// normalization (§4 below). `#[serde(default)]` so OLD persisted
    /// blobs (pre-#556) that lack this field deserialize as `false` —
    /// normalization then fixes any account whose role list still has
    /// the string. Enforcement/mutation of this field (a `SetSuperuser`
    /// wire op, `SessionPermissions` wiring) is task #557's scope, NOT
    /// this task's — this task only adds the field and the one-time
    /// re-encoding.
    #[serde(default)]
    superuser: bool,
}
```

Update `PersistedUser::from_record` to accept and store a `superuser: bool`
parameter (new inserts via `insert()` always pass `false` — nothing in
this task's scope creates a NEW superuser account via a field value;
that's #557's `SetSuperuser`/bootstrap wiring). Update any other
construction site in this file accordingly.

### 2. Third keyspace: `principal64 → username`

```rust
/// Tertiary index keyspace: key = principal64 projection (8 bytes,
/// big-endian u64), value = username (UTF-8 bytes). Maintained in
/// lock-step with `USERS_KEYSPACE`/`USER_ID_INDEX_KEYSPACE` via the same
/// `OwnedWriteBatch` so all three keyspaces stay consistent. Built once
/// at `open()` via boot-time normalization for pre-existing records (§4),
/// then maintained incrementally by `insert()`/`remove()`.
const PRINCIPAL64_INDEX_KEYSPACE: &str = "principal64_to_name_v1";
```

Add the `Keyspace` handle as a new field on `FjallUserDirectory`
(alongside `users`/`user_id_index`), opened in `open()` the same way the
existing two are.

### 3. Mint-time uniqueness/non-zero enforcement in `insert()`

Today `fresh_user_id()` mints 16 random bytes with no uniqueness check at
all (128 bits of randomness makes a user_id collision negligible on its
own). The NEW 63-bit `principal64` projection has a materially higher
collision probability across many accounts (birthday-bound), so `insert()`
needs an explicit retry loop on the PROJECTION, not the full 16 bytes:

```rust
fn mint_unique_user_id(&self) -> Result<[u8; 16]> {
    const MAX_ATTEMPTS: u32 = 16; // 2^-63-per-attempt event; this is generous headroom, not expected to ever loop in practice
    for _ in 0..MAX_ATTEMPTS {
        let user_id = Self::fresh_user_id();
        let projected = shamir_types::access::principal64(user_id);
        if projected == 0 {
            continue; // reserved for OWNER_SYSTEM/Actor::System — re-mint
        }
        let taken = self
            .principal64_index
            .contains_key(&projected.to_be_bytes())
            .map_err(|e| Error::Encoding(format!("fjall: contains_key: {e}")))?;
        if !taken {
            return Ok(user_id);
        }
    }
    Err(Error::Encoding(
        "principal64 mint: exhausted retry budget (this should be \
         cryptographically near-impossible — investigate RNG health)"
            .to_string(),
    ))
}
```

Call this from `insert()` instead of the bare `Self::fresh_user_id()`, and
extend the existing atomic `db.batch()` to insert into all THREE
keyspaces (`users`, `user_id_index`, `principal64_index`) — the batch
commit is already atomic across keyspaces, this is just a third
`batch.insert(...)` call with `&projected.to_be_bytes()` as the key and
the username as the value (same value shape as `user_id_index`).

### 4. Boot-time normalization inside `open()`

Per the design doc §6 item 2, this runs on EVERY `open()` call
(idempotent — must produce the same end state whether it's the first
boot ever or the 500th restart):

1. **Build `principal64_index` from existing `user_id_index` records.**
   Iterate `user_id_index` (key = 16-byte user_id, value = username).
   For each entry, compute `principal64(user_id)`. If the projection is
   `0`, or if it collides with an ALREADY-SEEN projection from an earlier
   iteration of this same loop: **fail `open()` closed**, returning an
   error that NAMES both usernames (the current entry's and the one
   already recorded for that projection) — do not silently re-mint,
   do not skip, do not warn-and-continue. This is a genuinely
   exceptional, operator-visible event (design doc: "an operator
   decision (drop/recreate one account) is" the correct response, not
   automatic remediation). Otherwise, stage a `principal64 → username`
   write for that projection. Write all staged entries in one
   `db.batch()` (this is a build-from-existing-data step, not a
   per-request path — an ordinary batch commit is fine, no special
   retry logic needed here since real collisions are handled by the
   fail-closed path).
   - **Idempotence requirement**: running this twice in a row (e.g. a
     restart with no changes in between) must produce byte-identical
     `principal64_index` contents both times — since the projection is a
     PURE function of the already-stored user_id, simply re-deriving and
     re-inserting the same key→value pairs is naturally idempotent; do
     not add any "only if keyspace is empty" guard that could skip
     re-deriving after a partial/interrupted prior boot.
2. **Migrate the `"superuser"` role string into the new flag.** For every
   user record in `users_v1` whose `roles` list contains the literal
   string `"superuser"`: set `superuser = true` on that record, remove
   the string from `roles`, and persist the updated blob. This is a
   deterministic re-encoding of an already-true fact (design doc: "not a
   reconciliation between conflicting sources"), not data migration in
   the risky sense — do it unconditionally on every boot (idempotent: a
   record already migrated has no `"superuser"` string left, so the
   `contains` check is simply false on the second run and nothing
   changes).
3. **Superuser count.** After normalization, count all persisted users
   with `superuser == true` and store it in a new
   `superuser_count: AtomicU64` field on `FjallUserDirectory`, warmed once
   at `open()` alongside the existing `tickets_cache` warm-up (can share
   the same iteration pass over `users_v1` for efficiency — don't add a
   second full scan if the `tickets_cache` warm-up loop can carry this
   too).

Do these steps BEFORE constructing the returned `Self` (or via a
dedicated `normalize()` associated function called from `open()`) so
`open()` never returns a directory in a not-yet-normalized state.

### 5. `remove()` — new method

```rust
/// Permanently delete a user account: all three keyspaces, atomically.
///
/// Does NOT evict live sessions itself — `FjallUserDirectory` has no
/// handle to a `SessionStore`. Per the existing pattern in
/// `crates/shamir-connect/src/server/admin.rs` (`snapshot_by_user` then
/// kill, already used by the role-update/credential-change paths), the
/// CALLER is responsible for snapshotting and killing live sessions for
/// this `user_id` after a successful `remove()` — this is out of this
/// task's scope (the wire-level `DropUser` handler wiring is task #559's
/// job, "re-target the four surviving user DDL handlers").
///
/// Refuses to remove the last remaining superuser account (last-superuser
/// guard) — returns a typed error, does not delete anything.
pub fn remove(&self, username: &str) -> Result<bool> {
    let _guard = self.write_lock.lock();

    let blob = match self.read_blob(username)? {
        Some(b) => b,
        None => return Ok(false), // already absent — idempotent no-op
    };
    let user: PersistedUser = rmp_serde::from_slice(&blob)
        .map_err(|e| Error::Encoding(format!("rmp decode: {e}")))?;
    let user_id = user
        .user_id_array()
        .ok_or_else(|| Error::Encoding("corrupt user_id in persisted record".to_string()))?;

    if user.superuser && self.superuser_count.load(Ordering::Relaxed) <= 1 {
        return Err(Error::InvalidInput(
            "cannot remove the last remaining superuser account",
        ));
    }

    let projected = shamir_types::access::principal64(user_id);

    let mut batch = self.db.batch();
    batch.remove(&self.users, username.as_bytes());
    batch.remove(&self.user_id_index, &user_id[..]);
    batch.remove(&self.principal64_index, &projected.to_be_bytes());
    batch
        .commit()
        .map_err(|e| Error::Encoding(format!("fjall: batch commit: {e}")))?;

    self.db
        .persist(PersistMode::SyncAll)
        .map_err(|e| Error::Encoding(format!("fjall: persist: {e}")))?;

    // Evict the tickets_cache entry too — the cache is the AUTHORITATIVE
    // source `tickets_invalid_before_ns_by_user_id`/`state_by_user_id`
    // read from (see their doc comments: "a cold miss means unknown
    // user"). A stale cache entry surviving past `remove()` would make a
    // deleted account's user_id still resolve to a (stale) tib value
    // instead of "unknown" — reproducing exactly the fail-open bug §6
    // below closes, just via a different path. This is NOT optional.
    self.tickets_cache.remove(&user_id);

    if user.superuser {
        self.superuser_count.fetch_sub(1, Ordering::Relaxed);
    }

    Ok(true)
}
```

(Match this repo's exact `scc::HashMap` removal API — check
`SccHashMap::remove`'s real signature in the version already in use
elsewhere in this file/crate rather than assuming; adjust if the actual
method differs in name/return shape.)

### 6. `state_by_user_id` — new read method (for task #558)

```rust
/// One-lookup snapshot of a user's authoritative state, keyed by
/// `user_id` — built for task #558's Ticket-v2 resume rewrite (resume
/// re-verifies against the directory instead of trusting a stale ticket
/// snapshot). Returns `None` if the user_id is not found (unknown/removed
/// account).
pub fn state_by_user_id(&self, user_id: &[u8; 16]) -> Option<UserDirectoryState> {
    let username = self.read_username_by_user_id(user_id)?;
    let blob = self.read_blob(&username).ok().flatten()?;
    let user: PersistedUser = rmp_serde::from_slice(&blob).ok()?;
    Some(UserDirectoryState {
        username,
        roles: user.roles,
        superuser: user.superuser,
        tickets_invalid_before_ns: user.tickets_invalid_before_ns,
    })
}
```

Define `UserDirectoryState` as a small `pub struct` (four fields as
above) near `PersistedUser`. Factor the `user_id_index` reverse lookup
(username-by-user_id) into a small private helper
`read_username_by_user_id` if one doesn't already exist under a
different name — check first, `user_id()` (the existing trait method)
already does a *forward* lookup (username→user_id via decoding the
user's OWN blob), which is NOT what's needed here; you need the
REVERSE index (`user_id_index` keyspace) read directly.

### 7. Fix the fail-open `UserStateLookup` adapter

`crates/shamir-server/src/connection/user_state_lookup.rs` currently
ALWAYS returns `Some(tib)` — its own doc comment describes wanting to
return `None` for unknown users but the implementation never actually
distinguishes "unknown" from "known with tib=0" (it references a
hypothetical `user_id_exists` method that doesn't exist and silently
falls back to treating every miss as valid). Rewrite it using the new
`state_by_user_id`:

```rust
impl UserStateLookup for RedbUserStateLookup<'_> {
    fn lookup(&self, user_id: &[u8; 16]) -> Option<u64> {
        self.0
            .state_by_user_id(user_id)
            .map(|s| s.tickets_invalid_before_ns)
    }
}
```

Delete the old comment block's now-inaccurate rationale along with the
old implementation — this is the actual fix, not a workaround.

## Red tests required first (TDD)

1. **Boot normalization idempotence** — open a `FjallUserDirectory` at a
   tempdir path, insert a couple of users (including one via the
   internal helper that seeds a pre-#556-shaped blob with `"superuser"`
   still in `roles` and no `superuser` field, to simulate an
   upgrade-from-old-data boot), close/drop it, `open()` the SAME path
   again (triggering normalization a second time), assert: the
   `principal64_index` contents are identical both times, the migrated
   account's `roles` no longer contains `"superuser"` and its `superuser`
   field is `true`, and re-opening a third time changes nothing further.
2. **Unknown-user resume rejected** — build a `FjallUserDirectory`,
   `RedbUserStateLookup::lookup(&some_random_never_inserted_user_id)`
   must return `None`. Contrast with a REAL inserted user (even one with
   `tickets_invalid_before_ns == 0`, the default) returning `Some(0)`, to
   prove the fix distinguishes "unknown" from "known-but-zero" rather
   than collapsing both to `None` or both to `Some(0)`.
3. **Last-superuser removal refused** — insert exactly one superuser
   account (directly construct/seed it with `superuser: true`, or drive
   it through normalization with a `"superuser"`-role seed — whichever is
   less brittle given the actual test helpers already in this file/crate),
   call `remove()` on it, assert it returns an `Err` and the account is
   STILL present afterward (`lookup_by_name` still finds it). Then insert
   a SECOND superuser account and confirm `remove()` on the first one now
   succeeds (proves the guard is a genuine count check, not an
   unconditional "superuser accounts are undeletable" block).

Also add a focused test for the collision/zero-projection fail-closed
path if it can be constructed deterministically (e.g. by seeding two
raw records whose `user_id` bytes are engineered to project to the same
`principal64` value, then calling `open()` and asserting it errors and
the error message names both usernames) — if constructing a genuine
projection collision deterministically is impractical within reasonable
effort, at minimum unit-test the projection-collision-detection logic in
isolation (a pure function/helper, if you factor one out) rather than
skipping coverage of this path entirely.

## Out of scope — do not touch

- `SetSuperuser` wire op, role-string reservation in `update_roles`/
  `CreateScramUser`/`update_user`, `SessionPermissions` wiring off the
  flag, bootstrap writing the flag instead of the role string — task
  #557.
- Ticket v2 / the actual resume rewrite consuming `state_by_user_id` for
  its authoritative lookup (this task only builds the primitive; #558
  wires it into `process_resume`) — task #558.
- `PrincipalResolver`/`UserAdminPort`, retiring shamir-db's users/roles
  tables, the wire-level `DropUser` handler calling this task's new
  `remove()` and doing the session-kill — task #559.
- Anything in `crates/shamir-client-ts`/`shamir-query-builder` — no wire
  format changes in this task.
- The `database` scope field / boot audit diff mentioned in design doc §6
  item 3 — that is about shamir-db's Store B, not this task's directory.

## Definition of done

- `cargo fmt -p shamir-server -- --check` clean on touched files
  (pre-existing drift elsewhere not your concern).
- `cargo clippy -p shamir-server --all-targets -- -D warnings` clean,
  modulo the already-tracked pre-existing `read_planner.rs`
  `type_complexity` issue (task #562) — confirm via `git diff --stat`
  you haven't touched that file if you hit it.
- `cargo check --workspace --all-targets` clean.
- `./scripts/test.sh -p shamir-server --full` green, including the 3+
  new red tests.
- Every existing test in `crates/shamir-server/tests/user_directory.rs`
  and anywhere else that constructs/uses `FjallUserDirectory` still
  passes unmodified in behavior (this is an additive schema/keyspace
  bump — no existing `insert`/`lookup_by_name`/`update_roles`/
  `bump_tickets_invalid`/`update_credentials` caller should need to
  change its own call shape).
- `tickets_cache` eviction on `remove()` is present and covered by a
  test (not just present in the diff) — a stale cache entry surviving
  removal would silently reopen the exact fail-open bug this task
  closes via a different path.

When done, produce a final summary (not a bare tool call) listing: every
file changed with a one-line description, the full text of every new
test, the gate command outputs, and any place this brief's assumptions
didn't match the actual code (with how you resolved it) — in
particular the exact `scc::HashMap` removal API used for the
`tickets_cache` eviction, and the exact reverse-lookup helper name/shape
used for `state_by_user_id`.
