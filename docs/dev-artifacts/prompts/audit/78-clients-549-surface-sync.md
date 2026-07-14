בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Brief: client-side surface sync — remove role-object ops, add setSuperuser (task #560)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Context

`docs/dev-artifacts/design/identity-privilege-unification-548-549-decision.md` §7
is the source of truth for the phased plan this task is step #7-ish of
(renumbered to 78 in this campaign's own brief sequence). Two backend
changes have already landed and are now the ONLY truth on the wire:

1. **Task #557** added a new top-level wire op:
   `DbRequest::SetSuperuser { user: String, on: bool, hmac: Option<String> }`
   / `DbResponse::SuperuserSet { user: String, on: bool }`
   (`crates/shamir-query-types/src/wire/db_message.rs:173-178, 206`).
   HMAC canonical form: `canonical_set_superuser(user, on)` in
   `crates/shamir-query-types/src/hmac.rs` —
   `b"set_superuser\0<user>\0<on>"` with `<on>` as the literal string
   `"true"`/`"false"`.

2. **Task #559** deleted `CreateRole`/`DropRole`/`RenameRole` from the
   `BatchOp` enum entirely (`crates/shamir-query-types/src/batch/batch_op.rs`),
   deleted `ListOp::Roles`
   (`crates/shamir-query-types/src/admin/types/list_ops.rs`), and deleted
   the Rust query-builder's `create_role`/`drop_role`/`rename_role`/
   `list_roles` constructors (`crates/shamir-query-builder/src/ddl/auth.rs`,
   `crates/shamir-query-builder/src/ddl/list.rs`). "Role" is now a plain
   string label attached to a directory user — there is no "role object"
   to create/drop/rename/list anymore. `grant_role`/`revoke_role` (which
   attach/detach that string) are UNCHANGED and remain the only
   role-related ops.

Confirmed by investigation (read-only, before this brief was written):
the Rust query-builder side is ALREADY clean (#559 deleted its role-op
constructors as part of that task, since the `BatchOp` deletion was a
hard compile dependency). **`crates/shamir-client` (the separate Rust
client crate) has no independent copies of these builders** — it is a
thin re-export wrapper around `shamir-connect` + `shamir-query-builder`
(`crates/shamir-client/src/lib.rs:59` re-exports
`shamir_query_builder as builder`). So the Rust side of this task is
**already fully done** — nothing to change there. This task's actual
work is **entirely in `crates/shamir-client-ts`** (the TypeScript
client), which still has the pre-#559 role-object surface intact, plus
the `docs/guide-docs/client-server-protocol-spec/` documentation.

## Scope — exact locations (confirmed via investigation)

### §1. Delete TS role-object builders and their wire types

- `crates/shamir-client-ts/src/core/builders/admin.ts:283-329` —
  `createRole()` (283), `dropRole()` (293), `renameRole()` (327).
  Delete all three builder functions.
- `crates/shamir-client-ts/src/core/builders/ddl.ts:512-514` —
  `listRoles()`. Delete.
- `crates/shamir-client-ts/src/core/builders/index.ts:31` — remove the
  `listRoles` re-export from the flat barrel.
- `crates/shamir-client-ts/src/core/types/admin.ts:228-232, 238-242,
  261-264` — `CreateRoleOp`, `DropRoleOp`, `RenameRoleOp` interfaces.
  Delete all three.
- `crates/shamir-client-ts/src/core/types/ddl.ts:446-455` — the
  `ListOp` union currently includes `{ list: 'roles' }` around line
  452. Remove that variant from the union.
- `crates/shamir-client-ts/src/core/hmac.ts:134-136, 183-187` —
  `canonicalDropRole()` and `canonicalCreateRole()`. Delete both (no
  `canonicalRenameRole` exists to delete — confirmed `renameRole` was
  never HMAC-gated on the TS side either, mirroring the Rust side).
  Grep the whole `crates/shamir-client-ts/src/` tree afterward for any
  remaining reference to `createRole`/`dropRole`/`renameRole`/
  `listRoles`/`CreateRoleOp`/`DropRoleOp`/`RenameRoleOp`/
  `canonicalCreateRole`/`canonicalDropRole` — there must be zero left
  (including in `.d.ts` output if the build generates one, and in any
  barrel/index re-export you haven't been told about explicitly above —
  the investigation above is thorough but grep to be sure).

### §2. Add a `setSuperuser` builder

Add a new builder mirroring the shape of the other single-purpose admin
ops already in `crates/shamir-client-ts/src/core/builders/admin.ts`
(look at an existing simple op like `dropUser`/`grantRole` for the
established pattern: builder function signature, HMAC wiring via
`.hmac(tag)` chaining if that's the pattern used, or however the
existing ops attach their HMAC — follow the SAME pattern, don't invent
a new one). It must:
- Take `user: string, on: boolean` and produce the wire shape matching
  `DbRequest::SetSuperuser { user, on, hmac }` (top-level request, NOT
  a `BatchOp` — confirm how the TS client already models the
  distinction between `BatchOp`-shaped ops and standalone top-level
  `DbRequest` variants, if any other precedent exists; if `SetSuperuser`
  is the FIRST standalone non-batch admin op the TS client models,
  say so explicitly in your final report and describe the approach you
  took).
- Add a `canonicalSetSuperuser(user, on)` HMAC helper in
  `crates/shamir-client-ts/src/core/hmac.ts` producing EXACTLY
  `` `set_superuser\0${user}\0${on ? 'true' : 'false'}` `` (as bytes,
  matching the Rust canonical form byte-for-byte — check how existing
  canonical-form helpers in that file encode strings to bytes, e.g.
  UTF-8 + null-byte separators, and follow the same encoding).
- Add the corresponding TypeScript request/response type(s) (mirroring
  `DbRequest::SetSuperuser`/`DbResponse::SuperuserSet`) in
  `crates/shamir-client-ts/src/core/types/` wherever the existing
  non-batch admin request/response types live (or wherever the
  established pattern for top-level `DbRequest` variants is, if that's
  a different file than the `BatchOp`-shaped types in `admin.ts`/`ddl.ts`).

### §3. Update the TS test suite

- `crates/shamir-client-ts/src/core/builders/__tests__/admin.test.ts:447-540`
  — remove the `createRole`/`dropRole`/`renameRole` test blocks.
- `crates/shamir-client-ts/src/core/builders/__tests__/ddl.test.ts:505-506`
  — remove the `listRoles` test.
- `crates/shamir-client-ts/src/__tests__/e2e-permissions.test.ts:255-322`
  (an A8 test doing `createRole` → `grantRole` → `revokeRole`) and
  `:669-711` (a G3 test doing `createRole` → `dropRole`) — these two
  e2e tests exercise a REAL role-object lifecycle that no longer
  exists. Rewrite them (don't just delete) to prove the SAME underlying
  concern — that granting/revoking a role string actually gates access
  — using ONLY `grantRole`/`revokeRole` against a user created via the
  existing `createUser` builder (no role-object step). If the test's
  original intent genuinely can't be preserved without a role object
  (e.g. it was specifically testing role-object metadata, not the
  grant/revoke access-gating effect), say so explicitly in your report
  rather than silently dropping coverage.
- Add new tests for `setSuperuser`: at minimum, a builder-level test
  (wire shape + HMAC canonical form matches the Rust side byte-for-byte
  — you can cross-check against `crates/shamir-query-types/src/hmac.rs`'s
  `canonical_set_superuser` and/or the wire test fixtures in
  `crates/shamir-server/tests/set_superuser_wire.rs` from task #557 for
  the exact expected byte sequence).
- Run the TS suite via `npm test` (defined in
  `crates/shamir-client-ts/package.json:25`, runs `vitest run`) from
  inside `crates/shamir-client-ts/`. Confirm green before finishing.

### §4. Update `docs/guide-docs/client-server-protocol-spec/`

Grep `docs/guide-docs/client-server-protocol-spec/` (including `diagrams/`) for
`CreateRole`/`DropRole`/`RenameRole`/`list_roles`/`listRoles`/any
mention of the ticket's `roles` field or ticket wire version `1`
(now `2` per task #558) — files confirmed to contain at least one such
reference during investigation: `AUTH_PROTOCOL.md`,
`IMPLEMENTATION_GUIDE.md`, `CLIENT_BROWSER.md`, `SECURITY_MODEL.md`,
`SESSION_RESUMPTION.md`, `diagrams/01-initial-auth.md`,
`diagrams/02-resumption.md`, `diagrams/03-bootstrap.md`,
`diagrams/06-update-user.md`, `diagrams/12-anti-downgrade-matrix.md`.
For each: update stale role-object language to the current "role is a
string label" model, update ticket version references to v2, and add
brief documentation of the new `SetSuperuser` op wherever the doc
already enumerates admin/auth wire ops. Keep edits surgical — don't
rewrite whole documents, just correct the specific stale claims.
`docs/dev-artifacts/research/coverage-ts-query-builder.md` also has stale coverage
entries (line ~376, ~422-425) — update those too since they're
directly about the TS client this task changes.

## Out of scope

- Do NOT touch anything in `crates/shamir-query-builder`,
  `crates/shamir-query-types`, `crates/shamir-db`, `crates/shamir-server`,
  or `crates/shamir-client` (the Rust client) — investigation confirmed
  all of these are already correct/unaffected. If you find something
  there that looks wrong, STOP and report it in your final summary
  rather than fixing it — it's either a misunderstanding on your part
  or a genuine discovery that needs its own task.
- Do NOT touch task #561 (chown/chgrp/addGroupMember target validation
  via `PrincipalResolver`) — unrelated task, different files.

## Red tests required first

Per this repo's TDD discipline: for the `setSuperuser` addition, write
the failing builder/HMAC test FIRST (it will fail to compile/import
since the builder doesn't exist yet), then implement the builder to
make it pass. For the role-op deletions, the existing tests at the
locations in §3 are already "red" in the sense that they test
functionality this task removes — convert them per §3's instructions
(delete the pure role-object ones, rewrite the two e2e ones) rather
than leaving them as dead references to deleted builders.

## Definition of done

- Zero remaining references anywhere in `crates/shamir-client-ts/src/`
  to `createRole`/`dropRole`/`renameRole`/`listRoles`/`CreateRoleOp`/
  `DropRoleOp`/`RenameRoleOp`/`canonicalCreateRole`/`canonicalDropRole`
  (verify via grep).
- `setSuperuser` builder + HMAC canonical form + types exist and are
  tested, matching the Rust wire shape and canonical-form bytes
  exactly.
- `npm test` (vitest) green in `crates/shamir-client-ts/`.
- `docs/guide-docs/client-server-protocol-spec/` and
  `docs/dev-artifacts/research/coverage-ts-query-builder.md` no longer reference the
  deleted role-object ops or the stale ticket-v1 shape, and document
  `SetSuperuser`.
- No changes outside `crates/shamir-client-ts/` and `docs/`.

## Verification (orchestrator will also run these independently — do
not skip them yourself before reporting done)

```
cd crates/shamir-client-ts
npm test
```

If there's a TypeScript typecheck script (`npm run typecheck` or
similar in `package.json`), run that too and confirm clean.

## Report

When done, produce a final summary (not a bare tool call) listing:
exactly what you changed grouped by §1-§4, the full text of the new
`setSuperuser` test(s), the two rewritten e2e tests' new bodies (or an
explicit note if coverage couldn't be preserved), the `npm test` output,
and any discrepancy between this brief's assumptions and what you
actually found in the code (call each one out explicitly rather than
silently resolving it).
