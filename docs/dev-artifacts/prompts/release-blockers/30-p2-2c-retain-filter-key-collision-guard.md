# Brief — sub-bug 2c's key-length retain heuristic needs an explicit safety net

Task: #993 in the session TaskList. Found by the post-campaign `@oh`
review of #957-971 (2026-08-04/05). Read this brief in full — a scoping
decision has already been made (below); the task's own text suggested a
larger fix, this brief deliberately narrows it.

## The gap — confirmed by direct trace

`pre_commit.rs`'s sub-bug 2c retain filter
(`rederive_base_index_ops_post_stage`, ~line 1339-1360) identifies which
staged `IndexWriteOp`s belong to the base_index (regular/unique) family —
so it can retract ops for an index DROP'd between stage and commit — using
ONLY a physical-layout heuristic: key length exactly 41 or 25 bytes, plus
`key[0] <= 1`. Anything else is assumed to be sorted/index2 and left
untouched.

Verified TODAY this is safe for every EXISTING backend (re-derive this
yourself by reading each, don't just trust this brief's numbers — backend
code may have shifted):
- base_index regular: 41 bytes (intentional match — the filter's target).
- base_index unique: 25 bytes (intentional match).
- sorted: keys start with `SORTED_TAG = 0x80`, caught by the `> 1` guard
  regardless of length.
- FTS (`fts_backend.rs`) and FTS-ranked (`fts_ranked_backend.rs`): 29-byte
  posting keys (`descriptor.id.to_le_bytes()` 4-byte prefix + tokenized
  payload) — different length, safe.
- Functional (`functional_backend.rs`): 37-byte posting keys (same 4-byte
  id prefix + value payload) — different length, safe.
- Vector (`vector_backend.rs`): `plan_insert`/`plan_update`/etc. return
  `Ok(Vec::new())` — vector NEVER produces `IndexWriteOp::SetPosting`/
  `RemovePosting` at all (its state lives in an in-memory HNSW graph
  managed directly by the adapter, not through the posting-op pipeline).
  Not a candidate for this collision at all.

**The actual risk**: index2 posting keys are prefixed by a 4-byte `u32`
LE descriptor id (verified this session during #988's work). Byte[0] of
such a key is simply the LOW byte of that `u32` id — which absolutely CAN
be `0` or `1` for a low-numbered id (e.g. id=1 → LE bytes `[1,0,0,0,...]`).
So a HYPOTHETICAL future index2 backend whose posting-key format happens
to total exactly 41 or 25 bytes (regardless of its id) would have its ops
silently and incorrectly retracted by this filter whenever that backend's
assigned id has a low byte of 0 or 1 — a nondeterministic-looking data
bug (only misfires for SOME index ids, not others) with zero compile-time
or runtime signal.

## Why the full fix (an explicit family tag on `IndexWriteOp`) is OUT OF
## SCOPE here — orchestrator decision, do not second-guess

The task's own description suggests "replace the key-length/first-byte
heuristic with an explicit tag or discriminant on `IndexWriteOp`." **Do
not do this.** Verified this session: `IndexWriteOp::SetPosting`/
`RemovePosting` are constructed at 13+ call sites across 3 crates
(`shamir-tx`, `shamir-engine`, `shamir-index` — includes
`index_manager.rs`, `index_manager_unique.rs`, `sorted_index_manager.rs`,
`fts_backend.rs`, `fts_ranked_backend.rs`, `functional_backend.rs`,
`write_ops.rs`, `tx_context.rs`, `repo_tx_gate.rs`, `commit.rs`,
`table_manager_changefeed.rs`, plus benches). Adding a new field to the
enum variant would require touching every one of these — large, invasive,
and disproportionate for a review finding explicitly marked "non-blocking,
low priority, tech-debt... no live backend triggers it today."

**Scoped-down fix**: close the risk with a loud, unmissable FAILURE
SIGNAL for whoever adds a colliding backend in the future, rather than a
type-level redesign today:
1. Strengthen the existing doc comment above the retain filter into an
   explicit, impossible-to-miss "contract" for future index2-backend
   authors.
2. Add a regression test that locks in the CURRENT safe key-length/prefix
   facts for every existing backend by constructing a REAL op through
   each backend's REAL `plan_*` method (not hand-rolled bytes) and
   asserting each falls outside the 41/25-byte-and-`key[0]<=1` danger
   zone — so the test breaks (loudly, at CI time) the moment any EXISTING
   backend's key format ever changes to collide, and so a future PR adding
   a new backend has an obvious, nearby example test to extend.

This does not make the collision structurally impossible (a genuinely new
backend that nobody thinks to add a test for could still collide
silently) — but it is the right-sized fix for a "no live risk today, low
priority" finding, converting an invisible landmine into a documented,
tested, extend-me-when-you-add-a-backend contract.

## Required work

### 1. Strengthen the doc comment

Rewrite the comment block above the retain filter (`pre_commit.rs`
~line 1318-1330) to state explicitly, as its own clearly-marked paragraph:
"**Contract for future index2 backends**: any NEW index2 backend's
posting-key format MUST NOT produce a key of length exactly 41 or 25
bytes whose first byte is `0` or `1` — such a key would be silently
misidentified as a base_index op and retracted here. See
`<test file>::<test name>` for the regression test locking in the
current safe values for every existing backend; extend it when adding a
new backend." (Fill in the actual test file/name once you've written it
in step 2.)

### 2. Regression test

Find the right home — check `crates/shamir-engine/src/table/tests/` for
an existing file that already exercises sub-bug 2c specifically
(`p02_base_index_rederive_tests.rs` has
`p02_regular_drop_index_before_commit_no_orphan`/
`p02_unique_drop_index_before_commit_no_orphan`, which test the retain
filter's BEHAVIOR functionally but not this specific key-collision
concern) — decide whether to extend that file or add a small new one per
this repo's test-organization convention (one file per logically related
group; check `crates/shamir-engine/src/table/tests/mod.rs` for the
existing module list before creating a new file).

Write a test that, for each of the 5 backend kinds that actually produce
posting ops (base_index regular, base_index unique, sorted, FTS, FTS-
ranked, functional — 6 total, skip vector since it never produces
posting ops), constructs a REAL `IndexWriteOp` via that backend's real
`plan_insert`/`plan_record_created`/equivalent method (whatever the
correct entry point is for each — you may need one small helper per
family, mirroring how existing tests in this workspace set up each
backend type), extracts the resulting posting key, and asserts:
- base_index regular/unique keys DO match the 41/25-byte-and-`<=1`
  pattern (they're SUPPOSED to — that's the filter's actual target).
- sorted/FTS/FTS-ranked/functional keys do NOT match it.

If constructing all 6 through their real `plan_*` methods is
disproportionately heavy machinery for this one test, a lighter
acceptable alternative: a focused unit test that directly calls each
backend's own internal key-building function (if one is exposed/testable
in isolation) with a fixed id, asserting the resulting length/first-byte
against the retain filter's exact literal logic (`len() != 41 && len() !=
25` fails-safe; `key[0] > 1` fails-safe) — use your judgment on which
approach is less brittle and more maintainable, and justify the choice in
your report.

## Scope discipline

- Do NOT add a family/discriminant field to `IndexWriteOp` — explicitly
  out of scope per the orchestrator decision above.
- Do NOT change the retain filter's actual logic/behavior — this task is
  a safety-net (test + doc) addition only.
- Do NOT touch vector's posting-op behavior (it has none) or any other
  index family's logic.

## Gate (MANDATORY)

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine --full
```

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Do NOT run
`git commit` or `git add` — the orchestrator verifies your diff and the
test run, then commits. Only edit/create files and run read-only/test/gate
commands.

## What to report back

Show the strengthened doc comment (before/after). Show the new test in
full and explain which of the two test-construction approaches you chose
and why. Confirm you verified the test actually catches a REAL collision
(e.g. temporarily using a hand-constructed 25-byte "fake FTS key" with
first byte 0 to prove the test would fail if a backend really collided —
then remove that temporary probe). Give exact gate command output.
