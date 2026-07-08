Task: HIGH-security — TLS accept / WS upgrade have no timeout and no
per-IP connection cap (audit top-5 #4,
`docs/audits/2026-07-06-security-network-surface.md` §2a).

## Where

`crates/shamir-server/src/server/server_launcher.rs` — THREE accept
loops, each with the same shape:

- `accept_loop_tcp` (~line 783)
- `accept_loop_ws_native` (~line 844)
- `accept_loop_ws_browser` (~line 916)

Each does, after a **global** slot reservation via `ConnLimiter::try_acquire()`:

```rust
tokio::spawn(async move {
    let _guard = guard;
    let tls = match acceptor.accept(tcp).await {   // <-- NO TIMEOUT
        Ok(t) => t,
        Err(e) => { /* log, return */ }
    };
    // accept_loop_ws_native / accept_loop_ws_browser ALSO do, after this:
    let ws = match accept_native_ws(tls).await { ... };   // <-- NO TIMEOUT
    // (or accept_browser_ws for the browser loop)
    let framer = ...;
    handle_connection(ctx, peer_addr, framer, exporter).await;
});
```

`crates/shamir-server/src/conn_limiter.rs` — `ConnLimiter` is a single
global `AtomicUsize` counter (`max_active_connections`, default 10000).
It has NO per-IP tracking — one attacker IP can occupy an unbounded
fraction of the global cap.

There IS a `Duration` already threaded through for a related but
DIFFERENT purpose: `ctx.auth_init_timeout`
(`crates/shamir-server/src/connection/connection_context.rs:58`,
config key `security.connection.auth_init_timeout_ms`, default 5000ms
— `crates/shamir-server/src/config.rs:311-313`). It is currently only
applied INSIDE `handle_connection`, bounding the first-frame read
AFTER the TLS handshake / WS upgrade already completed
(`crates/shamir-server/src/connection/handshake.rs:552-569`:
`tokio::time::timeout(ctx.auth_init_timeout, read_fut)`). The gap this
task closes is the window BEFORE that — the TLS handshake itself and
the WS upgrade — which currently has no timeout at all.

## Why this is HIGH (per the audit: "the one trivial unauth-DoS")

A client that opens a TCP connection and never completes (or very
slowly completes) the TLS handshake — or, for the WS loops, completes
TLS but stalls the WS upgrade handshake — holds:
1. One global connection slot (`ConnLimiter` guard) forever, since
   nothing ever times out the `acceptor.accept(tcp).await` /
   `accept_native_ws(tls).await` future.
2. No IP-level limit either — the SAME attacker IP can open many such
   connections up to the ENTIRE global cap (10000 by default),
   completely starving legitimate clients with a single host and a
   handful of slow-loris sockets, no distributed botnet needed.

## Fix

### Part 1 — timeout around accept/upgrade (do this first — it's the
core, simplest fix)

Wrap the TLS accept call (and, for the two WS loops, the WS upgrade
call) in `tokio::time::timeout(ctx.auth_init_timeout, ...)` in all
THREE accept loops. Reuse the EXISTING `ctx.auth_init_timeout` value
(already plumbed into `ConnectionContext`, no new config knob needed) —
conceptually this closes the same "how long may an unauthenticated
peer occupy a slot" window, just extended to cover the handshake/
upgrade phase that currently has no bound at all. On timeout: log at
`debug!` level (matching the existing style for accept/handshake
failures in these loops) and `return` from the spawned task (the
`_guard` drops, releasing the connection slot — same cleanup path as
every other early-exit in these loops already uses).

Concretely, e.g. for `accept_loop_tcp`:
```rust
let tls = match tokio::time::timeout(ctx.auth_init_timeout, acceptor.accept(tcp)).await {
    Ok(Ok(t)) => t,
    Ok(Err(e)) => {
        tracing::debug!(?peer_addr, ?e, "tls handshake failed");
        return;
    }
    Err(_) => {
        tracing::debug!(?peer_addr, "tls handshake timed out");
        return;
    }
};
```
Apply the analogous wrap to `accept_native_ws(tls).await` in
`accept_loop_ws_native` and `accept_browser_ws(tls, &policy).await` in
`accept_loop_ws_browser` (same timeout value, same log-and-return
pattern).

### Part 2 — per-IP connection cap (companion fix, same audit finding)

Add a per-IP cap alongside the existing global `ConnLimiter`, enforced
at the SAME point (before `acceptor.accept`, right after the global
`try_acquire()` succeeds — cheapest-check-first ordering: if the global
cap already rejects, don't bother with a per-IP lookup). Design:

- A new structure (e.g. `PerIpLimiter` in `conn_limiter.rs`, sibling to
  `ConnLimiter`) backed by a concurrent map keyed by the peer's IP
  address (use `peer_addr.ip()` — you already have `peer_addr` from
  `listener.accept()` in every loop). Per this project's concurrency
  ideology (CLAUDE.md): use `scc::HashMap<IpAddr, AtomicUsize,
  shamir_collections::THasher>` or `DashMap` with the workspace's
  `THasher` — NOT `std::sync::Mutex<HashMap<...>>`. Follow the same
  `try_acquire()` / RAII-guard-with-Drop pattern as `ConnLimiter` so
  cleanup is automatic on every early-exit path (panic, timeout, TLS
  failure, etc.) — do not hand-roll decrement calls at each return site.
- A new config value, e.g. `max_active_connections_per_ip` (add to
  `ConnectionSecurity` in `config.rs` alongside `auth_init_timeout_ms`/
  `max_active_connections`, with a sane default — e.g. 100 — and the
  same `#[serde(default = "...")]` pattern already used for the other
  two fields in that struct. `0` should mean "no limit", mirroring
  `ConnLimiter::new`'s existing convention).
- Wire the new limiter into `ConnectionContext`/the accept-loop
  plumbing the same way `ConnLimiter` already is (constructed once in
  the caller that builds `ConnLimiter::new(...)`, passed down to each
  of the 3 accept loops), and call `try_acquire(peer_addr.ip())`
  immediately after the existing global `limiter.try_acquire()`
  succeeds, before TLS accept. On a per-IP cap rejection: same
  `tracing::debug!` + `continue` pattern the loops already use for the
  global-cap-rejection case (release the just-acquired global guard by
  letting it drop when the local `guard` variable goes out of scope —
  do NOT leak the global slot).

Keep the per-IP map bounded in practice: since the guard's `Drop`
removes the counter back toward zero (and a `0`-count entry can be
pruned or left as a cheap `AtomicUsize::new(0)` placeholder — cheaper
to leave it and let a low-frequency background reaper sweep zero-count
entries if you find one already exists for a similar purpose in this
crate; if not, a simple "check-and-remove-if-zero on release" inside
the guard's `Drop` is acceptable and keeps the map from growing
unboundedly across many distinct historical IPs).

## TDD requirement

1. **Red**: write tests (likely `#[tokio::test]` in a new or existing
   test module under `crates/shamir-server/src/server/tests/` or
   `crates/shamir-server/tests/` — check which already exists for
   `server_launcher`/`conn_limiter` and match that convention) that:
   - For Part 1: a TCP connection that connects but never sends TLS
     ClientHello bytes is dropped (the accept loop's task returns)
     within roughly `auth_init_timeout` — assert this via a short
     test-scoped timeout override (do NOT wait the full 5s default in
     a unit test; check whether `ConnectionContext`/the accept-loop
     signature already accepts an injectable timeout for testing, or
     whether you need to thread a small `Duration` test override
     through — prefer reusing whatever pattern existing tests in this
     crate already use for testing timeout-bound code, rather than
     inventing a new mechanism).
   - For Part 2: `PerIpLimiter::try_acquire` returns `Some` up to the
     per-IP cap, `None` on the cap+1'th call form the SAME IP, and
     `Some` again for a DIFFERENT IP even while the first IP is at cap
     (unit test on the limiter type directly — no need for a full
     TCP-level test here, that's what `ConnLimiter`'s own existing
     tests likely already look like — mirror that structure).
2. **Green**: implement Parts 1 and 2.
3. Confirm existing `server_launcher`/`conn_limiter`/connection-related
   tests still pass.

## Test scope command

```
./scripts/test.sh -p shamir-server
```

## Gate (must be clean before finishing)

```
cargo fmt -p shamir-server -- --check
cargo clippy -p shamir-server --all-targets -- -D warnings
```

If clippy flags PRE-EXISTING lints in code you did not touch, do not
fix them here — note them in your final report instead.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

When done, report exactly:
- Part 1: which of the 3 accept loops were changed, confirmation the
  SAME `ctx.auth_init_timeout` value is reused (not a new config knob),
  and the failing-test-then-passing evidence.
- Part 2: the new `PerIpLimiter` type's design (map choice, guard/Drop
  behavior, config key added + default value), and its test evidence.
- Whether both parts were completed, or only Part 1 (state explicitly
  if Part 2 was deferred and why — e.g. time, or a discovered blocker).
- Gate results (exact commands + pass/fail).
