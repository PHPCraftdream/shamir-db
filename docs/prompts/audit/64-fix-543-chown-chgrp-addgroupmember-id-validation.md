Task #543 — validate that `chown`/`chgrp`/`add_group_member` target ids
resolve to a real existing principal/group before writing them into the
catalogue, and explicitly forbid `chown` to `OWNER_SYSTEM` (id `0`) for a
non-System actor.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Why this matters (audit finding, admin-ddl #5 / identity-session #5)

`crates/shamir-db/src/shamir_db/execute/admin_access.rs` (confirmed at
the time of this brief — re-verify line numbers, code may have shifted):

- `handle_chown` (~line 84): `meta.owner = Actor::from_owner_id(op.owner)`
  — writes a raw client-supplied `u64` straight into the catalogue, no
  existence check.
- `handle_chgrp` (~line 125): `meta.group = op.group` — same, for a group
  id.
- `handle_add_group_member`/`handle_remove_group_member` (~lines 282,
  319): `self.shamir.add_group_member(group_id, op.user)` — `op.user` is
  never checked against the user directory.

**Impact**: the caller already had to hold `Manage` on the resource to
reach these handlers (`authorize_access(..., Action::Manage)` runs first
in every one of these — this is NOT a privilege-escalation bug, the
actor already had legitimate management rights over the resource). But
within that already-privileged action, an owner can:
- `chown` a resource to `op.owner == 0` (`OWNER_SYSTEM`) — the resource
  becomes System-owned; only `Actor::System` (or whoever holds root
  Manage) can manage it thereafter. For a non-System actor this is a
  one-way, irrecoverable self-lockout (a footgun/DoS on their own
  resource, or if done maliciously to someone else's resource they still
  had Manage on, a way to orphan it from its rightful owner).
- `chown`/`chgrp`/`add_group_member` to a nonexistent principal/group id
  — the resource is silently orphaned to an id nobody currently holds.
  Per the identity audit's related finding (principal ids are
  `fxhash(username) & i64::MAX` — see task #548, NOT this task's
  concern to fix), a dangling owner-id can later become inheritable by a
  FUTURE user whose username happens to hash to that same id, silently
  handing them ownership of someone else's old resource. This task closes
  the "write a dangling id at all" gap; #548 is the separate, larger
  identity-model redesign — do not conflate the two or attempt #548's
  fix here.

## Read first

- `crates/shamir-db/src/shamir_db/execute/admin_access.rs` — the four
  handlers above, and their existing `authorize_access(..., Manage)`
  calls (keep those, you're adding validation AFTER auth, before the
  catalogue write).
- `crates/shamir-types/src/access.rs`: `principal_id(username) -> u64`,
  `Actor::from_owner_id`/`to_owner_id`, `OWNER_SYSTEM` (the `0` sentinel).
- `crates/shamir-db/src/shamir_db/shamir_db/access_control.rs`'s
  `access_tree` method — it already builds a `name_of: TFxMap<u64,
  String>` by iterating `self.system_store.load_users()` and computing
  `principal_id(uname)` for each entry (~lines 516-528 at the time of
  this brief). This is the SAME O(N) scan-and-hash-compare idiom you need
  for "does user id X exist" — there is no id-keyed user store (users are
  name-keyed; the id is *derived* from the name via `principal_id`), so
  reuse this exact pattern rather than inventing a different one. This is
  an admin-rate DDL op, not a hot path, so an O(N) scan over
  `load_users()` is an accepted, deliberate trade-off here (unlike the
  hot-path `scc::*::len()` ban in CLAUDE.md, which is about O(1)
  cardinality on frequently-hit paths).
- `self.shamir.load_group(group_id)` (via `system_store.load_group`,
  already used by `group_members`/`user_in_group` — an id-keyed lookup,
  not a scan) is the correct existence check for group ids (`op.group` in
  `ChgrpOp`, and the group id resolved by `resolve_group_id` for
  `add_group_member`'s GROUP side — that one's already resolved via
  `resolve_group_id`, which already errors on a nonexistent group; only
  the USER-id side of `add_group_member` is unchecked).

## The fix

**`handle_chown`**: before `meta.owner = Actor::from_owner_id(op.owner)`:
1. If `op.owner == OWNER_SYSTEM` (`0`) AND `self.actor` is not
   `Actor::System`, reject with a clear error (new or reused error code —
   check the existing `err`/`err_code` closures already defined in this
   function for the idiom; something like `"invalid_owner"` or
   `"owner_not_found"`, pick one and use it consistently across all three
   handlers below).
2. Otherwise, verify `op.owner` resolves to a real user: scan
   `self.shamir.system_store()... load_users()` (or whatever the correct
   accessor is — `access_tree`'s existing code shows the call chain) and
   confirm some user's `principal_id(name) == op.owner`. If none match,
   reject with the same error code.

**`handle_chgrp`**: when `op.group == Some(g)`, verify `g` resolves to a
real group via `load_group(g)` (`Ok(Some(_))` → valid, `Ok(None)` or
`Err` → reject). `op.group == None` (clearing the group) needs no
validation — always allowed.

**`handle_add_group_member`** (and consider `handle_remove_group_member`
too, though removing a nonexistent membership is arguably a harmless
no-op — investigate and decide, document your reasoning either way):
verify `op.user` resolves to a real user via the same `principal_id`
scan-and-compare as `chown`. The GROUP side is already validated by the
existing `resolve_group_id` call — do not duplicate that check.

Use a SINGLE shared error code across all these new rejections (add a
small private helper in this file, or module-level, that does the
`load_users()` scan-and-compare once, since chown and add_group_member
both need it — do not copy-paste the scan twice).

## Explicit scope boundary

Do NOT attempt to fix the underlying identity-model problem (principal
ids are non-cryptographic hashes of a MUTABLE username, ids can collide,
users can be renamed and orphan their old id) — that is task #548's
job, a DECISION task requiring investigation + a decision doc + explicit
user sign-off before any code. This task ONLY closes "don't write an id
that doesn't currently resolve to anything" — a narrower, purely
additive validation that doesn't touch the identity model itself.

## Test requirement

In `shamir-db`'s test suite:
- `chown` to a nonexistent user id is rejected (not silently written).
- `chown` to `OWNER_SYSTEM` (`0`) by a non-System actor is rejected.
- `chown` to `OWNER_SYSTEM` by `Actor::System` itself still succeeds (this
  is the LEGITIMATE path — System reassigning a resource back to itself,
  or an admin explicitly making a resource System-owned via an
  System-run/root session — do not break this).
- `chgrp` to a nonexistent group id is rejected; `chgrp` to `None`
  (clearing) still succeeds.
- `add_group_member` with a nonexistent user id is rejected; with a real
  existing user id still succeeds (don't regress the happy path — find
  and reuse the existing group-membership tests for the correct-path
  assertion).

## Test scope

```
./scripts/test.sh -p shamir-db
```

## Verification (lighter per-task gate, agreed for this campaign)

```
cargo check --workspace --all-targets
./scripts/test.sh -p shamir-db
```
Full fmt/clippy/test --full is FINAL-GATE's (#529's) job. This task does
NOT block FINAL-GATE (MEDIUM footgun/DoS requiring already-owner
privilege, not a privilege-escalation bug) — do not add it to #529's
blockedBy.

## Report format

```
[Implementation] Status: fixed / scoped-down-with-followup
  > chown: id-existence check + OWNER_SYSTEM-non-System-forbidden,
    confirmed System-actor chown-to-System still works
  > chgrp: group-existence check, confirmed group:None (clear) still
    works unconditionally
  > add_group_member: user-id-existence check; remove_group_member
    decision (validated too / left as harmless no-op) + reasoning
  > Shared error code / helper used consistently, no duplicated scan
  > New tests: confirmed RED before the fix, GREEN after, for each case
    above
[Verification]
  cargo check --workspace --all-targets: pass/fail
  ./scripts/test.sh -p shamir-db: pass/fail
```

Given this touches the DAC ownership-mutation path used by every
`chown`/`chgrp`/`add_group_member` admin op, this MUST go through an
adversarial review pass before committing — same discipline as
#537/#540/#541/#542 this campaign. If that review finds a genuine bug,
the orchestrator fixes it directly (never re-delegates), re-verifies,
and sends the fix through a second review pass before committing.
