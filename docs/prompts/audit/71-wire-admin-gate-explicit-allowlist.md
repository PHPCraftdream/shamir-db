בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Brief: wire-admin coarse DAC gate — explicit per-op allowlist (task #553)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Context

`docs/design/root-user-group-dac-posture-550-decision.md` §2 (already
signed off by the project owner) documents this exact design in full —
read it first, it is the source of truth for every claim below. This
brief is the actionable slice of that decision.

Today, TWO identical, duplicated blocks (confirmed byte-for-byte
identical apart from context) gate every non-superuser batch request:

`crates/shamir-server/src/db_handler/handler.rs:396-406`:
```rust
// Admin / auth gate.
if !session.permissions.is_superuser {
    for (alias, entry) in &batch.queries {
        if entry.op.is_admin() {
            return DbResponse::Error {
                code: "permission_denied".into(),
                message: format!("query '{}' requires superuser (admin/auth op)", alias),
            };
        }
    }
}
```

`crates/shamir-server/src/db_handler/tx_handlers.rs:102-112` (identical
shape, same comment, same logic).

`BatchOp::is_admin()` (`crates/shamir-query-types/src/batch/batch_op.rs:582-654`)
is a broad `matches!` over ~50 op variants (every DDL/admin op). This
coarse gate blocks 4 harmless READ-ONLY introspection ops
(`List`, `AccessTree`, `DescribeTable`, `GetTableSchema`) for every
non-superuser, even when they'd pass their own real per-table/per-path
authorization checks further down the stack (each of these has its own
independent handler with its own `authorize_access` call —
`crates/shamir-db/src/shamir_db/execute/admin_list.rs`,
`admin_access.rs`'s `handle_access_tree`, `admin_describe.rs`,
`admin_schema.rs` — none of this brief's work touches those handlers,
they are already correct and unaffected).

## The rejected approach — DO NOT implement this

An earlier proposal was to relax the gate via the existing `is_write()`
classifier: exempt any `is_admin()` op where `is_write() == false`.
**This was found unsafe during design review and must NOT be used:**

1. `BatchOp::Batch` (nested sub-batch) also has `is_write() == false`
   for an EMPTY/read-only-looking wrapper (its own `is_write()` is
   defined recursively over its children,
   `crates/shamir-query-types/src/batch/batch_op.rs:775`), but
   `required_access(Batch)` returns `None`
   (`batch_op.rs:543`, part of the exhaustive "everything else" match
   arm at lines 485-555) — meaning the per-op authorization loop in
   `execute_as`/`tx_execute_as` NEVER inspects a sub-batch's nested
   queries at all. Today `is_admin(Batch) == true` closes this by
   blocking ALL non-superuser sub-batches outright. Under the rejected
   `is_write()`-based relaxation, `Batch{ r: Read(forbidden_table) }`
   would pass the coarse gate (Batch itself isn't "write") and its
   nested Read would execute with **zero per-table authorization** —
   reopening the exact bug class task #510 closed for `Subscribe`.
2. `is_write() == false` is ALSO true for 8 other ops never intended to
   be touched here: `GetBufferConfig`, `MigrationStatus`,
   `InternerDump`, `ChangesSince`, `ListValidators`,
   `ListPublications`, `ListSubscriptions`, `ReplicationStatus`
   (`batch_op.rs:668-686`). Two are concretely dangerous to open
   blindly: `ListPublications`/`ListSubscriptions`/`ReplicationStatus`
   would expose replication topology to any authenticated user;
   `InternerDump` would leak every interned field name in a repo,
   including field names from tables the caller has no rights to.

## The correct fix

Replace the coarse `is_admin()` check with an **explicit allowlist of
exactly 4 ops**, never derived from `is_write()` or any other
classifier:

```rust
if !session.permissions.is_superuser {
    for (alias, entry) in &batch.queries {
        let exempt = matches!(
            entry.op,
            BatchOp::List(_)
                | BatchOp::AccessTree(_)
                | BatchOp::DescribeTable(_)
                | BatchOp::GetTableSchema(_)
        );
        if entry.op.is_admin() && !exempt {
            return DbResponse::Error {
                code: "permission_denied".into(),
                message: format!("query '{}' requires superuser (admin/auth op)", alias),
            };
        }
    }
}
```

Apply this IDENTICALLY in both `handler.rs` and `tx_handlers.rs`.
Factor the predicate into ONE shared helper (e.g. a free function
`fn is_coarse_admin_gate_exempt(op: &BatchOp) -> bool` in a shared
module both files already import from, or a method on `BatchOp` itself
in `shamir-query-types` if that's a cleaner fit) so the two copies
cannot drift apart — this mirrors the "duplicated enforcement logic"
hazard task #546 already fixed for the DML per-op mapping.

**Do not extend the allowlist beyond these 4 ops.** In particular,
`BatchOp::Batch` must NEVER be added to it until `required_access`/the
per-op authorization loop is taught to recurse into nested
`SubBatchOp` queries — that is separate, not-yet-scoped work, out of
scope for this task. If you find yourself tempted to widen the
allowlist to "make a test pass," stop — the correct fix is a narrower
test, not a wider allowlist.

**`AccessTree` will still be effectively superuser-only in practice**
even after this change — its own handler
(`crates/shamir-db/src/shamir_db/execute/admin_access.rs`,
`handle_access_tree`) gates on `Manage(Root)`, which `permits()`
grants owner-or-System only (a non-superuser `Actor::User` is never
Root's owner by default). This is EXPECTED and correct — the coarse
gate's job is only to stop blocking the op outright; the op's own real
authorization (already correct, untouched by this task) still applies
underneath. Do not "fix" this by changing `handle_access_tree`'s gate —
that's a separate, unscoped design question.

## Test scope

The tests below all drive `BatchRequest`s through the REAL server
dispatch path (`ShamirDbHandler::handle`/`handle_tx`, whichever exists
in this crate's test harness — follow the existing pattern in
`crates/shamir-server/tests/` for constructing a session with a
non-superuser `SessionPermissions` and dispatching a batch), not the
internal `shamir-db` methods directly — the coarse gate lives in
`shamir-server`, and a test that bypasses the dispatcher wouldn't catch
a regression here.

Required test matrix (mirrors #546's coverage-matrix precedent):
- (a) A non-superuser CAN `DescribeTable`/`GetTableSchema` a table they
  own/have `Read` on (op passes the coarse gate AND its own real
  per-table check).
- (b) A non-superuser CANNOT `DescribeTable`/`GetTableSchema` a table
  they have no rights to (coarse gate passes it through, but the op's
  own real authorization still denies it — confirms the coarse gate
  relaxation doesn't accidentally grant a bypass).
- (c) `AccessTree` and `List` targeting `Users`/`Roles` stay DENIED for
  a non-superuser (their own handlers' `Manage(Root)` gate still
  applies underneath the now-passing coarse gate).
- (d) `List` targeting `Databases` is ALLOWED for a non-superuser (per
  the #552 Root posture — Root's default mode permits List/Read
  traversal).
- (e) **A nested `Batch` containing a forbidden-table `Read` is STILL
  DENIED** for a non-superuser — this is the direct regression test
  for the bypass the rejected `is_write()`-based approach would have
  reopened. Construct `Batch{ sub_alias: Read(table_the_actor_cannot_read) }`
  and confirm the whole request is rejected by the coarse gate (since
  `Batch` is NOT in the allowlist), exactly as it is today.
- (f) Every OTHER non-exempted `is_admin()` op (pick a representative
  sample: `CreateDb`, `Chmod`, `CreateUser`, plus explicitly the 8 ops
  the rejected approach would have silently exempted —
  `GetBufferConfig`, `MigrationStatus`, `InternerDump`, `ChangesSince`,
  `ListValidators`, `ListPublications`, `ListSubscriptions`,
  `ReplicationStatus`) remains superuser-only through the coarse gate.

Run:
```
./scripts/test.sh -p shamir-server --full
```

## Definition of done

- `cargo fmt -p shamir-server -- --check` clean.
- `cargo clippy -p shamir-server --all-targets -- -D warnings` clean.
- `./scripts/test.sh -p shamir-server --full` green, including the new
  tests above.
- `handler.rs` and `tx_handlers.rs` share ONE predicate implementation,
  not two independently-maintained copies.
- No change to `admin_list.rs`/`admin_access.rs`/`admin_describe.rs`/
  `admin_schema.rs` or any other per-op authorization logic — this task
  is scoped strictly to the coarse wire-level gate.
