# F-79 (#906) — remove remaining std::sync::Mutex from runtime paths or narrow the invariant

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Only edit files;
the orchestrator commits.

## Background

F-66 (#892, commit `829f1227`) replaced `TxContext::ri_barrier_tokens`'s
`std::sync::Mutex<TFxSet<u64>>` with a lock-free `scc::HashSet`, citing
"the commit path is a hot path, poisoning is unacceptable." The same
primitive remains at two more sites, each carrying the identical
poisoning exposure (a panic under the guard poisons the mutex — every
LATER `.lock().unwrap()` anywhere else in the process then panics too,
turning one bug into a permanent, unrelated cascade):

1. **`PredicateSet.inner: std::sync::Mutex<Vec<PredicateDep>>`**
   (`crates/shamir-tx/src/predicate_set.rs`, currently ~lines 44-100).
   Per-tx, doc-commented as "append-only during execution... the executor
   runs a tx's queries serially so contention is nil."
2. **`RepoTxGate.pending_commits: std::sync::Mutex<Vec<PendingCommit>>`**
   (`crates/shamir-tx/src/repo_tx_gate.rs`, currently ~lines 137-140).
   Shared ACROSS transactions — a group-commit queue: many concurrent
   committers push, one leader pops the whole vec under lock. Doc-
   commented as "short-section... only push/drain, no `.await` held
   across."

CLAUDE.md's NORMATIVE concurrency section bans `std::sync::Mutex` /
`parking_lot::*` on hot paths, permitting them ONLY as a low-frequency /
setup-only fallback with an inline comment naming the contention model.
Both sites already carry such a comment — but F-66's own precedent
argument ("the commit path is a hot path, poisoning is unacceptable")
applies verbatim to at least one of them, and `PredicateSet`'s own doc
comment is what F-66 originally cited as the justification for leaving A
mutex in place — i.e. the two sites currently cite EACH OTHER as
precedent for staying as-is, which is not the same as either site having
been independently justified.

## What you must actually determine, per site — read the real code, don't guess

**`PredicateSet`**: is "the executor runs a tx's queries serially" ACTUALLY
true for every code path that touches a `PredicateSet` instance, including
any background/cleanup task, or only for the common case? Trace every
caller of `push`/`len`/`is_empty`/`with_iter`/`snapshot_deps` (grep the
whole `shamir-engine`/`shamir-tx` call graph, not just the obvious ones)
and confirm whether TRUE single-threaded-per-instance access holds. If it
does, a `Mutex` is unnecessary overhead/poisoning-risk for something that
is never actually contended — either drop the lock entirely (if a
single-owner-with-interior-mutability shape like an unsynchronized cell
is provably sound given `TxContext`'s actual `Send`/ownership shape) or
replace with a lock-free append-only structure.

**`RepoTxGate::pending_commits`**: this one IS genuinely multi-writer
(concurrent committers push, a leader drains). This is the stronger
candidate for a REAL lock-free migration (pillar 5: `scc`, a lock-free
MPSC-style queue, or equivalent) — check what's already available in this
workspace's dependency tree (`scc = "3.8"` is already a `shamir-tx`
dependency; check whether `scc` offers a queue primitive suited to
push-many/drain-all, or whether a different, well-audited lock-free
queue crate is warranted — do not roll a hand-written lock-free structure
from scratch for this).

## Two acceptable outcomes — pick per site, write down the reasoning

1. **Migrate to a lock-free structure** (pillar 5), exactly as F-66 did,
   OR
2. **Formally narrow the invariant**: state in ONE authoritative place
   (e.g. a shared doc section, not two sites citing each other) which
   sites are sanctioned `std::sync::Mutex` exceptions and why, with the
   ACTUAL contention model named (not "contention is nil" as an
   assertion — show the trace that proves it), and make CLAUDE.md's
   normative rule and the code's doc comments agree — CLAUDE.md itself
   may need a cross-reference added so the exception is discoverable from
   the rule, not just from the code.

**Not acceptable**: leaving the current state where the normative rule
says one thing, two runtime sites do another, and the two sites cite each
other as their justification. Every site must end this task independently
justified — either migrated, or narrowed with real evidence.

## The `scc::*::len()` audit (mandatory, cheap)

Re-confirm F-66's `scc::HashSet` swap didn't introduce a banned
`scc::*::len()` call anywhere reachable (clippy.toml's
`disallowed-methods` bans it as O(N) — `clippy --workspace --all-targets
-D warnings` should already catch a live violation, but the task
explicitly asks for a fresh, deliberate grep as cheap insurance, since a
future `#[allow]` could mask a real one). Grep the whole workspace, not
just `shamir-tx`, for any `.len()`/`.is_empty()` call on a value whose
type is `scc::HashMap`/`scc::HashSet`/`scc::TreeIndex`, confirm each
resolves to either an O(1) `AtomicUsize` mirror (per CLAUDE.md's stated
pattern) or a documented, explicitly-annotated
`#[allow(clippy::disallowed_methods)] // O(N) ack: <why>` off-hot-path
use. Report what you found (even if "nothing new since F-66," say so
explicitly).

## Definition of done

- Each of the two sites has EITHER a lock-free replacement (with a
  red-then-green test proving behavior is unchanged — build the same
  scenario against the old Mutex-based code and the new lock-free code,
  assert identical outcomes) OR a documented, evidence-backed narrowing
  that makes CLAUDE.md and the code agree (with a test or an explicit
  trace-based argument, not just an assertion, that contention is
  genuinely nil for that site).
- The `scc::*::len()` audit's finding stated explicitly in the commit
  message.
- `cargo fmt -p shamir-tx -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `./scripts/test.sh -p shamir-tx -p shamir-engine --full` green.
- Behavior must be UNCHANGED for both sites — this is a concurrency-
  primitive swap, not a semantic change. If you migrate
  `RepoTxGate::pending_commits`, prove the group-commit leader-drains-all
  semantics still hold under concurrent pushes (a test with several
  concurrent pushers + one drainer, asserting no lost/duplicated entries).
- Do not touch F-66's already-shipped `ri_barrier_tokens` migration.
- Do not run this task concurrently with any other task touching
  `predicate_set.rs` or `repo_tx_gate.rs`.

Commit with `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>` per
repo convention.
