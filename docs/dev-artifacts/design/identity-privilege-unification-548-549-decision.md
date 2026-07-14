בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Tasks #548 + #549: unified stable identity + first-class privilege axis — design decision

One coherent target architecture for two coupled findings:

- **#548** (model-core F2, admin-ddl #4, identity-session #1+#2):
  the security `Actor` and every catalogue `owner` field are bound to
  `fxhash(username)` — a non-cryptographic hash of a mutable,
  attacker-chosen string. Partial decision already recorded in
  `docs/dev-artifacts/design/principal-id-identity-548-decision.md` (Option A: reuse
  the 16-byte `user_id`; clean cutover, no migration of stale owner
  values).
- **#549** (admin-ddl #1+#2, identity-session #4, model-core F4):
  two disconnected user/role stores, and a second privilege axis
  (`role == "superuser"`) that bypasses the POSIX `permits()` model by
  string comparison alone.

They are one design problem: whatever the canonical stable user
identity becomes for #548 is necessarily the key the unified user/role
store of #549 hangs off. This document proposes both together, per the
campaign's investigate → decision-doc → sign-off pattern (precedent:
#512, #533, #548).

Every claim below was re-verified against the working tree on
2026-07-12; file:line references are to that state.

---

## 1. The current state, precisely

### 1.1 Three identity axes for one person

| Axis | Where minted | Where used | Properties |
|---|---|---|---|
| `username` (string) | operator input | key of BOTH stores; SCRAM login; PII | mutable in principle, attacker-chosen |
| `principal_id = fxhash64(username) & i64::MAX` | recomputed per request (`crates/shamir-connect/src/server/session.rs:246-252`) | `Actor::User(u64)` for every authz decision (`crates/shamir-server/src/db_handler/handler.rs:119-125`); persisted `owner` on every catalogue record (`crates/shamir-types/src/access.rs:213-229`); group member lists (`crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:456-471`) | deterministic function of a mutable name; not preimage-resistant; id `0` aliases `Actor::System` (`access.rs:21`, `access.rs:48-54`) with no guard |
| `user_id` ([u8; 16], random) | once, at account creation (`FjallUserDirectory::insert` → `fresh_user_id`, `crates/shamir-server/src/user_directory.rs:263-265`, `349-389`) | session-revocation epoch keyed by it (`user_directory.rs:230-240`); `Session.user_id` (`session.rs:111`); resumption tickets (`crates/shamir-connect/src/server/ticket.rs:54`); per-user session cap/kill (`session.rs:428-442`) | cryptographically random, stable for the account's lifetime, never reused — and **completely disconnected from `Actor`/ownership** |

The confirmed failure modes of the middle axis (full detail in the
#548 doc, re-verified): inheritance-on-recreate (drop `alice`, recreate
`alice` → identical `principal_id` → silent inheritance of everything
the old alice owned), collision forging (fxhash offers no assurance
against a crafted username colliding with a target owner id), and the
unguarded id-0 → `Actor::System` alias.

### 1.2 Two disconnected user/role stores

**Store A — `FjallUserDirectory`** (`crates/shamir-server/src/user_directory.rs`).
The ONLY store the wire trusts:

- Login reads SCRAM keys by username, then `user_id` and `roles` from
  it (`crates/shamir-server/src/connection/handshake.rs:405-459`).
- Admin wire ops write it: `create_scram_user`
  (`crates/shamir-server/src/db_handler/admin.rs:43-131`),
  `update_roles`/`bump_tickets_invalid`
  (`user_directory.rs:391-420`), `update_credentials` (changePassword,
  `user_directory.rs:428-461`).
- Bootstrap seeds the first superuser into it
  (`crates/shamir-server/src/bootstrap.rs:91-158`).
- Durable (fjall, fsync per write), atomic username↔user_id index
  (`user_directory.rs:39-44`, `372-377`), owns the
  `tickets_invalid_before_ns` revocation epoch.

**Store B — the shamir-db `users` / `roles` catalogue tables**
(`crates/shamir-db/src/shamir_db/execute/admin_users_roles.rs`),
written by the `CreateUser`/`DropUser`/`CreateRole`/`DropRole`/
`RenameRole`/`GrantRole`/`RevokeRole` BatchOps:

- **No stable id at all** — purely name-keyed records
  (`admin_users_roles.rs:63-67`).
- `CreateUser` is a blind `SetOp` upsert with no existence guard —
  re-creating an existing name silently overwrites the record
  (`admin_users_roles.rs:63-78`).
- Stores a `password_hash` that **nothing ever authenticates against**
  (the code says so itself, `admin_users_roles.rs:39-45`).
- **Never read at login or resume.** `GrantRole superuser bob` via DDL
  mutates only this table (`admin_users_roles.rs:587-679`) — bob's
  live wire rights are untouched. Phantom grants; a false appearance
  of working administration.
- `DropUser` deletes only this phantom record
  (`admin_users_roles.rs:84-214`) — **the actual SCRAM login account is
  untouched**. Worse: `FjallUserDirectory` has NO delete method at all
  (`user_directory.rs:342-461` — `insert`/`update_roles`/`bump`/
  `user_id`/`update_credentials` is the whole trait surface). Today it
  is impossible to actually delete a login account.
- `RenameRole` has **no reserved-name guard** — renaming the string
  `"superuser"` itself is accepted (`admin_users_roles.rs:365-585`,
  guards check only source-exists and dest-free). Inert only because
  this store is never consulted for privileges — incoherence, not
  safety.
- `Role.permissions` is stored and never enforced anywhere
  (`admin_users_roles.rs:238-243`; enforcement is exclusively
  `is_superuser` + `permits()`).

What Store B is legitimately used for today: `access_tree` display
(`access_control.rs:713-725`, which re-derives ids via
`principal_id(uname)`), the owner-delegation `database` scope in
`authorize_user_lifecycle`
(`crates/shamir-db/src/shamir_db/execute/admin_dispatch.rs:141-160`,
consumed at `admin_users_roles.rs:36-38`, `152-177`), and `List`
introspection (`admin_list.rs:84`, `123`).

### 1.3 The string-keyed privilege axis

- `SessionPermissions::from_roles` sets `is_superuser` iff the literal
  string `"superuser"` appears in the roles list
  (`session.rs:33-41`).
- `session_actor` maps `is_superuser` → `Actor::System`
  (`handler.rs:119-125`) — the same variant that unconditionally
  bypasses `permits()` (`access.rs:652-654`) and that internal engine
  code uses for its own writes (e.g.
  `admin_users_roles.rs:199`). Consequences: a superuser's created
  resources are stamped `owner = 0` = System — **ownership attribution
  is lost** for every admin-created object; and "server administrator"
  is conflated with "the engine itself".
- The gate sites: admin BatchOps (`handler.rs:397-404`,
  `tx_handlers.rs:103-108`), `create_scram_user` (`admin.rs:50-54`),
  wire admin commands (`crates/shamir-connect/src/server/admin.rs:68-74`),
  replication (`repl_handler.rs:46` — `is_superuser ||
  has_role("replicator")`). A role's declared permissions never matter;
  escalation is by NAME.

### 1.4 Resume trusts a snapshot

- The ticket carries `roles` (`ticket.rs:71-74`); resume rebuilds
  `SessionPermissions` from that snapshot without consulting the
  directory (`crates/shamir-connect/src/server/resume.rs:354-402`).
  Integrity rests entirely on "every rights-changing write bumps
  `tickets_invalid_before_ns`" (step 9, `resume.rs:272-275`) — one
  forgotten bump anywhere and a stale ticket resurrects revoked
  superuser powers.
- **Confirmed bug found during this investigation:** the server's
  `UserStateLookup` adapter never returns `None`
  (`crates/shamir-server/src/connection/user_state_lookup.rs:14-37`
  — it wraps `tickets_invalid_before_ns_by_user_id`, whose miss value
  is `0`, in `Some(..)` unconditionally, despite its own doc comment
  claiming unknown users are rejected). Since no delete method exists
  this is currently unreachable-in-practice, but the moment account
  deletion lands (below), **a deleted user's outstanding ticket would
  still resume**. The fix is part of this design, not a separate
  finding.

---

## 2. Decision 1 — the canonical identity, and a sanity-check of the recorded #548 choice

### 2.1 The recorded decision holds

The user chose Option A: the 16-byte random `user_id` minted by
`FjallUserDirectory::insert` becomes the `Actor`/`owner` identity;
clean cutover, no remap of stale catalogue owners; `DropUser` policy
unchanged. Investigating the two-store picture **confirms** rather
than undermines this: the directory is the only store that (a) already
mints a stable id, (b) is already consulted at every login and every
revocation decision, and (c) survives independently of the engine's
query machinery. Nothing about store unification changes what "the"
id is — it changes who else gets to *see* it.

### 2.2 Refinement 1: the wire/catalogue representation is a 63-bit projection, made unique by construction

`Actor::User(u64)`, the catalogue `owner`/`group` encoding
(`QueryValue::Int` = i64, `access.rs:213-229`), group member lists
(`Vec<u64>`), the `Chown`/`Chgrp`/`AddGroupMember` wire ops, and the
TS client all speak u64-that-fits-i64. The 16-byte `user_id` does not
fit. Two honest options:

- **(i) Widen everything to 128 bits** — `Actor::User(u128)` or
  `[u8; 16]`, owner persisted as bytes/hex, group members as lists of
  bytes, wire ops and both clients changed. Maximal fidelity; large,
  cross-cutting blast radius through the catalogue encoding, the wire
  protocol, and every client.
- **(ii) A fixed 63-bit projection** `principal64(user_id) =
  u64::from_be_bytes(user_id[0..8]) & i64::MAX`, with uniqueness and
  non-zero-ness **enforced at mint time**: `FjallUserDirectory::insert`
  gains a third keyspace (`principal64 → username`); if the freshly
  minted `user_id`'s projection is `0` or already taken, re-mint the
  whole 16 bytes and retry (bounded loop; a repeat is a ~2⁻⁶³ event).
  The projection is a pure function of `user_id` — derivable anywhere
  the 16 bytes are in hand (e.g. `session_actor` from
  `session.user_id`), no lookup, no second source of truth.

**Recommendation: (ii).** This must be said plainly because "no
compromises" was the instruction: (ii) is not a probabilistic
compromise. With mint-time uniqueness the 63-bit id is
collision-free *by construction* (exactly like `group_id`'s monotonic
counter, `access_control.rs:269-307`, just random instead of
sequential), non-zero by construction (closing failure mode 3
structurally, where today there is only an "astronomically unlikely"
comment, `access.rs:29-32`), and unguessable (63 random bits that are
a function of NO attacker input — there is nothing to search, unlike
fxhash of a chosen name). Option (i) buys zero additional security
and costs a catalogue-encoding break, a wire break, and a client
break across three repos. The full 16 bytes remain the identity
everywhere they already live (directory, tickets, session,
revocation); `principal64` is its fixed public projection into the
POSIX model. If mutual-TLS-grade identity ever arrives (#512's
follow-up), it anchors to the 16 bytes, not the projection — nothing
is foreclosed.

### 2.3 Refinement 2: `Actor` gains an `Admin` variant (superuser ≠ System)

See §4 — recorded here because it touches the same enum. `Actor`
becomes:

```rust
pub enum Actor {
    System,        // in-process engine/bootstrap code ONLY; never minted from a wire session
    Admin(u64),    // superuser session: bypasses permits(), but owns as principal64
    User(u64),     // regular session: full POSIX evaluation
}
```

`permits()`/`authorize_access` treat `Admin` exactly like `System`
(bypass); `to_owner_id()` returns the real id, so admin-created
resources are finally attributed to their creator instead of owner=0
(`ResourceMeta::owned_enforced`, `access.rs:203-209`, starts doing
the right thing for admins automatically). `from_owner_id` is
unchanged — admin-ness is a live session property, never a persisted
owner property, so nothing round-trips through it.

`principal_id(username)` (`access.rs:33-35`) and
`Session::principal_id()` (`session.rs:246-252`) are **deleted**, not
deprecated. `session_actor` (`handler.rs:119-125`) becomes:
superuser → `Actor::Admin(principal64(session.user_id))`, else
`Actor::User(principal64(session.user_id))`.

### 2.4 Cutover consequences (unchanged posture from the recorded decision)

Existing catalogue `owner` values and existing group `members` lists
hold old hash-derived numbers. Per the recorded decision they are NOT
remapped: they become stale data. Practical effect: previously-owned
resources evaluate as owned-by-nobody-logged-in (their old ids no
longer match any session's actor) — an operator re-`chown`s what
matters. Because the new id space is disjoint-by-randomness from
nothing in particular, an accidental match between a stale hash value
and a fresh `principal64` is a ~N·M/2⁶³ event; acceptable, and
detectable via `access_tree` (owner ids with `owner_name: null`).

---

## 3. Decision 2 — the directory is canonical; shamir-db stops pretending to own users

### 3.1 The layering question, answered

Is shamir-db even meant to know about SCRAM login? **No — and it
already doesn't.** `ShamirDb::execute_as(actor, ...)` receives an
`Actor` from its caller; the engine never authenticates anyone. Its
`users` table exists only to *look like* user administration while
being read by nothing that matters (§1.2). The correct shape is:

- **`shamir-server`'s `FjallUserDirectory` is the single source of
  truth** for accounts: username, `user_id`, SCRAM credentials, roles,
  the superuser flag (§4), optional `database` scope, revocation
  epoch. It already has durability, cross-keyspace atomicity, the
  epoch mechanism, and the only login path.
- **`shamir-db` becomes identity-agnostic**: it consumes opaque
  `principal64` ids in `Actor`/owner/group-member positions and
  gains one narrow read-only port for the places that need to resolve
  or enumerate principals:

  ```rust
  // shamir-db (or shamir-types) — implemented by the embedding layer.
  pub trait PrincipalResolver: Send + Sync {
      fn resolve(&self, principal64: u64) -> Option<PrincipalInfo>; // name, user_id, db scope, superuser
      fn list(&self) -> Vec<PrincipalInfo>;                          // access_tree / List introspection
  }
  ```

  Consumers: `access_tree` name resolution (replacing the
  `principal_id(uname)` loop at `access_control.rs:713-725`),
  `authorize_user_lifecycle`'s database-scope lookup
  (`admin_dispatch.rs:141+`), and the #543 follow-up ("does this
  chown/addGroupMember target resolve to a real principal" — the
  validation that was explicitly deferred to this decision,
  `crates/shamir-db/src/shamir_db/tests/admin_access_validation_tests.rs:1-45`).
  Absent resolver (embedded/test use) → names resolve to `null`,
  scope-delegation unavailable, target-validation skipped — the
  engine still enforces `permits()` on opaque ids exactly as now.

- **User/role DDL BatchOps route through a write port** implemented in
  `shamir-server` over the directory:

  ```rust
  pub trait UserAdminPort: Send + Sync {
      async fn create_user(&self, name, password, roles, database) -> Result<[u8; 16]>;
      async fn drop_user(&self, name) -> Result<bool>;
      async fn grant_role(&self, user, role) -> Result<()>;
      async fn revoke_role(&self, user, role) -> Result<()>;
      async fn set_superuser(&self, user, on: bool) -> Result<()>;
  }
  ```

  `handle_create_user`/`handle_drop_user`/`handle_grant_role`/
  `handle_revoke_role` re-target this port (authorization gates —
  `authorize_user_lifecycle`, `Manage(Root)`, HMAC confirmation —
  stay exactly where they are). Without an installed port these ops
  return a typed `not_supported`. Argon2id derivation stays
  server-side inside the port impl (reusing the `create_scram_user`
  path, `admin.rs:69-104`); shamir-db never touches SCRAM crypto.

This closes the phantom-grant hole in the strongest possible way:
DDL administration and wire administration become the SAME writes to
the SAME store, so `GrantRole` finally affects real login rights AND
inherits the epoch bump (`update_roles` already bumps
`tickets_invalid_before_ns`, `user_directory.rs:391-409`) — live
sessions of the target die on their next request with no new
invalidation code. `DropUser` finally deletes the actual account
(the directory gains a `remove` — batch-deleting all three keyspace
entries + epoch bump + session kill), fixing the
"accounts are undeletable" gap of §1.2.

### 3.2 What happens to the other store

The shamir-db `users` and `roles` **tables are retired as catalogue
concepts**: handlers stop reading/writing them, `List` introspection
of users re-points at the resolver, and the `roles` table's op family
(`CreateRole`/`DropRole`/`RenameRole`) is **deleted from the BatchOp
surface** — under this design a "role" is a plain string label
attached to users in the directory (as the live system already
treats it); there is no role *object*, no unenforced `permissions`
blob, and therefore nothing to create, rename, or drop. The
`RenameRole`-can-rename-`"superuser"` question dissolves rather than
needing a guard. (`GrantRole`/`RevokeRole` survive — they now edit
directory role labels through the port.)

On-disk data of the old tables is not destroyed — the system store
keeps whatever rows exist; we stop ascribing meaning to them. This
mirrors the #548 posture for owner values.

**Alternatives considered and rejected:**

- *Keep both stores, add synchronization.* Dual-write across two
  durability domains (system-store WAL vs fjall) with no cross-store
  transaction = guaranteed drift windows, which is precisely the bug
  class being fixed. Every future feature would have to remember to
  write both. Rejected outright.
- *Make shamir-db canonical and have the server read it.* Puts SCRAM
  secrets and the login hot path behind the engine's query machinery
  and its MVCC/WAL lifecycle; inverts the dependency (shamir-connect
  would need shamir-db); loses the directory's purpose-built
  atomicity (`update_credentials`' single-fsync crash-safety
  argument, `user_directory.rs:437-449`). Rejected.
- *Keep the users table as a synced read-replica for introspection.*
  A cache that can lie about security state is worse than a resolver
  call that cannot. Admin introspection is not a hot path. Rejected.

---

## 4. Decision 3 — superuser becomes a first-class flag, not a magic string

### 4.1 Mechanism

- `PersistedUser` gains `superuser: bool` (serde-default `false`)
  (`user_directory.rs:77-90`). The role list is for ordinary labels
  (`"replicator"`, app-defined names); the string `"superuser"` is
  **reserved** — rejected with a typed error by every role-writing
  path (`update_roles` at the directory boundary, the `GrantRole`
  port, `CreateScramUser`'s roles parameter, `update_user`'s wire
  path in `crates/shamir-connect/src/server/admin.rs:207-264`).
- Granting/revoking the flag is its own explicit operation —
  `SetSuperuser { user, on }` — superuser-gated and covered by the
  destructive-op HMAC confirmation (the #542/#551 gate,
  `admin.rs:328-459`), because it is the single most dangerous write
  in the system. It refuses to turn off the **last** superuser
  (directory keeps an O(1) count), closing the self-lockout footgun.
- `SessionPermissions.is_superuser` is populated from the flag at
  handshake and at resume (§5) — `from_roles`' string scan
  (`session.rs:35-41`) goes away; `has_role` stays for
  `"replicator"`-style scoped capabilities (`repl_handler.rs:46`),
  which remain plain labels: they are only writable through
  superuser-gated ops, so they need no reservation machinery.
- `session_actor` maps the flag to `Actor::Admin(principal64)`
  (§2.3). `Actor::System` is no longer constructible from any wire
  session — it remains what its doc already claims it is: the
  engine's own identity for internal writes and bootstrap.
- Bootstrap (`ensure_superuser`, `bootstrap.rs:91-158`) sets the flag
  instead of inserting the role string; the
  `superuser_ever_existed` invariant machinery
  (`crates/shamir-connect/src/server/bootstrap.rs`,
  `server_meta.rs:459-465`) keys off the flag count.

### 4.2 Why a flag and not "keep the role string but validate it"

Reserving the name while keeping privilege as data-in-a-list was
considered (smallest diff): every enforcement site keeps doing string
compares, `is_superuser` remains a derived property recomputed at N
places, and any future roles surface (import, sync, a new admin API)
must remember the reservation. A boolean on the account record makes
the privilege axis a *schema* fact — visible in one field, granted by
one gated op, impossible to smuggle in through a list write. This is
the same structural move as #548 itself: stop deriving a security
property from a string.

Making superuser a full `permits()` participant (e.g. a synthetic
"admins" group with rwx everywhere) was also considered and rejected:
an administrator who can `chmod`/`chown` arbitrarily can always
re-grant themselves access, so mode bits can never *contain* an
admin; pretending otherwise adds evaluation cost and false comfort.
POSIX itself models root as a bypass. The honest design is an
explicit, attributed bypass — `Actor::Admin`.

---

## 5. Decision 4 — resume re-verifies against the directory; the ticket stops carrying roles

### 5.1 Mechanism

- `TicketPlain` **v2**: drop the `roles` field (`ticket.rs:71-74`);
  bump the version byte to 2. v1 tickets are rejected (the version
  check at `resume.rs:236-238` already fails closed) → each existing
  client performs ONE full SCRAM re-auth after the upgrade. Tickets
  live ≤ 24 h (`ticket.rs:305-314`), so this is a bounded, benign,
  self-healing event — and it is the honest expression of the new
  rule: *a ticket proves continuity of authentication; it does not
  carry authorization.*
- `process_resume` step 8 widens: instead of fetching only the epoch,
  it fetches `(tickets_invalid_before_ns, roles, superuser,
  username)` from the directory **by `user_id`** (the directory
  already maintains `user_id → username`, `user_directory.rs:44`;
  add a `state_by_user_id` read = two point gets + one msgpack
  decode). Unknown user → hard `AuthFailed`, which also fixes the
  fail-open adapter of §1.4 as a by-product
  (`user_state_lookup.rs:14-37` is replaced by the richer lookup that
  genuinely returns `None`).
- The session is built from the looked-up roles/flag, not from any
  snapshot. Username also comes from the directory, so a (future)
  rename cannot resurrect a stale name through a ticket.
- The epoch (`tickets_invalid_before_ns`) is **retained unchanged**:
  it is still what kills *live in-memory sessions* on the next
  request after a rights change (`request_loop.rs:292-299`, spec
  §7.5), and still bounds `original_auth_at_ns`. Resume-time lookup
  and the epoch are complementary: the epoch handles "session already
  open", the lookup handles "authorization re-imported at
  reconnection".

### 5.2 The tradeoff, priced

Cost per resume: ~2 fjall point reads + one small msgpack decode —
single-digit microseconds against a path whose raison d'être is
skipping ~100 ms of Argon2id. Resume happens per reconnect, not per
request; there is no hot-path impact. Complexity cost: one trait
widening (`UserStateLookup` → returns a small struct instead of a
bare u64). What we buy: the "every bump path must be complete forever"
invariant stops being load-bearing for resumed sessions — a whole
class of future forgot-to-bump bugs is downgraded from
privilege-resurrection to at-most-one-session-lifetime staleness.
The trust-the-snapshot alternative saves microseconds nobody will
ever measure and keeps a standing fragility. Not close.

---

## 6. Decision 5 — cutover, concretely (the riskiest question, answered without hand-waving)

Three different data populations, three different (all-clean-cutover)
answers, each for a stated reason:

1. **Catalogue `owner`/`group`-member values (old hash ids)** —
   *left as stale data, no remap.* Already decided and recorded for
   #548; the group-membership lists share the posture (they are the
   same id space). Operator re-chowns/re-adds what matters;
   `access_tree` makes orphans visible (`owner_name: null`).

2. **The directory's own records** — *one idempotent boot-time
   normalization inside `FjallUserDirectory::open`, and this is the
   only data-touching step in the whole design:*
   - build the new `principal64 → username` keyspace from existing
     records (a deterministic index derivation, not a remap — the
     projection is a pure function of the already-stored `user_id`).
     If two existing users' projections collide or one is `0`
     (probability ≈ N²/2⁶⁴ — for any real deployment, effectively
     zero): **fail boot, closed, naming both usernames.** Silent
     re-minting is not acceptable (it would invalidate a user's
     future ownership references); an operator decision
     (drop/recreate one account) is.
   - `role list contains "superuser"` → set the `superuser` flag,
     remove the string from the list, persist. **This step is not
     optional and not a compromise of the no-migration posture:**
     without it, every existing deployment's admin account (bootstrap
     writes the role string today, `bootstrap.rs:133`,
     `insert_with_role:163-174`) loses superuser at upgrade =
     guaranteed total lockout. It is a re-encoding of one already-true
     fact into the new schema, deterministic and idempotent — not a
     reconciliation between conflicting sources.

3. **The shamir-db `users`/`roles` tables** — *dropped from the read
   path, explicitly NOT imported into the directory.* This is the
   crux the task brief asked to face squarely. The instinct "losing
   role/grant data on cutover could de-privilege real accounts" has
   the polarity backwards here, because this store was **never
   effective**: no login, resume, or gate has ever read it
   (§1.2). Therefore:
   - Importing it cannot *restore* anything — there is nothing it
     ever granted.
   - Importing it CAN *escalate*: a years-old `GrantRole superuser
     bob` that never worked would silently start working. An
     import turns every historical phantom grant into a live one, at
     the exact moment nobody is looking. That is the one genuinely
     dangerous option on the table, and it is rejected.
   - What IS owed to the operator is *visibility*: a one-time boot
     audit (runs while the old tables still exist on disk) that
     diffs Store B against the directory and logs WARN lines —
     usernames present only in one store, role sets that diverge,
     phantom `superuser` grants — with an explicit "these had no
     effect and were not applied" trailer. Log-only, no auto-apply,
     removable after one release.

   The `database` scope field (the only Store-B datum with live
   enforcement meaning, via `authorize_user_lifecycle`) moves
   *schema-wise* to the directory record; existing scoped users, if
   any deployment has them, appear in the audit diff for manual
   re-creation. Given this feature's age and the store's overall
   phantom status, building an auto-carry for this one field is not
   justified — flagged here so the sign-off is informed.

---

## 7. Phased implementation plan

Each step is one commit-gated brief in the established pipeline
(`docs/dev-artifacts/prompts/audit/<NN>-*.md`; next free number 70). Tests named
per step; every brief carries the standard git-mutation ban.

- **#70 `identity-548-actor-principal64`** — `shamir-types`:
  add `Actor::Admin(u64)`; add `principal64([u8;16]) -> u64`;
  **delete** `principal_id()` and `Session::principal_id()`;
  `permits`/`class_of`/`authorize_access` handle `Admin` as bypass;
  `session_actor` reads `session.user_id`. Compile-driven sweep of
  the ~25 test/bench call sites found by grep (benches construct
  `Actor::User(principal64(fixed_bytes))` fixtures;
  `permission_e2e.rs:458-460`'s local mirror is replaced). Red test
  first: recreate-same-username-gets-different-actor;
  admin-created-resource-owner-is-admin-id-not-zero.
- **#71 `identity-548-directory-v2`** — `shamir-server`:
  `PersistedUser` v2 (`superuser`, `database`), `principal64`
  keyspace + mint-time uniqueness/non-zero retry, `remove()` (3-key
  batch delete + epoch bump), `state_by_user_id`, superuser count +
  last-superuser guard, boot normalization + fail-closed collision
  check, fix the `UserStateLookup` fail-open adapter. Red tests:
  normalization idempotence; unknown-user resume rejected;
  last-superuser revoke refused.
- **#72 `privilege-549-superuser-flag`** — flag-fed
  `SessionPermissions`; reserve `"superuser"` in every role-writing
  path; `SetSuperuser` op (HMAC canonical form + gate); bootstrap
  writes the flag; `superuser_ever_existed` keyed to flag count.
- **#73 `resume-549-authoritative-lookup`** — Ticket v2 (drop
  `roles`, version bump), resume-time directory lookup by `user_id`,
  session built from looked-up state. Red tests: v1 ticket rejected;
  role-revoked-then-resume gets non-admin session even if no epoch
  bump ran (deliberately simulate the forgotten-bump bug).
- **#74 `stores-549-shamir-db-retirement`** — `PrincipalResolver` +
  `UserAdminPort` traits; re-target the four surviving user DDL
  handlers; delete `CreateRole`/`DropRole`/`RenameRole` BatchOps;
  `access_tree`/`List`/`authorize_user_lifecycle` via resolver;
  boot audit diff; server boot wires both ports.
- **#75 `clients-549-surface-sync`** — TS + Rust client builders:
  remove role-object ops, add `setSuperuser`, update HMAC canonical
  helpers, docs (`docs/client-server-protocol-spec` notes the wire
  break + ticket v2 forced re-auth).
- **#76 `validation-543-followup`** — the deferred #543 half:
  `chown`/`chgrp`/`addGroupMember` targets must resolve via the
  resolver (now that ids are real); update the convention-documenting
  tests in `admin_access_validation_tests.rs`.

**Ordering:** #70 and #71 are independent and parallelizable
(different crates; the seam is the pure `principal64` function).
#72–#74 each require both; #72 and #73 are mutually independent;
#74 is the largest and should land alone. #75 strictly after #74;
#76 strictly after #74. **Risk flags:** #73 (ticket v2) and #74/#75
(BatchOp removal) are wire-visible breaks — acceptable pre-1.0 and
consistent with the no-compromise instruction, but they are the two
irreversible-once-shipped steps; everything else is internal. The #71
boot normalization is idempotent and re-runnable; its fail-closed
collision branch is the only way boot can newly refuse to start
(deliberate, message names the accounts).

---

## 8. What this design deliberately does NOT do

- **No 128-bit `Actor`/owner widening** (§2.2) — zero security gain
  over mint-unique `principal64`, triple-repo blast radius.
- **No remap/migration tooling for stale catalogue owners or group
  members** — per the recorded #548 decision.
- **No import of the phantom store** — §6.3; the safe direction is
  audit-and-drop.
- **No resurrection of `Role.permissions` / no general RBAC matrix
  engine.** The enforcement model is POSIX owner/group/mode + an
  explicit admin bypass + plain capability labels. Building a
  permission-matrix interpreter for a field nothing ever read would
  be maximalism, not correctness; if fine-grained RBAC is ever truly
  wanted, it deserves its own design doc against real requirements.
- **No per-request directory role re-check.** The epoch mechanism
  already gives next-request revocation for live sessions at O(1)
  in-memory cost (`user_directory.rs:230-240`); adding a directory
  read per request buys nothing the epoch+resume-lookup pair doesn't
  already guarantee.
- **No mutual-TLS / certificate-anchored identity** — out of scope
  exactly as recorded in
  `docs/dev-artifacts/design/resumption-ticket-channel-binding-512-decision.md`;
  this design keeps the 16-byte `user_id` as the anchor such a
  feature would later bind to.
- **No `RenameUser` op.** The architecture now permits one safely
  (stable id, mutable label — the group precedent), but it is a
  feature, not part of this remediation; noting only that nothing
  here forecloses it.

## 9. What needs the user's sign-off

1. The `principal64` projection with mint-time uniqueness (§2.2) as
   the concrete realization of the already-chosen Option A — vs
   widening to full 128-bit ids everywhere.
2. `Actor::Admin(u64)` — superuser keeps the bypass but stops
   masquerading as `System`, gaining ownership attribution (§2.3, §4).
3. Directory-canonical store unification: shamir-db user/role tables
   retired, DDL routed through ports, `CreateRole`/`DropRole`/
   `RenameRole` deleted from the wire surface (§3).
4. Superuser as a reserved flag + dedicated HMAC-gated `SetSuperuser`
   op + last-superuser lockout guard (§4).
5. Ticket v2 without roles + resume-time authoritative lookup (one
   forced re-auth per client at upgrade) (§5).
6. The three-part cutover posture, in particular: boot-time directory
   normalization (required to avoid admin lockout) and
   audit-without-import for the phantom store (§6).
