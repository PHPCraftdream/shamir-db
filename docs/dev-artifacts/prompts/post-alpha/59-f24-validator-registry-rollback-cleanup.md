# Brief for F-24 (#817, P3) — undo in-memory validator registration on schema-activation failure

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

F-4 (#794, already landed) made schema DDL follow validate → precompile →
persist → activate, with a catalogue rollback (`save_table_meta(&rec_prev)`)
if the final "activate" step (`ShamirDb::compile_table_schema`,
`crates/shamir-db/src/shamir_db/shamir_db/schema_management.rs` ~line
486-529) fails — see the three call sites in
`crates/shamir-db/src/shamir_db/execute/admin_schema.rs`
(`handle_set_table_schema` ~line 505-519, `handle_add_schema_rule` ~line
663, `handle_remove_schema_rule` ~line 786). The catalogue rollback is
correct and already tested.

The `/crush` post-wave review (NF-3) found that `compile_table_schema`'s
**in-memory** side effects are NOT symmetrically undone when it fails
partway through:

```rust
pub(crate) async fn compile_table_schema(...) -> DbResult<()> {
    let name = schema_validator_name(db_name, repo_name, table_name);
    let validator: Arc<dyn RecordValidator> = Arc::new(SchemaValidator::new(rules));

    if self.validators.id_for_name(&name).is_some() {
        self.validators.replace_artifact(&schema_validator_id, validator);
    } else {
        self.validators
            .register(schema_validator_id, &name, validator)   // <- step A
            .map_err(|e| DbError::Validation(e.to_string()))?;
    }

    let table = self.get_table(db_name, repo_name, table_name).await?;   // <- can fail (step B)
    let binding = ValidatorBinding { .. };
    table.add_validator_binding(binding).await?;                          // <- can fail (step C)

    let table_ref = Self::table_ref_str(db_name, repo_name, table_name);
    self.validators.add_binding(&schema_validator_id, &table_ref);         // <- step D (infallible)

    Ok(())
}
```

If step A (a FRESH `register`, i.e. this is the table's first-ever schema,
not an ALTER) succeeds but step B or C fails, the function returns `Err`
— but the validator registered in step A is now permanently orphaned in
`ValidatorRegistry::by_id`/`name_to_id` (`crates/shamir-engine/src/
validator/registry.rs`), under the name `schema_validator_name(db, repo,
table)`. Nothing ever calls `ValidatorRegistry::remove` for it. The caller
(`admin_schema.rs`) then rolls the CATALOGUE back to `rec_prev` (correct),
but has no way to know an in-memory registration needs undoing too — and
`compile_table_schema` itself doesn't attempt it.

## Investigated escalation — this is not purely cosmetic; a retry can hit a real bug

Re-reading `schema_validator_id`'s selection at each of the 3 call sites
(`admin_schema.rs`, e.g. ~line 444-451): it is **reused** if the catalogue
record already has `SCHEMA_VALIDATOR_ID_FIELD` (an ALTER), else **freshly
minted** via `RecordId::new()`. So for a table's FIRST schema declaration
that fails at step B/C above (leaving an orphaned registration under
`name`), a **retry of the exact same DDL call** (same table, no
`SCHEMA_VALIDATOR_ID_FIELD` in the rolled-back catalogue) mints a **NEW**
`RecordId` — call it `id2` — different from the orphan's id (`id1`).
`compile_table_schema`'s branch then sees `id_for_name(&name).is_some()`
(true, `id1` is still registered under this name) and takes the
`replace_artifact(&id2, validator)` branch — but `replace_artifact`
(`registry.rs` ~line 93-97) only mutates `by_id` for the id it's given
(`id2`), which does not exist yet, so it silently returns `false` (a
no-op) — **and this return value is never checked**. The retry then
proceeds through `get_table`/`add_validator_binding`/`add_binding` using
`id2`, all of which succeed, and the catalogue ends up recording
`schema_validator_id = id2` as the active schema validator for the table —
**but `ValidatorRegistry::by_id` has NO entry for `id2` at all** (only the
stale `id1` orphan under the same `name`). Any later write-path validator
dispatch that resolves the bound validator via `get_by_id(&id2)`
(check `crates/shamir-engine`'s write path for how bound validators are
looked up — e.g. wherever `ValidatorBinding::validator_id` is resolved
during an insert/update) would get `None` and presumably skip/no-op the
schema check entirely — **a silent schema-validation bypass**, not merely
a leaked map entry.

**Confirm this chain by reading the actual write-path validator-dispatch
code** (find where `ValidatorBinding.validator_id` is resolved against
`ValidatorRegistry::get_by_id` before deciding whether to run schema
enforcement) before treating this as certain — the analysis above is
based on reading `registry.rs` and `schema_management.rs` directly, but
verify the write-path's actual behavior on a `get_by_id` miss (silently
skips vs. some other fallback) to state the true severity precisely in
your summary and in the doc update below.

## The fix

Confine the whole fix to `compile_table_schema` itself — no changes needed
at the 3 `admin_schema.rs` call sites. Track whether THIS call performed a
**fresh** registration (the `else` branch, step A) as opposed to an
ALTER's `replace_artifact` (which must NEVER be undone on failure — the
validator already existed before this call and belonged to a previously-
active, still-valid schema version). If a fresh registration happened and
any LATER step in the same call fails, undo exactly that registration
(`ValidatorRegistry::remove`) before returning the error — restoring the
registry to the state it was in before this call started, symmetric with
how the catalogue rollback restores `rec_prev`. This also directly
eliminates the escalation above: no orphan survives past the end of the
failed call, so a retry never encounters a stale name collision under a
mismatched id.

Sketch (adapt to whatever reads cleanest, e.g. wrapping the
post-registration steps in an inner `async` block or just tracking a
`bool` and checking it in each new early-return path — this function is
short, either approach is fine):

```rust
let freshly_registered = self.validators.id_for_name(&name).is_none();
if freshly_registered {
    self.validators
        .register(schema_validator_id, &name, validator)
        .map_err(|e| DbError::Validation(e.to_string()))?;
} else {
    self.validators.replace_artifact(&schema_validator_id, validator);
}

let activation_result: DbResult<()> = async {
    let table = self.get_table(db_name, repo_name, table_name).await?;
    let binding = ValidatorBinding { .. };
    table.add_validator_binding(binding).await?;
    let table_ref = Self::table_ref_str(db_name, repo_name, table_name);
    self.validators.add_binding(&schema_validator_id, &table_ref);
    Ok(())
}
.await;

if activation_result.is_err() && freshly_registered {
    self.validators.remove(&schema_validator_id);
}
activation_result
```

Document the fix with a comment citing F-24/#817 and explaining WHY the
`replace_artifact` (ALTER) branch must never be undone (it's swapping an
existing, previously-working validator — rolling it back would need the
OLD artifact restored, not a bare removal, and that's already handled by
the catalogue-level `rec_prev` rollback pointing back at the unchanged
`schema_validator_id`/artifact).

## Tests

Add test(s) to whatever test file already covers `compile_table_schema`
(check `crates/shamir-db/src/shamir_db/shamir_db/` and
`crates/shamir-db/src/shamir_db/execute/tests/` — search for existing
schema-activation-rollback tests, e.g. near F-4's own tests, for the
established fixture/mocking pattern to force a failure at step B or C):

1. **Core regression**: force `compile_table_schema` to fail AFTER a fresh
   `validators.register` succeeds (e.g. inject a failure in
   `add_validator_binding` — check whether this is feasible via an
   existing test double/error-injection seam, or whether forcing
   `get_table` to fail for a table that doesn't exist is a simpler,
   already-available way to reach the same code path) — assert that after
   the call returns `Err`, `self.validators.id_for_name(&name)` (or
   equivalent) is `None` again — the orphan is gone.
2. **ALTER case unaffected**: an existing validator (already registered,
   reached via the `replace_artifact` branch) that fails at step B/C must
   NOT be removed from the registry — the pre-existing validator must
   still resolve normally afterward (regression guard against the fix
   over-reaching into the ALTER path).
3. If the escalation chain above is confirmed real, consider adding (or
   at least documenting as a follow-up if it's too large for this task)
   a test proving the RETRY-after-failure scenario now succeeds cleanly
   (second attempt, same table, actually activates and is resolvable via
   `get_by_id`) — this is the practical proof the silent-bypass scenario
   can no longer occur.

## Docs

Update `docs/guide-docs/KNOWN_LIMITATIONS.md` if F-4's existing entry
mentions this residual (search for "compile_table_schema" or the F-4/#794
bullet) — mark it closed, and if your investigation confirms the
escalation chain (silent schema-bypass-after-retry), state that precisely
rather than only describing the cosmetic leak.

## Constraints

- Do NOT touch the 3 `admin_schema.rs` call sites' catalogue-rollback
  logic — already correct, out of scope.
- Do NOT change `ValidatorRegistry::remove`'s existing contract (it does
  NOT enforce `is_bound` refusal — the facade does; this call site is
  safe to use it directly since a freshly-orphaned validator was never
  bound to anything in the first place — verify `add_binding` (step D)
  truly never ran before the failure you're undoing, since `remove`
  doesn't itself check binding state).
- Do NOT modify `replace_artifact`'s signature/behavior — if you choose to
  add a defensive check on its return value as a hardening measure, keep
  it minimal and clearly comment why (secondary hardening, not the
  primary fix, since the primary fix already eliminates the known path
  to a stale-name-collision).
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-db -- --check` and
  `cargo clippy -p shamir-db --all-targets -- -D warnings` must be clean.

## Verification the orchestrator will run

```
cargo fmt -p shamir-db -- --check
cargo clippy -p shamir-db --all-targets -- -D warnings
./scripts/test.sh -p shamir-db -- schema
./scripts/test.sh -p shamir-db --full
```
