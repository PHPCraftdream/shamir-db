# Brief — #1002 (part of #1001): fix two stale comments left over from #983's round-1 misdiagnosis

Task: #1002 in the session TaskList (a decomposed leaf of the former #1001
umbrella). Small, doc/comment-only task — two specific stale comments
flagged by `@oh`'s adversarial review of the #983/#984/#997/#998 batch.
Both are a direct consequence of #983 having TWO independent root causes
(see `docs/dev-artifacts/prompts/bugfix-983/01-binary-value-roundtrip.md`
through `04-fix-filtervalue-binary-string-ambiguity.md` for the full
investigation trail) — the first delegated attempt found and fixed root
cause #1 (a missing `Bin`-vs-`Bin` comparison arm) and, believing that was
the whole bug, left comments describing the SECOND root cause (the
`FilterValue` untagged-enum wire ambiguity) as still-open/client-side. It
was later found and fixed in the same #983 commit (`80d08caa`). These two
comments were never updated to reflect that.

## Comment 1 — `crates/shamir-engine/src/query/filter/tests/eval_bytes_tests.rs:408-413`

Current text (as of this brief):
```rust
// `make_record` inserts `bin = InnerValue::Bin(vec![1, 2, 3])`. A
// `FilterValue::Binary` literal MUST match byte-for-byte via the bytes-eval
// path (`RawScalar::Bin` vs `FilterValue::Binary`, eval_bytes.rs ~487). This
// pins the engine-side filter floor for #983: if a row is stored as `Bin`, the
// binary-equality filter finds it (the defect for #983 is client-side — see the
// TS bisection — so this Rust test must PASS unchanged).
```

The final sentence ("the defect for #983 is client-side… this Rust test
must PASS unchanged") is from the round-1 misdiagnosis. It's misleading now:
this Rust-side comparison gap (no `Bin`-vs-`Bin` arm in
`compare_values`/`scalar_ref_cmp`/`scalar_ref_cmp_qv`) WAS itself a genuine
engine-side bug, fixed in the SAME commit (`80d08caa`) that added the
comparison arm at `crates/shamir-engine/src/query/filter/resolve.rs` (grep
for the `(Value::Bin(a), Value::Bin(b)) => Some(a.cmp(b))` arm to confirm
the current line). Read `git show 80d08caa --stat` and the actual diff to
`resolve.rs` to confirm the exact before/after, then rewrite the comment to
state the TRUE history precisely: the test was written to pin down the
engine-side comparison gap, which was real, found, and fixed in 80d08caa —
not "client-side, no Rust change needed". Do not just delete the sentence;
replace it with an accurate one so a future reader understands why this
test exists and what commit closed the gap it guards against.

## Comment 2 — `crates/shamir-client/tests/batch_for_each_e2e.rs:159-186` (the doc comment on `audit_insert_body_with_div_guard`)

Current text describes TWO separate, pre-existing reasons a simpler
unique-index-violation-based test scenario was avoided in favor of a
`math/mod` div-by-zero failure trigger:

1. `FilterValue`'s untagged msgpack round-trip collapsing a small-int
   literal array into the WRONG variant (`Binary` instead of `Array`) —
   THIS IS THE EXACT #983 BUG, root cause #2 (the `de_binary_strict`
   deserializer in `crates/shamir-query-types/src/filter/filter_value.rs`
   now rejects `visit_seq`, so `Array` can no longer be mis-decoded as
   `Binary`). This reason is now STALE — the bug it describes is fixed.
2. Unique-index validation only checking against DURABLE committed state,
   so two conflicting inserts within the SAME still-open transaction never
   cross-validate against each other. **This is a SEPARATE, unrelated issue,
   NOT fixed by #983 or by #987** (#987 fixed a different bug: a tx staged
   BEFORE a `CREATE UNIQUE INDEX` wasn't retroactively validated at commit —
   a timing/ordering bug in index creation, not the general
   staged-vs-staged-in-the-same-tx non-cross-validation gap this comment
   describes). Confirm this by reading #987's actual fix
   (`crates/shamir-engine/src/tx/`, search recent commits/CHANGELOG for
   #987) and confirming it does NOT touch the general two-inserts-in-one-tx
   case. If you find it's actually unrelated as expected, leave reason 2's
   text as-is (still an accurate, open issue) — do NOT mark it fixed.

**What to do:**
- Update reason 1's text to past tense, citing the actual fix (#983,
  commit `80d08caa`, the `de_binary_strict` deserializer) — it is no longer
  a live blocker.
- Leave reason 2 exactly as it is (still open, unrelated) UNLESS your own
  investigation finds it's ALSO been closed by something else — verify
  first, don't assume.
- **Then determine: with reason 1 gone, does reason 2 ALONE still force
  this test to use the `math/mod` workaround instead of a plain
  unique-index violation?** Almost certainly yes (reason 2 is a structural
  gap unrelated to #983) — if so, say so explicitly in the comment (e.g.
  "issue (1) above is fixed as of #983 (commit 80d08caa); issue (2) alone
  still makes a unique-index-based conflict unreliable within a single
  open transaction, so this workaround remains necessary") and do NOT
  attempt to revert `for_each_iteration_error_mid_loop_rolls_back_whole_tx_over_real_wire`
  to a simpler unique-index-based trigger — that would very likely
  reintroduce the exact flake reason 2 describes. If your investigation
  surprises you and reason 2 turns out to ALSO be resolved, then and only
  then experiment with reverting to the simpler shape, and if it works
  reliably (run the test standalone at least 10x to rule out flakiness),
  do the revert and explain why in your report. Default expectation:
  reason 2 still stands, no revert.

## Scope discipline

- Comment/doc-only changes. No behavior change.
- Do not touch any other code in either file beyond the two comments
  described above (and, only if your investigation genuinely supports it,
  the test body change described in the conditional revert path).
- Do not touch `#983`'s actual fix commits or any other file.

## Gate (MANDATORY)

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -- eval_bytes
./scripts/test.sh -p shamir-client -- batch_for_each
```

⚠️ Raw `cargo test` is BLOCKED by this repo's perimeter guard. Use
`./scripts/test.sh` (`-p <crate>`, `-- <substring>` for a narrow run).

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Do NOT run
`git commit` or `git add` — the orchestrator verifies your diff and the
test run, then commits. Only edit/create files and run read-only / test /
gate commands.

## What to report back

- The corrected text for both comments (paste the new versions).
- Confirmation (via `git show 80d08caa` or equivalent) that comment 1's
  claim about the engine-side fix is accurate.
- Confirmation of whether #987 does or doesn't touch the general
  same-tx-cross-validation gap comment 2 describes.
- Whether you attempted the revert path for the `audit_insert_body_with_div_guard`
  workaround, and if so, the result of running the affected test 10x
  standalone.
- Exact gate command output.
