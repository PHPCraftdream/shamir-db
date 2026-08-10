# Brief 67 — #1061 round 2: strengthen test 4's equivalence check

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## What round 1 got right — do not touch

Tests 1, 2, 3 in `crates/shamir-engine/src/table/tests/p1061_pipeline_property_tests.rs`
are correct and pass. `cargo fmt`/`clippy` are clean, the full
`shamir-engine`/`shamir-index` suite is green (1941/1941, 0 timeouts).
Leave tests 1-3 exactly as they are.

## What round 1 got right about test 4's deviation (verified, not assumed)

Round 1 honestly reported deviating from the brief's "byte-identical
posting sets" requirement to a weaker "same COUNT of postings" comparison
in `p1061_equivalence_with_old_path_byte_identical_postings`, citing
"separate `TableManager` instances have independent interners, so
interner IDs for the same strings differ across tables."

**I (the orchestrator) verified this claim directly**: I temporarily
changed the assertion back to `assert_eq!(postings_new, postings_old, ...)`
(true byte-identical `BTreeSet` comparison) and re-ran the test — it FAILS.
So the deviation reason is factually correct, not an excuse. There is no
`TableManager` API to force two independently-constructed instances to
share one interner's numbering space (confirmed: no
`with_shared_interner` or equivalent method exists), so literal
byte-identical posting-key comparison is not achievable here without a much
larger change (e.g., restructuring the test to build both index families
inside ONE shared-interner table, which would defeat the point of
comparing the actual PUBLIC `create_index` entry point end-to-end for two
genuinely separate tables). Do not attempt to force byte-identical
comparison again — it's a known dead end for this test's setup.

## The gap this round closes

Round 1's own "Residual Risks" section flagged the real weakness of a
COUNT-only comparison: "a regression that mis-indexes records with correct
count but wrong RecordIds would not be caught by this test alone." That
gap is real and worth closing without needing byte-identical keys.

**Fix: replace the count comparison with a per-distinct-value RecordId-set
comparison via `lookup_by_index`.** This is stronger than a count (it
catches "right count, wrong records" mis-indexing) and is immune to the
interner-numbering mismatch (it operates at the observable-behavior level
— `lookup_by_index(name_interned, &[value])` on EACH table returns
`RecordId`s from THAT table's own data, so there's no cross-table id
comparison to go wrong).

In `p1061_equivalence_with_old_path_byte_identical_postings`
(rename it to `p1061_equivalence_with_old_path_per_value_lookup_sets` to
match what it now actually does), after building both `tbl_new` and
`tbl_old` with the identical fixture and creating the index on each:

```rust
// For every DISTINCT indexed value in the fixture (not just a handful),
// assert the lookup result SET matches between old and new paths.
use std::collections::BTreeSet;

let distinct_values: BTreeSet<String> = fixture_rows
    .iter()
    .filter_map(|(_, name_opt)| name_opt.clone())
    .collect();

for value in &distinct_values {
    let lookup_value = vec![InnerValue::Str(value.clone())];

    let results_new = tbl_new
        .index_manager_ref()
        .lookup_by_index(name_interned_new, &lookup_value)
        .await
        .unwrap()
        .map(|arc| arc.iter().copied().collect::<BTreeSet<_>>())
        .unwrap_or_default();
    let results_old = tbl_old
        .index_manager_ref()
        .lookup_by_index(name_interned_old, &lookup_value)
        .await
        .unwrap()
        .map(|arc| arc.iter().copied().collect::<BTreeSet<_>>())
        .unwrap_or_default();

    // RecordIds are globally unique (not interner-numbered), so a direct
    // set comparison is valid here — unlike the raw posting KEY bytes,
    // which embed the interner-numbered name_interned and are therefore
    // NOT comparable across two independently-constructed tables.
    assert_eq!(
        results_new.len(),
        results_old.len(),
        "value '{value}': result set SIZE must match between old and new paths"
    );
    // Since RecordIds are freshly generated per-table (timestamp+seq based,
    // not content-derived), we cannot assert the SAME RecordId appears in
    // both sets directly. Instead, verify each RETURNED record, in ITS OWN
    // table, actually has this value at the "name" field — i.e. no
    // cross-contamination (a record with a DIFFERENT value wrongly
    // returned by one path but not the other).
    //
    // NOTE: `TableManager::get(rid)` returns `InnerValue`, NOT `QueryRecord`
    // (`get_value_str` lives on `QueryRecord`, shamir-query-types, and does
    // NOT apply here — verified, do not reach for it). Extract the field by
    // matching on `InnerValue::Map` and looking up the interned "name" key
    // directly, mirroring how every other test in this file BUILDS records
    // (`m.insert(name_key, InnerValue::Str(...))`) — read it back the same way.
    let name_key = {
        let interner = tbl_new.interner().get().await.unwrap();
        interner.touch_ind("name").unwrap().into_key()
    };
    for rid in &results_new {
        let rec = tbl_new.get(*rid).await.unwrap();
        let InnerValue::Map(m) = &rec else {
            panic!("expected record {rid:?} to be a Map");
        };
        assert_eq!(
            m.get(&name_key),
            Some(&InnerValue::Str(value.clone())),
            "new path: record {rid:?} returned for value '{value}' must actually have that value"
        );
    }
    let name_key_old = {
        let interner = tbl_old.interner().get().await.unwrap();
        interner.touch_ind("name").unwrap().into_key()
    };
    for rid in &results_old {
        let rec = tbl_old.get(*rid).await.unwrap();
        let InnerValue::Map(m) = &rec else {
            panic!("expected record {rid:?} to be a Map");
        };
        assert_eq!(
            m.get(&name_key_old),
            Some(&InnerValue::Str(value.clone())),
            "old path: record {rid:?} returned for value '{value}' must actually have that value"
        );
    }
}
```

Verify `TMap`'s (or whatever the `InnerValue::Map` inner type is)
`.get(&key)` signature against the actual type before using it — check how
`insert_record`/the record-building helpers already in this file construct
and read maps, and match that exact pattern. This is scaffolding to convey
INTENT (per-value lookup-set equivalence, verified against each record's
own actual field content), not a guaranteed-to-compile snippet — adjust
names/types as needed while preserving the approach.

**Also keep the existing "both-sides-empty false-pass" guard** (assert the
total posting count across both paths is a substantial fraction of the
fixture, not zero) — this still matters as a sanity check independent of
the per-value comparison above.

This closes round 1's self-identified gap: a mis-indexing bug that
preserves total COUNT but attaches the wrong RecordId to a value (or wrong
value to a RecordId) would now fail the per-record content check above,
which the pure count comparison could never catch.

## After the fix — re-run and confirm

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -- p1061_
./scripts/test.sh -p shamir-index
./scripts/test.sh -p shamir-engine
```

All `p1061_*` tests must pass (including the renamed test 4). Paste the
exact nextest output for all of them.

## Gate before you report done

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-index
./scripts/test.sh -p shamir-engine
```

Report the exact diff and the exact nextest output for all `p1061_*`
tests plus the full suite's final summary line.
