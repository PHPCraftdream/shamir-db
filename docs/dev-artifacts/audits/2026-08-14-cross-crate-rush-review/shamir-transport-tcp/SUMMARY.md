<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-transport-tcp — SUMMARY independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

The initialized-buffer violation remains the primary transport defect. Most other rows describe genuine documentation, diagnostic or test gaps, or optional API/performance choices. Additional caller inspection disproves a complete pre-authentication lifetime-bound assurance.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 30 | 29 | 0 | 0 | 0 | 1 | 0 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1-1"></a>

### Claim 1.1 — Zero-length payload write silently emits the close sentinel; module doc contradicts spec §2

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

write_frame([]), write_frame_into([]), and a four-zero-byte prereserved buffer emit close; readers cannot return empty data. The false empty-data documentation remains. Current msgpack callers produce nonempty payloads; exclusive use of write_close is not itself a protocol requirement.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:8](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L8); [crates/shamir-transport-tcp/src/framing.rs:53](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L53); [crates/shamir-transport-tcp/src/framing.rs:153](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L153); [crates/shamir-transport-tcp/src/framing.rs:250](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L250).

<a id="review-1-2"></a>

### Claim 1.2 — `tls.rs` has zero unit tests; all error branches unexercised (happy-path-only e2e coverage)

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Only listener unit tests are registered. Integration tests do not assert malformed PEM, missing PKCS8, key/certificate mismatch or incomplete exporter causes. Server half-present-file coverage exits before calling the TLS constructor. Exact cause assertions must catch accepting malformed input, not merely any setup error.

Evidence: [crates/shamir-transport-tcp/src/tests/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tests/mod.rs#L1); [crates/shamir-transport-tcp/src/tls.rs:45](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L45); [crates/shamir-server/src/tests/tls_tests.rs:33](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/tests/tls_tests.rs#L33).

<a id="review-1-3"></a>

### Claim 1.3 — Test name promises TLS-bind-on-unspecified coverage the body deliberately doesn't deliver

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The named test binds loopback. A mutation rejecting unspecified TLS binds only inside bind_validated remains undetected despite the separate allows predicate assertions.

Evidence: [crates/shamir-transport-tcp/src/tests/listener_tests.rs:58](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tests/listener_tests.rs#L58); [crates/shamir-transport-tcp/src/tests/listener_tests.rs:101](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tests/listener_tests.rs#L101).

<a id="review-1-4"></a>

### Claim 1.4 — Write-path boundary branches, EOF-vs-close distinction, and IPv4-mapped-IPv6 edge untested

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

No writer oversize or mapped-address case is registered. Truncated-payload coverage checks is_err and clearing, so changing UnexpectedEof into PeerClose would pass. A valid matching prefix above the writer cap is needed to isolate removal of the cap clause.

Evidence: [crates/shamir-transport-tcp/tests/framing.rs:226](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/tests/framing.rs#L226); [crates/shamir-transport-tcp/tests/framing.rs:240](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/tests/framing.rs#L240); [crates/shamir-transport-tcp/tests/framing.rs:309](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/tests/framing.rs#L309); [crates/shamir-transport-tcp/src/tests/listener_tests.rs:14](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tests/listener_tests.rs#L14).

<a id="review-2-1"></a>

### Claim 2.1 — Sync CPU-bound bootstrap helpers lack a spawn_blocking / bootstrap-only contract

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The helpers are synchronous and lack scheduling guidance. The actual server caller initializes TLS before launching listeners; no per-connection use or measured problematic stall is established.

Evidence: [crates/shamir-transport-tcp/src/tls.rs:28](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L28); [crates/shamir-transport-tcp/src/tls.rs:41](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L41); [crates/shamir-server/src/server/server_launcher.rs:601](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L601).

<a id="review-2-2"></a>

### Claim 2.2 — e2e tests run Argon2id inline on the test runtime thread while a peer task is spawned

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Default current-thread tests call synchronous process_challenge. The peer is awaiting the proof, so this establishes worker occupancy, not a current deadlock or future timeout inevitability.

Evidence: [crates/shamir-transport-tcp/tests/handshake_e2e.rs:113](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/tests/handshake_e2e.rs#L113); [crates/shamir-transport-tcp/tests/handshake_e2e.rs:278](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/tests/handshake_e2e.rs#L278); [crates/shamir-transport-tcp/tests/echo_e2e.rs:376](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/tests/echo_e2e.rs#L376); [crates/shamir-connect/src/client/handshake.rs:231](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/client/handshake.rs#L231).

<a id="review-3-1"></a>

### Claim 3.1 — `read_frame_into`: unsafe `set_len` before `.await` — formal UB on uninit memory, and cancellation leaves `buf.len()` covering uninitialized bytes

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

For a fresh Vec and positive accepted length, reserve/set_len falsely declares initialized bytes before Tokio 1.49.0 ReadBuf::new. A safe reader may inspect initialized(); dropping a pending payload read leaves the enlarged buffer observable. Server cancellation discards its buffers, so remote disclosure is not proven. Completed-read tests miss both witnesses.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:124](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L124); [crates/shamir-transport-tcp/src/framing.rs:127](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L127); [crates/shamir-transport-tcp/src/framing.rs:129](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L129); [Cargo.lock:4195](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4195); [crates/shamir-server/src/framer.rs:224](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/framer.rs#L224).

<a id="review-3-2"></a>

### Claim 3.2 — `NoCaVerify` disables all server authentication — chain, CertificateVerify proof-of-possession, and hostname — guarded only by a doc comment

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Both certificate and TLS13 signature callbacks return assertions. rustls 0.23.37 client/tls13.rs delegates CertificateVerify validation to this callback. However, production Client::connect performs independent exporter-bound protocol authentication; bypassing CA/hostname is intentional, while omitting proof-of-possession and insufficient helper obligations remain.

Evidence: [crates/shamir-transport-tcp/src/tls.rs:138](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L138); [crates/shamir-transport-tcp/src/tls.rs:158](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L158); [Cargo.lock:3048](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3048); [crates/shamir-client/src/client.rs:614](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L614); [crates/shamir-connect/src/client/handshake.rs:258](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/client/handshake.rs#L258).

<a id="review-3-3"></a>

### Claim 3.3 — `extract_tls_exporter` collapses all errors (incl. incomplete handshake) into `None`, inviting constant/absent channel binding

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The error is erased, and server TCP/native-WS callers substitute zeros. They run after successful TLS handshakes; the normal pinned established exporter path provides no demonstrated failure trigger. Client::connect fails closed. Fresh nonces invalidate the simple recorded-proof replay scenario.

Evidence: [crates/shamir-transport-tcp/src/tls.rs:81](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L81); [crates/shamir-server/src/server/server_launcher.rs:1163](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L1163); [crates/shamir-server/src/server/server_launcher.rs:1391](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L1391); [crates/shamir-client/src/client.rs:480](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L480); [crates/shamir-connect/src/common/auth_message.rs:87](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/common/auth_message.rs#L87).

<a id="review-4-1"></a>

### Claim 4.1 — Full-size allocation on declared length before any payload byte arrives (16 MiB amplification per connection)

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Accepted headers cause full allocation/reservation before payload arrival. Server 16 MiB reads require authentication and have connection/in-flight limits and a whole-frame deadline; server pre-auth reads use 4 KiB. Client pre-auth reads are an exception. Capacity multiplication is source-proven, RSS totals are not.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:67](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L67); [crates/shamir-transport-tcp/src/framing.rs:124](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L124); [crates/shamir-server/src/connection/handshake.rs:717](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/handshake.rs#L717); [crates/shamir-server/src/connection/request_loop.rs:297](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L297); [crates/shamir-client/src/client.rs:511](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L511).

<a id="review-4-2"></a>

### Claim 4.2 — Pooled scratch buffers grow monotonically to the frame high-water mark; the documented `shrink_to_fit` mitigation is implemented by nobody

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The client reader intentionally retains one Vec until task exit, matching the documented monotonic-capacity contract. This is a capacity-policy tradeoff, not an accidental leak or violated reclamation guarantee. Server request buffers are fresh and transferred to dispatch tasks.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:89](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L89); [crates/shamir-client/src/client.rs:307](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L307); [crates/shamir-client/src/client.rs:312](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L312); [crates/shamir-server/src/connection/request_loop.rs:278](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L278); [crates/shamir-server/src/connection/request_loop.rs:310](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L310).

<a id="review-4-3"></a>

### Claim 4.3 — Two `read_exact` calls per frame: extra read round-trip on unbuffered plain-TCP streams

Status: `unverified`. Current risk: `low`.

Prior-cycle decision: `unverified`.

There are two read stages, but their syscall and latency cost depends on stream implementation, readiness, fragmentation and platform. tokio-rustls 0.26.4 buffers plaintext. No universal ratio or measured improvement follows.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:50](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L50); [crates/shamir-transport-tcp/src/framing.rs:68](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L68); [Cargo.lock:4223](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4223).

<a id="review-4-4"></a>

### Claim 4.4 — Allocating `read_frame`/`write_frame` remain the ergonomic defaults and are still on a production per-request hot path

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Client::roundtrip calls write_frame after envelope serialization, introducing another combined-buffer allocation and payload copy. The cost mechanism is real; contention or latency payoff is unmeasured.

Evidence: [crates/shamir-client/src/client.rs:1278](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1278); [crates/shamir-transport-tcp/src/framing.rs:160](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L160); [crates/shamir-transport-tcp/src/framing.rs:162](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L162).

<a id="review-5-1"></a>

### Claim 5.1 — Write-side frame cap is hardcoded to `MAX_FRAME_SIZE_DEFAULT`; reader/writer API asymmetry

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

All writers enforce 16 MiB while readers accept a supplied cap. No negotiated limit is implemented. Capped writer companions are an API enhancement; this asymmetry alone does not prove an emitted production handshake exceeds 4 KiB.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:47](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L47); [crates/shamir-transport-tcp/src/framing.rs:153](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L153); [crates/shamir-transport-tcp/src/framing.rs:192](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L192); [crates/shamir-transport-tcp/src/framing.rs:250](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L250).

<a id="review-5-2"></a>

### Claim 5.2 — Normative loopback predicate is not reusable, so shamir-server duplicates spec §2.2 policy

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The private transport helper and server helper perform identical standard-library classification. Public allows/validate_addr are already reusable with profile translation. Sharing the primitive is optional maintenance work, not a current whitelist divergence.

Evidence: [crates/shamir-transport-tcp/src/listener.rs:40](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/listener.rs#L40); [crates/shamir-transport-tcp/src/listener.rs:51](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/listener.rs#L51); [crates/shamir-server/src/config.rs:873](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/config.rs#L873).

<a id="review-5-3"></a>

### Claim 5.3 — Docs advertise Unix-domain-socket binds for `Plain`; the API is `SocketAddr`/TCP-only

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

UDS is permitted by the broader protocol, but these functions accept SocketAddr and return TcpListener. Clarification is sufficient; no new UDS stub is necessary.

Evidence: [crates/shamir-transport-tcp/src/listener.rs:30](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/listener.rs#L30); [crates/shamir-transport-tcp/src/listener.rs:75](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/listener.rs#L75).

<a id="review-5-4"></a>

### Claim 5.4 — Exporter doc cluster misdescribes the adapter abstraction

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The function names the wrong trait, and standalone rustls connections have no local implementations. Conversely, explicit impls do make both tokio-rustls stream types implement ConnectionExporter; the WS helper simply forwards.

Evidence: [crates/shamir-transport-tcp/src/tls.rs:73](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L73); [crates/shamir-transport-tcp/src/tls.rs:88](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L88); [crates/shamir-transport-tcp/src/tls.rs:98](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L98); [crates/shamir-transport-ws/src/tls_exporter.rs:20](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/tls_exporter.rs#L20).

<a id="review-5-5"></a>

### Claim 5.5 — `tokio` dependency pulls `features = ["full"]` in a library crate

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The manifest enables full. A narrower feature declaration is optional; workspace/downstream feature unification can retain the same capabilities and no size or compile-time savings were measured.

Evidence: [crates/shamir-transport-tcp/Cargo.toml:14](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/Cargo.toml#L14).

<a id="review-5-6"></a>

### Claim 5.6 — Transport wire structs and fixtures duplicated across the two e2e test files

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Four Wire types, fast_kdf and make_user are independently copied. Shared fixtures reduce drift but do not create an independent protocol oracle; both e2e files use handwritten peers rather than the production launcher/client.

Evidence: [crates/shamir-transport-tcp/tests/handshake_e2e.rs:41](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/tests/handshake_e2e.rs#L41); [crates/shamir-transport-tcp/tests/echo_e2e.rs:57](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/tests/echo_e2e.rs#L57).

<a id="review-6-1"></a>

### Claim 6.1 — `write_frame_prereserved` reports every contract violation as `TooLarge`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

A three-byte buffer yields actual=0 with a positive maximum; prefix=999 with 100 payload bytes also produces a false greater-than diagnostic. Tests pin this classification and the TCP adapter forwards it. No wire corruption is emitted by these rejected calls.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:241](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L241); [crates/shamir-transport-tcp/src/framing.rs:250](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L250); [crates/shamir-transport-tcp/tests/framing.rs:315](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/tests/framing.rs#L315); [crates/shamir-server/src/framer.rs:317](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/framer.rs#L317).

<a id="review-6-2"></a>

### Claim 6.2 — `Box<dyn Error + Send + Sync>` in library APIs instead of a `thiserror` enum

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Both TLS constructors retain boxed errors and a string missing-key case. Concrete causes can be downcast; server stringification is its own choice. An enumerable typed surface improves diagnostics/convention conformance, but boxing at this boundary is not a medium runtime defect.

Evidence: [crates/shamir-transport-tcp/src/tls.rs:30](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L30); [crates/shamir-transport-tcp/src/tls.rs:44](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L44); [crates/shamir-transport-tcp/src/tls.rs:50](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L50); [crates/shamir-server/src/tls.rs:116](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/tls.rs#L116).

<a id="review-6-3"></a>

### Claim 6.3 — TLS config builders can panic on missing/ambiguous rustls crypto provider

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

rustls 0.23.37 auto-installs the unambiguous AWS-LC default. Its crypto/mod.rs and client/client_conn.rs retain panic paths for ambiguous/custom-provider selection without installation or incompatible provider suites. No current-default boot failure is established.

Evidence: [crates/shamir-transport-tcp/src/tls.rs:54](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L54); [crates/shamir-transport-tcp/src/tls.rs:64](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L64); [crates/shamir-transport-tcp/Cargo.toml:18](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/Cargo.toml#L18); [Cargo.lock:3048](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3048).

<a id="review-6-4"></a>

### Claim 6.4 — Post-error stream state is undocumented (`TooLarge` desyncs the stream)

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

TooLarge consumes the header but not its declared body; retrying interprets body bytes as another prefix. Other errors may consume zero or partial bytes. The server breaks on read errors, but transport documentation omits the conservative discard obligation.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:50](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L50); [crates/shamir-transport-tcp/src/framing.rs:56](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L56); [crates/shamir-server/src/connection/request_loop.rs:283](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L283).

<a id="review-6-5"></a>

### Claim 6.5 — Cancel-safety contract lives in the consumer, not the crate

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Tokio 1.49.0 read_exact may consume bytes before cancellation. Cancelling after part of a header and recreating the frame future loses framing position. Server select cancellation is terminal; neither public reader documents this general obligation.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:45](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L45); [crates/shamir-transport-tcp/src/framing.rs:98](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L98); [Cargo.lock:4195](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4195); [crates/shamir-server/src/connection/request_loop.rs:231](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L231).

<a id="review-7-1"></a>

### Claim 7.1 — Crate-root re-exports are partial, stale, and used by nobody

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Root exports omit pooled helpers and several TLS items, but public module paths remain usable and repository callers use them. No requirement mandates complete root exports; this is discoverability preference, not a defective public path.

Evidence: [crates/shamir-transport-tcp/src/lib.rs:8](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/lib.rs#L8); [crates/shamir-transport-tcp/src/lib.rs:12](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/lib.rs#L12); [crates/shamir-client/src/client.rs:39](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L39).

<a id="review-7-2"></a>

### Claim 7.2 — Function-local `use` statements violate the imports-at-top rule (5 sites)

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Six use statements occur in four bodies without a documented scope exception. This is convention-only.

Evidence: [crates/shamir-transport-tcp/tests/framing.rs:50](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/tests/framing.rs#L50); [crates/shamir-transport-tcp/tests/framing.rs:155](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/tests/framing.rs#L155); [crates/shamir-transport-tcp/tests/tls13_only.rs:36](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/tests/tls13_only.rs#L36); [crates/shamir-transport-tcp/tests/echo_e2e.rs:208](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/tests/echo_e2e.rs#L208).

<a id="review-7-3"></a>

### Claim 7.3 — Speculative dead public consts with cargo-cult `#[allow(dead_code)]`

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The public loopback constants have no repository Rust consumers and their allowances are redundant. Public reachability means deleting them is an API change, not removal of unreachable implementation.

Evidence: [crates/shamir-transport-tcp/src/listener.rs:96](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/listener.rs#L96).

<a id="review-7-4"></a>

### Claim 7.4 — In-src `tests/` layout covers only one of three modules

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The crate-root unit manifest includes listener_tests only; framing contract tests are valid integration targets. Layout consistency is separate from coverage, though default --lib selection excludes these framing tests.

Evidence: [crates/shamir-transport-tcp/src/tests/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tests/mod.rs#L1); [crates/shamir-transport-tcp/tests/framing.rs:203](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/tests/framing.rs#L203); [scripts/test.sh:177](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/scripts/test.sh#L177).

<a id="review-7-5"></a>

### Claim 7.5 — Spec wire constant duplicated from a direct dependency

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The exporter labels currently agree byte-for-byte. Duplication is prospective maintenance risk, not a present binding mismatch; both same-helper peers can remain green after a shared wrong-label mutation.

Evidence: [crates/shamir-transport-tcp/src/tls.rs:18](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L18); [crates/shamir-connect/src/common/domain_tags.rs:35](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/common/domain_tags.rs#L35).

<a id="review-7-6"></a>

### Claim 7.6 — Mixed spellings of the same upstream crate in one import block

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Direct rustls_pki_types and rustls::pki_types paths refer to the same locked package. Mixed spelling is stylistic, not a type incompatibility.

Evidence: [crates/shamir-transport-tcp/src/tls.rs:10](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L10); [crates/shamir-transport-tcp/src/tls.rs:12](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L12); [Cargo.lock:3063](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3063).

## Revalidated plan decisions

| Plan decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 16 | 16 | 0 | 0 | 0 | 0 | 0 |

Historical P0/P1/P2 numbering is an identifier, not a current release mandate. The reasons below include completion status, safety qualifications and discriminating acceptance requirements.

<a id="plan-p0-1"></a>

### Plan P0.1 — P0.1

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Not implemented. Initialized resize is a straightforward safe baseline; alternatively read through ReadBuf::uninit over spare capacity and expose length only after initialization. A Drop guard alone cannot repair the read_exact precondition. Preserve complete-frame success and error-buffer behavior; add legal-reader observation and pending-read cancellation oracles.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:124](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L124); [crates/shamir-transport-tcp/src/framing.rs:129](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L129); [crates/shamir-transport-tcp/tests/framing.rs:203](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/tests/framing.rs#L203).

<a id="plan-p0-2"></a>

### Plan P0.2 — P0.2

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

No rejection or documentation correction landed. Rejecting empty generic writes is an API behavior change, not the only conforming solution: documenting and testing their close semantics also resolves the false promise. If rejection is chosen, validate all three writers before I/O and keep write_close functional.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:8](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L8); [crates/shamir-transport-tcp/src/framing.rs:153](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L153); [crates/shamir-transport-tcp/src/framing.rs:209](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L209); [crates/shamir-transport-tcp/src/framing.rs:250](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L250).

<a id="plan-p0-3"></a>

### Plan P0.3 — P0.3

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Typed errors and explicit fallible provider builders remain absent. The recipe must not reject an absent preinstalled default when pinned rustls can legitimately auto-select AWS-LC. Preserve provider selection and update client/server consumers for signature changes.

Evidence: [crates/shamir-transport-tcp/src/tls.rs:30](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L30); [crates/shamir-transport-tcp/src/tls.rs:54](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L54); [crates/shamir-transport-tcp/src/tls.rs:63](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L63); [Cargo.lock:3048](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3048).

<a id="plan-p1-4"></a>

### Plan P1.4 — P1.4

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Full declared-length allocation remains. Incremental growth must stay capped, initialize storage safely, retain exact-frame semantics and make expiry terminal. Deployment guidance alone accepts/manages the allocation risk; it does not remove the mechanism. Address the newly identified missing proof-stage deadline separately.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:67](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L67); [crates/shamir-transport-tcp/src/framing.rs:124](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L124); [crates/shamir-server/src/connection/handshake.rs:291](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/handshake.rs#L291).

<a id="plan-p1-5"></a>

### Plan P1.5 — P1.5

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

No reclamation policy exists. Automatic shrinking would change the documented monotonic-capacity behavior; prefer an explicit caller policy or opt-in buffer wrapper. Apply it to the persistent client buffer, not nonexistent server connection scratch, and test large-to-small transitions without asserting RSS release.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:89](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L89); [crates/shamir-client/src/client.rs:307](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L307); [crates/shamir-transport-tcp/tests/framing.rs:116](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/tests/framing.rs#L116).

<a id="plan-p1-6"></a>

### Plan P1.6 — P1.6

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Malformed-prefix variants are absent. The recipe is sound if it preserves pre-I/O rejection and genuine TooLarge classification and updates TCP error mapping. Default Framer prechecks still serve non-TCP implementations and are not universally redundant.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:241](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L241); [crates/shamir-transport-tcp/tests/framing.rs:315](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/tests/framing.rs#L315); [crates/shamir-server/src/framer.rs:118](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/framer.rs#L118); [crates/shamir-server/src/framer.rs:313](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/framer.rs#L313).

<a id="plan-p1-7"></a>

### Plan P1.7 — P1.7

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

The listed test gaps remain. Use exact variants/error kinds, genuine constructor inputs and an actual unspecified bind. A pre-handshake exporter test needs a real rustls connection through an adapter, not merely a mock returning Err. Existing EOF cleanup is counter-evidence, not newly completed work.

Evidence: [crates/shamir-transport-tcp/src/tests/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tests/mod.rs#L1); [crates/shamir-transport-tcp/tests/framing.rs:240](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/tests/framing.rs#L240); [crates/shamir-transport-tcp/src/tests/listener_tests.rs:101](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tests/listener_tests.rs#L101); [crates/shamir-transport-tcp/src/tls.rs:88](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L88).

<a id="plan-p1-8"></a>

### Plan P1.8 — P1.8

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

CertificateVerify still unconditionally succeeds. rustls 0.23.37 crypto::verify_tls13_signature can verify with the selected provider algorithms without CA trust. Preserve protocol Ed25519 authentication; an optional X.509/SPKI pin must not be substituted for the separate identity pin. Include invalid-signature controls.

Evidence: [crates/shamir-transport-tcp/src/tls.rs:158](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L158); [crates/shamir-client/src/client.rs:484](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L484); [Cargo.lock:3048](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3048).

<a id="plan-p1-9"></a>

### Plan P1.9 — P1.9

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Option/error erasure and server zero fallbacks remain. Returning Result requires updating TCP, native WS and client callers to fail closed. Preserve the current project label, empty context and 32 bytes; strict RFC terminology correction must not silently change deployed wire binding.

Evidence: [crates/shamir-transport-tcp/src/tls.rs:80](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L80); [crates/shamir-server/src/server/server_launcher.rs:1163](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L1163); [crates/shamir-server/src/server/server_launcher.rs:1391](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L1391).

<a id="plan-p1-10"></a>

### Plan P1.10 — P1.10

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Reader cancellation/error contracts remain absent. Require conservative connection discard after partial reads, but do not assert every Err necessarily consumes bytes. Documentation cannot by itself repair the unsafe buffer or missing handshake deadline; writer cancellation can also leave partial output.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:36](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L36); [crates/shamir-transport-tcp/src/framing.rs:72](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L72); [crates/shamir-server/src/connection/request_loop.rs:231](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L231).

<a id="plan-p2-11"></a>

### Plan P2.11 — P2.11

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Capped writers and a shared helper remain absent. Prefer additive companions to signature churn, retain the protocol's default ceiling and strict mapped-address rejection, and distinguish future negotiation from the existing phase limits.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:153](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L153); [crates/shamir-transport-tcp/src/listener.rs:51](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/listener.rs#L51); [crates/shamir-server/src/config.rs:877](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/config.rs#L877).

<a id="plan-p2-12"></a>

### Plan P2.12 — P2.12

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Exports remain unchanged. Completing them is additive; deletion removes valid public paths despite no repository callers. Full root re-exporting is a design choice, not a correctness requirement.

Evidence: [crates/shamir-transport-tcp/src/lib.rs:12](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/lib.rs#L12); [crates/shamir-transport-tcp/src/lib.rs:16](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/lib.rs#L16).

<a id="plan-p2-13"></a>

### Plan P2.13 — P2.13

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Six local imports across four bodies remain. Hoisting is a scoped style edit; merge the already duplicated CertificateDer import rather than introducing duplicate bindings.

Evidence: [crates/shamir-transport-tcp/tests/framing.rs:50](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/tests/framing.rs#L50); [crates/shamir-transport-tcp/tests/tls13_only.rs:36](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/tests/tls13_only.rs#L36); [crates/shamir-transport-tcp/tests/echo_e2e.rs:208](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/tests/echo_e2e.rs#L208).

<a id="plan-p2-14"></a>

### Plan P2.14 — P2.14

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Scheduling docs/test changes remain absent. spawn_blocking isolates KDF work; switching to multi_thread merely supplies other workers and does not make synchronous work nonblocking. Neither approach proves the historical timeout forecast.

Evidence: [crates/shamir-transport-tcp/src/tls.rs:28](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L28); [crates/shamir-transport-tcp/tests/handshake_e2e.rs:278](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/tests/handshake_e2e.rs#L278); [crates/shamir-transport-tcp/tests/echo_e2e.rs:376](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/tests/echo_e2e.rs#L376).

<a id="plan-p2-15"></a>

### Plan P2.15 — P2.15

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

The cleanup has not landed. Single-source the label without changing bytes or its public path. Privatizing context/deleting constants changes public API. A crate-root src/tests/framing_tests.rs still does not implement the expressly described per-module directory layout.

Evidence: [crates/shamir-transport-tcp/src/tls.rs:18](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L18); [crates/shamir-transport-tcp/src/tls.rs:20](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L20); [crates/shamir-transport-tcp/src/listener.rs:96](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/listener.rs#L96); [crates/shamir-transport-tcp/src/tests/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tests/mod.rs#L1).

<a id="plan-p2-16"></a>

### Plan P2.16 — P2.16

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

All bundled changes remain outstanding. Persistent buffering must preserve unread bytes and cancellation teardown; client writer scratch needs serialized ownership. Fixture sharing does not establish wire conformance, feature trimming may yield no savings, and syscall/latency claims remain unverified.

Evidence: [crates/shamir-client/src/client.rs:1278](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1278); [crates/shamir-transport-tcp/Cargo.toml:14](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/Cargo.toml#L14); [crates/shamir-transport-tcp/src/tls.rs:73](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L73); [crates/shamir-transport-tcp/tests/handshake_e2e.rs:41](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/tests/handshake_e2e.rs#L41).

## Additional observations

| Observation decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 4 | 4 | 0 | 0 | 0 | 0 | 0 |

Existing observation IDs remain stable. New cycle-2 rows follow them; cross-module duplicates and extra triggers are grouped explicitly. None is an implemented fix.

<a id="observation-new-1"></a>

### Observation NEW.1 — Production server can retain an unauthenticated connection indefinitely after sending challenge

Status: `confirmed-open`. Current risk: `high`.

Additional observation in this independent cycle; it may overlap an existing root.

A peer completes TLS, sends a valid auth_init within the first-frame timeout, receives challenge, then sends no client_proof or stalls mid-proof. run_handshake awaits the transport read without a timeout; its caller awaits it directly. The local ServerHandshake is not covered by SessionStore GC, and request_loop's idle timer has not started. Global/per-IP caps and rate limiting bound admission but do not reclaim these occupied slots. This contradicts AUTH_PROTOCOL §8.2's HANDSHAKE_TIMEOUT lifecycle. Existing slow_loris.rs covers silence before auth_init only. A discriminating oracle must reach challenge, withhold/trickle proof, and observe terminal closure plus slot release under the separate handshake budget.

Evidence: [crates/shamir-server/src/connection/handshake.rs:291](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/handshake.rs#L291); [crates/shamir-server/src/connection/handshake.rs:717](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/handshake.rs#L717); [crates/shamir-server/src/connection/handshake.rs:771](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/handshake.rs#L771); [crates/shamir-server/src/server/server_launcher.rs:1165](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L1165); [crates/shamir-server/src/connection/connection_context.rs:55](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/connection_context.rs#L55); [crates/shamir-server/src/scheduler.rs:194](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/scheduler.rs#L194); [crates/shamir-server/tests/slow_loris.rs:79](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/tests/slow_loris.rs#L79); [docs/guide-docs/client-server-protocol-spec/AUTH_PROTOCOL.md:507](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/guide-docs/client-server-protocol-spec/AUTH_PROTOCOL.md#L507).

Grouping/duplicate: [../shamir-server/SUMMARY.md#NEW.1](../shamir-server/SUMMARY.md#new-1). This is not an additional independent defect.

<a id="observation-new-2"></a>

### Observation NEW.2 — Production Rust client applies the data-frame limit before server authentication

Status: `confirmed-open`. Current risk: `medium`.

Additional observation in this independent cycle; it may overlap an existing root.

Client::connect reads challenge and auth_ok with MAX_FRAME_SIZE_DEFAULT=16 MiB before process_auth_ok verifies the identity at line 614. An attacker TLS endpoint or active MITM can declare a payload over 4 KiB but within 16 MiB; read_frame allocates it instead of enforcing the protocol's pre-authentication ceiling. Thus the reports' 4 KiB unauthenticated-bound assurance is server-specific. An adversarial handshake peer sending a 4097-byte declaration must be rejected before body allocation; no execution was attempted.

Evidence: [crates/shamir-client/src/client.rs:511](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L511); [crates/shamir-client/src/client.rs:559](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L559); [crates/shamir-client/src/client.rs:614](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L614); [crates/shamir-transport-tcp/src/framing.rs:67](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L67); [docs/guide-docs/client-server-protocol-spec/TRANSPORT_TCP.md:29](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/guide-docs/client-server-protocol-spec/TRANSPORT_TCP.md#L29).

<a id="observation-new-3"></a>

### Observation NEW.3 — Client TLS12 rejection test cannot distinguish a version-selection regression

Status: `confirmed-open`. Current risk: `low`.

Additional observation in this independent cycle; it may overlap an existing root.

The test asserts only client_result.is_err and ignores the server result. If make_client_config_no_ca is changed to offer TLS12 as well, NoCaVerify::verify_tls12_signature still returns Tls12NotOffered, so the test remains green after negotiation advances into TLS12. rustls 0.23.37 src/client/tls12.rs invokes that callback. Assert the version-stage rejection with compatible positive controls, rather than treating any handshake error as proof that TLS12 was never negotiated. Published source: https://docs.rs/crate/rustls/0.23.37/source/src/client/tls12.rs.

Evidence: [crates/shamir-transport-tcp/tests/tls13_only.rs:130](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/tests/tls13_only.rs#L130); [crates/shamir-transport-tcp/tests/tls13_only.rs:151](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/tests/tls13_only.rs#L151); [crates/shamir-transport-tcp/tests/tls13_only.rs:155](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/tests/tls13_only.rs#L155); [crates/shamir-transport-tcp/src/tls.rs:141](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L141); [Cargo.lock:3048](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3048).

<a id="observation-new-4"></a>

### Observation NEW.4 — Project-specific exporter is incorrectly described as strict RFC 9266 compliance

Status: `confirmed-open`. Current risk: `low`.

Additional observation in this independent cycle; it may overlap an existing root.

The implementation and project specification use EXPORTER-ShamirDB-AUTH-v1. RFC 9266 §2 fixes a different label, EXPORTER-Channel-Binding. Internal same-helper roundtrips cannot detect this standards-description mismatch. The custom binding is not thereby shown insecure; document it as a deliberate project-specific exporter, preserving deployed bytes unless a separately versioned protocol change is intended. See [RFC 9266 §2](https://www.rfc-editor.org/rfc/rfc9266.html#section-2).

Evidence: [crates/shamir-transport-tcp/src/tls.rs:6](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L6); [crates/shamir-transport-tcp/src/tls.rs:18](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L18); [crates/shamir-transport-tcp/src/tls.rs:80](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L80); [docs/guide-docs/client-server-protocol-spec/TRANSPORT_TCP.md:41](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/guide-docs/client-server-protocol-spec/TRANSPORT_TCP.md#L41); [docs/guide-docs/client-server-protocol-spec/SECURITY_MODEL.md:154](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/guide-docs/client-server-protocol-spec/SECURITY_MODEL.md#L154).

## Evidence and recipe corrections

- The current overview's read-deadline qualification is incomplete: the server client_proof stage has no deadline or handshake-state GC. Initial-frame timeout and post-auth idle timeout do not cover it.
- The 4 KiB pre-authentication assurance applies to server reads, not the production Rust client's challenge/auth_ok reads.
- The registered client TLS12 test is not a discriminating oracle for the client's version builder: NoCaVerify independently rejects every TLS12 signature.
- RFC 9266 compliance is overstated because the project deliberately uses a different exporter label; clarify the custom binding rather than changing it without a protocol compatibility decision.
- Do not count optional root exports, feature breadth, boxed boundary errors, equivalent predicate duplication or documented buffer retention as medium runtime defects. Severity is recalibrated in the rows.
- Zero is a protocol close marker, but the protocol does not require that only the write_close helper may emit it. Correct documentation plus a pinned close behavior is a valid alternative to changing empty-write semantics.
- The echo test's pooled sequential server is not the production concurrent request loop. Its approximately halved allocator-pressure comment is not a measured production guarantee.
- No source-fixed label is justified by historical Miri or task-completion wording; the relevant source mechanisms remain unchanged from the earlier report snapshot.

## Module scope and limitations

Coverage: 8 assigned documents, 79 current claim rows, 16 plan rows, 0 pre-existing observation rows; 4 added observation rows in this cycle. Counts are calculated from the accepted rows.

Assigned documents: [SUMMARY.md](SUMMARY.md); [api-wire-protocol.md](api-wire-protocol.md); [concurrency-lockfree.md](concurrency-lockfree.md); [correctness-tdd.md](correctness-tdd.md); [error-handling-lifecycle.md](error-handling-lifecycle.md); [performance-hotpath.md](performance-hotpath.md); [security-crypto.md](security-crypto.md); [style-claude-md.md](style-claude-md.md).

- Read-only source, documentation, history and published-package inspection only. No files written and no builds, tests, Miri, benchmarks, reproductions or agents executed.
- All eight assigned documents, including collapsed historical claims and recipes, were read completely. There are no current Observation rows or assigned TASK_GROUPS.md.
- HEAD remained the requested revision. Cargo.lock became externally modified during review, including rustls 0.23.45; dependency conclusions use the immutable requested HEAD's Cargo.lock, independently reread with git show, and rustls 0.23.37 archive bytes. Relevant inspected source files had no working-tree differences.
- Inspected cached published sources for Tokio 1.49.0, rustls 0.23.37, tokio-rustls 0.26.4, rustls-pki-types 1.14.0, rcgen 0.13.2, zeroize 1.8.2 and DashMap 6.1.0. Archive checksums were not independently calculated.
- No measured latency, syscall ratio, RSS total, crash threshold, test-pass result or demonstrated production memory disclosure is claimed.
- Coverage is complete for the assigned checklist, not an exhaustive audit of the crate, server, client or transitive dependencies.

## Guarantee checks

- **TCP framing uses a big-endian u32 byte length; zero denotes close.** — `supported`. Both readers decode big-endian lengths and return PeerClose for zero. All writers emit matching framing. The separate assertion that empty application data is expressible contradicts this behavior. Reference: docs/guide-docs/client-server-protocol-spec/TRANSPORT_TCP.md:25.
- **read_frame_into never exposes uninitialized bytes through its safe interface.** — `diverges`. set_len precedes initialization and read_exact. Tokio 1.49.0 src/io/util/read_exact.rs constructs ReadBuf::new, which treats the entire slice as initialized; a legal reader can inspect initialized(). Cancellation also leaves the enlarged length. Published source: https://docs.rs/crate/tokio/1.49.0/source/src/io/util/read_exact.rs. Reference: crates/shamir-transport-tcp/src/framing.rs:93.
- **Both supplied TLS configurations permit TLS 1.3 only.** — `supported`. Both constructors explicitly select TLS13. Negative tests are registered, but the client test's bare rejection oracle does not independently distinguish a version-selection regression from its verifier's separate TLS12 rejection. Reference: docs/guide-docs/client-server-protocol-spec/TRANSPORT_TCP.md:35.
- **TLS 0-RTT is disabled.** — `supported`. The constructors retain rustls 0.23.37 defaults: client enable_early_data=false and server max_early_data_size=0. This establishes constructor configuration, not immutability of returned configs. Published source: https://docs.rs/crate/rustls/0.23.37/source/src/server/builder.rs. Reference: docs/guide-docs/client-server-protocol-spec/TRANSPORT_TCP.md:43.
- **Certificate-chain trust is replaced by protocol Ed25519 identity authentication.** — `supported`. Client::connect verifies SCRAM, the configured identity pin or explicit TOFU choice, and the exporter-bound identity signature before returning. This does not excuse the separate unconditional CertificateVerify callback or establish security for arbitrary helper consumers. Reference: docs/guide-docs/client-server-protocol-spec/TRANSPORT_TCP.md:39.
- **The exporter matches the project-specific label, empty context and 32-byte contract.** — `supported`. extract_tls_exporter supplies EXPORTER-ShamirDB-AUTH-v1, Some(empty context), and a 32-byte output. The length is already documented. Reference: docs/guide-docs/client-server-protocol-spec/TRANSPORT_TCP.md:41.
- **The custom exporter is strictly RFC 9266 compliant.** — `diverges`. RFC 9266 §2 specifies EXPORTER-Channel-Binding, not the project's custom label. The implementation matches the project protocol, but the standards-compliance assertion needs qualification. See [RFC 9266 §2](https://www.rfc-editor.org/rfc/rfc9266.html#section-2). Reference: docs/guide-docs/client-server-protocol-spec/SECURITY_MODEL.md:154.
- **Pre-authentication frames are capped at 4 KiB.** — `diverges`. Server auth_init and client_proof reads use MAX_PRE_AUTH_FRAME. Production Rust client challenge and auth_ok reads instead use the 16 MiB data limit before authenticating the server. Reference: docs/guide-docs/client-server-protocol-spec/TRANSPORT_TCP.md:29.
- **Handshake state without a proof expires after HANDSHAKE_TIMEOUT.** — `diverges`. The server times TLS and the first auth_init frame, but run_handshake awaits client_proof without a deadline. Its state is local, not in SessionStore's GC, and the authenticated idle timer starts only afterward. Reference: docs/guide-docs/client-server-protocol-spec/AUTH_PROTOCOL.md:507.
- **Post-authentication server reads have a finite whole-frame lifetime.** — `supported`. The select timer is created once for the complete frame read and is not reset by partial bytes. Expiry breaks the connection loop and discards the buffer; it is stronger than a per-byte resettable idle timer. Reference: crates/shamir-server/src/connection/request_loop.rs:279.
- **Plain TCP cannot bind outside the strict loopback whitelist.** — `supported`. bind_validated checks IPv4/IPv6 is_loopback before creating TcpListener. The production configuration repeats the same predicate. This supports the address gate, not UDS support or every broader plain-profile opt-in requirement. Reference: docs/guide-docs/client-server-protocol-spec/TRANSPORT_TCP.md:12.
- **No locks exist in the transport implementation, and SessionStore::len is not an scc traversal.** — `supported`. The crate's implementation contains no explicit locks or registries. SessionStore::len delegates to DashMap 6.1.0 _len, which sums shard lengths under read locks; this is O(shards), not lock-free. Published source: https://docs.rs/crate/dashmap/6.1.0/source/src/lib.rs. Reference: crates/shamir-transport-tcp/src/lib.rs:8.
- **Returned private-key PEM and the constructor's PEM byte copy are zeroized on ordinary drop.** — `supported`. Both buffers use Zeroizing; zeroize 1.8.2 Drop calls Zeroize. This is not a guarantee covering every rcgen intermediate, caller-owned input, persisted key or non-unwinding termination. Published source: https://docs.rs/crate/zeroize/1.8.2/source/src/lib.rs. Reference: crates/shamir-transport-tcp/src/tls.rs:37.
- **The framing and TLS tests are registered and selected by the full integration workflow.** — `supported`. listener_tests is the only unit module; Cargo discovers the four integration files. Default scripts/test.sh and cargo tl select --lib, excluding those files. CI's --full kind(test) job includes them; no execution result was verified. Reference: crates/shamir-transport-tcp/src/lib.rs:13.

## Reviewer's prior-cycle comparison

These are the independent reviewer's comparisons before parent refinements; the accepted ledgers above govern final decisions and counts.

- The earlier provider Claim correctness-tdd.md#6 remains refuted, not fixed: pinned rustls already auto-installed an unambiguous provider. SUMMARY.md#6.3 remains a conditional risk, recalibrated from medium to low under the inspected defaults.
- SUMMARY.md#3.1 remains confirmed-open/high. The proof is the exact Tokio initialized-buffer contract plus cancellation behavior, not historical Miri labels or UB asserted on every invocation.
- Material assurance disagreement: current reports qualify production reads using initial-frame and idle deadlines, but server client_proof waiting is outside both. This independently confirmed open/high caller lifecycle defect has no assigned current row.
- Material oracle disagreement: registered client TLS12 rejection is not independent proof of version selection because the verifier separately rejects TLS12 signatures. Current code still explicitly restricts TLS13.
- SUMMARY.md#4.2 remains structurally open but changes from medium to low: client retention is expressly intentional and documented, while universal server lifetime retention is refuted. Automatic shrinking must not silently contradict the existing capacity contract.
- SUMMARY.md#1.1 changes from medium to low: the documentation is false, but zero generic output is a valid close frame and exclusive write_close use is not normative. P0.2's rejection recipe is an API choice, not the only conforming fix.
- SUMMARY.md#6.1 changes from medium to low; SUMMARY.md#6.2 changes from medium to nit. Misclassified diagnostics are concrete, whereas boxed errors permit downcasting and are not themselves a demonstrated runtime/security failure.
- Root-export completeness, equivalent whitelist factoring, feature trimming and test placement are not independent runtime bugs. Their conditions remain inspected and open at nit severity rather than being omitted from coverage.
- The requested frozen dependency graph was preserved analytically despite an external Cargo.lock update during review; no conclusions substitute rustls 0.23.45 semantics for the requested 0.23.37 pin.
- Rust Intel semantic-conformance and oracle discipline guided the counterfactual checks; no implementation or full-category audit was performed.

## Current follow-up order

1. Repair read_frame_into initialization soundness and cancellation-buffer observability; require legal-reader and pending-read cancellation oracles.
2. Bound the production server's entire proof-wait handshake stage using the protocol's separate HANDSHAKE_TIMEOUT, with terminal teardown and observable slot release.
3. Apply the 4 KiB pre-authentication ceiling in production Rust client handshake reads.
4. Restore TLS CertificateVerify proof-of-possession and strengthen the client TLS-version rejection oracle without replacing protocol Ed25519 authentication.
5. Preserve exporter errors and remove server zero fallbacks; qualify RFC compliance without silently changing the project label.
6. Correct contradictory empty-data documentation, malformed-prefix errors and reader cancellation/discard contracts.
7. Evaluate declared-length allocation and persistent client capacity with explicit deployment budgets; treat reclamation and buffering changes as policy/performance work requiring measurements.
8. Handle typed TLS errors, API aliases, fixture sharing, feature breadth, imports and layout as lower-priority scoped maintenance.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-transport-tcp — Synthesized 7-lens review (consolidated single-file follow-up to the 2026-08-14 cross-crate review)

Crate reviewed: `crates/shamir-transport-tcp/` — the TLS 1.3 (rustls) + length-prefix msgpack
TCP transport binding per spec `TRANSPORT_TCP.md`, consumed by `shamir-server`, `shamir-client`,
and `shamir-transport-ws`.

Review basis: the seven 2026-08-14 lens reports under this directory, all read in full and
synthesized/deduplicated here — `correctness-tdd.md`, `concurrency-lockfree.md`,
`security-crypto.md`, `performance-hotpath.md`, `api-wire-protocol.md`,
`error-handling-lifecycle.md`, `style-claude-md.md`. Calibration for structure/tone/rigor:
`shamir-client-node/SUMMARY.md` and `shamir-transport-ipc/SUMMARY.md` (the lean single-file
exemplars; this crate itself served as the IPC review's calibration reference). The
workspace-wide `SUMMARY.md` was consulted only for this crate's raw lens-tagged row
(0c / 1h / 12m / 22l / 14n = 49 — this synthesis reproduces that raw count exactly).
Spot-checks: `src/framing.rs`, `src/tls.rs`, `src/listener.rs`, `src/lib.rs`, `Cargo.toml`
re-read in full; every headline file:line re-verified against source, including the
production cancellation reachability of the headline finding
(`shamir-server/src/connection/handshake.rs:717`, `request_loop.rs:280` — both wrap
`read_frame_into` in `select!`/timeout branches). Read-only synthesis — no build/test/lint
commands, no source files modified. No new defects surfaced during spot-checks, so nothing
below is marked "(added during synthesis)".

## Executive summary

The crate is structurally one of the cleaner transports in the workspace — zero locks of any
kind, profile-aware fail-closed bind validation, TLS 1.3-only enforced with negative tests in
both directions, and a Miri-vetted framing codec — but it carries the workspace's headline
memory-safety defect: `read_frame_into` (`src/framing.rs:117-136`) sets `Vec::len` over
uninitialized memory before the `.await`, which is formally UB on every call and leaves the
caller holding a poisoned buffer after `select!`/`timeout` cancellation — a pattern the
production server's request loop and handshake use today, with no cancellation-path test and
no documented poison contract. Fix that first (P0). Next: close the write-side close-sentinel
hazard (any zero-length `write_frame` silently emits a graceful close that tears down the
peer's connection while the module doc claims empty frames are legal), and give the
boot-critical TLS config constructors a typed, non-panicking error surface (`Box<dyn Error>`
returns today; latent `CryptoProvider` panic paths; `make_client_config_no_ca` is infallible
by signature).

---

## 1. correctness-tdd

Overall: small, logically tight, well-tested happy paths (boundary framing tests, Miri pair,
TLS 1.3-only in both directions). The gaps are on the edges the tests don't reach; no
critical/high correctness bug beyond the shared unsafe finding (3.1).

### 1.1 [MEDIUM] Zero-length payload write silently emits the close sentinel; module doc contradicts spec §2
- **File:line:** `src/framing.rs:8-9` (doc), `:53-55`, `:107-109` (read-side `PeerClose` mapping), `:149-166` (`write_frame`), `:187-206` (`write_frame_into`), `:241-250` (`write_frame_prereserved`); spec: `docs/guide-docs/client-server-protocol-spec/TRANSPORT_TCP.md:28,106`.
- **Issue:** The module doc states "`length == 0` is a graceful close indicator. Empty frames are also legal at the application level (caller decides)." These two sentences cannot both hold: `read_frame`/`read_frame_into` map `len == 0` *exclusively* to `Err(FrameError::PeerClose)`, so an "empty frame" can never be delivered as data — and spec §2 defines `length=0` as graceful close only. Meanwhile all three write functions accept a zero-length payload (and `write_frame_prereserved` accepts a 4-byte all-zero buffer), emitting the wire close marker with no guard and no test pinning the behavior.
- **Failure scenario:** Any caller that writes an empty payload through the generic `&[u8]` API (only rmp-serde's non-empty output saves today's callers) silently signals graceful close: the peer's read returns `PeerClose` and its request loop tears down the connection (`shamir-client/src/client.rs:227-230` breaks on `PeerClose`; `tests/echo_e2e.rs:305` breaks on any frame error) — a silent connection drop that looks like the *other* side asked to close, with no error on the writer side and subsequent requests hanging until timeout.
- **Suggested fix:** Delete the "Empty frames are also legal" sentence, and reject zero-length payloads in all three write functions (e.g. a dedicated `FrameError::EmptyFrame`), reserving `length=0` for `write_close` only — making the writer fail closed instead of sending a control frame as data. Add a test whichever way is chosen; today the documented contract is false and untested.
- *Also flagged by: api-wire-protocol finding 1 (same root cause; merged here).*

### 1.2 [MEDIUM] `tls.rs` has zero unit tests; all error branches unexercised (happy-path-only e2e coverage)
- **File:line:** `src/tls.rs:41-58` (`make_server_config_from_pem` error paths), `:50` (`NoKeyInPem` arm), `:77-83` (`extract_tls_exporter` `None` path); no unit-test file exists for the module.
- **Issue:** The `tls` module is covered only indirectly by e2e tests (`tests/handshake_e2e.rs`, `tests/echo_e2e.rs`, `tests/tls13_only.rs`), all driving the success path. Never executed by any test: PEM parse failure, missing-PKCS8-key path (`ok_or("no PKCS8 key in PEM")`), key/cert mismatch (`with_single_cert` error), and `extract_tls_exporter` returning `None` for an incomplete handshake. Against CLAUDE.md's Red/Green/Refactor protocol these library error branches have no failing-test-first record — they are the most likely real-world misconfiguration surface (persisted cert/key corrupted or rotated out of sync, per `generate_self_signed_server_cert`'s "caller persists" contract).
- **Failure scenario:** A corrupted or swapped cert/key PEM at server boot yields *some* `Err` — but nothing pins *which* error, or that a wrong-format key (e.g. PKCS#1 `BEGIN RSA PRIVATE KEY`) is rejected rather than silently mis-parsed; a refactor of the PEM-parsing arms regresses silently with a green suite.
- **Suggested fix:** Add a unit-test file for `tls.rs` (per CLAUDE.md test layout) covering bad-PEM, empty-PEM, no-PKCS8-block, cert/key-mismatch → `Err`, and a pre-handshake exporter extraction → `None` (a `rustls` `ConnectionCommon` that hasn't completed a handshake suffices). Note: typed variants from the 6.2 fix are what make these assertable per failure class.
- *Also flagged by: error-handling-lifecycle finding 4 part (a) (same gap; merged here).*

### 1.3 [LOW] Test name promises TLS-bind-on-unspecified coverage the body deliberately doesn't deliver
- **File:line:** `src/tests/listener_tests.rs:100-106`.
- **Issue:** `bind_validated_succeeds_for_tls_on_unspecified` binds `127.0.0.1:0`, not the unspecified address — the inline comment admits it ("use loopback to avoid firewall prompts in CI"). The test is vacuous relative to its name; only the pure predicate `allows()` is unit-tested (`listener_tests.rs:58-65`). A regression in `bind_validated`'s pass-through (e.g. accidentally applying the loopback gate to all profiles) would still pass this test's TLS branch as written.
- **Failure scenario:** the exact regression the name claims to guard ships green.
- **Suggested fix:** Rename to `bind_validated_succeeds_for_tls_on_loopback` and add a separate test calling `bind_validated` with `0.0.0.0` + `TlsExporter` (or, if binding `0.0.0.0` is genuinely off-limits in CI, assert only `validate_addr(0.0.0.0, Tls).is_ok()` under a name that says so).

### 1.4 [LOW] Write-path boundary branches, EOF-vs-close distinction, and IPv4-mapped-IPv6 edge untested
- **File:line:** `src/framing.rs:153-158`, `:192-197`, `:250` (untested `TooLarge` branches — the `declared_len > MAX` half of the `||` at `:250` is dead in tests); `:49-50` (abrupt-EOF path); `src/listener.rs:51-56` (IPv4-mapped edge).
- **Issue:** The read side's size boundary is well tested (`rejects_oversized_frame_declaration`, `frame_exactly_at_size_limit_is_accepted`, `read_frame_into_rejects_oversized`), but no test ever executes the write-side rejections (payload > 16 MiB in `write_frame`/`write_frame_into`); also untested: abrupt EOF (peer vanishes without the close frame) yielding `FrameError::Io(UnexpectedEof)` — the distinction consumers rely on (`shamir-client/src/client.rs:227-234` treats `PeerClose` and other errors differently) — and the IPv4-mapped input `::ffff:127.0.0.1`, which std's `is_loopback()` classifies as non-loopback (fail-closed, so correct, but undocumented and unpinned).
- **Failure scenario:** A refactor dropping the `declared_len > MAX` clause as "unreachable" would be green — nothing would catch it.
- **Suggested fix:** Cheap tests: `write_frame` with 16 MiB + 1 (one allocation, tolerable), a prereserved buffer with a valid prefix but > MAX payload, a `duplex` dropped mid-frame → `Io` error, and `Plain.allows(::ffff:127.0.0.1) == false` to pin the fail-closed behavior.
- *Also flagged by: error-handling-lifecycle finding 4 part (b) (same gap; merged here).*

---

## 2. concurrency-lockfree

**General verdict: clean under all five pillars.** Zero `std::sync::Mutex`/`RwLock`/`parking_lot`
sites, zero `scc::*`/`DashMap`/`ArcSwap` usage, zero locks of any kind — so no lock-across-`.await`
or lock-contention exposure. Shared state is limited to `Arc` (TLS configs, test fixtures) and
`AtomicU64` counters in tests; the framing API is per-connection `&mut R`/`&mut W` exclusive
ownership (the ideal pillar-1 shape) with pooled `*_into` alternatives (pillar 3); pillar 4 is
vacuously satisfied (no hash-keyed structure). Checked and NOT flagged:
`tests/echo_e2e.rs:439`'s `session_store.len()` resolves to `shamir-connect`'s
`SessionStore::len` → `DashMap::len`, which is O(shards) (fixed, effectively constant), not the
banned O(N) `scc::*::len`, is absent from `clippy.toml` disallowed-methods, and is a one-shot
test assertion.

### 2.1 [LOW] Sync CPU-bound bootstrap helpers lack a spawn_blocking / bootstrap-only contract
- **File:line:** `src/tls.rs:28-38` (`generate_self_signed_server_cert`), `:41-58` (`make_server_config_from_pem`).
- **Issue:** Both are synchronous and CPU-bound (rcgen ECDSA P-256 keygen; PEM parse + rustls `ServerConfig` build). CLAUDE.md pillar 2 routes CPU-bound work through `tokio::task::spawn_blocking`. These are one-shot bootstrap by intent — the doc says the caller "persists for reuse across restarts" — but the bootstrap-only contract is not written down, and the crate's own e2e tests already call them inside `#[tokio::test]` bodies (`tests/echo_e2e.rs:177-178`, `tests/handshake_e2e.rs:131-132`, `tests/tls13_only.rs:101-102,133-134`), i.e. on a runtime thread.
- **Failure scenario:** none today (bootstrap-only). A future caller that regenerates a cert/config per rebind or per connection inside async code stalls every task sharing that worker thread (on an embedded `current_thread` runtime, the whole accept loop) for the duration of keygen + config build.
- **Suggested fix:** encode the contract in the doc comments ("bootstrap-only; if invoked at runtime, call from `tokio::task::spawn_blocking`"), or add `async` wrappers that internally `spawn_blocking` the keygen. No behavioral change for current callers.

### 2.2 [NIT] e2e tests run Argon2id inline on the test runtime thread while a peer task is spawned
- **File:line:** `tests/echo_e2e.rs:376` and `tests/handshake_e2e.rs:278` (`hs.process_challenge(...)` → Argon2id derive); pattern: default `#[tokio::test]` (current-thread) + `tokio::spawn`ed server task at `tests/echo_e2e.rs:196`, `tests/handshake_e2e.rs:144`.
- **Issue:** `#[tokio::test]` defaults to a `current_thread` runtime; the client-side Argon2id KDF (~19 MB, tens of ms) executes inline and blocks the only thread, so the spawned server task cannot be polled during the derive. Safe today only because the protocol dependency chain is strictly sequential (the server is parked awaiting a proof frame the client has not yet sent).
- **Failure scenario:** any refactor that makes the server need the thread during the client's blocking compute (server-side derive, a `tokio::time::timeout` around the server's `read_frame`, extra server tasks) surfaces as a mysterious SLOW/TIMEOUT in nextest — exactly the hang class CLAUDE.md mandates hunting to root cause — with no single-task culprit visible.
- **Suggested fix:** switch these two tests to `#[tokio::test(flavor = "multi_thread")]`, or wrap the KDF step in `tokio::task::spawn_blocking` (also matches pillar 2). Test-only; no production code change.

---

## 3. security-crypto

**The crate is the TLS/framing boundary, not the SCRAM implementation** (HMAC/SCRAM/Argon2 live in
`shamir-connect`), and its crypto posture is largely sound: TLS 1.3-only enforced on both sides
with negative tests, the exporter extracted per RFC 9266 and proven equal on both ends in e2e,
the Plain profile fails closed on non-loopback binds (incl. `0.0.0.0`/`::`), the private-key PEM
is `Zeroizing`-wrapped. (Note: the ungated use of `rustls::client::danger` is fine — rustls
0.23.37 as resolved in `Cargo.lock` has no `dangerous_configuration` feature at all.) The dominant
risk is the crate's one `unsafe` block; second is the trust boundary documented only in prose.

### 3.1 [HIGH] `read_frame_into`: unsafe `set_len` before `.await` — formal UB on uninit memory, and cancellation leaves `buf.len()` covering uninitialized bytes
- **File:line:** `src/framing.rs:117-136` (`buf.reserve(len)` at `:124`, `unsafe { buf.set_len(len) }` at `:126-128` under `#[allow(clippy::uninit_vec)]`, `read_exact(buf).await` at `:129`, error-path `buf.clear()` at `:134`).
- **Issue:** `buf.reserve(len)` + `unsafe { buf.set_len(len) }` exposes uninitialized capacity as `&mut Vec<u8>`, deref-coerced to `&mut [u8]` and passed to `reader.read_exact(buf).await`. (a) Creating a `&mut [u8]` over uninitialized bytes is itself undefined behavior (reference validity requires initialized memory) — tokio's `read_exact` wraps the slice in `ReadBuf::new`, a constructor that asserts initialization. The code carries `#[allow(clippy::uninit_vec)]`, the lint that exists to flag exactly this. (b) The SAFETY comment only covers the Ok/Err outcomes, but this is an `async fn`: if the future is **dropped mid-await** (cancellation via `select!` or `timeout` — not an `Err` return), no code runs, and the caller is left holding `buf` with `len == declared_len` while the tail is still uninitialized. Any later read of `buf[..]` is UB and can disclose stale heap bytes; the error-path `buf.clear()` does not run on cancellation. (c) Sub-point carried from the error-handling lens: the SAFETY claim "the uninit bytes are never observed by safe code" also rests on the `AsyncRead` implementation never *reading* the output slice — the `poll_read` contract does not guarantee that; the strictly sound form reads into `&mut [MaybeUninit<u8>]` (`ReadBuf::uninit`/`read_buf`). *(This is the workspace headline finding: reachable via `select!`/`timeout` cancellation on a production path — `shamir-server/src/connection/handshake.rs:716-717` and `request_loop.rs:279-308` both wrap this exact function; spot-checked against source.)*
- **Failure scenario:** a caller wraps `read_frame_into` in `tokio::time::timeout(...)` or `select!` (exactly what `shamir-server` does today). Both current call sites happen to discard `frame_buf` after a cancelled branch (and `request_loop.rs` builds a fresh `Vec` per iteration), so nothing *currently* observes the uninit tail — but the crate neither documents this poison-after-cancellation contract nor enforces it; the next caller that logs/inspects the buffer after a timed-out read gets undefined behavior / potential secret disclosure from freed memory. The "Miri-safe" tests (`tests/framing.rs:189-250`) cover only the happy and error paths — no cancellation-path test exists.
- **Suggested fix:** make the uninitialized window unobservable instead of argued away: read into `buf.spare_capacity_mut()` via `ReadBuf::uninit` (small `poll_fn` loop over `AsyncRead::poll_read`), and issue `unsafe { buf.set_len(n) }` only after the read completes with n initialized bytes. Interim hardening: a `Drop` guard holding `&mut Vec<u8>` across the `.await` that `clear()`s on any exit (covers cancellation), a prominent `# Cancellation` section in the doc comment, and a `tokio::time::timeout`-based regression test alongside the Miri pair.
- *Also flagged by: error-handling-lifecycle finding 8 (nit — SAFETY argument rests on callee behavior; same root cause, subsumed by the primary fix). Related but distinct: 6.5 (stream-position cancel-safety, which exists independently of the unsafe block).*

### 3.2 [MEDIUM] `NoCaVerify` disables all server authentication — chain, CertificateVerify proof-of-possession, and hostname — guarded only by a doc comment
- **File:line:** `src/tls.rs:63-69` (`make_client_config_no_ca`), `:124-171` (`NoCaVerify`, esp. `:152-159` unconditional `HandshakeSignatureValid::assertion()`; `:138` `ServerCertVerified::assertion()`).
- **Issue:** The custom `ServerCertVerifier` returns `assertion()` unconditionally for both cert and TLS 1.3 signature verification. This is a documented design decision (identity is pinned at the protocol layer: Ed25519 signature over the TLS exporter + SCRAM server signature, spec §6.3/§3.3), and the e2e tests exercise the pin path via `HandshakeBuilder::pinned_hash`. But the only thing standing between a downstream caller and a fully unauthenticated, indistinguishable-from-TLS channel is a doc comment — a public, one-call API with zero structural enforcement; `verify_tls13_signature` → assertion means even proof-of-possession of the presented cert's key is skipped, so TOFU-style pinning *at the TLS layer* is impossible by construction.
- **Failure scenario:** a tool, REPL, or new client built on `make_client_config_no_ca()` that skips (or mis-orders) the `process_auth_ok` pin/signature verification silently accepts any attacker-presented self-signed cert; the attacker completes a TLS 1.3 handshake as a MITM endpoint. Because `identity_sig` would then fail later, a careful caller is saved — but nothing forces that code to exist, and a partial integration (accepting `auth_ok` without verifying `identity_sig`/`server_pub_key` against a pin) fails open.
- **Suggested fix:** (a) verify the TLS 1.3 CertificateVerify signature against the *end-entity cert's own* public key (e.g. `rustls-webpki`'s `EndEntityCert` without chain building) — keeps "accept any self-signed cert" semantics while restoring proof-of-possession; (b) consider an enforcing variant taking the pinned SPKI/pubkey hash as a parameter so the config itself refuses unpinned servers, keeping the current function as the explicitly-named TOFU escape hatch; (c) add a `# SECURITY` doc section naming the caller obligations verbatim.

### 3.3 [LOW] `extract_tls_exporter` collapses all errors (incl. incomplete handshake) into `None`, inviting constant/absent channel binding
- **File:line:** `src/tls.rs:77-83` (`.ok()?` at `:81`).
- **Issue:** `.ok()?` maps every `rustls::Error` — including `HandshakeNotComplete` — into `None`, discarding the reason entirely; the doc comment even acknowledges two distinct causes. An `Option<[u8; 32]>` return invites `unwrap_or`/`unwrap_or_default` misuse at the call site; substituting a placeholder exporter would silently reduce the §4.2 channel binding to a known constant, letting a recorded proof replay across connections (the exact attack the exporter binding exists to stop). The fixed 32-byte output size is also undocumented relative to the spec's channel-binding length; both e2e tests hit the unwrap-or-panic path (`expect("exporter")`), so debugging a live mismatch offers no diagnostic beyond a swallowed error.
- **Failure scenario:** a future caller treats `None` as "exporter unavailable, continue without binding" (e.g. to match `BindingMode::TlsNoExport` peers) instead of aborting; SCRAM is then bound to a constant and loses its MITM-defence. Intermittent production binding failures log only `None` with no rustls error to explain why.
- **Suggested fix:** return `Result<[u8; 32], ExporterError>` (a thiserror enum distinguishing `HandshakeNotComplete` from cipher/algorithm errors — or at minimum `Result<_, rustls::Error>`), document "None MUST be treated as fatal for `TlsExporter` mode" if the `Option` stays, and document the fixed 32-byte contract per spec §4.2. All current callers `.expect()`, so this is API hardening, not a live bug.
- *Also flagged by: api-wire-protocol finding 6 and error-handling-lifecycle finding 7 (same root cause; merged here).*

---

## 4. performance-hotpath

**Structurally clean against pillar 3:** no locks anywhere, length-cap checks run *before* any
allocation, the pooled `read_frame_into`/`write_frame_into`/`write_frame_prereserved` variants
keep the per-frame hot path allocation-free, and `reserve`-after-`clear()` ordering means buffer
growth never memcpy-copies stale bytes (no hidden O(N²)). Remaining findings are memory-shape,
not CPU. Test coverage for this theme is good (`tests/framing.rs` pins capacity reuse,
single-write wire equivalence, uninit-safety; `benches/framing.rs` follows the
`bench_scale_tool::Harness` convention); the only uncovered behavior is the growth/retention
policy (4.2).

### 4.1 [MEDIUM] Full-size allocation on declared length before any payload byte arrives (16 MiB amplification per connection)
- **File:line:** `src/framing.rs:67` (`read_frame`, `vec![0u8; len]`), `:124-128` (`read_frame_into`, `reserve` + `set_len`).
- **Issue:** Both read functions reserve/allocate the declared frame length immediately after the 4-byte prefix and the `max_frame_size` check. Nothing verifies the peer actually delivers the bytes; the allocation is fully committed from 4 bytes of attacker- or bug-controlled input. The cap bounds a *single* frame, not the aggregate: no per-connection memory accounting, no staged growth.
- **Failure scenario:** 1,000 open connections each send only a length prefix of `0x00FF_FFFF` (16 MiB − 1) and then trickle or stall — ~16 GiB resident from 4 KB of wire input, held as long as each `read_exact` waits. `shamir-server`'s connection limiter bounds connection count, not bytes-buffered-per-connection, so the product is unguarded at this layer.
- **Suggested fix:** Stage the allocation: read into a reusable buffer that grows only as bytes actually arrive (start at a small soft cap such as `shamir_tunables::IO_FRAME_BUFFER_CAP` (4096), double while reading, abort with `TooLarge`/`Io` if the peer under-delivers), or add an explicit `max_prealloc` below `max_frame_size` switching slow paths to incremental reads. Cheaper alternative: keep the upfront alloc but document that deployments must pair this transport with per-connection buffered-byte caps.

### 4.2 [MEDIUM] Pooled scratch buffers grow monotonically to the frame high-water mark; the documented `shrink_to_fit` mitigation is implemented by nobody
- **File:line:** `src/framing.rs:89-91` (doc promise), `:117-124` (`clear()` + `reserve()`), `:168-173` (`write_frame_into` scratch).
- **Issue:** `read_frame_into`/`write_frame_into` deliberately keep capacity at the high-water mark ("Use `Vec::shrink_to_fit` periodically if memory is a concern"). A repo-wide grep shows the *only* occurrence of `shrink_to_fit` in the workspace is that doc line — no consumer (`shamir-server/src/connection/request_loop.rs`, `shamir-server/src/framer.rs`, `shamir-client/src/client.rs`) ever shrinks. Buffers start at `IO_FRAME_BUFFER_CAP` = 4096, so retention is invisible in normal traffic and only surfaces after one large frame. No test covers growth/retention behavior.
- **Failure scenario:** A connection serves one 16 MiB SELECT result; its read buffer pins ≥16 MiB and its write scratch up to another 16 MiB for the connection's remaining lifetime, even if every subsequent frame is 100 B. A fleet of long-lived connections that each once touched a large result retains ~32 MiB × N indefinitely — memory that looks like a leak in RSS monitoring and never returns.
- **Suggested fix:** Own the policy inside the crate instead of deferring to callers: add hysteresis (`if buf.capacity() > HIGH_WATER * 2 && buf.capacity() > SHRINK_FLOOR { buf.shrink_to_fit(); }` on frame completion or idle), or introduce a `FrameBuf` newtype encapsulating grow/shrink. At minimum, add the shrink call at the two real consumer sites plus a regression test pinning the policy.

### 4.3 [LOW] Two `read_exact` calls per frame: extra read round-trip on unbuffered plain-TCP streams
- **File:line:** `src/framing.rs:49-50` (`read_frame`), `:103-104` (`read_frame_into`).
- **Issue:** The 4-byte length prefix is read with its own `read_exact` before the payload read. On `tokio-rustls` streams this is absorbed by rustls' internal plaintext buffer, but on the sanctioned `ListenerProfile::Plain` loopback path and any raw `TcpStream`/unbuffered reader, every frame costs at least two read syscalls, and payload bytes coalesced into the same TCP segment are left in the kernel buffer. No `BufReader` guidance or helper exists anywhere in the crate.
- **Failure scenario:** Loopback/plain deployments (the documented same-host embedded use case) pay ~2× syscall overhead per frame in the request loop; at small frame sizes the fixed cost is a measurable fraction of per-op latency.
- **Suggested fix:** document that plain-profile consumers must wrap the stream in `tokio::io::BufReader`, or provide a buffered variant that peeks/consumes header and payload from one buffered read (`read_buf`-based header fill that can carry into the payload read).

### 4.4 [LOW] Allocating `read_frame`/`write_frame` remain the ergonomic defaults and are still on a production per-request hot path
- **File:line:** `src/framing.rs:45-70` (`read_frame`, fresh `Vec` per call), `:149-166` (`write_frame`, 4+N `Vec` per call); consumer evidence: `crates/shamir-client/src/client.rs:1004` (per-request `write_frame` in the send loop).
- **Issue:** The crate correctly documents the allocating variants as the non-hot-path choice and ships pooled alternatives; the server uses the pooled/prereserved APIs throughout. The client, however, calls the allocating `write_frame` once per request on its send path (one heap allocation + memcpy of the full envelope per request, on top of the msgpack serialization allocation). Nothing marks the allocating variants as non-hot-path beyond prose.
- **Failure scenario:** Sustained client request streams keep a per-request malloc/free (and for large envelopes, a full extra payload memcpy) that the server side already eliminated — asymmetric hot-path cost and allocator contention under multi-connection clients.
- **Suggested fix:** Migrate `shamir-client`'s request write path to `write_frame_into` with a per-connection scratch (mirroring the server's request loop), and/or mark the allocating variants with explicit "do not use in per-frame loops" guidance so future consumers reach for the pooled API by default. *(Primary remediation lives in shamir-client; recorded here because the API surface is this crate's.)*

---

## 5. api-wire-protocol

**The public surface is clean and consistent with the spec's shape** — TLS 1.3-only configs,
exporter-based channel binding, a profile-aware bind validator, well-documented framing
primitives with pooled variants. The builder-only query-construction rule is honored: no raw
`serde_json` anywhere; protocol objects flow through `shamir-connect` builders, and the
test-local msgpack `Wire*` structs fall under the documented wire-format-test exception. The
gaps are API-asymmetry/diagnosability items and doc drift (the empty-frame contract and
`Box<dyn Error>` items live under their primary lenses 1.1 and 6.2).

### 5.1 [LOW] Write-side frame cap is hardcoded to `MAX_FRAME_SIZE_DEFAULT`; reader/writer API asymmetry
- **File:line:** `src/framing.rs:153`, `:192`, `:250`.
- **Issue:** `read_frame`/`read_frame_into` take a `max_frame_size` parameter (doc calls it "the negotiated maximum"), but all three write variants check against the compile-time `MAX_FRAME_SIZE_DEFAULT` only. The writer cannot honor a negotiated cap (the read-side API is clearly built for that), and `shamir-server`'s pre-auth tightening pattern (`MAX_PRE_AUTH_FRAME` on read) cannot be mirrored on write.
- **Failure scenario:** A future negotiated cap of e.g. 1 MiB: local `write_frame` succeeds with a 4 MiB payload; peer's `read_frame(1 MiB)` returns `TooLarge` and drops the connection (spec §2/§8: "Frame too large → TCP close without reply") — a bug that only manifests cross-version, asymmetrically killing the connection.
- **Suggested fix:** Thread `max_frame_size` through the write functions (or take it once in a shared `FrameLimits` struct / `write_frame_capped` companions; existing call sites pass `MAX_FRAME_SIZE_DEFAULT`). If the cap is genuinely fixed by spec forever, document that on the read-side parameter so the asymmetry is explicit rather than latent.
- *Also flagged by: correctness-tdd finding 4, error-handling-lifecycle finding 9 (nit), and the cap half of security-crypto finding 5 (nit) — same root cause; merged here.*

### 5.2 [LOW] Normative loopback predicate is not reusable, so shamir-server duplicates spec §2.2 policy
- **File:line:** `src/listener.rs:51-56` (private `is_loopback`), `:89-94`; consumer: `crates/shamir-server/src/config.rs:841-850`.
- **Issue:** `validate_addr`/`ListenerProfile::allows` require the crate's `ListenerProfile` enum, which the server's own `ProfileKind` does not map to; shamir-server therefore re-implements the loopback predicate inline (its comment says exactly this). The TRANSPORT_TCP §2.2 NORMATIVE policy now lives in two independently-maintained copies.
- **Failure scenario:** a future spec change to the allowed loopback range (or additional address classes) updated in one copy but not the other yields two validators that disagree about the same bind address.
- **Suggested fix:** expose the pure predicate (`pub fn is_loopback_ip(ip: IpAddr) -> bool` in `listener.rs`, spec-cited) so `validate_addr` and shamir-server share one implementation; optionally add `From<ListenerProfile>`-style interop with the server's profile type.

### 5.3 [NIT] Docs advertise Unix-domain-socket binds for `Plain`; the API is `SocketAddr`/TCP-only
- **File:line:** `src/listener.rs:4-6` (module doc), `:30-35` (`Plain` variant doc).
- **Issue:** Both docs say Plain is permitted on loopback "or Unix domain sockets", but the crate offers only `SocketAddr`-based validation/bind; no UDS path or `allows_unix` predicate exists. An operator or contributor reading the spec-guided docs finds no API to express or validate a UDS bind.
- **Suggested fix:** drop the UDS clause until a UDS bind helper lands, or add an explicit `ListenerProfile::allows_unix_path()` / `bind_validated_unix` stub with the same fail-closed policy.
- *Also flagged by: style-claude-md finding 7 (same doc; merged here).*

### 5.4 [NIT] Exporter doc cluster misdescribes the adapter abstraction
- **File:line:** `src/tls.rs:73-74` (fn doc claims "Generic over `T: rustls::ConnectionTrait`" — the signature is actually generic over the crate-local `ConnectionExporter`, `:88-96`), `:85-88` (trait doc claims the streams "implement" the trait — they do not; the local impls for `tokio_rustls::{server,client}::TlsStream` appear at `:98`/`:111`).
- **Issue:** The stated rationale ("avoids importing rustls's stable trait") is undercut by the trait's signature referencing `rustls::Error` anyway and the impl bodies calling rustls's own `export_keying_material` directly. The doc misleads contributors about the extension point (e.g. `shamir-transport-ws/src/tls_exporter.rs` must impl the local trait, not rustls's).
- **Suggested fix:** reword the fn doc to "generic over `ConnectionExporter` (impl'd for both tokio-rustls stream halves; other backends add their own impl)" and the trait doc to describe the adapter pattern.
- *Also flagged by: style-claude-md finding 6 (trait-side doc; same doc cluster, merged here).*

### 5.5 [NIT] `tokio` dependency pulls `features = ["full"]` in a library crate
- **File:line:** `Cargo.toml:14`.
- **Issue:** A transport library needs only `net`, `io-util`, `rt` (and `macros`/`time` for dev/tests); `full` unions signal/process/fs/etc. into every downstream build via feature unification and masks which capabilities the crate actually uses. Compile-time/binary-surface bloat for consumers only.
- **Suggested fix:** declare the minimal feature set in `[dependencies]` and the extras under `[dev-dependencies]`.

### 5.6 [NIT] Transport wire structs and fixtures duplicated across the two e2e test files
- **File:line:** `tests/handshake_e2e.rs:41-111`, `tests/echo_e2e.rs:57-146`.
- **Issue:** `WireAuthInit`/`WireChallenge`/`WireClientProof`/`WireAuthOk`, `fast_kdf()`, and `make_user()` are copy-pasted between the two integration tests; each file independently defines what it asserts to be the transport-local wire format.
- **Failure scenario:** a spec §6 envelope change updated in one file but not the other leaves both tests green while they encode two different wire formats.
- **Suggested fix:** share the `Wire*` structs and fixtures via a `tests/common/mod.rs`-style helper (or move them into a `#[cfg(test)]` unit module under `src/tests/`).

---

## 6. error-handling-lifecycle

**The crate largely honors the documented discipline:** `FrameError`/`ListenerBindError` are
proper `thiserror` enums, `?` throughout, bind validation fails closed *before* the socket
exists, and the framing error paths are unusually well tested (including a Miri-run test pinning
the `buf.clear()` cleanup, `tests/framing.rs:226-250`). The weak spots concentrate in `tls.rs`
(its error surface forces the only server consumer to stringify errors; latent panic paths the
crate's own tests paper over by pre-installing a crypto provider) and in contract hygiene.

### 6.1 [MEDIUM] `write_frame_prereserved` reports every contract violation as `TooLarge`
- **File:line:** `src/framing.rs:241-246` (`buf.len() < 4` → `TooLarge { actual: 0, max: 16 MiB }`), `:250-255` (declared/actual mismatch → `TooLarge`); test enshrining it: `tests/framing.rs:309-324`; downstream: `shamir-server/src/framer.rs:118-122,190-193` pre-checks, `:244-245`, `map_tcp_err` at `:313-320` forwards verbatim.
- **Issue:** A `buf.len() < 4` input is a *malformed* argument, not an oversized frame, yet it returns `FrameError::TooLarge { actual: 0 }` — the `Display` output ("frame too large: 0 > 16777216") is simply false. A prefix/payload mismatch also yields `TooLarge` even when both numbers are far below the cap ("frame too large: 999 > 16777216" when the real problem is 999 ≠ 100). `FrameError` has no `Malformed` variant, so two distinct programmer bugs collapse into a misleading size-limit classification. The test `write_frame_prereserved_rejects_malformed_buffers` asserts the wrong variant, cementing it; `shamir-server`'s framer default impls already pre-check `buf.len() < 4` and return their own `FramerError::Decode` — evidence the transport-level error was unusable as-is.
- **Failure scenario:** a serialization change writes a wrong prefix; dashboards/log-based `TooLarge` rate alerts fire and the investigation chases a phantom malicious/oversized peer instead of the local bug.
- **Suggested fix:** dedicated variants for caller-contract violations, e.g. `PrefixTooShort(usize)` and `PrefixMismatch { declared, actual }` (or a `Malformed { reason }`). These are programmer bugs, a different error class from the protocol-limit rejection; keep `TooLarge` exclusively for size-cap violations; update the enshrining test; drop shamir-server's now-redundant pre-checks or leave them as defense-in-depth.
- *Also flagged by: correctness-tdd finding 3, api-wire-protocol finding 3, and the sentinel half of security-crypto finding 5 (nit) — same root cause; merged here.*

### 6.2 [MEDIUM] `Box<dyn Error + Send + Sync>` in library APIs instead of a `thiserror` enum
- **File:line:** `src/tls.rs:30` (`generate_self_signed_server_cert`), `:44` (`make_server_config_from_pem`), plus the ad-hoc stringly error at `:50` (`ok_or("no PKCS8 key in PEM")??`).
- **Issue:** CLAUDE.md: "`thiserror` for library error enums (with `#[from]` where natural)"; "`Box<dyn Error>` is a last resort for boundary code." These are the crate's public, boot-critical APIs; the crate already depends on `thiserror` and uses it correctly for `FrameError`/`ListenerBindError`. The cost is visible downstream: `shamir-server/src/tls.rs:92,116` must stringify (`TlsError::Build(e.to_string())`), collapsing rcgen generation failures, PEM-parse failures, and rustls config-build failures into one opaque `String`. Callers cannot distinguish "malformed key material" from "malformed cert chain" programmatically — relevant at a security boundary where an operator needs to know whether to replace the key file or the cert file — and can't `matches!` on specific causes (which also degrades the testability of 1.2).
- **Failure scenario:** an operator misconfiguration or a disk fault truncates a PEM; the server logs `tls build: <flat string>` and there is no programmatic way for the launcher/tests to branch on the failure class; string-matching breaks across rustls/rcgen upgrades.
- **Suggested fix:** `#[derive(Debug, Error)] pub enum TlsConfigError { #[from] Rcgen(rcgen::Error), #[from] Pem(rustls_pki_types::pem::Error), #[from] Rustls(rustls::Error), #[error("no PKCS8 key in PEM")] NoKeyInPem }` and return it from both functions, matching the crate's own `FrameError`/`ListenerBindError` pattern.
- *Also flagged by: api-wire-protocol finding 2 (medium), security-crypto finding 4 (low), correctness-tdd finding 8 (nit) — same root cause; merged here.*

### 6.3 [MEDIUM] TLS config builders can panic on missing/ambiguous rustls crypto provider
- **File:line:** `src/tls.rs:54` (`ServerConfig::builder_with_protocol_versions`), `:63-69` (`make_client_config_no_ca`, returning `Arc<ClientConfig>` with no `Result`).
- **Issue:** rustls 0.23's `builder_with_protocol_versions` resolves the *process-default* `CryptoProvider` and **panics** when none is installed and the unified crate features are ambiguous (both `ring` and `aws_lc_rs` present anywhere in the feature graph). `make_client_config_no_ca` returns `Arc<ClientConfig>` directly — not fallible — so this failure mode is unrepresentable in its signature. Every test in the crate must pre-install a provider (`let _ = default_provider().install_default();` in `tests/handshake_e2e.rs:116`, `tests/tls13_only.rs:19-21`, `tests/echo_e2e.rs:154`), evidence the library depends on ambient process state it neither sets nor checks; there are 60+ scattered `install_default()` call sites workspace-wide and `shamir-client` even installs the provider inside its own library code (`shamir-client/src/client.rs:377-379`). This violates CLAUDE.md's "Return `Result<T, E>`. Avoid `panic!`" pillar.
- **Failure scenario:** a future workspace dependency enables rustls's `ring` feature; feature unification makes the provider ambiguous; `make_server_config_from_pem` then panics at server boot (`shamir-server/src/tls.rs::load_or_generate` installs no provider) instead of returning an `Err` the boot path could report. An external consumer calling `make_client_config_no_ca()` before installing a provider panics at runtime instead of getting a typed error.
- **Suggested fix:** thread an explicit provider: `ClientConfig::builder_with_provider(provider).with_protocol_versions(&[&rustls::version::TLS13])` (returns `Result`), or at minimum fetch `CryptoProvider::get_default()` and return a typed error when absent; make both functions return `Result<_, TlsConfigError>` (see 6.2). Minimum interim: document the requirement on both constructors.
- *Also flagged by: correctness-tdd finding 6 (same root cause; merged here).*

### 6.4 [LOW] Post-error stream state is undocumented (`TooLarge` desyncs the stream)
- **File:line:** `src/framing.rs:45-70` (`read_frame`), `:98-138` (`read_frame_into`).
- **Issue:** after `TooLarge`, the 4-byte prefix is consumed but the peer's payload bytes are still in flight; after *any* `Err` the stream is mid-frame. The public docs say nothing about this. The production caller happens to do the right thing (`shamir-server/src/connection/request_loop.rs:283-287` breaks on any `Err` and tears down), but that invariant lives in the consumer, not in the crate that owns the contract. (`PeerClose` is the one benign case — nothing follows a length-zero frame.)
- **Failure scenario:** a new caller matches `TooLarge` to send an error envelope and keeps reading the same stream; the abandoned payload bytes are parsed as the next length prefix, producing garbage frames or a cascade of false `TooLarge`/`PeerClose` errors.
- **Suggested fix:** document on both read functions: "On any `Err` other than `PeerClose` the stream is left desynchronized — the caller MUST drop the connection."

### 6.5 [LOW] Cancel-safety contract lives in the consumer, not the crate
- **File:line:** `src/framing.rs:45-138`.
- **Issue:** tokio's `read_exact` is not cancel-safe: dropping a `read_frame(_into)` future mid-frame silently consumes N bytes and desyncs the stream *without any error surfacing*. `request_loop.rs:231-236` (and `:276-277`) documents locally why its `select!` is safe ("Cancel-safety of the read branch is intentionally NOT required"); the knowledge exists only in that consumer. *(Distinct defect from 3.1: this is the stream-position/protocol desync on future-drop, which exists for the safe allocating `read_frame` too; 3.1 is the uninit-memory hazard. The trigger — `select!`/`timeout` cancellation — is shared.)*
- **Failure scenario:** a future caller wraps these functions in `tokio::time::timeout` (a natural per-read slow-loris defense) and continues reading after the timeout — frames silently split, payloads corrupt, with no error to catch.
- **Suggested fix:** add a `# Cancel-safety` doc section to `read_frame`/`read_frame_into`: the future must run to completion, or the stream must be discarded.

---

## 7. style-claude-md

**Structural conformance is largely strong:** `src/tests/mod.rs` is a manifest-only `mod.rs`, no
inline `#[cfg(test)] mod tests` in any implementation file, `lib.rs` holds only module
declarations, test wiring, and re-exports, all four `src/` files keep imports at top,
one-file-one-primary-export holds (`framing.rs` a tightly-coupled codec group, `tls.rs` a
coherent TLS-wiring group, `listener.rs` one policy concern), bench uses
`bench_scale_tool::Harness` (no Criterion), `thiserror` for both public error enums, doctests
disabled crate-wide with examples marked `rust,ignore`. The two real drifts are at the crate
edges (re-exports, test-file imports).

### 7.1 [MEDIUM] Crate-root re-exports are partial, stale, and used by nobody
- **File:line:** `src/lib.rs:12-19` (vs `src/framing.rs`, `src/tls.rs`).
- **Issue:** `pub use framing::{read_frame, write_frame, FrameError, MAX_FRAME_SIZE_DEFAULT}` and the tls block snapshot only the initial-cut API. Everything added since is missing from the root: `read_frame_into` (framing.rs:98), `write_frame_into` (:187), `write_frame_prereserved` (:233), `write_close` (:209), the `ConnectionExporter` trait (tls.rs:88), `EXPORTER_LABEL`/`EXPORTER_CONTEXT` (tls.rs:18,20) — even though the framing docs steer hot-path callers to the pooled variants. A workspace-wide grep finds **zero** imports through the root paths — every consumer (`shamir-server`, `shamir-client`, `shamir-transport-ws`, and this crate's own tests/bench) uses `shamir_transport_tcp::framing::*` / `::tls::*` / `::listener::*`.
- **Failure scenario:** a reader treats the root block as the canonical API surface, writes `use shamir_transport_tcp::read_frame_into;`, and fails to compile; each new public framing/tls item silently widens the gap, leaving two divergent import paths for the same crate (already visible: `shamir_transport_tcp::framing::write_frame` in `shamir-client/src/tests/demux_tests.rs:16` vs the root re-export elsewhere).
- **Suggested fix:** delete the root re-export block (module paths are the de-facto canonical surface) or complete it. If kept, also move `#[cfg(test)] mod tests;` (lib.rs:13-14) up beside the `pub mod` declarations so the file reads module decls → test wiring → re-exports.
- *Also flagged by: api-wire-protocol finding 7 (low) — same root cause; merged here.*

### 7.2 [MEDIUM] Function-local `use` statements violate the imports-at-top rule (5 sites)
- **File:line:** `tests/framing.rs:50`, `:155`; `tests/tls13_only.rs:36-38`; `tests/echo_e2e.rs:208`.
- **Issue:** CLAUDE.md ("Imports at the top") mandates all `use` statements in the file header, with three documented exceptions — none applies here: `tests/framing.rs:50,155` (`use tokio::io::AsyncWriteExt;` inside two tests; no collision exists, hoisting trivial); `tests/tls13_only.rs:36-38` (three imports inside `tls12_only_server_config`, while the file header at line 11 already imports `CertificateDer` from the same path — mechanical merge); `tests/echo_e2e.rs:208` (`use shamir_connect::common::latency::{target_constant_time_ms, LatencyPadGuard};` inside the server-task async block; the surrounding comment explains latency padding, not import scope). All `src/` files are clean — violations confined to `tests/`.
- **Failure scenario:** none functional; a direct, repeated breach of a documented mandatory convention, inconsistent with the crate's otherwise clean `src/` imports.
- **Suggested fix:** hoist all five imports to their file headers.

### 7.3 [LOW] Speculative dead public consts with cargo-cult `#[allow(dead_code)]`
- **File:line:** `src/listener.rs:96-100`.
- **Issue:** `LOOPBACK_V4`/`LOOPBACK_V6` are referenced nowhere in the workspace — not even by this crate's own tests (`src/tests/listener_tests.rs` builds addresses by string parsing) — despite the comment "Common loopback addresses for documentation / examples / tests". The `#[allow(dead_code)]` attributes are pure noise: dead_code never fires on `pub` items reachable from a lib crate root. Both constants restate std (`Ipv4Addr::LOCALHOST`/`Ipv6Addr::LOCALHOST`).
- **Failure scenario:** none functional; misleading public surface plus allow-attributes advertising expected dead code.
- **Suggested fix:** delete both consts (or fold them into the reusable predicate from 5.2's fix); alternatively keep them non-`pub` and actually use them in `listener_tests.rs`.
- *Also flagged by: api-wire-protocol finding 10 (nit) and the allow-attribute half of correctness-tdd finding 9 (nit) — same root cause; merged here.*

### 7.4 [LOW] In-src `tests/` layout covers only one of three modules
- **File:line:** `src/tests/mod.rs:1`.
- **Issue:** `src/tests/` exists and conforms to the manifest-only rule, but declares only `listener_tests`. `framing.rs` has extensive pure-logic codec tests — including the explicitly miri-safe `std::io::Cursor` pair (`tests/framing.rs:203-250`) that needs no I/O at all — living at the crate-root `tests/`, and `tls.rs` has no unit-level tests under `src/tests/` at all (see 1.2). The "one `tests/` directory per module" layout is only partially realized; crate-root `tests/` is legitimate for socket-bound e2e coverage, but the pure-logic cases have two competing homes.
- **Failure scenario:** none; consistency drift — a contributor adding a framing unit test finds no in-src precedent and follows the integration-file pattern instead.
- **Suggested fix:** move the pure codec tests (at minimum the Cursor/miri pair) into `src/tests/framing_tests.rs` and add a `tls_tests.rs` for the pure config builders; keep the socket-bound e2e files in `tests/`.
- *Also flagged by: the layout half of correctness-tdd finding 9 (nit) — same root cause; merged here.*

### 7.5 [LOW] Spec wire constant duplicated from a direct dependency
- **File:line:** `src/tls.rs:18` (dup of `crates/shamir-connect/src/common/domain_tags.rs:35`); related: `EXPORTER_CONTEXT` (`:20`) is `pub` with no external consumer anywhere in the workspace.
- **Issue:** `EXPORTER_LABEL: &[u8] = b"EXPORTER-ShamirDB-AUTH-v1"` re-defines `shamir_connect::common::domain_tags::TLS_EXPORTER_LABEL` byte-for-byte, and this crate already depends on `shamir-connect` (`Cargo.toml:11`).
- **Failure scenario:** the label is versioned ("...-AUTH-v1"); if a v2 label lands in `domain_tags` and this copy is missed, client and server derive channel-binding material under different labels — a silent protocol mismatch that an e2e test mocking both ends through these same constants would not catch.
- **Suggested fix:** `use shamir_connect::common::domain_tags::TLS_EXPORTER_LABEL;` (re-export under the `tls::` path if stability is needed) and make `EXPORTER_CONTEXT` private.

### 7.6 [NIT] Mixed spellings of the same upstream crate in one import block
- **File:line:** `src/tls.rs:10,12-13`.
- **Issue:** the header imports the same upstream module via two paths — `rustls::pki_types::{...}` (rustls's re-export, line 10) and `rustls_pki_types::{...}` (the direct dependency, lines 12-13) — in the same file.
- **Suggested fix:** pick one spelling (prefer the direct `rustls_pki_types` paths, matching the RUSTSEC-2025-0134 rationale in `Cargo.toml:21-23`) and note the choice.

---

## Finding counts

Raw lens-tagged findings across the seven source files: **49** (0 critical · 1 high · 12 medium ·
22 low · 14 nit) — identical to this crate's row in the workspace `SUMMARY.md` per-crate table
(those are pre-dedup lens-tagged counts). After merging same-root-cause defects flagged by
multiple lenses onto their primary lens: **30 distinct defects**.

| Severity | Lens-tagged findings | Distinct defects (deduped) | Deduped finding numbers |
|---|---|---|---|
| critical | 0 | 0 | — |
| high | 1 | 1 | 3.1 (unsafe uninit read path) |
| medium | 12 | 10 | 1.1, 1.2, 3.2, 4.1, 4.2, 6.1, 6.2, 6.3, 7.1, 7.2 |
| low | 22 | 13 | 1.3, 1.4, 2.1, 3.3, 4.3, 4.4, 5.1, 5.2, 6.4, 6.5, 7.3, 7.4, 7.5 |
| nit | 14 | 6 | 2.2, 5.3, 5.4, 5.5, 5.6, 7.6 |
| **total** | **49** | **30** | 1 high · 10 medium · 13 low · 6 nit |

Dedup mapping (raw source-file findings → primary entry here; severity of a group = max of its
members):

- **1.1** = correctness #1 + api #1 · **1.2** = correctness #2 + error-handling #4a · **1.4** = correctness #7 + error-handling #4b · **1.3** = correctness #5 (unmerged)
- **2.1** = concurrency #1 · **2.2** = concurrency #2 (unmerged)
- **3.1** = security #1 + error-handling #8 (nit, subsumed) · **3.2** = security #2 (unmerged) · **3.3** = security #3 + api #6 + error-handling #7
- **4.1–4.4** = performance #1–#4 (unmerged)
- **5.1** = api #4 + correctness #4 + error-handling #9 + security #5 (cap half) · **5.2** = api #5 · **5.3** = api #9 + style #7 · **5.4** = api #8 + style #6 · **5.5** = api #11 · **5.6** = api #12
- **6.1** = error-handling #3 + correctness #3 + api #3 + security #5 (sentinel half) · **6.2** = error-handling #2 + api #2 + security #4 + correctness #8 · **6.3** = error-handling #1 + correctness #6 · **6.4** = error-handling #5 · **6.5** = error-handling #6 (kept distinct from 3.1: stream-desync vs uninit-memory)
- **7.1** = style #1 + api #7 · **7.2** = style #2 · **7.3** = style #4 + api #10 + correctness #9 (allow half) · **7.4** = style #5 + correctness #9 (layout half) · **7.5** = style #3 · **7.6** = style #8

(Security #5 was a bundle splitting across 5.1 and 6.1; correctness #9 split across 7.3 and 7.4;
error-handling #4 split across 1.2 and 1.4 — each half merged into its matching defect.)

## Fix Plan

**P0 — before anything else ships from this crate**

1. **Make the uninitialized window unobservable in `read_frame_into` (3.1 — the workspace
   headline finding).** Read into `buf.spare_capacity_mut()` via `ReadBuf::uninit` (small
   `poll_fn` loop over `AsyncRead::poll_read`) and `unsafe { buf.set_len(n) }` only after the
   read completes with n initialized bytes; interim hardening: a `Drop` guard that `clear()`s the
   buffer on *any* exit (covering `select!`/`timeout` cancellation), a `# Cancellation` doc
   section, and a `tokio::time::timeout`-based regression test beside the existing Miri pair.
   Closes **3.1** (including its subsumed SAFETY-form sub-point); the `# Cancel-safety` doc from
   item 9 should land in the same edit.
2. **Reserve `length == 0` exclusively for `write_close` (1.1).** Reject zero-length payloads in
   `write_frame`/`write_frame_into`/`write_frame_prereserved` (dedicated `FrameError::EmptyFrame`)
   and delete the false "Empty frames are also legal" doc sentence; add a pinning test. Closes
   **1.1**.
3. **Typed, non-panicking TLS config surface (6.2 + 6.3).** Introduce `TlsConfigError`
   (thiserror, `#[from]` rcgen/PEM/rustls + `NoKeyInPem`), return it from both constructors,
   thread an explicit `CryptoProvider` (`builder_with_provider`) or fail with a typed error when
   `get_default()` is absent; make `make_client_config_no_ca` fallible. This is also the
   enabler for item 7's per-variant assertions. Closes **6.2**, **6.3**.

**P1 — soon**

4. **Bound the read-path allocation (4.1):** staged growth (soft cap → double while reading) or
   an explicit `max_prealloc`; at minimum document the deployment pairing requirement. Closes
   **4.1**.
5. **Own the shrink policy in-crate (4.2):** hysteresis in `read_frame_into`/`write_frame_into`
   or a `FrameBuf` newtype, plus a regression test pinning growth/retention. Closes **4.2**.
6. **Malformed-prefix error variants (6.1):** `PrefixTooShort`/`PrefixMismatch` (or
   `Malformed { reason }`) for `write_frame_prereserved`'s contract checks; update the enshrining
   test (`tests/framing.rs:309-324`); re-evaluate shamir-server's duplicate pre-checks. Closes
   **6.1**.
7. **Close the test gaps (1.2 + 1.4 + 1.3):** a `tls.rs` error-branch suite (bad/empty PEM,
   no-PKCS8, cert/key mismatch, pre-handshake exporter `None` — assertable per variant once item
   3 lands), write-side oversized/EOF/IPv4-mapped pins, and rename the vacuous
   `bind_validated_succeeds_for_tls_on_unspecified` + add a real unspecified-address test. Closes
   **1.2**, **1.4**, **1.3**.
8. **Harden `NoCaVerify` (3.2):** verify the TLS 1.3 CertificateVerify against the end-entity
   cert's own key (rustls-webpki `EndEntityCert`), add a `# SECURITY` doc section naming the
   caller obligations, consider a pinned-hash enforcing variant. Closes **3.2**.
9. **`extract_tls_exporter` → `Result` (3.3):** typed distinction of `HandshakeNotComplete` from
   exporter refusal; document the fatal-on-None rule and the fixed 32-byte §4.2 contract. Closes
   **3.3**.
10. **Stream-contract doc sections (6.4 + 6.5):** "on any `Err` other than `PeerClose` the stream
    is desynchronized — drop the connection" and "the future must run to completion, or the
    stream must be discarded" on both read functions. Closes **6.4**, **6.5**.

**P2 — backlog**

11. **Symmetric frame limits + shared loopback predicate (5.1 + 5.2):** `max_frame_size`
    parameters (or a shared `FrameLimits`) on the write side; expose `pub fn is_loopback_ip` and
    have shamir-server drop its inline copy. Closes **5.1**, **5.2**.
12. **Re-export cleanup (7.1):** delete the root re-export block or complete it (including the
    `lib.rs:13-14` ordering). Closes **7.1**.
13. **Hoist the five function-local imports (7.2).** Closes **7.2**.
14. **Bootstrap/cancel hygiene in tests + docs (2.1 + 2.2):** document the bootstrap-only
    contract (or add `spawn_blocking` wrappers) for the two sync TLS helpers; switch the two KDF
    e2e sites to `multi_thread` flavor or `spawn_blocking`. Closes **2.1**, **2.2**.
15. **Single-source the spec constants + dead-code cleanup (7.5 + 7.3 + 7.4):** import
    `TLS_EXPORTER_LABEL` from `shamir-connect`, make `EXPORTER_CONTEXT` private, delete
    `LOOPBACK_V4`/`LOOPBACK_V6`, split per-module `tests/` dirs as the crate grows. Closes
    **7.5**, **7.3**, **7.4**.
16. **Doc/dep nits + consumer follow-through (4.3 + 4.4 + 5.3 + 5.4 + 5.5 + 5.6 + 7.6):**
    BufReader guidance for the plain profile; migrate shamir-client's send loop to
    `write_frame_into` (primary remediation lives in that crate); drop or implement the UDS doc
    clause; fix the exporter doc cluster; trim tokio features; share the e2e `Wire*` fixtures;
    pick one `rustls_pki_types` spelling. Closes **4.3**, **4.4**, **5.3**–**5.6**, **7.6**.

</details>
