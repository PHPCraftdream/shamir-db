<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-transport-ws — concurrency-lockfree independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Allocation and clone mechanisms are confirmed, but neither is a measured throughput regression. Crate-owned synchronization is absent; deployed split halves use dependency synchronization and the current test is not simultaneous bidirectional traffic.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 2 | 2 | 0 | 0 | 0 | 0 | 0 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — Send framing ships only the allocating variant — production send path allocates + memcpys per message while recv is zero-alloc

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The WS writer rebuilds a prefixed Vec from borrowed payload bytes. Receive reuse applies only to fitting caller scratch; exact tungstenite 0.24.0 allocates frame/message storage and production allocates fresh request Vecs.

Evidence: [crates/shamir-transport-ws/src/framing.rs:119](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/framing.rs#L119); [crates/shamir-server/src/framer.rs:123](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/framer.rs#L123); [crates/shamir-server/src/framer.rs:357](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/framer.rs#L357); [crates/shamir-server/src/connection/request_loop.rs:278](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L278).

Grouping/duplicate: [performance-hotpath.md#1](performance-hotpath.md#review-1). This is not an additional independent defect.

<a id="review-2"></a>

### Claim 2 — `accept_browser_ws` deep-clones the entire origin allowlist per accepted connection

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Derived Clone copies Vec&lt;String&gt; in the launcher and again in the transport. The exact callback permits a borrow, while Arc sharing can remove the launcher copy. Cost depends on configured policy size and connection churn.

Evidence: [crates/shamir-transport-ws/src/browser.rs:22](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/browser.rs#L22); [crates/shamir-transport-ws/src/server.rs:118](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/server.rs#L118); [crates/shamir-server/src/server/server_launcher.rs:1472](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L1472); [Cargo.lock:4467](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4467).

## Evidence and recipe corrections

- futures-util 0.3.32 SplitStream and SplitSink both poll a BiLock; full-stack lock freedom is positively contradicted, not merely unverified. The guard is released after each poll, so this does not demonstrate a lock held across an entire await. Source: https://docs.rs/crate/futures-util/0.3.32/source/src/stream/stream/split.rs.
- split_halves_concurrent_send_recv never sends a server payload while reading one. Its writer is the peer, and server_sink only closes afterward.
- Ownership-taking send changes must propagate through the borrowed FrameWriter interface and preserve response-budget guard lifetime until completion or failure.
- A borrowed callback is supported by the exact 0.24.0 sources; an explicit lifetime annotation or new Arc-specific public accept API is not inherently necessary.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

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

</details>
