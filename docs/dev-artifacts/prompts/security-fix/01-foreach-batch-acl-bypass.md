# CRITICAL — `ForEach`/`Batch` bypass per-table ACL enforcement

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## The bug (confirmed independently three times: direct code reading by
## the orchestrator, plus two separate read-only research agents)

`BatchOp::required_access` (`crates/shamir-query-types/src/batch/batch_op.rs:468-558`)
returns `None` for `BatchOp::Batch` and `BatchOp::ForEach` (they have no
`table_ref()`, so they fall into the `return None` arm alongside genuine
admin/DDL ops). The per-op authorization pre-check loops in
`crates/shamir-db/src/shamir_db/execute/db_execute.rs:57-63`
(`execute_as`) and `crates/shamir-db/src/shamir_db/execute/db_tx.rs:144-162`
(`tx_execute_as`) are BOTH a single, non-recursive walk:

```rust
for entry in request.queries.values() {
    if let Some((action, path)) = entry.op.required_access(db_name) {
        self.authorize_access(&actor, &path, action).await...?;
    }
}
```

Since `Batch`/`ForEach` contribute `None`, NEITHER of these loops ever
checks what tables a `Batch`/`ForEach`'s NESTED body actually touches.
Consequence: an authenticated non-superuser actor with permission on
SOME tables but NOT on a specific forbidden table can read/insert/
update/delete that forbidden table's data by simply wrapping the op in
a top-level `ForEach` (even with a trivial single-iteration `over`) or a
plain `Batch` sub-batch — the per-table check for the INNER op never
runs. Nesting `Batch` inside `ForEach` (or vice versa, to arbitrary
depth) escapes it identically at every level, since the loop is
one-level-deep regardless.

This is EXACTLY the same shape of gap `#660` already fixed for
`distinct_repos` (which also needed to recurse into `Batch`/`ForEach`
bodies to see the whole tree) — this bug is `#660`'s twin for
AUTHORIZATION instead of repo/dependency detection, and was apparently
never given the same recursive treatment.

A doc comment in `crates/shamir-server/src/db_handler/admin.rs:587-597`
(`is_coarse_admin_gate_exempt`) shows the `Batch` half of this gap was
ALREADY KNOWN and explicitly reasoned about ("`BatchOp::Batch` MUST
NEVER be added here: its `required_access` is `None`... Exempting
`Batch` would let `Batch{ Read(forbidden_table) }` execute with zero
per-table authorization — reopening the bug class task #510 closed for
`Subscribe`") — but the mitigation applied there was narrower than the
actual bug: NOT exempting `Batch` from the coarse wire-level gate does
NOT close the underlying per-table authorization hole this brief
targets; it only avoids making one specific symptom (the coarse gate
itself) worse. `ForEach` (added later, in Epic04) was never given even
that partial mitigation.

**Reachability**: confirmed reachable via the plain `ShamirDb::execute_as`/
`tx_execute_as` facade methods (used by the live server) AND via the WASM
function host's DB gateway — `crates/shamir-db/src/shamir_db/shamir_db/db_gateway.rs:285-294`'s
`FacadeDbGateway::execute` decodes the wire request and calls
`self.shamir.execute_as(...)` directly, the SAME vulnerable path. Fixing
`execute_as`/`tx_execute_as` closes both.

## The fix — mirror `#660`'s `distinct_repos`/`collect_repos` pattern exactly

`crates/shamir-query-types/src/batch/query_entry.rs:91-111` already has
the template to copy:

```rust
pub fn distinct_repos(queries: &TMap<String, QueryEntry>) -> TFxSet<String> {
    let mut repos = TFxSet::default();
    collect_repos(queries, &mut repos);
    repos
}

fn collect_repos(queries: &TMap<String, QueryEntry>, repos: &mut TFxSet<String>) {
    for qe in queries.values() {
        if let Some(tr) = qe.op.table_ref() {
            repos.insert(tr.repo.clone());
        }
        match &qe.op {
            BatchOp::Batch(sub) => collect_repos(&sub.batch.queries, repos),
            BatchOp::ForEach(fe) => collect_repos(&fe.batch.queries, repos),
            _ => {}
        }
    }
}
```

1. In the SAME file (`crates/shamir-query-types/src/batch/query_entry.rs`),
   add a new public function, e.g.:

   ```rust
   /// Recursively collect every `(Action, ResourcePath)` authorization
   /// requirement across the WHOLE query tree — including inside nested
   /// `Batch`/`ForEach` bodies, at any depth. This is the single source of
   /// truth the per-op authorization pre-check loops
   /// (`ShamirDb::execute_as` / `tx_execute_as`) must use instead of a
   /// flat, one-level walk — a flat walk sees `None` for `Batch`/`ForEach`
   /// (they have no `table_ref()`) and silently skips whatever tables
   /// their nested body actually touches (the #660-class bug, but for
   /// authorization instead of repo detection).
   pub fn collect_required_access(
       queries: &TMap<String, QueryEntry>,
       db: &str,
   ) -> Vec<(Action, ResourcePath)> {
       let mut out = Vec::new();
       collect_required_access_into(queries, db, &mut out);
       out
   }

   fn collect_required_access_into(
       queries: &TMap<String, QueryEntry>,
       db: &str,
       out: &mut Vec<(Action, ResourcePath)>,
   ) {
       for qe in queries.values() {
           if let Some(req) = qe.op.required_access(db) {
               out.push(req);
           }
           match &qe.op {
               BatchOp::Batch(sub) => collect_required_access_into(&sub.batch.queries, db, out),
               BatchOp::ForEach(fe) => collect_required_access_into(&fe.batch.queries, db, out),
               _ => {}
           }
       }
   }
   ```

   Check the exact imports needed (`Action`/`ResourcePath` are already
   used by `required_access`'s signature in this same crate — reuse
   those, don't add a new dependency). Re-export `collect_required_access`
   the same way `distinct_repos` is already re-exported through
   `shamir_query_types::batch::*` / `shamir-engine`'s re-export / however
   `crates/shamir-db` currently imports `distinct_repos` (check its
   existing `use` statements for the exact path and mirror it).

2. In `crates/shamir-db/src/shamir_db/execute/db_execute.rs`'s
   `execute_as` (~line 57-63), replace the flat loop with one iterating
   `collect_required_access(&request.queries, db_name)` instead of
   `request.queries.values()` + `entry.op.required_access(db_name)`
   inline. Preserve the exact same per-entry `authorize_access` call and
   error mapping.

3. In `crates/shamir-db/src/shamir_db/execute/db_tx.rs`'s `tx_execute_as`
   (~line 144-162), same change — replace the flat loop with
   `collect_required_access(&request.queries, db_name)`, preserving the
   existing `acl_cache` de-duplication logic (the cache still works
   correctly over the recursively-collected list — it's keyed on
   `(ResourcePath, Action)`, unrelated to how the list was gathered).

4. Do NOT change `required_access`'s own per-op mapping, `table_ref()`,
   `is_admin()`, or the coarse wire-admin gate
   (`is_coarse_admin_gate_exempt` in `crates/shamir-server/src/db_handler/admin.rs`)
   — none of those need to change; the fix is entirely in HOW the
   authorization pre-check loop gathers its list of things to check.

## Tests (the critical part — this is a security bug, prove it's closed)

Add tests (check `crates/shamir-db/src/shamir_db/tests/enforcement_tests.rs`
and `coverage_matrix_tests.rs` for the existing style/harness conventions
— a non-`System`/non-superuser `Actor` with SOME but not all table
permissions, calling `execute_as`/`tx_execute_as` directly):

1. **The exact bypass, closed**: an actor with permission on table `A`
   but explicitly NO permission on table `B` — a batch containing ONLY a
   top-level `ForEach` (trivial single-element `over`) whose body reads
   (or inserts/updates/deletes) table `B` — must now be REJECTED with
   `access_denied` (previously: silently succeeded).
2. **Same for plain `Batch`** (sub-batch, non-transactional body) — same
   shape, same assertion.
3. **Nested arbitrarily**: a `Batch` containing a `ForEach` containing
   another `Batch` whose innermost body touches the forbidden table `B`
   — must still be rejected (proves the recursion isn't just one level
   deep).
4. **Positive/regression**: the SAME nested shapes (`ForEach`, `Batch`,
   nested combination) where the inner body touches an ALLOWED table —
   must still succeed normally. This is essential: don't just prove
   "now everything is denied", prove legitimate nested usage still works
   (this session's #661 fix depends on nested Batch/ForEach being usable
   inside transactions — don't regress that).
5. **Both entry points**: mirror tests 1-4 for BOTH `execute_as`
   (non-transactional) AND `tx_execute_as` (transactional/interactive) —
   the bug and the fix are symmetric across both call sites; test both,
   don't assume one implies the other.
6. **`Actor::System` unaffected**: a quick sanity check that `Actor::System`
   (the admin bypass) still works through nested Batch/ForEach exactly as
   before (System should never hit a denial) — this is almost certainly
   already implied by existing tests but worth a one-line confirmation
   here for completeness.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-query-types -p shamir-db --full` green,
  including all new tests.
- `./scripts/test.sh -p shamir-engine --full` green (no regression in
  the engine's own batch/ForEach tests — this fix touches only the
  authorization pre-check, not engine execution, so this should be
  unaffected, but confirm).
- `cargo fmt --all -- --check` clean (or scoped to touched crates,
  report which).
- `cargo clippy --workspace --all-targets -- -D warnings` clean (full
  workspace).
- Report literal command output for all of the above.
- Explicitly confirm, with reasoning: (a) the fix closes the bypass for
  BOTH `execute_as` and `tx_execute_as`, (b) the WASM host gateway
  (`FacadeDbGateway::execute` → `execute_as`) is fixed as a consequence
  without needing its own separate change (trace the call chain
  yourself to confirm, don't just take this brief's word for it), and
  (c) legitimate nested Batch/ForEach usage (this session's #661 fix)
  is not regressed.

## Out of scope

- Do NOT change `is_coarse_admin_gate_exempt` or the coarse wire-admin
  gate's own classification logic in `crates/shamir-server/src/db_handler/admin.rs`
  — this fix operates entirely at the per-table `authorize_access`
  layer, upstream of that gate.
- Do NOT touch anything from this session's #661-671 wave or the CI
  fixes already landed today (sq8 tolerance, `$fn`+`$ref` write-value
  fix, MvccStore deadlock fix).
- Do NOT attempt to fix any OTHER finding from today's read-only
  research batch (docs/dev-artifacts/research/2026-07-17-release-audit/)
  — this brief is scoped to this ONE critical ACL bypass only.
