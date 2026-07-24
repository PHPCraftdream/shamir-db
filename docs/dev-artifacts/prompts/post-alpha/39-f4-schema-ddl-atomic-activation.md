# Brief for #794 (F-4) — schema DDL: validate before persist, rollback on activation failure

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Bug 1 — catalogue write happens before the schema is proven compilable

`crates/shamir-db/src/shamir_db/execute/admin_schema.rs` has THREE DDL
handlers that all share the identical buggy step order:

- `handle_set_table_schema` (~line 298-425)
- `handle_add_schema_rule` (~line 427-551)
- `handle_remove_schema_rule` (~line 553-657)

Each one, in order:

1. Reads the current catalogue record (`rec`) via `load_table_record`.
2. Runs SOME validation (`validate_fk_indexes`, `validate_unique_indexes`,
   `validate_nested_path_transforms`, `validate_no_self_referential_cascade`
   — `handle_set_table_schema` only; check whether the add/remove-rule
   handlers run an equivalent subset, they may not run all four).
3. `serialise_rules(...)` → `schema_qv` (the catalogue-form
   `QueryValue::List(Map)` representation).
4. Mutates `rec` in place (`map_insert` for `SCHEMA_FIELD`,
   `SCHEMA_VALIDATOR_ID_FIELD`, `SCHEMA_VERSION_FIELD`).
5. **`self.shamir.system_store().save_table_meta(&rec)` — WRITES the
   catalogue.**
6. `interner_mgr.persist()`.
7. **`parse_schema(&schema_qv, interner)` — only NOW does the code check
   the schema round-trips into `Vec<FieldRule>` without error** (unknown
   type tag, malformed rule shape, etc. — see `parse_schema` in
   `crates/shamir-db/src/shamir_db/shamir_db/schema_management.rs`, around
   line 58).
8. `self.shamir.compile_table_schema(...)` — constructs the
   `SchemaValidator`, registers/replaces it in the global validator
   registry, and persists a `ValidatorBinding` to the table's info-twin
   (`table.add_validator_binding(binding).await` — a real I/O write that
   can fail).

If step 7 or step 8 fails, the function returns an error to the caller —
but the catalogue record written in step 5 is ALREADY PERSISTED with the
new (invalid, or never-activated) schema. A subsequent read of the table's
schema (e.g. `DESCRIBE`, or the boot-pass `boot_compile_schemas` on the
next restart) sees a `schema` field that was never actually validated to
compile, or that compiled but whose validator was never successfully
bound live.

### The fix

Reorder to **validate → parse (precompile) → atomic catalogue write →
activate (compile_table_schema)**, with a **rollback** of the catalogue
write if activation fails:

1. Keep steps 1-4 as-is (read, validate, serialise, mutate `rec`) — but
   **clone the catalogue record's PRE-mutation state** (or simply re-read
   it, whichever is cheaper/clearer given the existing code shape) before
   mutating, so there's something to roll back to.
2. **Move `parse_schema(&schema_qv, interner)` to run BEFORE
   `save_table_meta`** — if it errors, return the error immediately;
   nothing has been persisted yet, so there's nothing to roll back.
3. Persist the catalogue (`save_table_meta` + `interner_mgr.persist()`)
   exactly as today, now AFTER the parse/precompile gate has already
   proven the schema is well-formed.
4. Call `compile_table_schema(...)` (activation) exactly as today. **If it
   fails**, attempt a best-effort rollback: `save_table_meta` the
   PRE-mutation record captured in step 1, so the catalogue reflects the
   old (still-active, still-working) schema rather than a new one that was
   never actually activated. Log (via `tracing::warn!` or this file's
   existing logging convention — check what it uses) if the rollback
   write ITSELF fails (rare, but must not panic or silently swallow) —
   then still return the ORIGINAL activation error to the caller (the
   rollback attempt's own failure is secondary information, not the
   primary error).
5. Apply this identical reorder to all THREE handlers
   (`handle_set_table_schema`, `handle_add_schema_rule`,
   `handle_remove_schema_rule`) — check each one's exact current step
   order first (they may not be byte-identical to each other), but the
   target end state (validate → precompile → persist → activate, with
   rollback-on-activation-failure) is the same for all three.

Do NOT attempt a general transactional/WAL-backed atomicity mechanism —
this is a best-effort compensating rollback (persist the old record back),
matching the review's ask ("atomic catalogue write → RCU activation, with
rollback at error activation") without inventing new infrastructure.

## Bug 2 — unknown schema constraint values silently become `None` instead of erroring

`crates/shamir-db/src/shamir_db/shamir_db/schema_management.rs`'s
`parse_one_rule` (~line 108-238) parses several OPTIONAL constraint
sub-fields from the catalogue `Map` via a pattern that CANNOT distinguish
"field absent" from "field present but its string/shape doesn't match any
known variant":

```rust
let array_of = item
    .get("array_of")
    .and_then(|v| v.as_str())
    .and_then(|s| parse_type_tag(s).ok());   // line ~185: unknown string -> None, same as absent
let format = item
    .get("format")
    .and_then(|v| v.as_str())
    .and_then(shamir_engine::validator::schema::FormatKind::parse); // same pattern
let compare = item.get("compare").and_then(parse_cross_field_compare); // `parse_cross_field_compare` returns `Option`, swallows a malformed `op` string via `?`-chained `.as_str()?` collapsing to `None`
let foreign_key = item.get("foreign_key").and_then(parse_foreign_key_ref); // `parse_foreign_key_ref` similarly returns `Option`
```

Inside `parse_foreign_key_ref` (~line 282-310) and
`parse_cross_field_compare` (~line 320+), the `on_delete`/`on_update`
action strings ALSO silently default to `FkAction::NoAction` for any
unrecognized string (not just an absent field):

```rust
let on_delete = match map.get("on_delete").and_then(|v| v.as_str()) {
    Some("restrict") => FkAction::Restrict,
    Some("cascade") => FkAction::Cascade,
    Some("set_null") => FkAction::SetNull,
    _ => FkAction::NoAction,   // catches BOTH "absent" and "present but garbled"
};
```

**The consequence**: a schema DDL with a typo'd or otherwise-invalid
`array_of`/`format`/`compare`/`foreign_key`/`on_delete`/`on_update` value
does not error — the constraint is silently dropped (or, for
`on_delete`/`on_update`, silently downgraded to `NoAction`), and the DDL
call reports success. The caller has no way to know their constraint
wasn't applied.

### The fix

For each of these, distinguish "field absent" (legitimate `None` /
default, no error) from "field present but does not parse" (hard error,
`DbError::Validation`):

- `array_of`: if `item.get("array_of")` is `Some(v)`, `v.as_str()` must
  succeed AND `parse_type_tag` on it must succeed, or return
  `DbError::Validation` naming the field and the bad value. Only a
  genuinely-absent `"array_of"` key stays `None`.
- `format`: same pattern — `FormatKind::parse` failing on a PRESENT value
  is an error, not a silent `None`.
- `compare`/`foreign_key`: these currently return `Option<T>` from
  standalone functions using `?`-chaining, which conflates "the whole
  `compare`/`foreign_key` Map key is absent" with "the Map key is present
  but malformed inside" (e.g. `other` present but not a List, `op`
  present but not a recognized operator string). Change
  `parse_cross_field_compare`/`parse_foreign_key_ref` to return
  `DbResult<Option<T>>` instead of `Option<T>` — `Ok(None)` for "the outer
  key is absent", `Err(...)` for "the outer key is present but its
  contents are invalid", `Ok(Some(...))` for success. Update
  `parse_one_rule`'s call sites to propagate the error via `?` instead of
  the current `.and_then(...)`.
- `on_delete`/`on_update` (inside `parse_foreign_key_ref`, once it's
  `DbResult`-returning): same split — a PRESENT-but-unrecognized string
  errors; an ABSENT field defaults to `NoAction` exactly as today
  (documented backward-compat default for legacy rows, per the existing
  comment — do not change that default, only tighten what counts as
  "absent").

Do NOT change `parse_one_rule_default`'s existing tier-3 "log + drop"
behavior for the `default` field specifically — that one is explicitly,
deliberately boot-resilient (its own doc comment explains why: boot must
not fail outright on a stale/corrupt persisted default). This task's fix
is scoped to `array_of`/`format`/`compare`/`foreign_key`/`on_delete`/
`on_update` only, which are DDL-time-facing (the caller gets to see and
fix the error immediately), unlike `default` which can also be hit by the
boot-pass re-parsing an already-persisted (and previously-accepted at DDL
time) record.

## Tests

Find or create the existing test file(s) covering
`crates/shamir-db/src/shamir_db/execute/admin_schema.rs` and
`crates/shamir-db/src/shamir_db/shamir_db/schema_management.rs` (check
this repo's test-organization convention — one `tests/` dir per module,
`tests/mod.rs` manifest-only, no inline `#[cfg(test)] mod tests`) and add:

1. **Bug 1 regression**: a `SetTableSchema`/`AddSchemaRule`/
   `RemoveSchemaRule` DDL call whose schema is well-formed enough to pass
   the earlier index/cascade validations but fails `parse_schema` (e.g. an
   unknown type tag smuggled in some way the earlier validations don't
   catch — check what's actually reachable; if nothing in the DTO layer
   can produce an unparseable `schema_qv` today, construct the test at the
   `schema_management.rs` unit level instead, calling the reordered
   handler logic directly, or note in the test why an end-to-end DTO-level
   repro isn't reachable and test at the lower level that IS reachable).
   Assert the catalogue record is UNCHANGED (still the pre-DDL schema/
   version) after the failed call — this is the core regression (bad
   schema no longer gets persisted).
2. **Activation-failure rollback**: simulate `compile_table_schema` (or
   its `table.add_validator_binding` I/O) failing after a successful
   catalogue write, and assert the catalogue is rolled back to its
   pre-DDL state (may need a failure-injection mechanism — check existing
   test fixtures/doubles for `system_store`/`add_validator_binding` before
   inventing a new one).
3. **Bug 2 regression**: one test per fixed field
   (`array_of`/`format`/`compare`/`foreign_key`/`on_delete`/`on_update`)
   with a garbled-but-present value → DDL call returns an error (not
   silent success with the constraint dropped). One test per field
   confirming a genuinely-ABSENT field still defaults correctly (no
   regression to the legitimate default path).

## Constraints

- Do NOT touch `validate_fk_indexes`/`validate_unique_indexes`/
  `validate_nested_path_transforms`/`validate_no_self_referential_cascade`
  themselves — this task reorders what runs relative to the catalogue
  write, it doesn't change those validators' own logic.
- Do NOT change `parse_one_rule_default`'s tier-3 boot-resilient behavior.
- Do NOT add a general transaction/WAL mechanism for the DDL catalogue
  write — the rollback is a best-effort compensating write, matching
  what's already the pattern elsewhere in this codebase for similar
  "persist then activate" sequences (e.g. `restore.rs`'s
  `cleanup_staged_temp_dir` pattern from an earlier task in this same
  campaign — best-effort cleanup/rollback, logged on its own failure, not
  propagated over the primary error).
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-db -p shamir-engine` and
  `cargo clippy -p shamir-db -p shamir-engine --all-targets -- -D
  warnings` must be clean for crates you touch.
- Follow workspace conventions: `use` at file top, `mod.rs` re-exports
  only, one primary export per file, surgical diff.

## Verification the orchestrator will run

```
cargo fmt -p shamir-db -p shamir-engine -- --check
cargo clippy -p shamir-db -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-db -p shamir-engine
```
