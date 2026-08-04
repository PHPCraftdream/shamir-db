# Brief — #970 follow-up: `include` still silently ignored on non-btree index_type

Task: #990 in the session TaskList. Found by an adversarial `@oh` review of
the just-completed #957-971 wave (2026-08-04), specifically re-checking
#970's (P1-6) cross-type validation sweep. Read this brief in full.

## The gap — confirmed by direct read

#970 added 8 cross-type validation checks (server `admin_table_index.rs` /
Rust `CreateIndex::try_build()` / TS `createIndex()`) closing the class of
bug where an option meaningful only to ONE index family is silently ignored
when set alongside a DIFFERENT `index_type` (headline case:
`.unique().index_type("vector")` silently created a non-unique vector
index). **The `include` (covering-index) field was missed from that sweep.**

Verified directly: `crates/shamir-db/src/shamir_db/execute/admin_table_index.rs`
has `let non_btree = matches!(itype, Some("vector") | Some("fts") |
Some("functional"));` (~line 387) and 8 checks using it (~lines 389-446),
ALL running BEFORE the non-btree dispatch (~line 448). But the existing
`include`-without-`sorted` check —
`if !op.include.is_empty() && !op.sorted { return Err(...) }` — sits at
~line 463, AFTER the non-btree dispatch's early return (line 448-458),
i.e. still in the btree-only region. So `.include([...]).index_type("fts")`
(or `"vector"`/`"functional"`) is dispatched straight to `create_index_v2`
and the `include` value is silently dropped — the EXACT bug class check 2
(`unique` on non-btree) was added to close, just for a different field.
Same gap exists in `CreateIndex::try_build()`
(`crates/shamir-query-builder/src/ddl/create_index.rs`, its mirror check is
at ~line 254, same problem — it's placed AFTER the 8 new checks but the 8
new checks don't cover `include`).

**Additional, pre-existing gap found while investigating (not introduced by
#970, but directly relevant — close it in the same pass):** the TS builder
(`crates/shamir-client-ts/src/core/builders/ddl.ts`'s `createIndex()`) has
**NO `include`-related check at all** — not even the base
`include`-without-`sorted` rejection that server/Rust have always had. It
was one of the two checks (`include`-without-`sorted`,
`sorted`-multi-field) that #970's own investigation phase noted the TS
builder never mirrored (only `unique && sorted` was mirrored before #970).
Since this brief is already touching `include` validation in this file,
close BOTH gaps together: add the base `include`-without-`sorted` check AND
the new non-btree check, so TS finally has full parity with server/Rust for
`include`.

## Required work

### 1. Server (`admin_table_index.rs`)

Add a new check in the pre-dispatch region (alongside the existing 8, i.e.
BEFORE the `if op.index_type.as_deref().is_some_and(|t| t != "btree")`
dispatch at ~line 448):

```rust
// 9. `include` (covering index) is only meaningful for sorted btree indexes.
if !op.include.is_empty() && non_btree {
    return Err(err(format!(
        "`include` is not supported for '{}' indexes; covering fields are \
         only valid for sorted indexes",
        itype.unwrap()
    )));
}
```

Leave the EXISTING `!op.include.is_empty() && !op.sorted` check (~line 463)
exactly where it is — it still correctly covers the btree-family
include-without-sorted case (regular/unique/`index_type: None`/`"btree"`
with `include` set but not `sorted`). The new check only closes the
non-btree gap.

### 2. Rust `CreateIndex::try_build()`

Same addition, in the same relative position (alongside the other 8
cross-type checks, before the existing `include`-without-`sorted` check).
Add a new `CreateIndexBuildError` variant (e.g. `IncludeUnsupportedForType
{ index_type: String }`) with a `Display` impl following the exact style
of the other 8 variants #970 added (state the rule, cross-reference the
server message). Update `try_build()`'s doc comment list of covered errors
to add this one.

### 3. TS builder (`createIndex()` in `ddl.ts`)

Add BOTH:
- The base check: `!sorted && include is non-empty` → throw (mirrors the
  server's pre-existing ~line 463 check — this closes a gap that predates
  #970, not something #970 introduced).
- The new check: `non_btree && include is non-empty` → throw (mirrors the
  new server/Rust check above).

Match wording/style to the other 8 checks #970 already added in this same
function.

### 4. Tests

- Extend `crates/shamir-query-builder/tests/create_index_try_build_msgpack.rs`
  Part C with a new test: `.include([...]).index_type("fts")` (or
  `"vector"`/`"functional"`) rejected with the new
  `CreateIndexBuildError::IncludeUnsupportedForType` variant.
- Extend `crates/shamir-db/tests/create_index_validation_e2e.rs` (the file
  #970 created) with a live server-side test proving the same rejection
  through the full wire pipeline.
- Extend `crates/shamir-client-ts/src/core/builders/__tests__/ddl.test.ts`
  with tests for BOTH the base include-without-sorted throw AND the new
  non-btree throw.

## Scope discipline

- Do NOT touch any of the other 8 checks #970 added — this is one
  additional check plus closing the TS-side `include` parity gap.
- Do NOT touch `vector_quantization`'s handling or any other #970 area —
  out of scope, already covered.
- Keep the fix minimal — one new check per layer (server/try_build), plus
  the TS parity completion described above.

## Gate (MANDATORY)

```
cargo fmt -p shamir-query-builder -p shamir-db -- --check
cargo clippy -p shamir-query-builder -p shamir-db --all-targets -- -D warnings
./scripts/test.sh -p shamir-query-builder -p shamir-db --full
```
Plus, since `ddl.ts` is touched:
```
npx tsc --noEmit   # run inside crates/shamir-client-ts
npx vitest run src/core/builders/__tests__/ddl.test.ts
```

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Do NOT run
`git commit` or `git add` — the orchestrator verifies your diff and the
test run, then commits. Only edit/create files and run read-only/test/gate
commands.

## What to report back

Show the exact new check added in all 3 layers with exact wording. List
every new test added. Give exact gate command output.
