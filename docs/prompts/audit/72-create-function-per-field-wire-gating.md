בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Brief: CreateFunctionOp per-field wire gating for security/secret_grants/visibility (task #554)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Context

`docs/design/root-user-group-dac-posture-550-decision.md` §3 (already
signed off by the project owner) documents this exact design in full —
read it first, it is the source of truth for every claim below. This
brief is the actionable slice of that decision.

`crates/shamir-wasm-host/src/meta.rs` already defines `Security`
(`Invoker`/`Definer`), `Visibility` (`Public`/`Private`), and
`CreateFunctionOptions { replace, visibility, security, secret_grants,
net_grants }` (lines 45-48, 15-18, 193-198) — the in-process API
(`ShamirDb::create_function_with_opts_as`,
`crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:134-`)
already fully supports all three fields. The gap is purely
wire-reachability: `CreateFunctionOp`
(`crates/shamir-query-types/src/admin/types/function_ops.rs:17-25`) has
only `create_function`, `source`, `wasm`, `replace` — no
`security`/`secret_grants`/`visibility` field exists on the wire, so
every wire-created function is silently forced to
`Security::Invoker`, empty `secret_grants`, and `Visibility::Private`
(`CreateFunctionOptions::default()`,
`crates/shamir-wasm-host/src/meta.rs:201-210`) regardless of what the
in-process API could otherwise do. This is safe as shipped (every
default is least-privileged) but the gap must be closed with THREE
DIFFERENT gates, not one uniform check — reasoned from what each field
actually does, not from op-symmetry with e.g. `chmod`.

## Scope

### 1. Wire type: add 3 fields + `hmac` to `CreateFunctionOp`

`crates/shamir-query-types/src/admin/types/function_ops.rs`:
```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateFunctionOp {
    pub create_function: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wasm: Option<String>,
    #[serde(default)]
    pub replace: bool,
    /// `"public"` or `"private"` (parsed via `Visibility::from_str`,
    /// `shamir-wasm-host/src/meta.rs`). Absent/None → `Visibility::Private`
    /// (unchanged default). No extra gate — Private is already the
    /// default, and setting Public on your own newly-created resource is
    /// harmless (same as the existing chmod-to-Public path, which needs
    /// only ordinary owner+Manage rights already implied by CREATE).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
    /// `"invoker"` or `"definer"` (parsed via `Security::from_str`).
    /// Absent/None → `Security::Invoker` (unchanged default). Setting
    /// `"definer"` requires an `hmac` tag — see §3 below.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security: Option<String>,
    /// Non-empty requires BOTH `Action::Manage` on `ResourcePath::Root`
    /// AND an `hmac` tag — see §3/§4 below.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secret_grants: Vec<String>,
    /// Hex-encoded HMAC-SHA256 tag, required IFF `security == Some("definer")`
    /// or `secret_grants` is non-empty (conditional — NOT required for
    /// every `CreateFunctionOp`, unlike `chmod`/`drop_db`/etc.). See §4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hmac: Option<String>,
}
```

### 2. `create_function_with_opts_as` gains a per-field gate

`crates/shamir-db/src/shamir_db/shamir_db/function_management.rs`'s
`create_function_with_opts_as` already checks
`authorize_access(&actor, &ResourcePath::FunctionNamespace, Action::Create)`
(line 141-143) — this stays, unconditionally, exactly as today (it
already bounds Definer's blast radius, see below). ADD, before that
existing check or right after it:

- If `opts.secret_grants` is non-empty: additionally require
  `authorize_access(&actor, &ResourcePath::Root, Action::Manage)`.
  Rationale (from the decision doc): `secret_grants` names OS-seeded
  process environment variables (`GlobalVars::seed_env`), a resource
  class the creator has NO defined rights over at all — there is no
  "which secrets can this actor grant" concept anywhere in this
  codebase yet (confirmed: none found). Without this gate, any actor
  holding bare `Create` on `FunctionNamespace` could request
  `secret_grants: ["ADMIN_DB_PASSWORD"]` on their own new function and
  exfiltrate host secrets by calling it. This is deliberately
  admin-only pending a real secrets-ACL (a natural fit for the
  identity/privilege work in #548/#549/#550 §1) — do not invent a
  finer-grained check that doesn't actually exist.
- `Security::Definer` needs NO extra authorization check here (the
  creator already owns their own brand-new function by construction —
  `owned_enforced(actor)` at save time,
  `function_management.rs:219` per the design doc's trace — so Definer-
  on-your-own-new-function is never a self-escalation; you cannot
  hijack another owner's identity this way). Its gate is HMAC-only —
  see §3/§4.

The `wasm-db`-level function signature does NOT need to change to add
an `Actor`-scoped "may grant this secret" parameter — there is nothing
to check per-secret; the gate is coarse (`Manage(Root)`) exactly
because no finer concept exists.

### 3. HMAC canonical form: `create_function`

`crates/shamir-query-types/src/hmac.rs` — add to the module doc table
and implement:
```rust
/// `b"create_function\0<name>\0<security>\0<secret_grants_csv>"`
///
/// `<security>` is `"invoker"` or `"definer"` (the value actually being
/// set — Display of the parsed `Security`, or the literal unparsed
/// string if you don't want a hmac.rs -> wasm-host dependency; match
/// whatever `canonical_chmod` etc. do for enum-valued fields elsewhere
/// in this file for the established convention). `<secret_grants_csv>`
/// is the grants joined by `,` in the order given (empty string if
/// none) — this must be BYTE-IDENTICAL between client and server, so
/// do not sort/dedupe; whatever order the caller supplies is what gets
/// hashed, and the server must canonicalize identically from the
/// deserialized `Vec<String>` in wire order.
pub fn canonical_create_function(name: &str, security: &str, secret_grants: &[String]) -> Vec<u8> {
    join_null(&[
        b"create_function",
        name.as_bytes(),
        security.as_bytes(),
        secret_grants.join(",").as_bytes(),
    ])
}
```
Follow this file's existing `join_null`/module-doc-table conventions
exactly (see `canonical_chmod`/`canonical_create_group` for the
established shape) — add one row to the doc table at the top of the
file.

### 4. Server: conditional HMAC check in `check_destructive_hmacs`

`crates/shamir-server/src/db_handler/admin.rs`'s `check_destructive_hmacs`
(~line 361-492) is a `match` over `&entry.op` where every existing arm
UNCONDITIONALLY requires an hmac tag. `CreateFunctionOp` needs a
CONDITIONAL arm — HMAC is required only when `security ==
Some("definer")` or `secret_grants` is non-empty:
```rust
BatchOp::CreateFunction(op) => {
    let needs_hmac = op.security.as_deref() == Some("definer")
        || !op.secret_grants.is_empty();
    if !needs_hmac {
        continue; // no hmac needed — same as the `_ => continue` fallthrough
    }
    (
        canon::canonical_create_function(
            &op.create_function,
            op.security.as_deref().unwrap_or("invoker"),
            &op.secret_grants,
        ),
        op.hmac.as_ref(),
    )
}
```
Insert this arm before the trailing `_ => continue,` (existing line
~473). This is the ONLY file that needs a conditional (as opposed to
unconditional) HMAC arm today — read the existing arms above it for
the established two-tuple `(canonical, supplied)` shape before writing
this one; do not restructure the function's overall shape for this one
op.

### 5. Wire dispatcher: pass the 3 new fields through

`crates/shamir-db/src/shamir_db/execute/admin_function.rs`'s
`handle_create_function` (lines 15-75) currently calls
`create_function_from_source_as`/`create_function_from_wasm_as`
(which always use `CreateFunctionOptions::default()` + `replace`).
Change it to build a real `CreateFunctionOptions` from the op's new
fields and call `create_function_with_opts_as` directly instead:
```rust
let visibility = match op.visibility.as_deref() {
    Some(s) => s.parse().map_err(|e: String| err(e))?,
    None => Visibility::Private,
};
let security = match op.security.as_deref() {
    Some(s) => s.parse().map_err(|e: String| err(e))?,
    None => Security::Invoker,
};
let opts = CreateFunctionOptions {
    replace: op.replace,
    visibility,
    security,
    secret_grants: op.secret_grants.clone(),
    net_grants: Vec::new(), // unchanged — net_grants has its own separate wiring (task #544), not part of this op
};
```
then call `self.shamir.create_function_with_opts_as(&op.create_function, source, opts, self.actor.clone())`
with `source` built the same way it is today (`FunctionSource::Source`/
`FunctionSource::Wasm`). Preserve the existing `if source.is_none() &&
wasm.is_none()` error path unchanged.

### 6. Client builders (Rust + TS) — wire the 3 fields + hmac

`crates/shamir-query-builder/src/ddl/function.rs`'s `CreateFunction`
builder gains `.visibility(impl Into<String>)`, `.security(impl
Into<String>)`, `.secret_grants(impl IntoIterator<Item = impl
Into<String>>)`, and the standard `.hmac(impl Into<String>)` (mirror
the exact `.hmac()` shape already used by ~20 other builders in this
crate, e.g. `crates/shamir-query-builder/src/ddl/access_control.rs:86`).
`build()` threads the new fields into `CreateFunctionOp`.

`crates/shamir-client-ts/src/core/hmac.ts` gains `canonicalCreateFunction`
mirroring `canonicalCreateGroup`'s shape (line ~348) — same
null-byte-joined convention, calling the shared `joinNull` helper.
Whatever TS builder/type currently maps to `create_function` on the
wire (check `crates/shamir-client-ts/src/core/builders/` and
`crates/shamir-client-ts/src/core/types/` for the existing pattern used
by `create_group`/`chmod`) gains the matching `visibility`/`security`/
`secret_grants`/`hmac` fields.

## Test scope

Required tests (mirrors the coverage-matrix precedent from #546/#553):
- `visibility: "public"` on a wire-created function: no hmac required,
  succeeds; function's persisted `Visibility` is `Public`.
- `security: "definer"` WITHOUT an hmac tag: rejected `hmac_required`.
- `security: "definer"` WITH a correct hmac tag: succeeds; function's
  persisted `Security` is `Definer`.
- `security: "definer"` WITH a WRONG/mismatched hmac tag: rejected
  `hmac_mismatch`.
- non-empty `secret_grants` WITHOUT `Manage(Root)`: rejected
  `access_denied` (even with a correct hmac — authorization is checked
  independently of HMAC, same "both must pass" pattern as every other
  gated op in this codebase).
- non-empty `secret_grants` WITH `Manage(Root)` but WITHOUT an hmac
  tag: rejected `hmac_required`.
- non-empty `secret_grants` WITH `Manage(Root)` AND a correct hmac tag:
  succeeds; function's persisted `secret_grants` matches.
- A `CreateFunctionOp` with NEITHER `security: "definer"` NOR non-empty
  `secret_grants` (the common case — plain function creation): NO hmac
  required at all (regression test — confirms the conditional gate
  doesn't accidentally make hmac mandatory for ordinary function
  creation, which would break every existing caller).

Run:
```
./scripts/test.sh -p shamir-query-types -p shamir-query-builder -p shamir-db -p shamir-server --full
```
Plus the TS suite for the client-side canonical function and builder
changes (follow this repo's existing TS test-run convention for
`shamir-client-ts`).

## Out of scope

- `net_grants` — already wired separately (task #544); do not touch its
  plumbing.
- Any new "which secrets can X grant" ACL concept — deliberately not
  built here, per the decision doc; the gate is coarse
  (`Manage(Root)`) on purpose.
- An `alter_function` op or any ALTER-style mutation of an existing
  function's `security`/`secret_grants`/`visibility` — this brief is
  CREATE-only, matching `CreateFunctionOp`'s actual wire surface today.

## Definition of done

- `cargo fmt` clean for every touched crate.
- `cargo clippy --all-targets -- -D warnings` clean for every touched
  crate (the pre-existing `shamir-engine` `type_complexity` warning,
  task #562, is NOT part of this task's scope — confirm via `git diff
  --stat` that you haven't touched that file if you see it).
- `./scripts/test.sh -p shamir-query-types -p shamir-query-builder -p shamir-db -p shamir-server --full`
  green, including all 8 tests above.
- TS build/tests green for the client-side changes.
