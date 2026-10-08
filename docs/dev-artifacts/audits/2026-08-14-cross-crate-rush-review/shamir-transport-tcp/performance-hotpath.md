<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-transport-tcp — performance-hotpath revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Upfront allocation, persistent client buffer capacity and allocating client writes remain source-proven. The claimed universal syscall ratio, RSS totals and server lifetime retention require correction or measurement.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 4 | 3 | 0 | 0 | 0 | 1 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — Full-size allocation on declared length before any payload byte arrives (16 MiB amplification per connection)

Status: `confirmed-open`. Current risk: `medium`.

Both readers allocate/reserve the accepted declared length before reading payload bytes. Production unauthenticated reads are capped at 4 KiB; 16 MiB allocation is post-authentication and subject to connection limits and an idle deadline. Reserved capacity is not a measured RSS total.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:67](../../../../../crates/shamir-transport-tcp/src/framing.rs#L67); [crates/shamir-transport-tcp/src/framing.rs:124](../../../../../crates/shamir-transport-tcp/src/framing.rs#L124); [crates/shamir-server/src/connection/handshake.rs:714](../../../../../crates/shamir-server/src/connection/handshake.rs#L714); [crates/shamir-server/src/connection/request_loop.rs:280](../../../../../crates/shamir-server/src/connection/request_loop.rs#L280); [crates/shamir-server/src/connection/request_loop.rs:297](../../../../../crates/shamir-server/src/connection/request_loop.rs#L297).

Grouping/duplicate: `SUMMARY.md#4.1`. This row is not another independent defect.

<a id="review-2"></a>

### Claim 2 — Pooled scratch buffers grow monotonically to the frame high-water mark; the documented `shrink_to_fit` mitigation is implemented by nobody

Status: `confirmed-open`. Current risk: `medium`.

Pooled helpers retain capacity without shrinking, and the client reader keeps one buffer for its task lifetime. The server request loop instead allocates a fresh request buffer and writes separately owned prereserved replies, refuting the stated universal 32 MiB server lifetime-retention example.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:89](../../../../../crates/shamir-transport-tcp/src/framing.rs#L89); [crates/shamir-transport-tcp/src/framing.rs:199](../../../../../crates/shamir-transport-tcp/src/framing.rs#L199); [crates/shamir-client/src/client.rs:312](../../../../../crates/shamir-client/src/client.rs#L312); [crates/shamir-server/src/connection/request_loop.rs:278](../../../../../crates/shamir-server/src/connection/request_loop.rs#L278); [crates/shamir-server/src/connection/request_loop.rs:310](../../../../../crates/shamir-server/src/connection/request_loop.rs#L310); [crates/shamir-server/src/connection/request_loop.rs:198](../../../../../crates/shamir-server/src/connection/request_loop.rs#L198).

Grouping/duplicate: `SUMMARY.md#4.2`. This row is not another independent defect.

<a id="review-3"></a>

### Claim 3 — Two `read_exact` calls per frame: extra read round-trip on unbuffered plain-TCP streams

Status: `unverified`. Current risk: `low` (provisional; not a confirmed defect).

Separate header/payload read operations and missing BufReader guidance are confirmed. A universal syscall count, approximately doubled syscall overhead and measurable latency impact are not established by source inspection or supplied measurements.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:50](../../../../../crates/shamir-transport-tcp/src/framing.rs#L50); [crates/shamir-transport-tcp/src/framing.rs:68](../../../../../crates/shamir-transport-tcp/src/framing.rs#L68); [crates/shamir-transport-tcp/src/framing.rs:104](../../../../../crates/shamir-transport-tcp/src/framing.rs#L104); [crates/shamir-transport-tcp/src/framing.rs:129](../../../../../crates/shamir-transport-tcp/src/framing.rs#L129).

Grouping/duplicate: `SUMMARY.md#4.3`. This row is not another independent defect.

<a id="review-4"></a>

### Claim 4 — Allocating `read_frame`/`write_frame` remain the ergonomic defaults and are still on a production per-request hot path

Status: `confirmed-open`. Current risk: `low`.

Client roundtrip still calls allocating write_frame per request. That helper allocates a combined prefix/payload Vec and copies the payload. No latency or allocator-contention improvement has been measured here.

Evidence: [crates/shamir-client/src/client.rs:1278](../../../../../crates/shamir-client/src/client.rs#L1278); [crates/shamir-transport-tcp/src/framing.rs:160](../../../../../crates/shamir-transport-tcp/src/framing.rs#L160); [crates/shamir-transport-tcp/src/framing.rs:162](../../../../../crates/shamir-transport-tcp/src/framing.rs#L162).

Grouping/duplicate: `SUMMARY.md#4.4`. This row is not another independent defect.

## Corrections and qualified non-findings

- 16 GiB resident from 4 KiB of headers is not source-proven: reservation, physical residency and allocator/OS behavior differ. The server also enforces pre-authentication 4 KiB limits and connection/read deadlines.
- Server request buffers are not pooled across requests; do not claim its per-request reads are allocation-free or its read/write scratch universally persists for the connection lifetime.
- Capacity-reuse tests exist, but they do not test large-growth reclamation. The existing read test explicitly requires unchanged capacity for small frames at crates/shamir-transport-tcp/tests/framing.rs:116.
- The clear-before-reserve ordering and constant-size prereserved validation are source-supported; negligible latency is not a measurement.
- Wire-equivalence tests compare decoded payloads, not underlying write-call counts or TLS-record counts; one write_all may perform multiple underlying writes.
- The benchmark uses the mandated harness, but pooled scratch is recreated in each setup at crates/shamir-transport-tcp/benches/framing.rs:63, despite comments claiming cross-iteration reuse. It does not prove persistent-buffer steady-state behavior.
- Miri execution and soundness are not established by tests named Miri-safe.

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
