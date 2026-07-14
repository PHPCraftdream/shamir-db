בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Task #548: unbind security-`Actor` from `fxhash(username)` — design decision

Systemic finding spanning four audit reports (model-core F2, admin-ddl
#4, identity-session #1+#2). Investigated per this campaign's
established investigate → decision-doc → ask-before-implementing
pattern (precedent: #512, #533).

## The problem, precisely

`principal_id(username) = fxhash::hash64(username) & i64::MAX`
(`crates/shamir-types/src/access.rs:33-35`, re-derived per-request at
`crates/shamir-connect/src/server/session.rs`'s `Session::principal_id()`)
is the value stored as `Actor::User(id)` for every authorization
decision AND as the `owner` field in every catalogue record (tables,
databases, functions, etc.). It is:

- **Derived from a MUTABLE field** — nothing prevents renaming a user
  (no `RenameUser` op exists today, but nothing architecturally rules
  one out later), and even without rename, re-creating a user with the
  same name after a drop recomputes the SAME `principal_id`.
- **Non-cryptographic** — `fxhash` is designed for hash-table
  performance, not preimage/collision resistance. It is realistic that
  an attacker who controls a username string (anyone who can create a
  user) can search for a name whose `principal_id` matches — or is
  close enough to birthday-collide with — another principal's id, given
  enough attempts. This is fundamentally different from the codebase's
  OTHER identity mechanism, the 16-byte random `user_id` used for
  session revocation (`tickets_invalid_before_ns_by_user_id`), which
  IS cryptographically unguessable and IS stable across a user's
  lifetime.

Three concrete failure modes (confirmed by investigation, file:line
references below; re-verify against current code before implementing):

1. **Inheritance-on-recreate.** `DropUser` (`handle_drop_user`,
   `crates/shamir-db/src/shamir_db/execute/admin_users_roles.rs`)
   deletes the user record but does NOT touch any catalogue `owner`
   field that currently equals the dropped user's `principal_id` —
   confirmed no cascading reassignment exists. Those resources are left
   "orphaned" to a `principal_id` that no longer resolves to a live
   session. If a NEW user is later created with the SAME username, its
   freshly-computed `principal_id` is IDENTICAL to the old one (it's a
   pure function of the string), so the new user silently inherits
   every resource/group the old, unrelated "alice" ever owned. The
   16-byte `user_id`-based session-revocation mechanism does not help
   here — the new user has a fresh `user_id`; ownership was never keyed
   on that.
2. **Collision forging.** An attacker able to create/name users (or, in
   principle, rename one, once that op exists) can search for a
   username string whose `principal_id` matches a specific target
   owner's id, then create that user and inherit the target's
   ownership/group membership. `fxhash` is not preimage-resistant; the
   practical cost of this search was not benchmarked as part of this
   investigation (out of scope — the point is the id space offers no
   cryptographic assurance against it, unlike a random 63-bit id).
3. **id-0 aliases `Actor::System`.** `OWNER_SYSTEM = 0`
   (`access.rs`), and `Actor::from_owner_id(0) == Actor::System`
   unconditionally (`access.rs:47-54`). The existing comment ("a
   username hashing to 0 is astronomically unlikely") is a probabilistic
   hope, not an enforced invariant — no code path checks `principal_id(name)
   == 0` and rejects it. `handle_create_user`
   (`admin_users_roles.rs:16-82`) is also a blind `SetOp` upsert keyed on
   `name` with NO existence/uniqueness guard — re-creating an existing
   username silently overwrites the record (a separate, narrower bug
   independent of the hash-collision question, but compounding it: even
   a NON-adversarial double-`CreateUser` racing with itself has no
   protection).

## Why this is a decision, not a direct fix

The remedy touches: the on-disk catalogue's `owner` field semantics
(currently "hash of a name", would become "some other stable id" for
every future write, with EXISTING rows still holding the old
hash-based values — a migration/compat question), the `Session` →
`Actor` mapping, `DropUser`'s ownership-orphaning behavior (leave as
orphaned-but-now-truly-unguessable vs. actively reassign/block), and
whether a future `RenameUser` needs to preserve or intentionally sever
the identity link. This is squarely an architecture change requiring
explicit sign-off on the target representation and migration path
before any code lands — not something to force through inside a
"LOW/MEDIUM hardening" pass.

## Existing precedent in this codebase

Groups already solve exactly this class of problem correctly:
`create_group_as` (`access_control.rs`) allocates `group_id` from a
monotonic counter persisted under the `settings` key `"next_group_id"`,
incremented under a lock — a `group_id` is stable, collision-free by
construction, and NEVER a function of the (renameable) group `name`.
`RenameGroup` already exists and correctly renames the DISPLAY name
under the unchanged `group_id` — proving the "stable id, mutable
label" split already works in this codebase for one principal type.

Users ALSO already have a stable, non-hash, unguessable identifier: the
16-byte random `user_id` minted at creation (`FjallUserDirectory::insert`
in `crates/shamir-server/src/user_directory.rs`, a monotonic-counter-fed
128-bit value, NOT a hash), currently used ONLY for session-revocation
bookkeeping (`tickets_invalid_before_ns_by_user_id`). The fix this
decision is about is fundamentally: **stop deriving `Actor`/ownership
from a hash of the username, and start deriving it from (some
projection of) the SAME stable identifier session-revocation already
trusts** — closing the gap between "the id we use to decide whether your
session is still valid" and "the id we use to decide what you own",
which today are two unrelated numbers for the same person.

## Options

### Option A — `owner`/`Actor::User` carries the existing 16-byte `user_id` (or a stable 64-bit derivative of it)

Replace `principal_id(username)` everywhere with the user's actual
`user_id` (or, if a `u64` is structurally required by `Actor::User(u64)`
today, a stable 64-bit slice/fold of the 16-byte value — NOT a hash of
the username). `Session` already carries `user_id`; the `Session` →
`Actor` bridge (`session_actor` in `handler.rs`) would read it directly
instead of calling `principal_id()`.

- **Pros**: Reuses the ALREADY-random, already-collision-resistant
  identifier the codebase already trusts for revocation — no new
  cryptographic primitive, no new storage. Closes failure mode 1
  entirely (a user recreated under the same name gets a genuinely fresh
  random id, no inheritance). Narrows failure mode 2 to "guess a random
  63/128-bit value" (cryptographically infeasible) instead of "search
  fxhash's much smaller effective space." Failure mode 3 (id-0 aliasing)
  becomes trivially preventable (reject a freshly-minted id of exactly
  0, essentially never happens with a real RNG, and can be explicitly
  checked/retried at mint time since minting is under the orchestrator's
  control, unlike a hash of attacker-chosen input).
- **Cons**: EXISTING catalogue rows (every already-created table/db/
  function's `owner` field) hold the OLD hash-based value. A migration
  step is needed to remap "which `principal_id` used to mean which
  user" — requires either (a) a reverse index from old `principal_id` →
  username built BEFORE this migration runs (feasible only if every
  currently-live user's username is enumerable, which it is — scan the
  users table, recompute old-style `principal_id` for each, build the
  remap), or (b) accepting that pre-migration ownership becomes
  unresolvable/orphaned-to-System and must be manually re-chowned by an
  operator post-migration. This is real, one-time migration work, not
  free.
- **Blast radius** (per investigation): ~5 core files, ~10-15
  production call sites, mostly funneled through
  `Actor::to_owner_id()`/`Actor::from_owner_id()` — moderate, not
  sprawling, since the codebase already centralizes owner-id
  conversion at the `Actor` boundary.

### Option B — keep `principal_id` as a u64, but mint it as a monotonic counter (mirroring `group_id`) instead of hashing the username

Add a `next_principal_id` (or reuse/extend the existing `user_id`
minting counter) settings-keyed counter, exactly like `create_group_as`'s
`next_group_id`. `CreateUser` allocates a fresh counter value at
creation time (independent of the username string entirely); `Session`
would need to carry this value (fetched once at login, not recomputed
from the username per-request) instead of computing a hash.

- **Pros**: Same collision/inheritance-on-recreate guarantees as Option
  A (monotonic values never repeat, independent of a mutable name).
  Smaller change in one sense — no need to reconcile with the EXISTING
  16-byte `user_id` representation, since this introduces its own
  narrower counter, analogous to how groups already work.
- **Cons**: Introduces a THIRD identifier scheme for the same logical
  user (username, 16-byte `user_id`, now also a u64 `principal_id`
  counter) where option A would have reduced two-into-one. Same
  migration burden as Option A for existing catalogue rows (old
  hash-based `owner` values still need remapping). Doesn't leverage the
  session-revocation `user_id`'s already-proven stability — a
  parallel, redundant mechanism for essentially the same guarantee.

### Option C — keep username-derived ids for backward compatibility, add guards only

Keep `principal_id = fxhash(username)` as-is, but: (i) `CreateUser`
computes the candidate `principal_id`, rejects it if `== 0` (retry
requires renaming, not silently proceeding) OR if it collides with a
DIFFERENT existing username's `principal_id` (requires a reverse
`principal_id → username` index, maintained alongside the primary
`username → record` store); (ii) `DropUser` either blocks dropping a
user who still owns resources (forcing an explicit re-chown first) or
actively walks the catalogue re-assigning owned resources to
`Actor::System` before allowing the drop, closing failure mode 1 by
never leaving an inheritable dangling id.

- **Pros**: No catalogue migration needed — existing `owner` values
  keep their meaning. Smallest code diff of the three options.
- **Cons**: Does NOT close failure mode 2 (forged-collision) at all —
  `fxhash` remains non-cryptographic; two DIFFERENT usernames can still
  collide, and the guard in (i) only catches an EXACT re-derivation
  collision on creation, not two distinct usernames whose hashes happen
  to match (that pair could be created in either order and the SECOND
  one would be rejected by the reverse-index check — so this actually
  DOES catch cross-username collisions too, correcting the above: the
  reverse index makes (i) closer to complete for the create-time case).
  It does NOT protect against a FUTURE `RenameUser` colliding with an
  existing id (would need the same reverse-index check re-run on
  rename). It is fundamentally a patch on a design whose core property
  (id derived from mutable, attacker-influenced input) remains — every
  future feature touching identity has to remember to consult the
  reverse index, whereas Options A/B make the class of bug structurally
  impossible by using an identifier no one chooses.

## Orchestrator's recommendation

**Option A** — reuse the existing 16-byte `user_id`. It requires no new
minting mechanism (unlike B) and closes the underlying design flaw
structurally rather than patching around it (unlike C), at the cost of
a one-time catalogue migration this investigation believes is tractable
(the remap only needs each live user's CURRENT username, which is
enumerable). This recommendation is not a final decision — the user's
sign-off below determines what actually gets implemented.

## What needs the user's sign-off before any code

1. Which option (A / B / C / something else).
2. If A or B: how to handle EXISTING catalogue rows' stale
   hash-derived `owner` values — migrate-with-remap, or accept orphaning
   to System and require manual re-chown.
3. `DropUser`'s ownership-disposition policy going forward (leave
   orphaned to the now-dead stable id — safe under A/B since the id is
   never reused; block-until-rechowned; or auto-reassign to System).
4. Whether this migration should be a synchronous startup step (repo
   opens, migration runs once, done) or an explicit, operator-invoked
   admin command — affects blast-radius/rollback risk framing.

## Decision (recorded)

The user chose, via `AskUserQuestion` on 2026-07-12:

1. **Option A** — reuse the existing 16-byte `user_id` as the
   `Actor`/`owner` identity, replacing `fxhash(username)`.
2. **No migration** — a clean, hard cutover. Existing catalogue rows'
   `owner` fields keep whatever old hash-derived value they already
   have; no remap/reverse-index-and-rewrite step is built. Those values
   simply stop being meaningful under the new scheme (in effect,
   pre-cutover ownership becomes stale data — the user explicitly
   accepted this rather than building migration tooling).
3. **`DropUser` — no new policy work.** Consistent with "hard cutover,
   no migrations": no blocking-on-ownership or auto-reassignment logic
   is being added. `DropUser`'s existing behavior (delete the user
   record, leave any `owner`-tagged rows as-is) is UNCHANGED — this is
   now safe-by-construction under Option A, since a stable `user_id` is
   never reused/regenerated for a different person, so an "orphaned"
   owner value can never be silently inherited by a future different
   user the way a re-hashed username could.

Implementation proceeds under this decision — see
`docs/dev-artifacts/prompts/audit/<NN>-security-548-*.md` for the implementation
brief.

