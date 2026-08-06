# Brief — F-4: restore `IF NOT EXISTS` semantics for sorted/index2 CREATE

## Context

S.H.A.M.I.R. Database, `crates/shamir-db` + `crates/shamir-engine`. An
adversarial review of the R0 correctness-freeze wave
(`docs/dev-artifacts/research/2026-08-06-r0-wave-adversarial-review.md`
§F-4) found that commit `6602ea4e` (R0-C's cross-family name-uniqueness
preflight) broke `IF NOT EXISTS` for the sorted and index2 index families.
Tracked as task #1029.

## The defect

`crates/shamir-db/src/shamir_db/execute/admin_table_index.rs`'s
`handle_create_index` (search for `let already_exists = if op.unique`,
currently around `:352-374`) implements `IF NOT EXISTS`'s no-op semantics
via a pre-check:

```rust
let already_exists = if op.unique {
    table.unique_index_exists(&op.create_index).await
} else {
    table.index_exists(&op.create_index).await   // only the REGULAR base_index family
};
if already_exists {
    if op.if_not_exists { return Ok(... "existed": true ...); }
    return Err(...);
}
```

This check only ever probes `unique_index_exists`/`index_exists` — it never
checks `sorted_index_exists`/`index2_exists`. For `index_type: "fts"|
"vector"|"functional"` or `sorted: true`, `already_exists` is always
`false`, so control falls through into `create_sorted_index_with_include`/
`create_index_v2`, where R0-C's own cross-family preflight
(`TableManager::any_index_exists`, already `pub` — see
`table_manager_index_mgmt.rs:1457` or nearby, may have shifted) unconditionally
returns `DbError::KeyExists` — **regardless of the `if_not_exists` flag**,
because that preflight has no knowledge of the caller's intent.

Net effect: `CREATE SORTED INDEX idx ON t(age) IF NOT EXISTS` (or the
equivalent for fts/vector/functional) on an already-existing index now
returns an error instead of the expected `{"created": false, "existed":
true}` no-op.

## The fix

Replace the handler's family-narrow `already_exists` probe with the
already-existing cross-family helper:

```rust
let already_exists = table.any_index_exists(&op.create_index).await;
```

This makes `IF NOT EXISTS` work uniformly across all four families, and —
as a side effect worth understanding, not fighting — it also means CREATE
`sorted`/index2 **without** `if_not_exists` now properly returns an
"already exists" error on a duplicate name, instead of the old silent
last-write-wins replace `SortedIndexManager::register` used to do when
called with no pre-check at all. **This is the correct behavior, not a
regression to work around**: regular/unique CREATE already errored on a
duplicate name without `if_not_exists` before R0-C ever landed — sorted's
old "silently replace" behavior was an accidental inconsistency (the
handler simply never checked sorted's existence at all pre-R0-C), not a
deliberately guaranteed contract. Do not try to restore last-write-wins
for sorted; align it with how every other family already behaved.

Add a one-line `CHANGELOG.md` note under `[Unreleased]` (near the existing
R0-C entry, or as a new small bullet) stating plainly: sorted CREATE without
`IF NOT EXISTS` now errors on a duplicate name (previously silently
replaced the definition) — this is an intentional consistency fix aligning
sorted with regular/unique's pre-existing behavior, not a newly introduced
restriction.

## Tests

- `CREATE SORTED INDEX idx ON t(age)` then `CREATE SORTED INDEX idx ON
  t(age) IF NOT EXISTS` → returns `{"created": false, "existed": true}`,
  no error.
- Same for at least one index2 kind (functional or fts) — `CREATE INDEX ...
  IF NOT EXISTS` on an already-existing index2 name → no-op success.
- `CREATE SORTED INDEX idx ON t(age)` then `CREATE SORTED INDEX idx ON
  t(age)` (no `IF NOT EXISTS`) → now returns an error (document this in the
  test's assertion/comment as the intentional consistency fix, referencing
  regular/unique's pre-existing identical behavior).
- Confirm each new test fails against the pre-fix code (the `IF NOT EXISTS`
  cases should currently error instead of no-op'ing).

## Constraints

- Follow `CLAUDE.md` conventions (tests under existing `tests/` directories,
  no inline `#[cfg(test)] mod tests {}`).
- Gate: `cargo fmt -p shamir-db -p shamir-engine`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `./scripts/test.sh -p shamir-db -p shamir-engine --full`, and
  `./scripts/test.sh @oracle` must all be clean.
- Do NOT touch F-3 (#1030 — DROP TABLE CASCADE / doctor::repair() admission
  bypass) — separate task, separate brief coming next.
- This is a small, surgical fix — one line in the handler plus tests plus a
  CHANGELOG note. If you find it's larger than that, stop and report why.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or
any git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## Definition of done

- [ ] `handle_create_index` uses `any_index_exists` instead of the
      family-narrow check.
- [ ] `IF NOT EXISTS` is a correct no-op for sorted and at least one index2
      kind on an already-existing name.
- [ ] Sorted CREATE without `IF NOT EXISTS` on a duplicate name now errors
      (documented as intentional in the test and in CHANGELOG.md).
- [ ] Tests confirmed to fail against pre-fix code.
- [ ] fmt/clippy/tests green (report exact commands and pass/fail).
