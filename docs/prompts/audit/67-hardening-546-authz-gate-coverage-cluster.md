Task #546 — a cluster of five LOW-severity hardening findings (no live
bypass found in any of them; each is a latent "silently forget the gate
on a new op" hazard) plus an integration coverage-matrix test.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Priority order (do (a) first — it's the structural centerpiece; the
## rest are independent and can be scoped down if time runs short — see
## the explicit permission section below)

### (a) Unify the duplicated `BatchOp → (Action, ResourcePath)` mapping

Confirmed byte-for-byte duplicated today (re-verify line numbers, code
may have shifted) in TWO real enforcement loops:

- `crates/shamir-db/src/shamir_db/execute/db_execute.rs` (~lines 52-70,
  inside `execute_as` — the non-tx wire path).
- `crates/shamir-db/src/shamir_db/execute/db_tx.rs` (~lines 139-160+,
  inside `tx_execute_as` — the interactive-tx path, which ALSO wraps the
  same match in a per-call ACL inline cache `FxHashMap<(ResourcePath,
  Action), bool>` — keep that caching wrapper where it is, only extract
  the match itself).

Both currently do:

```rust
let action = match &entry.op {
    BatchOp::Read(_) => Action::Read,
    BatchOp::Insert(_) => Action::Create,
    BatchOp::Set(_) | BatchOp::Update(_) => Action::Write,
    BatchOp::Delete(_) => Action::Delete,
    _ => Action::Write,
};
```

Note the existing code ALREADY has a wildcard (`_ => Action::Write`) —
part of this fix is turning that into a real exhaustive match so a new
`BatchOp` variant with a `table_ref()` forces the author to explicitly
decide its `Action`, not silently fall into `Write`.

**Fix**: add a method on `BatchOp` (in
`crates/shamir-query-types/src/batch/batch_op.rs`, right next to the
existing `is_write`/`table_ref` — follow `is_write`'s own "exhaustive
match, no wildcard, comment explaining why" convention exactly, it's in
the same file) with a signature like:

```rust
/// Returns the `(Action, ResourcePath)` this op must be authorized
/// against, for ops with a `table_ref()`. `None` for admin/DDL ops
/// (authorized separately in `execute_admin`) and read-only/
/// introspection ops with no target table.
pub fn required_access(&self, db: &str) -> Option<(Action, ResourcePath)> {
    let action = match self {
        BatchOp::Read(_) => Action::Read,
        BatchOp::Insert(_) => Action::Create,
        BatchOp::Set(_) | BatchOp::Update(_) => Action::Write,
        BatchOp::Delete(_) => Action::Delete,
        // ... every OTHER variant explicitly, no wildcard ...
    };
    let tref = self.table_ref()?;
    Some((action, ResourcePath::Table {
        db: db.to_string(),
        store: tref.repo.clone(),
        table: tref.table.clone(),
    }))
}
```

Investigate the exact shape needed: `is_write`'s exhaustive match already
enumerates every `BatchOp` variant and classifies read/write — reuse
that classification's structure so `required_access`'s per-variant
`Action` choices are consistent with `is_write`'s existing read/write
split (a variant `is_write` marks `false` should not suddenly require
`Action::Write` here). Check whether `Action` needs any NEW variant
beyond `Read`/`Create`/`Write`/`Delete` for any `BatchOp` case not
covered by the current 4-armed match (e.g. does `Set` really always mean
`Write`, or could an upsert-that-creates need `Create` semantics? — the
existing pre-fix code already treats `Set` as `Write` unconditionally,
so match that unless you find a concrete reason not to; if you do find
one, flag it in your report rather than silently changing established
behavior).

Update BOTH `db_execute.rs` and `db_tx.rs` to call `entry.op
.required_access(db_name)` instead of duplicating the match. Also check
whether `crates/shamir-engine/src/query/auth/session.rs`'s test-only
classifier (~lines 234-563, per the audit finding) should ALSO be
switched to call this new method (reducing it from a third independent
copy to a consumer of the single source of truth) — investigate whether
it's structurally able to (it may operate on a different type or in a
context without access to `ResourcePath`/`db_name` the same way) and
report your decision either way.

### (b) Doc-guard the transparent `authorize(&actor, ...)` trace calls

`crates/shamir-engine/src/query/query_runner.rs` has several calls
(~lines 320, 375, 468, 590, 706 — re-verify) to an `authorize(&actor,
path, action)` function that is a pure R2 observability trace (always
returns `Ok` — see `access.rs`'s `authorize` definition, ~line 567) —
NOT an enforcement gate. The REAL gate runs earlier, in
`execute_as`/`tx_execute_as` (the very functions touched by item (a)
above). Add a doc comment at each call site (or, if cleaner, rename the
function to something like `trace_access` at its definition and update
all call sites) making it unmistakable that this is observability, not
enforcement — so a future refactor doesn't see `authorize(...)` at these
five sites, assume it's the real gate, and remove the outer
`authorize_access` call thinking it's now redundant.

### (c) CREATE TOCTOU in `handle_create_db`/`handle_create_repo`

`crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs` (~lines
29-48, 123-172 — re-verify) does exists-check → authorize → create with
no atomicity between the authorize check and the create call. The audit
frames this as narrow (create already happens under an internal
per-instance lock, so ACL is still respected end-to-end) — it's an
idempotency/race-on-the-exists-check hazard, not a rights bypass.
**Fix**: hold the authorize→exists→create sequence under one lock (reuse
whatever lock `create_db_as`/`add_repo_as` already take internally for
the create step, if one exists — investigate before adding a NEW lock),
or re-run the exists-check INSIDE `create_db_as`/`add_repo_as` under
that same lock so the whole sequence is atomic relative to a concurrent
duplicate-create racing in.

### (d) Encapsulate the `Manage(Root)` gate inside group-CRUD/user-lifecycle

`crates/shamir-db/src/shamir_db/shamir_db/access_control.rs`'s
`create_group`/`add_group_member`/etc. (~lines 201-287 — re-verify) do
NO authorization check themselves — safety today relies ENTIRELY on
every dispatcher handler (`admin_access.rs`'s `handle_create_group` etc.)
pre-calling `authorize_access(&actor, &ResourcePath::Root,
Action::Manage)` before reaching these methods. **Fix**: investigate the
cleanest way to make this structurally enforced rather than
convention-enforced — options to weigh: (i) move the
`authorize_access(Root, Manage)` check INSIDE each of these
`ShamirDb` methods directly (duplicates the check if a caller already
did it, but makes each method self-defending); (ii) introduce a
lightweight "already-authorized" marker type (e.g. a private
zero-sized-type token only `authorize_access` can construct, threaded as
an extra parameter) so the compiler enforces that a caller went through
authorization first. Pick whichever is a better fit for this codebase's
existing patterns (check whether a similar "proof of authorization"
idiom already exists anywhere) — a full typestate redesign across every
admin method is likely too large for this task's LOW-severity scope;
prefer option (i) (redundant inline check) unless option (ii) is
genuinely cheap here. Whichever you choose, verify it doesn't
double-deny a legitimate System actor (the checks must compose, not
conflict).

### (e) Mark System-wrapper convenience fns as non-wire-reachable

`create_db`/`add_repo`/`rename_table` (the versions WITHOUT `_as` — in
`db_management.rs`/`table_management.rs`/`system_store.rs`) currently
have no marker distinguishing them from their `_as`-suffixed,
actor-aware siblings. No wire-reachable path calls them today (the wire
always goes through `execute_as(real_actor, ...)`), but nothing stops a
FUTURE wire handler from accidentally calling the System-wrapper
directly, silently bypassing all ACL. **Fix**: investigate the cheapest
structural guard — `pub(crate)` visibility (if nothing outside the crate
legitimately needs the System-wrapper — check callers first, tests may
use it), a `#[doc(hidden)]` + a loud doc-comment warning, or (if
`pub(crate)` would break existing external callers) at minimum a
prominent `// SAFETY:`-style doc comment on each function stating "never
call this from a wire-reachable path; use the `_as` variant with the
real actor". Grep every call site of each bare (non-`_as`) function
first so you don't break a legitimate internal caller.

## Coverage matrix (gate-coverage rec.2)

Add an integration test that, for each (object × op) pair in
`docs/roadmap/ACCESS_HIERARCHY.md`, drives the REAL enforcement path
(`execute_as`/`tx_execute_as`, and the WASM `db_execute` gateway if it
has its own independent gate — check) with a no-rights `Actor::User`
(no grants, no ownership, default-open-nothing setup) and asserts
`access_denied` for every one. Investigate the cleanest way to drive
this as data (a table of `(ResourcePath variant, Action)` pairs derived
from — or cross-checked against — `ACCESS_HIERARCHY.md`'s own listed
matrix) rather than hand-writing one test function per cell, so the
matrix itself stays the single source of truth for what's covered.

## Explicit permission to scope down

This is a 5-sub-item cluster plus a coverage matrix — genuinely large
for a single LOW-severity task. All items are independent of each other
(item (a)'s refactor doesn't depend on (b)-(e) or vice versa). If you
run out of safe runway partway through:
- (a) is the highest-value item (turns a silent-miss hazard into a
  compile error) — land it even if nothing else fits.
- (b) is nearly free (doc comments / a rename) — land it alongside (a).
- (c)/(d)/(e) are each standalone; land whichever you have confidence
  in, and for whichever you DON'T reach, leave a properly-scoped
  follow-up task description in your final report (same rigor as this
  brief) rather than half-implementing any one of them.
- The coverage matrix is valuable but the largest single item — if
  everything else is done and this doesn't fit, it's the correct thing
  to defer to a follow-up.

## Test requirement

- (a): a test proving a hypothetical new `BatchOp` variant would need an
  explicit match arm (this is really a "the match compiles today and is
  exhaustive" property — if Rust's own exhaustiveness check is the
  actual enforcement mechanism, a dedicated runtime test may not be
  meaningful; note in your report if you conclude the compiler check
  itself IS the test). Do add a behavioral test confirming
  `required_access` produces the SAME `(Action, ResourcePath)` the old
  duplicated code did for a representative sample of ops (regression,
  not just "compiles").
- (c): a test proving two concurrent `create_db`/`add_repo` calls for
  the same name don't produce an inconsistent/duplicate state (adjust to
  whatever the actual race window allows exercising deterministically —
  investigate the existing hook/pause patterns used elsewhere in this
  campaign, e.g. #534/#538's `BackfillPauseHook`-style seams, if a
  deterministic reproduction needs one).
- (d): a test proving the Manage(Root) gate still holds after your
  chosen fix (a non-System, non-Manage actor calling the now-guarded
  method directly, bypassing the dispatcher, is still denied).
- Coverage matrix: as described above.

## Test scope

```
./scripts/test.sh -p shamir-db -p shamir-query-types -p shamir-engine
```

## Verification (lighter per-task gate, agreed for this campaign)

```
cargo check --workspace --all-targets
./scripts/test.sh -p shamir-db -p shamir-query-types -p shamir-engine
```
Full fmt/clippy/test --full is FINAL-GATE's (#529's) job. This task does
NOT block FINAL-GATE (LOW hardening, no live bypass) — do not add it to
#529's blockedBy.

## Report format

```
[Implementation] Status: fixed / scoped-down-with-followup
  > (a) required_access: exact shape, both real loops + the test-only
    classifier updated (or explicitly not, with reasoning)
  > (b) doc-guard / rename: which approach, all 5 call sites covered
  > (c) TOCTOU: exact locking mechanism chosen
  > (d) Manage(Root) encapsulation: which option chosen (inline check vs
    typestate), reasoning
  > (e) System-wrapper marking: which mechanism, confirmed no legitimate
    caller broken
  > Coverage matrix: how driven (data table vs hand-written), how many
    (object × op) cells covered
  > Items NOT completed (if scoped down): list + follow-up task
    description filed with the same rigor as this brief
  > New tests: confirmed RED before / GREEN after for each item actually
    implemented
[Verification]
  cargo check --workspace --all-targets: pass/fail
  ./scripts/test.sh -p shamir-db -p shamir-query-types -p shamir-engine: pass/fail
```

Given this touches the core per-op authorization dispatch used by every
wire request (item a) and the group/root Manage gate (item d), this
MUST go through an adversarial review pass before committing — same
discipline as the rest of this campaign. If that review finds a genuine
bug, the orchestrator fixes it directly (never re-delegates),
re-verifies, and sends the fix through a second review pass before
committing.
