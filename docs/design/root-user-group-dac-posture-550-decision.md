בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Task #550: Root/User/Group always-open meta + wire-admin DAC posture — design decision

Three independent, LOW-severity design-clarification questions from
model-core F5, admin-ddl #6, gate-coverage #2, wasm-functions #3.
Investigated per this campaign's established pattern. The orchestrator's
first-pass proposals for all three were put to the user; the user
rejected §1's "leave as open(), just document it" as too shallow
("у этого должны быть отдельные права" — this needs real, separate
permissions) and asked for further independent design consultation
(`fl`/`fm` sub-agents) on all three before anything is finalized. This
revision folds in that consultation. **None of the three has final
user sign-off yet — that is still pending; this document records the
now-vetted proposals, not a decision to implement.**

## 1. Root/User/Group real permission model (revised per user's rejection of "just document open()")

**Confirmed current code**
(`crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:132-134`):
```rust
ResourcePath::Root | ResourcePath::User { .. } | ResourcePath::Group { .. } => {
    Ok(ResourceMeta::open())
}
```
Never catalogue-resolved; `set_resource_meta` explicitly rejects all
three (`access_control.rs:225-231`). `ResourcePath::User`/`Group` are
never used as real enforcement targets today — all user/group admin
ops gate on the blanket `Manage(Root)` instead. Note: `User`/`Group`
carry a `name: String`, not a stable id — see the #548/#549 dependency
note at the end of this section.

**Consultation verdict (agent `fl`) — one distinct model per kind, not
a single blanket fix:**

- **Root: full persisted meta, default `0o755`, owner `System`.**
  Mirrors the EXISTING `FunctionNamespace` precedent (a singleton whose
  meta already lives in the `settings` table, `access_control.rs:122-131`)
  — no new mechanism, just applying the pattern already used elsewhere
  in this same function to Root too:
  ```rust
  ResourcePath::Root => match self.system_store.load_setting("root_meta").await {
      Ok(Some(v)) => Ok(ResourceMeta::from_record(&v)),
      Ok(None) => Ok(ResourceMeta { owner: Actor::System, group: None, mode: 0o755 }),
      Err(e) => Err(e), // fail-closed, same as every other arm
  }
  ```
  Key: `settings["root_meta"]`. No migration — an absent key is the
  default, and `0o755` changes nothing observable today (Execute/Read
  for traversal+listing stay open; only "write to root", i.e. creating
  a top-level database, narrows from other-writable to owner-only,
  which matches the coarse wire-gate's existing intent). `set_resource_meta`
  gains a matching `Root` arm (mirrors the `FunctionNamespace` write
  arm at `access_control.rs:213-224`) — `chown /` becomes meaningful:
  it delegates `Manage(Root)` (group administration, per §2/§3's gates)
  to a named non-System principal, which is impossible today. Guardrail:
  reject a `chmod` that clears owner-Execute on Root when owner ≠
  System (an unrecoverable self-lockout; System-owned Root always
  recovers since System bypasses `permits` entirely).

- **User objects: a FIXED, computed 3-tier rule — not persisted meta.**
  ```rust
  ResourcePath::User { name } => Ok(ResourceMeta {
      owner: Actor::User(principal_id(name)), // → principal64 after #548
      group: None,
      mode: 0o750, // owner (self) rwx; others: nothing
  })
  ```
  Never stored. Effect via the existing `permits()` machinery: System —
  full; the user themselves — Read (see own metadata) + owner-`Manage`
  covering self-service (note: password changes already have their own
  dedicated flow, task #547 — this is not a duplicate mechanism, just
  the general self-`Manage` umbrella `changePassword` could route
  through instead of being a special case); everyone else — denied
  Read/List (user enumeration stops being free the moment any caller
  targets `User` paths). This is explicitly NOT "`open()` with extra
  steps" — `open()` passes every actor; this passes only self and
  System, a real behavioral narrowing the instant it's wired to
  anything. Not persisted because users have no natural
  owner-other-than-self and no group story until #548/#549's identity
  work lands (persisting a mode here now would create a second
  identity-adjacent store colliding with `FjallUserDirectory` becoming
  canonical, per that decision doc's §3).

- **Groups: persisted `owner` on the existing group record, computed
  mode.** Groups DO have a natural owner (creator) and a natural
  "group" (themselves):
  ```rust
  ResourcePath::Group { name } => {
      let gid = self.resolve_group_id(...)?;
      let rec = self.system_store.load_group(gid).await?...;
      Ok(ResourceMeta {
          owner: ResourceMeta::owner_field(&rec).unwrap_or(Actor::System),
          group: Some(gid), // members are the group's own "group" class
          mode: 0o750, // owner rwx-Manage; members read (roster); others nothing
      })
  }
  ```
  Storage: add an `owner` field to the group record `save_group`
  writes (`system_store.rs:630`), stamped from the acting actor in
  `create_group_as` (`access_control.rs:269`). Legacy records lacking
  the field default to `System` (fail-safe: only superuser manages
  pre-existing groups). Effect: `add/remove_group_member_as`,
  `rename_group_as`, `drop_group_as` change their gate from blanket
  `Manage(Root)` to `Manage(Root) OR Manage(Group{...})` — a group's
  creator manages their own group without needing global root admin.
  `create_group_as` itself keeps `Manage(Root)` (creation writes into
  the Root container). Mode stays fixed/computed (not settable) —
  no demonstrated need for per-group `chmod`; a future `chown group://x`
  (ownership transfer) is a trivial follow-on if ever needed.

**Dependency note**: the User-arm's `owner` computation uses
`principal_id(name)` today; once #548/#549 land, this becomes a
one-line swap to `principal64`. The Root/Group parts are independent of
#548/#549 and can land on their own schedule.

## 2. Wire-admin DAC posture — REVISED: the original blanket-relaxation proposal is unsafe as drafted

**Confirmed current code** — `crates/shamir-server/src/db_handler/handler.rs:396-406`
and `tx_handlers.rs:102-112`, identical shape (both gates, confirmed
identical, no divergent behavior to reconcile):
```rust
if !session.permissions.is_superuser {
    for (alias, entry) in &batch.queries {
        if entry.op.is_admin() {
            return DbResponse::Error {
                code: "permission_denied".into(),
                message: format!("query '{}' requires superuser (admin/auth op)", alias),
            };
        }
    }
}
```

**The orchestrator's ORIGINAL proposal — gate on `is_admin() &&
is_write()` instead of `is_admin()` alone — is REJECTED after
independent consultation (agent `fm`) found it unsafe:**

1. **A real ACL bypass via nested `Batch`.** `is_write()` is false for
   `BatchOp::Batch` too (its "writeness" is defined recursively over
   its children, `batch_op.rs:775`), and `required_access(Batch)`
   returns `None` (`batch_op.rs:543`) — so `execute_as`'s per-op
   authorization loop never inspects a sub-batch's nested queries at
   all (`db_execute.rs:58`; the engine's own recursion,
   `query_runner.rs:108-180`, only emits no-op `trace_access` for DML).
   Today `is_admin(Batch) == true` closes this by blocking ALL
   non-superuser sub-batches outright. Under the blanket
   `is_write()`-based relaxation, `Batch{ r: Read(forbidden_table) }`
   would pass the coarse gate (Batch is "not write") and its nested
   Read would execute with **zero per-table authorization** — reopening
   exactly the class of bug task #510 closed for `Subscribe`.
2. **Silent scope creep to 8 more ops.** `is_write()` is ALSO false for
   `GetBufferConfig`, `MigrationStatus`, `InternerDump`, `ChangesSince`,
   `ListValidators`, `ListPublications`, `ListSubscriptions`,
   `ReplicationStatus` — none of which the audit or the original
   proposal considered. Two are concretely dangerous to open blindly:
   `ListPublications`/`ListSubscriptions`/`ReplicationStatus` are
   gated only by Root `List` (open to any authenticated user per §1),
   exposing replication topology to everyone; `InternerDump` is gated
   by store `Read` (default-open), which would leak every interned
   field name in a repo — including field names from tables the caller
   has no rights to.
3. **`AccessTree`'s promised "scoped tree" doesn't actually happen.**
   Traced: `AccessTree` gates on `Manage(Root)` (`admin_access.rs:447`),
   which `permits()` grants owner-or-System only — a non-superuser is
   `Actor::User`, never Root's owner, so `AccessTree` stays **denied**
   for every non-superuser regardless of this fix. The original
   proposal's framing ("AccessTree scoped to what they can see") was
   wrong; the corrected test expectation is `access_denied`, not a
   filtered tree.
4. **`List` is safe as-is**: traced every `List` arm —
   `Users`/`Roles` require Root `Manage` (denied for non-superusers,
   same reasoning as AccessTree); `Databases` is Root `List`→Read on
   the already-open Root mode (deliberate per §1). No leak.
5. **`DescribeTable`/`GetTableSchema` are safe as-is**: both gate on
   `Action::Read` on the specific target table — exposes nothing
   beyond what a data `Read` on that table already permits.
6. **`tx_handlers.rs` needs the identical fix** — confirmed no
   meaningful divergence from `handler.rs`'s gate; factor the predicate
   into one shared helper so the two copies cannot drift apart (the
   same "duplicated enforcement logic" hazard task #546 already fixed
   for the DML per-op mapping applies here too).

**Revised decision: an EXPLICIT per-op allowlist, not a classifier-based
rule:**
```rust
let exempt = matches!(entry.op,
    BatchOp::List(_) | BatchOp::AccessTree(_)
    | BatchOp::DescribeTable(_) | BatchOp::GetTableSchema(_));
if entry.op.is_admin() && !exempt { /* deny, same as today */ }
```
Exactly the 4 ops the audit named, explicitly enumerated — NOT derived
from `is_write()` (which silently includes `Batch` and 7 other ops
never intended to be touched here). Extending the exemption to any of
the other 8 read-only-by-`is_write()` ops is a SEPARATE, deliberate
decision each, to be made individually if a real need arises — never
via a blanket classifier again. `Batch` is never exempted until
`required_access`/the per-op authorization loop is taught to recurse
into `SubBatchOp` (a separate, not-yet-scoped piece of work — flagged
as a prerequisite if `Batch` nesting of admin ops is ever wanted, not
part of this decision).

**Required before this can land** (revised): a coverage-matrix-style
test (mirroring #546's precedent) proving: (a) a non-superuser CAN
`DescribeTable`/`GetTableSchema` a table they own/have `Read` on; (b)
a non-superuser CANNOT `DescribeTable`/`GetTableSchema` a table they
have no rights to; (c) `AccessTree`/`Users`-and-`Roles`-`List` stay
denied for non-superusers (confirming the "scoped tree" framing was
corrected, not silently reintroduced); (d) `Databases`-`List` is
allowed (per §1's Root posture); (e) **a nested `Batch` containing a
forbidden-table `Read` is still denied** — this is the regression test
for the bypass this consultation found; (f) every non-exempted
`is_admin()` op, including all 8 ops this consultation flagged as
silently swept in by the rejected classifier-based approach, remains
superuser-only.

## 3. `CreateFunctionOp` wire fields — REVISED: per-field gating, not one blanket "Manage" check

**Confirmed current code** — `crates/shamir-query-types/src/admin/types/function_ops.rs:17-25`:
```rust
pub struct CreateFunctionOp {
    pub create_function: String,
    pub source: Option<String>,
    pub wasm: Option<String>,
    pub replace: bool,
}
```
No `security`, `secret_grants`, or `visibility` field. Wire-created
functions always get `Security::Invoker`, empty `secret_grants`,
default visibility. The in-process API (`create_function_with_opts`/
`CreateFunctionOptions`) already supports all three — wire-reachability
gap only, confirmed safe as shipped (every default is the
least-privileged option).

**The orchestrator's original proposal — gate `Definer`/non-empty
`secret_grants` at CREATE the same way a hypothetical future `chmod`-
style ALTER would — is refined after independent consultation (agent
`fm`)**: there is no `alter_function` op to mirror, and treating the
three new fields as one bundle is wrong — they need three DIFFERENT
answers, reasoned from first principles about what each field actually
does, not from op-symmetry:

- **`visibility` — no extra gate.** Private is the default; setting
  Public on your own newly-created resource is harmless (matches
  today's `chmod`-to-Public path, which needs only ordinary owner+Manage,
  already implied by CREATE itself).
- **`security: Definer` — gate with the EXISTING CREATE authorization
  check PLUS the destructive-op HMAC confirmation (task #542/#551's
  mechanism), not an extra `Manage` check.** Traced: a fresh function is
  always `owned_enforced(actor)` at save time
  (`function_management.rs:219`) — the creator IS the owner by
  construction, and even `replace=true` re-stamps ownership to the
  replacer, so Definer-on-your-own-brand-new-function is never a
  self-escalation (the escalation ceiling is already the creator's own
  rights; you cannot hijack another owner's identity this way). The
  real danger is identical in kind to the POSIX setuid bit this
  codebase ALREADY requires HMAC confirmation for on `chmod`
  (`effective_fn_actor`'s Definer/setuid escalation logic,
  `access_control.rs:655-682`): Definer is a STANDING escalation vector
  for every FUTURE caller of this function, regardless of who created
  it. The correct parity is therefore with `chmod`'s setuid HMAC gate,
  not with an extra authorization check — extend the HMAC canonical-input
  table (`hmac.rs`) with a `create_function` entry that includes the
  `security` field's value (e.g. `b"create_function\0<name>\0definer"`),
  so setting Definer at creation requires the SAME "did you mean it"
  confirmation setting it via chmod-after-the-fact would.
- **`secret_grants` (non-empty) — gate with `Action::Manage` on
  `ResourcePath::Root`, PLUS the same HMAC confirmation.** Traced:
  unlike Definer, this is NOT bounded by the creator's own rights —
  `secret_grants` names OS-seeded process environment variables
  (`GlobalVars::seed_env`), a resource class the creator has NO defined
  rights over at all. There is no existing concept anywhere of "which
  secrets this actor is allowed to grant" (confirmed: none found —
  inventing a real per-secret ACL is out of scope here, and is a
  natural fit for the identity/privilege work already queued in
  #548/#549/this same #550 doc's §1, not something to improvise ad hoc).
  Without a gate, any actor holding bare Create-on-`FunctionNamespace`
  could request `secret_grants: ["ADMIN_DB_PASSWORD"]` on their own new
  function and exfiltrate host secrets by simply calling it. Until a
  real secrets-ACL exists, the correct, honest gate is admin-only
  (`Manage(Root)`) + HMAC — narrower than ideal, but not inventing a
  false sense of granularity that doesn't exist yet.

**Framing correction**: "gate CREATE exactly like a future ALTER would"
imports a phantom op and was the wrong frame. The right rule gates by
WHAT EACH FIELD DOES, not by op symmetry — and when an `alter_function`
op is eventually designed, it inherits these same three per-field rules
for free, rather than needing its own separate reasoning pass.

**Scope note (unchanged)**: no setuid-equivalent bit is being added for
functions — `Security::Definer` already IS the modern replacement for
that legacy concept (per `effective_fn_actor`'s own doc comment). Only
`security`/`secret_grants`/`visibility` are added to the wire op.

## Test scope (once implemented)

```
./scripts/test.sh -p shamir-db -p shamir-query-types -p shamir-server
```

## Implementation notes

Per the established prompt-first pipeline, each of the three
(now independently re-vetted) sub-decisions becomes its own
`docs/prompts/audit/<NN>-*.md` brief once the user gives final
sign-off — they are independent enough to land as three separate
commits/reviews rather than one combined pass, given how much each
one's shape changed under scrutiny. None of this blocks FINAL-GATE.

## Status: sign-off still pending on all three (revised proposals below)

1. **§1 (Root/User/Group real permissions)**: Root — settings-backed
   persisted meta (default `0o755`/System), mirroring the existing
   `FunctionNamespace` pattern. User — fixed computed 3-tier rule
   (System/self/nobody), never persisted. Group — persisted `owner`
   field added to the existing group record, computed `0o750` mode,
   member-visible roster. Await sign-off on this REVISED (not the
   original "just document open()") proposal.
2. **§2 (wire-admin DAC posture)**: an EXPLICIT 4-op allowlist
   (`List`/`AccessTree`/`DescribeTable`/`GetTableSchema`), NOT the
   originally-proposed `is_write()`-based blanket rule (which
   consultation found reopens a real nested-`Batch` ACL bypass and
   silently sweeps in 8 unreviewed ops plus 2 concrete data leaks).
   Await sign-off on this REVISED, narrower proposal.
3. **§3 (`CreateFunctionOp` wire fields)**: three fields, three
   different gates — `visibility` ungated, `security: Definer` via
   HMAC parity with `chmod`'s setuid gate (not an extra Manage check),
   `secret_grants` via `Manage(Root)` + HMAC (since no finer-grained
   secrets-ACL concept exists yet). Await sign-off on this REVISED,
   per-field proposal (not the original single blanket "Manage" gate).
