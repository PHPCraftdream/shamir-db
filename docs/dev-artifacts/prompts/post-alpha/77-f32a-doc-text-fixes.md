# Brief for F-32a (#841, P2) — README role wording + AGENTS.md crate count

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

A third-wave review (R11/C12) flagged two stale doc claims in the public
repo root, part of a larger doc-hygiene task (F-32, #825) being closed in
several independently-committed pieces. This piece is the two purely
mechanical text fixes — no judgment calls, no repo-artifact decisions
(those are handled separately). **Do not touch anything under
`docs/dev-artifacts/` or `docs/checkpoints/`** — this project's standing
rule (confirmed by the user this session) is that development-history
artifacts are never deleted or reorganized away, regardless of any
generic "doc hygiene" framing.

## Fix 1 — README's stale "Create/Drop User / Role" wording

`README.md:58` reads:
```
- ✅ Auth ops (Create/Drop User / Role, Grant / Revoke)
```

This is stale. Role OBJECTS (a `CreateRole`/`DropRole` DDL lifecycle) do
not exist anywhere in the wire protocol — verify this yourself before
editing (grep `crates/shamir-query-types/src` for `CreateRoleOp`/
`DropRoleOp`; you will find neither). What actually exists:
- `User.roles: Vec<String>` (`crates/shamir-query-types/src/auth/types.rs:146,151`)
  — roles are plain string labels attached to a user, not standalone
  objects with their own lifecycle.
- `GrantRoleOp`/`RevokeRoleOp` (`auth/types.rs:225-242`) — attach/detach a
  role label to/from a user.
- `CreateUserOp`/`DropUserOp` (`auth/types.rs:178-224`) — users ARE
  full DDL-creatable/droppable objects (this part of the old wording is
  still accurate).
- A `Role` struct (`auth/types.rs:139-142`, "named set of permissions")
  still exists as a reflection/permission-model type, but is not exposed
  as a `CreateRole`/`DropRole` wire operation — don't conflate this with
  "role objects" in the DDL-lifecycle sense the old README line implies.

Rewrite the line to reflect this accurately — Users are created/dropped;
role labels are granted/revoked, not created/dropped as objects. Keep the
surrounding bullet-list style and checkmark format exactly as-is; this is
a wording fix, not a restructure.

## Fix 2 — AGENTS.md's stale crate count

`AGENTS.md:28-38` ("📦 Workspace" section) reads:
```
`Cargo.toml` declares `members = ["crates/*"]` and excludes `shamir-client-node`
(napi-rs binding, MSVC-only on Windows — built separately). The default
workspace ships **10 crates**:

`shamir-types`, `shamir-storage`, `shamir-query-types`, `shamir-engine`,
`shamir-db`, `shamir-connect`, `shamir-server`, `shamir-transport-tcp`,
`shamir-transport-ws`, `shamir-client`.
```

This is stale — the workspace actually has 23 crates today. `CLAUDE.md`'s
own "📦 Workspace" section (top of the file) already has the CORRECT,
current list — use it verbatim as the source of truth (do not
independently re-derive the list from `Cargo.toml`, and do not just bump
the number without listing all 23 names):

```
shamir-collections, shamir-types, shamir-storage, shamir-query-types,
shamir-query-builder, shamir-query-builder-macros, shamir-engine,
shamir-funclib, shamir-wal, shamir-tx, shamir-db, shamir-connect,
shamir-server, shamir-transport-tcp, shamir-transport-ws,
shamir-client, shamir-sdk, shamir-sdk-macros, shamir-tunables,
shamir-wasm-host, shamir-index, shamir-numa, shamir-bench-utils.
```

Replace AGENTS.md's stale 10-crate list with this real 23-crate list (same
"ships **23 crates**" framing style CLAUDE.md and README.md already use —
`README.md:75` already says 23 correctly, so AGENTS.md is the only one
out of sync). Verify against `Cargo.toml` yourself (`members =
["crates/*"]`, and check the exclude list) that this count/list is still
accurate at the moment you make the edit — don't just copy CLAUDE.md
blindly if you find a discrepancy (state clearly in your summary if you
find one).

## Constraints

- Docs-only (README.md, AGENTS.md). No `.rs` files, no
  `docs/dev-artifacts/`, no `docs/checkpoints/`.
- Do NOT touch README's "published binaries are not available yet" line
  — that's explicitly deferred to actual tag/release time, not this task.
- Do NOT invent a "role objects" migration note or changelog entry — this
  is a doc wording fix, not a feature announcement.

## Verification the orchestrator will run

Docs-only — no build/test gate. The orchestrator will re-verify both
claims (wire-protocol grep for role ops, `Cargo.toml`'s actual crate list)
against the diff before committing.
