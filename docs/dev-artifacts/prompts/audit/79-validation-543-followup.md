בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Brief: re-enable chown/chgrp/addGroupMember target-existence validation (task #561)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Context

This is the deferred half of task #543. Read
`crates/shamir-db/src/shamir_db/tests/admin_access_validation_tests.rs`
lines 1-45 in full first — it's the authoritative record of what #543
originally attempted and why it was scoped down. Summary: #543 tried to
make `chown`/`chgrp`/`add_group_member` reject targets (owner/group/
member ids) that don't resolve to a real, existing record. It broke 14+
pre-existing tests across `shamir-db` and `shamir-server` because, at
the time, "does this numeric id correspond to a real user" was
incoherent — `principal_id` was an fxhash of a MUTABLE username (task
#548's problem), and there were two desynced user/role stores (task
#549's problem). Both are now resolved: task #559 landed a real
`PrincipalResolver` trait (`crates/shamir-db/src/shamir_db/ports.rs`)
backed by the durable `FjallUserDirectory`, injected into `ShamirDb` via
`.with_principal_resolver(...)`.

**This task does NOT re-attempt #543's original, broader design.**
Investigation (already done, load-bearing for this brief's scope split
below) shows the three targets have different dependencies:

- `handle_chown`'s owner id (`op.owner: u64`, a `principal64` value) and
  `handle_add_group_member`'s member id (`op.user: u64`) are USER
  targets — these genuinely needed #559's resolver to become checkable
  at all (a numeric id is meaningless without a directory to resolve it
  against).
- `handle_chgrp`'s group id (`op.group: Option<u64>`) is a GROUP
  target — groups have ALWAYS been directly checkable via the
  already-existing `group_id_exists` helper in
  `crates/shamir-db/src/shamir_db/execute/admin_access.rs` (used today
  by `handle_add_group_member` for the GROUP side of a `GroupRef::Id`).
  This check never depended on the identity-model work at all; it was
  bundled into #543's deferral only because enforcing it broke
  pre-existing tests, not because it was theoretically blocked.

This asymmetry drives the design below: the two USER-target checks are
**gated on a resolver being installed** (skip when absent — chown/
add_group_member are core ACL ops that have never required an injected
port, and hard-failing them in every test that doesn't wire a resolver
would be enormous, unjustified scope creep). The GROUP-target check is
**unconditional** (no port dependency, so no reason to gate it).

## Scope

### §1. `handle_chown` — owner existence check (resolver-gated)

In `crates/shamir-db/src/shamir_db/execute/admin_access.rs`'s
`handle_chown`, after the existing `OWNER_SYSTEM` guard (lines 110-125)
and its explanatory NOTE comment (lines 126-136, which should be
UPDATED, not left stale, once this lands), add: if `op.owner !=
OWNER_SYSTEM` AND `self.shamir.principal_resolver()` is `Some(resolver)`,
require `resolver.resolve(op.owner).is_some()` — reject with the
existing `ERR_INVALID_OWNER` ("invalid_owner") code and a clear message
if it doesn't resolve. When no resolver is installed, skip the check
entirely (current permissive behavior preserved).

### §2. `handle_chgrp` — group existence check (unconditional)

In the same file's `handle_chgrp`, after the existing NOTE comment
(lines 180-184, also to be updated/removed), when `op.group` is
`Some(gid)`, require `self.group_id_exists(gid).await?` — reject with
`ERR_INVALID_OWNER` if the group doesn't exist. `None` (clearing the
group) is never checked (matches the existing early-return pattern
already in the file for the `None` case, if any — verify and preserve
it). This check does NOT depend on any resolver/port — `group_id_exists`
already works unconditionally today.

### §3. `handle_add_group_member` — member existence check (resolver-gated)

In the same file's `handle_add_group_member`, after the existing GROUP-
side `group_id_exists` check (lines 356-370, unchanged) and its stale
NOTE comment about the deferred member check (lines 372-378, to be
updated/removed), add: when `self.shamir.principal_resolver()` is
`Some(resolver)`, require `resolver.resolve(op.user).is_some()` before
calling `add_group_member_as` — reject with `ERR_INVALID_OWNER` if it
doesn't resolve. When no resolver is installed, skip (current
permissive behavior preserved).

### §4. Fix the ~6 known-breaking pre-existing test call sites

Investigation found exactly these call sites will break (all `chgrp`
to a group id that was never created — the unconditional §2 check):

- `crates/shamir-db/src/shamir_db/tests/admin_access_validation_tests.rs`
  around line 294-296: `chgrp(..., Some(999_999))` — this is one of the
  test-file's OWN "documents the deferral" tests; per the DoD below,
  this whole file's deferral-documentation tests need to become real
  positive+negative assertions, so this specific call gets rewritten
  as part of that broader change, not patched in isolation.
- `crates/shamir-server/tests/hmac_gate.rs` around lines 741, 766-768,
  799: three `chgrp(..., 3)` calls (variations with/without a valid
  HMAC) targeting a group id `3` that's never created via
  `create_group`. Fix by creating a real group first (via the same
  `ShamirDb`/harness the test already uses) and chgrp-ing to that
  group's real id instead of the literal `3`. Preserve each test's
  actual point (HMAC gate behavior) — only the group-id needs to
  become real, not the test's assertions about HMAC acceptance/
  rejection.

Do a repo-wide grep yourself for `chgrp(` / `ChgrpOp` across
`crates/shamir-db/` and `crates/shamir-server/` test directories to
confirm this list is complete before/after your change — the
investigation above was thorough but re-verify, since new tests may
have been added since it ran.

Do NOT touch: chown-to-arbitrary-uncreated-id tests (e.g. `chown(...,
7)`, `chown(..., 1)`, `chown(..., 999_999)`) — these constructed
`ShamirDb` WITHOUT a resolver installed, so §1's check is a no-op for
them and they should keep passing unchanged. Same for
`add_group_member`-to-uncreated-user-id tests without a resolver
installed (§3's check is a no-op there too). If you find one of these
DOES break, that's a signal your resolver-gating isn't actually gated
correctly — stop and investigate rather than patching the test.

### §5. Flip `admin_access_validation_tests.rs`'s deferral-documentation
tests into real assertions

This file's whole point (per its own module doc, lines 1-45) was to
document what #543 couldn't land. Now that §1-§3 land, update it to
prove the REAL current behavior:

- **Positive** (with a resolver installed, via a mock or the real
  `crates/shamir-server`-style adapter — pick whichever this crate's
  existing test conventions favor; check `access_tree_tests.rs`'s
  `MockAliceResolver`/`root_user_group_meta_tests.rs`'s `MockResolver`
  from task #559 for the established mock pattern in THIS crate, don't
  invent a new one): `chown`/`add_group_member` to a nonexistent
  principal64 is rejected with `invalid_owner`; to an existing one
  (resolvable via the mock) succeeds.
- **Negative/degrade**: with NO resolver installed, `chown`/
  `add_group_member` to an arbitrary nonexistent id still succeeds
  (permissive fallback preserved) — keep (or add, if missing) a test
  proving this explicitly, so a future regression that accidentally
  makes the check unconditional gets caught.
- **Group check**: `chgrp` to a real (created) group id succeeds; to a
  nonexistent one fails with `invalid_owner` — this one is unconditional
  (no resolver needed), so a single positive + single negative test
  suffices, no with/without-resolver split needed.
- Update the module's top-of-file doc comment (lines 1-45) to reflect
  what's now actually enforced, replacing the "scoped down, deferred"
  framing with the real, current design (resolver-gated for user
  targets, unconditional for group targets) and a pointer to this
  task's number.

## Out of scope

- Do NOT touch `handle_remove_group_member`'s deliberate
  no-existence-check design (lines 419-425 of `admin_access.rs`) — its
  comment explains why removal is intentionally unchecked (idempotent
  set-removal, nothing can be orphaned). Leave it exactly as-is.
- Do NOT change `resolve_group_id`'s existing `GroupRef::Name`
  resolution behavior (a scan) — only `GroupRef::Id`'s pass-through gap
  is in scope, and that gap is ALREADY closed (pre-#561, via
  `group_id_exists` in `handle_add_group_member`) — §2 just extends the
  same closed gap to `handle_chgrp`.
- Do NOT touch task #560 (client builder sync) or #565's items — unrelated.
- Do NOT re-attempt a user-must-exist check with NO resolver installed
  (i.e., do NOT fall back to any hash-based bridge, Store B, or scan of
  some other store) — the whole point of gating on `principal_resolver()`
  is that absence means "cannot check, so don't."

## Red tests required first

Per this repo's TDD discipline: write the failing tests described in
§5 FIRST (they should fail against the current, unmodified handlers —
confirm the "with-resolver, nonexistent target rejected" test genuinely
fails before your change), then implement §1-§3, then fix the §4 call
sites, confirming each test now passes for the right reason.

## Definition of done

- `handle_chown` rejects `op.owner` (non-`OWNER_SYSTEM`) that doesn't
  resolve via an installed `PrincipalResolver`; no-op when absent.
- `handle_add_group_member` rejects `op.user` that doesn't resolve via
  an installed `PrincipalResolver`; no-op when absent.
- `handle_chgrp` rejects `op.group` (when `Some`) that doesn't resolve
  via `group_id_exists`, unconditionally.
- `admin_access_validation_tests.rs` rewritten per §5: real positive/
  negative/degrade assertions, stale deferral-doc comment updated.
- The ~6 pre-existing `chgrp`-to-nonexistent-group test call sites (§4)
  fixed to use a real, created group id.
- Every OTHER pre-existing test (chown/add_group_member to an
  uncreated id, without a resolver installed) still passes unchanged —
  confirm via a full `-p shamir-db -p shamir-server --full` run.
- `cargo check --workspace --all-targets` clean.
- `cargo fmt -p shamir-db -p shamir-server -- --check` clean (only
  touched files matter — do NOT run unscoped `cargo fmt --all`; if
  `--check` shows drift in files you did NOT edit, leave those alone).
- `cargo clippy -p shamir-db -p shamir-server --all-targets -- -D warnings`
  clean except the two already-tracked pre-existing issues:
  `crates/shamir-engine/src/table/read_planner.rs:466`
  (`clippy::type_complexity`, task #562) and
  `crates/shamir-query-types/src/tests/hmac_tests.rs` octal_escapes
  lines — confirm via `git diff --stat` you haven't touched either
  file, then re-run clippy WITHOUT `-D warnings` to prove zero new
  warnings.
- `./scripts/test.sh -p shamir-db -p shamir-server --full` green.

## Report

When done, produce a final summary (not a bare tool call): what changed
grouped by §1-§5, the full text of every new/rewritten test, the gate
command outputs, and every discrepancy between this brief's assumptions
and the actual code encountered — call each one out explicitly. In
particular confirm: the exact mock-resolver pattern you reused (name +
file), whether the §4 test list was complete or you found additional
breaking call sites, and the exact wording of `ERR_INVALID_OWNER`
messages you used for each of the three checks.
