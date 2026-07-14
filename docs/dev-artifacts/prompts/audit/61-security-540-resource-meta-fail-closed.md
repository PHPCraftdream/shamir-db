Task #540 — make the `resource_meta` catalogue resolver fail-closed on a
real storage error instead of silently collapsing it into "record absent"
→ `ResourceMeta::default()` == `open()` (owner=System, mode 0o777).

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## The bug (confirmed by the orchestrator's own read — re-verify line
## numbers, code may have shifted)

`crates/shamir-db/src/shamir_db/shamir_db/access_control.rs`'s
`resource_meta` (currently ~lines 30-100) resolves a `ResourcePath` to its
`ResourceMeta`. Every mode-bearing branch does the same collapse:

```rust
ResourcePath::Database { db } => {
    let rec = self.system_store.load_database(db).await;
    rec.ok()
        .flatten()
        .map(|r| ResourceMeta::from_record(&r))
        .unwrap_or_default()
}
```

`rec.ok()` turns `Err(_)` (real storage I/O error, catalogue-page
corruption, deserialization failure — a `DbResult<Option<QueryValue>>`'s
error variant) into `None`, indistinguishable from a legitimate "record
genuinely absent". Both then fall through to `ResourceMeta::default()`,
which is `open()`: owner=System, mode 0o777. The same shape repeats for
`Store` (~51-57), `Table` (~58-64), `Function` (~65-71), `FunctionFolder`
(~72-82), and `FunctionNamespace` (~83-92, via `load_setting`).

**Impact**: a transient/structural catalogue-read failure on ANY of these
resources becomes a full auth bypass. A private table (owner=victim,
mode 0o700) whose meta record fails to read (disk hiccup, corrupted
catalogue page, deserialization bug) silently becomes world-readable/
writable to any `Actor::User` via Other-Read on the fallback 0o777 —
same for ancestor traversal `Execute` checks. This is fail-OPEN, the
opposite of the intended security posture.

**One documented, correct exception**: `FunctionFolder`'s fallback
(~lines 73-75) exists on purpose — a function whose name contains `/`
implicitly creates intermediate folders that were never explicitly
`CREATE`d, and those must still open (see the `#118` reference in the
existing comment). This is a real `Ok(None)` case (record genuinely never
created), not an error case — your fix must NOT break this path.

## Read first

- `crates/shamir-db/src/shamir_db/shamir_db/access_control.rs` — the full
  `resource_meta` resolver, `authorize_access` (the enforcement gate that
  calls it — traversal loop + target check), `access_tree` (introspection,
  calls `resource_meta` ~6 times for tree assembly).
- `crates/shamir-types/src/access.rs` — `AccessError` (fields: `actor`,
  `path: String`, `action` — no message field, kept small deliberately for
  the hot `Result<_, AccessError>` path; `clippy::result_large_err`), and
  `ResourceMeta`/`ResourcePath`/`permits`/`authorize`.
- Every external call site of `resource_meta` (grep `\.resource_meta\(` —
  confirmed non-test call sites at the time of this brief):
  - `crates/shamir-db/src/shamir_db/execute/admin_describe.rs` (~line
    169-176, inside a `DbResult`-returning describe-table handler, already
    uses `.map_err(|e| err(e.to_string()))?` on a sibling call just above —
    same pattern applies here).
  - `crates/shamir-db/src/shamir_db/execute/admin_access.rs` (three call
    sites, ~lines 38/75/112 — chmod/chown/chgrp handlers, all already
    inside fallible `DbResult`-returning functions with `.map_err(err_access)?`
    used on the line just above each call).
  - `crates/shamir-db/src/shamir_db/shamir_db/access_control.rs` itself:
    `authorize_access`'s traversal loop and target check (~369, ~381), and
    `access_tree`'s ~6 call sites (all already inside `DbResult<QueryValue>`
    — trivial `?` additions).
  - Several test files under `crates/shamir-db/tests/` and
    `crates/shamir-db/src/shamir_db/tests/access_meta_tests.rs` — these
    will need their call sites updated to match the new signature (likely
    `.await.unwrap()` or similar); this is expected and fine, not a scope
    violation.

## The fix

Change `resource_meta`'s signature from `-> ResourceMeta` to
`-> DbResult<ResourceMeta>`. In each mode-bearing branch, replace the
`rec.ok().flatten()...unwrap_or_default()` collapse with a match that
distinguishes the three real cases:

```rust
ResourcePath::Database { db } => match self.system_store.load_database(db).await {
    Ok(Some(r)) => Ok(ResourceMeta::from_record(&r)),
    Ok(None) => Ok(ResourceMeta::default()), // genuinely absent — still open by design
    Err(e) => {
        log::warn!("resource_meta: failed to load database '{}' meta: {e}", db);
        Err(e)
    }
}
```

Apply the same shape to `Store`, `Table`, `Function`, and
`FunctionNamespace` (the `load_setting` branch — same `Err` → log +
propagate). For `FunctionFolder`, preserve the existing intentional
`Ok(None)` → `open()` fallback (this is NOT an error case — only the
`Err(_)` arm changes to log + propagate; do not touch the `Ok(None)`
arm's behavior). `Root`/`User`/`Group`/`Record`/`Index` branches return
`Ok(ResourceMeta::open())` / recurse into the `Table` branch's `Ok(...)`
unchanged (no I/O there, nothing to fail).

**`authorize_access`** (the enforcement gate) must treat a `resource_meta`
`Err` as an immediate, unconditional DENY — never proceed to the
`permits` check on an error. Synthesize an `AccessError` with the same
shape as a normal denial (actor, path rendered to `String`, the action
that was being checked when the load failed — `Action::Execute` for a
traversal-ancestor failure, the requested `action` for the target-check
failure) so the error type stays `Result<(), AccessError>` (do not widen
this hot-path return type). Log the real underlying storage error via
`log::warn!` at the point of failure (include the actor/path/action for
correlatability) before returning the synthesized `AccessError` — the
real error detail is for the log, not for the caller-visible
`AccessError`.

**`access_tree`** (already `DbResult<QueryValue>`) and
**`admin_describe.rs`**/**`admin_access.rs`**'s call sites: propagate
via `?` (or `.map_err(...)` matching each site's existing error-mapping
idiom, e.g. `admin_access.rs`'s `.map_err(err_access)` pattern used on
the authorize call immediately above each `resource_meta` call — check
whether that same `err_access` helper is reusable here or whether a
`DbError` needs no further mapping since these functions already return
`DbResult`).

**Do not change**: `ResourceMeta::default()`'s own definition, the
`Ok(None)` semantics for any resource, `effective_fn_actor` (a separate,
already-fail-closed function — task #541 is its own follow-up, out of
scope here), or `set_resource_meta` (already correctly fallible).

## Test requirement

Add a fault-injecting regression test in `shamir-db`'s test suite proving:
1. When the underlying `system_store.load_*` call returns `Err`, `resource_meta`
   returns `Err` (not a default-open `ResourceMeta`), AND `authorize_access`
   denies (not grants) an actor that would have been permitted under the
   old fail-open default.
2. The `Ok(None)` implicit-FunctionFolder-open path (#118) still opens —
   a regression test proving this is NOT broken by your change (a
   never-created folder in a slash-named function's path must still
   permit traversal).

Check how existing tests fault-inject storage errors elsewhere in this
codebase (e.g. `shamir-storage`'s test fixtures, or a `system_store` test
double/mock in `shamir-db`'s own test harness) rather than inventing a new
mechanism — reuse whatever seam already exists for forcing a `load_*`
call to return `Err`. If no such seam exists for `system_store` in
`shamir-db`'s tests, investigate the smallest addition that makes one
possible (e.g. a wrapping store that can be told to fail the next N
calls) rather than a large test-infrastructure rewrite.

## Test scope

```
./scripts/test.sh -p shamir-db
```

## Verification (lighter per-task gate, agreed for this campaign)

```
cargo check --workspace --all-targets
./scripts/test.sh -p shamir-db
```
Full fmt/clippy/test --full is FINAL-GATE's (#529's) job.

## Report format

```
[Implementation] Status: fixed / scoped-down-with-followup
  > resource_meta: new signature, exact branches changed, confirmed the
    FunctionFolder Ok(None)-open path is untouched
  > authorize_access: exact denial-synthesis on Err, confirmed no widening
    of the AccessError type, confirmed log::warn! includes actor/path/action
  > All non-test call sites updated: admin_describe.rs, admin_access.rs
    (x3), access_tree (x~6) — list each one and how it now propagates
  > New fault-injection test: confirmed RED before the fix (old code
    fail-open on injected error), GREEN after
  > #118 implicit-folder regression test: still passes
[Verification]
  cargo check --workspace --all-targets: pass/fail
  ./scripts/test.sh -p shamir-db: pass/fail
```

Given this changes a public method's signature on the core
`ShamirDb::resource_meta` authorization-resolution path used by every
`authorize_access` call in the whole engine, this MUST go through an
adversarial review pass before committing — same discipline as #534/#537/
#538/#539 this campaign. If that review finds a genuine bug, the
orchestrator fixes it directly (never re-delegates), re-verifies, and
sends the fix through a second review pass before committing.
