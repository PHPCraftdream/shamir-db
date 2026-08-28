# shamir-transport-ws — Concurrency & lock-free invariants

## Summary

The crate is lock-free by construction and pillar-compliant: reading all six source files plus a
crate-wide grep found zero `std::sync::Mutex`/`RwLock`/`parking_lot`, zero atomics, and no
`scc`/`dashmap`/hash-map surface at all (so there are no locks held across `.await` and no
`scc::*::len()` sites by definition) — all potentially shared state is avoided through ownership
(`&mut` free-fn parameters, `split()` halves), every I/O op is `async fn`, which is exactly the
pillar-1/2 shape. The only theme-relevant deviations are two low-severity O(x→0) items: the
send-framing path ships only the allocating variant while its TCP sibling (and this crate's own
recv side) established zero-alloc variants, and the browser accept path deep-clones the origin
allowlist per connection instead of sharing it via `Arc`. Test coverage of the concurrency
claims this crate makes — split-half duplex under real tokio tasks
(`tests/framing_round_trip.rs::split_halves_concurrent_send_recv`) and live accept-path wiring
(`src/tests/server_tests.rs`) — is present and adequate.

## Findings

### 1. Send framing ships only the allocating variant — production send path allocates + memcpys per message while recv is zero-alloc

- **File:line:** `crates/shamir-transport-ws/src/framing.rs:114-124` (parity claim in doc at
  `:62`); production consumer `crates/shamir-server/src/framer.rs:357` and `:412`
- **Severity:** low
- **Issue:** `ws_send_sink` builds a fresh `Vec::with_capacity(4 + payload.len())` and memcpys the
  entire payload on every call — a heap allocation plus full-payload copy per sent frame, i.e.
  "allocation in loops" on the per-message request/response path (pillar 3, O(x→0)). The framing
  doc (`framing.rs:62`) claims parity with `shamir-transport-tcp`'s Optim #7 single-syscall
  pattern, but the TCP sibling ships **three** variants (`shamir-transport-tcp/src/framing.rs:149`
  `write_frame`, `:187` `write_frame_into` caller-scratch, `:233` `write_frame_prereserved`
  zero-copy-encoding), while the WS send side exposes only the allocating one. The asymmetry
  within this crate is the sharpest statement of the gap: `ws_recv_into`/`ws_recv_into_stream`
  write into a caller scratch buffer (zero-alloc steady state, `framing.rs:92/132`), so a fully
  pooled per-connection duplex loop is impossible on WS — the recv half can be zero-alloc, the
  send half cannot. The real hot path routes through `shamir-server/src/framer.rs`, which calls
  `ws_send_sink(&mut self.0, payload)` per message, so every server-sent frame pays the
  allocation + memcpy.
- **Failure scenario:** none for correctness; the cost is one allocator round-trip plus a
  full-payload memcpy per frame on the send hot path — for multi-MB SELECT results that is an
  extra full-payload copy per message that a pre-reserved WS variant would avoid.
- **Suggested fix:** mirror the TCP trio where it maps onto tungstenite's owned
  `Message::Binary(Vec<u8>)` model: (a) add a pre-reserved variant — caller supplies the framed
  buffer (4-byte BE prefix + payload), the fn validates the prefix in O(1) exactly as
  `write_frame_prereserved` does, then `sink.send(Message::Binary(buf))`; this is the variant
  that actually removes the extra copy, since tungstenite copies into its internal write buffer
  at flush regardless. (b) optionally an owned-`Vec` variant that reserves and prepends the
  header in place (`Vec::splice(0..0, …)`) to drop the per-send allocation. Do **not** copy
  `write_frame_into`'s scratch-buffer signature blindly — `Message::Binary` takes ownership, so
  a pooled scratch would need a `clone()` (same memcpy, zero gain).

### 2. `accept_browser_ws` deep-clones the entire origin allowlist per accepted connection

- **File:line:** `crates/shamir-transport-ws/src/server.rs:118` (type at
  `crates/shamir-transport-ws/src/browser.rs:23-25`; caller
  `crates/shamir-server/src/server/server_launcher.rs:1266`)
- **Severity:** low
- **Issue:** `let policy = policy.clone();` deep-copies `BrowserOriginPolicy` (a `Vec<String>`:
  1 + P heap allocations, P = number of configured origins) on **every** accepted browser
  connection solely to move owned state into the `move` handshake callback — per-op state
  duplication for a value that is immutable for the handshake's lifetime. The pillars ask for
  read-shared config to move by refcount, not by deep copy (pillar 3 / pillar 5's
  single-writer-many-reader → `Arc` guidance; no lock is involved here).
- **Failure scenario:** none for correctness; wasted allocation + linear copy on the
  per-connection accept path — amortizes to nothing at low churn, visible at high connection
  churn and/or large operator allowlists.
- **Suggested fix:** take `policy: &Arc<BrowserOriginPolicy>` (or add an `accept_browser_ws_arc`
  overload) and clone the `Arc` into the closure — one atomic refcount bump instead of 1 + P
  allocations. The sole production caller (`server_launcher.rs:1196`) already builds the policy
  once at boot (`browser_origin_policy_from`, `:924-930`) and holds it per-listener, so switching
  that field to `Arc<BrowserOriginPolicy>` is a mechanical change. (If a compile check confirms
  tungstenite 0.24/0.29's `Callback` trait carries no `'static` bound, a directly borrowing
  closure would remove the clone entirely — but the `Arc` route is safe without build
  verification.)

No further findings for this theme: no `Mutex`/`RwLock`/`parking_lot` anywhere in the crate, no
locks (hence none across `.await`), no `scc`/`dashmap` maps (hence no `scc::*::len()` call sites
and no Fx-hash default to violate), and no sync I/O on async paths.
