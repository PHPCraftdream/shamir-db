Task: HIGH-security residual cluster — 4 independent findings from
`docs/audits/2026-07-06-security-network-surface.md`. Task #495.

These are INDEPENDENT findings — fix each on its own merits. Per this
campaign's established pattern, if any single finding is genuinely
high-complexity/structural beyond what's tractable here, STOP on that
ONE finding, document your investigation + a follow-up task
description, and continue with the others.

## Finding 1d (MEDIUM-HIGH) — resumption ticket not bound to the TLS channel VALUE

`crates/shamir-connect/src/server/resume.rs`'s `process_resume` (confirm
current line numbers, ~line 221-360): `plain.channel_binding_at_auth` is
stored in the ticket (captured at original auth time), but it is NEVER
compared against `request.channel_binding_now` (the CURRENT connection's
TLS exporter value). Only `check_anti_downgrade` runs (line ~278), which
verifies the STRENGTH of the binding mode, not the VALUE — a stolen
resumption ticket is therefore portable across TLS sessions/networks,
because the new session is created using the CURRENT exporter (line
~346, ~360) with no equality check against the value stored at original
auth. Full SCRAM auth is channel-bound; resume is not — an asymmetry.

### Fix — 1d

1. In `process_resume`, after decrypting the ticket and before creating
   the new session, compare `plain.channel_binding_at_auth` against
   `request.channel_binding_now` using constant-time equality (this repo
   already has timing-safe comparison primitives for secret material —
   grep for the existing pattern used for HMAC/proof comparisons, e.g. in
   `common/scram.rs` or wherever SCRAM verification lives, and reuse it;
   do NOT introduce a new non-constant-time `==` on this comparison).
2. On mismatch, reject the resume attempt (same error class as other
   resume-rejection paths — check what `process_resume` already does for
   a rejected resume, e.g. anti-downgrade failure, and mirror that error
   shape).
3. Investigate: is there a legitimate mode where `channel_binding_now`
   is intentionally different from `channel_binding_at_auth` (e.g. an
   explicitly-permitted no-binding fallback mode for compatibility)? If
   so, the comparison must only apply when both sides indicate binding
   was actually used — check `check_anti_downgrade`'s existing logic for
   how it already reasons about binding-mode presence/absence, and make
   the new value-comparison consistent with that logic (don't require
   equality between two "no binding" sentinels if that's a legitimate
   state, but DO require equality whenever binding was used at auth time).
4. Correct the stale doc-comment/promise at `session.rs` (search for
   "future ticket bindings" or similar — the audit cites `session.rs:128-130`,
   confirm current location) to describe the FIXED behavior.

## Finding 2b-i (HIGH) — no per-connection/per-user subscription limit

`crates/shamir-server/src/subscriptions/registry.rs` (confirm current
line, audit cites `:39`) and `crates/shamir-server/src/db_handler/subscribe_handler.rs`
(audit cites `:38`): there is no cap on the number of active subscriptions
per connection or per user. Each `Subscribe` op spawns a bridge task +
broadcast receiver (confirmed still true — read `bridge_task` in
`subscriptions/bridge.rs`, note this file ALREADY has a proper per-table
read-ACL fix from a prior task #439/CRIT-5, do not touch that logic,
only add the count limit around it). A single connection (or a single
user across up to `MAX_SESSIONS_PER_USER=16` sessions) can spawn
unbounded bridge tasks and receivers.

### Fix — 2b-i

1. Add a per-connection subscription count limit, enforced in
   `activate_subscriptions` (or `subscribe_handler.rs`, wherever the
   `Subscribe` op is dispatched) BEFORE spawning the bridge task — reject
   with a clear error if the connection is already at its cap.
2. Investigate whether a per-user (across sessions) cap makes sense
   given the existing `MAX_SESSIONS_PER_USER=16` mechanism — check
   `server/session.rs` for how per-user limits are currently tracked and
   decide whether extending that same tracking to subscriptions is
   straightforward, or whether a per-connection cap alone is sufficient
   for this pass (a per-user cross-session cap can be a documented
   follow-up if it requires touching the session-tracking machinery more
   invasively).
3. Pick a sane default limit (grep this codebase's `shamir-tunables`
   crate for how similar operator-configurable limits are defined and
   follow that convention — e.g. a named constant with a doc comment
   explaining the choice, ideally operator-configurable like other
   tunables in this repo rather than hardcoded).
4. Add a regression test that spawns subscriptions past the limit and
   confirms the N+1th is rejected, not silently accepted.

## Finding 2b-ii (HIGH) — reactive subscriptions ignore operator query-limits

`crates/shamir-server/src/subscriptions/reactive.rs` (confirm current
lines, audit cites `:64-76,113-151`): `DeliverMode::Batch`/`Call`
reactive subscriptions use `BatchLimits::default()` instead of
`self.query_limits` (the actual operator-configured limits). This means
one cheap external insert can trigger N expensive batch re-queries under
the SUBSCRIBING actor's identity, unbounded by whatever limits the
operator actually configured for that actor/session.

### Fix — 2b-ii

1. Thread `self.query_limits` (or wherever the real operator-configured
   limits live for this connection/session) through to wherever
   `BatchLimits::default()` is currently used in the reactive-delivery
   path, replacing the hardcoded default.
2. Confirm this doesn't change behavior for a client that never set
   custom limits (i.e., `query_limits` should already equal
   `BatchLimits::default()` in that case, so this is purely closing the
   gap for clients/actors that DID configure tighter or different limits
   — verify this assumption by reading how `query_limits` is populated
   elsewhere in this codebase, e.g. non-reactive query paths).
3. Add a regression test proving a reactive subscription with a
   restrictive operator-configured limit actually gets bounded batches,
   not `BatchLimits::default()`-sized ones.

## Finding HIGH (WASM aggregate fuel fan-out) — from section 2b

`crates/shamir-wasm-host/src/wasm/wasm_function.rs` (confirm current
lines, audit cites `:341-343` for the fuel-reset and `:319-323` for the
missing epoch mechanism): every WASM `Store` (including nested `ctx.call`
invocations) gets a FRESH `set_fuel(1e9)` — `host_call.rs` (audit cites
`:81`) only limits call-DEPTH (32), not the aggregate COUNT of nested
calls, so one request can burn `(nested call count) × 1e9` instructions
with no overall ceiling. Additionally, time spent in host-awaits isn't
charged against fuel at all, and a pure-CPU guest with no `epoch_interruption`
configured pins a tokio worker for its ENTIRE fuel budget — N concurrent
CPU-bound guests can freeze the runtime.

### Fix — WASM fuel fan-out (investigate; scope down per audit's own guidance if too invasive)

1. Per the audit's fix sketch: configure `epoch_interruption` on the
   wasmtime `Engine`/`Store` so a long-running guest can be interrupted
   from outside (a periodic epoch-tick), NOT just relying on fuel
   exhaustion (which doesn't fire for host-await time). Investigate
   wasmtime's actual epoch API (read wasmtime's docs/the vendored crate,
   don't guess versions) before implementing.
2. Add a wall-clock deadline per REQUEST (not per nested Store) so the
   guest's total wall-clock time (across all nested calls) is bounded
   regardless of how fuel is being consumed or reset.
3. Add a genuine AGGREGATE fuel budget across nested `ctx.call`
   invocations within one logical request — instead of each nested Store
   getting a full fresh `1e9`, the nested calls should draw down from a
   shared per-request budget. Investigate how `ctx.call`/`host_call.rs`
   currently threads context between nested Store instances to find the
   right place to carry this shared budget.
4. **Scope-down escape valve**: if implementing genuine epoch_interruption
   + aggregate cross-Store fuel budget requires a larger architectural
   change to how `wasm_function.rs`/`host_call.rs` manage Store lifecycle
   than is safe for a single surgical pass, implement the WALL-CLOCK
   DEADLINE part alone (item 2, the simplest and most directly effective
   mitigation against runtime-freezing) and defer the rest with a
   documented follow-up task, per this campaign's established pattern
   (see tasks #488's 3.2, #489's 2.1/2.3, #494's 1.6 for the precedent).

## Finding HIGH (SSRF via WASM egress) — section 2c

`crates/shamir-wasm-host/src/wasm/host_http.rs` (confirm current line,
audit cites `:145`) does NOT call `check_url_allowed` at all — the
allowlist check is delegated entirely to an out-of-scope `CurlNetGateway`
with no re-verification of redirects. Additionally, `net_gateway.rs`
(confirm lines, audit cites `:162-167`) checks the allowlist against the
literal host STRING without DNS resolution — `meta.attacker.com` could
resolve to `169.254.169.254` (cloud IMDS) and pass the string-level
allowlist check. A separate MEDIUM sub-finding: non-canonical IP forms
(`2130706433`, `0x7f000001`, `[::ffff:a9fe:a9fe]`) bypass the private-IP
detector (`net_gateway.rs:170-191`).

### Fix — SSRF (this is the highest-risk finding in this cluster; investigate thoroughly)

1. Make `host_http.rs`'s WASM guest-initiated HTTP path actually CALL
   `check_url_allowed` (or wherever the real allowlist+private-IP check
   lives) — do not rely solely on the out-of-scope gateway.
2. Resolve DNS and check the ALLOWLIST/PRIVATE-IP RESULT of the
   resolved IP, not just the literal hostname string — this closes the
   `meta.attacker.com → 169.254.169.254` bypass.
3. Re-verify on EVERY redirect hop, not just the initial URL — if the
   underlying HTTP client follows redirects transparently, either disable
   auto-redirect-following and re-check each hop manually, or find
   wherever this codebase's HTTP client exposes a redirect-policy hook.
4. Fix the non-canonical-IP bypass: normalize an IP string (decimal,
   hex, IPv4-mapped-IPv6, etc.) to its canonical form BEFORE the
   private-range check, so `2130706433`/`0x7f000001`/`[::ffff:a9fe:a9fe]`
   are all correctly recognized as loopback/private.
5. **Scope-down escape valve**: if pinning the connection to a
   pre-resolved, pre-checked IP (to prevent a TOCTOU DNS-rebind between
   check and connect) requires swapping out the underlying HTTP client
   library or a larger architectural change, implement items 1, 2, and 4
   (the most directly reachable, lowest-risk wins) fully, and document
   items 3 (redirect re-checking) and the DNS-rebind-TOCTOU concern as an
   explicit, scoped-down follow-up task if it proves too invasive for
   this pass — but attempt all 5 first; this finding is HIGH severity and
   worth real effort before deferring anything.

## TDD/regression requirement

For EACH finding you fix: add a regression test that would FAIL without
the fix. For 1d: a resume attempt with mismatched channel-binding value
must be rejected. For 2b-i: exceeding the subscription cap must be
rejected. For 2b-ii: a reactive subscription with a restrictive limit
must actually be bounded. For the WASM/SSRF findings: whatever subset you
implement needs a real test proving the specific attack scenario
described in the audit is now blocked (e.g. a guest attempting to reach
a private IP via a non-canonical form, or via DNS resolving to a private
IP, must be rejected).

## Test scope

```
./scripts/test.sh -p shamir-connect
./scripts/test.sh -p shamir-server
./scripts/test.sh -p shamir-wasm-host
```

## Gate

```
cargo fmt -p shamir-connect -p shamir-server -p shamir-wasm-host -- --check
cargo clippy -p shamir-connect -p shamir-server -p shamir-wasm-host --all-targets -- -D warnings
```

If clippy flags PRE-EXISTING lints in code you did not touch, do not fix
them here — note them in your final report instead.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

For EACH of the findings (1d, 2b-i, 2b-ii, WASM-fuel-fanout, SSRF):
```
[Finding X] Status: fixed / partially-fixed / deferred
  > What changed + regression test added (if fixed/partial)
  > OR: deferral reason + follow-up task description (if deferred)
```
Full test/gate results (exact commands + pass/fail) for whichever crates
were actually touched.
