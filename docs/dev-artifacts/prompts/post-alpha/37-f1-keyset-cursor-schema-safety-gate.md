# Brief for #792 (F-1) — keyset cursor: gate on schema-typed homogeneous scalar

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context — what's already closed, what's still open

`crates/shamir-server/src/db_handler/cursor_handlers.rs` runs a keyset
cursor (inclusive `field >= seek_key` boundary filter) whenever
`pagination_mode_for_query` sees a single-field `ORDER BY` shape. Three
prior tasks (CR-D2 #783, W-2/W-3 #789) closed SPECIFIC data-shape gaps in
this scheme by probing the pinned snapshot once at `CreateCursor` time and
falling back the WHOLE cursor to `PaginationMode::Offset` when the probe
finds something unsafe:

- **`Null`/missing value** — closed via `order_by_column_contains_null`
  (a cheap `WHERE <field> IS NULL LIMIT 1` probe).
- **`Dec`/`Big`/`Bin`/`List` value** — closed via `safe_seek_key` (detects
  an unconvertible/uncomparable bookmark candidate, degrades that call to
  offset).

Read `docs/guide-docs/KNOWN_LIMITATIONS.md`'s "Keyset-mode cursors" section
(search for "CR-D2 #783, W-2/W-3 #789") in full before starting — it has
the complete, precise history of what's closed and why, including the
exact mechanism (`compare_values`'s `(Null, _) => None` fallthrough,
`QvSortKey`'s Null-sorts-last-under-ASC placement) that makes an unsafe
column silently drop rows past page 1 instead of erroring.

**Two gaps remain STILL OPEN** (also documented in that same section):

1. **Mixed `QueryValue` type in one `ORDER BY` column** (e.g. some rows
   store `Int`, others `Str`, in the same field path) — not detected.
2. **`NaN` in an `F64` `ORDER BY` column** — not detected. `NaN`'s
   `partial_cmp` always returns `None`, so a NaN-valued row is silently
   dropped past page 1 the same way. `NaN` also breaks the tie-run counter
   (`f64`'s `PartialEq` is always `false` for `NaN`).

The doc explicitly notes there is NO existing cheap runtime probe for
either case (unlike `Null`, which `Filter::IsNull` already covers) — a
"does this field ever hold a second type" or "is this field ever NaN"
probe would need either a new filter primitive or a full scan, and a full
fix (two-phase scan: keyset over comparable rows + an offset-bookmarked
tail phase for the rest) is out of scope here.

An independent release-readiness review (2026-07-24, Wave F planning)
flagged this as a release blocker: **"документированная бесшумная потеря
строк — слишком сильный дефект даже для alpha"** (a documented silent
row-loss defect is too severe even for an alpha release), and proposed the
fix path this task implements: **stop relying on a full-scan-only runtime
probe for these two cases, and instead gate keyset eligibility on the
table's SCHEMA when one is bound** — if the ORDER BY column has a
schema-enforced, fixed, non-container scalar type, every row satisfying
that schema is provably homogeneous by construction (mixed-type is
impossible), which closes gap 1 for schema-typed tables. Columns without a
schema-enforced scalar type (schemaless documents, `Any`-typed fields, or
no schema validator bound to the table at all) keep using
`PaginationMode::Offset` unconditionally — no probe needed, no silent
loss possible, and this is the conservative/safe default for the common
schemaless-document case.

## Design (already investigated — this is the concrete plan)

### Why `F64`/`Dec`/`Big`-typed columns stay excluded even under schema

Checked `crates/shamir-engine/src/validator/schema/field_rule.rs`'s
`check_f64` (around line 138): the schema validator's `F64` type check does
**NOT** reject `NaN` today (no `is_nan()` check anywhere in that function).
So even a schema-declared `F64` field can still hold `NaN` — schema
enforcement does NOT close gap 2 for float columns. Therefore the schema
gate this task adds must accept ONLY `TypeTag` values that can never be
`NaN` or a container: `Int`, `Bool`, `String`, `Bin`. Reject `F64`, `Dec`,
`Big`, `List`, `Map`, `Set`, `Any`, `Null` — those keep using
`PaginationMode::Offset` regardless of schema, until a follow-up task
closes the NaN case specifically (e.g. by adding a NaN-detection probe or
constraint — explicitly out of scope here).

### How to reach the schema's `FieldRule` for the ORDER BY column path

Investigated the call chain available on `table: &Table`
(`crates/shamir-engine/src/table/table.rs`) inside `create_cursor`:

- `table.validator_bindings() -> Arc<Vec<ValidatorBinding>>`
  (`crates/shamir-engine/src/table/table_manager_validators.rs`) — each
  binding has a `validator_id: RecordId`.
- `table.validator_registry_ref() -> Option<&Arc<ValidatorRegistry>>`
  (same file) — `None` for tables with no validators bound at all (the
  schemaless case — treat this as "no schema gate available", fall back to
  offset for anything but already-safe query shapes... actually simpler:
  treat `None` registry the same as "no matching schema rule found" below).
- `ValidatorRegistry::get_by_id(&RecordId) -> Option<Arc<dyn RecordValidator>>`
  (`crates/shamir-engine/src/validator/registry.rs`).
- The concrete schema validator type is
  `crates/shamir-engine/src/validator/schema/schema_validator.rs`'s
  `SchemaValidator { pub rules: Vec<FieldRule>, ... }`, which `impl
  RecordValidator for SchemaValidator` (around line 112 of that file).
- **Missing piece**: `RecordValidator` (`record_validator.rs`, around line
  145) currently has NO downcast support — no `Any` bound, no
  `as_schema_validator()` method. You need to add ONE narrow, minimal
  downcast hook so cursor code can ask "is this bound validator a
  `SchemaValidator`, and if so what are its rules?" without the validator
  trait becoming a general `dyn Any` grab-bag. Suggested shape (adjust to
  fit the trait's existing conventions — check how other `RecordValidator`
  impls are structured first, e.g. `grep -rn "impl RecordValidator for"`):
  ```rust
  trait RecordValidator: Send + Sync {
      // ...existing methods...
      /// Downcast hook for callers that need schema-rule introspection
      /// (e.g. the keyset-cursor schema-safety gate). Default `None`;
      /// only `SchemaValidator` overrides it.
      fn as_schema_rules(&self) -> Option<&[FieldRule]> { None }
  }
  ```
  and in `SchemaValidator`'s impl: `fn as_schema_rules(&self) -> Option<&[FieldRule]> { Some(&self.rules) }`.
- `FieldRule { path: Vec<String>, ty: TypeTag, constraints }`
  (`crates/shamir-engine/src/validator/schema/field_rule.rs`) — match
  `path` against the `ReadQuery`'s single `order_by.items[0].field` (also
  a `Vec<String>`) for an exact match.
- **Field must also be `required` (or otherwise provably always-present
  under the schema)** — check `FieldRule`/the schema rule set for however
  "required" is represented today (grep for `required` in
  `crates/shamir-engine/src/validator/schema/`). If the ORDER BY field is
  merely `nullable`/optional even under an eligible `TypeTag`, keep relying
  on the EXISTING `order_by_column_contains_null` probe for that
  null/missing case (it already runs and already closes that specific
  subgap) — don't skip it, the schema gate only needs to prove "no SECOND
  non-null type is possible", not "never null" (that's the Null probe's
  job, already solved).
- Multiple bindings can exist (`Vec<ValidatorBinding>`, ordered by
  `priority`) — if MULTIPLE schema validators are bound and they disagree
  on the type for the same path (unusual but not impossible), or if no
  binding yields a `FieldRule` for that exact path at all, treat the
  column as NOT schema-typed (fall back to offset) — conservative default,
  do not try to merge/reconcile conflicting schemas.

## What "done" looks like

1. New function (naming to match the file's existing style, e.g.
   `order_by_column_is_schema_typed_scalar` next to
   `order_by_column_contains_null` in `cursor_handlers.rs`) that:
   - Takes `&Table` and the `ReadQuery`'s order-by field path.
   - Returns `true` only if a bound schema validator has an exact
     `FieldRule` match for that path whose `ty` is one of `Int`, `Bool`,
     `String`, `Bin` (see the exclusion list above for why `F64`/`Dec`/
     `Big`/containers/`Any` are excluded).
   - Returns `false` for: no validator bound, validator bound but not a
     `SchemaValidator` (via the new downcast hook), no `FieldRule` for
     that path, ambiguous/conflicting bindings, or an excluded `TypeTag`.
   - This is a **pure metadata check — no read/scan of the data**, so it
     should run BEFORE `order_by_column_contains_null`'s snapshot probe
     (cheaper, and short-circuits the whole keyset attempt for schemaless
     tables without paying for a probe read at all).
2. Wire it into `create_cursor`'s existing mode-selection block (around
   line 1116, `let mut mode = pagination_mode_for_query(&query);`):
   `PaginationMode::Keyset` is only kept if BOTH the existing shape check
   AND the new schema-type gate pass; otherwise fall back to `Offset`
   immediately (before the null probe — no need to probe data for a
   column that's already ruled ineligible). If the schema gate passes,
   still run the existing null probe afterward exactly as today (it
   handles nullable-but-typed columns).
3. Update `docs/guide-docs/KNOWN_LIMITATIONS.md`'s "Keyset-mode cursors"
   section: change "Mixed `QueryValue` type... STILL OPEN" to reflect the
   new state — **closed for schema-typed `Int`/`Bool`/`String`/`Bin`
   columns via the new schema gate; still open (by design, until a
   follow-up) for schemaless columns of those same value-shapes, and for
   ANY `F64`/`Dec`/`Big`/container-typed column regardless of schema** —
   those simply never use keyset mode now, so the "silent loss" framing no
   longer applies to them (they always paginate via offset, which has its
   own separate, already-documented and accepted duplicate-row tradeoff
   under concurrent writes — do not conflate the two). Also update the
   `NaN` bullet similarly: still open for `F64` columns (schema or not),
   but those columns no longer attempt keyset at all, so — same as above —
   reframe from "silent loss risk" to "always offset-paginated, no keyset
   attempted".
4. Update `docs/guide-docs/client-server-protocol-spec/CURSORS.md` if it
   describes keyset eligibility criteria (check first; only touch if it
   actually documents this).
5. **Tests** (in
   `crates/shamir-server/src/db_handler/tests/cursor_handler_tests.rs`,
   following this file's existing patterns for schema-binding setup — grep
   it for any existing schema-validator test fixture helpers first, reuse
   rather than reinvent):
   - A table with a schema declaring the ORDER BY field as `Int` (or
     `String`) and `required: true` → cursor uses `PaginationMode::Keyset`
     (assert via whatever the test file's existing convention is for
     observing pagination mode — check how W-2/W-3's tests asserted this).
   - A table with a schema declaring the ORDER BY field as `F64` →
     cursor falls back to `Offset` even though the shape check alone would
     have picked `Keyset` (regression test proving the F64 exclusion).
   - A table with NO schema bound at all (schemaless) → cursor falls back
     to `Offset` (regression test for the conservative default).
   - A table with a schema declaring the field as `Int` but the field is
     `nullable`/optional (not `required`) → schema gate still passes
     (keyset attempted), but the EXISTING null probe still catches an
     actual null row and falls back correctly (prove the two mechanisms
     compose, don't regress the null probe's existing coverage).
   - A mixed-type regression case: if you can construct a table+schema
     where the OLD code (shape-only gate) would have picked keyset and
     silently dropped a differently-typed row, and the NEW code correctly
     refuses keyset for that same setup because there's no schema (or the
     schema doesn't cover it) — this is the core regression this task
     fixes, make sure at least one test exercises it end-to-end (create
     cursor, scroll past page 1, assert no silent row loss — or assert the
     cursor is in Offset mode from creation, whichever is easier to assert
     against this file's existing helpers).

## Constraints

- Do NOT attempt to close the NaN-in-F64 gap itself (no new NaN-detection
  primitive, no new filter op) — this task's F64 exclusion is the accepted
  mitigation for release purposes; a real NaN-safe design is explicitly
  future work.
- Do NOT change `order_by_column_contains_null`, `safe_seek_key`, or any
  other already-closed CR-D2/W-2/W-3 mechanism's behavior — this task adds
  an ADDITIONAL gate that runs before them, it does not replace them.
- Do NOT touch the two-phase-scan / engine-owned-cursor ideas mentioned in
  `KNOWN_LIMITATIONS.md` as future work — out of scope.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine -p shamir-server` and
  `cargo clippy -p shamir-engine -p shamir-server --all-targets -- -D
  warnings` must be clean for crates you touch.
- Follow workspace conventions: `use` at file top, `mod.rs` re-exports
  only, one primary export per file, surgical diff.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -p shamir-server -- --check
cargo clippy -p shamir-engine -p shamir-server --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -p shamir-server
```
