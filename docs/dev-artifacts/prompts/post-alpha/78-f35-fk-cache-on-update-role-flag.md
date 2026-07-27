# Brief for F-35 (#843, P0) — FK reverse cache must track on_update separately from on_delete

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

A readonly review (`docs/dev-artifacts/research/2026-07-27-new-wave-readonly-review.md`,
finding P0-1) found a real, confirmed gap in F-28's FK-atomicity closure:
`FkReverseCache` (`crates/shamir-engine/src/repo/fk_reverse_cache.rs`)
only ever records `fk.on_delete` in each `ReverseFkEntry.action` field
(`build_reverse_fk_entries`, line 266: `action: fk.on_delete`). But
`is_fk_parent_with_action` (line 167-176) — which
`implicit_tx_isolation_for_fk_parent`
(`crates/shamir-engine/src/query/batch/query_runner.rs:345-373`) calls for
BOTH the implicit DELETE arm and the implicit UPDATE arm — is documented,
in both `query_runner.rs`'s own doc comment (line 336-337: *"non-`NoAction`
`on_delete`/`on_update` action"*) AND `fk_on_update.rs`'s module doc
(line 71-74: *"opens the implicit tx as `Serializable` when this table is
an FK parent with a non-`NoAction` **`on_update`** action"*), as covering
`on_update` too. It does not — the underlying cache literally has no
`on_update` information at all.

**Confirmed by reading the code, not just the review**: a FK declared with
`on_delete = NoAction, on_update = Restrict` (or `Cascade`/`SetNull`) will
have `is_fk_parent_with_action` return `false` for its parent table
(because the ONE cached `action` field is `NoAction`), so
`implicit_tx_isolation_for_fk_parent` returns `Snapshot` for an implicit
UPDATE against that parent — even though `plan_fk_on_update`
(`fk_on_update.rs`) independently detects the real `on_update` action and
runs a child-table scan against it. That scan's cross-transaction race
(a concurrent child insert landing between the scan and this tx's commit)
is therefore NOT closed for `on_update`-only FKs, silently contradicting
`fk_on_update.rs`'s own doc claim of closure.

**Also confirmed**: `fk_race_closure_tests.rs`
(`crates/shamir-engine/src/query/batch/tests/`) only constructs FKs via
`ForeignKeyRef::with_on_delete` — check this yourself before writing any
new test — so there is currently no deterministic test that would catch
an `on_update`-only race, which is exactly why this shipped undetected.

## What to build

### 1. `ReverseFkEntry` — two action fields, not one

`crates/shamir-engine/src/repo/fk_reverse_cache.rs`:

```rust
pub struct ReverseFkEntry {
    pub child_table: String,
    pub child_field: String,
    pub parent_ref_field: String,
    pub on_delete: FkAction,
    pub on_update: FkAction,
}
```

(`FkAction` is a plain `Copy` enum — check
`crates/shamir-query-types/src/admin/types/fk_action.rs` — so carrying two
of them is free; no need to box/Arc anything differently.)

Rename the old single `action` field to `on_delete` and add `on_update` —
grep every existing reader of `.action` (in `fk_restrict.rs`, `fk_actions.rs`,
`fk_reverse_cache.rs` itself, and any test file) and update each to read
whichever field it actually needs:
- `fk_restrict.rs`'s RESTRICT-only discovery filters by RESTRICT semantics
  on DELETE → reads `on_delete`.
- `fk_actions.rs`'s CASCADE/SET NULL discovery on DELETE → reads
  `on_delete`.
- Anything discovering the UPDATE-time fan-out (`fk_on_update.rs`'s own
  discovery, if it has one, or wherever `plan_fk_on_update` sources its
  action) should read `on_update` — check whether `fk_on_update.rs`
  currently re-derives this from a live schema lookup rather than the
  cache at all; if so, leave that call path alone (only this cache-role
  question is in scope) and just make sure the CACHE's own two role-flag
  helpers below are correct.

### 2. Two role-flag helpers, or one flag-pair helper

`is_fk_parent_with_action` (line 167) currently answers one boolean. Split
it into two (or return a small struct/tuple) so callers can ask the RIGHT
question for the operation they're actually about to perform:

```rust
/// Is `table` an FK parent with a non-NoAction on_delete action?
pub fn is_fk_parent_with_delete_action(&self, table: &str) -> bool { ... }

/// Is `table` an FK parent with a non-NoAction on_update action?
pub fn is_fk_parent_with_update_action(&self, table: &str) -> bool { ... }
```

Update `query_runner.rs`'s `implicit_tx_isolation_for_fk_parent` (and
whatever calls it — check both the implicit DELETE arm and the implicit
UPDATE arm's call sites) so the DELETE arm calls the delete-flag helper
and the UPDATE arm calls the update-flag helper. Read the existing
call-site code carefully first (~line 200-330 of `query_runner.rs`,
per the review's citation) — there may already be a DELETE-vs-UPDATE
distinction in HOW `implicit_tx_isolation_for_fk_parent` is invoked
(e.g. an operation-kind parameter) that this fix should plug into, rather
than inventing a new parallel path. State in your summary which shape you
found and how you wired it.

### 3. Tests — MANDATORY, in the same commit

Extend `crates/shamir-engine/src/query/batch/tests/fk_race_closure_tests.rs`
(read it in full first — mirror its existing structure/helpers exactly,
including its `RaceInjectingResolver` pattern) with deterministic tests for
FKs shaped `on_delete = NoAction, on_update = Restrict | Cascade | SetNull`:

- For each of the 3 `on_update` action kinds: a concurrent-race test proving
  the implicit UPDATE path now upgrades to Serializable and the race is
  caught (mirroring whatever the existing `on_delete` tests assert —
  `PhantomConflict`/`tx_conflict`/abort-then-retry, whichever shape they
  use).
- Cover BOTH commit orderings the review calls out: child-write-first and
  parent-update-first (check whether the existing on_delete tests already
  parametrize over ordering — if so, mirror that structure for on_update
  too rather than writing ad-hoc one-off orderings).
- A regression test that an FK with on_delete=NoAction, on_update=NoAction
  (truly no action either way) correctly does NOT upgrade to Serializable
  — confirming this fix didn't accidentally make every FK-parent table
  serializable regardless of action kind.

## Constraints

- Do NOT touch `fk_on_update.rs`'s actual row-level plan/scan logic
  (`plan_fk_on_update`, `collect_parent_values`, `child_has_reference`) —
  those are already correct per F-28 Step 2/5; this fix is scoped to the
  CACHE's role-flag data and the isolation-upgrade DECISION consuming it.
- Do NOT touch `FkReverseCache`'s invalidate/populate/generation machinery
  — that's a separate finding (F-36, blocked on this task landing first
  since both touch the same file).
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine -- --check` and
  `cargo clippy -p shamir-engine --all-targets -- -D warnings` must be
  clean.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -- fk
./scripts/test.sh -p shamir-engine --full
```
