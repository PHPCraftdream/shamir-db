# FG-6: `scalar_at`/`ScalarRef` — Eq filter and ORDER BY don't see `Big` values

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context (found during FG-1 verification, already root-caused — read before touching code)

After FG-1, a `u64 > i64::MAX` decodes losslessly to `Value::Big`/
`QueryValue::Big` instead of wrapping/clamping. Two downstream consumers of
that value are structurally blind to it:

**1. `Eq` filter doesn't match.** Full root-cause analysis already written
in `crates/shamir-engine/src/query/filter/tests/eval_tests/u64_big_filter_match_tests.rs`
— read it in full first. Summary: `FilterNode::Compare` reads the field via
`RecordRef::scalar_at`, which returns `None` on BOTH paths for a promoted
Big value:
- Lens/hot path (`RecordView`): `uint_to_record_value` maps `u64 > max` to
  `RecordValue::Str(Cow::Owned(decimal))` — `record_ref.rs`'s `scalar_at`
  has an explicit `Cow::Owned(_) => None` branch (`ScalarRef::Str(&'a str)`
  cannot borrow an owned `String` with the right lifetime).
- Tree/cold path (`InnerValue`): `ScalarRef` (`scalar_ref.rs`) has no `Big`
  variant at all — `inner_to_scalar` maps `Big` to `None`.
- Net effect: the compare falls into the `(None, _) => false` fallback —
  no panic, no wrong answer, just a silent non-match.

**2. `ORDER BY` doesn't cross-compare `Int`/`Big` correctly.** Test +
comment already in
`crates/shamir-engine/src/query/read/tests/qv_postprocess_tests.rs`
(`order_by_mixed_int_and_big_compare_values_works`) — read it first.
Summary: `apply_order_by_qv`'s `QvSortKey` maps `Big(b)` →
`Str(b.to_string())` but has no `Int`↔`Str`/`Int`↔`Big` cross-type
comparison arm, so mixed Int+Big sorting falls into `_ => Equal`
(insertion order preserved instead of a real sort). **`compare_values`**
(used by `Filter::ValueCompare`, and by `Min`/`Max` aggregates) already
handles this correctly — the gap is specific to `QvSortKey`, not the whole
comparison layer. Read `compare_values`'s existing Int/Big arm as your
reference for what `QvSortKey` needs to gain.

Already documented in
`docs/guide-docs/client-server-protocol-spec/NUMERIC_WIRE_SEMANTICS.md`
("Known limitation" section) and the FG-1 `CHANGELOG.md` bullet, and
cross-referenced from `docs/guide-docs/KNOWN_LIMITATIONS.md` (§7, this
campaign) — update all three once this is fixed (see Docs section below).

## Your decision to make (not pre-decided — pick one, document why)

(a) Add `ScalarRef::Big` + a `scalar_at` path that reads a `Big` directly.
    Solves the tree/cold path cleanly. For the lens/hot path's
    `Cow::Owned` case, you'll need a separate mechanism (the borrow
    lifetime genuinely doesn't work for `ScalarRef::Str(&'a str)`) —
    investigate whether a `ScalarRef::Big(BigInt)` (owned, not borrowed)
    variant sidesteps this, or whether the lens path needs its own
    escape hatch.
(b) `FilterNode::Compare` falls back to `materialize_at` (a full value
    read, not just `scalar_at`) whenever `scalar_at` returns `None` for a
    field that DID resolve as present — trades a slower path for a
    correct one only in the promoted-Big case (rare in practice), leaves
    the hot path fully unchanged for every other value type.
(c) Add a cross-type `Int`↔`Str`/`Int`↔`Big` comparison arm to
    `QvSortKey`, mirroring `compare_values`'s existing arm — this alone
    fixes ORDER BY but does NOT fix the Eq filter gap (item 1 above),
    which is a separate code path. If you pick a fix for item 1 that
    doesn't naturally also cover item 2, you still need this arm
    separately for ORDER BY.

Read `lookup_records_via_index`/`compare_values`/the two test files above
in full BEFORE deciding — this is a real design choice with real tradeoffs
(hot-path cost vs. correctness), not a mechanical fill-in. Pick the
approach that fixes BOTH item 1 and item 2 with the least hot-path cost to
the common (non-Big) case; if no single approach naturally covers both,
implement two targeted fixes and say so.

## Tests (MANDATORY)

1. **Flip** the existing `u64_big_filter_match_tests.rs` assertions — they
   currently assert "does NOT match"; after your fix, assert "DOES match".
   Update the test names/doc comments to stop describing a bug — describe
   the now-correct behavior.
2. **Flip** `order_by_mixed_int_and_big_compare_values_works` in
   `qv_postprocess_tests.rs` similarly — real cross-type sort assertion,
   not just `compare_values` in isolation (exercise the real ORDER BY path,
   `apply_order_by_qv`, not just the comparator function directly).
3. Confirm `compare_values`-based paths (`Filter::ValueCompare`, `Min`/`Max`
   aggregates) still pass unmodified — they were already correct; this
   proves your fix didn't disturb them.

## Docs (update to remove the now-fixed "known limitation" framing)

- `docs/guide-docs/client-server-protocol-spec/NUMERIC_WIRE_SEMANTICS.md`:
  update/remove the "Known limitation" section to describe the fix instead
  of the gap.
- `docs/guide-docs/KNOWN_LIMITATIONS.md` §7 ("Numbers"): remove the
  "Known residual gap" sub-bullet (this campaign, just landed) — the gap
  it describes is fixed by this task.
- `CHANGELOG.md`: one `[Unreleased]` bullet — this is a correctness fix
  closing a real, previously-documented gap, not a breaking change.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh @engine --full` green.
- `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets
  -- -D warnings` clean.
- Report which option (a/b/c, or a hybrid) you took and why, and confirm
  both mandatory test flips actually flip (fail without your fix, pass
  with it — you can verify this by temporarily checking out the test
  assertions' old form mentally / diffing, not by reverting real code).

If, after real investigation, the chosen approach turns out structurally
harder than expected (e.g. the lens-path `Cow::Owned` lifetime problem in
option (a) has no clean solution) — do not force a broken or
silently-incomplete fix. Report the precise structural blocker, and fall
back to whichever of (b)/(c) is cleanest, documenting the tradeoff you
accepted.
