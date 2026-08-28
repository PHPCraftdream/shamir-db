# shamir-transport-tcp -- Concurrency & lock-free invariants

## Summary

The crate is essentially clean under all five pillars. Full read + grep confirm zero `std::sync::Mutex`/`RwLock`/`parking_lot` sites, zero `scc::*`/`DashMap`/`ArcSwap` usage, and zero locks of any kind -- so there is no lock-across-`.await` or lock-contention exposure at all. Shared state is limited to `Arc` (TLS configs, test fixtures) and `AtomicU64` counters in tests; the framing API is per-connection `&mut R`/`&mut W` exclusive ownership, the ideal pillar-1 shape, with allocation-per-frame documented and pooled (`*_into`) alternatives supplied (pillar 3). Pillar 4 is vacuously satisfied -- the crate owns no hash-keyed structure. The only deviations found are two sync CPU-bound bootstrap helpers with no `spawn_blocking` guidance (pillar 2, letter) and a test pattern that runs Argon2id inline on a current-thread runtime while a peer task is spawned. Checked and NOT flagged: `tests/echo_e2e.rs:439`'s `session_store.len()` resolves to `shamir-connect`'s `SessionStore::len` -> `DashMap::len`, which is O(shards) (fixed, effectively constant), not the banned O(N) `scc::*::len`, is absent from `clippy.toml` disallowed-methods, and is a one-shot test assertion.

## Findings

### 1. Sync CPU-bound bootstrap helpers lack a spawn_blocking / bootstrap-only contract (pillar 2, letter)
- File:line: `src/tls.rs:28-38` (`generate_self_signed_server_cert`), `src/tls.rs:41-58` (`make_server_config_from_pem`)
- Severity: low
- Issue: Both are synchronous and CPU-bound (rcgen ECDSA P-256 key generation; PEM parse + rustls `ServerConfig` build). CLAUDE.md pillar 2 routes CPU-bound work through `tokio::task::spawn_blocking`. These are one-shot bootstrap by intent -- the doc comment says the caller "persists for reuse across restarts" -- but the bootstrap-only contract is not written down, and the crate's own e2e tests already call them inside `#[tokio::test]` bodies (`tests/echo_e2e.rs:177-178`, `tests/handshake_e2e.rs:131-132`, `tests/tls13_only.rs:101-102` and `133-134`), i.e. on a runtime thread.
- Failure scenario: none today (bootstrap-only). A future caller that regenerates a cert/config per rebind or per connection inside async code stalls every task sharing that worker thread (on an embedded `current_thread` runtime, the whole accept loop) for the duration of keygen + config build.
- Suggested fix: encode the contract in the doc comments ("bootstrap-only; if invoked at runtime, call from `tokio::task::spawn_blocking`"), or add `async` wrappers that internally `spawn_blocking` the keygen. No behavioral change needed for current callers.

### 2. e2e tests run Argon2id inline on the test runtime thread while a peer task is spawned (fragile, no live failure)
- File:line: `tests/echo_e2e.rs:376` and `tests/handshake_e2e.rs:278` (`hs.process_challenge(...)` -> Argon2id derive); pattern: default `#[tokio::test]` (current-thread) + `tokio::spawn`ed server task at `tests/echo_e2e.rs:196`, `tests/handshake_e2e.rs:144`
- Severity: nit
- Issue: `#[tokio::test]` defaults to a `current_thread` runtime; the client-side Argon2id KDF (~19 MB, tens of ms) executes inline and blocks the only thread, so the spawned server task cannot be polled during the derive. This is safe today only because the protocol dependency chain is strictly sequential (the server is parked awaiting a proof frame the client has not yet sent).
- Failure scenario: any refactor that makes the server need the thread during the client's blocking compute (server-side derive, a `tokio::time::timeout` wrapped around the server's `read_frame`, extra server tasks) would surface as a mysterious SLOW/TIMEOUT in nextest -- exactly the hang class CLAUDE.md mandates hunting to root cause -- with no single-task culprit visible.
- Suggested fix: switch these two tests to `#[tokio::test(flavor = "multi_thread")]`, or wrap the KDF step in `tokio::task::spawn_blocking` (which also matches pillar 2 and makes the tests independent of task-scheduling order). Test-only; no production code change.

No findings for the remaining theme items: no `scc::*::len()` anywhere in the crate (the single store-`len()` call is `DashMap::len`, constant over shards, test-only, not disallowed); no `std::sync::Mutex`/`RwLock`/`parking_lot` on any path; no locks held across `.await` (no locks exist); no hidden O(N)/O(N^2) helpers (per-frame costs are linear in frame bytes, which is inherent, and the O(1) `write_frame_prereserved` validation is explicitly commented).
