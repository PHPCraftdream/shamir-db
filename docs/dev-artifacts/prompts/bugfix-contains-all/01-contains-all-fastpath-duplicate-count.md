# Bug — `$contains_all` fast-path counts duplicate field elements toward the required total

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing. This has caused repeated
problems in prior sessions and must not happen again.

## The bug (found by a read-only research audit)

`crates/shamir-engine/src/query/filter/filter_node.rs:601-624`
(`FilterNode::ContainsAllSet::matches`) — this fast path is selected
whenever ALL filter values in a `$contains_all` are literals (the common
case, `compile.rs:286-305`). It counts **raw element hits**, not distinct
set-members found:

```rust
let required = values.len();
let found = match &field_qv {
    QueryValue::List(list) => list.iter().filter(|item| values.contains(*item)).count(),
    ...
};
found >= required
```

If the field's `List` contains duplicates, each duplicate that happens to
be a member of the required set increments `found` — so duplicates of ONE
required value can numerically stand in for OTHER required values that are
actually absent.

**Trigger:** filter `{"tags": {"$contains_all": ["a", "b"]}}` against a
record with `tags = ["a", "a"]`. `required = 2`. `found` = 2 (both `"a"`
elements are members of `{"a","b"}`) → the fast path reports **match**,
even though `"b"` is completely absent from `tags`.

The slow-path twin `ContainsAll` (`filter_node.rs:573-599`, taken only when
some filter value is non-literal — e.g. one operand is a `$param`) does
this correctly: `values.iter().all(|fv| field contains fv)` → `false` for
the exact same input. So the SAME logical filter gives different, silently
wrong answers depending on whether its value list happens to be
all-literal — a correctness bug reachable through the common (all-literal)
path, silently, with no error.

## The fix

Change `ContainsAllSet::matches` to count **distinct set members actually
found** in the field's list, not raw element hits. Two equivalent
approaches — pick whichever fits the existing code shape best:

1. Track which specific set members have been seen (e.g. iterate the
   field's list once, and for each element that is in the required set,
   mark that specific required-value as found — using a bitset/HashSet
   keyed on the required values, or by removing found items from a
   scratch copy of the set), then check all required values were marked.
2. Or simply: `values.iter().all(|fv| list.contains(fv))` — mirroring the
   slow path's semantics exactly (this is simpler and provably correct;
   prefer it unless there's a clear reason the codebase wants a
   single-pass-over-the-field-list approach for performance — if so,
   implement approach 1, but justify the choice in your summary).

The fix must make `ContainsAllSet` and `ContainsAll` (the slow path) agree
on every input — that is the correctness bar. Read both implementations
(`filter_node.rs:573-599` and `:601-624`) and the surrounding compile-path
selection logic (`compile.rs:286-305`) before writing the fix, to confirm
you understand exactly which code path is taken when and why they must
match.

## Tests (find the existing filter test file/module — likely
## `crates/shamir-engine/src/query/filter/tests/` or wherever `ContainsAll`/
## `filter_node` tests currently live; follow its conventions)

1. **The exact bug, closed**: field `tags = ["a", "a"]`, filter
   `{"tags": {"$contains_all": ["a", "b"]}}` (all-literal, so the FAST path
   is exercised) — must NOT match (assert `false`/no match), since `"b"`
   is genuinely absent.
2. **Positive case still works**: field `tags = ["a", "b", "a"]` (with a
   duplicate PLUS all required values genuinely present), same filter —
   must match.
3. **Parity test**: construct the identical logical filter/field pair once
   with all-literal values (exercises `ContainsAllSet`) and once with one
   operand replaced by a `$param`/non-literal (exercises `ContainsAll`) —
   assert both paths agree on the SAME set of inputs, for at least the
   duplicate-elements case and a few other representative cases (empty
   field list, exact match, superset match, subset-missing-one).
4. **Regression**: any existing `$contains_all` tests must continue to
   pass unchanged.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-engine --full` green, including all new
  tests.
- `cargo fmt --all -- --check` clean (or scoped to `shamir-engine`, report
  which).
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Report literal command output for all of the above.
- Confirm explicitly: does your fix change the fast path's PERFORMANCE
  characteristics in any way that matters (e.g. now O(n*m) instead of
  O(n))? State the complexity before/after in your summary — this is a
  hot filter path, so a correctness fix that accidentally reintroduces a
  much worse complexity class should be flagged, though correctness must
  win if there's a genuine tradeoff.

## Out of scope

- Do NOT touch `ContainsAnySet`/`InSet`'s own separate coercion-divergence
  issues (a different, already-known, lower-priority finding about
  Int/F64 coercion in fast-path membership probes) — that is a separate
  task, not in scope here.
- Do NOT touch anything unrelated to `ContainsAllSet`'s counting logic and
  its direct test coverage.
