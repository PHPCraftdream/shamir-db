# Brief for F-42 (#850, P1) — index-create must not go live before its interner ids are durably persisted

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

A readonly review (`docs/dev-artifacts/research/2026-07-27-new-wave-readonly-review.md`,
finding P1-3) found that `create_index`/`create_unique_index_locked`
(`crates/shamir-engine/src/table/table_manager_index_mgmt.rs:437-514`,
fixed by an earlier task — F-33 Step 5/#839 — to actually CALL
`interner.persist()`) create and register the index FIRST, then persist
the interner:

```rust
pub async fn create_index(&self, name: &str, paths: &[&str]) -> DbResult<()> {
    let index_def = self.build_index_definition(name, paths).await?;
    let records = self.collect_all_current_records().await?;
    self.index_manager
        .create_index_from_records(index_def, records)
        .await?;                       // <-- index is now LIVE
    self.interner.persist().await      // <-- if THIS fails, DDL returns
                                        //     Err, but the index above is
                                        //     already registered
}
```

If `self.interner.persist()` itself fails (durable-storage error), the
function returns `Err` to the caller — but `create_index_from_records`
already ran, so the index is LIVE in `self.index_manager` regardless. The
caller believes the whole operation failed; a subsequent `DescribeTable`/
query would actually find the index present. On a hybrid repo (F-33) this
is especially dangerous: the persist call exists SPECIFICALLY to durably
save the interner ids the new index definition references — if it fails,
the live index now depends on interner state that may not survive a
restart, reopening exactly the corruption class F-33 was built to
prevent, at the failure-path level rather than the happy path.

**Read `build_index_definition` (~line 630-644) and
`create_unique_index_locked` (~line 495-514) in full first** — both
follow the identical intern-then-register-then-persist shape.

## What to build

### 1. Reorder: persist BEFORE publish

The interning itself (`build_index_definition`'s `intern_string`/
`intern_path` calls) already happens BEFORE `create_index_from_records`
runs — the in-memory interner state has the new ids assigned by the time
the index is registered. The ONLY thing currently deferred to the end is
the DURABLE persist of those already-in-memory ids. Move the persist call
to run immediately after `build_index_definition` (before
`collect_all_current_records`/`create_index_from_records`/
`create_unique_index_from_records`), so a persist failure aborts BEFORE
the index is ever published — no rollback needed, because nothing was
published yet:

```rust
pub async fn create_index(&self, name: &str, paths: &[&str]) -> DbResult<()> {
    let index_def = self.build_index_definition(name, paths).await?;
    // F-42: persist the interner's newly-touched ids BEFORE the index
    // goes live — a persist failure must abort before publish, not after.
    self.interner.persist().await?;
    let records = self.collect_all_current_records().await?;
    self.index_manager
        .create_index_from_records(index_def, records)
        .await
}
```

Apply the same reordering to `create_unique_index_locked`. Verify this
reordering has no other functional side effect — `create_index_from_records`/
`create_unique_index_from_records` only need `index_def` (already fully
built, interned in-memory, by `build_index_definition`) and the collected
records; they don't depend on the interner having been durably persisted
first. State in your summary that you've confirmed this independently by
reading the actual call signatures, not just trusting this brief's claim.

### 2. Check `create_sorted_index_with_include` for the SAME pre-existing pattern

`crates/shamir-engine/src/table/table_manager_sorted_index.rs`'s
`create_sorted_index_with_include` (~line 22-56) has the IDENTICAL shape:
interns name+path ids, calls `self.sorted_indexes.register(def).await?`
(publish), THEN `self.interner.persist().await?` — the same
register-before-persist ordering the review flagged for the other two
functions, just not explicitly cited by name in the review's "Где"
section. Confirm this yourself, and apply the SAME reordering fix here
too for consistency (persist before register) UNLESS you find a concrete
reason this function's case is different (e.g. `intern_included_paths`
needing to run against the freshly-registered def for some reason — check
before assuming symmetry is safe). State your finding and decision
explicitly in your summary either way.

## Tests — MANDATORY, in the same commit

Extend the existing index-creation test coverage (find the right test
module — check `crates/shamir-engine/src/table/tests/` for existing
`create_index`/`create_unique_index` tests to extend, or add a focused
new test file if none fits):

1. **Fault-injection: persist failure leaves no live index.** You'll need
   a way to make `self.interner.persist()` fail deterministically for a
   single call — check `InternerManager`'s structure for an existing
   test-only failure-injection seam, or whether the underlying `info_store`
   can be wrapped with a failing `Store` (mirroring the failing-store test
   doubles already added this session in `storage_mirrored_tests.rs` for
   F-39/F-41 — check those for the pattern to reuse/adapt at this layer).
   Confirm: after a persist failure, `create_index`/`create_unique_index`
   returns `Err`, AND the index does NOT exist afterward (check via
   `index_manager_ref().iter_indexes()` or the equivalent introspection
   this crate already uses in its own tests) — the actual atomicity proof,
   not just "an error was returned".
2. **Regression, happy path**: the existing non-failure case still creates
   the index and persists correctly (find and re-confirm any existing test
   already covering this rather than skipping it).
3. If you applied the same fix to `create_sorted_index_with_include`, add
   the equivalent fault-injection test for it too.

## Constraints

- Do NOT touch `build_index_definition`'s interning logic itself, only
  the ORDER of the persist call relative to registration.
- Do NOT change any public API signatures.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine -- --check` and
  `cargo clippy -p shamir-engine --all-targets -- -D warnings` must be
  clean.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -- index
./scripts/test.sh -p shamir-engine --full
```
