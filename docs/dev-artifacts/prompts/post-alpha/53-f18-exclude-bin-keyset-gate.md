# Brief for F-18 (#811, P2) — exclude `TypeTag::Bin` from the schema-typed keyset gate

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

Found by both the `/crush` post-wave review
(`docs/dev-artifacts/research/2026-07-26-wave-f-post-review-crush/REPORT.md`,
NF-1) and the deeper static-audit review
(`docs/dev-artifacts/research/2026-07-26-new-wave-release-review.md`, R1's
addendum). `order_by_column_is_schema_typed_scalar`
(`crates/shamir-server/src/db_handler/cursor_handlers.rs`, accepted
`TypeTag` set: `Int | Bool | String | Bin`) accepts `TypeTag::Bin`, but
`safe_seek_key` (same file, `~line 641-648`) always returns `None` for
`QueryValue::Bin(_)` — there is no `compare_values` arm for
`(Bin, Bin)` in `shamir-engine`'s `resolve.rs`, so a `Bin`-typed keyset
cursor can never actually use a real keyset seek; it always degrades to
the per-call offset-bookmark fallback (W-2's existing safety net) on every
`FetchNext` past the first.

**No row loss results from this** — W-2's fallback correctly prevents it.
The only cost is wasted work: the gate lets a `Bin` column enter
`PaginationMode::Keyset` (paying for the null-probe read at
`create_cursor` time) only for `safe_seek_key` to immediately neutralize
every subsequent bookmark anyway. This is a correctness-neutral,
efficiency/clarity fix — much smaller in scope than F-17 (#810, already
completed, which fixed the actual P0 historical-row-homogeneity gap in
this same gate).

Note: F-17 (#810) added a `keyset_safe` field to `FieldRule`/`FieldRuleDto`
and changed `order_by_column_is_schema_typed_scalar` to require
`keyset_safe == true` on top of the `TypeTag` check — that change already
landed (commit `93608455`). This task is independent and orthogonal: it
narrows the ACCEPTED `TypeTag` set itself, unrelated to the `keyset_safe`
proof mechanism.

## What to fix

1. In `order_by_column_is_schema_typed_scalar`'s final `matches!` (the
   accepted `TypeTag` set), remove `TypeTag::Bin` — leaving `Int | Bool |
   String` (mirroring how `F64`/`Dec`/`Big`/containers are already
   excluded, per the doc comment's existing rationale for those).
2. Update the function's doc comment to note `Bin` is now excluded too, and
   why (no `compare_values` arm, so it can never benefit from Keyset mode —
   cite this brief / F-18 / #811).
3. Add a **positive keyset test for `Bool`**, which the two reviews noted
   has NO existing positive keyset-mode test despite being in the accepted
   set and being genuinely comparable via `compare_values` (confirmed:
   `resolve.rs` has a working `(Bool, Bool)` arm) — find
   `crates/shamir-server/src/db_handler/tests/cursor_handler_tests.rs`'s
   existing per-`TypeTag` positive/negative test pattern (e.g. the `Int`/
   `Str` positive tests, the `Bin`/`List`/`Dec`/`Big`/`F64` exclusion tests)
   and add one for `Bool` following the same structure: bind a schema
   declaring a `Bool` field (on an empty table, so F-17's `keyset_safe`
   proof is `true`), seed rows with distinct `Bool` values, open a
   keyset-requesting cursor, and assert `PaginationMode::Keyset` is reached
   and every row is returned exactly once.
4. Update the existing `Bin`-related test(s) in `cursor_handler_tests.rs`
   (search for `bin_order_by_value_uses_offset_fallback_not_silent_drop` or
   similarly named) — since `Bin` is now excluded from the gate BEFORE
   `safe_seek_key` is ever reached, confirm whether this test's assertion
   message/reasoning needs updating (it may currently say something like
   "a schema-typed Bin ORDER BY column passes F-1's schema-typed-scalar gate
   ... W-2's fix is a per-bookmark fallback" — that's no longer accurate
   once `Bin` is excluded at the GATE level; the test's `PaginationMode`
   assertion itself likely doesn't need to change value (still `Offset`),
   but the REASON has changed from "gate passes, per-call fallback kicks
   in" to "gate itself now excludes Bin" — update the doc comment/assertion
   message accordingly, don't just leave it stale).
5. Update `docs/guide-docs/KNOWN_LIMITATIONS.md` §6 if it mentions `Bin` in
   the accepted-`TypeTag` list for this gate (search for "Bin" near the
   "Mixed `QueryValue` type" bullet F-17 already rewrote) — correct it to
   reflect the now-narrower accepted set (`Int`/`Bool`/`String`).

## Constraints

- Do NOT touch the `keyset_safe`/F-17 proof mechanism — this task only
  narrows the `TypeTag` set, independent of that mechanism.
- Do NOT touch `List`/`Dec`/`Big`/`F64`/container exclusions — already
  correct and out of scope.
- `cargo fmt -p shamir-server -- --check` and
  `cargo clippy -p shamir-server --all-targets -- -D warnings` must be
  clean.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.

## Verification the orchestrator will run

```
cargo fmt -p shamir-server -- --check
cargo clippy -p shamir-server --all-targets -- -D warnings
./scripts/test.sh -p shamir-server -- cursor_handler
./scripts/test.sh -p shamir-server --full
```
