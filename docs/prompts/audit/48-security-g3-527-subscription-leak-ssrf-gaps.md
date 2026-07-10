Task: G3 (task #527) — two independent security-residual findings from
task #495's `@fl` review (former #513 + #514). Fix both; they touch
different crates and don't interact.

## Part 1 (former #513) — subscription-cap slot leak when the bridge task exits on its own

### Current behavior (confirmed by reading the code)

`crates/shamir-server/src/subscriptions/registry.rs`'s `SubscriptionRegistry`
tracks active subscriptions via an `AtomicUsize` counter (`active`) that
mirrors the cardinality of an `scc::HashMap<u64, ActiveSubscription>`
(`subs`). The counter is decremented ONLY in two places: `remove(id)`
(explicit `Unsubscribe`) and `close_all()` (connection teardown).

`crates/shamir-server/src/db_handler/subscribe_handler.rs::activate_subscriptions`
(around line 60-97) reserves a slot (`registry.try_reserve()`), spawns the
bridge task (`tokio::spawn(bridge::bridge_task(...))`), and inserts the
`JoinHandle` into the registry (`registry.insert(sub_id, ActiveSubscription
{ bridge_handle: handle })`). **Nothing ever notices when `bridge_task`
finishes on its own** — natural stream completion, an internal error, or
the CRIT-5 "all sources denied" abort case documented in this same file's
doc comment (line ~27). When that happens, the `scc::HashMap` entry AND
the `active` counter both stay exactly as they were — the slot is
permanently leaked for the life of the connection (or until the client
happens to send an `Unsubscribe` for an id it may not even know is dead).

### Fix

Give `bridge_task` a way to remove its own entry from the registry when it
exits, regardless of exit path (success, error, or abort-via-CRIT-5).

`crates/shamir-connect/src/server/conn_services.rs:24` shows
`ConnectionServices::extensions: Option<Arc<dyn std::any::Any + Send + Sync>>`
— it's ALREADY an `Arc`. You can obtain an owned, independently-cloneable
`Arc<SubscriptionRegistry>` from it without any type changes:
`conn.extensions.clone().unwrap().downcast::<SubscriptionRegistry>()` (note:
`Arc::downcast` on an OWNED `Arc<dyn Any + Send + Sync>`, not
`downcast_ref` on a borrow — check the exact API needed, `Arc<dyn Any>`'s
`downcast` method requires `Any: Send + Sync` bounds already satisfied
here).

Recommended shape: pass this `Arc<SubscriptionRegistry>` (or a suitable
clone) plus `sub_id` into `bridge_task`. Inside `bridge_task`'s async
body, construct a small RAII guard (e.g. `struct SubscriptionSlotGuard {
registry: Arc<SubscriptionRegistry>, sub_id: u64 }` with a `Drop` impl
that calls `self.registry.remove(self.sub_id);`) and bind it to a local
(`let _guard = SubscriptionSlotGuard { ... };`) near the top of the
function body, before any `.await` point that could exit early. Because
tokio task cancellation (via `JoinHandle::abort()`) unwinds through
in-scope locals at the next await point, this guard's `Drop` fires on
EVERY exit path: natural return, early error return, AND external abort
(e.g. the explicit-`Unsubscribe` path in `subscribe_handler.rs`, which
already calls `registry.remove(id)` itself before the abort takes effect
— the guard's later `Drop`-triggered `remove()` call in that case is a
safe, idempotent no-op since `scc::HashMap::remove` on an already-gone key
just returns `None`, so `active` won't be double-decremented — verify
this is actually how `registry.remove` is implemented, since correctness
here hinges on it, don't just assume it and move on).

**Do not restructure `ActiveSubscription`'s existing `Drop`-triggers-abort
mechanism** (`crates/shamir-server/src/subscriptions/registry.rs:13-17`)
— that's the OPPOSITE direction (registry removal triggers task abort) and
must keep working exactly as today for the explicit-unsubscribe and
close_all paths. This fix ADDS the missing direction (task self-exit
triggers registry removal), it doesn't replace the existing one.

### TDD

A regression test proving: spawn a subscription whose bridge task exits on
its own shortly after starting (e.g. a fabricated all-sources-denied
case, or a source that completes immediately) WITHOUT any explicit
Unsubscribe, then assert `registry.count()` returns to 0 (or the
pre-subscribe value) after waiting for the bridge task to actually finish
(poll/await the JoinHandle or a short deterministic wait — avoid a flaky
sleep-based test if there's a way to synchronize on task completion
directly, e.g. re-fetching the same `JoinHandle` isn't possible since it
was moved into the registry, so use an alternate signal like a broadcast
channel the test can await, or `tokio::task::yield_now()` in a retry loop
with a bounded attempt count rather than an unconditional fixed sleep).

## Part 2 (former #514) — SSRF guard: DNS-rebind TOCTOU + missing octal/short IP forms

### Current behavior (confirmed by reading the code)

`crates/shamir-wasm-host/src/net_gateway.rs::check_url_allowed_resolved`
(currently ~line 129) does its own `tokio::net::lookup_host` DNS
resolution to validate no resolved address is private/loopback, then
returns `Result<(), String>` — it does NOT return which IP(s) it
validated. The actual network call happens separately:
`crates/shamir-db/src/shamir_db/curl_gateway.rs::fetch` (currently ~line
32-40) calls `check_url_allowed_resolved` first, then shells out to the
SYSTEM `curl` BINARY as a subprocess using the ORIGINAL hostname/URL —
curl performs its OWN independent DNS resolution when it actually
connects.

**This is a genuine DNS-rebind TOCTOU**: between the guard's validation
resolution and curl's connection-time resolution, an attacker controlling
authoritative DNS for the requested domain can return a safe IP for the
first (validation) query and a private/internal IP for the second
(connection) query — the two resolutions are independent, so validating
"the resolved IP was safe" proves nothing about what curl will actually
connect to moments later.

### Fix

Close the gap by pinning curl's connection to the EXACT IP(s) that were
validated, using curl's `--resolve <host>:<port>:<address>` flag (this is
built for precisely this purpose — it makes curl skip its own DNS lookup
for that host:port and use the given address directly, while still
sending the correct `Host`/SNI based on the original hostname).

1. Change `check_url_allowed_resolved`'s return type (or add a sibling
   function) to also return the validated `IpAddr` (or the full resolved
   set) it checked — the exact-allowlist-match early-return path (line
   ~137-139) doesn't resolve DNS at all, so decide what that path returns
   for "the IP to pin" (likely: skip pinning entirely for that path, since
   it's an operator-opted-in exact host and no DNS resolution happened to
   rebind).
2. In `curl_gateway.rs::fetch`, after getting the validated IP(s) back,
   add a `--resolve host:port:ip` curl config entry (curl supports this in
   its config-file form too, matching the existing config-file-based
   invocation pattern already used here — check how `config_path` /
   `curl.cfg` is built and add to that same mechanism rather than
   inventing a new one) so curl's actual connection uses the SAME IP the
   guard validated, closing the re-resolution window. If multiple IPs were
   resolved (e.g. both an A and AAAA record), pick one deterministically
   (the brief leaves this to your judgment — e.g. prefer the first
   non-private address, or pin ALL of them via multiple `--resolve`
   entries if curl's config format supports repeating the flag for the
   same host — investigate and document your choice).

### Missing IP forms in `canonicalize_ip`

`crates/shamir-wasm-host/src/net_gateway.rs::canonicalize_ip` (currently
~line 235) already handles: standard dotted-quad / RFC-5952 IPv6, and a
BARE (whole-string) decimal `u32` or `0x`-prefixed hex `u32`. It does NOT
handle classic BSD `inet_aton`-compatible forms that some resolvers /
libc implementations still accept and that are a well-known SSRF-allowlist-
bypass vector:

1. **Octal per-octet dotted forms** — e.g. `0177.0.0.1` (leading `0` on a
   dotted component means octal: `0177` octal = `127` decimal → loopback).
   Rust's `Ipv4Addr::from_str` correctly REJECTS leading zeros (falls
   through step 1 of `canonicalize_ip`), and the current bare-u32 check
   (step 2) doesn't apply either since the string contains dots — so
   `canonicalize_ip("0177.0.0.1")` currently returns `None`, meaning the
   caller doesn't recognize this as a literal IP at all and may pass it
   through to DNS resolution / hostname-pattern allowlist matching
   without the private-IP literal check ever firing.
2. **Short/shorthand dotted forms** — e.g. `127.1` (interpreted by
   `inet_aton` as `127.0.0.1`), `192.168.1` (→ `192.168.0.1`), a 2- or
   3-component form where the LAST component absorbs the remaining bytes.

Add support for both to `canonicalize_ip` (or a helper it calls),
matching `inet_aton`'s actual parsing rules (each of up to 4 dot-separated
components may itself be decimal, octal (leading `0`), or hex (leading
`0x`); the last component present absorbs however many bytes remain to
make 4 total). Write this carefully and test it explicitly against the
two examples above plus a few more (a hex-octet form, a 3-component form)
— this is exactly the kind of parsing logic where an off-by-one silently
reintroduces the bypass it's meant to close.

### TDD

1. Tests for `canonicalize_ip("0177.0.0.1")` and similar octal forms
   resolving to the correct loopback/private `Ipv4Addr`.
2. Tests for short forms (`"127.1"`, `"192.168.1"`, etc.) resolving
   correctly.
3. An end-to-end test through `check_url_allowed` (or
   `check_host_allowed`) proving these forms are REJECTED when they
   resolve to a private/loopback address, even though they weren't
   caught before this fix.
4. A test for the DNS-rebind fix: this is harder to test with a REAL
   attacker-controlled DNS server; at minimum, verify the `--resolve`
   pinning mechanism is actually wired into curl's invocation (e.g. by
   inspecting the generated `curl.cfg` in a test, confirming it contains
   the expected `--resolve host:port:ip` entry with the SAME ip that
   `check_url_allowed_resolved` validated) rather than trying to simulate
   a live DNS-rebind race, which isn't practical in a unit test.

## Verification (lighter per-task gate, agreed this session)

```
cargo check --workspace --all-targets
./scripts/test.sh -p shamir-server -p shamir-wasm-host -p shamir-db
```

Do NOT run the full fmt/clippy/test --full gate — that's FINAL-GATE's job.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

```
[Part 1] Status: fixed
  > Fix mechanism (RAII guard shape, where it's constructed/held)
  > Confirm registry.remove()'s idempotency was verified, not assumed
  > New regression test, how it avoids flaky sleep-based synchronization

[Part 2] Status: fixed
  > DNS-rebind fix: exact mechanism (--resolve wiring), which IP is pinned
    when multiple resolve, and how the exact-allowlist-match path is handled
  > canonicalize_ip additions: octal + short forms, exact parsing rules
    implemented
  > New regression tests for both sub-parts

[Verification]
  cargo check --workspace --all-targets: pass/fail
  ./scripts/test.sh -p shamir-server -p shamir-wasm-host -p shamir-db: pass/fail
```
