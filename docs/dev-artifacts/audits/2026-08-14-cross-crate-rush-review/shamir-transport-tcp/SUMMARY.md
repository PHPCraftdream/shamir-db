<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-transport-tcp — SUMMARY revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

The headline pooled-read soundness defect remains open, along with close-marker handling, TLS/API diagnostics and test gaps. No listed remediation is source-proven complete. The synthesis needs factual corrections for provider initialization, exporter fallback, replay, memory retention, measurements and style severity.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 30 | 29 | 0 | 0 | 0 | 1 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1-1"></a>

### Claim 1.1 — Zero-length payload write silently emits the close sentinel; module doc contradicts spec §2

Status: `confirmed-open`. Current risk: `medium`.

All three data writers still accept zero-length payloads; both readers return PeerClose and the contradictory empty-data documentation remains.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:8](../../../../../crates/shamir-transport-tcp/src/framing.rs#L8); [crates/shamir-transport-tcp/src/framing.rs:53](../../../../../crates/shamir-transport-tcp/src/framing.rs#L53); [crates/shamir-transport-tcp/src/framing.rs:192](../../../../../crates/shamir-transport-tcp/src/framing.rs#L192); [crates/shamir-transport-tcp/src/framing.rs:250](../../../../../crates/shamir-transport-tcp/src/framing.rs#L250).

<a id="review-1-2"></a>

### Claim 1.2 — `tls.rs` has zero unit tests; all error branches unexercised (happy-path-only e2e coverage)

Status: `confirmed-open`. Current risk: `medium`.

Only listener_tests is registered as a unit module. Reviewed transport integration and server lifecycle tests do not assert the specified PEM/key/mismatch/exporter failure branches.

Evidence: [crates/shamir-transport-tcp/src/tests/mod.rs:1](../../../../../crates/shamir-transport-tcp/src/tests/mod.rs#L1); [crates/shamir-transport-tcp/src/tls.rs:45](../../../../../crates/shamir-transport-tcp/src/tls.rs#L45); [crates/shamir-transport-tcp/tests/handshake_e2e.rs:131](../../../../../crates/shamir-transport-tcp/tests/handshake_e2e.rs#L131).

<a id="review-1-3"></a>

### Claim 1.3 — Test name promises TLS-bind-on-unspecified coverage the body deliberately doesn't deliver

Status: `confirmed-open`. Current risk: `low`.

The named unspecified bind test still uses 127.0.0.1; predicate coverage does not test bind-time pass-through for an unspecified TLS address.

Evidence: [crates/shamir-transport-tcp/src/tests/listener_tests.rs:58](../../../../../crates/shamir-transport-tcp/src/tests/listener_tests.rs#L58); [crates/shamir-transport-tcp/src/tests/listener_tests.rs:101](../../../../../crates/shamir-transport-tcp/src/tests/listener_tests.rs#L101).

<a id="review-1-4"></a>

### Claim 1.4 — Write-path boundary branches, EOF-vs-close distinction, and IPv4-mapped-IPv6 edge untested

Status: `confirmed-open`. Current risk: `low`.

Writer oversize and mapped-address pins remain absent. A truncated EOF cleanup test exists but lacks the claimed Io(UnexpectedEof) versus PeerClose assertion.

Evidence: [crates/shamir-transport-tcp/tests/framing.rs:226](../../../../../crates/shamir-transport-tcp/tests/framing.rs#L226); [crates/shamir-transport-tcp/tests/framing.rs:240](../../../../../crates/shamir-transport-tcp/tests/framing.rs#L240); [crates/shamir-transport-tcp/tests/framing.rs:309](../../../../../crates/shamir-transport-tcp/tests/framing.rs#L309); [crates/shamir-transport-tcp/src/tests/listener_tests.rs:14](../../../../../crates/shamir-transport-tcp/src/tests/listener_tests.rs#L14).

<a id="review-2-1"></a>

### Claim 2.1 — Sync CPU-bound bootstrap helpers lack a spawn_blocking / bootstrap-only contract

Status: `confirmed-open`. Current risk: `low`.

The synchronous TLS helpers still lack runtime/bootstrap guidance. Current production invocation is startup initialization; a hot-path stall is hypothetical.

Evidence: [crates/shamir-transport-tcp/src/tls.rs:22](../../../../../crates/shamir-transport-tcp/src/tls.rs#L22); [crates/shamir-transport-tcp/src/tls.rs:40](../../../../../crates/shamir-transport-tcp/src/tls.rs#L40); [crates/shamir-server/src/server/server_launcher.rs:601](../../../../../crates/shamir-server/src/server/server_launcher.rs#L601).

<a id="review-2-2"></a>

### Claim 2.2 — e2e tests run Argon2id inline on the test runtime thread while a peer task is spawned

Status: `confirmed-open`. Current risk: `nit`.

Both tests retain current-thread runtimes and direct synchronous challenge derivation. Blocking the worker is structural; the forecast future timeout is not demonstrated.

Evidence: [crates/shamir-transport-tcp/tests/handshake_e2e.rs:113](../../../../../crates/shamir-transport-tcp/tests/handshake_e2e.rs#L113); [crates/shamir-transport-tcp/tests/handshake_e2e.rs:278](../../../../../crates/shamir-transport-tcp/tests/handshake_e2e.rs#L278); [crates/shamir-transport-tcp/tests/echo_e2e.rs:152](../../../../../crates/shamir-transport-tcp/tests/echo_e2e.rs#L152); [crates/shamir-transport-tcp/tests/echo_e2e.rs:376](../../../../../crates/shamir-transport-tcp/tests/echo_e2e.rs#L376).

<a id="review-3-1"></a>

### Claim 3.1 — `read_frame_into`: unsafe `set_len` before `.await` — formal UB on uninit memory, and cancellation leaves `buf.len()` covering uninitialized bytes

Status: `confirmed-open`. Current risk: `high`.

Accepted fresh/grown buffers expose uninitialized bytes through read_exact's initialized ReadBuf contract. Cancellation skips clearing. Production adapters and timeout/select callers reach it, but discard cancelled buffers; no remote disclosure was established.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:124](../../../../../crates/shamir-transport-tcp/src/framing.rs#L124); [crates/shamir-transport-tcp/src/framing.rs:129](../../../../../crates/shamir-transport-tcp/src/framing.rs#L129); [crates/shamir-server/src/framer.rs:224](../../../../../crates/shamir-server/src/framer.rs#L224); [crates/shamir-server/src/connection/handshake.rs:717](../../../../../crates/shamir-server/src/connection/handshake.rs#L717); [crates/shamir-server/src/connection/request_loop.rs:280](../../../../../crates/shamir-server/src/connection/request_loop.rs#L280).

<a id="review-3-2"></a>

### Claim 3.2 — `NoCaVerify` disables all server authentication — chain, CertificateVerify proof-of-possession, and hostname — guarded only by a doc comment

Status: `confirmed-open`. Current risk: `medium`.

Both verifier assertions remain unconditional. Missing CertificateVerify validation is an API hardening concern; current Rust client protocol authentication checks the pin, SCRAM signature and exporter-bound identity signature.

Evidence: [crates/shamir-transport-tcp/src/tls.rs:138](../../../../../crates/shamir-transport-tcp/src/tls.rs#L138); [crates/shamir-transport-tcp/src/tls.rs:158](../../../../../crates/shamir-transport-tcp/src/tls.rs#L158); [crates/shamir-client/src/client.rs:614](../../../../../crates/shamir-client/src/client.rs#L614); [crates/shamir-connect/src/client/handshake.rs:258](../../../../../crates/shamir-connect/src/client/handshake.rs#L258).

<a id="review-3-3"></a>

### Claim 3.3 — `extract_tls_exporter` collapses all errors (incl. incomplete handshake) into `None`, inviting constant/absent channel binding

Status: `confirmed-open`. Current risk: `low`.

Error erasure remains. Production server TCP/native-WS callers already fall back to zeros after successful TLS handshakes, while the client returns an error. None reachability in normal established TLS 1.3 is not demonstrated; output length is already documented.

Evidence: [crates/shamir-transport-tcp/src/tls.rs:71](../../../../../crates/shamir-transport-tcp/src/tls.rs#L71); [crates/shamir-transport-tcp/src/tls.rs:81](../../../../../crates/shamir-transport-tcp/src/tls.rs#L81); [crates/shamir-server/src/server/server_launcher.rs:1163](../../../../../crates/shamir-server/src/server/server_launcher.rs#L1163); [crates/shamir-server/src/server/server_launcher.rs:1391](../../../../../crates/shamir-server/src/server/server_launcher.rs#L1391); [crates/shamir-client/src/client.rs:480](../../../../../crates/shamir-client/src/client.rs#L480).

<a id="review-4-1"></a>

### Claim 4.1 — Full-size allocation on declared length before any payload byte arrives (16 MiB amplification per connection)

Status: `confirmed-open`. Current risk: `medium`.

Readers still allocate accepted declared length before payload arrival. Server pre-authentication allocation is capped at 4 KiB; post-authentication uses 16 MiB with an idle deadline. Capacity amplification is proven, resident totals are not.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:67](../../../../../crates/shamir-transport-tcp/src/framing.rs#L67); [crates/shamir-transport-tcp/src/framing.rs:124](../../../../../crates/shamir-transport-tcp/src/framing.rs#L124); [crates/shamir-server/src/connection/handshake.rs:714](../../../../../crates/shamir-server/src/connection/handshake.rs#L714); [crates/shamir-server/src/connection/request_loop.rs:297](../../../../../crates/shamir-server/src/connection/request_loop.rs#L297).

<a id="review-4-2"></a>

### Claim 4.2 — Pooled scratch buffers grow monotonically to the frame high-water mark; the documented `shrink_to_fit` mitigation is implemented by nobody

Status: `confirmed-open`. Current risk: `medium`.

Helpers and persistent client reader retain high-water capacity without shrink policy. Server requests use fresh owned buffers and prereserved replies, so the synthesis's universal 32 MiB server lifetime-retention scenario is unsupported.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:89](../../../../../crates/shamir-transport-tcp/src/framing.rs#L89); [crates/shamir-transport-tcp/src/framing.rs:199](../../../../../crates/shamir-transport-tcp/src/framing.rs#L199); [crates/shamir-client/src/client.rs:312](../../../../../crates/shamir-client/src/client.rs#L312); [crates/shamir-server/src/connection/request_loop.rs:278](../../../../../crates/shamir-server/src/connection/request_loop.rs#L278); [crates/shamir-server/src/connection/request_loop.rs:198](../../../../../crates/shamir-server/src/connection/request_loop.rs#L198).

<a id="review-4-3"></a>

### Claim 4.3 — Two `read_exact` calls per frame: extra read round-trip on unbuffered plain-TCP streams

Status: `unverified`. Current risk: `low` (provisional; not a confirmed defect).

Two separate read operations and absent buffering guidance are source-supported; universal syscall counts, approximately doubled overhead and latency effects lack measurement.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:50](../../../../../crates/shamir-transport-tcp/src/framing.rs#L50); [crates/shamir-transport-tcp/src/framing.rs:68](../../../../../crates/shamir-transport-tcp/src/framing.rs#L68); [crates/shamir-transport-tcp/src/framing.rs:104](../../../../../crates/shamir-transport-tcp/src/framing.rs#L104); [crates/shamir-transport-tcp/src/framing.rs:129](../../../../../crates/shamir-transport-tcp/src/framing.rs#L129).

<a id="review-4-4"></a>

### Claim 4.4 — Allocating `read_frame`/`write_frame` remain the ergonomic defaults and are still on a production per-request hot path

Status: `confirmed-open`. Current risk: `low`.

The client request path still invokes write_frame, causing a combined-buffer allocation and payload copy. No measured latency or contention impact is claimed.

Evidence: [crates/shamir-client/src/client.rs:1278](../../../../../crates/shamir-client/src/client.rs#L1278); [crates/shamir-transport-tcp/src/framing.rs:160](../../../../../crates/shamir-transport-tcp/src/framing.rs#L160).

<a id="review-5-1"></a>

### Claim 5.1 — Write-side frame cap is hardcoded to `MAX_FRAME_SIZE_DEFAULT`; reader/writer API asymmetry

Status: `confirmed-open`. Current risk: `low`.

Writers remain fixed at 16 MiB and readers remain parameterized. Current spec has phase-specific fixed limits; negotiated-limit failures remain hypothetical.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:47](../../../../../crates/shamir-transport-tcp/src/framing.rs#L47); [crates/shamir-transport-tcp/src/framing.rs:153](../../../../../crates/shamir-transport-tcp/src/framing.rs#L153); [crates/shamir-transport-tcp/src/framing.rs:192](../../../../../crates/shamir-transport-tcp/src/framing.rs#L192); [crates/shamir-transport-tcp/src/framing.rs:250](../../../../../crates/shamir-transport-tcp/src/framing.rs#L250).

<a id="review-5-2"></a>

### Claim 5.2 — Normative loopback predicate is not reusable, so shamir-server duplicates spec §2.2 policy

Status: `confirmed-open`. Current risk: `low`.

Both private predicates remain, currently equivalent. No shared exported predicate or actual policy disagreement exists.

Evidence: [crates/shamir-transport-tcp/src/listener.rs:51](../../../../../crates/shamir-transport-tcp/src/listener.rs#L51); [crates/shamir-server/src/config.rs:877](../../../../../crates/shamir-server/src/config.rs#L877).

<a id="review-5-3"></a>

### Claim 5.3 — Docs advertise Unix-domain-socket binds for `Plain`; the API is `SocketAddr`/TCP-only

Status: `confirmed-open`. Current risk: `nit`.

UDS wording remains unqualified despite a TCP-only enforcement surface. The broader specification legitimately permits UDS.

Evidence: [crates/shamir-transport-tcp/src/listener.rs:4](../../../../../crates/shamir-transport-tcp/src/listener.rs#L4); [crates/shamir-transport-tcp/src/listener.rs:30](../../../../../crates/shamir-transport-tcp/src/listener.rs#L30); [crates/shamir-transport-tcp/src/listener.rs:75](../../../../../crates/shamir-transport-tcp/src/listener.rs#L75).

<a id="review-5-4"></a>

### Claim 5.4 — Exporter doc cluster misdescribes the adapter abstraction

Status: `confirmed-open`. Current risk: `nit`.

Function docs still name rustls::ConnectionTrait and trait docs claim unsupported direct rustls implementations/version independence. Tokio-rustls streams do implement the local trait through the explicit impls, contrary to the synthesis's wording.

Evidence: [crates/shamir-transport-tcp/src/tls.rs:73](../../../../../crates/shamir-transport-tcp/src/tls.rs#L73); [crates/shamir-transport-tcp/src/tls.rs:85](../../../../../crates/shamir-transport-tcp/src/tls.rs#L85); [crates/shamir-transport-tcp/src/tls.rs:98](../../../../../crates/shamir-transport-tcp/src/tls.rs#L98); [crates/shamir-transport-ws/src/tls_exporter.rs:20](../../../../../crates/shamir-transport-ws/src/tls_exporter.rs#L20).

<a id="review-5-5"></a>

### Claim 5.5 — `tokio` dependency pulls `features = ["full"]` in a library crate

Status: `confirmed-open`. Current risk: `nit`.

Full Tokio features remain enabled. Dependency breadth is proven, binary/compile savings are not measured.

Evidence: [crates/shamir-transport-tcp/Cargo.toml:14](../../../../../crates/shamir-transport-tcp/Cargo.toml#L14).

<a id="review-5-6"></a>

### Claim 5.6 — Transport wire structs and fixtures duplicated across the two e2e test files

Status: `confirmed-open`. Current risk: `nit`.

Both integration files still duplicate handshake wire shapes and KDF/user fixtures.

Evidence: [crates/shamir-transport-tcp/tests/handshake_e2e.rs:41](../../../../../crates/shamir-transport-tcp/tests/handshake_e2e.rs#L41); [crates/shamir-transport-tcp/tests/echo_e2e.rs:57](../../../../../crates/shamir-transport-tcp/tests/echo_e2e.rs#L57).

<a id="review-6-1"></a>

### Claim 6.1 — `write_frame_prereserved` reports every contract violation as `TooLarge`

Status: `confirmed-open`. Current risk: `medium`.

Malformed prefix inputs remain misclassified, the integration test asserts the wrong variant, and TCP forwarding preserves it.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:241](../../../../../crates/shamir-transport-tcp/src/framing.rs#L241); [crates/shamir-transport-tcp/src/framing.rs:250](../../../../../crates/shamir-transport-tcp/src/framing.rs#L250); [crates/shamir-transport-tcp/tests/framing.rs:309](../../../../../crates/shamir-transport-tcp/tests/framing.rs#L309); [crates/shamir-server/src/framer.rs:317](../../../../../crates/shamir-server/src/framer.rs#L317).

<a id="review-6-2"></a>

### Claim 6.2 — `Box<dyn Error + Send + Sync>` in library APIs instead of a `thiserror` enum

Status: `confirmed-open`. Current risk: `medium`.

Boxed TLS constructor errors and the missing-key string remain. Server lifecycle code flattens them, although concrete boxed errors can be downcast.

Evidence: [crates/shamir-transport-tcp/src/tls.rs:30](../../../../../crates/shamir-transport-tcp/src/tls.rs#L30); [crates/shamir-transport-tcp/src/tls.rs:44](../../../../../crates/shamir-transport-tcp/src/tls.rs#L44); [crates/shamir-transport-tcp/src/tls.rs:50](../../../../../crates/shamir-transport-tcp/src/tls.rs#L50); [crates/shamir-server/src/tls.rs:116](../../../../../crates/shamir-server/src/tls.rs#L116).

<a id="review-6-3"></a>

### Claim 6.3 — TLS config builders can panic on missing/ambiguous rustls crypto provider

Status: `confirmed-open`. Current risk: `medium`.

Convenience builders retain ambient-provider panic paths for ambiguity/custom-provider selection or incompatible suites. Pinned rustls automatically installs a unique default; missing explicit installation alone is not a defect under current AWS-LC defaults.

Evidence: [crates/shamir-transport-tcp/src/tls.rs:54](../../../../../crates/shamir-transport-tcp/src/tls.rs#L54); [crates/shamir-transport-tcp/src/tls.rs:64](../../../../../crates/shamir-transport-tcp/src/tls.rs#L64); [crates/shamir-transport-tcp/Cargo.toml:18](../../../../../crates/shamir-transport-tcp/Cargo.toml#L18); [Cargo.lock:3048](../../../../../Cargo.lock#L3048).

<a id="review-6-4"></a>

### Claim 6.4 — Post-error stream state is undocumented (`TooLarge` desyncs the stream)

Status: `confirmed-open`. Current risk: `low`.

Header consumption before TooLarge and potentially partial I/O reads remain undocumented at the transport API. The server discards the connection on error.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:36](../../../../../crates/shamir-transport-tcp/src/framing.rs#L36); [crates/shamir-transport-tcp/src/framing.rs:56](../../../../../crates/shamir-transport-tcp/src/framing.rs#L56); [crates/shamir-server/src/connection/request_loop.rs:283](../../../../../crates/shamir-server/src/connection/request_loop.rs#L283).

<a id="review-6-5"></a>

### Claim 6.5 — Cancel-safety contract lives in the consumer, not the crate

Status: `confirmed-open`. Current risk: `low`.

Read futures still lack a cancellation/stream-discard contract. Server cancellation teardown is documented locally, not in the generic transport API.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:45](../../../../../crates/shamir-transport-tcp/src/framing.rs#L45); [crates/shamir-transport-tcp/src/framing.rs:98](../../../../../crates/shamir-transport-tcp/src/framing.rs#L98); [crates/shamir-server/src/connection/request_loop.rs:231](../../../../../crates/shamir-server/src/connection/request_loop.rs#L231).

<a id="review-7-1"></a>

### Claim 7.1 — Crate-root re-exports are partial, stale, and used by nobody

Status: `confirmed-open`. Current risk: `low`.

Root exports remain partial and unused by repository Rust consumers. This is API consistency debt, not a medium runtime defect or proof of no external consumers.

Evidence: [crates/shamir-transport-tcp/src/lib.rs:12](../../../../../crates/shamir-transport-tcp/src/lib.rs#L12); [crates/shamir-transport-tcp/src/lib.rs:16](../../../../../crates/shamir-transport-tcp/src/lib.rs#L16).

<a id="review-7-2"></a>

### Claim 7.2 — Function-local `use` statements violate the imports-at-top rule (5 sites)

Status: `confirmed-open`. Current risk: `nit`.

All listed local imports remain; the actual count is six statements in four bodies. Placement is convention-only and has no established runtime impact.

Evidence: [crates/shamir-transport-tcp/tests/framing.rs:50](../../../../../crates/shamir-transport-tcp/tests/framing.rs#L50); [crates/shamir-transport-tcp/tests/framing.rs:155](../../../../../crates/shamir-transport-tcp/tests/framing.rs#L155); [crates/shamir-transport-tcp/tests/tls13_only.rs:36](../../../../../crates/shamir-transport-tcp/tests/tls13_only.rs#L36); [crates/shamir-transport-tcp/tests/echo_e2e.rs:208](../../../../../crates/shamir-transport-tcp/tests/echo_e2e.rs#L208).

<a id="review-7-3"></a>

### Claim 7.3 — Speculative dead public consts with cargo-cult `#[allow(dead_code)]`

Status: `confirmed-open`. Current risk: `nit`.

Both public constants and redundant allowances remain without repository consumers; this is public-surface/style noise.

Evidence: [crates/shamir-transport-tcp/src/listener.rs:96](../../../../../crates/shamir-transport-tcp/src/listener.rs#L96).

<a id="review-7-4"></a>

### Claim 7.4 — In-src `tests/` layout covers only one of three modules

Status: `confirmed-open`. Current risk: `low`.

Only listener unit tests are registered. Pure framing tests remain integration tests and no TLS unit error suite exists.

Evidence: [crates/shamir-transport-tcp/src/tests/mod.rs:1](../../../../../crates/shamir-transport-tcp/src/tests/mod.rs#L1); [crates/shamir-transport-tcp/tests/framing.rs:203](../../../../../crates/shamir-transport-tcp/tests/framing.rs#L203).

<a id="review-7-5"></a>

### Claim 7.5 — Spec wire constant duplicated from a direct dependency

Status: `confirmed-open`. Current risk: `low`.

The identical exporter label remains independently defined and exporter context remains public. Current wire values agree; future divergence is maintenance risk.

Evidence: [crates/shamir-transport-tcp/src/tls.rs:18](../../../../../crates/shamir-transport-tcp/src/tls.rs#L18); [crates/shamir-transport-tcp/src/tls.rs:20](../../../../../crates/shamir-transport-tcp/src/tls.rs#L20); [crates/shamir-connect/src/common/domain_tags.rs:35](../../../../../crates/shamir-connect/src/common/domain_tags.rs#L35).

<a id="review-7-6"></a>

### Claim 7.6 — Mixed spellings of the same upstream crate in one import block

Status: `confirmed-open`. Current risk: `nit`.

TLS imports still mix the direct pki-types crate and rustls's re-export; no functional type mismatch is shown.

Evidence: [crates/shamir-transport-tcp/src/tls.rs:10](../../../../../crates/shamir-transport-tcp/src/tls.rs#L10); [crates/shamir-transport-tcp/src/tls.rs:12](../../../../../crates/shamir-transport-tcp/src/tls.rs#L12).

## Current fix-plan state

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 16 | 16 | 0 | 0 | 0 | 0 | 0 |

A source-fixed item closes only its stated mechanism. Partial items retain the obligations named below; proposed fixes must obey the corrections and current contracts, not merely copy the historical recipe.

<a id="plan-p0-1"></a>

### Plan P0.1 — P0.1

Status: `confirmed-open`. Current risk: —.

No uninitialized ReadBuf/spare-capacity implementation, cancellation guard, cancellation documentation or cancellation regression test exists. A Drop guard alone would not repair the initialized-buffer precondition violation.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:124](../../../../../crates/shamir-transport-tcp/src/framing.rs#L124); [crates/shamir-transport-tcp/src/framing.rs:129](../../../../../crates/shamir-transport-tcp/src/framing.rs#L129); [crates/shamir-transport-tcp/tests/framing.rs:189](../../../../../crates/shamir-transport-tcp/tests/framing.rs#L189).

<a id="plan-p0-2"></a>

### Plan P0.2 — P0.2

Status: `confirmed-open`. Current risk: —.

No empty-data error or rejection exists in any writer; the false empty-data documentation remains and no empty-writer regression test is present.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:8](../../../../../crates/shamir-transport-tcp/src/framing.rs#L8); [crates/shamir-transport-tcp/src/framing.rs:153](../../../../../crates/shamir-transport-tcp/src/framing.rs#L153); [crates/shamir-transport-tcp/src/framing.rs:192](../../../../../crates/shamir-transport-tcp/src/framing.rs#L192); [crates/shamir-transport-tcp/src/framing.rs:250](../../../../../crates/shamir-transport-tcp/src/framing.rs#L250).

<a id="plan-p0-3"></a>

### Plan P0.3 — P0.3

Status: `confirmed-open`. Current risk: —.

TLS constructors remain boxed/infallible and use ambient-provider convenience builders. Explicit fallible provider configuration is absent; treating absence of a preinstalled default as an error would unnecessarily reject rustls's valid automatic-provider case.

Evidence: [crates/shamir-transport-tcp/src/tls.rs:30](../../../../../crates/shamir-transport-tcp/src/tls.rs#L30); [crates/shamir-transport-tcp/src/tls.rs:44](../../../../../crates/shamir-transport-tcp/src/tls.rs#L44); [crates/shamir-transport-tcp/src/tls.rs:54](../../../../../crates/shamir-transport-tcp/src/tls.rs#L54); [crates/shamir-transport-tcp/src/tls.rs:63](../../../../../crates/shamir-transport-tcp/src/tls.rs#L63).

<a id="plan-p1-4"></a>

### Plan P1.4 — P1.4

Status: `confirmed-open`. Current risk: —.

No staged allocation, max_prealloc or buffered-byte deployment contract has been added. The practical 16 MiB production threat requires post-authentication access; pre-authentication reads already use 4 KiB.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:67](../../../../../crates/shamir-transport-tcp/src/framing.rs#L67); [crates/shamir-transport-tcp/src/framing.rs:124](../../../../../crates/shamir-transport-tcp/src/framing.rs#L124); [crates/shamir-server/src/connection/handshake.rs:714](../../../../../crates/shamir-server/src/connection/handshake.rs#L714).

<a id="plan-p1-5"></a>

### Plan P1.5 — P1.5

Status: `confirmed-open`. Current risk: —.

No shrink/hysteresis wrapper or large-growth reclamation test exists. Apply any policy to genuinely persistent buffers, particularly the client reader, rather than assuming server requests reuse scratch.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:89](../../../../../crates/shamir-transport-tcp/src/framing.rs#L89); [crates/shamir-transport-tcp/src/framing.rs:199](../../../../../crates/shamir-transport-tcp/src/framing.rs#L199); [crates/shamir-client/src/client.rs:312](../../../../../crates/shamir-client/src/client.rs#L312); [crates/shamir-server/src/connection/request_loop.rs:278](../../../../../crates/shamir-server/src/connection/request_loop.rs#L278).

<a id="plan-p1-6"></a>

### Plan P1.6 — P1.6

Status: `confirmed-open`. Current risk: —.

Malformed-prefix variants and updated assertions are absent; server default prechecks and TCP error forwarding remain.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:241](../../../../../crates/shamir-transport-tcp/src/framing.rs#L241); [crates/shamir-transport-tcp/tests/framing.rs:309](../../../../../crates/shamir-transport-tcp/tests/framing.rs#L309); [crates/shamir-server/src/framer.rs:118](../../../../../crates/shamir-server/src/framer.rs#L118); [crates/shamir-server/src/framer.rs:317](../../../../../crates/shamir-server/src/framer.rs#L317).

<a id="plan-p1-7"></a>

### Plan P1.7 — P1.7

Status: `confirmed-open`. Current risk: —.

TLS error tests, writer oversize tests, exact EOF error-kind assertions, mapped-address pin and real unspecified TLS bind coverage remain absent. Existing EOF cleanup coverage is not new remediation.

Evidence: [crates/shamir-transport-tcp/src/tests/mod.rs:1](../../../../../crates/shamir-transport-tcp/src/tests/mod.rs#L1); [crates/shamir-transport-tcp/tests/framing.rs:226](../../../../../crates/shamir-transport-tcp/tests/framing.rs#L226); [crates/shamir-transport-tcp/src/tests/listener_tests.rs:101](../../../../../crates/shamir-transport-tcp/src/tests/listener_tests.rs#L101).

<a id="plan-p1-8"></a>

### Plan P1.8 — P1.8

Status: `confirmed-open`. Current risk: —.

CertificateVerify still returns an assertion and no explicit security-obligations section or enforcing variant exists. Optional certificate/SPKI pinning must not be confused with the protocol's separate Ed25519 identity pin.

Evidence: [crates/shamir-transport-tcp/src/tls.rs:60](../../../../../crates/shamir-transport-tcp/src/tls.rs#L60); [crates/shamir-transport-tcp/src/tls.rs:158](../../../../../crates/shamir-transport-tcp/src/tls.rs#L158); [crates/shamir-client/src/client.rs:486](../../../../../crates/shamir-client/src/client.rs#L486).

<a id="plan-p1-9"></a>

### Plan P1.9 — P1.9

Status: `confirmed-open`. Current risk: —.

Exporter extraction still returns Option and erases errors; no fatal-on-failure contract exists. Fixed output length is already documented. Consumer follow-through must remove server zero fallbacks, not merely change e2e expect calls.

Evidence: [crates/shamir-transport-tcp/src/tls.rs:71](../../../../../crates/shamir-transport-tcp/src/tls.rs#L71); [crates/shamir-transport-tcp/src/tls.rs:77](../../../../../crates/shamir-transport-tcp/src/tls.rs#L77); [crates/shamir-server/src/server/server_launcher.rs:1163](../../../../../crates/shamir-server/src/server/server_launcher.rs#L1163).

<a id="plan-p1-10"></a>

### Plan P1.10 — P1.10

Status: `confirmed-open`. Current risk: —.

Both public read contracts still omit cancellation and post-error stream-discard guidance. Describe possible partial consumption rather than assert that every error necessarily desynchronizes.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:36](../../../../../crates/shamir-transport-tcp/src/framing.rs#L36); [crates/shamir-transport-tcp/src/framing.rs:72](../../../../../crates/shamir-transport-tcp/src/framing.rs#L72); [crates/shamir-server/src/connection/request_loop.rs:231](../../../../../crates/shamir-server/src/connection/request_loop.rs#L231).

<a id="plan-p2-11"></a>

### Plan P2.11 — P2.11

Status: `confirmed-open`. Current risk: —.

Writer limit parameters/FrameLimits and a shared exported loopback predicate are absent; the server copy remains. Future negotiated limits should not be presented as an existing protocol feature.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:153](../../../../../crates/shamir-transport-tcp/src/framing.rs#L153); [crates/shamir-transport-tcp/src/framing.rs:250](../../../../../crates/shamir-transport-tcp/src/framing.rs#L250); [crates/shamir-transport-tcp/src/listener.rs:51](../../../../../crates/shamir-transport-tcp/src/listener.rs#L51); [crates/shamir-server/src/config.rs:877](../../../../../crates/shamir-server/src/config.rs#L877).

<a id="plan-p2-12"></a>

### Plan P2.12 — P2.12

Status: `confirmed-open`. Current risk: —.

Root exports remain partial and test wiring interleaved. Prefer completing/clarifying the public surface unless removal's compatibility impact is accepted.

Evidence: [crates/shamir-transport-tcp/src/lib.rs:12](../../../../../crates/shamir-transport-tcp/src/lib.rs#L12); [crates/shamir-transport-tcp/src/lib.rs:13](../../../../../crates/shamir-transport-tcp/src/lib.rs#L13); [crates/shamir-transport-tcp/src/lib.rs:16](../../../../../crates/shamir-transport-tcp/src/lib.rs#L16).

<a id="plan-p2-13"></a>

### Plan P2.13 — P2.13

Status: `confirmed-open`. Current risk: —.

The listed function-local imports remain. Correct the task count to six use statements in four bodies; this is style-only cleanup.

Evidence: [crates/shamir-transport-tcp/tests/framing.rs:50](../../../../../crates/shamir-transport-tcp/tests/framing.rs#L50); [crates/shamir-transport-tcp/tests/framing.rs:155](../../../../../crates/shamir-transport-tcp/tests/framing.rs#L155); [crates/shamir-transport-tcp/tests/tls13_only.rs:36](../../../../../crates/shamir-transport-tcp/tests/tls13_only.rs#L36); [crates/shamir-transport-tcp/tests/echo_e2e.rs:208](../../../../../crates/shamir-transport-tcp/tests/echo_e2e.rs#L208).

<a id="plan-p2-14"></a>

### Plan P2.14 — P2.14

Status: `confirmed-open`. Current risk: —.

No bootstrap/spawn_blocking documentation was added and both transport e2e KDF calls remain inline on current-thread runtimes.

Evidence: [crates/shamir-transport-tcp/src/tls.rs:22](../../../../../crates/shamir-transport-tcp/src/tls.rs#L22); [crates/shamir-transport-tcp/src/tls.rs:40](../../../../../crates/shamir-transport-tcp/src/tls.rs#L40); [crates/shamir-transport-tcp/tests/handshake_e2e.rs:278](../../../../../crates/shamir-transport-tcp/tests/handshake_e2e.rs#L278); [crates/shamir-transport-tcp/tests/echo_e2e.rs:376](../../../../../crates/shamir-transport-tcp/tests/echo_e2e.rs#L376).

<a id="plan-p2-15"></a>

### Plan P2.15 — P2.15

Status: `confirmed-open`. Current risk: —.

Exporter constants remain duplicated/public, both loopback constants remain, and source test layout still registers listener_tests only. All bundled cleanup items are outstanding.

Evidence: [crates/shamir-transport-tcp/src/tls.rs:18](../../../../../crates/shamir-transport-tcp/src/tls.rs#L18); [crates/shamir-transport-tcp/src/tls.rs:20](../../../../../crates/shamir-transport-tcp/src/tls.rs#L20); [crates/shamir-transport-tcp/src/listener.rs:96](../../../../../crates/shamir-transport-tcp/src/listener.rs#L96); [crates/shamir-transport-tcp/src/tests/mod.rs:1](../../../../../crates/shamir-transport-tcp/src/tests/mod.rs#L1).

<a id="plan-p2-16"></a>

### Plan P2.16 — P2.16

Status: `confirmed-open`. Current risk: —.

No BufReader guidance, pooled client writer migration, UDS qualification, exporter-doc correction, Tokio feature trim, shared e2e fixture or import-spelling unification has landed. The proposed syscall/latency payoff remains unverified.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:36](../../../../../crates/shamir-transport-tcp/src/framing.rs#L36); [crates/shamir-client/src/client.rs:1278](../../../../../crates/shamir-client/src/client.rs#L1278); [crates/shamir-transport-tcp/src/listener.rs:30](../../../../../crates/shamir-transport-tcp/src/listener.rs#L30); [crates/shamir-transport-tcp/src/tls.rs:73](../../../../../crates/shamir-transport-tcp/src/tls.rs#L73); [crates/shamir-transport-tcp/Cargo.toml:14](../../../../../crates/shamir-transport-tcp/Cargo.toml#L14); [crates/shamir-transport-tcp/tests/handshake_e2e.rs:41](../../../../../crates/shamir-transport-tcp/tests/handshake_e2e.rs#L41); [crates/shamir-transport-tcp/tests/echo_e2e.rs:57](../../../../../crates/shamir-transport-tcp/tests/echo_e2e.rs#L57).

## Corrections and qualified non-findings

- Keep the unsafe pooled-read headline, but remove UB on every call and unqualified remote-disclosure language. Fresh/grown uninitialized storage is the mechanism; production cancellation paths discard their buffers.
- Historical Miri-test and commit-completion wording is not fix proof. The reviewed Miri-named tests cover successful reads and error cleanup, not cancellation or arbitrary legal AsyncRead observation.
- The provider panic is conditional. Pinned rustls auto-installs an unambiguous provider; current manifests/defaults select AWS-LC. Test install_default calls do not prove missing installation must panic.
- All current exporter callers expect is false: production server TCP/native-WS paths use unwrap_or zeros, while the Rust client returns an error. Their calls follow successful TLS handshakes, so fallback-trigger reachability is not established for normal pinned TLS 1.3.
- The fixed 32-byte exporter contract is already documented; constant binding does not by itself enable recorded-proof replay because fresh nonces are included. It weakens channel separation/relay resistance.
- Qualify 16 MiB amplification as accepted-length capacity allocation, primarily post-authentication in this server. Do not assert measured RSS or ignore pre-authentication limits, connection caps and read deadlines.
- Refute the universal server lifetime-retention example: request buffers are freshly allocated and owned by dispatch tasks, and replies are prereserved owned buffers. Persistent client reader retention remains open.
- Two read operations do not establish a universal syscall ratio or latency penalty. Single write_all does not prove one underlying write/TLS record. Bench setup recreates pooled scratch rather than retaining it across iterations.
- Existing truncated EOF cleanup coverage must be acknowledged; the missing exact error-kind oracle remains. Default lib-only test execution excludes the framing/TLS integration files.
- No explicit locks in this crate is source-supported, not a transitive lock-free guarantee. SessionStore::len uses DashMap shard locks/counts, not banned O(N) scc traversal.
- Tokio-rustls streams genuinely implement ConnectionExporter through the local impls; the WS helper reuses them. The actual doc defects are the named generic trait, standalone rustls-connection claim and version-decoupling rationale.
- Recalibrate root-export consistency to low and local-import/dead-constant cleanup to nit. There are six local use statements across four bodies, not five.
- Boxed errors allow downcasting; server string flattening is a caller decision. Typed public variants remain absent.
- Post-error stream documentation should say partial consumption may occur and require conservative discard, rather than claim every Err necessarily desynchronizes.
- Original 49 lens rows and 30 synthesis entries are historical coverage counts, not counts of currently confirmed unresolved defects after corrections.
- Fix Plan item 1 incorrectly references item 9 for cancel-safety documentation; the stream-contract item is 10.

## Current follow-up order

1. P0.1: repair pooled-read initialization soundness and cancellation-buffer safety; add registered cancellation and adversarial-reader oracles.
2. P0.2: reserve zero-length wire output for explicit close and pin all three data-writer rejections.
3. P0.3 and P1.7: add typed, explicitly fallible TLS configuration and genuine error-path coverage; preserve valid automatic-provider behavior.
4. P1.9: retain exporter error diagnostics and fail closed in server consumers instead of substituting zeros.
5. P1.4–P1.5: bound declared-length allocation and address genuinely persistent client capacity; do not base prioritization on unmeasured RSS totals.
6. P1.6, P1.8 and P1.10: correct malformed-prefix diagnostics, CertificateVerify/security obligations and partial-read stream contracts.
7. P2.16: remove source-proven per-request client framing allocation; measure plain-TCP buffering benefits before asserting syscall/latency gains.
8. Remaining API consistency, fixture deduplication, test layout, import placement and documentation cleanup are lower-priority maintenance work.

## Coverage and limitations

- All 49 original findings, 30 consolidated findings and 16 numbered Fix Plan items are addressed; no TASK_GROUPS file exists in the assigned directory.
- Read-only inspection only: no files changed, worktrees, agents, builds, tests, Miri, benchmarks or reproductions. Historical execution/pass claims remain independently unverified.
- Runtime latency, syscall ratios, resident-memory totals and exploitability were not measured. Structural facts and conditional threat models are distinguished below.
- Pinned upstream source was inspected read-only for rustls 0.23.37, Tokio 1.49.0 and DashMap 6.1.0; evidence references below remain repository-relative.
- HEAD remained the requested revision with no relevant working-tree differences. Existing-claim revalidation is not a fresh whole-repository audit.

## Reviewed document inventory

- [correctness-tdd.md](./correctness-tdd.md) — 9 claim decisions; 0 explicit plan items.
- [concurrency-lockfree.md](./concurrency-lockfree.md) — 2 claim decisions; 0 explicit plan items.
- [security-crypto.md](./security-crypto.md) — 5 claim decisions; 0 explicit plan items.
- [performance-hotpath.md](./performance-hotpath.md) — 4 claim decisions; 0 explicit plan items.
- [api-wire-protocol.md](./api-wire-protocol.md) — 12 claim decisions; 0 explicit plan items.
- [error-handling-lifecycle.md](./error-handling-lifecycle.md) — 9 claim decisions; 0 explicit plan items.
- [style-claude-md.md](./style-claude-md.md) — 8 claim decisions; 0 explicit plan items.
- [SUMMARY.md](./SUMMARY.md) — 30 claim decisions; 16 explicit plan items.

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
