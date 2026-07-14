Task #541 — close the missing-owner-field escalation gap in
`effective_fn_actor`.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## The bug (confirmed by the orchestrator's own read — re-verify line
## numbers, code may have shifted; #540 just touched this same file)

`ShamirDb::effective_fn_actor` in
`crates/shamir-db/src/shamir_db/shamir_db/access_control.rs` (currently
~lines 537-560) is already fail-closed for a WHOLLY MISSING function
record:

```rust
pub async fn effective_fn_actor(&self, fn_name: &str, caller: &Actor) -> Actor {
    let Ok(Some(rec)) = self.system_store.load_function(fn_name).await else {
        return caller.clone();
    };
    let res_meta = ResourceMeta::from_record(&rec);
    let fn_meta = FunctionMeta::from_record(&rec);
    match fn_meta.security {
        Security::Definer => res_meta.owner,
        Security::Invoker => {
            if Mode::is_setuid(res_meta.mode) {
                res_meta.owner
            } else {
                caller.clone()
            }
        }
    }
}
```

But `ResourceMeta::from_record` (`crates/shamir-types/src/access.rs`,
currently ~lines 236-249) has its OWN, separate fallback for a record
that loads successfully but simply lacks an `owner` field:

```rust
let owner = rec
    .get("owner")
    .and_then(|v| v.as_u64())
    .map(Actor::from_owner_id)
    .unwrap_or(Actor::System);
```

**Impact**: a function whose catalogue record loads (so
`effective_fn_actor`'s outer `let Ok(Some(rec)) = ...` guard passes) but
whose record lacks the `owner` field entirely — a legacy record
predating the owner field, or a partially-written/corrupted record — and
is declared `Security::Definer` (or `Invoker` + the legacy setuid mode
bit) silently resolves `res_meta.owner` to `Actor::System`. Any
unprivileged caller invoking that function is escalated to full
`Actor::System` privilege inside the function body: read/write any
table, run DDL, create users — a complete admin bypass, triggered purely
by a data-completeness gap in one catalogue record, not by any
legitimate design intent.

## Read first

- `crates/shamir-db/src/shamir_db/shamir_db/access_control.rs`'s
  `effective_fn_actor` — read the full doc comment above it too (the
  "Fail-closed guarantee" section already documents the WHOLLY-MISSING-
  record case as closed; your fix extends this guarantee to the
  present-but-owner-absent case and that doc comment must be updated to
  describe the new, wider guarantee honestly, not just patched around).
- `crates/shamir-types/src/access.rs`'s `ResourceMeta::from_record` and
  `Actor::from_owner_id`/`to_owner_id`/`OWNER_SYSTEM` (owner id `0` is the
  System sentinel — `from_owner_id(0) == Actor::System`).
- `crates/shamir-engine/src/function/` — `FunctionMeta::from_record` and
  the `Security` enum, for context on the Definer/Invoker semantics this
  function already documents (do not touch that decision table's logic,
  only the owner-resolution input to it).

## The fix

`ResourceMeta::from_record`'s existing `unwrap_or(Actor::System)` default
is the CORRECT behavior for every OTHER caller of `from_record` (DDL
introspection, `access_tree`, `resource_meta` after #540's fix, etc. — a
brand-new or legacy catalogue object with no owner field is intentionally
"open, owned by System", matching the file's documented
backward-compatibility contract). **Do not change `from_record`'s default
for those callers.** The gap is specific to `effective_fn_actor`'s
privilege-ESCALATION use of `owner` — that one call site needs to
distinguish "owner field genuinely present and equals System" (legitimate
— an admin explicitly created this definer function to run as System)
from "owner field absent, defaulted to System by `from_record`" (must
NOT escalate).

Add a way to check whether the `owner` field is genuinely present on a
record, without changing `from_record`'s default-open contract for every
other caller. Candidate shape (adjust to fit the codebase's conventions,
investigate the cleanest seam):

```rust
impl ResourceMeta {
    /// Returns `Some(owner)` iff the record has an explicit `owner`
    /// field; `None` if the field is absent (distinct from a record
    /// whose owner field is explicitly `0`/System — that IS `Some(System)`).
    /// Used by callers (e.g. `effective_fn_actor`) that need to tell
    /// "explicitly owned by System" apart from "no owner recorded,
    /// defaulted by `from_record`" — a distinction `from_record` itself
    /// deliberately erases for its own (correct, unrelated) callers.
    pub fn owner_field(rec: &QueryValue) -> Option<Actor> {
        rec.get("owner").and_then(|v| v.as_u64()).map(Actor::from_owner_id)
    }
}
```

In `effective_fn_actor`, resolve the owner for escalation purposes via
this new method instead of `res_meta.owner` for the two escalation arms
(`Security::Definer`, and the setuid branch of `Security::Invoker`):

```rust
match fn_meta.security {
    Security::Definer => match ResourceMeta::owner_field(&rec) {
        Some(owner) => owner,
        None => caller.clone(), // fail-closed: no recorded owner, cannot escalate
    },
    Security::Invoker => {
        if Mode::is_setuid(res_meta.mode) {
            match ResourceMeta::owner_field(&rec) {
                Some(owner) => owner,
                None => caller.clone(),
            }
        } else {
            caller.clone()
        }
    }
}
```

(Or factor the `owner_field(&rec).unwrap_or_else(|| caller.clone())` /
fail-closed lookup into a small local helper if that reads cleaner — your
call, keep it simple.) Everywhere else `res_meta`/`from_record` is used in
this file (`resource_meta`, `access_tree`, etc.) is unaffected — this is a
surgical, single-call-site fix, not a `from_record` semantics change.

Update `effective_fn_actor`'s doc comment (the "Fail-closed guarantee"
section) to state the fuller guarantee: escalation happens ONLY when the
function record is loaded AND carries an explicit, present `owner` field
— never via `from_record`'s default-System fallback, whether that
fallback triggers because the record is absent OR because the record is
present but the field itself is missing.

## Test requirement

A regression test in `shamir-db`'s test suite: construct (or persist) a
function catalogue record that is `Security::Definer` (or `Invoker` +
setuid mode) but has NO `owner` field (or an owner field explicitly
absent — check how existing tests construct function records with
specific fields to reuse that seam rather than inventing a new one), call
`effective_fn_actor` with a non-System caller, and assert the returned
actor is the ORIGINAL CALLER, not `Actor::System`. Also add/confirm a
sibling test proving a function record that DOES have an explicit
`owner: 0` (System) field still legitimately escalates Definer callers to
System — this is the case your fix must NOT break (an admin-created
definer function that intentionally runs as System).

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
  > owner_field (or equivalent) added: exact shape, confirmed
    from_record's existing default-open contract is UNCHANGED for every
    other caller (list them)
  > effective_fn_actor: exact escalation-arm changes, confirmed the
    setuid-Invoker branch is also covered, not just Definer
  > Doc comment updated to state the fuller fail-closed guarantee
  > New regression test: confirmed RED before the fix, GREEN after
  > Sibling test proving explicit owner=System definer functions still
    escalate correctly (not broken by this fix)
[Verification]
  cargo check --workspace --all-targets: pass/fail
  ./scripts/test.sh -p shamir-db: pass/fail
```

Given this changes a privilege-escalation decision path used by every
WASM function invocation in the engine, this MUST go through an
adversarial review pass before committing — same discipline as
#534/#537/#538/#539/#540 this campaign. If that review finds a genuine
bug, the orchestrator fixes it directly (never re-delegates),
re-verifies, and sends the fix through a second review pass before
committing.
