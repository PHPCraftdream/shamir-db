בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Brief: close the unlocked group-record TOCTOU race (task #563)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Context

Found by adversarial review of task #552 (Root/User/Group real
permission model). `crates/shamir-db/src/shamir_db/system_store.rs`'s
`add_group_member`, `remove_group_member`, and `set_group_owner`
(lines ~704-776) are all unlocked read-modify-write:
`load_group(group_id)` → mutate one field (members list, or owner) →
`save_group(...)` (a full-record overwrite of `group_id`/`name`/
`members`/`owner`). None of them take any lock. `group_id_lock`
(`crates/shamir-db/src/shamir_db/shamir_db/core.rs:76`,
`access_control.rs:392`) is a single global `Arc<Mutex<()>>` scoped
ONLY to `create_group_as`'s `next_group_id` counter bump — it is never
acquired by any of the group-record-mutation paths above.

**Concrete race**: two concurrent ops on the SAME `group_id` — e.g.
`set_group_owner(gid, Bob)` (chown) racing with
`add_group_member(gid, x)` — each independently reads the pre-mutation
record, applies its own single-field change, and writes back the
WHOLE record. Whichever `save_group` call lands second wins outright,
silently discarding the other call's change (last-writer-wins on the
full record, not per-field).

**A fourth call site with the exact same shape, NOT named in the
original review finding — found during this brief's investigation**:
`rename_group_as` (`crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:474-507`)
also does an unlocked read (`group_members(gid)` +
`load_group(gid)`'s owner field) → mutate (new name) → `save_group(...)`
sequence. It is the SAME race class as the three named in the original
finding and belongs in this task's scope.

## Design

Read `crates/shamir-db/src/shamir_db/shamir_db/core.rs`'s
`admin_user_locks` field (line ~58) and its accessor method (line
~471) IN FULL — this is the established, idiomatic pattern for exactly
this problem elsewhere in the codebase (`DashMap<Key, Arc<Mutex<()>>,
THasher>`, get-or-insert-then-lock, entries deliberately leak forever
since admin ops are rare and the key space is small/bounded — see the
identical `repo_create_locks` field for a second precedent). Mirror
this pattern exactly, do not invent a new one:

1. Add a new field to `ShamirDb` (`core.rs`):
   `group_member_locks: Arc<DashMap<u64, Arc<Mutex<()>>, THasher>>`
   (keyed by `group_id` — a `u64`, unlike `admin_user_locks`'s
   `String` key, since groups are id-keyed not name-keyed). Initialize
   it in the constructor alongside `admin_user_locks`/
   `repo_create_locks`. Add an accessor method mirroring
   `admin_user_locks()`/`repo_create_locks()`'s exact shape and doc
   comment style.

2. In `crates/shamir-db/src/shamir_db/shamir_db/access_control.rs`,
   acquire this per-`group_id` lock (get-or-insert the `Arc<Mutex<()>>`
   for the group_id, then `.lock().await`) around the ENTIRE
   read-modify-write sequence — from the initial `load_group`/
   `group_members` read through the final `save_group` call — in all
   FOUR call sites:
   - `add_group_member_as` (line ~522-530)
   - `remove_group_member_as` (line ~545 onward — read the method in
     full, it's cut off in this brief's excerpt)
   - `set_resource_meta`'s `ResourcePath::Group` arm (line ~334-340)
   - `rename_group_as` (line ~474-507)

   The lock must be held across the WHOLE sequence in each case
   (read → mutate → write), not just around the final write — that's
   the entire point of closing a check-then-act race.

3. Do NOT move the lock-acquire/release logic into
   `system_store.rs`'s methods themselves (`add_group_member`,
   `remove_group_member`, `set_group_owner`, `save_group`) — the lock
   map lives on `ShamirDb`, and `system_store.rs`'s `SystemStore`
   struct doesn't have access to it. Keep `system_store.rs`'s methods
   exactly as they are (thin, unlocked primitives); the locking
   responsibility belongs at the `ShamirDb`-level call sites in
   `access_control.rs`, matching how `admin_user_locks` is used
   (acquired at the `ShamirDb`-level GrantRole/RevokeRole call sites,
   not inside whatever lower-level storage helper they call).

4. `create_group_as` is UNCHANGED — it already has its own correct
   locking (`group_id_lock`) for the counter-bump-then-insert sequence,
   which is a DIFFERENT operation (allocating a NEW group_id) than the
   four above (mutating an EXISTING group_id's record). Do not merge
   or conflate the two locks — a per-existing-group_id lock for
   mutation is orthogonal to the global counter lock for allocation.

5. `drop_group_as` (`access_control.rs:442-445`) — investigate whether
   it needs the SAME per-group_id lock. It calls
   `system_store.remove_group(group_id)` directly (a delete, not a
   read-modify-write on the record's fields) — determine whether a
   concurrent add/remove-member/set-owner/rename racing a concurrent
   drop could leave a genuinely bad state (e.g. resurrecting a group
   record after its "removal" wins a race against a slower
   read-modify-write that was already in flight when the drop started).
   If you find this is a real, closeable gap, close it with the SAME
   lock (acquire it in `drop_group_as` too); if you determine it's a
   different, out-of-scope concern (e.g. the delete is idempotent/safe
   regardless), say so explicitly in your report rather than silently
   leaving it unexamined.

## Red tests required first

Write a concurrent-stress regression test (in
`crates/shamir-db/src/shamir_db/tests/` — check the existing test file
layout for where group-related tests already live, e.g. wherever
task #552's own tests for `add_group_member`/`set_group_owner` are, and
add to that file rather than creating a new one unless none exists)
that:
- Creates a group.
- Spawns N concurrent tasks (e.g. `tokio::spawn`), some calling
  `add_group_member_as`/`remove_group_member_as` with distinct member
  ids, one calling a chown-equivalent (`set_resource_meta` on
  `ResourcePath::Group`) to a specific owner, targeting the SAME
  `group_id`.
- Awaits all of them, then reloads the group record and asserts BOTH
  that every member add/remove that should have "won" is correctly
  reflected AND that the owner change survived (wasn't silently
  reverted by a racing member-mutation's stale-read overwrite).
- Confirm this test FAILS (reproduces the race) against the CURRENT,
  unmodified code before you implement the fix — this is the whole
  point of a red test. If it doesn't reliably fail without the fix
  (races can be flaky to reproduce), consider a smaller, more
  deterministic reproduction (e.g. manually interleave two calls via
  explicit `tokio::sync::Notify`/barrier synchronization to force the
  exact interleaving that loses an update, rather than relying on pure
  scheduling luck) — a flaky-red test is not acceptable proof.

## Out of scope

- Do NOT touch `create_group_as`'s existing `group_id_lock` — different
  operation (allocation vs. mutation), unrelated to this fix.
- Do NOT touch anything in `crates/shamir-server` or the wire/HMAC
  layer — this is purely a `shamir-db`-internal concurrency-correctness
  fix, no wire-shape or authorization-model change.
- Do NOT attempt the CAS/conditional-write alternative design
  mentioned as option (b) in the original finding, or the
  single-serialized-helper alternative (c) — the per-group_id lock (a)
  is the established, idiomatic pattern in this codebase (two existing
  precedents: `admin_user_locks`, `repo_create_locks`); use it, don't
  design something new.

## Definition of done

- `group_member_locks` field + accessor added to `ShamirDb`, mirroring
  `admin_user_locks`'s exact shape/doc style.
- All four (or five, if `drop_group_as` is determined to need it) call
  sites acquire the per-`group_id` lock across their full
  read-modify-write sequence.
- A concurrent-stress regression test proves the race is closed (fails
  without the fix, passes with it).
- `cargo check --workspace --all-targets` clean.
- `cargo fmt -p shamir-db -- --check` clean (only the file(s) you
  touch — do NOT run unscoped `cargo fmt --all`; leave pre-existing
  drift in untouched files alone).
- `cargo clippy -p shamir-db --all-targets -- -D warnings` clean except
  the one remaining pre-existing, already-tracked issue: this crate's
  own dependency on `shamir-engine`'s `read_planner.rs:466`
  `type_complexity` lint has JUST been fixed by task #562 (commit
  `ab86b4b7`) — confirm your clippy run is actually clean now, not
  just "matches the old known-issue list."
- `./scripts/test.sh -p shamir-db --full` green.

## Report

When done, produce a final summary (not a bare tool call): every file
changed, the full text of the new concurrent-stress test (including
how you proved it genuinely reproduces the race before your fix), the
gate command outputs, your finding on `drop_group_as` (§5 above —
fixed, or explicitly out of scope with reasoning), and every
discrepancy between this brief's assumptions and the actual code you
found.
