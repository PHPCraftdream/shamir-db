<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-transport-tcp — performance-hotpath independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Allocation and retained-capacity mechanisms are visible, but documented pooling is intentional and costs are unmeasured. Server request ownership differs from the historical pooled-loop scenario.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 4 | 3 | 0 | 0 | 0 | 1 | 0 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — Full-size allocation on declared length before any payload byte arrives (16 MiB amplification per connection)

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Accepted lengths allocate upfront. Server pre-auth reads are 4 KiB and data reads are authenticated and deadline/count bounded; the Rust client's pre-auth reads instead allow 16 MiB. No resident-memory total is established.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:67](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L67); [crates/shamir-transport-tcp/src/framing.rs:124](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L124); [crates/shamir-server/src/connection/handshake.rs:717](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/handshake.rs#L717); [crates/shamir-client/src/client.rs:511](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L511).

Grouping/duplicate: [SUMMARY.md#4.1](SUMMARY.md#review-4-1). This is not an additional independent defect.

<a id="review-2"></a>

### Claim 2 — Pooled scratch buffers grow monotonically to the frame high-water mark; the documented `shrink_to_fit` mitigation is implemented by nobody

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Client retention is explicit and matches monotonic-capacity documentation. Server requests use fresh owned buffers and replies are separately owned. Automatic reclamation is an optional policy change, not a currently promised guarantee.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:89](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L89); [crates/shamir-client/src/client.rs:307](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L307); [crates/shamir-server/src/connection/request_loop.rs:278](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L278); [crates/shamir-server/src/connection/request_loop.rs:198](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L198).

Grouping/duplicate: [SUMMARY.md#4.2](SUMMARY.md#review-4-2). This is not an additional independent defect.

<a id="review-3"></a>

### Claim 3 — Two `read_exact` calls per frame: extra read round-trip on unbuffered plain-TCP streams

Status: `unverified`. Current risk: `low`.

Prior-cycle decision: `unverified`.

Two read stages are real. Their syscall ratio and latency effect remain platform/transport dependent and unmeasured; tokio-rustls 0.26.4 consumes buffered plaintext.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:50](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L50); [crates/shamir-transport-tcp/src/framing.rs:68](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L68); [Cargo.lock:4223](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4223).

Grouping/duplicate: [SUMMARY.md#4.3](SUMMARY.md#review-4-3). This is not an additional independent defect.

<a id="review-4"></a>

### Claim 4 — Allocating `read_frame`/`write_frame` remain the ergonomic defaults and are still on a production per-request hot path

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Client roundtrip uses write_frame and adds a Vec allocation/copy after serialization. Performance payoff from migration is not measured.

Evidence: [crates/shamir-client/src/client.rs:1278](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1278); [crates/shamir-transport-tcp/src/framing.rs:160](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L160).

Grouping/duplicate: [SUMMARY.md#4.4](SUMMARY.md#review-4-4). This is not an additional independent defect.

## Evidence and recipe corrections

- The pooled benchmark creates fresh scratch in each setup; it does not measure retaining one buffer across iterations.
- Its largest listed round-trip size is 1 MiB, not the documented 16 MiB ceiling.
- The echo fixture's pooled read loop is not the production concurrent server loop; do not transfer its allocation-free or approximately halved allocator-pressure claims.
- A single write_all can produce multiple underlying writes and TLS records. Payload-equivalence assertions are not record-count measurements.
- Capacity reclamation changes the explicitly documented monotonic-capacity behavior; make the policy deliberate and preferably opt-in.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-transport-tcp -- Performance & O(x->0)

## Summary

The crate is structurally clean against pillar 3: no locks anywhere (pillar 1 also holds), length-cap checks run *before* any allocation, the pooled `read_frame_into`/`write_frame_into`/`write_frame_prereserved` variants exist precisely to keep the per-frame hot path allocation-free, and the `reserve`-after-`clear()` ordering means buffer growth never memcpy-copies stale bytes (no hidden O(N²)). The remaining findings are memory-shape, not CPU: the read path allocates the full declared frame length upfront from a 4-byte header (16 MiB amplification per connection), and the pooled scratch buffers grow monotonically to the frame high-water mark with no shrink policy implemented by any consumer. No critical or high findings for this theme.

## Findings

### 1. Full-size allocation on declared length before any payload byte arrives (16 MiB amplification per connection)
- **File:line:** `crates/shamir-transport-tcp/src/framing.rs:67` (`read_frame`), `framing.rs:124-128` (`read_frame_into`)
- **Severity:** medium
- **Issue:** Both read functions reserve/allocate the declared frame length immediately after reading the 4-byte prefix and the `max_frame_size` check — `vec![0u8; len]` / `reserve(len)` + `set_len(len)`. Nothing verifies that the peer actually delivers the bytes; the allocation is fully committed from 4 bytes of attacker- or bug-controlled input. The cap (`MAX_FRAME_SIZE_DEFAULT` = 16 MiB) bounds a *single* frame, not the aggregate: the framing layer has no per-connection memory accounting and no staged growth (grow-as-bytes-arrive).
- **Failure scenario:** 1,000 open connections each send only a length prefix of `0x00FF_FFFF` (16 MiB − 1) and then trickle or stall. That is ~16 GiB resident from 4 KB of wire input, held for as long as each `read_exact` waits. `shamir-server`'s connection limiter bounds connection count, not bytes-buffered-per-connection, so the product of the two is unguarded at this layer.
- **Suggested fix:** Stage the allocation: read into a reusable buffer that grows only as bytes actually arrive (e.g. start at a small soft cap such as `shamir_tunables::IO_FRAME_BUFFER_CAP` (4096), double while reading, abort with `TooLarge`/`Io` if the peer under-delivers), or add an explicit `max_prealloc` below `max_frame_size` that switches slow paths to incremental reads. Cheaper alternative: keep the upfront alloc but document that deployments must pair this transport with per-connection buffered-byte caps.

### 2. Pooled scratch buffers grow monotonically to the frame high-water mark; the documented `shrink_to_fit` mitigation is implemented by nobody
- **File:line:** `crates/shamir-transport-tcp/src/framing.rs:89-91` (doc promise), `framing.rs:117-124` (`clear()` + `reserve()`), also `framing.rs:168-173` (`write_frame_into` scratch)
- **Severity:** medium
- **Issue:** `read_frame_into`/`write_frame_into` deliberately keep capacity at the high-water mark of frames seen ("capacity grows monotonically... Use `Vec::shrink_to_fit` periodically if memory is a concern"). A repo-wide grep shows the *only* occurrence of `shrink_to_fit` in the workspace is that doc line — no consumer (`shamir-server/src/connection/request_loop.rs`, `shamir-server/src/framer.rs`, `shamir-client/src/client.rs`) ever shrinks. The buffers start at `IO_FRAME_BUFFER_CAP` = 4096, so the retention is invisible in normal traffic and only surfaces after one large frame.
- **Failure scenario:** A connection serves one 16 MiB SELECT result; its read buffer pins ≥16 MiB and its write scratch up to another 16 MiB for the connection's remaining lifetime, even if every subsequent frame is 100 B. A fleet of long-lived connections that each once touched a large result retains ~32 MiB × N indefinitely — memory that looks like a leak in RSS monitoring and never returns.
- **Suggested fix:** Own the policy inside the crate instead of deferring to callers: add hysteresis to `read_frame_into`/`write_frame_into` (e.g. `if buf.capacity() > HIGH_WATER * 2 && buf.capacity() > SHRINK_FLOOR { buf.shrink_to_fit(); }` on frame completion or on idle), or introduce a `FrameBuf` newtype encapsulating grow/shrink. At minimum, add the shrink call at the two real consumer sites and a regression test pinning the policy (no test currently covers growth/retention behavior).

### 3. Two `read_exact` calls per frame: extra read round-trip on unbuffered plain-TCP streams
- **File:line:** `crates/shamir-transport-tcp/src/framing.rs:49-50` (`read_frame`), `framing.rs:103-104` (`read_frame_into`)
- **Severity:** low
- **Issue:** The 4-byte length prefix is read with its own `read_exact` before the payload read. On `tokio-rustls` streams this is absorbed by rustls' internal plaintext buffer (no extra syscall once a record is decrypted), but on the sanctioned `ListenerProfile::Plain` loopback path and any raw `TcpStream`/unbuffered reader, every frame costs at least two read syscalls, and payload bytes coalesced into the same TCP segment are left in the kernel buffer rather than consumed by the header read. There is no `BufReader` guidance or helper anywhere in the crate.
- **Failure scenario:** Loopback/plain deployments (the documented same-host embedded use case) pay a ~2× syscall overhead per frame in the request loop; at small frame sizes the fixed cost is a measurable fraction of per-op latency.
- **Suggested fix:** Either document that plain-profile consumers must wrap the stream in `tokio::io::BufReader`, or provide a buffered variant that peeks/consumes the header and payload from one buffered read (e.g. `read_buf`-based header fill that can carry into the payload read).

### 4. Allocating `read_frame`/`write_frame` remain the ergonomic defaults and are still on a production per-request hot path
- **File:line:** `crates/shamir-transport-tcp/src/framing.rs:45-70` (`read_frame`, fresh `Vec` per call), `framing.rs:149-166` (`write_frame`, 4+N `Vec` per call); consumer evidence: `crates/shamir-client/src/client.rs:1004` (per-request `write_frame` in the send loop)
- **Severity:** low
- **Issue:** The crate correctly documents the allocating variants as the non-hot-path choice and ships pooled alternatives; the server (`shamir-server` request loop, framer, handshake) uses the pooled/prereserved APIs throughout. The client, however, calls the allocating `write_frame` once per request on its send path (one heap allocation + memcpy of the full envelope per request, on top of the msgpack serialization allocation). Within this crate the API design permits it silently — nothing marks the allocating variants as non-hot-path beyond prose.
- **Failure scenario:** Sustained client request streams keep a per-request malloc/free (and for large envelopes, a full extra payload memcpy) that the server side already eliminated — asymmetric hot-path cost, and allocator contention under multi-connection clients.
- **Suggested fix:** Migrate `shamir-client`'s request write path to `write_frame_into` with a per-connection scratch (mirroring the server's request loop), and/or mark the allocating variants `#[doc(hidden)]`-adjacent guidance ("do not use in per-frame loops") so future consumers reach for the pooled API by default. (Primary remediation lives in shamir-client; recorded here because the API surface is this crate's.)

## Theme notes (no finding, for the record)

- Positive: cap check (`len > max_frame_size`) precedes every allocation (`framing.rs:56-61`, `framing.rs:110-115`); `TooLarge` rejects without touching the buffer (covered by tests).
- Positive: no `Mutex`/`RwLock`/`scc::len()` anywhere in the crate — pillar 1/3 compliant.
- Positive: `write_frame_prereserved`'s O(1) contract check is correctly argued as negligible vs. the I/O cost (`framing.rs:237-255`).
- Test coverage for this theme is good: `tests/framing.rs` pins capacity reuse, single-write wire equivalence, and uninit-safety (Miri-safe no-IO-driver tests); `benches/framing.rs` follows the `bench_scale_tool::Harness` convention and covers both allocating and pooled variants. The only uncovered behavior is the growth/retention policy (finding 2) — nothing asserts or bounds capacity high-water behavior.

</details>
