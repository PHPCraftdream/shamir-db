Task #551 — extend the destructive-op HMAC "did-you-mean-it" confirmation
(task #542's mechanism) to the 5 group-mutating ops that #542 deliberately
left fully UNWIRED: `CreateGroup`, `DropGroup`, `RenameGroup`,
`AddGroupMember`, `RemoveGroupMember`. Lowest-severity tier of the
destructive-op HMAC cluster per the original audit's ranking — no
version bump, same precedent as #542/#537.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Read first — the exact pattern to mirror

`docs/dev-artifacts/prompts/audit/63-security-542-hmac-coverage-extension.md` (the
brief for #542) and its landed implementation are the template for
EVERYTHING in this task — read the actual current code at each of these
locations before writing anything, since #542/#546/#547 all touched
nearby files this session:

- `crates/shamir-query-types/src/hmac.rs` — the canonical-input module.
  `canonical_chgrp`'s `Option<u64>` → `"null"` sentinel pattern is your
  template for any optional-field encoding here (none of these 5 ops has
  an optional field, but `GroupRef`'s two variants need the SAME
  multi-variant-canonicalization treatment `canonical_resource_ref` got
  for `ResourceRef`'s 6 variants — exhaustive match, no wildcard).
- `crates/shamir-client-ts/src/core/hmac.ts` — the byte-for-byte TS
  mirror, including `canonicalResourceRef`'s `never`-typed exhaustiveness
  guard (added after #542's adversarial review found the TS side could
  silently fall through on an unmatched variant) — apply the SAME
  exhaustiveness pattern to your new `canonicalGroupRef` helper.
- `crates/shamir-server/src/db_handler/admin.rs`'s `check_destructive_hmacs`
  match — the shape for each new arm.
- `crates/shamir-query-builder/src/ddl/access_control.rs` — the
  `Chmod`/`Chown`/`ChgrpBuilder` pattern (builder struct returned instead
  of `BatchOp` directly, `.hmac(...)` method, `IntoBatchOp`/`From<T> for
  BatchOp` impls) — apply the same shape to `create_group`/`drop_group`/
  `rename_group`/`add_group_member`/`remove_group_member`'s builders
  (check `crates/shamir-query-builder/src/ddl/auth.rs`'s `CreateGroup`-
  adjacent code if group builders already live in a different file than
  access_control.rs — investigate the actual location first).
- `crates/shamir-client-ts/src/core/builders/admin.ts` — the TS builder
  side, same pattern.

## The op structs and their fields (confirmed at the time of this brief)

`crates/shamir-query-types/src/admin/access.rs`:
- `GroupRef` — `Name { name: String }` | `Id { id: u64 }` (two variants,
  no wildcard when canonicalizing — same discipline as `ResourceRef`).
- `CreateGroupOp { create_group: String }`
- `DropGroupOp { drop_group: GroupRef, if_exists: bool }`
- `RenameGroupOp { rename_group: GroupRef, to: String }`
- `AddGroupMemberOp { add_group_member: GroupRef, user: u64 }`
- `RemoveGroupMemberOp { remove_group_member: GroupRef, user: u64 }`

None currently have an `hmac` field — add `hmac: Option<String>`
(`#[serde(default, skip_serializing_if = "Option::is_none")]`, doc
comment naming the canonical-input shape) to all 5, matching
`DropUserOp`'s exact shape (the established template referenced in
#542's own brief).

## Canonical input design

```
create_group      b"create_group\0<name>"
drop_group        b"drop_group\0<group_ref>"
rename_group      b"rename_group\0<group_ref>\0<to>"
add_group_member  b"add_group_member\0<group_ref>\0<user>"
remove_group_member b"remove_group_member\0<group_ref>\0<user>"
```

`<group_ref>` is produced by a new `canonical_group_ref(r: &GroupRef) ->
String` helper (Rust) / `canonicalGroupRef` (TS), exhaustively covering
both variants — pick a stable, unambiguous rendering (e.g.
`"name:<name>"` / `"id:<id>"`, mirroring `canonical_resource_ref`'s
`"scheme://path"` shape) that can never collide between the two variants
for any input (a group literally named `"id:3"` must not canonicalize to
the same string as `GroupRef::Id { id: 3 }` — pick a separator/prefix
that avoids this, and note your reasoning in the doc comment). Update
`hmac.rs`'s module-doc table (the "Per-op canonical input" table) to add
these 5 rows, matching the existing table's format — also update the
row that currently says "Group ops... NOT yet covered by the HMAC gate"
(added by #542) to remove that caveat once these land.

## The fix (mechanical, same shape as #542 — apply to all 5 ops)

1. Add `hmac` field to the 5 op structs (above).
2. Add `canonical_group_ref` + the 5 `canonical_*` functions to
   `hmac.rs`.
3. Mirror all 6 new Rust functions byte-for-byte in `hmac.ts`
   (`canonicalGroupRef` with the `never`-typed exhaustiveness guard, plus
   the 5 `canonical*` functions).
4. Extend `check_destructive_hmacs`'s match for `CreateGroup`/
   `DropGroup`/`RenameGroup`/`AddGroupMember`/`RemoveGroupMember`,
   pulling `op.hmac.as_ref()` and computing the canonical via the new
   helpers — same shared `Some(tag)`/`None` handling below the match, no
   duplication.
5. Wire the Rust query-builder's `create_group`/`drop_group`/
   `rename_group`/`add_group_member`/`remove_group_member` functions to
   return a builder (not `BatchOp` directly) with a `.hmac(...)` method,
   same shape as `Chmod`/`ChgrpBuilder`.
6. Wire the TS builders (`crates/shamir-client-ts/src/core/builders/
   admin.ts`) the same way.

## Test requirement (learn from #542's own gap)

#542's adversarial review found that 5 of its 9 new ops shipped with
ONLY `hmac_required`/`accepted` tests, missing `hmac_mismatch` —
discovered only after commit, requiring a follow-up fix. **Do not repeat
that gap here.** For EVERY ONE of these 5 ops, write all three states
from the start:
- `hmac_required` (no `hmac` field → rejected)
- `hmac_mismatch` (wrong tag → rejected)
- accepted (correct tag → succeeds)

in `crates/shamir-server/tests/hmac_gate.rs`, following the exact
existing pattern for `chmod`/`chown`/`chgrp` (three `#[tokio::test]`
functions per op, `session_key`/`canon::compute_tag_hex` for the
positive case, `"deadbeef".repeat(8)` for the mismatch case). Also add
canonical byte-layout tests for the 6 new Rust functions
(`canonical_group_ref` + the 5 ops) in
`crates/shamir-query-types/src/tests/hmac_tests.rs`, mirroring the
existing `canonical_drop_*`/`canonical_chmod` test shapes.

## Secondary consideration (informational, do if convenient)

#542's review also noted the napi-client e2e harness
(`tests/e2e/helpers/hmac.js`, `tests/e2e/tests/12-hmac-gate.test.js`) was
never extended for ANY of #542's 9 ops, let alone these 5. If you have
runway left after the mandatory scope above, extend that harness to
cover this task's 5 group ops (and #542's 9, if genuinely convenient in
the same pass — do not let this balloon the task). If you don't reach
it, leave an honest note in your report rather than silently dropping it
again — this specific gap has now been flagged twice.

## Test scope

```
./scripts/test.sh -p shamir-server -p shamir-query-types -p shamir-query-builder
```
Plus the shamir-client-ts test suite (`npm run typecheck` / `npm test`
inside `crates/shamir-client-ts`) for any `.ts` files touched.

## Verification (lighter per-task gate, agreed for this campaign)

```
cargo check --workspace --all-targets
./scripts/test.sh -p shamir-server -p shamir-query-types -p shamir-query-builder
```
Full fmt/clippy/test --full is FINAL-GATE's (#529's) job. This task does
NOT block FINAL-GATE — do not add it to #529's blockedBy.

## Report format

```
[Implementation] Status: fixed / scoped-down-with-followup
  > canonical_group_ref: exact shape, collision-avoidance reasoning
  > All 5 ops: hmac field + canonical fn (both languages) + server match
    + client builder (both languages)
  > hmac.rs module-doc table updated, "Group ops NOT yet covered" caveat
    removed
  > New tests: confirmed all THREE states (required/mismatch/accepted)
    for every one of the 5 ops, from the start
  > Secondary consideration (e2e harness): done / not reached + honest
    note
[Verification]
  cargo check --workspace --all-targets: pass/fail
  ./scripts/test.sh -p shamir-server -p shamir-query-types -p shamir-query-builder: pass/fail
  shamir-client-ts (typecheck + test): pass/fail
```

Given this is a wire-protocol change touching the destructive-op HMAC
gate on both Rust and TypeScript, this MUST go through an adversarial
review pass before committing — same discipline as #537/#540-#547 this
campaign. If that review finds a genuine bug, the orchestrator fixes it
directly (never re-delegates), re-verifies, and sends the fix through a
second review pass before committing.
