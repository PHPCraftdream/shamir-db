בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Brief: close the group-name uniqueness gap across create/rename (task #570)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Context

Found by adversarial review of task #563 (group-record TOCTOU fix),
then deepened by direct investigation.

**`create_group_as` has NO name-uniqueness check at all** — not a race,
a complete absence.
`crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:397-437`
allocates a new `group_id` (correctly serialized under `group_id_lock`
for the counter bump) and calls
`self.system_store.save_group(group_id, name, &[], actor.to_owner_id())`
unconditionally. **Two SEQUENTIAL (non-concurrent) `create_group("ops")`
calls both succeed**, producing two groups both named "ops" — no
concurrency is even needed to trigger this; confirmed by reading the
function, no existing test in
`crates/shamir-db/src/shamir_db/tests/group_tests.rs` covers this at
all (grepped for "duplicate"/"already exists"/"KeyExists" — zero
hits).

**`rename_group_as` DOES check uniqueness** (`access_control.rs:517-525`
scans `self.system_store.load_groups()` for a conflicting name) but
this check is only protected by task #563's per-`group_id` lock, which
serializes mutations on the SAME `group_id` — not across DIFFERENT
`group_id`s. Two concurrent renames targeting two different groups to
the same target name each lock only their own id, both pass the
uniqueness scan before either write lands, and both succeed.

Both gaps break the invariant `resolve_group_id`/`GroupRef::Name`
relies on (a name resolves to exactly one group) — with two groups
sharing a name, name-based resolution becomes nondeterministic
(whichever a linear scan finds first).

## Design (already decided — implement this, do not re-litigate)

Reuse the EXISTING `group_id_lock`
(`crates/shamir-db/src/shamir_db/shamir_db/core.rs:88`, a single global
`Arc<Mutex<()>>` — already acquired in `create_group_as` at line 403
for the counter bump) as the SINGLE point of serialization for
name-uniqueness across the WHOLE group namespace. Do NOT introduce a
second new lock, and do NOT extend task #563's `group_member_locks`
(that map is per-`group_id`, scoped to field-mutation RMW on an
EXISTING group — a fundamentally different concern from
allocating/renaming into the shared name namespace).

1. **In `create_group_as`** (`access_control.rs:397-437`): while STILL
   HOLDING `group_id_lock` (already acquired at line 403 for the
   counter), ADD a name-uniqueness check — scan
   `self.system_store.load_groups().await?` for an existing group whose
   `name` matches, BEFORE allocating the id / calling `save_group`.
   Reject with the same error shape `rename_group_as` already uses:
   `Err(DbError::KeyExists(format!("group '{}' already exists", name)))`.
   Read `rename_group_as`'s existing conflict-scan code
   (`access_control.rs:518-525`) for the exact iteration pattern to
   mirror (it filters on `g.get("name").and_then(|v| v.as_str()) ==
   Some(name)` — for `create_group_as` you don't need the
   `group_id != gid` exclusion since there's no existing group_id to
   exclude yet).

2. **In `rename_group_as`** (`access_control.rs:496-540`): in addition
   to the existing per-`group_id` lock from #563
   (`group_member_locks`, already acquired at line 510-515), ALSO
   acquire `group_id_lock` — held across the SAME span, from before the
   uniqueness scan (line 518) through the `save_group` call (line
   538ish). Acquire both locks (order doesn't matter for deadlock
   purposes here since `group_id_lock` and any single `group_member_locks`
   entry are never both held by two DIFFERENT tasks in the opposite
   order elsewhere in the codebase — confirm this remains true after
   your change by checking every other `group_id_lock` acquisition
   site, there should only be the one in `create_group_as` today).
   Remove (or update) the doc comment added during #563's review at
   `access_control.rs` (search for "NOTE: this check is NOT protected
   against a cross-group_id race" — task #563 added this caveat
   comment; once you close the gap here, update/remove that comment so
   it doesn't claim a limitation that no longer exists).

3. **This gives ONE global lock (`group_id_lock`) covering the entire
   group-NAME namespace for both allocation paths** (create + rename),
   while `group_member_locks` (#563) remains responsible for the
   separate, orthogonal concern of same-`group_id` field-mutation
   races (add/remove-member, chown, drop). Do not conflate the two.

## Red tests required first

In `crates/shamir-db/src/shamir_db/tests/group_tests.rs`:

1. **Sequential duplicate-create test** (no concurrency needed — this
   reproduces the gap on its own): `create_group("ops")` then
   `create_group("ops")` again — the second call must return an error
   (confirm this currently SUCCEEDS on the unmodified code, i.e. is a
   genuine red test, before implementing the fix).

2. **Concurrent-stress test** mirroring #563's
   `group_member_toctou_concurrent_mutations_lose_no_update` harness
   (barrier-synchronized, `multi_thread` flavor, N concurrent tasks
   released simultaneously): spawn several concurrent `create_group_as`
   calls all targeting the SAME name — assert exactly ONE succeeds and
   the rest return `KeyExists` (or equivalent), never more than one
   group ending up with that name. Confirm this fails (more than one
   survives) on the unmodified code before the fix, passes
   deterministically after.

3. **Concurrent create-vs-rename test**: one group already exists with
   name "alpha"; spawn a concurrent `create_group_as("beta")` and
   `rename_group_as(<some other existing group>, "beta")` — assert only
   one of the two lands with the name "beta", the loser gets a clear
   conflict error, not a silent double-assignment. (This is the
   cross-path case the original review finding specifically called
   out — a create racing a rename on the same target name.)

## Out of scope

- Do NOT touch `group_member_locks` (task #563) or any of its 5 call
  sites — that lock's job is unrelated to this one.
- Do NOT touch the wire/HMAC layer, `shamir-server`, or any client code
  — this is purely a `shamir-db`-internal correctness fix.
- Do NOT introduce a new lock type or redesign the group-CRUD
  authorization gates (`authorize_group_manage_or_root`,
  `Manage(Root)` for create) — those stay exactly as they are.

## Definition of done

- `create_group_as` rejects a duplicate name (sequentially AND
  concurrently).
- `rename_group_as` rejects a duplicate name across DIFFERENT
  `group_id`s, even under concurrency (closes the gap #563's review
  flagged).
- The stale "NOT protected against a cross-group_id race" comment
  added during #563 is corrected/removed to reflect the closed gap.
- All 3 red tests above are added, confirmed failing on the unmodified
  code, passing after the fix.
- `cargo check --workspace --all-targets` clean.
- `cargo fmt -p shamir-db -- --check` clean (touched files only — do
  NOT run unscoped `cargo fmt --all`).
- `cargo clippy -p shamir-db --all-targets -- -D warnings` clean.
- `./scripts/test.sh -p shamir-db --full` green.

## Report

When done, produce a final summary (not a bare tool call): every file
changed, the full text of all 3 new tests (including proof each one
genuinely reproduces its gap before the fix — paste the failing-test
output), the gate command outputs, and every discrepancy between this
brief's assumptions and the actual code you found.
