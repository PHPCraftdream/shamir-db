Task #544 — two related WASM host-capability gaps: (1) the egress
allowlist is DB-wide, not per-function, contradicting the documented
capability model; (2) a guest function can overwrite an `env.*` global
via `global_set`, corrupting seeded secrets shared across functions in
the same process.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Finding 2 (do this one first — small, safe, self-contained)

`crates/shamir-wasm-host/src/wasm/host_globals.rs`'s `host_global_set`
(currently ~lines 5-27) writes ANY key unconditionally:

```rust
caller.data().globals.set(key, value);
```

Its sibling `host_global_get` (~lines 34-104) already gates reads on the
`env.*` namespace:

```rust
if let Some(env_name) = key.strip_prefix("env.") {
    if !caller.data().secret_grants.contains(env_name) {
        return Ok(0);
    }
}
```

But `global_set` has NO equivalent check — a guest can
`global_set("env.SOME_SECRET", <anything>)` and overwrite the
OS-seeded value in the shared `GlobalVars` (seeded once via
`GlobalVars::seed_env`, see `context.rs` ~line 204), corrupting it for
every other function invocation sharing that same globals instance
for the rest of the process's life (not a fresh copy per call).

**Fix**: in `host_global_set`, reject (or silently ignore — pick one,
document the choice) any `key` starting with `"env."`. Do NOT check
`secret_grants` here — the point isn't "can this function read this
secret", it's "no guest function may ever WRITE into the `env.*`
namespace at all", full stop, regardless of grants. Prefer returning a
trappable error over silent ignore if the wasm ABI here already surfaces
errors to the guest in a way that's distinguishable from a successful
no-op (check how other host functions in this file signal a guest-visible
error vs. a silent success, and match the existing convention — read
`wasm_function.rs`'s error-handling/trap convention for host functions
before deciding).

## Finding 1 (larger — per-function net_grants capability)

`crates/shamir-db/src/shamir_db/shamir_db/core.rs`'s `build_net_gateway`
(currently ~lines 601-609):

```rust
pub(super) fn build_net_gateway(&self) -> Arc<dyn NetGateway> {
    Arc::new(super::super::curl_gateway::CurlNetGateway::new(
        self.net_allowlist.to_vec(),
    ))
}
```

Called from `build_invoke_ctx` (~lines 589-599), which already has
`fn_name` available and already does an analogous per-function lookup
for `secret_grants`:

```rust
pub(super) fn build_invoke_ctx(&self, fn_name: &str, actor: Actor) -> FnCtx {
    let grants = self
        .function_meta(fn_name)
        .map(|m| m.secret_grants)
        .unwrap_or_default();
    FnCtx::with_globals(self.globals.clone())
        .with_registry(self.functions.clone())
        .with_net(self.build_net_gateway())
        .with_secret_grants(grants)
        .with_actor(actor)
}
```

`build_net_gateway` ignores `fn_name` entirely and always grants the
FULL `self.net_allowlist` — every function, regardless of owner or
declared capability, gets the DB's whole egress reach. This contradicts
`docs/roadmap/ACCESS_HIERARCHY.md:73-74`'s documented per-function
capability model.

### The fix

Mirror `secret_grants`'s existing shape exactly:

1. **`crates/shamir-wasm-host/src/meta.rs`**: add `net_grants: Vec<String>`
   to `FunctionMeta` (alongside `secret_grants`), update `new()`,
   `from_record` (default to empty `Vec` if the field is absent — same
   pattern as `secret_grants`), `inject_into` (persist as a
   `QueryValue::List`, same pattern), and `CreateFunctionOptions`
   (default empty, same pattern). This is the SAME struct/file
   `secret_grants` lives in — follow its exact conventions line-for-line,
   don't invent a different shape.
2. **`crates/shamir-db/src/shamir_db/shamir_db/core.rs`**: change
   `build_net_gateway(&self)` to `build_net_gateway(&self, fn_name: &str)`.
   Look up `self.function_meta(fn_name).map(|m| m.net_grants)`, and
   INTERSECT it with `self.net_allowlist` (the function's grant further
   restricts the DB-wide allowlist — a function can never exceed the DB's
   own ceiling, matching the audit's "intersect it with the host
   allowlist" framing). Update `build_invoke_ctx`'s call site to pass
   `fn_name` through (it already has it as a parameter).
3. **Design question to resolve before committing**: what does an EMPTY
   `net_grants` mean? Two candidate semantics:
   - **(a) Empty = full DB allowlist** (backward-compatible default —
     every existing function with no `net_grants` set behaves exactly as
     before this fix; this matches `secret_grants`'s OWN semantics, where
     an empty list means "no secrets granted", i.e. empty is the
     RESTRICTIVE default there — so this option is actually
     INCONSISTENT with the secret_grants precedent).
   - **(b) Empty = no egress at all** (matches `secret_grants`'s
     precedent — empty means nothing granted — but is a BREAKING change
     for every existing function that currently relies on DB-wide egress
     with no explicit grant needed).
   Investigate which one the audit intends (re-read
   `docs/roadmap/ACCESS_HIERARCHY.md:73-74` and the audit doc's finding
   #2/§2e) and pick the one that's actually consistent with the
   documented capability model AND doesn't silently break every
   pre-existing function that calls `http_fetch` today with no
   `net_grants` set. If the two goals conflict (closing the gap requires
   a breaking default), say so explicitly in your report rather than
   silently picking the convenient option — this is exactly the kind of
   call the orchestrator needs to review before commit.
4. Update every other caller of `build_net_gateway` (grep the whole
   workspace — there may be more than the one call site in
   `build_invoke_ctx`) to pass `fn_name`.

## Explicit permission to scope down

Finding 2 (global_set gate) is small, safe, and should always land.
Finding 1 (net_grants) is a genuine wire/catalogue-schema change with a
real design question (empty-grants semantics) that could interact badly
with existing functions if gotten wrong. If, after investigating,
closing finding 1 fully looks like it would silently break every
existing function's egress access (option (b) above) and you're not
confident that's the intended, safe rollout, it is FINE to:
- Land finding 1 with the BACKWARD-COMPATIBLE semantics (empty
  `net_grants` = full DB allowlist, i.e. option (a)) even though it's
  inconsistent with `secret_grants`'s own empty-means-nothing precedent,
  and clearly document in the code AND your report that this is a
  narrower capability model than `secret_grants`'s (a function must
  explicitly ask for LESS egress via a non-empty grant list; there's no
  way to get MORE than the DB default, but a function that never sets
  `net_grants` isn't retroactively locked out of the internet it could
  already reach). This still closes the audit's core complaint (a
  function CAN now be scoped to less than the full DB allowlist) without
  a breaking default.
- OR implement full finding-1 closure (option (b)) if you're confident
  it's correct and you've checked no existing test/functionality assumes
  DB-wide egress with an empty grant list — verify this empirically by
  running the existing WASM/net test suite before deciding.
Either way, do NOT leave it half-wired (e.g. the struct field exists but
`build_net_gateway` never reads it) — pick one semantics and wire it
through completely.

## Test requirement

- Finding 2: a guest `global_set("env.SOME_KEY", ...)` where
  `SOME_KEY` was seeded by `seed_env` does NOT overwrite the seeded
  value (read it back via a subsequent `global_get` — if `SOME_KEY` is
  in `secret_grants`, or via whatever seam the existing tests use to
  inspect `GlobalVars` directly — check `context.rs`'s existing tests
  for the established pattern).
- Finding 1: a function WITHOUT a net grant that requests egress to a
  host the DB-wide allowlist permits is denied (for whichever semantics
  you chose — write the test to match your chosen empty-means-X
  decision, and ALSO add a test proving a function WITH an explicit,
  narrower `net_grants` list is denied a host outside its own grant even
  though the DB allowlist would otherwise permit it — this is the actual
  scoping behavior the audit wants, independent of the empty-list
  question).

## Test scope

```
./scripts/test.sh -p shamir-db -p shamir-wasm-host
```

## Verification (lighter per-task gate, agreed for this campaign)

```
cargo check --workspace --all-targets
./scripts/test.sh -p shamir-db -p shamir-wasm-host
```
Full fmt/clippy/test --full is FINAL-GATE's (#529's) job. This task does
NOT block FINAL-GATE (MEDIUM hardening, no escalation) — do not add it
to #529's blockedBy.

## Report format

```
[Implementation] Status: fixed / scoped-down-with-followup
  > Finding 2 (global_set env.* gate): exact mechanism (reject vs
    silent-ignore, and why), confirmed secret_grants-gated global_get
    still works, confirmed a seeded env value survives a guest
    global_set attempt
  > Finding 1 (net_grants): net_grants field added mirroring
    secret_grants exactly; empty-list semantics chosen (a or b) +
    reasoning; build_net_gateway signature change + all call sites
    updated; confirmed a function WITH a narrower explicit grant is
    denied a host outside it
  > New tests: confirmed RED before the fix, GREEN after, for both
    findings
[Verification]
  cargo check --workspace --all-targets: pass/fail
  ./scripts/test.sh -p shamir-db -p shamir-wasm-host: pass/fail
```

Given this touches the WASM host's capability/security boundary (net
egress + the secret-seeded globals namespace), this MUST go through an
adversarial review pass before committing — same discipline as
#537/#540/#541/#542/#543 this campaign. If that review finds a genuine
bug, the orchestrator fixes it directly (never re-delegates),
re-verifies, and sends the fix through a second review pass before
committing.
