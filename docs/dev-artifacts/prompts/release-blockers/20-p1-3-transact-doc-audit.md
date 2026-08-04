# Brief — P1-3: `Store::transact`'s misleading doc framing + stale caller audit

Task: #968 in the session TaskList. Source: `docs/dev-artifacts/research/2026-08-03-new-wave-readonly-review.md` §P1-3. Depends on #962 (sorted RENAME, already landed this session — this task must verify it, not modify it further unless a genuine gap is found). Read this brief in full; the scope here is deliberately narrower than the review's headline "split the API" framing, for reasons explained below.

## What the review is actually complaining about — verified

`crates/shamir-storage/src/types.rs` ~line 179-253 (`Store::transact` +
`Store::supports_atomic_transact`): the RUNTIME BEHAVIOR here is already
correct and already well-documented — F-77/#904 and F-85/#913 (both
predate this session) already fixed the actual overpromise bug and added
the `supports_atomic_transact()` capability flag as honest, queryable
metadata. **This is a documentation/naming clarity issue, not a runtime
correctness bug.** The review's specific complaint: the doc comment's
VERY FIRST SENTENCE reads "Atomic mixed-op batch — either ALL ops succeed
... or NONE are" — and only in a LATER paragraph does it clarify the
DEFAULT implementation is **NOT** atomic. A reader skimming just the first
line gets exactly the wrong impression.

## Why the review's suggested full API split is OUT OF SCOPE here

The review proposes splitting `transact` into two distinctly-named trait
methods (`apply_batch_ordered` for the non-atomic default, `atomic_transact`
for backends that guarantee it, erroring `UnsupportedCapability` otherwise).
**Do not implement this.** `Store::transact` is a trait method with MANY
implementors (every storage backend: `FjallStore`, `MirroredStore`,
`CachedStore`, `MemBufferStore`, `InMemoryStore`, and others) and — verified
via a fresh grep this session — at least 6 current production call sites
across `shamir-index` and `shamir-engine` (see below), several of which were
THEMSELVES rewritten by this session's own P0 work (#957-962/#972). A full
trait-method rename/split is a large, invasive, cross-cutting change that
would ALSO force re-touching this session's just-landed, just-verified P0
fixes for no behavioral gain (the flag-based capability model already
works correctly) — high risk, low marginal value, and explicitly the kind
of thing this task's own text authorizes scaling back
("либо честный rollback, либо..." pattern seen in sibling P1 tasks).

## Required work — scoped to doc accuracy + caller audit

### 1. Rewrite the misleading doc opening (the concrete, low-risk fix)

Rewrite `Store::transact`'s doc comment (~line 179-209) so the ACCURATE
default behavior is stated FIRST, not buried after a misleading opening
line. Suggested restructuring (adjust wording as needed, keep all the
existing accurate detail — this is a REORDER + REFRAME, not a content
cut):
- Lead with: "Apply a mixed-op batch sequentially. The DEFAULT
  implementation applies each op one at a time and provides **NO**
  cross-op atomicity — a crash or concurrent read mid-batch can observe a
  partially-applied state."
- THEN explain that backends with a native write-transaction API MAY
  override this to provide genuine atomicity, queryable via
  `supports_atomic_transact()`.
- Keep the rest of the existing detail (production caller list, F-77/F-85
  references) — just re-verify and update it per step 2 below.

### 2. Audit the doc's caller list against the ACTUAL current callers

The doc comment names exactly 4 production callers as self-healing via
"settle/re-scan": `SortedIndexManager::rekey_postings`, `apply_index_ops`,
`apply_index_ops_at_commit`, base_index `apply_ops`. A fresh grep this
session found MORE current callers of `.transact(` in production code —
verify the doc's list against reality and update it to be complete:
- `crates/shamir-index/src/base_index/index_manager.rs`
- `crates/shamir-index/src/base_index/sorted_index_manager.rs`
- `crates/shamir-index/src/vector/snapshot.rs`
- `crates/shamir-index/src/write_ops.rs`
- `crates/shamir-engine/src/tx/apply_replicated.rs`
- `crates/shamir-engine/src/tx/commit_phases.rs`

For EACH call site: read the surrounding code and confirm whether it
genuinely tolerates non-atomic `transact` (self-heals or is provably safe
under partial application), or whether it silently ASSUMES atomicity
without checking `supports_atomic_transact()` — if you find a genuine case
where correctness DEPENDS on atomicity but the flag is never checked, STOP
and report it as a real bug (do not fix it yourself — that's a distinct,
potentially P0-level finding deserving its own reviewed task, not a
doc-audit side-fix). Update the doc's caller list to accurately reflect
every current caller and its tolerance story.

### 3. Specifically re-verify #962 (sorted RENAME) per this task's own note

`SortedIndexManager::rekey_postings` (landed via #962 this session,
`crates/shamir-index/src/base_index/sorted_index_manager.rs`) was
specifically built with its OWN tombstone + idempotent-resume mechanism
(NOT relying on `transact`'s atomicity) — confirm this is still true by
reading `rekey_postings`'s current implementation and its doc comment.
Report your confirmation explicitly; do not modify #962's code unless you
find it genuinely does NOT tolerate non-atomic `transact` (in which case,
same rule as above: report, don't silently patch).

## Gate (MANDATORY — doc changes still need the full gate since this
touches a widely-implemented trait's doc, and the caller-list audit may
surface follow-up findings)

```
cargo fmt -p shamir-storage -- --check
cargo clippy -p shamir-storage --all-targets -- -D warnings
./scripts/test.sh -p shamir-storage
```
If any OTHER crate's file needs a doc/comment touch during the audit (e.g.
correcting a caller's own stale comment about its self-heal story), run the
same 3-command gate for that crate too.

## Scope discipline

- Do NOT implement the `apply_batch_ordered`/`atomic_transact` API split —
  see "Why out of scope" above.
- Do NOT modify any caller's actual LOGIC — this task is doc/comment
  accuracy only, unless you find a genuine correctness bug, in which case
  STOP and report it rather than fixing it inline.
- Do NOT touch `supports_atomic_transact()`'s behavior or default — it is
  already correct (F-85/#913).

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Do NOT run
`git commit` or `git add` — the orchestrator verifies your diff and the
test run, then commits. Only edit files and run read-only/test/gate
commands.

## What to report back

Show the exact before/after of `transact`'s doc comment. List every
current production caller checked, its self-heal tolerance story, and
whether the doc's caller list needed updating. Explicitly confirm #962's
`rekey_postings` still correctly tolerates non-atomic `transact`. Report
any genuine correctness bug found (do not fix it) as a clearly flagged
finding. Give exact gate command output.
