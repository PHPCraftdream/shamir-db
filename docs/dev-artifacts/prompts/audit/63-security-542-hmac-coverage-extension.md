Task #542 — extend the destructive-op HMAC "did-you-mean-it" confirmation
to privilege-grant, create, chmod/chown/chgrp, and retention ops. This is
a wire-protocol change (adds new/optional fields to existing op structs
on both the Rust and TypeScript sides), same class as #537. No version
bump — follow that precedent (the campaign has already decided, for this
kind of additive-optional-field wire change, not to bump crate/package
versions unless explicitly asked).

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Why this matters (audit finding, admin-ddl #3 / gate-coverage rec.5)

`check_destructive_hmacs` in
`crates/shamir-server/src/db_handler/admin.rs` (currently ~lines 137-215,
called from `handler.rs` around line 378) is a "did you mean it" guard —
NOT an authentication gate (the module's own doc comment at the top of
that function explains why: TLS+SCRAM already authenticates the session;
anyone holding a valid `session_id` can act as that session by
construction). It currently covers ONLY: `DropDb`, `DropRepo`,
`DropTable`, `DropIndex`, `DropUser`, `DropRole`,
`Start/Commit/RollbackMigration` (the match arms at ~lines 157-195), with
`_ => continue` (line 196) letting every other op through unconfirmed.

**Impact**: a stolen/hijacked LIVE superuser ticket (leaked
`session_id`/resumption ticket, attacker has no password) cannot drop a
table without a valid HMAC tag — but CAN, with zero confirmation:
`GrantRole superuser <attacker>` (the single most dangerous op in the
whole system), `Chown` a victim's resource to themselves, `Chmod 0o777`
on a secret table, `PurgeHistory`/`SetRetention` (irreversible audit-trail
loss), `CreateUser`, and every group-mutating op. The asymmetry is
severe: the LEAST destructive op in the current list (`DropRole`) is
HMAC-gated while a full superuser grant is not.

## Read first

- `crates/shamir-query-types/src/hmac.rs` — the canonical-input module.
  Read its module doc (the "why HMAC at all" + "per-op canonical input"
  table) in full; your new ops extend that same table and that same
  design, not a different mechanism.
- `crates/shamir-server/src/db_handler/admin.rs`'s `check_destructive_hmacs`
  — the full match, and its doc comment.
- `crates/shamir-client-ts/src/core/hmac.ts` — the byte-for-byte TS mirror
  of `hmac.rs` (`canonicalDropTable`, etc.) that the client uses to
  compute tags before sending. Server and client MUST agree byte-for-byte
  on canonical input construction — this is the wire-format-stable part.
- `crates/shamir-client-ts/src/core/builders/ddl.ts` (grep
  `signer.hmacTagHex(canonical)` — the existing attachment pattern for
  `dropTable`/`dropIndex`/etc. builders) and
  `crates/shamir-client-ts/src/core/builders/admin.ts` — the builders for
  the ops you're extending (`chmod`, `chown`, `chgrp`, `createGroup`,
  `dropGroup`, `renameGroup`, `addGroupMember`, `removeGroupMember`,
  `grantRole`, `revokeRole`, `createUser`, `createRole`, `setRetention`,
  `purgeHistory` — confirm exact builder file locations, they may be
  split across `ddl.ts`/`admin.ts`/others; grep each op name to find its
  actual builder).
- The op struct definitions (confirmed locations at the time of this
  brief — re-verify, code may have shifted):
  - `crates/shamir-query-types/src/admin/access.rs`: `ChmodOp`, `ChownOp`,
    `ChgrpOp`, `CreateGroupOp`, `DropGroupOp`, `RenameGroupOp`,
    `AddGroupMemberOp`, `RemoveGroupMemberOp` — NONE currently have an
    `hmac` field.
  - `crates/shamir-query-types/src/admin/types/retention.rs`:
    `PurgeHistoryOp`, `SetRetentionOp` — neither has an `hmac` field.
  - `crates/shamir-query-types/src/auth/types.rs`: `CreateUserOp`,
    `CreateRoleOp`, `GrantRoleOp`, `RevokeRoleOp` — none have an `hmac`
    field. (`DropUserOp`/`DropRoleOp` in the same file already have one —
    use their exact shape as your template: `#[serde(default,
    skip_serializing_if = "Option::is_none")] pub hmac: Option<String>`,
    with a doc comment naming the canonical-input shape.)

## The fix

**1. Add `hmac: Option<String>` fields** (matching `DropUserOp`'s exact
`#[serde]` attributes and doc-comment style) to every op struct listed
above: `ChmodOp`, `ChownOp`, `ChgrpOp`, `CreateGroupOp`, `DropGroupOp`,
`RenameGroupOp`, `AddGroupMemberOp`, `RemoveGroupMemberOp`,
`PurgeHistoryOp`, `SetRetentionOp`, `CreateUserOp`, `CreateRoleOp`,
`GrantRoleOp`, `RevokeRoleOp`.

**2. Add a `canonical_*` Rust helper per op** in
`crates/shamir-query-types/src/hmac.rs`, following the existing
`join_null(&[b"<op_name>", ...fields...])` pattern. Design the canonical
input for each op from ALL fields that determine the op's effect (not
just an id) — get this right, since it's what "did you mean this
EXACT action" is protecting:
- `chmod`: op name, resource path (canonicalize however `ResourceRef`
  already renders for other purposes — check if there's an existing
  `Display`/canonical-string method on `ResourceRef`/`ResourcePath`
  rather than inventing a new encoding), mode (decimal or octal string,
  pick one and be consistent with how `drop_index`'s `unique` bool was
  encoded as `"0"`/`"1"`).
- `chown`: resource path, owner id.
- `chgrp`: resource path, group id (or a sentinel for `None`/clear —
  mirror how `unique: bool` was encoded, pick a clear textual sentinel
  like `"null"` for the clear-group case and document it).
- `create_group`: group name.
- `drop_group`: group ref (name or id — canonicalize consistently for
  both `GroupRef` variants).
- `rename_group`: group ref, new name.
- `add_group_member`/`remove_group_member`: group ref, user id.
- `create_user`: username ONLY (never the password — it must never enter
  an HMAC canonical input or be logged; the tag confirms "you meant to
  create this account", not the credential).
- `create_role`: role name (permissions list is more debatable — simplest
  correct option: just the role name, matching `drop_role`'s precedent of
  identifying by name only; note this in your report if you choose a
  richer canonical input instead, with reasoning).
- `grant_role`/`revoke_role`: role name, target username — this is the
  audit's #1 priority (the single most dangerous op class).
- `set_retention`: table/repo, retention value's canonical string form
  (check `Retention`'s existing serialization/Display for a stable
  representation — don't invent a second one).
- `purge_history`: table/repo, scope's canonical string form (same
  caution — check `PurgeScope`'s existing rendering).

Update `hmac.rs`'s module-doc table (the "Per-op canonical input" table
at the top) to list every new op, matching the existing table's format.

**3. Mirror EVERY new `canonical_*` function in
`crates/shamir-client-ts/src/core/hmac.ts`** — byte-for-byte identical
construction (same field order, same separators, same encodings) to the
Rust side. This is the wire-format-stable half; a mismatch here means the
client computes a tag the server will always reject.

**4. Extend `check_destructive_hmacs`'s match** in
`crates/shamir-server/src/db_handler/admin.rs` to cover `GrantRole`,
`RevokeRole`, `CreateUser`, `Chmod`, `Chown`, `Chgrp`, `SetRetention`,
`PurgeHistory`, `CreateGroup`, `DropGroup`, `AddGroupMember`,
`RemoveGroupMember`, `RenameGroup`, `CreateRole` — following the exact
match-arm shape already used for `DropUser`/`DropRole`/etc. (compute
canonical via the new helper, pull `op.hmac.as_ref()`, fall through to
the same `Some(tag)`/`None` handling below the match — do not duplicate
that handling, it's shared after the match).

**5. Wire the client-side builders** (`ddl.ts`/`admin.ts` or wherever
each op's builder actually lives) to compute and attach `hmac` the same
way `dropTable`/`dropIndex`/etc. already do (`signer.hmacTagHex(canonical)`
via the new `canonicalX` TS function) for every op you extended.

## Explicit permission to scope down (given the breadth)

This touches ~14 op structs across 2 languages (Rust op structs + hmac.rs
+ TS hmac.ts + TS builders + server match extension + tests for all of
it) — a large surface. If, after starting, the full set proves too much
for one safe pass:

- **Do not leave the fix half-wired** (e.g. Rust-side HMAC required but
  the TS client never sends one — that would just turn every affected op
  into a permanent hard failure for TS callers, a worse regression than
  the current gap).
- If you must scope down, prioritize by the audit's own severity
  ordering: `GrantRole`/`RevokeRole` first (named the single most
  dangerous op), then `Chown`/`Chmod`/`Chgrp`, then `CreateUser`/
  `CreateRole`, then `SetRetention`/`PurgeHistory`, then the group ops
  (`CreateGroup`/`DropGroup`/`RenameGroup`/`Add|RemoveGroupMember`) last
  (audit calls these lower-severity than privilege/ownership ops).
- For whatever you DON'T cover in this pass, fully wire it (struct field
  + canonical fn + both languages + server match) for the ops you commit
  to, and leave a properly scoped follow-up task (with the same rigor
  this brief was written with) for the remainder — do not half-wire any
  single op end-to-end.

## Test requirement

For every op you extend (fully wired): a `shamir-server` test proving the
op is REJECTED (`hmac_required`) with no `hmac` field, REJECTED
(`hmac_mismatch`) with a wrong tag, and ACCEPTED with the correctly
computed tag (mirror the existing `DropUser`/`DropRole` HMAC tests —
find them via grep in `shamir-server`'s test suite and use the same
shape). Add/extend a `shamir-query-types` test proving each new
`canonical_*` Rust function produces the documented byte layout (mirror
existing `canonical_drop_*` tests). If TS builder wiring is completed for
an op, add/extend a `shamir-client-ts` test proving the builder attaches
a tag that the Rust `canonical_*` + `verify_tag_hex` accepts (check
whether an existing cross-language fixture/golden-vector test already
does this for `drop_table` etc. — reuse that pattern, don't invent a new
cross-language test mechanism).

## Test scope

```
./scripts/test.sh -p shamir-server -p shamir-query-types
```
Also run the shamir-client-ts test suite for any TS files you touch —
check `crates/shamir-client-ts/package.json` for its test script (likely
`npm test` or `pnpm test` inside that directory) since it's outside the
Rust `./scripts/test.sh` wrapper's scope.

## Verification (lighter per-task gate, agreed for this campaign)

```
cargo check --workspace --all-targets
./scripts/test.sh -p shamir-server -p shamir-query-types
```
Plus the TS test run described above for any `.ts` files touched.
Full fmt/clippy/test --full is FINAL-GATE's (#529's) job.

## Report format

```
[Implementation] Status: fixed / scoped-down-with-followup
  > Ops fully wired this pass (struct field + canonical fn both languages
    + server match + client builder + tests): list each one
  > Ops NOT covered this pass (if scoped down): list each one + why,
    and confirm a follow-up task was filed with the same rigor
  > hmac.rs module-doc table updated to match
  > New tests: confirmed each rejects-without-hmac /
    rejects-wrong-hmac / accepts-correct-hmac for every op fully wired
[Verification]
  cargo check --workspace --all-targets: pass/fail
  ./scripts/test.sh -p shamir-server -p shamir-query-types: pass/fail
  shamir-client-ts test run (for any .ts files touched): pass/fail
```

Given this is a wire-protocol change spanning both the Rust and
TypeScript client, touching the destructive-op confirmation gate
directly, this MUST go through an adversarial review pass before
committing — same discipline as #537/#540/#541 this campaign. If that
review finds a genuine bug, the orchestrator fixes it directly (never
re-delegates), re-verifies, and sends the fix through a second review
pass before committing.
